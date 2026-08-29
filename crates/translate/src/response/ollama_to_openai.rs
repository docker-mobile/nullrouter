//! Ollama NDJSON responses -> OpenAI `chat.completion.chunk` stream.
//!
//! Ports `open-sse/translator/response/ollama-to-openai.js`.
//!
//! Ollama streams newline-delimited JSON rather than SSE — the framing difference
//! is handled upstream of this module by `Encoding::Ndjson`. What is handled here
//! is the shape: each line is a whole object carrying `message`, and the final line
//! sets `done: true` and reports token counts.
//!
//! Two details are easy to get wrong and are pinned by tests below: `done_reason`
//! must become `tool_calls` when the turn emitted any, or a client will stop instead
//! of running the tool; and `arguments` arrives as an object where OpenAI clients
//! expect a JSON string.

use serde_json::{Value, json};

use crate::concerns::{ChunkMeta, UsageKind, build_chunk, to_openai_finish, to_openai_usage};
use crate::schema::{openai_block, openai_finish, role};
use crate::state::StreamState;

fn chunk_meta(state: &StreamState) -> ChunkMeta {
    ChunkMeta {
        id: state
            .message_id
            .clone()
            .unwrap_or_else(|| format!("chatcmpl-{}", state.clock.now_millis())),
        created: state
            .ollama_created
            .unwrap_or_else(|| state.clock.now_seconds()),
        model: state.model.clone().unwrap_or_default(),
    }
}

/// Translate one Ollama NDJSON object into zero or more OpenAI chunks.
pub fn translate(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    if !raw.is_object() {
        return Vec::new();
    }

    // Identity is fixed on the first line so every chunk of one response shares it.
    if state.message_id.is_none() {
        state.message_id = Some(format!("chatcmpl-{}", state.clock.now_millis()));
        state.ollama_created = Some(state.clock.now_seconds());
        if state.model.is_none() {
            state.model = raw.get("model").and_then(Value::as_str).map(str::to_owned);
        }
    }

    if raw.get("done").and_then(Value::as_bool) == Some(true) {
        if let Some(usage) = to_openai_usage(raw, UsageKind::Ollama) {
            state.usage = Some(usage);
        }
        let reason = finish_reason(raw, state);
        state.finish_reason = Some(reason.clone());
        let mut chunk = build_chunk(&chunk_meta(state), json!({}), Some(&reason));
        // Usage rides the terminal chunk, as upstream does.
        if let Some(usage) = state.usage.as_ref()
            && let Some(object) = chunk.as_object_mut()
        {
            object.insert("usage".to_owned(), usage.to_value());
        }
        return vec![chunk];
    }

    let Some(message) = raw.get("message") else {
        return Vec::new();
    };
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let thinking = message
        .get("thinking")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool_calls = message.get("tool_calls").and_then(Value::as_array);

    // An empty keep-alive line carries nothing for the client.
    if content.is_empty() && thinking.is_empty() && tool_calls.is_none() {
        return Vec::new();
    }

    let mut delta = serde_json::Map::new();
    if !content.is_empty() {
        delta.insert("content".to_owned(), json!(content));
    }
    if !thinking.is_empty() {
        // Ollama's `thinking` is OpenAI's `reasoning_content`.
        delta.insert("reasoning_content".to_owned(), json!(thinking));
    }
    if let Some(calls) = tool_calls {
        // Recorded so the terminal chunk can report `tool_calls`: Ollama often
        // reports `done_reason: "stop"` on a turn that called a tool, and a client
        // reading `stop` would never run it.
        state.ollama_had_tool_calls = true;
        delta.insert(
            "tool_calls".to_owned(),
            Value::Array(
                calls
                    .iter()
                    .enumerate()
                    .map(|(index, call)| openai_tool_call(call, index, state))
                    .collect(),
            ),
        );
    }

    vec![build_chunk(&chunk_meta(state), Value::Object(delta), None)]
}

/// The finish reason for a terminal chunk.
fn finish_reason(raw: &Value, state: &StreamState) -> String {
    let reported = raw
        .get("done_reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if reported == openai_finish::TOOL_CALLS || state.ollama_had_tool_calls {
        return openai_finish::TOOL_CALLS.to_owned();
    }
    to_openai_finish(reported, "ollama")
}

/// One Ollama tool call in OpenAI's shape.
fn openai_tool_call(call: &Value, position: usize, state: &StreamState) -> Value {
    let function = call.get("function");
    let index = function
        .and_then(|function| function.get("index"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| u64::try_from(position).unwrap_or(0));
    let name = function
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Ollama sends arguments as an object; OpenAI clients parse a JSON string.
    let arguments = match function.and_then(|function| function.get("arguments")) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_owned()),
        None => "{}".to_owned(),
    };
    let id = call.get("id").and_then(Value::as_str).map_or_else(
        || format!("call_{position}_{}", state.clock.now_millis()),
        str::to_owned,
    );
    json!({
        "index": index,
        "id": id,
        "type": openai_block::FUNCTION,
        "function": { "name": name, "arguments": arguments },
    })
}

/// Convert a non-streaming Ollama body into an OpenAI `chat.completion`.
///
/// Upstream's `ollamaBodyToOpenAI`. A non-streaming Ollama reply is a single JSON
/// object rather than a stream, so it does not go through [`translate`].
pub fn body_to_openai(body: &Value, state: &StreamState) -> Value {
    let message = body.get("message");
    let content = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let thinking = message
        .and_then(|message| message.get("thinking"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let calls = message
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    let mut out = serde_json::Map::new();
    out.insert("role".to_owned(), json!(role::ASSISTANT));
    if !content.is_empty() {
        out.insert("content".to_owned(), json!(content));
    }
    if !thinking.is_empty() {
        out.insert("reasoning_content".to_owned(), json!(thinking));
    }
    if !calls.is_empty() {
        out.insert(
            "tool_calls".to_owned(),
            Value::Array(
                calls
                    .iter()
                    .enumerate()
                    .map(|(index, call)| openai_tool_call(call, index, state))
                    .collect(),
            ),
        );
    }
    // A message with neither content nor calls still needs a content field: a
    // client reading `message.content` must not find it absent.
    if !out.contains_key("content") && !out.contains_key("tool_calls") {
        out.insert("content".to_owned(), json!(""));
    }

    let reported = body
        .get("done_reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let finish = if calls.is_empty() {
        to_openai_finish(reported, "ollama")
    } else {
        openai_finish::TOOL_CALLS.to_owned()
    };

    json!({
        "id": format!("chatcmpl-{}", state.clock.now_millis()),
        "object": "chat.completion",
        "created": state.clock.now_seconds(),
        "model": body.get("model").and_then(Value::as_str).unwrap_or("ollama"),
        "choices": [{ "index": 0, "message": Value::Object(out), "finish_reason": finish }],
        "usage": to_openai_usage(body, UsageKind::Ollama)
            .map_or(Value::Null, crate::concerns::Usage::to_value),
    })
}

#[cfg(test)]
mod tests {
    use super::{body_to_openai, translate};
    use crate::state::{Clock, StreamState};
    use serde_json::{Value, json};

    const fn state() -> StreamState {
        StreamState::new(Clock::Fixed(1_700_000_123_456))
    }

    #[test]
    fn content_lines_become_chunks_with_a_stable_id() {
        let mut state = state();
        let first = translate(
            &json!({ "model": "llama3.2", "message": { "role": "assistant", "content": "he" } }),
            &mut state,
        );
        let second = translate(
            &json!({ "model": "llama3.2", "message": { "role": "assistant", "content": "llo" } }),
            &mut state,
        );

        assert_eq!(first.len(), 1);
        let first = first.first().expect("first chunk");
        let second = second.first().expect("second chunk");
        assert_eq!(
            first.pointer("/choices/0/delta/content"),
            Some(&json!("he"))
        );
        assert_eq!(
            first.get("object").and_then(Value::as_str),
            Some("chat.completion.chunk")
        );
        // The model is taken from the stream, not invented.
        assert_eq!(first.get("model"), Some(&json!("llama3.2")));
        // Both chunks belong to one response, so the id must not change.
        assert_eq!(first.get("id"), second.get("id"));
    }

    #[test]
    fn thinking_is_reported_as_reasoning_content() {
        let mut state = state();
        let out = translate(
            &json!({ "message": { "content": "", "thinking": "let me see" } }),
            &mut state,
        );
        let chunk = out.first().expect("a chunk");
        assert_eq!(
            chunk.pointer("/choices/0/delta/reasoning_content"),
            Some(&json!("let me see"))
        );
        // No empty content field alongside it.
        assert!(chunk.pointer("/choices/0/delta/content").is_none());
    }

    #[test]
    fn an_empty_keepalive_line_produces_nothing() {
        let mut state = state();
        assert!(
            translate(&json!({ "message": { "content": "" } }), &mut state).is_empty(),
            "an empty line must not emit a chunk"
        );
        // A line with no message at all is also nothing.
        assert!(translate(&json!({ "model": "llama3.2" }), &mut state).is_empty());
    }

    #[test]
    fn the_final_line_carries_usage_and_a_finish_reason() {
        let mut state = state();
        translate(&json!({ "message": { "content": "hi" } }), &mut state);
        let done = translate(
            &json!({
                "model": "llama3.2",
                "done": true,
                "done_reason": "stop",
                "prompt_eval_count": 11,
                "eval_count": 7,
            }),
            &mut state,
        );

        assert_eq!(done.len(), 1);
        let done = done.first().expect("terminal chunk");
        assert_eq!(
            done.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
        // Ollama's counters are OpenAI's token fields.
        assert_eq!(done.pointer("/usage/prompt_tokens"), Some(&json!(11)));
        assert_eq!(done.pointer("/usage/completion_tokens"), Some(&json!(7)));
        assert_eq!(done.pointer("/usage/total_tokens"), Some(&json!(18)));
    }

    #[test]
    fn a_turn_that_called_a_tool_finishes_as_tool_calls() {
        let mut state = state();
        let call = translate(
            &json!({
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "function": { "index": 0, "name": "get_weather", "arguments": { "city": "Oslo" } },
                    }],
                },
            }),
            &mut state,
        );

        let emitted = call
            .first()
            .and_then(|chunk| chunk.pointer("/choices/0/delta/tool_calls/0"))
            .expect("tool call");
        assert_eq!(
            emitted.pointer("/function/name"),
            Some(&json!("get_weather"))
        );
        // OpenAI clients parse `arguments` as a JSON string, not an object.
        assert_eq!(
            emitted.pointer("/function/arguments"),
            Some(&json!(r#"{"city":"Oslo"}"#))
        );
        assert_eq!(emitted.get("type"), Some(&json!("function")));
        assert!(
            emitted
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("call_")),
            "a call needs an id for the client to reply to: {emitted}"
        );

        // Ollama reports `stop` here. Relaying that would leave the client never
        // running the tool it was just asked to run.
        let done = translate(&json!({ "done": true, "done_reason": "stop" }), &mut state);
        assert_eq!(
            done.first()
                .and_then(|chunk| chunk.pointer("/choices/0/finish_reason")),
            Some(&json!("tool_calls"))
        );
    }

    #[test]
    fn a_non_streaming_body_becomes_one_completion() {
        let body = json!({
            "model": "llama3.2",
            "message": { "role": "assistant", "content": "hello", "thinking": "hmm" },
            "done": true,
            "done_reason": "stop",
            "prompt_eval_count": 4,
            "eval_count": 2,
        });
        let out = body_to_openai(&body, &state());

        assert_eq!(
            out.get("object").and_then(Value::as_str),
            Some("chat.completion")
        );
        assert_eq!(out.get("model"), Some(&json!("llama3.2")));
        assert_eq!(
            out.pointer("/choices/0/message/content"),
            Some(&json!("hello"))
        );
        assert_eq!(
            out.pointer("/choices/0/message/reasoning_content"),
            Some(&json!("hmm"))
        );
        assert_eq!(
            out.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
        assert_eq!(out.pointer("/usage/total_tokens"), Some(&json!(6)));
    }

    #[test]
    fn a_tool_only_reply_still_reports_tool_calls_and_needs_no_content() {
        let body = json!({
            "model": "llama3.2",
            "message": {
                "role": "assistant",
                "tool_calls": [{ "function": { "name": "get_weather", "arguments": {} } }],
            },
            "done": true,
            "done_reason": "stop",
        });
        let out = body_to_openai(&body, &state());
        assert_eq!(
            out.pointer("/choices/0/finish_reason"),
            Some(&json!("tool_calls"))
        );
        // A tool-only message carries no content, and none is invented.
        assert!(out.pointer("/choices/0/message/content").is_none());
    }

    #[test]
    fn an_empty_reply_still_has_a_content_field() {
        let body = json!({ "model": "llama3.2", "message": { "role": "assistant" }, "done": true });
        let out = body_to_openai(&body, &state());
        // A client reading `message.content` must find it present.
        assert_eq!(out.pointer("/choices/0/message/content"), Some(&json!("")));
    }
}
