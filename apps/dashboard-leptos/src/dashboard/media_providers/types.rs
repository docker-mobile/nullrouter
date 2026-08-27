use serde::Serialize;

use crate::dashboard::ProviderStatus;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderKindConfig {
    pub id: String,
    pub label: &'static str,
    pub icon: &'static str,
    pub endpoint_method: &'static str,
    pub endpoint_path: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderTile {
    pub id: String,
    pub name: String,
    pub description: &'static str,
    pub color: &'static str,
    pub text_icon: &'static str,
    pub service_kinds: &'static [&'static str],
    pub no_auth: bool,
    pub custom: bool,
    pub status: ProviderStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderAction {
    pub label: &'static str,
    pub status_label: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderPlaceholder {
    pub title: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderComboPreview {
    pub id: String,
    pub name: &'static str,
    pub kind_id: &'static str,
    pub members: Vec<MediaProviderComboMember>,
    pub routing: &'static str,
    pub persisted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderComboMember {
    pub entry: String,
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderKindState {
    pub route_path: String,
    pub kind: MediaProviderKindConfig,
    pub providers: Vec<MediaProviderTile>,
    pub combos: Vec<MediaProviderComboPreview>,
    pub actions: &'static [MediaProviderAction],
    pub provider_mutations_wired: bool,
    pub combo_mutations_wired: bool,
    pub preview_notice: &'static str,
    pub placeholder: Option<MediaProviderPlaceholder>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderDetailState {
    pub route_path: String,
    pub kind: MediaProviderKindConfig,
    pub provider: MediaProviderTile,
    pub config_rows: Vec<MediaProviderConfigRow>,
    pub connection_actions: &'static [MediaProviderAction],
    pub test_actions: &'static [MediaProviderAction],
    pub connection_writes_wired: bool,
    pub test_execution_wired: bool,
    pub model_settings_wired: bool,
    pub preview_notice: &'static str,
    pub placeholder: Option<MediaProviderPlaceholder>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderConfigRow {
    pub label: &'static str,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProviderComboDetailState {
    pub route_path: String,
    pub combo_id: String,
    pub name: String,
    pub kind: MediaProviderKindConfig,
    pub members: Vec<MediaProviderComboMember>,
    pub round_robin: bool,
    pub actions: &'static [MediaProviderAction],
    pub persistence_wired: bool,
    pub test_execution_wired: bool,
    pub example_path: Option<&'static str>,
    pub example_body: Option<&'static str>,
    pub curl_preview: String,
    pub usage_log_status: &'static str,
    pub placeholder: Option<MediaProviderPlaceholder>,
}
