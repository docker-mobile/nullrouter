// Each integration-test binary compiles this module separately, so a helper used by one binary is
// dead code in every other. The warning is an artefact of that, not a signal.
#![allow(dead_code, reason = "shared across test binaries; each uses a subset")]

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use nullrouter_auth::{
    ApiKeyValidator, AuthConfig, AuthService, Clock, LockoutConfig, PasswordConfig,
};

pub(crate) const PASSWORD: &str = "g017-test-password";

pub(crate) const fn peer(octet: u8) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, octet)), 41000)
}

#[derive(Debug)]
pub(crate) struct ManualClock {
    pub(crate) now: AtomicU64,
}

impl ManualClock {
    pub(crate) const fn new(now: u64) -> Self {
        Self {
            now: AtomicU64::new(now),
        }
    }
}

impl Clock for ManualClock {
    fn now_seconds(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

pub(crate) fn service(
    clock: Arc<ManualClock>,
    validator: Arc<dyn ApiKeyValidator>,
    session_ttl: Duration,
    lockout: LockoutConfig,
) -> Result<AuthService, Box<dyn std::error::Error>> {
    let config = AuthConfig::new(
        b"g017-test-session-secret-32-bytes-minimum".to_vec(),
        PasswordConfig::Plaintext(PASSWORD.to_owned()),
    )?
    .with_session_ttl(session_ttl)
    .with_secure_cookie(true)
    .with_lockout(lockout)
    .with_state_validation_url("http://127.0.0.1:9/internal/v1/keys/validate")?;

    Ok(AuthService::with_dependencies(config, clock, validator)?)
}

/// A service whose cookie is not `Secure`, so a test client over plaintext keeps it.
///
/// `service` sets `with_secure_cookie(true)`, which is right for the cases asserting cookie flags
/// and wrong for anything that needs the cookie to come back on a second request.
pub(crate) fn default_service(
    clock: Arc<ManualClock>,
    validator: Arc<dyn ApiKeyValidator>,
    session_ttl: Duration,
    lockout: LockoutConfig,
) -> Result<AuthService, Box<dyn std::error::Error>> {
    let config = AuthConfig::new(
        b"g017-test-session-secret-32-bytes-minimum".to_vec(),
        PasswordConfig::Plaintext(PASSWORD.to_owned()),
    )?
    .with_session_ttl(session_ttl)
    .with_lockout(lockout)
    .with_state_validation_url("http://127.0.0.1:9/internal/v1/keys/validate")?;

    Ok(AuthService::with_dependencies(config, clock, validator)?)
}

pub(crate) const fn default_lockout() -> LockoutConfig {
    LockoutConfig {
        threshold: 5,
        window: Duration::from_secs(60),
        lock_duration: Duration::from_secs(120),
        capacity: 64,
    }
}

pub(crate) fn extract_cookie(
    response: &actix_web::dev::ServiceResponse,
) -> Result<String, Box<dyn std::error::Error>> {
    let value = response
        .headers()
        .get(actix_web::http::header::SET_COOKIE)
        .ok_or_else(|| std::io::Error::other("missing Set-Cookie"))?
        .to_str()?;
    let pair = value
        .split(';')
        .next()
        .ok_or_else(|| std::io::Error::other("missing cookie pair"))?;
    Ok(pair.to_owned())
}
