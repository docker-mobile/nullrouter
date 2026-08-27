use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    pub(crate) require_api_key: bool,
    pub(crate) has_password: bool,
    pub(crate) tunnel_dashboard_access: bool,
    pub(crate) oidc_configured: bool,
    pub(crate) enable_request_logs: bool,
    pub(crate) enable_translator: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Keys {
    pub(crate) keys: &'static [serde_json::Value],
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Usage {
    pub(crate) stream_connected: bool,
    pub(crate) active_requests: u8,
    pub(crate) requests_today: u32,
    pub(crate) tokens_today: u32,
    pub(crate) estimated_cost: &'static str,
    pub(crate) topology_providers: &'static [UsageProviderNode],
    pub(crate) recent_requests: &'static [RecentRequest],
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct UsageProviderNode {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) accent: &'static str,
    pub(crate) slot_class: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct RecentRequest {
    pub(crate) provider: &'static str,
    pub(crate) route: &'static str,
    pub(crate) status: &'static str,
    pub(crate) age: &'static str,
}

pub(crate) const fn settings() -> Settings {
    Settings {
        require_api_key: false,
        has_password: false,
        tunnel_dashboard_access: false,
        oidc_configured: false,
        enable_request_logs: false,
        enable_translator: false,
    }
}

pub(crate) const fn keys() -> Keys {
    Keys { keys: &[] }
}

pub(crate) const fn usage() -> Usage {
    Usage {
        stream_connected: false,
        active_requests: 0,
        requests_today: 0,
        tokens_today: 0,
        estimated_cost: "$0.00",
        topology_providers: &TOPOLOGY_PROVIDERS,
        recent_requests: &[],
    }
}

const TOPOLOGY_PROVIDERS: [UsageProviderNode; 6] = [
    UsageProviderNode {
        id: "claude",
        name: "Claude",
        accent: "#d97757",
        slot_class: "slot-one",
    },
    UsageProviderNode {
        id: "codex",
        name: "Codex",
        accent: "#38bdf8",
        slot_class: "slot-two",
    },
    UsageProviderNode {
        id: "cursor",
        name: "Cursor",
        accent: "#8b5cf6",
        slot_class: "slot-three",
    },
    UsageProviderNode {
        id: "cline",
        name: "Cline",
        accent: "#22c55e",
        slot_class: "slot-four",
    },
    UsageProviderNode {
        id: "openai",
        name: "OpenAI Compatible",
        accent: "#10b981",
        slot_class: "slot-five",
    },
    UsageProviderNode {
        id: "anthropic",
        name: "Anthropic Compatible",
        accent: "#f59e0b",
        slot_class: "slot-six",
    },
];
