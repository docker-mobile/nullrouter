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
//!
//! Providers whose upstream protocol is not OpenAI-, Claude-, or Gemini-shaped
//! require bespoke executors that are not ported; [`is_executor_supported`]
//! reports which, so callers can return an explicit error instead of a wrong
//! answer.

pub mod credentials;
pub mod errors;
pub mod executor;
pub mod stream;

use nullrouter_providers::Format;

pub use credentials::Credentials;
pub use errors::{FallbackDecision, UpstreamError, build_error_body, check_fallback_error};
pub use executor::{ExecuteError, ExecuteOutcome, ExecuteRequest, Executor, RawRequest};
pub use stream::{ClientFraming, StreamSummary, collapse_stream_to_json, pipe_stream};

/// Provider formats this port can actually execute.
///
/// The excluded formats need provider-specific request signing, envelopes, or
/// binary protocols (`kiro`, `cursor`, `commandcode`, `grok-web`,
/// `perplexity-web`, `codex`, `antigravity`, `gemini-cli`, `ollama`).
pub const fn is_format_supported(format: Format) -> bool {
    matches!(
        format,
        Format::OpenAi | Format::OpenAiResponses | Format::Claude | Format::Gemini | Format::Vertex
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
    }

    #[test]
    fn bespoke_formats_are_refused() {
        for format in [
            Format::Kiro,
            Format::Cursor,
            Format::CommandCode,
            Format::GrokWeb,
            Format::PerplexityWeb,
            Format::Codex,
            Format::Antigravity,
            Format::GeminiCli,
            Format::Ollama,
        ] {
            assert!(!is_format_supported(format), "{format:?} must be refused");
        }
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
