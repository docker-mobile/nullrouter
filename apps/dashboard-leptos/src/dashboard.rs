mod basic_chat;
pub mod basic_chat_live;
pub mod cli_tools_live;
pub mod combos_live;
pub mod console_log_live;
pub mod headroom_live;
pub mod mitm_live;
pub mod quota_live;
pub mod translator_live;

mod console_log;
mod fixtures;
mod g003;
mod media_providers;
pub mod migrate;
mod mitm;
mod parity;
pub mod pools_live;
mod pricing;
pub mod pricing_live;
pub mod providers_live;
mod proxy_pools;
pub mod settings;
mod translator;
mod usage;
pub mod usage_live;

use serde::Serialize;

pub use basic_chat::{
    BasicChatComposerState, BasicChatHistoryState, BasicChatModelOption, BasicChatProviderGroup,
    BasicChatSessionPreview, BasicChatState, basic_chat_dashboard_state,
    basic_chat_no_provider_state,
};
pub use console_log::{
    ConsoleLogAction, ConsoleLogEndpoint, ConsoleLogLevel, ConsoleLogLevelStyle, ConsoleLogLine,
    ConsoleLogRetention, ConsoleLogState, ConsoleLogStreamState, ConsoleLogStreamStatus,
    console_log_dashboard_state,
};
pub use g003::{
    DashboardPanelRow, DashboardPanelState, basic_chat_state, console_log_state,
    media_providers_web_state, profile_state, proxy_pools_state, translator_state,
};
pub use media_providers::{
    MediaProviderAction, MediaProviderComboDetailState, MediaProviderComboMember,
    MediaProviderComboPreview, MediaProviderConfigRow, MediaProviderDetailState,
    MediaProviderKindConfig, MediaProviderKindState, MediaProviderPlaceholder, MediaProviderTile,
    media_provider_combo_detail_state, media_provider_detail_state, media_provider_kind_state,
};
pub use mitm::{
    MitmAction, MitmDashboardState, MitmFieldState, MitmModelMapping, MitmServerState,
    MitmStatusCheck, MitmToolState, mitm_dashboard_state,
};
pub use parity::{
    CliToolDetailSection, CliToolDetailState, CliToolSummary, ComboSummary, QuotaRow,
    QuotaTrackerState, SkillSummary, TokenSaverState, cli_tool_detail_state, cli_tools,
    combo_summaries, quota_tracker_state, skill_summaries, token_saver_state,
};
pub use pricing::{PricingSettingsState, pricing_settings_state};
pub use proxy_pools::{
    ProxyPoolAction, ProxyPoolEmptyState, ProxyPoolEntry, ProxyPoolField, ProxyPoolModalState,
    ProxyPoolModals, ProxyPoolRowActions, ProxyPoolSelectionState, ProxyPoolTestStatus,
    ProxyPoolTotals, ProxyPoolType, ProxyPoolsState, RelayProviderAction, proxy_pool_modals,
    proxy_pool_sample_entry, proxy_pools_dashboard_state, proxy_pools_sample_state,
};
pub use settings::{
    LOGIN_ALWAYS_REQUIRED, REQUIRE_API_KEY_UNAVAILABLE, Resolution, SETTINGS_FIELDS,
    SETTINGS_GROUPS, SETTINGS_PATH, SettingsControl, SettingsField, SettingsGroup,
    SettingsSnapshot, SettingsValue, WriteOutcome, parse_settings, patch_body, resolve,
};
pub use translator::{
    TranslatorAction, TranslatorActionTone, TranslatorCapability, TranslatorMetaBadge,
    TranslatorState, TranslatorStep, TranslatorStepLanguage, translator_dashboard_state,
};
pub use usage::{RecentRequest, UsageProviderNode, UsageSnapshot, usage_snapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ProviderHealth {
    Connected,
    Degraded,
    Idle,
}

impl ProviderHealth {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Degraded => "Needs attention",
            Self::Idle => "No connections",
        }
    }

    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Connected => "is-connected",
            Self::Degraded => "is-degraded",
            Self::Idle => "is-idle",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderStatus {
    pub connected: u8,
    pub error: u8,
    pub total: u8,
    pub health: ProviderHealth,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderTile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub auth_label: String,
    pub accent: String,
    pub status: ProviderStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderGroup {
    pub title: String,
    pub subtitle: String,
    pub providers: Vec<ProviderTile>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderFormOption {
    pub id: String,
    pub name: String,
    pub auth_label: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct AuthMethodOption {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NewProviderState {
    pub title: &'static str,
    pub description: &'static str,
    pub provider_options: Vec<ProviderFormOption>,
    pub auth_methods: &'static [AuthMethodOption],
    pub default_auth_method: &'static str,
    pub is_active_default: bool,
    pub persistence_wired: bool,
    pub submit_label: &'static str,
    pub preview_notice: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderDetailAction {
    pub label: &'static str,
    pub status_label: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderDetailState {
    pub route_path: String,
    pub provider: ProviderTile,
    pub auth_modes: Vec<&'static str>,
    pub connection_count: u8,
    pub model_count: usize,
    pub connections_wired: bool,
    pub provider_settings_wired: bool,
    pub model_settings_wired: bool,
    pub actions: &'static [ProviderDetailAction],
    pub preview_notice: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelTile {
    pub id: String,
    pub provider: String,
    pub family: String,
    pub context: String,
    pub status: ProviderHealth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct EndpointRow {
    pub label: &'static str,
    pub value: &'static str,
    pub badge: EndpointBadge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum EndpointBadge {
    Local,
    Cloudflare,
    Tailscale,
}

impl EndpointBadge {
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Local => "endpoint-badge-local",
            Self::Cloudflare | Self::Tailscale => "endpoint-badge-remote",
        }
    }
}

pub fn provider_groups() -> Vec<ProviderGroup> {
    let oauth_providers = oauth_provider_tiles();

    vec![
        ProviderGroup {
            title: "OAuth Providers".to_owned(),
            subtitle: "Browser, CLI, and account-backed credentials".to_owned(),
            providers: oauth_providers,
        },
        ProviderGroup {
            title: "API Key Providers".to_owned(),
            subtitle: "OpenAI, Anthropic, and compatible upstreams".to_owned(),
            providers: fixtures::api_key_providers(),
        },
    ]
}

pub fn provider_new_state() -> NewProviderState {
    NewProviderState {
        title: "Add New Provider",
        description: "Configure a new AI provider to use with your applications.",
        provider_options: all_provider_tiles()
            .into_iter()
            .map(|provider| ProviderFormOption {
                id: provider.id,
                name: provider.name,
                auth_label: provider.auth_label,
                description: provider.description,
            })
            .collect(),
        auth_methods: &AUTH_METHOD_OPTIONS,
        default_auth_method: "api_key",
        is_active_default: true,
        persistence_wired: false,
        submit_label: "Create Provider (Preview only)",
        preview_notice: "Provider creation is disabled until dashboard persistence is wired.",
    }
}

pub fn provider_detail_state(provider_id: &str) -> Option<ProviderDetailState> {
    all_provider_tiles()
        .into_iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| {
            let model_count = model_catalog()
                .into_iter()
                .filter(|model| model.provider == provider.id)
                .count();

            ProviderDetailState {
                route_path: format!("/dashboard/providers/{}", provider.id),
                auth_modes: provider_auth_modes(&provider),
                connection_count: provider.status.connected,
                model_count,
                provider,
                connections_wired: false,
                provider_settings_wired: false,
                model_settings_wired: false,
                actions: &PROVIDER_DETAIL_ACTIONS,
                preview_notice: "Connection, model, proxy, and strategy actions are visible as disabled previews until host APIs are connected.",
            }
        })
}

fn oauth_provider_tiles() -> Vec<ProviderTile> {
    nullrouter_contracts::dashboard_providers()
        .into_iter()
        .map(|provider| {
            let id = provider.name.to_ascii_lowercase();
            ProviderTile {
                accent: provider_accent(&id).to_owned(),
                auth_label: "Contract".to_owned(),
                description: provider.status,
                id,
                name: provider.name,
                status: ProviderStatus {
                    connected: 0,
                    error: 0,
                    total: 0,
                    health: ProviderHealth::Idle,
                },
            }
        })
        .collect()
}

pub fn model_catalog() -> Vec<ModelTile> {
    nullrouter_contracts::model_list()
        .data
        .into_iter()
        .map(|entry| ModelTile {
            family: model_family(&entry.id).to_owned(),
            context: model_context(&entry.owned_by).to_owned(),
            status: model_status(&entry.owned_by),
            provider: entry.owned_by,
            id: entry.id,
        })
        .collect()
}

pub const fn endpoint_rows() -> &'static [EndpointRow] {
    fixtures::endpoint_rows()
}

fn provider_accent(provider_id: &str) -> &'static str {
    match provider_id {
        "claude" => "#d97757",
        "codex" => "#38bdf8",
        "cursor" => "#8b5cf6",
        "cline" => "#22c55e",
        _ => "#e56a4a",
    }
}

fn all_provider_tiles() -> Vec<ProviderTile> {
    oauth_provider_tiles()
        .into_iter()
        .chain(fixtures::api_key_providers())
        .collect()
}

fn provider_auth_modes(provider: &ProviderTile) -> Vec<&'static str> {
    match provider.auth_label.as_str() {
        "Contract" => vec!["OAuth"],
        "API key" => vec!["API Key"],
        _ => vec!["Provider credential"],
    }
}

fn model_family(model_id: &str) -> &'static str {
    if model_id.contains("sonnet") || model_id.contains("claude") {
        "Chat"
    } else if model_id.contains("gpt") {
        "OpenAI"
    } else if model_id.contains("gemini") {
        "Gemini"
    } else {
        "Router"
    }
}

fn model_context(provider_id: &str) -> &'static str {
    match provider_id {
        "gemini" => "1M",
        "anthropic" | "kiro" | "opencode" => "200K",
        _ => "128K",
    }
}

fn model_status(provider_id: &str) -> ProviderHealth {
    match provider_id {
        "openai" | "anthropic" | "codex" | "github" | "kiro" | "opencode" => {
            ProviderHealth::Connected
        }
        "gemini" => ProviderHealth::Degraded,
        _ => ProviderHealth::Idle,
    }
}

const AUTH_METHOD_OPTIONS: [AuthMethodOption; 2] = [
    AuthMethodOption {
        id: "api_key",
        label: "API Key",
        description: "Paste a provider token when the host persistence API is available.",
    },
    AuthMethodOption {
        id: "oauth2",
        label: "OAuth2",
        description: "Start an account-backed OAuth flow when callback persistence is wired.",
    },
];

const PROVIDER_DETAIL_ACTIONS: [ProviderDetailAction; 5] = [
    ProviderDetailAction {
        label: "Add Connection",
        status_label: "Preview only",
        enabled: false,
    },
    ProviderDetailAction {
        label: "Test Connection One-by-One",
        status_label: "Execution unavailable",
        enabled: false,
    },
    ProviderDetailAction {
        label: "Round Robin",
        status_label: "Settings preview",
        enabled: false,
    },
    ProviderDetailAction {
        label: "Apply Proxy",
        status_label: "Proxy pools offline",
        enabled: false,
    },
    ProviderDetailAction {
        label: "Edit Models",
        status_label: "Persistence unsupported",
        enabled: false,
    },
];
