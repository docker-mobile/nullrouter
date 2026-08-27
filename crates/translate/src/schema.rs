//! Pure data enums and token-limit arithmetic shared by all translators.
//!
//! Ports `open-sse/translator/schema/*` and
//! `open-sse/translator/formats/maxTokens.js`.

use serde_json::Value;

/// Message roles (OpenAI chat and Claude share these).
pub mod role {
    pub const USER: &str = "user";
    pub const ASSISTANT: &str = "assistant";
    pub const TOOL: &str = "tool";
    pub const SYSTEM: &str = "system";
    pub const DEVELOPER: &str = "developer";
}

/// Gemini uses `model` where OpenAI/Claude use `assistant`.
pub mod gemini_role {
    pub const USER: &str = "user";
    pub const MODEL: &str = "model";
}

/// OpenAI content-block discriminators.
pub mod openai_block {
    pub const TEXT: &str = "text";
    pub const IMAGE_URL: &str = "image_url";
    pub const IMAGE: &str = "image";
    pub const INPUT_AUDIO: &str = "input_audio";
    pub const AUDIO_URL: &str = "audio_url";
    pub const FILE: &str = "file";
    pub const FUNCTION: &str = "function";
}

/// Claude content-block discriminators.
pub mod claude_block {
    pub const TEXT: &str = "text";
    pub const IMAGE: &str = "image";
    pub const DOCUMENT: &str = "document";
    pub const TOOL_USE: &str = "tool_use";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const THINKING: &str = "thinking";
    pub const REDACTED_THINKING: &str = "redacted_thinking";
}

/// OpenAI `finish_reason` values — the hub format.
pub mod openai_finish {
    pub const STOP: &str = "stop";
    pub const LENGTH: &str = "length";
    pub const TOOL_CALLS: &str = "tool_calls";
    pub const CONTENT_FILTER: &str = "content_filter";
}

/// Claude `stop_reason` values.
pub mod claude_stop {
    pub const END_TURN: &str = "end_turn";
    pub const MAX_TOKENS: &str = "max_tokens";
    pub const TOOL_USE: &str = "tool_use";
    pub const STOP_SEQUENCE: &str = "stop_sequence";
}

/// Fallback model id when an upstream chunk omits one.
pub const MODEL_FALLBACK: &str = "unknown";
/// Default image mime for base64 blobs with no declared type.
pub const DEFAULT_IMAGE_MIME: &str = "image/png";

/// Upstream `DEFAULT_MAX_TOKENS`.
pub const DEFAULT_MAX_TOKENS: u64 = 64000;
/// Upstream `DEFAULT_MIN_TOKENS` — floor when tools are present.
pub const DEFAULT_MIN_TOKENS: u64 = 32000;

/// The Claude Code system prompt injected on OpenAI -> Claude translation.
pub const CLAUDE_SYSTEM_PROMPT: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Anthropic API version sent on Claude-format requests.
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Adjust `max_tokens` for request context (upstream `adjustMaxTokens`).
///
/// Raises the floor when tools are present, keeps `max_tokens` strictly above
/// any thinking budget, then clamps to `ceiling`.
pub fn adjust_max_tokens(body: &Value, ceiling: u64) -> u64 {
    let mut max_tokens = body
        .get("max_tokens")
        .and_then(Value::as_u64)
        .filter(|value| *value != 0)
        .unwrap_or(DEFAULT_MAX_TOKENS);

    let has_tools = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty());
    if has_tools && max_tokens < DEFAULT_MIN_TOKENS {
        max_tokens = DEFAULT_MIN_TOKENS;
    }

    // Claude requires max_tokens strictly greater than thinking.budget_tokens.
    if let Some(budget) = body
        .get("thinking")
        .and_then(|thinking| thinking.get("budget_tokens"))
        .and_then(Value::as_u64)
        .filter(|value| *value != 0)
        && max_tokens <= budget
    {
        max_tokens = budget.saturating_add(1024);
    }

    max_tokens.min(ceiling)
}

/// Build a base64 data URI (upstream `encodeDataUri`).
pub fn encode_data_uri(mime_type: &str, base64: &str) -> String {
    format!("data:{mime_type};base64,{base64}")
}

/// Parsed `data:` URI parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUri<'a> {
    pub mime_type: &'a str,
    pub base64: &'a str,
}

/// Parse a base64 data URI (upstream `parseDataUri`, pattern
/// `^data:([^;]+);base64,([\s\S]+)$`).
pub fn parse_data_uri(url: &str) -> Option<DataUri<'_>> {
    let rest = url.strip_prefix("data:")?;
    let semi = rest.find(';')?;
    let mime_type = rest.get(..semi)?;
    if mime_type.is_empty() {
        return None;
    }
    let base64 = rest.get(semi..)?.strip_prefix(";base64,")?;
    if base64.is_empty() {
        return None;
    }
    Some(DataUri { mime_type, base64 })
}

/// Collapse a single text part to a plain string (upstream `collapseTextParts`).
pub fn collapse_text_parts(parts: Vec<Value>) -> Value {
    if parts.len() == 1
        && let Some(part) = parts.first()
        && part.get("type").and_then(Value::as_str) == Some(openai_block::TEXT)
        && let Some(text) = part.get("text")
    {
        return text.clone();
    }
    Value::Array(parts)
}

/// `JSON.parse` with a fallback; non-string input passes through
/// (upstream `safeParseJSON`).
pub fn safe_parse_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

/// Extract concatenated text from a Claude/Gemini-style content array
/// (upstream `extractTextContent`).
pub fn extract_text_content(content: Option<&Value>, separator: &str) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some(claude_block::TEXT))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(separator),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_MAX_TOKENS, DEFAULT_MIN_TOKENS, adjust_max_tokens, collapse_text_parts,
        encode_data_uri, extract_text_content, parse_data_uri, safe_parse_json,
    };
    use serde_json::json;

    #[test]
    fn max_tokens_defaults_and_clamps() {
        assert_eq!(
            adjust_max_tokens(&json!({}), DEFAULT_MAX_TOKENS),
            DEFAULT_MAX_TOKENS
        );
        assert_eq!(
            adjust_max_tokens(&json!({ "max_tokens": 100 }), DEFAULT_MAX_TOKENS),
            100
        );
        // Never exceeds the ceiling.
        assert_eq!(
            adjust_max_tokens(&json!({ "max_tokens": 999_999 }), 128_000),
            128_000
        );
    }

    #[test]
    fn tools_raise_the_floor_but_not_past_the_ceiling() {
        let body = json!({ "max_tokens": 100, "tools": [{ "name": "x" }] });
        assert_eq!(
            adjust_max_tokens(&body, DEFAULT_MAX_TOKENS),
            DEFAULT_MIN_TOKENS
        );
        // Ceiling still wins over the tool floor.
        assert_eq!(adjust_max_tokens(&body, 1000), 1000);
        // Empty tools array does not raise the floor.
        let no_tools = json!({ "max_tokens": 100, "tools": [] });
        assert_eq!(adjust_max_tokens(&no_tools, DEFAULT_MAX_TOKENS), 100);
    }

    #[test]
    fn max_tokens_stays_above_thinking_budget() {
        let body = json!({ "max_tokens": 1000, "thinking": { "budget_tokens": 2000 } });
        assert_eq!(adjust_max_tokens(&body, DEFAULT_MAX_TOKENS), 3024);
    }

    #[test]
    fn data_uris_round_trip() {
        let uri = encode_data_uri("image/png", "QUJD");
        assert_eq!(uri, "data:image/png;base64,QUJD");
        let parsed = parse_data_uri(&uri).expect("round-trips");
        assert_eq!(parsed.mime_type, "image/png");
        assert_eq!(parsed.base64, "QUJD");

        assert!(parse_data_uri("https://example.test/a.png").is_none());
        assert!(parse_data_uri("data:image/png;base64,").is_none());
        // Multi-line base64 payloads are tolerated.
        assert!(parse_data_uri("data:image/png;base64,QUJD\nRUZH").is_some());
    }

    #[test]
    fn lone_text_part_collapses_to_string() {
        let single = collapse_text_parts(vec![json!({ "type": "text", "text": "hi" })]);
        assert_eq!(single, json!("hi"));

        let multiple = collapse_text_parts(vec![
            json!({ "type": "text", "text": "hi" }),
            json!({ "type": "image_url", "image_url": { "url": "u" } }),
        ]);
        assert!(multiple.is_array());
    }

    #[test]
    fn json_parsing_falls_back_to_the_raw_string() {
        assert_eq!(safe_parse_json(r#"{"a":1}"#), json!({ "a": 1 }));
        assert_eq!(safe_parse_json("not json"), json!("not json"));
    }

    #[test]
    fn text_extraction_joins_text_blocks_only() {
        let content = json!([
            { "type": "text", "text": "one" },
            { "type": "tool_use", "id": "t" },
            { "type": "text", "text": "two" },
        ]);
        assert_eq!(extract_text_content(Some(&content), "\n"), "one\ntwo");
        assert_eq!(extract_text_content(Some(&json!("plain")), "\n"), "plain");
        assert_eq!(extract_text_content(None, "\n"), "");
    }
}
