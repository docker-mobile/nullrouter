use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ComboSummary {
    pub name: &'static str,
    pub kind: &'static str,
    pub members: &'static [&'static str],
    pub routing: &'static str,
    pub persisted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QuotaTrackerState {
    pub live_limits_connected: bool,
    pub source_label: &'static str,
    pub rows: Vec<QuotaRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QuotaRow {
    pub provider: &'static str,
    pub used: u32,
    pub total: Option<u32>,
    pub reset_label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CliToolSummary {
    pub id: &'static str,
    pub name: &'static str,
    pub intent: &'static str,
    pub endpoint_mode: &'static str,
    pub status_checked: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CliToolDetailSection {
    pub title: &'static str,
    pub body: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CliToolDetailState {
    pub route_path: String,
    pub tool: CliToolSummary,
    pub base_url: &'static str,
    pub status_checked: bool,
    pub install_detection_wired: bool,
    pub api_keys_wired: bool,
    pub active_provider_count: u8,
    pub sections: &'static [CliToolDetailSection],
    pub preview_notice: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SkillSummary {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub endpoint: Option<&'static str>,
}

pub fn combo_summaries() -> Vec<ComboSummary> {
    vec![
        ComboSummary {
            name: "coding-fallback",
            kind: "chat",
            members: &["codex/gpt-5", "anthropic/claude-sonnet", "openai/gpt-5"],
            routing: "Fallback order preview",
            persisted: false,
        },
        ComboSummary {
            name: "web-research",
            kind: "search",
            members: &["9router-web-search", "9router-web-fetch"],
            routing: "Capability match preview",
            persisted: false,
        },
    ]
}

pub const fn quota_tracker_state() -> QuotaTrackerState {
    QuotaTrackerState {
        live_limits_connected: false,
        source_label: "Provider quota API not connected",
        rows: Vec::new(),
    }
}

pub const fn cli_tools() -> &'static [CliToolSummary] {
    &CLI_TOOLS
}

pub fn cli_tool_detail_state(tool_id: &str) -> Option<CliToolDetailState> {
    cli_tools()
        .iter()
        .find(|tool| tool.id == tool_id)
        .copied()
        .map(|tool| CliToolDetailState {
            route_path: format!("/dashboard/cli-tools/{}", tool.id),
            tool,
            base_url: "http://localhost:20128",
            status_checked: tool.status_checked,
            install_detection_wired: false,
            api_keys_wired: false,
            active_provider_count: 0,
            sections: &CLI_TOOL_DETAIL_SECTIONS,
            preview_notice: "Tool detection, API key lookup, model mapping, and config writes remain disabled until the host-side CLI tool APIs are connected.",
        })
}

pub const fn skill_summaries() -> &'static [SkillSummary] {
    &SKILLS
}

const CLI_TOOLS: [CliToolSummary; 8] = [
    CliToolSummary {
        id: "codex",
        name: "Codex CLI",
        intent: "OpenAI-compatible endpoint and key setup",
        endpoint_mode: "Base URL preview",
        status_checked: false,
    },
    CliToolSummary {
        id: "claude",
        name: "Claude Code",
        intent: "Anthropic-format routing through the local gateway",
        endpoint_mode: "Provider bridge preview",
        status_checked: false,
    },
    CliToolSummary {
        id: "opencode",
        name: "OpenCode",
        intent: "Model catalog and OpenAI-compatible key pairing",
        endpoint_mode: "Base URL preview",
        status_checked: false,
    },
    CliToolSummary {
        id: "cline",
        name: "Cline",
        intent: "Editor agent profile using `/v1` routes",
        endpoint_mode: "Client profile preview",
        status_checked: false,
    },
    CliToolSummary {
        id: "copilot",
        name: "Copilot CLI",
        intent: "CLI config guide for compatible models",
        endpoint_mode: "Instruction preview",
        status_checked: false,
    },
    CliToolSummary {
        id: "jcode",
        name: "JCode",
        intent: "Terminal coding agent endpoint mapping",
        endpoint_mode: "Instruction preview",
        status_checked: false,
    },
    CliToolSummary {
        id: "mitm",
        name: "MITM Bridge",
        intent: "Proxy inspection tool link from upstream dashboard",
        endpoint_mode: "Tool link preview",
        status_checked: false,
    },
    CliToolSummary {
        id: "antigravity",
        name: "Antigravity",
        intent: "Desktop agent config handoff",
        endpoint_mode: "Instruction preview",
        status_checked: false,
    },
];

const SKILLS: [SkillSummary; 8] = [
    SkillSummary {
        id: "9router",
        name: "9Router Entry",
        description: "Setup index for base URL, auth, model discovery, and capability skills.",
        endpoint: None,
    },
    SkillSummary {
        id: "9router-chat",
        name: "Chat",
        description: "Chat and code generation through OpenAI or Anthropic-compatible routes.",
        endpoint: Some("/v1/chat/completions"),
    },
    SkillSummary {
        id: "9router-image",
        name: "Image Generation",
        description: "Image generation capability guide for future provider execution.",
        endpoint: Some("/v1/images/generations"),
    },
    SkillSummary {
        id: "9router-tts",
        name: "Text-to-Speech",
        description: "Speech generation skill mapped to OpenAI-compatible audio routes.",
        endpoint: Some("/v1/audio/speech"),
    },
    SkillSummary {
        id: "9router-stt",
        name: "Speech-to-Text",
        description: "Audio transcription skill mapped to hosted transcription routes.",
        endpoint: Some("/v1/audio/transcriptions"),
    },
    SkillSummary {
        id: "9router-embeddings",
        name: "Embeddings",
        description: "Vector generation guide for RAG and semantic search clients.",
        endpoint: Some("/v1/embeddings"),
    },
    SkillSummary {
        id: "9router-web-search",
        name: "Web Search",
        description: "Search provider guide for Tavily, Exa, Brave, Serper, and compatible tools.",
        endpoint: Some("/v1/search"),
    },
    SkillSummary {
        id: "9router-web-fetch",
        name: "Web Fetch",
        description: "URL to text, markdown, or HTML fetching through web provider routes.",
        endpoint: Some("/v1/web/fetch"),
    },
];

const CLI_TOOL_DETAIL_SECTIONS: [CliToolDetailSection; 4] = [
    CliToolDetailSection {
        title: "Endpoint",
        body: "Use the local OpenAI-compatible base URL when the selected tool supports custom endpoint configuration.",
        enabled: false,
    },
    CliToolDetailSection {
        title: "API Key",
        body: "API key selection is shown as a placeholder because key persistence is not wired in this WASM slice.",
        enabled: false,
    },
    CliToolDetailSection {
        title: "Model Mapping",
        body: "Default aliases and model mapping controls mirror upstream intent without writing local tool config.",
        enabled: false,
    },
    CliToolDetailSection {
        title: "Install Status",
        body: "Host-side binary detection and status checks are not executed from the browser preview.",
        enabled: false,
    },
];
