use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TranslatorState {
    pub route_path: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub log_directory: &'static str,
    pub api_default_response_file: &'static str,
    pub common_actions: &'static [TranslatorAction],
    pub capabilities: &'static [TranslatorCapability],
    pub meta: &'static [TranslatorMetaBadge],
    pub steps: &'static [TranslatorStep],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TranslatorStep {
    pub id: u8,
    pub label: &'static str,
    pub file: &'static str,
    pub language: TranslatorStepLanguage,
    pub description: &'static str,
    pub preview: &'static str,
    pub primary_action: Option<TranslatorAction>,
    pub api_default_file: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TranslatorStepLanguage {
    Json,
    Text,
}

impl TranslatorStepLanguage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TranslatorAction {
    pub label: &'static str,
    pub status_label: &'static str,
    pub enabled: bool,
    pub tone: TranslatorActionTone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranslatorActionTone {
    Primary,
    Secondary,
}

impl TranslatorActionTone {
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Primary => "nr-button primary small",
            Self::Secondary => "nr-button secondary small",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TranslatorCapability {
    pub label: &'static str,
    pub detail: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TranslatorMetaBadge {
    pub label: &'static str,
    pub value: &'static str,
    pub detail: &'static str,
}

const LOAD_ACTION: TranslatorAction = TranslatorAction {
    label: "Load",
    status_label: "Disabled: browser WASM cannot read logs/translator files.",
    enabled: false,
    tone: TranslatorActionTone::Secondary,
};
const COPY_ACTION: TranslatorAction = TranslatorAction {
    label: "Copy",
    status_label: "Disabled until clipboard wiring is added to this panel.",
    enabled: false,
    tone: TranslatorActionTone::Secondary,
};
const FORMAT_ACTION: TranslatorAction = TranslatorAction {
    label: "Format",
    status_label: "Disabled until editable JSON buffers are wired.",
    enabled: false,
    tone: TranslatorActionTone::Secondary,
};
const TO_OPENAI_ACTION: TranslatorAction = TranslatorAction {
    label: "→ OpenAI",
    status_label: "Disabled: /api/translator/translate step 2 is not mounted.",
    enabled: false,
    tone: TranslatorActionTone::Primary,
};
const TO_TARGET_ACTION: TranslatorAction = TranslatorAction {
    label: "→ Target",
    status_label: "Disabled: /api/translator/translate step 3 is not mounted.",
    enabled: false,
    tone: TranslatorActionTone::Primary,
};
const SEND_ACTION: TranslatorAction = TranslatorAction {
    label: "Send",
    status_label: "Disabled: /api/translator/send is not mounted.",
    enabled: false,
    tone: TranslatorActionTone::Primary,
};

const COMMON_ACTIONS: [TranslatorAction; 3] = [LOAD_ACTION, COPY_ACTION, FORMAT_ACTION];

const CAPABILITIES: [TranslatorCapability; 5] = [
    TranslatorCapability {
        label: "Filesystem",
        detail: "logs/translator loading is unavailable in this WASM slice.",
        enabled: false,
    },
    TranslatorCapability {
        label: "Load",
        detail: "Load buttons are visible but disabled until file access is bridged.",
        enabled: false,
    },
    TranslatorCapability {
        label: "Save",
        detail: "Save is disabled until /api/translator/save is mounted.",
        enabled: false,
    },
    TranslatorCapability {
        label: "Provider execution",
        detail: "Send is disabled until the provider executor API is mounted.",
        enabled: false,
    },
    TranslatorCapability {
        label: "Persistence",
        detail: "Preview-only workspace; no translator log files are written.",
        enabled: false,
    },
];

const META_BADGES: [TranslatorMetaBadge; 4] = [
    TranslatorMetaBadge {
        label: "src",
        value: "default",
        detail: "source format not detected",
    },
    TranslatorMetaBadge {
        label: "dst",
        value: "openai",
        detail: "target format default",
    },
    TranslatorMetaBadge {
        label: "provider",
        value: "unwired",
        detail: "provider detection API unavailable",
    },
    TranslatorMetaBadge {
        label: "model",
        value: "unwired",
        detail: "model detection API unavailable",
    },
];

const STEP_PREVIEW_JSON: &str = r#"{
  "status": "preview",
  "detail": "Load is disabled until translator log file access is wired."
}"#;
const PROVIDER_RESPONSE_PREVIEW: &str =
    "event: preview\ndata: Provider response streaming is disabled in this WASM slice.";
const OPENAI_RESPONSE_PREVIEW: &str =
    "data: target → openai response preview waits for translator API wiring.";
const CLIENT_RESPONSE_PREVIEW: &str =
    "data: Final client response preview; API defaults also accept 7_res_client.json.";

const STEPS: [TranslatorStep; 7] = [
    TranslatorStep {
        id: 1,
        label: "Client Request",
        file: "1_req_client.json",
        language: TranslatorStepLanguage::Json,
        description: "Raw request from client",
        preview: STEP_PREVIEW_JSON,
        primary_action: Some(TO_OPENAI_ACTION),
        api_default_file: None,
    },
    TranslatorStep {
        id: 2,
        label: "Source Body",
        file: "2_req_source.json",
        language: TranslatorStepLanguage::Json,
        description: "After initial conversion",
        preview: STEP_PREVIEW_JSON,
        primary_action: None,
        api_default_file: None,
    },
    TranslatorStep {
        id: 3,
        label: "OpenAI Intermediate",
        file: "3_req_openai.json",
        language: TranslatorStepLanguage::Json,
        description: "source → openai",
        preview: STEP_PREVIEW_JSON,
        primary_action: Some(TO_TARGET_ACTION),
        api_default_file: None,
    },
    TranslatorStep {
        id: 4,
        label: "Target Request",
        file: "4_req_target.json",
        language: TranslatorStepLanguage::Json,
        description: "openai → target + URL + headers",
        preview: STEP_PREVIEW_JSON,
        primary_action: Some(SEND_ACTION),
        api_default_file: None,
    },
    TranslatorStep {
        id: 5,
        label: "Provider Response",
        file: "5_res_provider.txt",
        language: TranslatorStepLanguage::Text,
        description: "Raw SSE from provider",
        preview: PROVIDER_RESPONSE_PREVIEW,
        primary_action: None,
        api_default_file: None,
    },
    TranslatorStep {
        id: 6,
        label: "OpenAI Response",
        file: "6_res_openai.txt",
        language: TranslatorStepLanguage::Text,
        description: "target → openai (response)",
        preview: OPENAI_RESPONSE_PREVIEW,
        primary_action: None,
        api_default_file: None,
    },
    TranslatorStep {
        id: 7,
        label: "Client Response",
        file: "7_res_client.txt",
        language: TranslatorStepLanguage::Text,
        description: "Final response to client",
        preview: CLIENT_RESPONSE_PREVIEW,
        primary_action: None,
        api_default_file: Some("7_res_client.json"),
    },
];

pub const fn translator_dashboard_state() -> TranslatorState {
    TranslatorState {
        route_path: "/dashboard/translator",
        title: "Translator Debug",
        subtitle: "Replay request flow — matches log files",
        log_directory: "logs/translator/",
        api_default_response_file: "7_res_client.json",
        common_actions: &COMMON_ACTIONS,
        capabilities: &CAPABILITIES,
        meta: &META_BADGES,
        steps: &STEPS,
    }
}
