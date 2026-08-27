use nullrouter_contracts::model_list;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Health {
    ok: bool,
    service: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ModelInfo {
    id: String,
    name: String,
    kind: &'static str,
    owned_by: String,
    endpoint: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GeminiModelList {
    models: Vec<GeminiModel>,
}

#[derive(Debug, Clone, Serialize)]
struct GeminiModel {
    name: String,
    #[serde(rename = "displayName")]
    display_name: String,
    description: String,
    #[serde(rename = "supportedGenerationMethods")]
    supported_generation_methods: Vec<&'static str>,
    #[serde(rename = "inputTokenLimit")]
    input_token_limit: u32,
    #[serde(rename = "outputTokenLimit")]
    output_token_limit: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VoiceList {
    object: &'static str,
    data: Vec<Voice>,
}

#[derive(Debug, Clone, Serialize)]
struct Voice {}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct CountTokensResponse {
    input_tokens: usize,
}

pub(crate) const fn health(service: &'static str) -> Health {
    Health { ok: true, service }
}

pub(crate) fn model_info(id: &str, requested_kind: Option<&str>) -> Option<ModelInfo> {
    let (owner, model_name) = id.split_once('/')?;
    let kind = requested_kind
        .and_then(normalize_kind)
        .unwrap_or_else(|| infer_kind(model_name));

    Some(ModelInfo {
        id: id.to_owned(),
        name: model_name.to_owned(),
        kind,
        owned_by: owner.to_owned(),
        endpoint: endpoint_for_kind(kind),
    })
}

pub(crate) fn gemini_models() -> GeminiModelList {
    GeminiModelList {
        models: model_list()
            .data
            .into_iter()
            .map(|model| GeminiModel {
                name: format!("models/{}", model.id),
                display_name: model.id.clone(),
                description: format!("{} model: {}", model.owned_by, model.id),
                supported_generation_methods: vec!["generateContent", "streamGenerateContent"],
                input_token_limit: 128_000,
                output_token_limit: 8_192,
            })
            .collect(),
    }
}

pub(crate) const fn voices() -> VoiceList {
    VoiceList {
        object: "list",
        data: Vec::new(),
    }
}

pub(crate) const fn count_tokens(input_tokens: usize) -> CountTokensResponse {
    CountTokensResponse { input_tokens }
}

fn normalize_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "llm" | "chat" | "imageToText" => Some("llm"),
        "image" => Some("image"),
        "tts" => Some("tts"),
        "stt" => Some("stt"),
        "embedding" => Some("embedding"),
        "webSearch" | "search" => Some("webSearch"),
        "webFetch" | "fetch" => Some("webFetch"),
        _ => None,
    }
}

fn infer_kind(model_name: &str) -> &'static str {
    if model_name.contains("dall-e") {
        "image"
    } else if model_name.contains("embedding") {
        "embedding"
    } else if model_name.contains("tts") {
        "tts"
    } else if model_name.contains("whisper") {
        "stt"
    } else if model_name == "search" {
        "webSearch"
    } else if model_name == "fetch" {
        "webFetch"
    } else {
        "llm"
    }
}

fn endpoint_for_kind(kind: &str) -> &'static str {
    match kind {
        "image" => "/v1/images/generations",
        "tts" => "/v1/audio/speech",
        "stt" => "/v1/audio/transcriptions",
        "embedding" => "/v1/embeddings",
        "webSearch" => "/v1/search",
        "webFetch" => "/v1/web/fetch",
        _ => "/v1/chat/completions",
    }
}
