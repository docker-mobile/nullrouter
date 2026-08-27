//! Gemini streaming responses -> OpenAI `chat.completion.chunk` stream.
//!
//! Ports `open-sse/translator/response/gemini-to-openai.js`. Also serves the
//! `gemini-cli`, `antigravity`, and `vertex` formats, which share this shape.

use serde_json::{Value, json};

use crate::concerns::{
    ChunkMeta, UsageKind, build_chunk, reasoning_delta, to_openai_finish, to_openai_usage,
};
use crate::schema::{DEFAULT_IMAGE_MIME, encode_data_uri, openai_block, openai_finish, role};
use crate::state::StreamState;

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

/// Translate one Gemini response chunk into zero or more OpenAI chunks.
pub fn translate(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    let mut out = Vec::new();
    // Antigravity wraps the Gemini payload in `response`.
    let response = raw.get("response").unwrap_or(raw);
    let Some(candidate) = response
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
    else {
        return out;
    };

    if state.message_id.is_none() {
        state.message_id = Some(
            response
                .get("responseId")
                .and_then(Value::as_str)
                .map_or_else(
                    || format!("msg_{}", state.clock.now_millis()),
                    str::to_owned,
                ),
        );
        state.model = Some(
            response
                .get("modelVersion")
                .and_then(Value::as_str)
                .unwrap_or("gemini")
                .to_owned(),
        );
        state.function_index = 0;
        state.gemini_tool_call_count = 0;
        out.push(chunk(state, json!({ "role": role::ASSISTANT }), None));
    }

    if let Some(parts) = candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    {
        for part in parts {
            translate_part(part, state, &mut out);
        }
    }

    // Usage is read before the finish reason so it can ride the final chunk.
    let usage_meta = response
        .get("usageMetadata")
        .or_else(|| raw.get("usageMetadata"));
    if let Some(usage) = usage_meta.and_then(|meta| to_openai_usage(meta, UsageKind::Gemini)) {
        state.usage = Some(usage);
    }

    if let Some(reason) = candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    {
        let mut finish = to_openai_finish(reason, "gemini");
        // Gemini reports STOP even when it emitted tool calls.
        if finish == openai_finish::STOP && state.gemini_tool_call_count > 0 {
            openai_finish::TOOL_CALLS.clone_into(&mut finish);
        }
        let mut final_chunk = chunk(state, json!({}), Some(&finish));
        if let Some(usage) = state.usage
            && let Some(object) = final_chunk.as_object_mut()
        {
            object.insert("usage".to_owned(), usage.to_value());
        }
        out.push(final_chunk);
        state.finish_reason = Some(finish);
    }

    out
}

fn translate_part(part: &Value, state: &mut StreamState, out: &mut Vec<Value>) {
    let has_thought_signature =
        part.get("thoughtSignature").is_some() || part.get("thought_signature").is_some();
    let is_thought = part.get("thought").and_then(Value::as_bool) == Some(true);
    let text = part
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty());

    if has_thought_signature {
        if let Some(text) = text {
            out.push(chunk(state, text_delta(text, is_thought), None));
        }
        if let Some(call) = part.get("functionCall") {
            out.push(emit_function_call(call, state));
        }
        return;
    }

    // Gemini marks internal reasoning with `thought: true`, sometimes without a
    // signature; those must not surface as assistant content.
    if let Some(text) = text {
        out.push(chunk(state, text_delta(text, is_thought), None));
    }

    if let Some(call) = part.get("functionCall") {
        out.push(emit_function_call(call, state));
    }

    let inline = part.get("inlineData").or_else(|| part.get("inline_data"));
    if let Some(data) = inline
        .and_then(|inline| inline.get("data"))
        .and_then(Value::as_str)
    {
        let mime = inline
            .and_then(|inline| inline.get("mimeType").or_else(|| inline.get("mime_type")))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_IMAGE_MIME);
        out.push(chunk(
            state,
            json!({
                "images": [{
                    "type": openai_block::IMAGE_URL,
                    "image_url": { "url": encode_data_uri(mime, data) },
                }],
            }),
            None,
        ));
    }
}

fn text_delta(text: &str, is_thought: bool) -> Value {
    if is_thought {
        reasoning_delta(text, false)
    } else {
        json!({ "content": text })
    }
}

fn emit_function_call(call: &Value, state: &mut StreamState) -> Value {
    let raw_name = call.get("name").and_then(Value::as_str).unwrap_or_default();
    let name = state.original_tool_name(raw_name);
    let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
    let tool_index = state.function_index;
    state.function_index += 1;
    state.gemini_tool_call_count += 1;

    let arguments = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_owned());
    let id = format!("{name}-{}-{tool_index}", state.clock.now_millis());
    build_chunk(
        &chunk_meta(state),
        json!({
            "tool_calls": [{
                "id": id,
                "index": tool_index,
                "type": openai_block::FUNCTION,
                "function": { "name": name, "arguments": arguments },
            }],
        }),
        None,
    )
}
