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
}

impl SessionCodec {
    pub(crate) fn new(secret: &[u8], ttl: Duration, secure_cookie: bool) -> Self {
        Self {
            secret: secret.to_vec(),
            ttl,
            secure_cookie,
        }
    }

    pub(crate) fn create_token(&self, now: u64) -> Option<String> {
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

    pub(crate) fn verify(&self, token: &str, now: u64) -> bool {
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
