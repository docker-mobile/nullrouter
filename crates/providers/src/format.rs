//! Wire-format identification.
//!
//! Ports `open-sse/translator/formats.js` and the detection half of
//! `open-sse/services/provider.js`. Detection precedence is load-bearing: the
//! OpenAI-specific field probe deliberately runs *before* the Claude shape
//! check, and JS truthiness vs `!== undefined` are distinguished exactly as
//! upstream does.

use serde_json::Value;

use crate::registry;

/// A request/response wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    OpenAi,
    OpenAiResponses,
    Claude,
    Gemini,
    GeminiCli,
    Vertex,
    Codex,
    Antigravity,
    Kiro,
    Cursor,
    Ollama,
    CommandCode,
    GrokWeb,
    PerplexityWeb,
}

impl Format {
    /// Upstream string identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::OpenAiResponses => "openai-responses",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::GeminiCli => "gemini-cli",
            Self::Vertex => "vertex",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::Kiro => "kiro",
            Self::Cursor => "cursor",
            Self::Ollama => "ollama",
            Self::CommandCode => "commandcode",
            Self::GrokWeb => "grok-web",
            Self::PerplexityWeb => "perplexity-web",
        }
    }

    /// Parse an upstream format identifier.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "openai" => Self::OpenAi,
            // `openai-response` is a legacy spelling kept in upstream FORMATS.
            "openai-responses" | "openai-response" => Self::OpenAiResponses,
            "claude" => Self::Claude,
            "gemini" => Self::Gemini,
            "gemini-cli" => Self::GeminiCli,
            "vertex" => Self::Vertex,
            "codex" => Self::Codex,
            "antigravity" => Self::Antigravity,
            "kiro" => Self::Kiro,
            "cursor" => Self::Cursor,
            "ollama" => Self::Ollama,
            "commandcode" => Self::CommandCode,
            "grok-web" => Self::GrokWeb,
            "perplexity-web" => Self::PerplexityWeb,
            _ => return None,
        })
    }

    /// Formats that always stream from the client's perspective
    /// (upstream `clientRequestedStreaming`).
    pub const fn always_streams(self) -> bool {
        matches!(self, Self::Antigravity | Self::Gemini | Self::GeminiCli)
    }
}

/// JS truthiness: present, non-null, not `false`, not `0`, not `""`.
fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// JS `!== undefined`: the key exists, even when its value is `null`.
fn is_present(body: &Value, key: &str) -> bool {
    body.get(key).is_some()
}

/// Detect the source format from a request body (upstream `detectFormat`).
pub fn detect_format(body: &Value) -> Format {
    // OpenAI Responses API: `input` (array or string) and no `messages`.
    let input_is_array_or_string =
        matches!(body.get("input"), Some(Value::Array(_) | Value::String(_)));
    if is_truthy(body.get("input")) && input_is_array_or_string && body.get("messages").is_none() {
        return Format::OpenAiResponses;
    }

    // Antigravity: Gemini payload wrapped in `request`, flagged by userAgent.
    if body
        .get("request")
        .and_then(|request| request.get("contents"))
        .is_some_and(|contents| is_truthy(Some(contents)))
        && body.get("userAgent").and_then(Value::as_str) == Some("antigravity")
    {
        return Format::Antigravity;
    }

    // Gemini: top-level `contents` array.
    if matches!(body.get("contents"), Some(Value::Array(_))) {
        return Format::Gemini;
    }

    // OpenAI-specific fields, checked BEFORE Claude on purpose.
    if is_truthy(body.get("stream_options"))
        || is_truthy(body.get("response_format"))
        || is_present(body, "logprobs")
        || is_present(body, "top_logprobs")
        || is_present(body, "n")
        || is_present(body, "presence_penalty")
        || is_present(body, "frequency_penalty")
        || is_truthy(body.get("logit_bias"))
        || is_truthy(body.get("user"))
    {
        return Format::OpenAi;
    }

    if let Some(Value::Array(messages)) = body.get("messages") {
        if let Some(format) = detect_from_messages(body, messages) {
            return format;
        }
        // String content is ambiguous; only Claude-specific markers decide.
        if is_present(body, "system") || is_present(body, "anthropic_version") {
            return Format::Claude;
        }
    }

    Format::OpenAi
}

/// Claude-vs-OpenAI disambiguation from the first message's content blocks.
fn detect_from_messages(body: &Value, messages: &[Value]) -> Option<Format> {
    let content = messages.first()?.get("content")?;
    let Value::Array(parts) = content else {
        return None;
    };
    let first_type = parts.first()?.get("type")?.as_str()?;
    let model_has_slash = body
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| model.contains('/'));
    if first_type != "text" || model_has_slash {
        return None;
    }

    if is_truthy(body.get("system")) || is_truthy(body.get("anthropic_version")) {
        return Some(Format::Claude);
    }

    let has_claude_image = parts.iter().any(|part| {
        part.get("type").and_then(Value::as_str) == Some("image")
            && part
                .get("source")
                .and_then(|source| source.get("type"))
                .and_then(Value::as_str)
                == Some("base64")
    });
    if has_claude_image {
        return Some(Format::Claude);
    }

    let has_openai_image = parts.iter().any(|part| {
        part.get("type").and_then(Value::as_str) == Some("image_url")
            && is_truthy(part.get("image_url").and_then(|image| image.get("url")))
    });
    if has_openai_image {
        return Some(Format::OpenAi);
    }

    let has_claude_tool = parts.iter().any(|part| {
        matches!(
            part.get("type").and_then(Value::as_str),
            Some("tool_use" | "tool_result")
        )
    });
    if has_claude_tool {
        return Some(Format::Claude);
    }

    None
}

/// Detect format from the request path (upstream `detectFormatByEndpoint`).
///
/// Returns `None` to fall back to body-based detection.
pub fn detect_format_by_endpoint(pathname: &str, body: &Value) -> Option<Format> {
    if pathname.contains("/v1/responses") {
        return Some(Format::OpenAiResponses);
    }
    if pathname.contains("/v1/messages") {
        return Some(Format::Claude);
    }
    // Cursor CLI posts a Responses-shaped body to the chat endpoint.
    if pathname.contains("/v1/chat/completions")
        && matches!(body.get("input"), Some(Value::Array(_)))
    {
        return Some(Format::OpenAi);
    }
    None
}

const OPENAI_COMPATIBLE_PREFIX: &str = "openai-compatible-";
const ANTHROPIC_COMPATIBLE_PREFIX: &str = "anthropic-compatible-";

/// Default base URL for the dynamic OpenAI-compatible family.
pub const OPENAI_COMPAT_BASE: &str = "https://api.openai.com/v1";
/// Default base URL for the dynamic Anthropic-compatible family.
pub const ANTHROPIC_COMPAT_BASE: &str = "https://api.anthropic.com/v1";

/// The provider id whose host comes from the connection.
pub const OLLAMA_LOCAL_PROVIDER: &str = "ollama-local";
/// Where a local Ollama listens when the connection names no host
/// (upstream `OLLAMA_LOCAL_DEFAULT_HOST`).
pub const OLLAMA_LOCAL_DEFAULT_HOST: &str = "http://localhost:11434";

/// `true` for user-defined `openai-compatible-*` providers.
pub fn is_openai_compatible(provider: &str) -> bool {
    provider.starts_with(OPENAI_COMPATIBLE_PREFIX)
}

/// `true` for user-defined `anthropic-compatible-*` providers.
pub fn is_anthropic_compatible(provider: &str) -> bool {
    provider.starts_with(ANTHROPIC_COMPATIBLE_PREFIX)
}

/// Target wire format for a provider (upstream `getTargetFormat`).
pub fn target_format(provider: &str) -> Format {
    if is_openai_compatible(provider) {
        return if provider.contains("responses") {
            Format::OpenAiResponses
        } else {
            Format::OpenAi
        };
    }
    if is_anthropic_compatible(provider) {
        return Format::Claude;
    }
    registry::transport(provider)
        .and_then(|transport| Format::parse(transport.format_or_default()))
        .unwrap_or(Format::OpenAi)
}

/// Pick the transport whose format matches the client's, avoiding a lossy
/// translation hop (upstream `resolveTransport`).
pub fn resolve_transport(
    provider: &str,
    source_format: Format,
) -> Option<&'static registry::Transport> {
    let transports = registry::entry(provider)?.transports.as_ref()?;
    transports
        .iter()
        .find(|transport| transport.format_or_default() == source_format.as_str())
}

/// Every format a provider can be addressed in directly.
///
/// The primary transport's format plus each declared alternative. Used to answer
/// "could this provider take a Claude request without translation" without the
/// caller reaching into the registry itself.
pub fn transport_formats(provider: &str) -> Vec<Format> {
    let Some(entry) = registry::entry(provider) else {
        return Vec::new();
    };
    let mut formats: Vec<Format> = Vec::new();
    let mut push = |format: Option<Format>| {
        if let Some(format) = format
            && !formats.contains(&format)
        {
            formats.push(format);
        }
    };
    push(
        entry
            .transport
            .as_ref()
            .and_then(|transport| Format::parse(transport.format_or_default())),
    );
    for transport in entry.transports.iter().flatten() {
        push(Format::parse(transport.format_or_default()));
    }
    formats
}

/// The transport to dispatch one request on (upstream's `useTransport`).
///
/// A multi-transport provider is addressed in the client's own format where it can
/// be, which removes the translation hop entirely — `deepseek` answers Claude
/// requests at `/anthropic/v1/messages`, so a Claude client reaching it through this
/// router should not have its body rewritten to OpenAI and back.
///
/// `model` gates that, and the gate is the whole reason this is not just
/// [`resolve_transport`]. `opencode-go` fronts several vendors on one host and its
/// `kimi`/`glm` models serve `/chat/completions` only; routing a Claude request there
/// to `/messages` would be a 404 on a provider that works. A model that declares no
/// `supportedFormats` is unrestricted, which is upstream's default and keeps
/// `deepseek`, `glm`, `kimi` and the rest behaving as they did.
///
/// `None` means "use the provider's primary transport", i.e. translate as before.
pub fn runtime_transport(
    provider: &str,
    model: &str,
    source_format: Format,
) -> Option<&'static registry::Transport> {
    let transport = resolve_transport(provider, source_format)?;
    if model_serves_format(provider, model, source_format) {
        Some(transport)
    } else {
        None
    }
}

/// Whether a model's own endpoint list covers this format.
///
/// `true` when the model declares nothing, so the absence of a declaration is not
/// read as a refusal.
pub fn model_serves_format(provider: &str, model: &str, format: Format) -> bool {
    let Some(model) = registry::find_model(provider, model) else {
        return true;
    };
    if model.supported_formats.is_empty() {
        return true;
    }
    model
        .supported_formats
        .iter()
        .any(|declared| declared == format.as_str())
}

#[cfg(test)]
mod transport_tests {
    use super::{
        Format, model_serves_format, resolve_transport, runtime_transport, transport_formats,
    };

    #[test]
    fn a_multi_transport_provider_exposes_every_declared_format() {
        // deepseek fronts one host with two endpoints: /chat/completions and
        // /anthropic/v1/messages. Both must be visible, or a Claude client is
        // translated to OpenAI and back for nothing.
        let formats = transport_formats("deepseek");
        assert!(formats.contains(&Format::OpenAi), "{formats:?}");
        assert!(formats.contains(&Format::Claude), "{formats:?}");
    }

    #[test]
    fn a_single_transport_provider_exposes_only_its_own_format() {
        let formats = transport_formats("openai");
        assert_eq!(formats, vec![Format::OpenAi]);
        // An unknown provider claims nothing rather than guessing OpenAI.
        assert!(transport_formats("no-such-provider").is_empty());
    }

    #[test]
    fn the_claude_endpoint_is_selected_for_a_claude_request() {
        let transport = resolve_transport("deepseek", Format::Claude).expect("claude transport");
        assert_eq!(
            transport.base_url.as_deref(),
            Some("https://api.deepseek.com/anthropic/v1/messages")
        );
        // And its own auth descriptor travels with it: the Claude endpoint wants
        // x-api-key, not the bearer the OpenAI endpoint takes.
        let auth = transport.auth.as_ref().expect("auth");
        assert_eq!(auth.header.as_deref(), Some("x-api-key"));

        let openai = resolve_transport("deepseek", Format::OpenAi).expect("openai transport");
        assert_eq!(
            openai.base_url.as_deref(),
            Some("https://api.deepseek.com/chat/completions")
        );
    }

    #[test]
    fn a_format_the_provider_does_not_serve_resolves_to_nothing() {
        // Gemini is not one of deepseek's endpoints; the caller then translates
        // through the primary transport as before.
        assert!(resolve_transport("deepseek", Format::Gemini).is_none());
    }

    #[test]
    fn a_model_that_declares_narrower_formats_gates_the_transport() {
        // opencode-go fronts several vendors on one host. Its kimi and glm models
        // serve /chat/completions only; routing a Claude request to /messages there
        // is a 404 on a provider that works.
        assert!(model_serves_format(
            "opencode-go",
            "glm-5.2",
            Format::OpenAi
        ));
        assert!(!model_serves_format(
            "opencode-go",
            "glm-5.2",
            Format::Claude
        ));
        assert!(
            runtime_transport("opencode-go", "glm-5.2", Format::Claude).is_none(),
            "a Claude request on a chat-completions-only model must not take the /messages endpoint"
        );
        // A model on the same provider that does declare Claude gets it.
        assert!(model_serves_format(
            "opencode-go",
            "deepseek-v4-pro",
            Format::Claude
        ));
        assert!(runtime_transport("opencode-go", "deepseek-v4-pro", Format::Claude).is_some());
    }

    #[test]
    fn a_model_declaring_nothing_is_unrestricted() {
        // Upstream's default, and what keeps deepseek/glm/kimi behaving as before.
        assert!(model_serves_format(
            "deepseek",
            "deepseek-v4-pro",
            Format::Claude
        ));
        assert!(runtime_transport("deepseek", "deepseek-v4-pro", Format::Claude).is_some());
        // An unknown model is likewise not treated as a refusal.
        assert!(model_serves_format(
            "deepseek",
            "not-a-model",
            Format::Claude
        ));
    }

    #[test]
    fn every_declared_transport_format_is_selectable() {
        // Enumerates the registry rather than naming providers, so a new
        // multi-transport entry is covered the day it lands.
        let mut checked = 0;
        for entry in crate::registry::entries() {
            let Some(transports) = entry.transports.as_ref() else {
                continue;
            };
            for transport in transports {
                let Some(format) = Format::parse(transport.format_or_default()) else {
                    panic!(
                        "{}: transport format {:?} is unparsable",
                        entry.id, transport.format
                    );
                };
                let resolved = resolve_transport(&entry.id, format);
                assert!(
                    resolved.is_some(),
                    "{}: declared {format:?} but it is not selectable",
                    entry.id
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 17,
            "expected the known multi-transport set, saw {checked}"
        );
    }
}
