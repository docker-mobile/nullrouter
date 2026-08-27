//! OpenAI `chat.completion.chunk` stream -> Claude SSE events.
//!
//! Ports `open-sse/translator/response/openai-to-claude.js`.

use serde_json::{Value, json};

use crate::concerns::{Usage, extract_reasoning_text, from_openai_finish};
use crate::schema::{MODEL_FALLBACK, claude_block, role};
use crate::state::{ClaudeToolCall, StreamState};

/// Legacy prefix stripped defensively so tool names from older turns still
/// resolve. The current request translator emits no prefix, which makes this a
/// no-op — deliberately not coupled to that empty constant.
const CLAUDE_OAUTH_TOOL_PREFIX: &str = "proxy_";

/// Translate one OpenAI chunk into zero or more Claude events.
pub fn translate(chunk: &Value, state: &mut StreamState) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(choice) = chunk
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return out;
    };
    let delta = choice.get("delta");

    capture_usage(chunk, state);
    emit_message_start(chunk, state, &mut out);

    let reasoning = extract_reasoning_text(delta);
    if !reasoning.is_empty() {
        stop_text_block(state, &mut out);
        if !state.thinking_block_started {
            state.thinking_block_index = state.next_block_index;
            state.next_block_index += 1;
            state.thinking_block_started = true;
            out.push(json!({
                "type": "content_block_start",
                "index": state.thinking_block_index,
                "content_block": { "type": claude_block::THINKING, "thinking": "" },
            }));
        }
        out.push(json!({
            "type": "content_block_delta",
            "index": state.thinking_block_index,
            "delta": { "type": "thinking_delta", "thinking": reasoning },
        }));
    }

    if let Some(content) = delta
        .and_then(|delta| delta.get("content"))
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
    {
        stop_thinking_block(state, &mut out);
        if !state.text_block_started {
            state.text_block_index = state.next_block_index;
            state.next_block_index += 1;
            state.text_block_started = true;
            state.text_block_closed = false;
            out.push(json!({
                "type": "content_block_start",
                "index": state.text_block_index,
                "content_block": { "type": claude_block::TEXT, "text": "" },
            }));
        }
        out.push(json!({
            "type": "content_block_delta",
            "index": state.text_block_index,
            "delta": { "type": "text_delta", "text": content },
        }));
    }

    if let Some(tool_calls) = delta
        .and_then(|delta| delta.get("tool_calls"))
        .and_then(Value::as_array)
    {
        handle_tool_calls(tool_calls, state, &mut out);
    }

    if let Some(finish) = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .filter(|finish| !finish.is_empty())
    {
        handle_finish(finish, state, &mut out);
    }

    out
}

fn capture_usage(chunk: &Value, state: &mut StreamState) {
    let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) else {
        return;
    };
    let read = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let prompt = read("prompt_tokens");
    let output = read("completion_tokens");
    let details = usage.get("prompt_tokens_details");
    let detail = |key: &str| {
        details
            .and_then(|details| details.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let cache_read = detail("cached_tokens");
    let cache_create = detail("cache_creation_tokens");

    // OpenAI's prompt_tokens already includes cache tokens, so Claude's
    // input_tokens is the remainder.
    state.claude_input_tokens = prompt
        .saturating_sub(cache_read)
        .saturating_sub(cache_create);
    state.claude_output_tokens = output;
    state.claude_cache_read_tokens = cache_read;
    state.claude_cache_creation_tokens = cache_create;
    state.usage = Some(Usage {
        prompt_tokens: prompt,
        completion_tokens: output,
        total_tokens: prompt + output,
        cached_tokens: cache_read,
        cache_creation_tokens: cache_create,
        reasoning_tokens: 0,
    });
}

fn emit_message_start(chunk: &Value, state: &mut StreamState, out: &mut Vec<Value>) {
    if state.message_start_sent {
        return;
    }
    state.message_start_sent = true;

    let raw_id = chunk
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .strip_prefix("chatcmpl-")
        .map(str::to_owned)
        .or_else(|| chunk.get("id").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();

    let extend = |key: &str| {
        chunk
            .get("extend_fields")
            .and_then(|fields| fields.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    // Generic or too-short ids are replaced with a trace id or a timestamp.
    let message_id = if raw_id.is_empty() || raw_id == "chat" || raw_id.len() < 8 {
        extend("requestId")
            .or_else(|| extend("traceId"))
            .unwrap_or_else(|| format!("msg_{}", state.clock.now_millis()))
    } else {
        raw_id
    };

    state.message_id = Some(message_id.clone());
    state.model = Some(
        chunk
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(MODEL_FALLBACK)
            .to_owned(),
    );
    state.next_block_index = 0;

    out.push(json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": role::ASSISTANT,
            "model": state.model_or_fallback(),
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": { "input_tokens": 0, "output_tokens": 0 },
        },
    }));
}

fn handle_tool_calls(tool_calls: &[Value], state: &mut StreamState, out: &mut Vec<Value>) {
    for call in tool_calls {
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
        let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        let function_name = call
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default();

        // Some vendors repeat id with a null name on every argument chunk, so a
        // block is opened only once per index.
        if !id.is_empty() && !state.claude_tool_calls.contains_key(&index) {
            stop_thinking_block(state, out);
            stop_text_block(state, out);

            let block_index = state.next_block_index;
            state.next_block_index += 1;
            state.claude_tool_calls.insert(
                index,
                ClaudeToolCall {
                    id: id.to_owned(),
                    name: function_name.to_owned(),
                    block_index,
                },
            );

            let display_name = function_name
                .strip_prefix(CLAUDE_OAUTH_TOOL_PREFIX)
                .unwrap_or(function_name);
            out.push(json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": claude_block::TOOL_USE,
                    "id": id,
                    "name": display_name,
                    "input": {},
                },
            }));
        }

        if let Some(arguments) = call
            .get("function")
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str)
            .filter(|arguments| !arguments.is_empty())
            && state.claude_tool_calls.contains_key(&index)
        {
            // Buffered, not streamed: arguments are sanitized at finish.
            state
                .tool_arg_buffers
                .entry(index)
                .or_default()
                .push_str(arguments);
        }
    }
}

fn handle_finish(finish: &str, state: &mut StreamState, out: &mut Vec<Value>) {
    stop_thinking_block(state, out);
    stop_text_block(state, out);

    let calls: Vec<(u64, ClaudeToolCall)> = state
        .claude_tool_calls
        .iter()
        .map(|(index, call)| (*index, call.clone()))
        .collect();
    for (index, call) in calls {
        if let Some(buffered) = state.tool_arg_buffers.get(&index)
            && !buffered.is_empty()
        {
            let sanitized = sanitize_tool_args(&call.name, buffered);
            out.push(json!({
                "type": "content_block_delta",
                "index": call.block_index,
                "delta": { "type": "input_json_delta", "partial_json": sanitized },
            }));
        }
        out.push(json!({
            "type": "content_block_stop",
            "index": call.block_index,
        }));
    }

    state.finish_reason = Some(finish.to_owned());
    out.push(json!({
        "type": "message_delta",
        "delta": { "stop_reason": from_openai_finish(finish, "claude") },
        "usage": claude_usage_value(state),
    }));
    out.push(json!({ "type": "message_stop" }));
}

/// Claude-native usage object for `message_delta`.
fn claude_usage_value(state: &StreamState) -> Value {
    let mut usage = json!({
        "input_tokens": state.claude_input_tokens,
        "output_tokens": state.claude_output_tokens,
    });
    if let Some(object) = usage.as_object_mut() {
        if state.claude_cache_read_tokens > 0 {
            object.insert(
                "cache_read_input_tokens".to_owned(),
                json!(state.claude_cache_read_tokens),
            );
        }
        if state.claude_cache_creation_tokens > 0 {
            object.insert(
                "cache_creation_input_tokens".to_owned(),
                json!(state.claude_cache_creation_tokens),
            );
        }
    }
    usage
}

fn stop_thinking_block(state: &mut StreamState, out: &mut Vec<Value>) {
    if !state.thinking_block_started {
        return;
    }
    out.push(json!({
        "type": "content_block_stop",
        "index": state.thinking_block_index,
    }));
    state.thinking_block_started = false;
}

fn stop_text_block(state: &mut StreamState, out: &mut Vec<Value>) {
    if !state.text_block_started || state.text_block_closed {
        return;
    }
    state.text_block_closed = true;
    out.push(json!({
        "type": "content_block_stop",
        "index": state.text_block_index,
    }));
    state.text_block_started = false;
}

/// Repair tool arguments that non-Anthropic models commonly get wrong
/// (upstream `sanitizeToolArgs`).
fn sanitize_tool_args(tool_name: &str, args_json: &str) -> String {
    let Ok(mut args) = serde_json::from_str::<Value>(args_json) else {
        return args_json.to_owned();
    };
    let name = tool_name
        .strip_prefix(CLAUDE_OAUTH_TOOL_PREFIX)
        .unwrap_or(tool_name);
    if name == "Read" {
        sanitize_read_args(&mut args);
    }
    serde_json::to_string(&args).unwrap_or_else(|_| args_json.to_owned())
}

/// Coerce and clamp `Read` arguments (upstream `sanitizeReadArgs`).
fn sanitize_read_args(args: &mut Value) {
    let Some(object) = args.as_object_mut() else {
        return;
    };

    // Numeric strings become numbers.
    for key in ["limit", "offset"] {
        let coerced = object
            .get(key)
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<i64>().ok());
        if let Some(number) = coerced {
            object.insert(key.to_owned(), json!(number));
        }
    }

    if let Some(limit) = object.get("limit").and_then(Value::as_i64) {
        if limit > 2000 {
            object.insert("limit".to_owned(), json!(2000));
        } else if limit < 1 {
            object.remove("limit");
        }
    }
    if let Some(offset) = object.get("offset").and_then(Value::as_i64)
        && offset < 0
    {
        object.insert("offset".to_owned(), json!(0));
    }

    if object.contains_key("pages") {
        let file_path = object.get("file_path").and_then(Value::as_str);
        let pages = object.get("pages").and_then(Value::as_str);
        if !is_valid_pdf_pages(file_path, pages) {
            object.remove("pages");
        }
    }
}

/// `pages` is only valid as `N` or `N-M` on a `.pdf` path.
fn is_valid_pdf_pages(file_path: Option<&str>, pages: Option<&str>) -> bool {
    let Some(path) = file_path else { return false };
    if !path.to_lowercase().ends_with(".pdf") {
        return false;
    }
    let Some(pages) = pages else { return false };
    let (start, end) = pages
        .split_once('-')
        .map_or((pages, None), |(start, end)| (start, Some(end)));
    let is_digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    is_digits(start) && end.is_none_or(is_digits)
}
