//! Wire shapes the dashboard reads. Fields are the server's; unknown extras are ignored.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyRow {
    pub id: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct KeysList {
    #[serde(default)]
    pub keys: Vec<ApiKeyRow>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreatedKey {
    pub key: ApiKeyRow,
}

#[derive(Debug, Serialize)]
pub struct CreateKeyBody<'a> {
    pub name: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateKeyBody {
    pub is_active: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRow {
    pub id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub auth_type: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub test_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProvidersList {
    #[serde(default)]
    pub connections: Vec<ProviderRow>,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    #[serde(default)]
    pub total_requests: u64,
    #[serde(default)]
    pub total_prompt_tokens: u64,
    #[serde(default)]
    pub total_completion_tokens: u64,
    #[serde(default)]
    pub total_cached_tokens: u64,
    #[serde(default)]
    pub total_cost: u64,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageLive {
    #[serde(default)]
    pub live_telemetry: bool,
    #[serde(default)]
    pub active_requests: u64,
    #[serde(default)]
    pub requests_today: u64,
    #[serde(default)]
    pub tokens_today: u64,
    #[serde(default)]
    pub estimated_cost: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(default)]
    pub require_api_key: bool,
    #[serde(default)]
    pub tunnel_dashboard_access: bool,
    #[serde(default)]
    pub tunnel_url: String,
    #[serde(default)]
    pub tailscale_url: String,
    #[serde(default)]
    pub outbound_proxy_enabled: bool,
    #[serde(default)]
    pub outbound_proxy_url: String,
    #[serde(default)]
    pub outbound_no_proxy: String,
    #[serde(default)]
    pub oidc_issuer_url: String,
    #[serde(default)]
    pub oidc_client_id: String,
    #[serde(default)]
    pub oidc_client_secret_set: bool,
    #[serde(default)]
    pub oidc_scopes: String,
    #[serde(default)]
    pub oidc_login_label: String,
    #[serde(default)]
    pub pxpipe_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    #[serde(default)]
    pub authenticated: bool,
    #[serde(default)]
    pub require_login: bool,
    #[serde(default)]
    pub has_password: bool,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub login_method: String,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoginSuccess {
    #[serde(default)]
    pub success: bool,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoginDenied {
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub remaining_before_lock: u32,
}

#[derive(Debug, Serialize)]
pub struct LoginBody<'a> {
    pub password: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_api_key: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_dashboard_access: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbound_proxy_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pxpipe_enabled: Option<bool>,
}

pub fn display_name(entry: &nullrouter_providers::RegistryEntry) -> String {
    entry
        .display
        .as_ref()
        .and_then(|display| display.name.clone())
        .unwrap_or_else(|| entry.id.clone())
}
