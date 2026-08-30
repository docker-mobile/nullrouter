use serde::{Deserialize, Serialize};

mod auth;
mod errors;

pub use auth::{
    ApiKeyGateRequest, ApiKeyGateResponse, AuthorizeRequest, AuthorizeResponse,
    INTERNAL_API_KEY_GATE_PATH, INTERNAL_API_KEY_VALIDATE_PATH, INTERNAL_AUTHORIZE_PATH,
    SecretString, ValidateApiKeyRequest, ValidateApiKeyResponse,
};
pub use errors::{
    ErrorBody, ErrorEnvelope, ResponsesFailedEvent, ResponsesFailedResponse, invalid_request_error,
    provider_execution_error, responses_failed_event,
};

const MODEL_ROWS: [(&str, &str); 6] = [
    ("openai/gpt-5", "openai"),
    ("anthropic/claude-sonnet-4.5", "anthropic"),
    ("gemini/gemini-2.5-pro", "gemini"),
    ("github/gpt-4.1", "github"),
    ("kiro/claude-sonnet-4.5", "kiro"),
    ("opencode/sonnet", "opencode"),
];

const PROVIDER_ROWS: [(&str, &str, &str); 4] = [
    ("Claude", "/providers/claude.png", "OAuth-ready reference"),
    ("Codex", "/providers/codex.png", "Token import reference"),
    (
        "Cursor",
        "/providers/cursor.png",
        "Composer routing reference",
    ),
    ("Cline", "/providers/cline.png", "CLI bridge reference"),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionResponse {
    #[serde(rename = "currentVersion")]
    pub current_version: String,
    #[serde(rename = "latestVersion")]
    pub latest_version: Option<String>,
    #[serde(rename = "hasUpdate")]
    pub has_update: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthStatusResponse {
    /// Always `true`. Dashboard login is unconditional in nullrouter, so this
    /// reports a constant rather than a setting — see the note on
    /// `/api/auth/status` in `services/auth-actix/src/routes.rs`.
    #[serde(rename = "requireLogin")]
    pub require_login: bool,
    #[serde(rename = "authMode")]
    pub auth_mode: String,
    #[serde(rename = "oidcConfigured")]
    pub oidc_configured: bool,
    #[serde(rename = "oidcLoginLabel")]
    pub oidc_login_label: String,
    #[serde(rename = "hasPassword")]
    pub has_password: bool,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "loginMethod")]
    pub login_method: String,
    #[serde(rename = "oidcName")]
    pub oidc_name: Option<String>,
    #[serde(rename = "oidcEmail")]
    pub oidc_email: Option<String>,
    #[serde(rename = "oidcLogin")]
    pub oidc_login: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsResponse {
    #[serde(rename = "requireApiKey")]
    pub require_api_key: bool,
    #[serde(rename = "hasPassword")]
    pub has_password: bool,
    #[serde(rename = "tunnelDashboardAccess")]
    pub tunnel_dashboard_access: bool,
    #[serde(rename = "oidcConfigured")]
    pub oidc_configured: bool,
    #[serde(rename = "enableRequestLogs")]
    pub enable_request_logs: bool,
    #[serde(rename = "enableTranslator")]
    pub enable_translator: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeysResponse {
    pub keys: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderClientResponse {
    pub connections: Vec<serde_json::Value>,
    #[serde(rename = "providerOptions")]
    pub provider_options: Vec<serde_json::Value>,
    pub pagination: Pagination,
    pub totals: ProviderClientTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pagination {
    pub page: u16,
    #[serde(rename = "pageSize")]
    pub page_size: u16,
    pub total: u16,
    #[serde(rename = "totalPages")]
    pub total_pages: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderClientTotals {
    #[serde(rename = "eligibleConnections")]
    pub eligible_connections: u16,
    #[serde(rename = "providerFilteredConnections")]
    pub provider_filtered_connections: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub object: String,
    #[serde(rename = "owned_by")]
    pub owned_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelList {
    pub object: String,
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatRequest {
    pub model: Option<String>,
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DashboardProvider {
    pub name: String,
    pub icon: String,
    pub status: String,
}

pub const fn health_response() -> HealthResponse {
    HealthResponse { ok: true }
}

pub const fn init_response() -> &'static str {
    "Initialized"
}

pub fn version_response(current_version: impl Into<String>) -> VersionResponse {
    VersionResponse {
        current_version: current_version.into(),
        latest_version: None,
        has_update: false,
    }
}

pub fn auth_status_response() -> AuthStatusResponse {
    AuthStatusResponse {
        require_login: true,
        auth_mode: "password".to_owned(),
        oidc_configured: false,
        oidc_login_label: "Sign in with OIDC".to_owned(),
        has_password: false,
        display_name: "Password user".to_owned(),
        login_method: "Password".to_owned(),
        oidc_name: None,
        oidc_email: None,
        oidc_login: false,
    }
}

pub const fn settings_response() -> SettingsResponse {
    SettingsResponse {
        require_api_key: false,
        has_password: false,
        tunnel_dashboard_access: false,
        oidc_configured: false,
        enable_request_logs: false,
        enable_translator: false,
    }
}

pub const fn keys_response() -> KeysResponse {
    KeysResponse { keys: Vec::new() }
}

pub const fn providers_client_response() -> ProviderClientResponse {
    ProviderClientResponse {
        connections: Vec::new(),
        provider_options: Vec::new(),
        pagination: Pagination {
            page: 1,
            page_size: 20,
            total: 0,
            total_pages: 1,
        },
        totals: ProviderClientTotals {
            eligible_connections: 0,
            provider_filtered_connections: 0,
        },
    }
}

pub fn model_list() -> ModelList {
    ModelList {
        object: "list".to_owned(),
        data: MODEL_ROWS
            .iter()
            .map(|(id, owner)| ModelEntry {
                id: (*id).to_owned(),
                object: "model".to_owned(),
                owned_by: (*owner).to_owned(),
            })
            .collect(),
    }
}

pub fn dashboard_providers() -> Vec<DashboardProvider> {
    PROVIDER_ROWS
        .iter()
        .map(|(name, icon, status)| DashboardProvider {
            name: (*name).to_owned(),
            icon: (*icon).to_owned(),
            status: (*status).to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        auth_status_response, invalid_request_error, model_list, provider_execution_error,
        responses_failed_event,
    };

    #[test]
    fn model_list_is_openai_compatible() {
        let models = model_list();

        assert_eq!(models.object, "list");
        assert_eq!(models.data.len(), 6);
        assert!(models.data.iter().any(|model| model.id == "openai/gpt-5"));
        assert!(models.data.iter().all(|model| model.object == "model"));
    }

    #[test]
    fn auth_status_matches_reference_fallback() {
        let status = auth_status_response();

        assert!(status.require_login);
        assert_eq!(status.auth_mode, "password");
        assert!(!status.oidc_configured);
        assert!(!status.has_password);
        assert_eq!(status.display_name, "Password user");
        assert_eq!(status.login_method, "Password");
        assert_eq!(status.oidc_name, None);
        assert_eq!(status.oidc_email, None);
    }

    #[test]
    fn chat_errors_preserve_reference_shape() {
        let invalid = invalid_request_error("Invalid JSON body");
        let provider = provider_execution_error("openai/gpt-5", false);

        assert_eq!(invalid.error.error_type, "invalid_request_error");
        assert_eq!(invalid.error.message, "Invalid JSON body");
        assert_eq!(invalid.error.code, None);
        assert_eq!(provider.error.error_type, "not_implemented");
        assert_eq!(
            provider.error.code.as_deref(),
            Some("provider_execution_unimplemented"),
        );
        assert_eq!(provider.error.model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(provider.error.stream, Some(false));
    }

    #[test]
    fn responses_failed_event_preserves_provider_error() {
        let event = responses_failed_event(provider_execution_error("openai/gpt-5", true));

        assert_eq!(event.event_type, "response.failed");
        assert_eq!(event.response.status, "failed");
        assert_eq!(
            event.response.error.code.as_deref(),
            Some("provider_execution_unimplemented"),
        );
        assert_eq!(event.response.error.model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(event.response.error.stream, Some(true));
    }
}
