//! Request/response translation between the OpenAI, Claude, and Gemini wire
//! formats.
//!
//! Mirrors `open-sse/translator/index.js`: translation pivots through OpenAI
//! (`source -> openai -> target`) unless a direct pair is registered. Response
//! translation is incremental and carries [`state::StreamState`] across chunks.
//!
//! Ported pairs: `openai <-> claude` and `openai <-> gemini` (the latter also
//! covers `gemini-cli`, `vertex`, and `antigravity` on the response side, which
//! share Gemini's response shape). The bespoke `kiro`, `cursor`, `commandcode`,
//! and `ollama` formats belong to custom executors and are not ported; requests
//! for them pass through untranslated.

pub mod concerns;
pub mod request;
pub mod response;
pub mod schema;
pub mod sse;
pub mod state;

use std::collections::BTreeMap;

use nullrouter_providers::Format;
use serde_json::Value;

pub use concerns::{Usage, UsageKind};
pub use state::{Clock, StreamState};

/// A translated request plus any tool-name mapping the response side needs.
#[derive(Debug, Clone)]
pub struct TranslatedRequest {
    pub body: Value,
    /// Renamed tool name -> original name (empty when no renaming occurred).
    pub tool_name_map: BTreeMap<String, String>,
}

impl TranslatedRequest {
    const fn plain(body: Value) -> Self {
        Self {
            body,
            tool_name_map: BTreeMap::new(),
        }
    }
}

/// `true` when the two formats differ (upstream `needsTranslation`).
pub const fn needs_translation(source: Format, target: Format) -> bool {
    !formats_equivalent(source, target)
}

/// Formats that share a wire shape and need no translation between them.
const fn formats_equivalent(source: Format, target: Format) -> bool {
    matches!(
        (source, target),
        (Format::OpenAi, Format::OpenAi)
            | (Format::Claude, Format::Claude)
            | (Format::Gemini, Format::Gemini)
            | (Format::OpenAiResponses, Format::OpenAiResponses)
    )
}

/// Translate a request body from `source` to `target`.
///
/// `model` is the upstream model id to embed. `ceiling` bounds `max_tokens`;
/// pass [`schema::DEFAULT_MAX_TOKENS`] when the model's real cap is unknown.
#[allow(
    clippy::match_same_arms,
    reason = "the explicit OpenAI arm documents the pivot format, distinct from the unported fallback"
)]
pub fn translate_request(
    source: Format,
    target: Format,
    model: &str,
    body: &Value,
    stream: bool,
    ceiling: u64,
) -> TranslatedRequest {
    if formats_equivalent(source, target) {
        let mut passthrough = body.clone();
        if let Some(object) = passthrough.as_object_mut() {
            object.insert("model".to_owned(), Value::String(model.to_owned()));
        }
        return TranslatedRequest::plain(passthrough);
    }

    // Step 1: source -> OpenAI.
    let intermediate = match source {
        Format::OpenAi => body.clone(),
        Format::Claude => request::claude_to_openai::translate(model, body, stream),
        Format::Gemini | Format::GeminiCli | Format::Vertex => {
            request::gemini_to_openai::translate(model, body, stream)
        }
        // The Responses API carries `input[]`/`instructions` rather than
        // `messages[]`, so it needs its own regrouping pass.
        Format::OpenAiResponses => request::responses_to_openai::translate(model, body, stream),
        // No source translator: hand the body to the target step unchanged.
        _ => body.clone(),
    };

    // Step 2: OpenAI -> target.
    match target {
        Format::Claude => {
            let translated =
                request::openai_to_claude::translate(model, &intermediate, stream, ceiling);
            TranslatedRequest {
                body: translated.body,
                tool_name_map: translated.tool_name_map,
            }
        }
        Format::Gemini | Format::GeminiCli | Format::Vertex => TranslatedRequest::plain(
            request::openai_to_gemini::translate(model, &intermediate, stream),
        ),
        Format::OpenAi | Format::OpenAiResponses => {
            let mut result = intermediate;
            if let Some(object) = result.as_object_mut() {
                object.insert("model".to_owned(), Value::String(model.to_owned()));
                object.insert("stream".to_owned(), Value::Bool(stream));
            }
            TranslatedRequest::plain(result)
        }
        // Unported target format: pass the OpenAI-shaped body through.
        _ => TranslatedRequest::plain(intermediate),
    }
}

/// Translate one upstream response chunk from `target` back to `source`.
///
/// Returns zero or more chunks in the client's format.
#[allow(
    clippy::match_same_arms,
    reason = "the explicit OpenAI arm documents the pivot format, distinct from the unported fallback"
)]
pub fn translate_response(
    target: Format,
    source: Format,
    chunk: &Value,
    state: &mut StreamState,
) -> Vec<Value> {
    if formats_equivalent(source, target) {
        return vec![chunk.clone()];
    }

    // Step 1: target -> OpenAI.
    let intermediate: Vec<Value> = match target {
        Format::OpenAi => vec![chunk.clone()],
        Format::Claude => response::claude_to_openai::translate(chunk, state),
        Format::Gemini | Format::GeminiCli | Format::Vertex | Format::Antigravity => {
            response::gemini_to_openai::translate(chunk, state)
        }
        // Unported upstream format: forward as-is.
        _ => vec![chunk.clone()],
    };

    // Step 2: OpenAI -> source.
    match source {
        Format::Claude => intermediate
            .iter()
            .flat_map(|chunk| response::openai_to_claude::translate(chunk, state))
            .collect(),
        // The Responses API emits named lifecycle events, not chunks. Each
        // event's payload is returned here and the caller reads its `type` to
        // frame it — see `ClientFraming::ResponsesEvents`.
        Format::OpenAiResponses => intermediate
            .iter()
            .flat_map(|chunk| response::openai_to_responses::translate(Some(chunk), state))
            .map(|event| event.data)
            .collect(),
        _ => intermediate,
    }
}

/// Emit the terminal frames a format requires after the upstream stream ends.
///
/// The Responses API needs every open item closed and `response.completed`
/// emitted; a client hangs without it. Other formats need nothing extra.
pub fn finalize_response(source: Format, state: &mut StreamState) -> Vec<Value> {
    if source == Format::OpenAiResponses {
        return response::openai_to_responses::flush(state)
            .into_iter()
            .map(|event| event.data)
            .collect();
    }
    Vec::new()
}

/// Fresh streaming state for a translation.
pub const fn init_state(clock: Clock) -> StreamState {
    StreamState::new(clock)
}
