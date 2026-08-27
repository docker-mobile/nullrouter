use std::{env, net::IpAddr, time::Duration};

use rand::random;
use reqwest::Url;
use thiserror::Error;

const MIN_SESSION_SECRET_BYTES: usize = 32;
const DEFAULT_STATE_VALIDATION_URL: &str = "http://127.0.0.1:20134/internal/v1/keys/validate";
const DEFAULT_STATE_AUTH_SETTINGS_URL: &str = "http://127.0.0.1:20134/internal/v1/auth-settings";
/// Fallback public origin, used to build the OIDC `redirect_uri` and the SAML ACS
/// URL when the request carries no usable host. Matches the dashboard's port.
const DEFAULT_PUBLIC_ORIGIN: &str = "http://localhost:20128";

#[derive(Clone)]
pub enum PasswordConfig {
    BcryptHash(String),
    Plaintext(String),
}

#[derive(Debug, Clone)]
pub struct LockoutConfig {
    pub threshold: u32,
    pub window: Duration,
    pub lock_duration: Duration,
    pub capacity: usize,
}

impl Default for LockoutConfig {
    fn default() -> Self {
        Self {
            threshold: 5,
            window: Duration::from_secs(15 * 60),
            lock_duration: Duration::from_secs(15 * 60),
            capacity: 4_096,
        }
    }
}

#[derive(Clone)]
pub struct AuthConfig {
    session_secret: Vec<u8>,
    password: PasswordConfig,
    session_ttl: Duration,
    secure_cookie: bool,
    lockout: LockoutConfig,
    state_validation_url: Url,
    state_auth_settings_url: Url,
    state_timeout: Duration,
    /// Timeout for calls out to an identity provider. Longer than the loopback
    /// state timeout, since discovery and the token exchange cross the internet.
    oidc_timeout: Duration,
    /// Explicit public origin, when the deployment knows it. Overrides whatever
    /// the request's `Host`/`X-Forwarded-*` headers claim, which matters because
    /// the OIDC `redirect_uri` must match what the provider has registered.
    public_origin: Option<String>,
}

#[derive(Debug, Error)]
pub enum AuthConfigError {
    #[error("session secret must contain at least 32 bytes")]
    SessionSecretTooShort,
    #[error("session TTL must be greater than zero")]
    InvalidSessionTtl,
    #[error("lockout configuration must have positive limits and durations")]
    InvalidLockout,
    #[error("state validation URL is invalid")]
    InvalidStateValidationUrl,
    #[error("state validation URL must use loopback HTTP")]
    NonLoopbackStateValidationUrl,
    #[error("invalid environment value for {0}")]
    InvalidEnvironment(&'static str),
    #[error("state HTTP client could not be created")]
    StateClient,
}

impl AuthConfig {
    pub fn new(session_secret: Vec<u8>, password: PasswordConfig) -> Result<Self, AuthConfigError> {
        let state_validation_url = parse_state_url(DEFAULT_STATE_VALIDATION_URL)?;
        let state_auth_settings_url = parse_state_url(DEFAULT_STATE_AUTH_SETTINGS_URL)?;
        let config = Self {
            session_secret,
            password,
            session_ttl: Duration::from_secs(24 * 60 * 60),
            secure_cookie: false,
            lockout: LockoutConfig::default(),
            state_validation_url,
            state_auth_settings_url,
            state_timeout: Duration::from_secs(2),
            oidc_timeout: Duration::from_secs(10),
            public_origin: None,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn from_env() -> Result<Self, AuthConfigError> {
        let session_secret = match env::var("NULLROUTER_AUTH_SESSION_SECRET") {
            Ok(secret) => secret.into_bytes(),
            Err(env::VarError::NotPresent) => random::<[u8; 32]>().to_vec(),
            Err(env::VarError::NotUnicode(_)) => {
                return Err(AuthConfigError::InvalidEnvironment(
                    "NULLROUTER_AUTH_SESSION_SECRET",
                ));
            }
        };
        let password = match env::var("NULLROUTER_AUTH_PASSWORD_HASH") {
            Ok(hash) if !hash.trim().is_empty() => PasswordConfig::BcryptHash(hash),
            Ok(_) | Err(env::VarError::NotPresent) => PasswordConfig::Plaintext(
                env::var("INITIAL_PASSWORD").unwrap_or_else(|_| "123456".to_owned()),
            ),
            Err(env::VarError::NotUnicode(_)) => {
                return Err(AuthConfigError::InvalidEnvironment(
                    "NULLROUTER_AUTH_PASSWORD_HASH",
                ));
            }
        };
        let mut config = Self::new(session_secret, password)?;
        config.secure_cookie = env_bool("AUTH_COOKIE_SECURE", false)?;
        config.session_ttl = Duration::from_secs(env_u64(
            "NULLROUTER_AUTH_SESSION_TTL_SECONDS",
            24 * 60 * 60,
        )?);
        config.lockout = LockoutConfig {
            threshold: env_u32("NULLROUTER_AUTH_LOCKOUT_THRESHOLD", 5)?,
            window: Duration::from_secs(env_u64(
                "NULLROUTER_AUTH_LOCKOUT_WINDOW_SECONDS",
                15 * 60,
            )?),
            lock_duration: Duration::from_secs(env_u64(
                "NULLROUTER_AUTH_LOCKOUT_DURATION_SECONDS",
                15 * 60,
            )?),
            capacity: env_usize("NULLROUTER_AUTH_LOCKOUT_CAPACITY", 4_096)?,
        };
        config.state_timeout =
            Duration::from_secs(env_u64("NULLROUTER_AUTH_STATE_TIMEOUT_SECONDS", 2)?);
        config.oidc_timeout =
            Duration::from_secs(env_u64("NULLROUTER_AUTH_OIDC_TIMEOUT_SECONDS", 10)?);
        if let Ok(url) = env::var("NULLROUTER_STATE_VALIDATE_URL") {
            config.state_validation_url = parse_state_url(&url)?;
        }
        if let Ok(url) = env::var("NULLROUTER_STATE_AUTH_SETTINGS_URL") {
            config.state_auth_settings_url = parse_state_url(&url)?;
        }
        // Upstream reads BASE_URL for the same purpose.
        config.public_origin = ["NULLROUTER_PUBLIC_ORIGIN", "BASE_URL"]
            .into_iter()
            .filter_map(|name| env::var(name).ok())
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .find(|value| !value.is_empty());
        config.validate()?;
        Ok(config)
    }

    #[must_use]
    pub const fn with_session_ttl(mut self, session_ttl: Duration) -> Self {
        self.session_ttl = session_ttl;
        self
    }

    #[must_use]
    pub const fn with_secure_cookie(mut self, secure_cookie: bool) -> Self {
        self.secure_cookie = secure_cookie;
        self
    }

    #[must_use]
    pub const fn with_lockout(mut self, lockout: LockoutConfig) -> Self {
        self.lockout = lockout;
        self
    }

    pub fn with_state_validation_url(mut self, url: &str) -> Result<Self, AuthConfigError> {
        self.state_validation_url = parse_state_url(url)?;
        Ok(self)
    }

    pub fn with_state_auth_settings_url(mut self, url: &str) -> Result<Self, AuthConfigError> {
        self.state_auth_settings_url = parse_state_url(url)?;
        Ok(self)
    }

    #[must_use]
    pub fn with_public_origin(mut self, origin: &str) -> Self {
        let origin = origin.trim().trim_end_matches('/');
        self.public_origin = (!origin.is_empty()).then(|| origin.to_owned());
        self
    }

    #[must_use]
    pub const fn with_oidc_timeout(mut self, timeout: Duration) -> Self {
        self.oidc_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn with_state_timeout(mut self, timeout: Duration) -> Self {
        self.state_timeout = timeout;
        self
    }

    pub(crate) const fn validate(&self) -> Result<(), AuthConfigError> {
        if self.session_secret.len() < MIN_SESSION_SECRET_BYTES {
            return Err(AuthConfigError::SessionSecretTooShort);
        }
        if self.session_ttl.is_zero() {
            return Err(AuthConfigError::InvalidSessionTtl);
        }
        if self.lockout.threshold == 0
            || self.lockout.window.is_zero()
            || self.lockout.lock_duration.is_zero()
            || self.lockout.capacity == 0
            || self.state_timeout.is_zero()
            || self.oidc_timeout.is_zero()
        {
            return Err(AuthConfigError::InvalidLockout);
        }
        Ok(())
    }

    pub(crate) fn session_secret(&self) -> &[u8] {
        &self.session_secret
    }

    pub(crate) const fn password(&self) -> &PasswordConfig {
        &self.password
    }

    pub(crate) const fn session_ttl(&self) -> Duration {
        self.session_ttl
    }

    pub(crate) const fn secure_cookie(&self) -> bool {
        self.secure_cookie
    }

    pub(crate) const fn lockout(&self) -> &LockoutConfig {
        &self.lockout
    }

    pub(crate) const fn state_validation_url(&self) -> &Url {
        &self.state_validation_url
    }

    pub(crate) const fn state_auth_settings_url(&self) -> &Url {
        &self.state_auth_settings_url
    }

    pub(crate) const fn state_timeout(&self) -> Duration {
        self.state_timeout
    }

    pub(crate) const fn oidc_timeout(&self) -> Duration {
        self.oidc_timeout
    }

    pub(crate) fn configured_public_origin(&self) -> Option<&str> {
        self.public_origin.as_deref()
    }

    /// The origin used to build a `redirect_uri` or an ACS URL.
    ///
    /// Prefers the configured origin, then what the request reports, then a
    /// loopback default. Never returns an empty string, so a redirect is always
    /// built against something absolute.
    pub(crate) fn public_origin_or(&self, from_request: Option<&str>) -> String {
        self.configured_public_origin()
            .map(str::to_owned)
            .or_else(|| {
                from_request
                    .map(|origin| origin.trim().trim_end_matches('/').to_owned())
                    .filter(|origin| !origin.is_empty())
            })
            .unwrap_or_else(|| DEFAULT_PUBLIC_ORIGIN.to_owned())
    }

    pub(crate) const fn has_configured_password_hash(&self) -> bool {
        matches!(&self.password, PasswordConfig::BcryptHash(_))
    }
}

fn parse_state_url(value: &str) -> Result<Url, AuthConfigError> {
    let url = Url::parse(value).map_err(|_| AuthConfigError::InvalidStateValidationUrl)?;
    if url.scheme() != "http" {
        return Err(AuthConfigError::NonLoopbackStateValidationUrl);
    }
    let Some(host) = url.host_str() else {
        return Err(AuthConfigError::InvalidStateValidationUrl);
    };
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !is_loopback {
        return Err(AuthConfigError::NonLoopbackStateValidationUrl);
    }
    Ok(url)
}

fn env_bool(name: &'static str, default: bool) -> Result<bool, AuthConfigError> {
    match env::var(name) {
        Ok(value) if value.eq_ignore_ascii_case("true") || value == "1" => Ok(true),
        Ok(value) if value.eq_ignore_ascii_case("false") || value == "0" => Ok(false),
        Ok(_) | Err(env::VarError::NotUnicode(_)) => Err(AuthConfigError::InvalidEnvironment(name)),
        Err(env::VarError::NotPresent) => Ok(default),
    }
}

fn env_u64(name: &'static str, default: u64) -> Result<u64, AuthConfigError> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|_| AuthConfigError::InvalidEnvironment(name)),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(_)) => Err(AuthConfigError::InvalidEnvironment(name)),
    }
}

fn env_u32(name: &'static str, default: u32) -> Result<u32, AuthConfigError> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u32>()
            .map_err(|_| AuthConfigError::InvalidEnvironment(name)),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(_)) => Err(AuthConfigError::InvalidEnvironment(name)),
    }
}

fn env_usize(name: &'static str, default: usize) -> Result<usize, AuthConfigError> {
    match env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|_| AuthConfigError::InvalidEnvironment(name)),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(_)) => Err(AuthConfigError::InvalidEnvironment(name)),
    }
}
