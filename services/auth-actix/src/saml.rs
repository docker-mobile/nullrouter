//! SAML 2.0 service-provider support for dashboard sign-in.
//!
//! Ports what can be ported of `inspire/src/lib/auth/saml.js`, which delegates to
//! `@node-saml/node-saml`.
//!
//! # What is implemented, and what is refused
//!
//! Metadata generation and the outbound `AuthnRequest` are complete: both are
//! XML this service builds from settings, with no cryptography involved.
//!
//! Consuming an assertion is **not**. Trusting a `SAMLResponse` requires
//! verifying its XML digital signature, which means exclusive XML
//! canonicalisation (C14N) of the `SignedInfo` and of the signed element,
//! digesting the canonical form, and checking that digest's signature against the
//! IdP certificate. C14N is not available to this service, and a subtly wrong
//! implementation of it is an authentication bypass — an attacker who can shape
//! the XML around a legitimately signed fragment gets a session.
//!
//! So [`consume_response`] validates everything it can — configuration present,
//! response decodable, `InResponseTo` matching the request this browser started —
//! and then returns [`SamlError::VerificationUnavailable`] instead of a profile.
//! It never returns a profile for an unverified assertion. There is deliberately
//! no code path in this module that produces a session from a `SAMLResponse`.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::{Compression, write::DeflateEncoder};
use rand::random;
use std::io::Write as _;
use thiserror::Error;

use crate::{
    oidc::{normalize_base64, trim_trailing_slashes, wrap_base64},
    settings_client::AuthSettings,
};

pub(crate) const STATE_COOKIE: &str = "saml_state";

const DEFAULT_ISSUER: &str = "urn:9router:sp";
/// Cap on a decoded `SAMLResponse`. Assertions are a few kilobytes; anything
/// this large is not one.
const MAX_RESPONSE_BYTES: usize = 512 * 1_024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SamlError {
    #[error("SAML is not configured")]
    NotConfigured,
    #[error("the assertion POST carried no SAMLResponse")]
    MissingResponse,
    #[error("the SAMLResponse is not valid base64")]
    NotBase64,
    #[error("the SAMLResponse is too large")]
    TooLarge,
    #[error("InResponseTo mismatch: expected {expected}, received {received}")]
    RequestIdMismatch { expected: String, received: String },
    /// The refusal that keeps this module honest. See the module docs.
    #[error(
        "SAML assertion verification is not available in this build: verifying an assertion \
         requires XML signature verification (XML-DSig with exclusive canonicalisation), which \
         this service cannot perform. The assertion was NOT accepted."
    )]
    VerificationUnavailable,
    #[error("the router's own state service is unavailable")]
    StateUnavailable,
}

impl SamlError {
    /// The `?error=` code for a `/login` redirect.
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "saml_not_configured",
            Self::MissingResponse => "saml_missing_response",
            Self::NotBase64 => "saml_malformed_response",
            Self::TooLarge => "saml_response_too_large",
            Self::RequestIdMismatch { .. } => "saml_request_id_mismatch",
            Self::VerificationUnavailable => "saml_verification_unavailable",
            Self::StateUnavailable => "saml_state_unavailable",
        }
    }
}

/// A usable SAML configuration.
///
/// Mirrors upstream's `isSamlConfigured`: an entry point and a certificate are
/// both required. The certificate is required even though this build cannot
/// verify with it — reporting "configured" without one would misdescribe the
/// install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamlConfig {
    pub entry_point: String,
    pub issuer: String,
    pub certificate: String,
    pub attribute_email: String,
    pub attribute_name: String,
}

impl SamlConfig {
    pub fn from_settings(settings: &AuthSettings) -> Option<Self> {
        let entry_point = settings.saml_entry_point.trim();
        let certificate = format_x509_certificate(&settings.saml_cert);
        if entry_point.is_empty() || certificate.is_empty() {
            return None;
        }
        Some(Self {
            entry_point: entry_point.to_owned(),
            issuer: normalize_issuer(&settings.saml_issuer),
            certificate,
            attribute_email: settings.saml_attribute_email.trim().to_owned(),
            attribute_name: settings.saml_attribute_name.trim().to_owned(),
        })
    }
}

/// The SP entity ID, defaulting as upstream does.
pub fn normalize_issuer(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        DEFAULT_ISSUER.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Normalise a certificate to PEM, from PEM or bare base64.
///
/// Ports upstream `formatX509Certificate`. Empty when the input carries no
/// base64 at all, or when what it carries is not decodable — an undecodable
/// certificate is not a certificate, and reporting it as one would make the
/// install look configured when it is not.
pub fn format_x509_certificate(value: &str) -> String {
    let body = value
        .replace("-----BEGIN CERTIFICATE-----", "")
        .replace("-----END CERTIFICATE-----", "");
    let normalized = normalize_base64(&body);
    if normalized.is_empty() {
        return String::new();
    }
    let lines = wrap_base64(&normalized).join("\n");
    format!("-----BEGIN CERTIFICATE-----\n{lines}\n-----END CERTIFICATE-----")
}

/// The certificate body, without PEM armour or newlines, for embedding in XML.
fn certificate_body(certificate: &str) -> String {
    certificate
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect()
}

/// The ACS URL for an origin.
pub fn acs_url(origin: &str) -> String {
    format!("{}/api/auth/saml/acs", trim_trailing_slashes(origin))
}

/// SP metadata XML, built from settings.
///
/// Advertises the HTTP-POST assertion consumer at [`acs_url`] and, when a
/// certificate is configured, publishes it as the signing key. Deliberately does
/// **not** claim `WantAssertionsSigned="false"`: this SP wants signed
/// assertions, it simply cannot verify them yet.
pub fn metadata_xml(origin: &str, config: &SamlConfig) -> String {
    let entity_id = escape_xml(&config.issuer);
    let acs = escape_xml(&acs_url(origin));
    let certificate = certificate_body(&config.certificate);
    let key_descriptor = if certificate.is_empty() {
        String::new()
    } else {
        format!(
            "\n    <KeyDescriptor use=\"signing\">\
             \n      <ds:KeyInfo xmlns:ds=\"http://www.w3.org/2000/09/xmldsig#\">\
             \n        <ds:X509Data>\
             \n          <ds:X509Certificate>{certificate}</ds:X509Certificate>\
             \n        </ds:X509Data>\
             \n      </ds:KeyInfo>\
             \n    </KeyDescriptor>"
        )
    };

    let protocol = "urn:oasis:names:tc:SAML:2.0:protocol";
    let post_binding = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST";
    let name_id_format = "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress";

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <EntityDescriptor xmlns=\"urn:oasis:names:tc:SAML:2.0:metadata\" \
         entityID=\"{entity_id}\">\n  \
         <SPSSODescriptor AuthnRequestsSigned=\"false\" WantAssertionsSigned=\"true\" \
         protocolSupportEnumeration=\"{protocol}\">{key_descriptor}\n    \
         <NameIDFormat>{name_id_format}</NameIDFormat>\n    \
         <AssertionConsumerService index=\"1\" isDefault=\"true\" Binding=\"{post_binding}\" \
         Location=\"{acs}\"/>\n  \
         </SPSSODescriptor>\n\
         </EntityDescriptor>\n"
    )
}

/// An `AuthnRequest` and the id to remember for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthnRequest {
    /// Where to send the browser, with the request in `?SAMLRequest=`.
    pub redirect_url: String,
    /// The request `ID`, stored in the `saml_state` cookie and checked against
    /// the assertion's `InResponseTo`.
    pub request_id: String,
}

/// A SAML request id: `_` plus hex, since an `ID` must be an XML `NCName` and so
/// cannot start with a digit.
pub fn create_request_id() -> String {
    let bytes = random::<[u8; 16]>();
    let mut id = String::with_capacity(33);
    id.push('_');
    for byte in bytes {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

/// Build an `AuthnRequest` for the HTTP-Redirect binding.
///
/// The request is deflated (raw, no zlib header) and base64-encoded per
/// §3.4.4.1 of the SAML bindings spec. Unsigned, which matches the
/// `AuthnRequestsSigned="false"` this SP publishes in its metadata.
pub fn authn_request(
    origin: &str,
    config: &SamlConfig,
    issue_instant: &str,
) -> Option<AuthnRequest> {
    let request_id = create_request_id();
    let xml = format!(
        "<samlp:AuthnRequest xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\" \
         xmlns:saml=\"urn:oasis:names:tc:SAML:2.0:assertion\" ID=\"{id}\" Version=\"2.0\" \
         IssueInstant=\"{instant}\" ProtocolBinding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST\" \
         AssertionConsumerServiceURL=\"{acs}\" Destination=\"{destination}\">\
         <saml:Issuer>{issuer}</saml:Issuer>\
         </samlp:AuthnRequest>",
        id = escape_xml(&request_id),
        instant = escape_xml(issue_instant),
        acs = escape_xml(&acs_url(origin)),
        destination = escape_xml(&config.entry_point),
        issuer = escape_xml(&config.issuer),
    );

    let deflated = deflate(xml.as_bytes())?;
    let encoded = STANDARD.encode(deflated);
    let mut url = reqwest::Url::parse(&config.entry_point).ok()?;
    url.query_pairs_mut().append_pair("SAMLRequest", &encoded);
    Some(AuthnRequest {
        redirect_url: url.to_string(),
        request_id,
    })
}

fn deflate(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).ok()?;
    encoder.finish().ok()
}

/// Everything [`consume_response`] needs to judge an assertion POST.
#[derive(Debug, Clone)]
pub struct AssertionPost<'a> {
    /// The raw `SAMLResponse` form field.
    pub saml_response: Option<&'a str>,
    /// The `saml_state` cookie value, empty when absent.
    pub expected_request_id: &'a str,
}

/// Run every check that is possible, then refuse.
///
/// The return type is `Result<Never-ish, SamlError>` in spirit: the success arm
/// does not exist, because this build cannot establish that an assertion is
/// authentic. The checks still run first so an operator's `/login?error=` tells
/// them what is actually wrong with their setup — a missing certificate and an
/// unverifiable signature are different problems — rather than masking a
/// misconfiguration behind the blanket refusal.
pub fn consume_response(
    post: &AssertionPost<'_>,
    config: Option<&SamlConfig>,
) -> Result<std::convert::Infallible, SamlError> {
    let Some(raw) = post.saml_response.filter(|value| !value.is_empty()) else {
        return Err(SamlError::MissingResponse);
    };
    if config.is_none() {
        return Err(SamlError::NotConfigured);
    }
    if raw.len() > MAX_RESPONSE_BYTES {
        return Err(SamlError::TooLarge);
    }
    let decoded = STANDARD
        .decode(raw.as_bytes())
        .map_err(|_| SamlError::NotBase64)?;
    if decoded.len() > MAX_RESPONSE_BYTES {
        return Err(SamlError::TooLarge);
    }
    let xml = String::from_utf8_lossy(&decoded);

    // Replay protection, as upstream does it: the assertion must answer the
    // request this browser started. Checked before the refusal so a replayed or
    // unsolicited response is named as such.
    if !post.expected_request_id.is_empty() {
        let received = in_response_to(&xml).unwrap_or_default();
        if received != post.expected_request_id {
            return Err(SamlError::RequestIdMismatch {
                expected: post.expected_request_id.to_owned(),
                received: if received.is_empty() {
                    "none".to_owned()
                } else {
                    received
                },
            });
        }
    }

    // The signature would be verified here. It cannot be, so the assertion is
    // rejected. Do not replace this with a profile built from the XML above:
    // every value in it is attacker-controlled until a signature says otherwise.
    Err(SamlError::VerificationUnavailable)
}

/// Read `InResponseTo` out of a response document.
///
/// A deliberately narrow scan rather than an XML parse: it is only used to
/// reject a mismatch, never to establish anything positive.
fn in_response_to(xml: &str) -> Option<String> {
    let start = xml.find("InResponseTo")?;
    let rest = xml.get(start.saturating_add("InResponseTo".len())..)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next().filter(|ch| *ch == '"' || *ch == '\'')?;
    let value = rest.get(1..)?;
    let end = value.find(quote)?;
    value.get(..end).map(str::to_owned)
}

/// Escape text for an XML attribute or element body.
pub fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
