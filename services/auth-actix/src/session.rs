use std::time::Duration;

use actix_web::cookie::{Cookie, SameSite, time};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const COOKIE_NAME: &str = "auth_token";
const MAX_TOKEN_BYTES: usize = 4_096;
const MAX_CLOCK_SKEW_SECONDS: u64 = 60;

type HmacSha256 = Hmac<Sha256>;

pub(crate) struct SessionCodec {
    secret: Vec<u8>,
    ttl: Duration,
    secure_cookie: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenHeader {
    alg: String,
    typ: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionClaims {
    authenticated: bool,
    iat: u64,
    exp: u64,
    /// The signed-in user's id, when the session belongs to a managed account.
    ///
    /// Every identity field is optional and defaulted so a token minted before managed users existed
    /// still verifies. Those sessions carry no identity and resolve to the shared-password principal,
    /// which is correct: that is what they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// `admin`, `operator` or `viewer`.
    ///
    /// Carried in the token rather than looked up per request. The alternative is a state round trip
    /// on every authorization, which is on the path of every dashboard request. The cost is that a
    /// role change takes effect when the session is renewed rather than instantly, which is why
    /// disabling an account is the control that bites immediately: it is checked at sign-in, and a
    /// disabled user cannot mint a new token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

/// Who a verified session belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    /// `None` for a session created by the shared password, which has no account behind it.
    pub user_id: Option<String>,
    pub display_name: Option<String>,
    /// Absent on a shared-password session. Callers treat that as `admin`, because that session is
    /// the legacy full-access one and downgrading it would lock an un-migrated install out of its own
    /// settings.
    pub role: Option<String>,
}

/// The identity to mint a token for.
#[derive(Debug, Clone, Default)]
pub struct NewSession {
    pub user_id: Option<String>,
    pub display_name: Option<String>,
    pub role: Option<String>,
}

impl SessionCodec {
    pub(crate) fn new(secret: &[u8], ttl: Duration, secure_cookie: bool) -> Self {
        Self {
            secret: secret.to_vec(),
            ttl,
            secure_cookie,
        }
    }

    /// Mint a token for the shared-password principal, which carries no identity.
    pub(crate) fn create_token(&self, now: u64) -> Option<String> {
        self.create_token_for(now, &NewSession::default())
    }

    /// Mint a token carrying who signed in.
    pub(crate) fn create_token_for(&self, now: u64, session: &NewSession) -> Option<String> {
        let exp = now.checked_add(self.ttl.as_secs())?;
        let header = serde_json::to_vec(&TokenHeader {
            alg: "HS256".to_owned(),
            typ: "JWT".to_owned(),
        })
        .ok()?;
        let claims = serde_json::to_vec(&SessionClaims {
            authenticated: true,
            iat: now,
            exp,
            sub: session.user_id.clone(),
            name: session.display_name.clone(),
            role: session.role.clone(),
        })
        .ok()?;
        let header = URL_SAFE_NO_PAD.encode(header);
        let claims = URL_SAFE_NO_PAD.encode(claims);
        let signing_input = format!("{header}.{claims}");
        let mut mac = HmacSha256::new_from_slice(&self.secret).ok()?;
        mac.update(signing_input.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        Some(format!("{signing_input}.{signature}"))
    }

    /// Whether the token is valid. Thin wrapper over [`Self::identity`] so callers that do not care
    /// who signed in read the same way they did before identity existed.
    pub(crate) fn verify(&self, token: &str, now: u64) -> bool {
        self.identity(token, now).is_some()
    }

    /// The identity a valid token carries, or `None` when it does not verify.
    ///
    /// Every check that made `verify` return false still does: signature, algorithm, expiry, and the
    /// skew bounds. Identity is read only after all of them pass, so an unsigned claim cannot name a
    /// role.
    pub(crate) fn identity(&self, token: &str, now: u64) -> Option<SessionIdentity> {
        let claims = self.verified_claims(token, now)?;
        Some(SessionIdentity {
            user_id: claims.sub,
            display_name: claims.name,
            role: claims.role,
        })
    }

    fn verified_claims(&self, token: &str, now: u64) -> Option<SessionClaims> {
        if !self.signature_and_bounds_hold(token, now) {
            return None;
        }
        let claims_b64 = token.split('.').nth(1)?;
        let claims = URL_SAFE_NO_PAD.decode(claims_b64).ok()?;
        serde_json::from_slice::<SessionClaims>(&claims).ok()
    }

    fn signature_and_bounds_hold(&self, token: &str, now: u64) -> bool {
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return false;
        }
        let mut parts = token.split('.');
        let (Some(header), Some(claims), Some(signature)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        if parts.next().is_some() {
            return false;
        }
        let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(&self.secret) else {
            return false;
        };
        let signing_input = format!("{header}.{claims}");
        mac.update(signing_input.as_bytes());
        if mac.verify_slice(&signature).is_err() {
            return false;
        }
        let Ok(header) = URL_SAFE_NO_PAD.decode(header) else {
            return false;
        };
        let Ok(header) = serde_json::from_slice::<TokenHeader>(&header) else {
            return false;
        };
        if header.alg != "HS256" || header.typ != "JWT" {
            return false;
        }
        let Ok(claims) = URL_SAFE_NO_PAD.decode(claims) else {
            return false;
        };
        let Ok(claims) = serde_json::from_slice::<SessionClaims>(&claims) else {
            return false;
        };
        let latest_iat = now.saturating_add(MAX_CLOCK_SKEW_SECONDS);
        claims.authenticated
            && claims.iat <= latest_iat
            && claims.exp > now
            && claims.exp > claims.iat
            && claims.exp.saturating_sub(claims.iat) <= self.ttl.as_secs()
    }

    pub(crate) fn session_cookie(&self, token: String) -> Cookie<'static> {
        Cookie::build(COOKIE_NAME, token)
            .http_only(true)
            .secure(self.secure_cookie)
            .same_site(SameSite::Lax)
            .path("/")
            .max_age(time::Duration::seconds(ttl_seconds(self.ttl)))
            .finish()
    }

    pub(crate) fn clear_cookie(&self) -> Cookie<'static> {
        Cookie::build(COOKIE_NAME, "")
            .http_only(true)
            .secure(self.secure_cookie)
            .same_site(SameSite::Lax)
            .path("/")
            .max_age(time::Duration::ZERO)
            .expires(time::OffsetDateTime::UNIX_EPOCH)
            .finish()
    }

    pub(crate) const fn cookie_name() -> &'static str {
        COOKIE_NAME
    }
}

fn ttl_seconds(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX)
}
