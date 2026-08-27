use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
struct DashboardModelEntry {
    provider: &'static str,
    model: &'static str,
    #[serde(rename = "fullModel")]
    full_model: &'static str,
    alias: &'static str,
    caps: ModelCapabilities,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ModelCapabilities {
    vision: bool,
    search: bool,
    reasoning: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct DashboardModels {
    models: &'static [DashboardModelEntry],
}

const MODEL_CAPS_DEFAULT: ModelCapabilities = ModelCapabilities {
    vision: false,
    search: false,
    reasoning: false,
};

const DASHBOARD_MODELS_ARRAY: [DashboardModelEntry; 6] = [
    DashboardModelEntry {
        provider: "openai",
        model: "gpt-5",
        full_model: "openai/gpt-5",
        alias: "gpt-5",
        caps: MODEL_CAPS_DEFAULT,
    },
    DashboardModelEntry {
        provider: "anthropic",
        model: "claude-sonnet-4.5",
        full_model: "anthropic/claude-sonnet-4.5",
        alias: "claude-sonnet-4.5",
        caps: MODEL_CAPS_DEFAULT,
    },
    DashboardModelEntry {
        provider: "gemini",
        model: "gemini-2.5-pro",
        full_model: "gemini/gemini-2.5-pro",
        alias: "gemini-2.5-pro",
        caps: MODEL_CAPS_DEFAULT,
    },
    DashboardModelEntry {
        provider: "github",
        model: "gpt-4.1",
        full_model: "github/gpt-4.1",
        alias: "gpt-4.1",
        caps: MODEL_CAPS_DEFAULT,
    },
    DashboardModelEntry {
        provider: "kiro",
        model: "claude-sonnet-4.5",
        full_model: "kiro/claude-sonnet-4.5",
        alias: "claude-sonnet-4.5",
        caps: MODEL_CAPS_DEFAULT,
    },
    DashboardModelEntry {
        provider: "opencode",
        model: "sonnet",
        full_model: "opencode/sonnet",
        alias: "sonnet",
        caps: MODEL_CAPS_DEFAULT,
    },
];

pub(super) const fn dashboard_models() -> DashboardModels {
    DashboardModels {
        models: &DASHBOARD_MODELS_ARRAY,
    }
}
