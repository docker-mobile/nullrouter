mod types;

pub use types::{
    MitmAction, MitmDashboardState, MitmFieldState, MitmModelMapping, MitmServerState,
    MitmStatusCheck, MitmToolState,
};

const STATUS_CHECKS: [MitmStatusCheck; 3] = [
    MitmStatusCheck {
        label: "Cert",
        ok: false,
    },
    MitmStatusCheck {
        label: "Trusted",
        ok: false,
    },
    MitmStatusCheck {
        label: "Server",
        ok: false,
    },
];

const START_DNS: MitmAction = MitmAction {
    label: "Start DNS",
    status_label: "MITM DNS control unavailable",
    enabled: false,
};

const ANTIGRAVITY_HOSTS: [&str; 2] = [
    "daily-cloudcode-pa.googleapis.com",
    "cloudcode-pa.googleapis.com",
];
const COPILOT_HOSTS: [&str; 1] = ["api.individual.githubcopilot.com"];
const KIRO_HOSTS: [&str; 3] = [
    "runtime.us-east-1.kiro.dev",
    "q.us-east-1.amazonaws.com",
    "codewhisperer.us-east-1.amazonaws.com",
];

const ANTIGRAVITY_MODELS: [MitmModelMapping; 9] = [
    MitmModelMapping {
        name: "Gemini 3.5 Flash (Medium) / Default",
        alias: "gemini-3.5-flash-low",
    },
    MitmModelMapping {
        name: "Gemini 3.5 Flash (High)",
        alias: "gemini-3-flash-agent",
    },
    MitmModelMapping {
        name: "Gemini 3.5 Flash (Low)",
        alias: "gemini-3.5-flash-extra-low",
    },
    MitmModelMapping {
        name: "Gemini 3.1 Pro (Low)",
        alias: "gemini-3.1-pro-low",
    },
    MitmModelMapping {
        name: "Gemini 3.1 Pro (High)",
        alias: "gemini-pro-agent",
    },
    MitmModelMapping {
        name: "Claude Sonnet 4.6 (Thinking)",
        alias: "claude-sonnet-4-6",
    },
    MitmModelMapping {
        name: "Claude Opus 4.6 (Thinking)",
        alias: "claude-opus-4-6-thinking",
    },
    MitmModelMapping {
        name: "GPT-OSS 120B (Medium)",
        alias: "gpt-oss-120b-medium",
    },
    MitmModelMapping {
        name: "Gemini 3 Flash (Command)",
        alias: "gemini-3-flash",
    },
];

const COPILOT_MODELS: [MitmModelMapping; 5] = [
    MitmModelMapping {
        name: "GPT-5 mini",
        alias: "gpt-5-mini",
    },
    MitmModelMapping {
        name: "GPT-5.4 nano",
        alias: "gpt-5.4-nano",
    },
    MitmModelMapping {
        name: "Claude Haiku 4.5",
        alias: "claude-haiku-4.5",
    },
    MitmModelMapping {
        name: "GPT-4o",
        alias: "gpt-4o",
    },
    MitmModelMapping {
        name: "GPT-4.1",
        alias: "gpt-4.1",
    },
];

const KIRO_MODELS: [MitmModelMapping; 7] = [
    MitmModelMapping {
        name: "Claude Sonnet 5",
        alias: "claude-sonnet-5",
    },
    MitmModelMapping {
        name: "Claude Sonnet 4.5",
        alias: "claude-sonnet-4.5",
    },
    MitmModelMapping {
        name: "Claude Sonnet 4",
        alias: "claude-sonnet-4",
    },
    MitmModelMapping {
        name: "Claude Haiku 4.5",
        alias: "claude-haiku-4.5",
    },
    MitmModelMapping {
        name: "DeepSeek 3.2",
        alias: "deepseek-3.2",
    },
    MitmModelMapping {
        name: "MiniMax M2.1",
        alias: "minimax-m2.1",
    },
    MitmModelMapping {
        name: "Qwen3 Coder Next",
        alias: "simple-task",
    },
];

const TOOLS: [MitmToolState; 3] = [
    MitmToolState {
        id: "antigravity",
        name: "Antigravity",
        image: "/providers/antigravity.png",
        intercept_label: "Intercept Antigravity requests via MITM proxy",
        dns_instruction: "Toggle DNS to redirect Antigravity traffic through 9Router via MITM.",
        hosts: &ANTIGRAVITY_HOSTS,
        models: &ANTIGRAVITY_MODELS,
        server_running: false,
        dns_active: false,
        server_status_label: "Server off",
        dns_status_label: "DNS off",
        mapping_inputs_enabled: false,
        model_select_enabled: false,
        dns_action: START_DNS,
    },
    MitmToolState {
        id: "copilot",
        name: "GitHub Copilot",
        image: "/providers/copilot.png",
        intercept_label: "Intercept GitHub Copilot requests via MITM proxy",
        dns_instruction: "Toggle DNS to redirect GitHub Copilot traffic through 9Router via MITM.",
        hosts: &COPILOT_HOSTS,
        models: &COPILOT_MODELS,
        server_running: false,
        dns_active: false,
        server_status_label: "Server off",
        dns_status_label: "DNS off",
        mapping_inputs_enabled: false,
        model_select_enabled: false,
        dns_action: START_DNS,
    },
    MitmToolState {
        id: "kiro",
        name: "Kiro",
        image: "/providers/kiro.png",
        intercept_label: "Intercept Kiro requests via MITM proxy",
        dns_instruction: "Toggle DNS to redirect Kiro traffic through 9Router via MITM.",
        hosts: &KIRO_HOSTS,
        models: &KIRO_MODELS,
        server_running: false,
        dns_active: false,
        server_status_label: "Server off",
        dns_status_label: "DNS off",
        mapping_inputs_enabled: false,
        model_select_enabled: false,
        dns_action: START_DNS,
    },
];

pub const fn mitm_dashboard_state() -> MitmDashboardState {
    MitmDashboardState {
        route_path: "/dashboard/mitm",
        title: "MITM Proxy",
        risk_warning: "⚠️ MITM intercepts HTTPS traffic of IDE tools (Antigravity, GitHub Copilot, Kiro) via local CA to redirect requests to your providers. May violate ToS → account ban. Use at your own risk.",
        unsupported_notice: "MITM control is unsupported in this Rust/WASM dashboard. Server, certificate, DNS, and model mapping controls are disabled.",
        live_control_wired: false,
        server: MitmServerState {
            title: "MITM Server",
            status_label: "Stopped",
            running: false,
            checks: &STATUS_CHECKS,
            purpose: "Use Antigravity IDE & GitHub Copilot → with ANY provider/model from 9Router",
            how_it_works: "Antigravity/Copilot IDE request → DNS redirect to localhost:443 → MITM proxy intercepts → 9Router → response to Antigravity/Copilot",
            base_url: MitmFieldState {
                label: "9Router Base URL",
                value: "http://localhost:20128",
                placeholder: "http://localhost:20128",
                enabled: false,
            },
            api_key: MitmFieldState {
                label: "API Key",
                value: "",
                placeholder: "sk_9router (default)",
                enabled: false,
            },
            action: MitmAction {
                label: "Start Server",
                status_label: "MITM server control unavailable",
                enabled: false,
            },
        },
        tools: &TOOLS,
        hosts_instruction: "Edit hosts file manually to add the following entries:",
        mapping_notice: "Enable DNS to edit model mappings",
        mapping_placeholder: "provider/model-id",
        select_label: "Select",
    }
}
