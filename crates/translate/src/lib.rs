//! Request/response translation between the OpenAI, Claude, and Gemini wire
//! formats.
//!
//! Mirrors `open-sse/translator/index.js`: translation pivots through OpenAI
//! (`source -> openai -> target`) unless a direct pair is registered. Response
//! translation is incremental and carries [`state::StreamState`] across chunks.
//!
//! Ported pairs: `openai <-> claude`, `openai <-> gemini` (the latter also
//! covers `gemini-cli`, `vertex`, and `antigravity`, which share Gemini's response
//! shape), `openai <-> ollama`, and `commandcode -> openai`. The `kiro`, `cursor`,
//! `grok-web`, and `perplexity-web` formats need provider-specific request signing
//! and are not ported; requests for them pass through untranslated.

pub mod body;
pub mod concerns;
pub mod request;
pub mod response;
pub mod schema;
pub mod sse;
pub mod state;
pub mod thinking;

use std::collections::BTreeMap;

use nullrouter_providers::Format;
use serde_json::Value;

pub use body::translate_body;
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

/// Where a request came from and where it is going.
///
/// Grouped rather than passed loose because all four are read together to decide
/// the outbound shape, and a bare `(Format, Format, &str, &str)` argument list is
/// easy to transpose at a call site.
#[derive(Debug, Clone, Copy)]
pub struct RequestRoute<'a> {
    /// The wire format the client spoke.
    pub source: Format,
    /// The wire format the provider expects.
    pub target: Format,
    /// Resolved provider id, for per-model capability lookup.
    pub provider: &'a str,
    /// Upstream model id. May carry a `(...)` thinking suffix.
    pub model: &'a str,
}

/// Translate a request body from `source` to `target`.
///
/// `model` is the upstream model id to embed. It may carry a `(...)` thinking
/// suffix, which is stripped before the id reaches the provider. `provider` is
/// the resolved provider id, used to look up per-model thinking capabilities.
/// `ceiling` bounds `max_tokens`; pass [`schema::DEFAULT_MAX_TOKENS`] when the
/// model's real cap is unknown.
///
/// Thinking intent is read from `body` before translation and re-applied in the
/// target provider's native shape after, because each request translator only
/// carries the reasoning fields its own format owns.
#[allow(
    clippy::match_same_arms,
    reason = "the explicit OpenAI arm documents the pivot format, distinct from the unported fallback"
)]
pub fn translate_request(
    route: RequestRoute<'_>,
    body: &Value,
    stream: bool,
    ceiling: u64,
) -> TranslatedRequest {
    let RequestRoute {
        source,
        target,
        provider,
        model,
    } = route;
    // Captured before any translation: the translators drop the spellings they
    // do not own, so this is the last point the original intent is readable.
    let intent = thinking::extract_thinking(body);
    // The suffix is nullrouter's own routing syntax; a provider would reject it.
    let upstream_model = thinking::strip_thinking_suffix(model);

    if formats_equivalent(source, target) {
        let mut passthrough = body.clone();
        if let Some(object) = passthrough.as_object_mut() {
            object.insert("model".to_owned(), Value::String(upstream_model.to_owned()));
        }
        // Applied even on the passthrough path: a same-format request still needs
        // its reasoning fields in the shape this particular model reads.
        thinking::apply(target, provider, model, &mut passthrough, intent.as_ref());
        return TranslatedRequest::plain(passthrough);
    }

    // Step 1: source -> OpenAI.
    let intermediate = match source {
        Format::OpenAi => body.clone(),
        Format::Claude => request::claude_to_openai::translate(upstream_model, body, stream),
        Format::Gemini | Format::GeminiCli | Format::Vertex => {
            request::gemini_to_openai::translate(upstream_model, body, stream)
        }
        // The Responses API carries `input[]`/`instructions` rather than
        // `messages[]`, so it needs its own regrouping pass.
        Format::OpenAiResponses => {
            request::responses_to_openai::translate(upstream_model, body, stream)
        }
        // No source translator: hand the body to the target step unchanged.
        _ => body.clone(),
    };

    // Step 2: OpenAI -> target.
    let mut translated = match target {
        Format::Claude => {
            let translated = request::openai_to_claude::translate(
                upstream_model,
                &intermediate,
                stream,
                ceiling,
            );
            TranslatedRequest {
                body: translated.body,
                tool_name_map: translated.tool_name_map,
            }
        }
        Format::Gemini | Format::GeminiCli | Format::Vertex => TranslatedRequest::plain(
            request::openai_to_gemini::translate(upstream_model, &intermediate, stream),
        ),
        Format::Ollama => TranslatedRequest::plain(request::openai_to_ollama::translate(
            upstream_model,
            &intermediate,
            stream,
        )),
        // CommandCode reads OpenAI's own request shape; only its responses differ.
        // The provider forces streaming, so `stream` is already true here.
        Format::CommandCode => {
            let mut result = intermediate;
            if let Some(object) = result.as_object_mut() {
                object.insert("model".to_owned(), Value::String(upstream_model.to_owned()));
                object.insert("stream".to_owned(), Value::Bool(stream));
            }
            TranslatedRequest::plain(result)
        }
        Format::OpenAi | Format::OpenAiResponses => {
            let mut result = intermediate;
            if let Some(object) = result.as_object_mut() {
                object.insert("model".to_owned(), Value::String(upstream_model.to_owned()));
                object.insert("stream".to_owned(), Value::Bool(stream));
            }
            TranslatedRequest::plain(result)
        }
        // Unported target format: pass the OpenAI-shaped body through.
        _ => TranslatedRequest::plain(intermediate),
    };

    // Step 3: re-apply the captured intent in the target's native shape. Runs
    // last so it writes over whatever the translators carried across.
    thinking::apply(
        target,
        provider,
        model,
        &mut translated.body,
        intent.as_ref(),
    );
    translated
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
        Format::Ollama => response::ollama_to_openai::translate(chunk, state),
        Format::GrokWeb => response::grok_web_to_openai::translate(chunk, state),
        Format::CommandCode => response::commandcode_to_openai::translate(chunk, state),
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

/// Terminal chunks an *upstream* format needs synthesized once its body is exhausted.
///
/// Distinct from [`finalize_response`], which serves the *client* format. This one exists because some
/// upstreams never state that they finished: grok.com closes the connection with no finish reason, and
/// a client waiting for one hangs on a reply that has fully arrived.
///
/// The returned chunks are already in `source` shape, so the caller sends them as-is. Translating here
/// rather than at the call site keeps the OpenAI-intermediate step internal: the synthesized chunk is
/// created OpenAI-shaped, and running the upstream translator over it again would read it as an
/// upstream event and yield nothing.
pub fn finalize_upstream(target: Format, source: Format, state: &mut StreamState) -> Vec<Value> {
    let synthesized = match target {
        Format::GrokWeb => vec![response::grok_web_to_openai::finish(state)],
        _ => Vec::new(),
    };
    if synthesized.is_empty() || formats_equivalent(source, Format::OpenAi) {
        return synthesized;
    }
    // OpenAI -> client, the same second step `translate_response` applies.
    match source {
        Format::Claude => synthesized
            .iter()
            .flat_map(|chunk| response::openai_to_claude::translate(chunk, state))
            .collect(),
        Format::OpenAiResponses => synthesized
            .iter()
            .flat_map(|chunk| response::openai_to_responses::translate(Some(chunk), state))
            .map(|event| event.data)
            .collect(),
        _ => synthesized,
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
