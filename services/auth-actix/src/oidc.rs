//! OIDC authorization-code flow with PKCE, for dashboard sign-in.
//!
//! Ports `inspire/src/lib/auth/oidc.js` and the `/api/auth/oidc/*` routes.
//!
//! The one rule this module exists to enforce: a dashboard session is minted
//! only after an `id_token` has been verified against the provider's published
//! JWKS. Every failure path returns an error and redirects to `/login`; none of
//! them falls through to a session. If the signing algorithm is one this module
//! cannot verify, that is a failure too — never a skipped check.

use std::{collections::BTreeMap, time::Duration};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use rand::random;
use reqwest::{Client, Url};
use rsa::{
    BigUint, RsaPublicKey,
    pkcs1v15::{Signature, VerifyingKey},
    signature::Verifier as _,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::settings_client::AuthSettings;

pub(crate) const STATE_COOKIE: &str = "oidc_state";
pub(crate) const NONCE_COOKIE: &str = "oidc_nonce";
pub(crate) const VERIFIER_COOKIE: &str = "oidc_code_verifier";

const DEFAULT_SCOPES: &str = "openid profile email";
const DEFAULT_LOGIN_LABEL: &str = "Sign in with OIDC";
/// Matches upstream's `acceptedClockSkewMs` for SAML and is the usual allowance
/// for `exp`/`iat` on an ID token.
const CLOCK_SKEW_SECONDS: u64 = 60;
/// Discovery and JWKS documents are small; a provider streaming more than this
/// is misconfigured or hostile.
const MAX_METADATA_BYTES: usize = 256 * 1_024;

/// Why an OIDC step could not be completed.
///
/// Each variant maps to a stable `?error=` code on the `/login` redirect, so the
/// login page can say something specific without the message being a place for
/// provider text to leak into a URL.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OidcError {
    #[error("OIDC is not configured")]
    NotConfigured,
    #[error("the OIDC state parameter did not match the stored state")]
    InvalidState,
    #[error("the callback did not carry a code and state")]
    MissingCode,
    #[error("the OIDC discovery document could not be loaded: {0}")]
    Discovery(String),
    #[error("the OIDC token exchange failed: {0}")]
    TokenExchange(String),
    #[error("the OIDC provider did not return an id_token")]
    MissingIdToken,
    #[error("the id_token could not be verified: {0}")]
    IdToken(String),
    #[error("the router's own state service is unavailable")]
    StateUnavailable,
}

impl OidcError {
    /// The `?error=` code for a `/login` redirect.
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "oidc_not_configured",
            Self::InvalidState => "oidc_invalid_state",
            Self::MissingCode => "oidc_missing_code",
            Self::Discovery(_) => "oidc_discovery_failed",
            Self::TokenExchange(_) => "oidc_token_exchange_failed",
            Self::MissingIdToken => "oidc_missing_id_token",
            Self::IdToken(_) => "oidc_id_token_invalid",
            Self::StateUnavailable => "oidc_state_unavailable",
        }
    }
}

/// A usable OIDC configuration.
///
/// Only constructible from settings that carry an issuer, a client id, and a
/// client secret, so holding one of these means the flow can actually run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scopes: String,
    pub login_label: String,
}

impl OidcConfig {
    /// Build a config, or `None` when OIDC is not configured.
    ///
    /// Mirrors upstream's `isOidcConfigured`: issuer, client id, and client
    /// secret must all be non-empty after trimming.
    pub fn from_settings(settings: &AuthSettings) -> Option<Self> {
        let issuer_url = trim_trailing_slashes(&settings.oidc_issuer_url);
        let client_id = settings.oidc_client_id.trim();
        let client_secret = settings.oidc_client_secret.trim();
        if issuer_url.is_empty() || client_id.is_empty() || client_secret.is_empty() {
            return None;
        }
        Some(Self {
            issuer_url,
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            scopes: normalize_scopes(&settings.oidc_scopes),
            login_label: normalize_login_label(&settings.oidc_login_label),
        })
    }
}

/// Strip trailing slashes, so `https://idp/` and `https://idp` build the same
/// discovery URL.
pub fn trim_trailing_slashes(value: &str) -> String {
    value.trim().trim_end_matches('/').to_owned()
}

pub fn normalize_scopes(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        DEFAULT_SCOPES.to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub fn normalize_login_label(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        DEFAULT_LOGIN_LABEL.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The subset of a discovery document this flow uses.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Discovery {
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub authorization_endpoint: String,
    #[serde(default)]
    pub token_endpoint: String,
    #[serde(default)]
    pub jwks_uri: String,
}

/// The discovery URL for an issuer.
pub fn discovery_url(issuer_url: &str) -> String {
    format!(
        "{}/.well-known/openid-configuration",
        trim_trailing_slashes(issuer_url)
    )
}

/// A PKCE verifier and its S256 challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

/// Fresh PKCE material.
pub fn create_pkce_pair() -> PkcePair {
    let verifier = URL_SAFE_NO_PAD.encode(random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    PkcePair {
        verifier,
        challenge,
    }
}

/// A random `state` or `nonce`.
pub fn create_random_token() -> String {
    URL_SAFE_NO_PAD.encode(random::<[u8; 16]>())
}

/// Inputs for the authorization redirect.
#[derive(Debug, Clone)]
pub struct AuthorizationRequest<'a> {
    pub authorization_endpoint: &'a str,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub scopes: &'a str,
    pub state: &'a str,
    pub nonce: &'a str,
    pub code_challenge: &'a str,
}

/// Build the provider's authorization URL.
///
/// `None` when the endpoint is not a URL, rather than a redirect to something
/// that is not one.
pub fn authorization_url(request: &AuthorizationRequest<'_>) -> Option<String> {
    let mut url = Url::parse(request.authorization_endpoint).ok()?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", request.client_id)
        .append_pair("redirect_uri", request.redirect_uri)
        .append_pair("scope", &normalize_scopes(request.scopes))
        .append_pair("state", request.state)
        .append_pair("nonce", request.nonce)
        .append_pair("code_challenge", request.code_challenge)
        .append_pair("code_challenge_method", "S256");
    Some(url.to_string())
}

/// A token-endpoint response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenResponse {
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// The verified claims a session is built from.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct IdTokenClaims {
    #[serde(default)]
    pub sub: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub iss: Option<String>,
    #[serde(default)]
    pub aud: Option<Audience>,
    #[serde(default)]
    pub exp: Option<u64>,
    #[serde(default)]
    pub iat: Option<u64>,
}

/// `aud` is a string or an array of strings, per RFC 7519.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, expected: &str) -> bool {
        match self {
            Self::One(value) => value == expected,
            Self::Many(values) => values.iter().any(|value| value == expected),
        }
    }
}

/// Upstream `pickOidcDisplayName`.
pub fn pick_display_name(claims: &IdTokenClaims) -> String {
    [
        claims.preferred_username.as_deref(),
        claims.email.as_deref(),
        claims.name.as_deref(),
        claims.given_name.as_deref(),
        claims.sub.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|value| !value.is_empty())
    .unwrap_or("OIDC user")
    .to_owned()
}

/// Upstream `pickOidcEmail`.
pub fn pick_email(claims: &IdTokenClaims) -> String {
    claims.email.clone().unwrap_or_default()
}

/// A JSON Web Key Set, narrowed to the RSA signing keys this module can use.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct JwkSet {
    #[serde(default)]
    pub keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    #[serde(default)]
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
}

/// A decoded JWS header.
#[derive(Debug, Clone, Default, Deserialize)]
struct JwtHeader {
    #[serde(default)]
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

/// What an ID token must be checked against.
#[derive(Debug, Clone)]
pub struct IdTokenExpectations<'a> {
    pub issuer: &'a str,
    pub audience: &'a str,
    pub nonce: &'a str,
    pub now_seconds: u64,
}

/// Verify an `id_token`: signature first, then claims.
///
/// Signature verification is RS256 against the published JWKS. RS256 is the only
/// algorithm accepted, and `alg: none` or any other value is rejected — a token
/// whose signature this module cannot check is a failure, not a token to trust.
/// A `kid` in the header selects the key; without one, every RSA key is tried, so
/// a provider that omits `kid` still works while an unrelated key still fails.
pub fn verify_id_token(
    token: &str,
    jwks: &JwkSet,
    expectations: &IdTokenExpectations<'_>,
) -> Result<IdTokenClaims, OidcError> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(claims_b64), Some(signature_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(OidcError::IdToken(
            "id_token is not a three-part JWS".to_owned(),
        ));
    };

    let header: JwtHeader = decode_json(header_b64)
        .ok_or_else(|| OidcError::IdToken("id_token header is not JSON".to_owned()))?;
    if header.alg != "RS256" {
        return Err(OidcError::IdToken(format!(
            "id_token is signed with {}, but only RS256 can be verified here",
            if header.alg.is_empty() {
                "no algorithm"
            } else {
                header.alg.as_str()
            }
        )));
    }

    let signature = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|_| OidcError::IdToken("id_token signature is not base64url".to_owned()))?;
    let signing_input = format!("{header_b64}.{claims_b64}");

    let candidates: Vec<&Jwk> = jwks
        .keys
        .iter()
        .filter(|key| key.kty == "RSA")
        .filter(|key| key.alg.as_deref().is_none_or(|alg| alg == "RS256"))
        .filter(|key| match (&header.kid, &key.kid) {
            // A `kid` on both sides must match; otherwise every RSA key is a
            // candidate, and the signature check is what decides.
            (Some(wanted), Some(available)) => wanted == available,
            _ => true,
        })
        .collect();
    if candidates.is_empty() {
        return Err(OidcError::IdToken(
            "no usable RSA signing key in the provider's JWKS".to_owned(),
        ));
    }
    let verified = candidates.iter().any(|key| {
        rsa_public_key(key).is_some_and(|public_key| {
            Signature::try_from(signature.as_slice()).is_ok_and(|signature| {
                VerifyingKey::<Sha256>::new(public_key)
                    .verify(signing_input.as_bytes(), &signature)
                    .is_ok()
            })
        })
    });
    if !verified {
        return Err(OidcError::IdToken(
            "id_token signature does not match the provider's JWKS".to_owned(),
        ));
    }

    let claims: IdTokenClaims = decode_json(claims_b64)
        .ok_or_else(|| OidcError::IdToken("id_token claims are not JSON".to_owned()))?;
    check_claims(&claims, expectations)?;
    Ok(claims)
}

/// Claim checks, run only after the signature verified.
fn check_claims(
    claims: &IdTokenClaims,
    expectations: &IdTokenExpectations<'_>,
) -> Result<(), OidcError> {
    if claims.iss.as_deref().map(trim_trailing_slashes)
        != Some(trim_trailing_slashes(expectations.issuer))
    {
        return Err(OidcError::IdToken("id_token issuer mismatch".to_owned()));
    }
    if !claims
        .aud
        .as_ref()
        .is_some_and(|aud| aud.contains(expectations.audience))
    {
        return Err(OidcError::IdToken(
            "id_token audience does not include this client".to_owned(),
        ));
    }
    // The nonce ties the token to the browser that started the flow, so an
    // absent nonce is a mismatch rather than a skipped check.
    if claims.nonce.as_deref() != Some(expectations.nonce) {
        return Err(OidcError::IdToken("id_token nonce mismatch".to_owned()));
    }
    let Some(exp) = claims.exp else {
        return Err(OidcError::IdToken("id_token has no exp".to_owned()));
    };
    if exp.saturating_add(CLOCK_SKEW_SECONDS) <= expectations.now_seconds {
        return Err(OidcError::IdToken("id_token has expired".to_owned()));
    }
    if claims
        .iat
        .is_some_and(|iat| iat > expectations.now_seconds.saturating_add(CLOCK_SKEW_SECONDS))
    {
        return Err(OidcError::IdToken(
            "id_token was issued in the future".to_owned(),
        ));
    }
    Ok(())
}

/// Rebuild an RSA public key from a JWK's `n` and `e`.
fn rsa_public_key(key: &Jwk) -> Option<RsaPublicKey> {
    let modulus = URL_SAFE_NO_PAD.decode(key.n.as_deref()?).ok()?;
    let exponent = URL_SAFE_NO_PAD.decode(key.e.as_deref()?).ok()?;
    RsaPublicKey::new(
        BigUint::from_bytes_be(&modulus),
        BigUint::from_bytes_be(&exponent),
    )
    .ok()
}

fn decode_json<T: serde::de::DeserializeOwned>(segment: &str) -> Option<T> {
    let bytes = URL_SAFE_NO_PAD.decode(segment).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Format a certificate or key body as PEM lines of 64 characters.
///
/// Shared with the SAML module, which needs the same wrapping.
pub(crate) fn wrap_base64(value: &str) -> Vec<String> {
    value
        .as_bytes()
        .chunks(64)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

/// Re-encode base64 in the standard alphabet, dropping anything that is not
/// base64. Used by the SAML certificate normaliser.
pub(crate) fn normalize_base64(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '='))
        .collect();
    STANDARD
        .decode(cleaned.as_bytes())
        .map_or_else(|_| String::new(), |bytes| STANDARD.encode(bytes))
}

/// HTTP calls to the provider.
///
/// Separated from the pure logic above so routes can be tested against a stub
/// provider while this half is exercised against a real endpoint.
#[derive(Debug, Clone)]
pub struct OidcHttp {
    client: Client,
}

impl OidcHttp {
    pub fn new(timeout: Duration) -> Result<Self, crate::AuthConfigError> {
        Ok(Self {
            client: Client::builder()
                .timeout(timeout)
                .build()
                .map_err(|_| crate::AuthConfigError::StateClient)?,
        })
    }

    /// Fetch and parse a discovery document.
    pub async fn discovery(&self, issuer_url: &str) -> Result<Discovery, OidcError> {
        let url = discovery_url(issuer_url);
        let response = self.client.get(&url).send().await.map_err(|error| {
            OidcError::Discovery(format!("{url} could not be reached: {error}"))
        })?;
        if !response.status().is_success() {
            return Err(OidcError::Discovery(format!(
                "{url} responded with {}",
                response.status().as_u16()
            )));
        }
        let bytes = self.bounded_body(response).await?;
        let discovery: Discovery = serde_json::from_slice(&bytes)
            .map_err(|_| OidcError::Discovery(format!("{url} did not return a JSON document")))?;
        if discovery.token_endpoint.is_empty() {
            return Err(OidcError::Discovery(format!(
                "{url} does not advertise a token_endpoint"
            )));
        }
        Ok(discovery)
    }

    /// Fetch a JWKS.
    pub async fn jwks(&self, jwks_uri: &str) -> Result<JwkSet, OidcError> {
        if jwks_uri.is_empty() {
            return Err(OidcError::IdToken(
                "the provider's discovery document has no jwks_uri, so no signature can be verified"
                    .to_owned(),
            ));
        }
        let response = self.client.get(jwks_uri).send().await.map_err(|error| {
            OidcError::IdToken(format!("{jwks_uri} could not be reached: {error}"))
        })?;
        if !response.status().is_success() {
            return Err(OidcError::IdToken(format!(
                "{jwks_uri} responded with {}",
                response.status().as_u16()
            )));
        }
        let bytes = self
            .bounded_body(response)
            .await
            .map_err(|_| OidcError::IdToken(format!("{jwks_uri} returned an oversized body")))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| OidcError::IdToken(format!("{jwks_uri} did not return a JWKS")))
    }

    /// Exchange an authorization code for tokens.
    pub async fn exchange_code(
        &self,
        request: &TokenExchange<'_>,
    ) -> Result<TokenResponse, OidcError> {
        let mut form: BTreeMap<&str, &str> = BTreeMap::new();
        form.insert("grant_type", "authorization_code");
        form.insert("client_id", request.client_id);
        form.insert("code", request.code);
        form.insert("redirect_uri", request.redirect_uri);
        form.insert("code_verifier", request.code_verifier);
        if !request.client_secret.is_empty() {
            form.insert("client_secret", request.client_secret);
        }

        let response = self
            .client
            .post(request.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|error| {
                OidcError::TokenExchange(format!(
                    "{} could not be reached: {error}",
                    request.token_endpoint
                ))
            })?;
        let status = response.status();
        let bytes = self.bounded_body(response).await.map_err(|_| {
            OidcError::TokenExchange("the token endpoint returned an oversized body".to_owned())
        })?;
        let parsed: TokenResponse = serde_json::from_slice(&bytes).unwrap_or_default();
        if !status.is_success() {
            let detail = parsed
                .error_description
                .or(parsed.error)
                .unwrap_or_else(|| format!("token endpoint responded with {}", status.as_u16()));
            return Err(OidcError::TokenExchange(detail));
        }
        Ok(parsed)
    }

    async fn bounded_body(&self, response: reqwest::Response) -> Result<Vec<u8>, OidcError> {
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(MAX_METADATA_BYTES).unwrap_or(u64::MAX))
        {
            return Err(OidcError::Discovery("response is too large".to_owned()));
        }
        let bytes = response.bytes().await.map_err(|error| {
            OidcError::Discovery(format!("response could not be read: {error}"))
        })?;
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(OidcError::Discovery("response is too large".to_owned()));
        }
        Ok(bytes.to_vec())
    }
}

/// Inputs for a token exchange.
#[derive(Debug, Clone)]
pub struct TokenExchange<'a> {
    pub token_endpoint: &'a str,
    pub client_id: &'a str,
    pub client_secret: &'a str,
    pub code: &'a str,
    pub redirect_uri: &'a str,
    pub code_verifier: &'a str,
}
