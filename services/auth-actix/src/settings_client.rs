//! Reads the dashboard SSO configuration from `nullrouter-state`.
//!
//! This is the one place in `nullrouter-auth` that holds the OIDC client secret
//! and the SAML signing certificate in the clear. They arrive over the
//! loopback-only `/internal/v1/auth-settings` route, which is the reason the
//! public `GET /api/settings` can report `oidcClientSecretSet` instead of the
//! secret itself.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use thiserror::Error;

use crate::AuthConfigError;

/// Cap on the settings response. Generous next to the largest field (a PEM
/// certificate) and still small enough that a wrong endpoint cannot stream.
const MAX_SETTINGS_RESPONSE_BYTES: usize = 64 * 1_024;

/// The SSO configuration, as stored.
///
/// Untrimmed and unnormalised: `oidc` and `saml` decide what an empty field
/// means, so this type stays a faithful copy of what the operator saved.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthSettings {
    #[serde(default)]
    pub oidc_issuer_url: String,
    #[serde(default)]
    pub oidc_client_id: String,
    #[serde(default)]
    pub oidc_client_secret: String,
    #[serde(default)]
    pub oidc_scopes: String,
    #[serde(default)]
    pub oidc_login_label: String,
    #[serde(default)]
    pub saml_entry_point: String,
    #[serde(default)]
    pub saml_issuer: String,
    #[serde(default)]
    pub saml_cert: String,
    #[serde(default)]
    pub saml_attribute_email: String,
    #[serde(default)]
    pub saml_attribute_name: String,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    #[error("state settings service is unavailable")]
    Unavailable,
    #[error("state settings response is invalid")]
    InvalidResponse,
}

/// Source of the SSO configuration.
///
/// A trait so tests can supply settings without a running state service. There
/// is no default implementation on purpose: a provider that silently returned
/// empty settings would turn "state is down" into "SSO is not configured", and
/// those two must not look alike to a caller.
#[async_trait]
pub trait AuthSettingsProvider: Send + Sync {
    async fn settings(&self) -> Result<AuthSettings, SettingsError>;
}

pub(crate) struct HttpAuthSettingsProvider {
    client: Client,
    endpoint: Url,
}

impl HttpAuthSettingsProvider {
    pub(crate) fn new(endpoint: Url, timeout: Duration) -> Result<Self, AuthConfigError> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| AuthConfigError::StateClient)?;
        Ok(Self { client, endpoint })
    }
}

#[async_trait]
impl AuthSettingsProvider for HttpAuthSettingsProvider {
    async fn settings(&self) -> Result<AuthSettings, SettingsError> {
        let response = self
            .client
            .get(self.endpoint.clone())
            .send()
            .await
            .map_err(|_| SettingsError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(SettingsError::Unavailable);
        }
        if response.content_length().is_some_and(|length| {
            length > u64::try_from(MAX_SETTINGS_RESPONSE_BYTES).unwrap_or(u64::MAX)
        }) {
            return Err(SettingsError::InvalidResponse);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| SettingsError::Unavailable)?;
        if bytes.len() > MAX_SETTINGS_RESPONSE_BYTES {
            return Err(SettingsError::InvalidResponse);
        }
        serde_json::from_slice(&bytes).map_err(|_| SettingsError::InvalidResponse)
    }
}
