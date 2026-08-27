use crate::catalog::{DashboardModel, ModelCapabilities, OpenAiModel, Provider, ProviderStatus};

pub(crate) const PROVIDERS: [Provider; 8] = [
    Provider {
        id: "claude",
        name: "Claude",
        description: "OAuth-ready reference",
        auth_label: "Contract",
        accent: "#d97757",
        status: IDLE,
    },
    Provider {
        id: "codex",
        name: "Codex",
        description: "Token import reference",
        auth_label: "Contract",
        accent: "#38bdf8",
        status: IDLE,
    },
    Provider {
        id: "cursor",
        name: "Cursor",
        description: "Composer routing reference",
        auth_label: "Contract",
        accent: "#8b5cf6",
        status: IDLE,
    },
    Provider {
        id: "cline",
        name: "Cline",
        description: "CLI bridge reference",
        auth_label: "Contract",
        accent: "#22c55e",
        status: IDLE,
    },
    Provider {
        id: "openai",
        name: "OpenAI Compatible",
        description: "Custom base URLs, OpenAI chat, and responses format.",
        auth_label: "API key",
        accent: "#10b981",
        status: IDLE,
    },
    Provider {
        id: "anthropic",
        name: "Anthropic Compatible",
        description: "Claude-format upstreams with model mapping.",
        auth_label: "API key",
        accent: "#f59e0b",
        status: IDLE,
    },
    Provider {
        id: "gemini",
        name: "Gemini",
        description: "Google model bridge with native schema translation.",
        auth_label: "API key",
        accent: "#60a5fa",
        status: DEGRADED,
    },
    Provider {
        id: "openrouter",
        name: "OpenRouter",
        description: "OpenAI-compatible aggregation for fallback routing.",
        auth_label: "API key",
        accent: "#e56a4a",
        status: IDLE,
    },
];

pub(crate) const DASHBOARD_MODELS: [DashboardModel; 6] = [
    DashboardModel {
        id: "openai/gpt-5",
        provider: "openai",
        model: "gpt-5",
        full_model: "openai/gpt-5",
        alias: "gpt-5",
        caps: CAPS_DEFAULT,
    },
    DashboardModel {
        id: "anthropic/claude-sonnet-4.5",
        provider: "anthropic",
        model: "claude-sonnet-4.5",
        full_model: "anthropic/claude-sonnet-4.5",
        alias: "claude-sonnet-4.5",
        caps: CAPS_DEFAULT,
    },
    DashboardModel {
        id: "gemini/gemini-2.5-pro",
        provider: "gemini",
        model: "gemini-2.5-pro",
        full_model: "gemini/gemini-2.5-pro",
        alias: "gemini-2.5-pro",
        caps: CAPS_DEFAULT,
    },
    DashboardModel {
        id: "github/gpt-4.1",
        provider: "github",
        model: "gpt-4.1",
        full_model: "github/gpt-4.1",
        alias: "gpt-4.1",
        caps: CAPS_DEFAULT,
    },
    DashboardModel {
        id: "kiro/claude-sonnet-4.5",
        provider: "kiro",
        model: "claude-sonnet-4.5",
        full_model: "kiro/claude-sonnet-4.5",
        alias: "claude-sonnet-4.5",
        caps: CAPS_DEFAULT,
    },
    DashboardModel {
        id: "opencode/sonnet",
        provider: "opencode",
        model: "sonnet",
        full_model: "opencode/sonnet",
        alias: "sonnet",
        caps: CAPS_DEFAULT,
    },
];

pub(crate) const OPENAI_MODELS: [OpenAiModel; 6] = [
    OpenAiModel {
        id: "openai/gpt-5",
        object: "model",
        owned_by: "openai",
    },
    OpenAiModel {
        id: "anthropic/claude-sonnet-4.5",
        object: "model",
        owned_by: "anthropic",
    },
    OpenAiModel {
        id: "gemini/gemini-2.5-pro",
        object: "model",
        owned_by: "gemini",
    },
    OpenAiModel {
        id: "github/gpt-4.1",
        object: "model",
        owned_by: "github",
    },
    OpenAiModel {
        id: "kiro/claude-sonnet-4.5",
        object: "model",
        owned_by: "kiro",
    },
    OpenAiModel {
        id: "opencode/sonnet",
        object: "model",
        owned_by: "opencode",
    },
];

const IDLE: ProviderStatus = ProviderStatus {
    connected: 0,
    error: 0,
    total: 0,
    health: "Idle",
};

const DEGRADED: ProviderStatus = ProviderStatus {
    connected: 0,
    error: 1,
    total: 1,
    health: "Needs attention",
};

const CAPS_DEFAULT: ModelCapabilities = ModelCapabilities {
    vision: false,
    search: false,
    reasoning: false,
};
