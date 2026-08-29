//! Whole-body response translation, for non-streaming replies.
//!
//! Ports `translateNonStreamingResponse` from
//! `open-sse/handlers/chatCore/nonStreamingHandler.js`.
//!
//! The streaming translators in [`crate::response`] are incremental state machines
//! over frames. A provider that answers a genuine non-streaming request has no
//! frames: it returns one object in its own shape. Without this module that object
//! reached the client verbatim, so an OpenAI client asking a Claude-format provider
//! for `stream: false` received `content[]` and `stop_reason` where it expected
//! `choices[]` — a completion that looks empty rather than an error.

use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use crate::concerns::{UsageKind, from_openai_finish, to_openai_usage};
use crate::response::ollama_to_openai;
use crate::schema::{claude_block, openai_block, openai_finish, role};
use crate::state::StreamState;
use nullrouter_providers::Format;

/// Translate one complete non-streaming response body from `target` to `source`.
///
/// Returns the body unchanged when no translation applies, which is the common
/// case: most providers already answer in OpenAI's shape.
pub fn translate_body(target: Format, source: Format, body: &Value, state: &StreamState) -> Value {
    if target == source {
        return body.clone();
    }

    // Step 1: provider shape -> OpenAI.
    let pivot = match target {
        Format::Claude => claude_body_to_openai(body, state),
        Format::Gemini | Format::GeminiCli | Format::Vertex | Format::Antigravity => {
            gemini_body_to_openai(body, state)
        }
        Format::Ollama => ollama_to_openai::body_to_openai(body, state),
        // Already OpenAI-shaped, or a format with no ported body translator.
        _ => body.clone(),
    };

    // Step 2: OpenAI -> client shape.
    match source {
        Format::Claude => openai_body_to_claude(&pivot, state),
        _ => pivot,
    }
}

/// A Claude `message` body as an OpenAI `chat.completion`.
fn claude_body_to_openai(body: &Value, state: &StreamState) -> Value {
    // Some providers answer in OpenAI's shape even when the request was translated
    // to Claude's (xiaomi-tokenplan does). Re-translating that would destroy it.
    if body.get("choices").is_some()
        || body
            .get("content")
            .is_some_and(|content| !content.is_null() && !content.is_array())
    {
        return body.clone();
    }

    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for block in body
        .get("content")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        match block.get("type").and_then(Value::as_str) {
            Some(claude_block::TEXT) => {
                text.push_str(&strip_json_fence(
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ));
            }
            Some(claude_block::THINKING) => {
                thinking.push_str(
                    block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
            }
            Some(claude_block::TOOL_USE) => {
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                tool_calls.push(json!({
                    "id": block.get("id").cloned().unwrap_or(Value::Null),
                    "type": openai_block::FUNCTION,
                    "function": {
                        "name": block.get("name").and_then(Value::as_str).unwrap_or_default(),
                        // OpenAI clients parse `arguments` as a JSON string.
                        "arguments": serde_json::to_string(&input)
                            .unwrap_or_else(|_| "{}".to_owned()),
                    },
                }));
            }
            _ => {}
        }
    }

    let finish = match body.get("stop_reason").and_then(Value::as_str) {
        Some("end_turn") | None => openai_finish::STOP.to_owned(),
        Some("tool_use") => openai_finish::TOOL_CALLS.to_owned(),
        Some(other) => other.to_owned(),
    };
    let usage = body
        .get("usage")
        .and_then(|usage| to_openai_usage(usage, UsageKind::Claude));

    completion(
        &format!(
            "chatcmpl-{}",
            body.get("id")
                .and_then(Value::as_str)
                .map_or_else(|| state.clock.now_millis().to_string(), str::to_owned)
        ),
        body.get("model")
            .and_then(Value::as_str)
            .unwrap_or("claude"),
        state.clock.now_seconds(),
        assistant_message(&text, &thinking, tool_calls),
        &finish,
        usage,
    )
}

/// A Gemini `candidates` body as an OpenAI `chat.completion`.
fn gemini_body_to_openai(body: &Value, state: &StreamState) -> Value {
    // Antigravity wraps the payload in `response`.
    let response = body.get("response").unwrap_or(body);
    let Some(candidate) = response
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
    else {
        return body.clone();
    };

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for part in candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let is_thought = part.get("thought").and_then(Value::as_bool) == Some(true);
        if let Some(chunk) = part.get("text").and_then(Value::as_str) {
            if is_thought {
                reasoning.push_str(chunk);
            } else {
                text.push_str(chunk);
            }
        }
        if let Some(call) = part.get("functionCall") {
            let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            tool_calls.push(json!({
                "id": format!("call_{name}_{}_{}", state.clock.now_millis(), tool_calls.len()),
                "type": openai_block::FUNCTION,
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_owned()),
                },
            }));
        }
        // An image-generation model returns inline data; it is rendered as a data
        // URI in the text so an OpenAI client can display it at all.
        if let Some(data) = part
            .get("inlineData")
            .or_else(|| part.get("inline_data"))
            .and_then(|inline| inline.get("data"))
            .and_then(Value::as_str)
        {
            let mime = part
                .get("inlineData")
                .or_else(|| part.get("inline_data"))
                .and_then(|inline| inline.get("mimeType").or_else(|| inline.get("mime_type")))
                .and_then(Value::as_str)
                .unwrap_or(crate::schema::DEFAULT_IMAGE_MIME);
            let _ = write!(text, "\n![image](data:{mime};base64,{data})\n");
        }
    }

    let mut finish = candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .unwrap_or(openai_finish::STOP)
        .to_lowercase();
    // Gemini reports STOP even on a turn that called a tool.
    if finish == openai_finish::STOP && !tool_calls.is_empty() {
        openai_finish::TOOL_CALLS.clone_into(&mut finish);
    }

    let usage = response
        .get("usageMetadata")
        .or_else(|| body.get("usageMetadata"))
        .and_then(|meta| to_openai_usage(meta, UsageKind::Gemini));

    completion(
        &format!(
            "chatcmpl-{}",
            response
                .get("responseId")
                .and_then(Value::as_str)
                .map_or_else(|| state.clock.now_millis().to_string(), str::to_owned)
        ),
        response
            .get("modelVersion")
            .and_then(Value::as_str)
            .unwrap_or("gemini"),
        state.clock.now_seconds(),
        assistant_message(&text, &reasoning, tool_calls),
        &finish,
        usage,
    )
}

/// An OpenAI `chat.completion` as a Claude `message`.
///
/// Needed when a Claude client (`/v1/messages`) is routed to an OpenAI-shaped
/// provider and asks for `stream: false`.
fn openai_body_to_claude(body: &Value, state: &StreamState) -> Value {
    let Some(choice) = body
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return body.clone();
    };
    let message = choice.get("message");

    let mut content: Vec<Value> = Vec::new();
    if let Some(reasoning) = message
        .and_then(|message| {
            message.get("reasoning_content").or_else(|| {
                message
                    .get("provider_specific_fields")
                    .and_then(|fields| fields.get("reasoning_content"))
            })
        })
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        content.push(json!({ "type": claude_block::THINKING, "thinking": reasoning }));
    }
    if let Some(text) = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        content.push(json!({ "type": claude_block::TEXT, "text": text }));
    }
    for call in message
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let function = call.get("function");
        let name = function
            .and_then(|function| function.get("name"))
            .or_else(|| call.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let raw = function
            .and_then(|function| function.get("arguments"))
            .or_else(|| call.get("arguments"));
        content.push(json!({
            "type": claude_block::TOOL_USE,
            "id": call.get("id").and_then(Value::as_str).map_or_else(
                || format!("toolu_{}_{}", state.clock.now_millis(), content.len()),
                str::to_owned,
            ),
            "name": name,
            // Claude takes `input` as an object, where OpenAI sent a JSON string.
            "input": tool_input(raw),
        }));
    }
    // A Claude client requires at least one block.
    if content.is_empty() {
        content.push(json!({ "type": claude_block::TEXT, "text": "" }));
    }

    let usage = body.get("usage");
    let prompt = usage_field(usage, "prompt_tokens", "input_tokens");
    let completion_tokens = usage_field(usage, "completion_tokens", "output_tokens");

    json!({
        "id": body
            .get("id")
            .and_then(Value::as_str)
            .map_or_else(
                || format!("msg_{}", state.clock.now_millis()),
                |id| id.strip_prefix("chatcmpl-").unwrap_or(id).to_owned(),
            ),
        "type": "message",
        "role": role::ASSISTANT,
        "model": body.get("model").and_then(Value::as_str).unwrap_or("unknown"),
        "content": content,
        "stop_reason": from_openai_finish(
            choice.get("finish_reason").and_then(Value::as_str).unwrap_or(openai_finish::STOP),
            "claude",
        ),
        "stop_sequence": Value::Null,
        "usage": { "input_tokens": prompt, "output_tokens": completion_tokens },
    })
}

/// A usage number under either the OpenAI or the Claude spelling.
fn usage_field(usage: Option<&Value>, openai: &str, claude: &str) -> u64 {
    usage
        .and_then(|usage| {
            usage
                .get(openai)
                .or_else(|| usage.get(claude))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0)
}

/// Tool arguments as the object Claude expects.
fn tool_input(raw: Option<&Value>) -> Value {
    match raw {
        Some(Value::String(text)) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        Some(other) if other.is_object() => other.clone(),
        _ => json!({}),
    }
}

/// An OpenAI assistant message from its three possible parts.
fn assistant_message(text: &str, reasoning: &str, tool_calls: Vec<Value>) -> Value {
    let mut message = Map::new();
    message.insert("role".to_owned(), json!(role::ASSISTANT));
    if !text.is_empty() {
        message.insert("content".to_owned(), json!(text));
    }
    if !reasoning.is_empty() {
        message.insert("reasoning_content".to_owned(), json!(reasoning));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_owned(), Value::Array(tool_calls));
    }
    // A client reading `message.content` must find it present.
    if !message.contains_key("content") && !message.contains_key("tool_calls") {
        message.insert("content".to_owned(), json!(""));
    }
    Value::Object(message)
}

/// Assemble an OpenAI `chat.completion` envelope.
#[allow(
    clippy::needless_pass_by_value,
    reason = "message is moved into the returned envelope"
)]
fn completion(
    id: &str,
    model: &str,
    created: u64,
    message: Value,
    finish_reason: &str,
    usage: Option<crate::concerns::Usage>,
) -> Value {
    let mut out = Map::new();
    out.insert("id".to_owned(), json!(id));
    out.insert("object".to_owned(), json!("chat.completion"));
    out.insert("created".to_owned(), json!(created));
    out.insert("model".to_owned(), json!(model));
    out.insert(
        "choices".to_owned(),
        json!([{ "index": 0, "message": message, "finish_reason": finish_reason }]),
    );
    if let Some(usage) = usage {
        out.insert("usage".to_owned(), usage.to_value());
    }
    Value::Object(out)
}

/// Strip a wrapping markdown JSON fence.
///
/// Some providers (kimi) wrap a JSON answer in a markdown fence, which a client
/// parsing the content as JSON then fails on.
fn strip_json_fence(text: &str) -> String {
    let trimmed = text.trim();
    let Some(inner) = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
    else {
        return text.to_owned();
    };
    inner.trim_end().strip_suffix("```").map_or_else(
        // An opening fence with no close is not a fence; keep the original.
        || text.to_owned(),
        // Both fences eat their adjacent newline, matching upstream's
        // `/\n?\s*```\s*$/`: a leftover newline breaks a strict JSON parse.
        |body| body.trim_start_matches('\n').trim_end().to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::{strip_json_fence, translate_body};
    use crate::state::{Clock, StreamState};
    use nullrouter_providers::Format;
    use serde_json::{Value, json};

    const fn state() -> StreamState {
        StreamState::new(Clock::Fixed(1_700_000_123_456))
    }

    #[test]
    fn a_claude_body_becomes_an_openai_completion() {
        let body = json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [
                { "type": "thinking", "thinking": "considering" },
                { "type": "text", "text": "hello" },
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 3, "cache_read_input_tokens": 2 },
        });
        let out = translate_body(Format::Claude, Format::OpenAi, &body, &state());

        assert_eq!(
            out.get("object").and_then(Value::as_str),
            Some("chat.completion")
        );
        assert_eq!(
            out.pointer("/choices/0/message/content"),
            Some(&json!("hello"))
        );
        assert_eq!(
            out.pointer("/choices/0/message/reasoning_content"),
            Some(&json!("considering"))
        );
        // `end_turn` is Claude's spelling of `stop`.
        assert_eq!(
            out.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
        // Claude folds cache reads into prompt tokens.
        assert_eq!(out.pointer("/usage/prompt_tokens"), Some(&json!(12)));
        assert_eq!(out.pointer("/usage/completion_tokens"), Some(&json!(3)));
        // The id keeps its origin but gains the OpenAI prefix.
        assert_eq!(out.get("id"), Some(&json!("chatcmpl-msg_01")));
    }

    #[test]
    fn a_claude_tool_use_becomes_a_tool_call_with_string_arguments() {
        let body = json!({
            "model": "claude-sonnet-4-5",
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "get_weather",
                "input": { "city": "Oslo" },
            }],
            "stop_reason": "tool_use",
        });
        let out = translate_body(Format::Claude, Format::OpenAi, &body, &state());

        let call = out
            .pointer("/choices/0/message/tool_calls/0")
            .expect("tool call");
        assert_eq!(call.get("id"), Some(&json!("toolu_1")));
        assert_eq!(call.pointer("/function/name"), Some(&json!("get_weather")));
        // A JSON string, not an object: that is what an OpenAI client parses.
        assert_eq!(
            call.pointer("/function/arguments"),
            Some(&json!(r#"{"city":"Oslo"}"#))
        );
        assert_eq!(
            out.pointer("/choices/0/finish_reason"),
            Some(&json!("tool_calls"))
        );
        // A tool-only turn carries no content, and none is invented.
        assert!(out.pointer("/choices/0/message/content").is_none());
    }

    #[test]
    fn a_claude_body_with_null_content_still_yields_a_choices_array() {
        // A model that spends its whole budget on thinking answers `content: null`.
        // Returning that verbatim leaves an OpenAI client with no `choices` at all.
        let body = json!({ "model": "m3", "content": Value::Null, "stop_reason": "max_tokens" });
        let out = translate_body(Format::Claude, Format::OpenAi, &body, &state());
        assert_eq!(out.pointer("/choices/0/message/content"), Some(&json!("")));
        assert_eq!(
            out.pointer("/choices/0/finish_reason"),
            Some(&json!("max_tokens"))
        );
    }

    #[test]
    fn a_provider_that_answers_openai_shaped_under_a_claude_target_is_left_alone() {
        // xiaomi-tokenplan does this: the request was translated to Claude, but the
        // reply is OpenAI-native. Re-translating would destroy it.
        let body = json!({
            "id": "chatcmpl-x",
            "object": "chat.completion",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi" } }],
        });
        let out = translate_body(Format::Claude, Format::OpenAi, &body, &state());
        assert_eq!(out, body);
    }

    #[test]
    fn a_gemini_body_becomes_an_openai_completion() {
        let body = json!({
            "responseId": "resp_1",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "content": { "parts": [
                    { "text": "reasoning here", "thought": true },
                    { "text": "the answer" },
                ]},
                "finishReason": "STOP",
            }],
            "usageMetadata": {
                "promptTokenCount": 8,
                "candidatesTokenCount": 4,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 14,
            },
        });
        let out = translate_body(Format::Gemini, Format::OpenAi, &body, &state());

        assert_eq!(
            out.pointer("/choices/0/message/content"),
            Some(&json!("the answer"))
        );
        assert_eq!(
            out.pointer("/choices/0/message/reasoning_content"),
            Some(&json!("reasoning here"))
        );
        // Gemini's SCREAMING reason is lowercased.
        assert_eq!(
            out.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
        assert_eq!(out.pointer("/usage/total_tokens"), Some(&json!(14)));
    }

    #[test]
    fn a_gemini_function_call_forces_a_tool_calls_finish() {
        let body = json!({
            "candidates": [{
                "content": { "parts": [{ "functionCall": { "name": "lookup", "args": { "q": 1 } } }] },
                // Gemini reports STOP even when it called a tool.
                "finishReason": "STOP",
            }],
        });
        let out = translate_body(Format::Gemini, Format::OpenAi, &body, &state());
        assert_eq!(
            out.pointer("/choices/0/finish_reason"),
            Some(&json!("tool_calls")),
            "a client reading `stop` would never run the tool: {out}"
        );
        assert_eq!(
            out.pointer("/choices/0/message/tool_calls/0/function/arguments"),
            Some(&json!(r#"{"q":1}"#))
        );
    }

    #[test]
    fn an_openai_body_becomes_a_claude_message_for_a_claude_client() {
        let body = json!({
            "id": "chatcmpl-9",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "get_weather", "arguments": "{\"city\":\"Oslo\"}" },
                    }],
                },
                "finish_reason": "tool_calls",
            }],
            "usage": { "prompt_tokens": 6, "completion_tokens": 2 },
        });
        let out = translate_body(Format::OpenAi, Format::Claude, &body, &state());

        assert_eq!(out.get("type"), Some(&json!("message")));
        assert_eq!(out.get("role"), Some(&json!("assistant")));
        // The `chatcmpl-` prefix is not part of a Claude id.
        assert_eq!(out.get("id"), Some(&json!("9")));
        assert_eq!(out.pointer("/content/0/text"), Some(&json!("hello")));
        // Claude takes tool input as an object, not a JSON string.
        assert_eq!(
            out.pointer("/content/1/input"),
            Some(&json!({ "city": "Oslo" }))
        );
        assert_eq!(out.pointer("/content/1/type"), Some(&json!("tool_use")));
        assert_eq!(out.get("stop_reason"), Some(&json!("tool_use")));
        assert_eq!(out.pointer("/usage/input_tokens"), Some(&json!(6)));
        assert_eq!(out.pointer("/usage/output_tokens"), Some(&json!(2)));
    }

    #[test]
    fn a_claude_client_always_gets_at_least_one_content_block() {
        let body = json!({
            "choices": [{ "index": 0, "message": { "role": "assistant" }, "finish_reason": "stop" }],
        });
        let out = translate_body(Format::OpenAi, Format::Claude, &body, &state());
        // A Claude client rejects a message with an empty `content`.
        assert_eq!(out.pointer("/content/0/text"), Some(&json!("")));
        assert_eq!(out.get("stop_reason"), Some(&json!("end_turn")));
    }

    #[test]
    fn an_identical_source_and_target_is_passed_through() {
        let body = json!({ "anything": true });
        assert_eq!(
            translate_body(Format::OpenAi, Format::OpenAi, &body, &state()),
            body
        );
        // As is a format with no ported body translator.
        assert_eq!(
            translate_body(Format::Kiro, Format::OpenAi, &body, &state()),
            body
        );
    }

    #[test]
    fn a_json_fence_is_stripped_from_claude_text() {
        assert_eq!(strip_json_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_json_fence("```\n{\"a\":1}\n```"), "{\"a\":1}");
        // Prose is untouched, and an unclosed fence is not treated as one.
        assert_eq!(strip_json_fence("plain text"), "plain text");
        assert_eq!(strip_json_fence("```json\nno close"), "```json\nno close");
    }
}
