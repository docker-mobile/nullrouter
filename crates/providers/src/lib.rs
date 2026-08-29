//! Provider registry, model resolution, and wire-format detection for the
//! nullrouter Rust port.
//!
//! The registry data is dumped verbatim from the frozen 9Router reference in
//! `inspire/open-sse/providers/registry/` so transports, auth descriptors, and
//! model tables stay faithful to upstream rather than hand-transcribed.

pub mod capabilities;
pub mod format;
pub mod model;
pub mod models_list;
pub mod registry;
pub mod services;

pub use capabilities::{
    Capabilities, ThinkingFormat, ThinkingRange, for_model as capabilities_for_model, max_output,
    thinking_levels,
};
pub use format::{
    ANTHROPIC_COMPAT_BASE, Format, OPENAI_COMPAT_BASE, detect_format, detect_format_by_endpoint,
    is_anthropic_compatible, is_openai_compatible, resolve_transport, target_format,
};
pub use model::{
    ModelTarget, ParsedModel, infer_provider_from_model_name, infer_target, model_strip_list,
    model_target_format, parse_model, resolve_model_alias, resolve_target, split_thinking_suffix,
    upstream_model_id,
};
pub use models_list::{
    ComboView, ConnectionView, LLM_KIND, ModelRow, ModelsListInput, build_models_list,
};
pub use registry::{
    Auth, AuthSpec, Model, OAuth, Quirks, RegistryEntry, RetryEntry, Transport, entries, entry,
    find_model, models_for_provider, normalize_model_id, resolve_provider_id, transport,
};
pub use services::{
    ServiceEndpoint, ServiceKind, providers_for_service, service_endpoint, supports_service,
};

#[cfg(test)]
mod tests {
    use super::{Format, detect_format, detect_format_by_endpoint, parse_model, target_format};
    use serde_json::json;

    #[test]
    fn responses_api_detected_from_input_field() {
        let body = json!({ "model": "openai/gpt-5", "input": [{ "role": "user" }] });
        assert_eq!(detect_format(&body), Format::OpenAiResponses);

        // `input` as a plain string is also Responses API.
        let text_input = json!({ "model": "openai/gpt-5", "input": "hello" });
        assert_eq!(detect_format(&text_input), Format::OpenAiResponses);

        // With `messages` present it is not Responses API.
        let both = json!({ "input": [], "messages": [] });
        assert_ne!(detect_format(&both), Format::OpenAiResponses);
    }

    #[test]
    fn gemini_detected_from_contents_array() {
        let body = json!({ "contents": [{ "role": "user", "parts": [] }] });
        assert_eq!(detect_format(&body), Format::Gemini);
    }

    #[test]
    fn antigravity_requires_wrapper_and_user_agent() {
        let body = json!({
            "request": { "contents": [{ "role": "user" }] },
            "userAgent": "antigravity",
        });
        assert_eq!(detect_format(&body), Format::Antigravity);

        // Without the userAgent marker it is not antigravity.
        let no_marker = json!({ "request": { "contents": [{ "role": "user" }] } });
        assert_ne!(detect_format(&no_marker), Format::Antigravity);
    }

    #[test]
    fn openai_specific_fields_win_over_claude_shape() {
        // Claude-looking content blocks, but `n` is OpenAI-only and is checked first.
        let body = json!({
            "system": "be brief",
            "n": 1,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }] }],
        });
        assert_eq!(detect_format(&body), Format::OpenAi);

        // `logprobs: null` still counts as present (JS `!== undefined`).
        let explicit_null = json!({
            "system": "be brief",
            "logprobs": null,
            "messages": [{ "role": "user", "content": "hi" }],
        });
        assert_eq!(detect_format(&explicit_null), Format::OpenAi);

        // `user: ""` is falsy in JS and must NOT force OpenAI.
        let empty_user = json!({
            "user": "",
            "system": "be brief",
            "messages": [{ "role": "user", "content": "hi" }],
        });
        assert_eq!(detect_format(&empty_user), Format::Claude);
    }

    #[test]
    fn claude_detected_from_system_and_block_shapes() {
        let with_system = json!({
            "system": "be brief",
            "messages": [{ "role": "user", "content": "hi" }],
        });
        assert_eq!(detect_format(&with_system), Format::Claude);

        let base64_image = json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "look" },
                    { "type": "image", "source": { "type": "base64", "data": "AA" } },
                ],
            }],
        });
        assert_eq!(detect_format(&base64_image), Format::Claude);

        let tool_use = json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "go" },
                    { "type": "tool_result", "tool_use_id": "t1" },
                ],
            }],
        });
        assert_eq!(detect_format(&tool_use), Format::Claude);
    }

    #[test]
    fn openai_image_url_blocks_detect_as_openai() {
        let body = json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "look" },
                    { "type": "image_url", "image_url": { "url": "https://example.test/a.png" } },
                ],
            }],
        });
        assert_eq!(detect_format(&body), Format::OpenAi);
    }

    #[test]
    fn slashed_model_skips_the_ambiguous_block_probe() {
        // A `provider/model` id makes the text-block branch bail out; with no
        // Claude marker left, this falls through to OpenAI.
        let body = json!({
            "model": "openai/gpt-5",
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }] }],
        });
        assert_eq!(detect_format(&body), Format::OpenAi);
    }

    #[test]
    fn endpoint_detection_overrides_body_shape() {
        let body = json!({ "messages": [] });
        assert_eq!(
            detect_format_by_endpoint("/v1/messages", &body),
            Some(Format::Claude)
        );
        assert_eq!(
            detect_format_by_endpoint("/v1/responses", &body),
            Some(Format::OpenAiResponses)
        );
        assert_eq!(detect_format_by_endpoint("/v1/embeddings", &body), None);

        // Cursor CLI posts a Responses-shaped body to the chat endpoint.
        let input_body = json!({ "input": [] });
        assert_eq!(
            detect_format_by_endpoint("/v1/chat/completions", &input_body),
            Some(Format::OpenAi)
        );
    }

    #[test]
    fn target_format_follows_registry_and_compatible_families() {
        assert_eq!(target_format("openai"), Format::OpenAi);
        assert_eq!(target_format("anthropic"), Format::Claude);
        assert_eq!(target_format("gemini"), Format::Gemini);
        assert_eq!(target_format("openai-compatible-abc"), Format::OpenAi);
        assert_eq!(
            target_format("openai-compatible-responses-abc"),
            Format::OpenAiResponses
        );
        assert_eq!(target_format("anthropic-compatible-abc"), Format::Claude);
        // Unknown providers default to OpenAI.
        assert_eq!(target_format("nope"), Format::OpenAi);
    }

    #[test]
    fn model_strings_split_on_the_first_slash_only() {
        let parsed = parse_model("openrouter/deepseek/deepseek-chat");
        assert_eq!(parsed.provider.as_deref(), Some("openrouter"));
        assert_eq!(parsed.model, "deepseek/deepseek-chat");
        assert!(!parsed.is_alias);

        let aliased = parse_model("cc/claude-sonnet-4.5");
        assert_eq!(aliased.provider.as_deref(), Some("claude"));
        assert_eq!(aliased.provider_alias.as_deref(), Some("cc"));

        let bare = parse_model("my-alias");
        assert!(bare.is_alias);
        assert_eq!(bare.provider, None);
        assert_eq!(bare.model, "my-alias");
    }
}
