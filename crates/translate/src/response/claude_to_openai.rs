//! Claude SSE events -> OpenAI `chat.completion.chunk` stream.
//!
//! Ports `open-sse/translator/response/claude-to-openai.js`.

use serde_json::{Value, json};

use crate::concerns::{
    ChunkMeta, Usage, UsageKind, build_chunk, reasoning_delta, to_openai_finish, to_openai_usage,
};
use crate::schema::{claude_block, openai_finish, role};
use crate::state::{OpenAiToolCall, StreamState};

fn chunk_meta(state: &StreamState) -> ChunkMeta {
    ChunkMeta {
        id: format!(
            "chatcmpl-{}",
            state.message_id.as_deref().unwrap_or_default()
        ),
        created: state.clock.now_seconds(),
        model: state.model.clone().unwrap_or_default(),
    }
}

fn chunk(state: &StreamState, delta: Value, finish_reason: Option<&str>) -> Value {
    build_chunk(&chunk_meta(state), delta, finish_reason)
}

/// Translate one Claude stream event into zero or more OpenAI chunks.
pub fn translate(event: &Value, state: &mut StreamState) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(event_type) = event.get("type").and_then(Value::as_str) else {
        return out;
    };

    match event_type {
        "message_start" => handle_message_start(event, state, &mut out),
        "content_block_start" => handle_block_start(event, state, &mut out),
        "content_block_delta" => handle_block_delta(event, state, &mut out),
        "content_block_stop" => handle_block_stop(event, state, &mut out),
        "message_delta" => handle_message_delta(event, state, &mut out),
        "message_stop" => handle_message_stop(state, &mut out),
        _ => {}
    }

    out
}

fn handle_message_start(event: &Value, state: &mut StreamState, out: &mut Vec<Value>) {
    let message = event.get("message");
    state.message_id = Some(
        message
            .and_then(|message| message.get("id"))
            .and_then(Value::as_str)
            .map_or_else(
                || format!("msg_{}", state.clock.now_millis()),
                str::to_owned,
            ),
    );
    state.model = message
        .and_then(|message| message.get("model"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    state.tool_call_index = 0;

    // Claude reports input + cache counts here and only output_tokens in
    // message_delta, so the cache figures are captured now.
    if let Some(usage) = message
        .and_then(|message| message.get("usage"))
        .filter(|usage| usage.is_object())
    {
        let read = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        state.claude_input_tokens = read("input_tokens");
        state.claude_cache_read_tokens = read("cache_read_input_tokens");
        state.claude_cache_creation_tokens = read("cache_creation_input_tokens");
        state.claude_output_tokens = 0;
        let prompt = state.claude_input_tokens
            + state.claude_cache_read_tokens
            + state.claude_cache_creation_tokens;
        state.usage = Some(Usage {
            prompt_tokens: prompt,
            completion_tokens: 0,
            total_tokens: prompt,
            cached_tokens: state.claude_cache_read_tokens,
            cache_creation_tokens: state.claude_cache_creation_tokens,
            reasoning_tokens: 0,
        });
    }

    out.push(chunk(state, json!({ "role": role::ASSISTANT }), None));
}

fn handle_block_start(event: &Value, state: &mut StreamState, out: &mut Vec<Value>) {
    let index = event.get("index").and_then(Value::as_u64);
    let block = event.get("content_block");
    let block_type = block
        .and_then(|block| block.get("type"))
        .and_then(Value::as_str);

    match block_type {
        // Claude runs built-in tools itself; skip the whole block.
        Some("server_tool_use") => state.server_tool_block_index = index,
        Some(claude_block::TEXT) => state.text_block_started = true,
        Some(claude_block::THINKING) => {
            state.in_thinking_block = true;
            state.current_block_index = index;
            out.push(chunk(state, json!({ "content": "<think>" }), None));
        }
        Some(claude_block::TOOL_USE) => {
            let tool_index = state.tool_call_index;
            state.tool_call_index += 1;
            let raw_name = block
                .and_then(|block| block.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = state.original_tool_name(raw_name);
            let id = block
                .and_then(|block| block.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let call = OpenAiToolCall {
                index: tool_index,
                id: id.clone(),
                name: name.clone(),
                arguments: String::new(),
            };
            if let Some(index) = index {
                state.openai_tool_calls.insert(index, call);
            }
            out.push(chunk(
                state,
                json!({
                    "tool_calls": [{
                        "index": tool_index,
                        "id": id,
                        "type": crate::schema::openai_block::FUNCTION,
                        "function": { "name": name, "arguments": "" },
                    }],
                }),
                None,
            ));
        }
        _ => {}
    }
}

fn handle_block_delta(event: &Value, state: &mut StreamState, out: &mut Vec<Value>) {
    let index = event.get("index").and_then(Value::as_u64);
    if index.is_some() && index == state.server_tool_block_index {
        return;
    }
    let Some(delta) = event.get("delta") else {
        return;
    };

    match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => {
            if let Some(text) = delta.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                out.push(chunk(state, json!({ "content": text }), None));
            }
        }
        Some("thinking_delta") => {
            if let Some(text) = delta.get("thinking").and_then(Value::as_str)
                && !text.is_empty()
            {
                out.push(chunk(state, reasoning_delta(text, false), None));
            }
        }
        Some("input_json_delta") => {
            let Some(partial) = delta
                .get("partial_json")
                .and_then(Value::as_str)
                .filter(|partial| !partial.is_empty())
            else {
                return;
            };
            let Some(index) = index else { return };
            let Some(call) = state.openai_tool_calls.get_mut(&index) else {
                return;
            };
            call.arguments.push_str(partial);
            let (call_index, call_id) = (call.index, call.id.clone());
            out.push(chunk(
                state,
                json!({
                    "tool_calls": [{
                        "index": call_index,
                        "id": call_id,
                        "function": { "arguments": partial },
                    }],
                }),
                None,
            ));
        }
        _ => {}
    }
}

fn handle_block_stop(event: &Value, state: &mut StreamState, out: &mut Vec<Value>) {
    let index = event.get("index").and_then(Value::as_u64);
    if index.is_some() && index == state.server_tool_block_index {
        state.server_tool_block_index = None;
        return;
    }
    if state.in_thinking_block && index == state.current_block_index {
        out.push(chunk(state, json!({ "content": "</think>" }), None));
        state.in_thinking_block = false;
    }
    state.text_block_started = false;
    state.thinking_block_started = false;
}

fn handle_message_delta(event: &Value, state: &mut StreamState, out: &mut Vec<Value>) {
    if let Some(usage) = event.get("usage").filter(|usage| usage.is_object()) {
        // Anthropic sends input/cache once in message_start and only output
        // here, so absent fields fall back to what was captured earlier.
        let read =
            |key: &str, previous: u64| usage.get(key).and_then(Value::as_u64).unwrap_or(previous);
        state.claude_input_tokens = read("input_tokens", state.claude_input_tokens);
        state.claude_output_tokens = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        state.claude_cache_read_tokens =
            read("cache_read_input_tokens", state.claude_cache_read_tokens);
        state.claude_cache_creation_tokens = read(
            "cache_creation_input_tokens",
            state.claude_cache_creation_tokens,
        );

        let prompt = state.claude_input_tokens
            + state.claude_cache_read_tokens
            + state.claude_cache_creation_tokens;
        state.usage = Some(Usage {
            prompt_tokens: prompt,
            completion_tokens: state.claude_output_tokens,
            total_tokens: prompt + state.claude_output_tokens,
            cached_tokens: state.claude_cache_read_tokens,
            cache_creation_tokens: state.claude_cache_creation_tokens,
            reasoning_tokens: 0,
        });
    }

    let Some(stop_reason) = event
        .get("delta")
        .and_then(|delta| delta.get("stop_reason"))
        .and_then(Value::as_str)
    else {
        return;
    };

    let finish = to_openai_finish(stop_reason, "claude");
    state.finish_reason = Some(finish.clone());
    let mut final_chunk = chunk(state, json!({}), Some(&finish));

    if state.usage.is_some() {
        // Rebuilt from merged state (message_start cache + message_delta output).
        let merged = json!({
            "input_tokens": state.claude_input_tokens,
            "output_tokens": state.claude_output_tokens,
            "cache_read_input_tokens": state.claude_cache_read_tokens,
            "cache_creation_input_tokens": state.claude_cache_creation_tokens,
        });
        if let Some(usage) = to_openai_usage(&merged, UsageKind::Claude)
            && let Some(object) = final_chunk.as_object_mut()
        {
            object.insert("usage".to_owned(), usage.to_value());
        }
    }

    out.push(final_chunk);
    state.finish_reason_sent = true;
}

fn handle_message_stop(state: &mut StreamState, out: &mut Vec<Value>) {
    if state.finish_reason_sent {
        return;
    }
    let finish = state.finish_reason.clone().unwrap_or_else(|| {
        if state.openai_tool_calls.is_empty() {
            openai_finish::STOP.to_owned()
        } else {
            openai_finish::TOOL_CALLS.to_owned()
        }
    });
    let mut final_chunk = chunk(state, json!({}), Some(&finish));
    if state.usage.is_some()
        && let Some(object) = final_chunk.as_object_mut()
    {
        // message_stop uses the plain input/output totals, without detail
        // sub-objects (upstream builds this shape inline).
        let input = state.claude_input_tokens;
        let output = state.claude_output_tokens;
        object.insert(
            "usage".to_owned(),
            json!({
                "prompt_tokens": input,
                "completion_tokens": output,
                "total_tokens": input + output,
            }),
        );
    }
    out.push(final_chunk);
    state.finish_reason_sent = true;
}
