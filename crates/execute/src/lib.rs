//! Provider execution for the nullrouter Rust port.
//!
//! Turns a resolved provider/model plus credentials into a real upstream HTTP
//! call, then translates the response back into the client's wire format.
//!
//! Layering mirrors upstream `open-sse`:
//! - [`credentials`] — auth descriptors and URL resolution (`executors/default.js`)
//! - [`executor`] — dispatch with URL fallback and retry (`executors/base.js`)
//! - [`stream`] — response translation and framing (`handlers/chatCore/*`)
//! - [`errors`] — error classification and cooldown policy (`utils/error.js`)
//! - [`probe`] — asking a user-added provider which models it serves (`services/model.js`)
//!
//! Providers whose upstream protocol is not OpenAI-, Claude-, or Gemini-shaped
//! require bespoke executors that are not ported; [`is_executor_supported`]
//! reports which, so callers can return an explicit error instead of a wrong
//! answer.

pub mod bespoke;
pub mod credentials;
pub mod errors;
pub mod executor;
pub mod probe;
pub mod refresh;
pub mod stream;

use nullrouter_providers::Format;

pub use credentials::Credentials;
pub use errors::{FallbackDecision, UpstreamError, build_error_body, check_fallback_error};
pub use executor::{
    ExecuteError, ExecuteOutcome, ExecuteRequest, Executor, PreparedRequest, RawRequest, prepare,
};
pub use probe::{ProbeCache, ProbeError, ProbedModel, probe_models};
pub use refresh::{RefreshCache, RefreshError, Refreshed};
pub use stream::{
    ClientFraming, StreamSummary, collapse_stream_to_json, pipe_binary_stream, pipe_stream,
};

/// Provider formats this port can actually execute.
///
/// The excluded format needs a binary protocol this executor cannot express (`kiro`).
///
/// `ollama`, `gemini-cli`, and `commandcode` are included. None of them needs a
/// distinct executor: what they need is an envelope, a per-request header, or a URL
/// suffix, which [`crate::bespoke`] supplies as hooks on the shared path.
pub const fn is_format_supported(format: Format) -> bool {
    matches!(
        format,
        Format::OpenAi
            | Format::OpenAiResponses
            | Format::Claude
            | Format::Gemini
            | Format::Vertex
            | Format::Ollama
            | Format::GeminiCli
            | Format::CommandCode
            | Format::GrokWeb
            | Format::PerplexityWeb
            | Format::Antigravity
            | Format::Cursor
    )
}

/// `true` when a provider can be executed by the generic HTTP executor.
pub fn is_executor_supported(provider: &str) -> bool {
    is_format_supported(nullrouter_providers::target_format(provider))
}

/// Message returned for a provider whose bespoke executor is not ported.
pub fn unsupported_executor_message(provider: &str) -> String {
    let format = nullrouter_providers::target_format(provider);
    format!(
        "Provider '{provider}' uses the '{}' protocol, which requires a provider-specific \
         executor that is not implemented in this Rust port. Providers on the OpenAI-compatible, \
         Anthropic-compatible, and Gemini protocols are supported.",
        format.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::{is_executor_supported, is_format_supported, unsupported_executor_message};
    use nullrouter_providers::Format;

    #[test]
    fn mainstream_formats_are_executable() {
        assert!(is_format_supported(Format::OpenAi));
        assert!(is_format_supported(Format::Claude));
        assert!(is_format_supported(Format::Gemini));
        assert!(is_format_supported(Format::OpenAiResponses));
        // Ollama's executor is the default one with a different base URL, and both
        // its NDJSON framing and its wire shape are ported.
        assert!(is_format_supported(Format::Ollama));
        assert!(is_executor_supported("ollama-local"));
        assert!(is_executor_supported("ollama"));
        // These two need an envelope, a header, or a URL suffix — not an executor.
        assert!(is_format_supported(Format::GeminiCli));
        assert!(is_format_supported(Format::CommandCode));
        assert!(is_executor_supported("gemini-cli"));
        assert!(is_executor_supported("commandcode"));
    }

    #[test]
    fn bespoke_formats_are_refused() {
        // Each of these needs provider-specific request signing or a binary
        // protocol, which no hook on the shared path can supply.
        // One format is left. `kiro` signs its requests against AWS and speaks an event-stream protocol,
        // neither of which a hook on the shared path can supply.
        assert!(
            !is_format_supported(Format::Kiro),
            "kiro must still be refused"
        );
    }

    #[test]
    fn grok_web_dispatches_on_its_own_protocol() {
        // Ported from `open-sse/executors/grok-web.js`: the request is grok.com's own payload, the
        // credential is a session cookie, and the response is NDJSON rather than SSE. All three are
        // expressed as hooks on the shared path rather than as a separate executor.
        assert!(is_format_supported(Format::GrokWeb));
        assert!(is_executor_supported("grok-web"));
    }

    #[test]
    fn perplexity_web_dispatches_on_its_own_protocol() {
        // Ported from `open-sse/executors/perplexity-web.js`: the query is a JSON document in one
        // string field, follow-ups are continued server-side by thread id, and the answer arrives as
        // whole-answer SSE blocks rather than deltas.
        assert!(is_format_supported(Format::PerplexityWeb));
        assert!(is_executor_supported("perplexity-web"));
    }

    #[test]
    fn antigravity_dispatches_on_its_own_protocol() {
        // Ported from `open-sse/executors/antigravity.js`: Cloud Code Assist reached as the Antigravity
        // IDE reaches it, so the Gemini payload takes an envelope with an IDE-shaped request id, tool
        // groups are merged into one, and thinking fields are stripped at both levels.
        assert!(is_format_supported(Format::Antigravity));
        assert!(is_executor_supported("antigravity"));
    }

    #[test]
    fn cursor_dispatches_on_its_own_binary_protocol() {
        // Ported from `open-sse/executors/cursor.js`: Connect-RPC carrying protobuf, so the request is
        // bytes rather than JSON. Only the `ChatService` endpoint is ported — `AgentService` needs HTTP/2
        // duplex, which this executor's request/response shape has no room for.
        assert!(is_format_supported(Format::Cursor));
        assert!(is_executor_supported("cursor"));
    }

    #[test]
    fn common_providers_resolve_as_executable() {
        for provider in [
            "openai",
            "anthropic",
            "gemini",
            "groq",
            "deepseek",
            "openrouter",
            "mistral",
            "cerebras",
            "together",
            "xai",
        ] {
            assert!(is_executor_supported(provider), "{provider} should execute");
        }
    }

    #[test]
    fn bespoke_providers_are_reported_unsupported_with_a_clear_message() {
        assert!(!is_executor_supported("kiro"));
        let message = unsupported_executor_message("kiro");
        assert!(message.contains("kiro"), "{message}");
        assert!(message.contains("not implemented"), "{message}");
    }

    #[test]
    fn the_supported_set_covers_most_of_the_registry() {
        let with_transport: Vec<&str> = nullrouter_providers::entries()
            .iter()
            .filter(|entry| entry.transport.is_some())
            .map(|entry| entry.id.as_str())
            .collect();
        let supported = with_transport
            .iter()
            .filter(|provider| is_executor_supported(provider))
            .count();
        // The bespoke formats are a small minority of the registry.
        assert!(
            supported * 4 > with_transport.len() * 3,
            "expected >75% executable, got {supported}/{}",
            with_transport.len()
        );
    }
}
