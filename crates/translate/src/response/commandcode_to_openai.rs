//! `CommandCode` NDJSON events -> OpenAI `chat.completion.chunk` stream.
//!
//! Ports `open-sse/translator/response/commandcode-to-openai.js`.
//!
//! `CommandCode` speaks the AI SDK v5 event stream: one JSON object per line, no
//! `data:` prefix, with named event types rather than deltas on a chunk. Text,
//! reasoning, and tool arguments each arrive as their own `*-delta` events, and the
//! terminal `finish` event carries the usage totals.
//!
//! The awkward part is tool calls, which arrive one of two ways: streamed as
//! `tool-input-start` / `tool-input-delta`, or delivered whole as a single
//! `tool-call`. Emitting both would give the client the same call twice with its
//! arguments doubled, so a `tool-call` for an id already seen is dropped.

use serde_json::{Value, json};

use crate::concerns::{
    ChunkMeta, UsageKind, build_chunk, reasoning_delta, to_openai_finish, to_openai_usage,
};
use crate::schema::{openai_block, openai_finish, role};
use crate::state::StreamState;

fn chunk_meta(state: &StreamState) -> ChunkMeta {
    ChunkMeta {
        id: state
            .message_id
            .clone()
            .unwrap_or_else(|| format!("chatcmpl-{}", state.clock.now_millis())),
        created: state
            .command_code_created
            .unwrap_or_else(|| state.clock.now_seconds()),
        model: state.model.clone().unwrap_or_default(),
    }
}

/// Translate one `CommandCode` event into zero or more OpenAI chunks.
pub fn translate(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    // An already-OpenAI chunk passes straight through, as upstream allows.
    if raw.get("object").and_then(Value::as_str) == Some("chat.completion.chunk") {
        return vec![raw.clone()];
    }
    let Some(event) = raw.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };

    if state.message_id.is_none() {
        state.message_id = Some(format!("chatcmpl-{}", state.clock.now_millis()));
        state.command_code_created = Some(state.clock.now_seconds());
        if state.model.is_none() {
            state.model = Some(
                raw.get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("commandcode")
                    .to_owned(),
            );
        }
    }

    match event {
        "text-delta" => text_delta(raw, state),
        "reasoning-delta" => reasoning(raw, state),
        "tool-input-start" => tool_start(raw, state),
        "tool-input-delta" => tool_delta(raw, state),
        "tool-call" => whole_tool_call(raw, state),
        "finish-step" => {
            // Recorded, not emitted: the `finish` event carries the final chunk, and
            // a step boundary is not the end of the turn.
            state.finish_reason = Some(mapped_finish(raw));
            if let Some(usage) = raw
                .get("usage")
                .and_then(|usage| to_openai_usage(usage, UsageKind::CommandCode))
            {
                state.usage = Some(usage);
            }
            Vec::new()
        }
        "finish" => finish(raw, state),
        "error" => error_event(raw, state),
        // start, start-step, reasoning-start/end, text-start/end, and the metadata
        // events carry nothing a client can render.
        _ => Vec::new(),
    }
}

/// `finish_reason` for an event that reports one.
fn mapped_finish(raw: &Value) -> String {
    to_openai_finish(
        raw.get("finishReason")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "commandcode",
    )
}

/// Whether this is the first chunk of the response, which carries `role`.
const fn first_chunk(state: &mut StreamState) -> bool {
    let first = state.command_code_chunks == 0;
    state.command_code_chunks = state.command_code_chunks.saturating_add(1);
    first
}

fn text_delta(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    let text = raw
        .get("text")
        .or_else(|| raw.get("delta"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if text.is_empty() {
        return Vec::new();
    }
    let delta = if first_chunk(state) {
        json!({ "role": role::ASSISTANT, "content": text })
    } else {
        json!({ "content": text })
    };
    vec![build_chunk(&chunk_meta(state), delta, None)]
}

fn reasoning(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    let text = raw.get("text").and_then(Value::as_str).unwrap_or_default();
    if text.is_empty() {
        return Vec::new();
    }
    let delta = reasoning_delta(text, first_chunk(state));
    vec![build_chunk(&chunk_meta(state), delta, None)]
}

/// The event's tool id, under either spelling.
fn tool_id(raw: &Value) -> Option<&str> {
    raw.get("id")
        .or_else(|| raw.get("toolCallId"))
        .and_then(Value::as_str)
}

fn tool_start(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    let id = tool_id(raw).map_or_else(
        || {
            format!(
                "call_{}_{}",
                state.command_code_tools,
                state.clock.now_millis()
            )
        },
        str::to_owned,
    );
    let index = *state
        .command_code_tool_index
        .entry(id.clone())
        .or_insert_with(|| {
            let next = state.command_code_tools;
            state.command_code_tools = next.saturating_add(1);
            next
        });

    let mut delta = serde_json::Map::new();
    if first_chunk(state) {
        delta.insert("role".to_owned(), json!(role::ASSISTANT));
    }
    delta.insert(
        "tool_calls".to_owned(),
        json!([{
            "index": index,
            "id": id,
            "type": openai_block::FUNCTION,
            "function": {
                "name": raw.get("toolName").and_then(Value::as_str).unwrap_or_default(),
                // Arguments arrive in later deltas.
                "arguments": "",
            },
        }]),
    );
    vec![build_chunk(&chunk_meta(state), Value::Object(delta), None)]
}

fn tool_delta(raw: &Value, state: &StreamState) -> Vec<Value> {
    // A delta for a call that never started cannot be placed in the stream: its
    // index is unknown, and guessing one would merge it into another call.
    let Some(index) = tool_id(raw)
        .and_then(|id| state.command_code_tool_index.get(id))
        .copied()
    else {
        return Vec::new();
    };
    let fragment = raw
        .get("delta")
        .or_else(|| raw.get("inputTextDelta"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    vec![build_chunk(
        &chunk_meta(state),
        json!({
            "tool_calls": [{ "index": index, "function": { "arguments": fragment } }],
        }),
        None,
    )]
}

fn whole_tool_call(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    let Some(id) = raw
        .get("toolCallId")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Vec::new();
    };
    // Already streamed as deltas: emitting it again would duplicate the call and
    // double its arguments.
    if state.command_code_tool_index.contains_key(&id) {
        return Vec::new();
    }
    let index = state.command_code_tools;
    state.command_code_tools = index.saturating_add(1);
    state.command_code_tool_index.insert(id.clone(), index);

    let arguments = match raw.get("input") {
        Some(Value::String(text)) => text.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_owned()),
        None => "{}".to_owned(),
    };
    let mut delta = serde_json::Map::new();
    if first_chunk(state) {
        delta.insert("role".to_owned(), json!(role::ASSISTANT));
    }
    delta.insert(
        "tool_calls".to_owned(),
        json!([{
            "index": index,
            "id": id,
            "type": openai_block::FUNCTION,
            "function": {
                "name": raw.get("toolName").and_then(Value::as_str).unwrap_or_default(),
                "arguments": arguments,
            },
        }]),
    );
    vec![build_chunk(&chunk_meta(state), Value::Object(delta), None)]
}

fn finish(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    let reason = state
        .finish_reason
        .clone()
        .unwrap_or_else(|| mapped_finish(raw));
    let mut chunk = build_chunk(&chunk_meta(state), json!({}), Some(&reason));
    // `totalUsage` on the finish event wins over the per-step usage.
    let usage = raw
        .get("totalUsage")
        .and_then(|usage| to_openai_usage(usage, UsageKind::CommandCode))
        .or(state.usage);
    if let Some(usage) = usage
        && let Some(object) = chunk.as_object_mut()
    {
        object.insert("usage".to_owned(), usage.to_value());
        state.usage = Some(usage);
    }
    vec![chunk]
}

fn error_event(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    let message = match raw.get("error").or_else(|| raw.get("message")) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "unknown".to_owned(),
    };
    state.finish_reason = Some(openai_finish::STOP.to_owned());
    // The error is delivered as content: the stream has already started, so there
    // is no status left to change, and a silent truncation would look like a short
    // answer rather than a failure.
    vec![
        build_chunk(
            &chunk_meta(state),
            json!({ "content": format!("\n\n[CommandCode error: {message}]") }),
            None,
        ),
        build_chunk(&chunk_meta(state), json!({}), Some(openai_finish::STOP)),
    ]
}

#[cfg(test)]
mod tests {
    use super::translate;
    use crate::state::{Clock, StreamState};
    use serde_json::{Value, json};

    fn state() -> StreamState {
        StreamState::new(Clock::Fixed(1_700_000_123_456))
    }

    /// Translate a whole event stream, flattening the chunks.
    fn run(events: &[Value]) -> Vec<Value> {
        let mut state = state();
        events
            .iter()
            .flat_map(|event| translate(event, &mut state))
            .collect()
    }

    #[test]
    fn lifecycle_events_with_no_content_emit_nothing() {
        let out = run(&[
            json!({ "type": "start" }),
            json!({ "type": "start-step" }),
            json!({ "type": "text-start", "id": "t1" }),
            json!({ "type": "reasoning-start", "id": "r1" }),
            json!({ "type": "provider-metadata" }),
        ]);
        assert!(out.is_empty(), "got {out:?}");
        // An event with no `type` is not an event.
        assert!(run(&[json!({ "text": "hi" })]).is_empty());
    }

    #[test]
    fn text_deltas_become_chunks_and_the_first_carries_the_role() {
        let out = run(&[
            json!({ "type": "text-delta", "text": "po" }),
            json!({ "type": "text-delta", "text": "ng" }),
        ]);
        assert_eq!(out.len(), 2);
        let first = out.first().expect("first");
        assert_eq!(
            first.pointer("/choices/0/delta/role"),
            Some(&json!("assistant")),
            "the first chunk must open the message"
        );
        assert_eq!(
            first.pointer("/choices/0/delta/content"),
            Some(&json!("po"))
        );
        let second = out.get(1).expect("second");
        // Only the first chunk carries the role.
        assert!(second.pointer("/choices/0/delta/role").is_none());
        assert_eq!(
            second.pointer("/choices/0/delta/content"),
            Some(&json!("ng"))
        );
        // Both belong to one response.
        assert_eq!(first.get("id"), second.get("id"));
    }

    #[test]
    fn reasoning_deltas_are_reported_separately_from_content() {
        let out = run(&[json!({ "type": "reasoning-delta", "text": "thinking" })]);
        let chunk = out.first().expect("a chunk");
        assert_eq!(
            chunk.pointer("/choices/0/delta/reasoning_content"),
            Some(&json!("thinking"))
        );
        assert!(chunk.pointer("/choices/0/delta/content").is_none());
    }

    #[test]
    fn a_streamed_tool_call_accumulates_across_deltas() {
        let out = run(&[
            json!({ "type": "tool-input-start", "id": "tc_1", "toolName": "read_file" }),
            json!({ "type": "tool-input-delta", "id": "tc_1", "delta": "{\"path\":" }),
            json!({ "type": "tool-input-delta", "id": "tc_1", "delta": "\"a.rs\"}" }),
        ]);
        assert_eq!(out.len(), 3);

        let start = out
            .first()
            .and_then(|chunk| chunk.pointer("/choices/0/delta/tool_calls/0"))
            .expect("start");
        assert_eq!(start.get("id"), Some(&json!("tc_1")));
        assert_eq!(start.get("index"), Some(&json!(0)));
        assert_eq!(start.pointer("/function/name"), Some(&json!("read_file")));
        // Arguments start empty and arrive in the deltas.
        assert_eq!(start.pointer("/function/arguments"), Some(&json!("")));

        // Each delta reuses the same index, so a client concatenates them.
        let fragments: Vec<String> = out
            .iter()
            .skip(1)
            .filter_map(|chunk| {
                chunk
                    .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(fragments.join(""), r#"{"path":"a.rs"}"#);
        for chunk in out.iter().skip(1) {
            assert_eq!(
                chunk.pointer("/choices/0/delta/tool_calls/0/index"),
                Some(&json!(0))
            );
        }
    }

    #[test]
    fn a_whole_tool_call_after_deltas_is_dropped_rather_than_duplicated() {
        let out = run(&[
            json!({ "type": "tool-input-start", "id": "tc_1", "toolName": "read_file" }),
            json!({ "type": "tool-input-delta", "id": "tc_1", "delta": "{}" }),
            // The consolidated event for the same call. Emitting it would give the
            // client the call twice, with its arguments doubled.
            json!({ "type": "tool-call", "toolCallId": "tc_1", "toolName": "read_file", "input": {} }),
        ]);
        assert_eq!(
            out.len(),
            2,
            "the consolidated call was re-emitted: {out:?}"
        );
    }

    #[test]
    fn a_whole_tool_call_with_no_deltas_is_emitted_once() {
        let out = run(&[json!({
            "type": "tool-call",
            "toolCallId": "tc_9",
            "toolName": "list_dir",
            "input": { "path": "." },
        })]);
        let call = out
            .first()
            .and_then(|chunk| chunk.pointer("/choices/0/delta/tool_calls/0"))
            .expect("call");
        assert_eq!(call.get("id"), Some(&json!("tc_9")));
        // Arguments are a JSON string, which is what an OpenAI client parses.
        assert_eq!(
            call.pointer("/function/arguments"),
            Some(&json!(r#"{"path":"."}"#))
        );
        assert_eq!(
            out.first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/role")),
            Some(&json!("assistant")),
            "a tool-only turn still opens the message"
        );
    }

    #[test]
    fn two_distinct_tool_calls_get_distinct_indices() {
        let out = run(&[
            json!({ "type": "tool-input-start", "id": "a", "toolName": "one" }),
            json!({ "type": "tool-input-start", "id": "b", "toolName": "two" }),
        ]);
        assert_eq!(
            out.first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/tool_calls/0/index")),
            Some(&json!(0))
        );
        assert_eq!(
            out.get(1)
                .and_then(|chunk| chunk.pointer("/choices/0/delta/tool_calls/0/index")),
            Some(&json!(1)),
            "a second call sharing index 0 would overwrite the first"
        );
    }

    #[test]
    fn a_delta_for_an_unknown_call_is_ignored() {
        // Its index is unknown, and guessing would merge it into another call.
        assert!(
            run(&[json!({ "type": "tool-input-delta", "id": "never-started", "delta": "{}" })])
                .is_empty()
        );
    }

    #[test]
    fn the_finish_event_carries_the_reason_and_the_totals() {
        let out = run(&[
            json!({ "type": "text-delta", "text": "hi" }),
            json!({
                "type": "finish-step",
                "finishReason": "tool-calls",
                "usage": { "inputTokens": 4, "outputTokens": 2 },
            }),
            json!({
                "type": "finish",
                "totalUsage": { "inputTokens": 5, "outputTokens": 3, "totalTokens": 8 },
            }),
        ]);
        // `finish-step` reports but does not emit: the turn is not over.
        assert_eq!(out.len(), 2, "got {out:?}");
        let last = out.last().expect("final chunk");
        // CommandCode's `tool-calls` is OpenAI's `tool_calls`.
        assert_eq!(
            last.pointer("/choices/0/finish_reason"),
            Some(&json!("tool_calls"))
        );
        // `totalUsage` wins over the per-step numbers.
        assert_eq!(last.pointer("/usage/prompt_tokens"), Some(&json!(5)));
        assert_eq!(last.pointer("/usage/completion_tokens"), Some(&json!(3)));
        assert_eq!(last.pointer("/usage/total_tokens"), Some(&json!(8)));
    }

    #[test]
    fn a_finish_with_no_totals_falls_back_to_the_step_usage() {
        let out = run(&[
            json!({ "type": "finish-step", "finishReason": "stop", "usage": { "inputTokens": 7, "outputTokens": 1 } }),
            json!({ "type": "finish" }),
        ]);
        let last = out.last().expect("final chunk");
        assert_eq!(last.pointer("/usage/prompt_tokens"), Some(&json!(7)));
        assert_eq!(
            last.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
    }

    #[test]
    fn an_error_event_is_delivered_as_content_and_then_finishes() {
        let out = run(&[json!({ "type": "error", "error": "rate limited" })]);
        // Two chunks: the message, then a terminator. A silent truncation would look
        // like a short answer rather than a failure.
        assert_eq!(out.len(), 2);
        assert!(
            out.first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content"))
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("rate limited")),
            "got {out:?}"
        );
        assert_eq!(
            out.last()
                .and_then(|chunk| chunk.pointer("/choices/0/finish_reason")),
            Some(&json!("stop"))
        );
    }

    #[test]
    fn an_already_openai_chunk_passes_through() {
        let chunk = json!({
            "object": "chat.completion.chunk",
            "choices": [{ "index": 0, "delta": { "content": "hi" } }],
        });
        assert_eq!(run(std::slice::from_ref(&chunk)), vec![chunk]);
    }
}
