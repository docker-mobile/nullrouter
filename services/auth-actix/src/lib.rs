mod clock;
mod config;
pub mod contracts;
pub mod errors;
pub mod lockout;
pub mod oidc;
pub mod password;
pub mod responses;
pub mod routes;
pub mod saml;
pub mod session;
pub mod settings_client;
mod sso_routes;
pub mod state_client;

use std::sync::{Arc, Mutex};

pub use clock::{Clock, SystemClock};
pub use config::{AuthConfig, AuthConfigError, LockoutConfig, PasswordConfig};
pub use settings_client::{AuthSettings, AuthSettingsProvider, SettingsError};
pub use state_client::{ApiKeyValidation, ApiKeyValidator, StateValidationError};

use lockout::LockoutStore;
use oidc::OidcHttp;
use password::PasswordVerifier;
use session::SessionCodec;
use settings_client::HttpAuthSettingsProvider;
use state_client::HttpApiKeyValidator;

pub const SERVICE_NAME: &str = "nullrouter-auth";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 20135;

#[derive(Clone)]
pub struct AuthService {
    inner: Arc<AuthServiceInner>,
}

struct AuthServiceInner {
    config: AuthConfig,
    clock: Arc<dyn Clock>,
    password: PasswordVerifier,
    session: SessionCodec,
    lockout: Mutex<LockoutStore>,
    key_validator: ValidatorBackend,
    settings: SettingsBackend,
    oidc_http: Option<OidcHttp>,
}

enum ValidatorBackend {
    Http(HttpApiKeyValidator),
    Custom(Arc<dyn ApiKeyValidator>),
}

enum SettingsBackend {
    Http(HttpAuthSettingsProvider),
    Custom(Arc<dyn AuthSettingsProvider>),
}

impl AuthService {
    pub fn from_config(config: AuthConfig) -> Result<Self, AuthConfigError> {
        config.validate()?;
        let validator = HttpApiKeyValidator::new(
            config.state_validation_url().clone(),
            config.state_timeout(),
        )?;
        let settings = HttpAuthSettingsProvider::new(
            config.state_auth_settings_url().clone(),
            config.state_timeout(),
        )?;
        Self::new_inner(
            config,
            Arc::new(SystemClock),
            ValidatorBackend::Http(validator),
            SettingsBackend::Http(settings),
        )
    }

    pub fn with_dependencies(
        config: AuthConfig,
        clock: Arc<dyn Clock>,
        key_validator: Arc<dyn ApiKeyValidator>,
    ) -> Result<Self, AuthConfigError> {
        let settings = HttpAuthSettingsProvider::new(
            config.state_auth_settings_url().clone(),
            config.state_timeout(),
        )?;
        Self::new_inner(
            config,
            clock,
            ValidatorBackend::Custom(key_validator),
            SettingsBackend::Http(settings),
        )
    }

    /// Build a service with a caller-supplied settings source.
    ///
    /// Lets a test drive the OIDC and SAML routes without a running state
    /// service, which is what makes the "callback rejects a bad state" and
    /// "metadata contains the configured issuer" cases testable at all.
    pub fn with_settings_provider(
        config: AuthConfig,
        clock: Arc<dyn Clock>,
        key_validator: Arc<dyn ApiKeyValidator>,
        settings: Arc<dyn AuthSettingsProvider>,
    ) -> Result<Self, AuthConfigError> {
        Self::new_inner(
            config,
            clock,
            ValidatorBackend::Custom(key_validator),
            SettingsBackend::Custom(settings),
        )
    }

    fn new_inner(
        config: AuthConfig,
        clock: Arc<dyn Clock>,
        key_validator: ValidatorBackend,
        settings: SettingsBackend,
    ) -> Result<Self, AuthConfigError> {
        config.validate()?;
        Ok(Self {
            inner: Arc::new(AuthServiceInner {
                password: PasswordVerifier::new(config.password().clone()),
                session: SessionCodec::new(
                    config.session_secret(),
                    config.session_ttl(),
                    config.secure_cookie(),
                ),
                lockout: Mutex::new(LockoutStore::new(config.lockout().clone())),
                oidc_http: OidcHttp::new(config.oidc_timeout()).ok(),
                config,
                clock,
                key_validator,
                settings,
            }),
        })
    }

    pub(crate) fn config(&self) -> &AuthConfig {
        &self.inner.config
    }

    pub(crate) fn now(&self) -> u64 {
        self.inner.clock.now_seconds()
    }

    pub(crate) fn password(&self) -> &PasswordVerifier {
        &self.inner.password
    }

    pub(crate) fn session(&self) -> &SessionCodec {
        &self.inner.session
    }

    pub(crate) fn lockout(&self) -> &Mutex<LockoutStore> {
        &self.inner.lockout
    }

    pub(crate) async fn validate_api_key(
        &self,
        api_key: &str,
    ) -> Result<ApiKeyValidation, StateValidationError> {
        match &self.inner.key_validator {
            ValidatorBackend::Http(validator) => validator.validate(api_key).await,
            ValidatorBackend::Custom(validator) => validator.validate(api_key).await,
        }
    }

    /// The stored SSO configuration, secrets included.
    pub(crate) async fn auth_settings(&self) -> Result<AuthSettings, SettingsError> {
        match &self.inner.settings {
            SettingsBackend::Http(provider) => provider.settings().await,
            SettingsBackend::Custom(provider) => provider.settings().await,
        }
    }

    /// The OIDC HTTP client, absent only when it could not be constructed.
    pub(crate) fn oidc_http(&self) -> Option<&OidcHttp> {
        self.inner.oidc_http.as_ref()
    }
}

pub fn configure(service: AuthService) -> impl FnOnce(&mut actix_web::web::ServiceConfig) {
    move |config| routes::configure(config, service)
}
