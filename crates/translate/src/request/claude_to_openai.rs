//! Claude request -> OpenAI request.
//!
//! Ports `open-sse/translator/request/claude-to-openai.js`.

use serde_json::{Map, Value, json};

use crate::schema::{
    adjust_max_tokens, claude_block, collapse_text_parts, encode_data_uri, openai_block, role,
};

/// Placeholder inserted for a `tool_call` that never got a reply.
const MISSING_TOOL_RESPONSE: &str = "[No response received]";

/// Strip a leading `x-anthropic-billing-header:` line
/// (upstream `stripAnthropicBillingHeader`, case-insensitive).
fn strip_billing_header(text: &str) -> String {
    const MARKER: &str = "x-anthropic-billing-header:";
    let starts_with_marker = text
        .get(..MARKER.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(MARKER));
    if !starts_with_marker {
        return text.to_owned();
    }
    text.find('\n').map_or_else(String::new, |newline| {
        text.get(newline + 1..).unwrap_or_default().to_owned()
    })
}

/// Translate a Claude request body into OpenAI shape.
pub fn translate(model: &str, body: &Value, stream: bool) -> Value {
    let mut result = Map::new();
    result.insert("model".to_owned(), json!(model));

    let mut messages: Vec<Value> = Vec::new();

    // System prompt becomes a leading system message.
    if let Some(system) = body.get("system") {
        let content = match system {
            Value::Array(blocks) => blocks
                .iter()
                .map(|block| {
                    strip_billing_header(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            Value::String(text) => strip_billing_header(text),
            _ => String::new(),
        };
        if !content.is_empty() {
            messages.push(json!({ "role": role::SYSTEM, "content": content }));
        }
    }

    if let Some(source) = body.get("messages").and_then(Value::as_array) {
        for message in source {
            match convert_message(message) {
                Converted::One(message) => messages.push(message),
                Converted::Many(items) => messages.extend(items),
                Converted::None => {}
            }
        }
    }

    fix_missing_tool_responses(&mut messages);
    result.insert("messages".to_owned(), Value::Array(messages));
    result.insert("stream".to_owned(), json!(stream));

    if body
        .get("max_tokens")
        .and_then(Value::as_u64)
        .is_some_and(|value| value != 0)
    {
        result.insert(
            "max_tokens".to_owned(),
            json!(adjust_max_tokens(body, crate::schema::DEFAULT_MAX_TOKENS)),
        );
    }
    if let Some(temperature) = body.get("temperature") {
        result.insert("temperature".to_owned(), temperature.clone());
    }

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let converted: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "type": openai_block::FUNCTION,
                    "function": {
                        "name": tool.get("name").cloned().unwrap_or(Value::Null),
                        "description": tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        "parameters": tool
                            .get("input_schema")
                            .cloned()
                            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                    },
                })
            })
            .collect();
        result.insert("tools".to_owned(), Value::Array(converted));
    }

    if let Some(choice) = body.get("tool_choice") {
        result.insert("tool_choice".to_owned(), convert_tool_choice(choice));
    }

    // Reasoning effort: explicit field wins, else the nested `reasoning.effort`.
    if let Some(effort) = body.get("reasoning_effort") {
        result.insert("reasoning_effort".to_owned(), effort.clone());
    } else if let Some(effort) = body
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
    {
        result.insert("reasoning_effort".to_owned(), effort.clone());
    }
    if let Some(reasoning) = body.get("reasoning") {
        result.insert("reasoning".to_owned(), reasoning.clone());
    }

    Value::Object(result)
}

/// One Claude message can become zero, one, or several OpenAI messages.
enum Converted {
    None,
    One(Value),
    Many(Vec<Value>),
}

/// Wrap mid-conversation system text so the turn still ends as a user message,
/// avoiding an Anthropic prefill rejection (upstream `systemReminderText`).
fn system_reminder_text(content: Option<&Value>) -> String {
    let text = match content {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some(claude_block::TEXT))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };
    if text.trim().is_empty() {
        return String::new();
    }
    format!("<instructions>\n{text}\n</instructions>")
}

fn convert_message(message: &Value) -> Converted {
    let message_role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if message_role == role::SYSTEM {
        let text = system_reminder_text(message.get("content"));
        return if text.is_empty() {
            Converted::None
        } else {
            Converted::One(json!({ "role": role::USER, "content": text }))
        };
    }

    let mapped_role = if message_role == role::USER || message_role == role::TOOL {
        role::USER
    } else {
        role::ASSISTANT
    };

    match message.get("content") {
        Some(Value::String(text)) => {
            Converted::One(json!({ "role": mapped_role, "content": text }))
        }
        Some(Value::Array(blocks)) => convert_content_blocks(mapped_role, blocks),
        _ => Converted::None,
    }
}

fn convert_content_blocks(mapped_role: &str, blocks: &[Value]) -> Converted {
    let mut parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_results: Vec<Value> = Vec::new();

    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some(claude_block::TEXT) => {
                if let Some(text) = block.get("text") {
                    parts.push(json!({ "type": openai_block::TEXT, "text": text }));
                }
            }
            Some(claude_block::IMAGE) => {
                let source = block.get("source");
                if source
                    .and_then(|source| source.get("type"))
                    .and_then(Value::as_str)
                    == Some("base64")
                {
                    let media_type = source
                        .and_then(|source| source.get("media_type"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let data = source
                        .and_then(|source| source.get("data"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    parts.push(json!({
                        "type": openai_block::IMAGE_URL,
                        "image_url": { "url": encode_data_uri(media_type, data) },
                    }));
                }
            }
            Some(claude_block::TOOL_USE) => {
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                tool_calls.push(json!({
                    "id": block.get("id").cloned().unwrap_or(Value::Null),
                    "type": openai_block::FUNCTION,
                    "function": {
                        "name": block.get("name").cloned().unwrap_or(Value::Null),
                        "arguments": serde_json::to_string(&input)
                            .unwrap_or_else(|_| "{}".to_owned()),
                    },
                }));
            }
            Some(claude_block::TOOL_RESULT) => {
                tool_results.push(json!({
                    "role": role::TOOL,
                    "tool_call_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
                    "content": tool_result_content(block.get("content")),
                }));
            }
            _ => {}
        }
    }

    if !tool_results.is_empty() {
        if parts.is_empty() {
            return Converted::Many(tool_results);
        }
        let mut items = tool_results;
        items.push(json!({ "role": role::USER, "content": collapse_text_parts(parts) }));
        return Converted::Many(items);
    }

    if !tool_calls.is_empty() {
        let mut object = Map::new();
        object.insert("role".to_owned(), json!(role::ASSISTANT));
        if !parts.is_empty() {
            object.insert("content".to_owned(), collapse_text_parts(parts));
        }
        object.insert("tool_calls".to_owned(), Value::Array(tool_calls));
        return Converted::One(Value::Object(object));
    }

    if !parts.is_empty() {
        return Converted::One(json!({
            "role": mapped_role,
            "content": collapse_text_parts(parts),
        }));
    }

    // An explicitly empty content array is preserved as empty text.
    if blocks.is_empty() {
        return Converted::One(json!({ "role": mapped_role, "content": "" }));
    }

    Converted::None
}

/// Flatten a Claude `tool_result.content` into an OpenAI tool message body.
fn tool_result_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => {
            let text = parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some(claude_block::TEXT))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                serde_json::to_string(&parts).unwrap_or_default()
            } else {
                text
            }
        }
        Some(other) if !other.is_null() => serde_json::to_string(other).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Insert placeholder tool replies so every `tool_call` has a response, which
/// OpenAI requires (upstream `fixMissingToolResponsesOpenAI`).
fn fix_missing_tool_responses(messages: &mut Vec<Value>) {
    let mut index = 0;
    while index < messages.len() {
        let call_ids = messages
            .get(index)
            .filter(|message| message.get("role").and_then(Value::as_str) == Some(role::ASSISTANT))
            .and_then(|message| message.get("tool_calls"))
            .and_then(Value::as_array)
            .filter(|calls| !calls.is_empty())
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|call| call.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            });

        let Some(call_ids) = call_ids else {
            index += 1;
            continue;
        };

        // Only the contiguous run of tool replies immediately after counts.
        let mut responded: Vec<String> = Vec::new();
        let mut insert_at = index + 1;
        for scan in index + 1..messages.len() {
            let Some(message) = messages.get(scan) else {
                break;
            };
            let is_tool_reply = message.get("role").and_then(Value::as_str) == Some(role::TOOL);
            let reply_id = message.get("tool_call_id").and_then(Value::as_str);
            match (is_tool_reply, reply_id) {
                (true, Some(id)) => {
                    responded.push(id.to_owned());
                    insert_at = scan + 1;
                }
                _ => break,
            }
        }

        let missing: Vec<String> = call_ids
            .into_iter()
            .filter(|id| !responded.contains(id))
            .collect();
        if missing.is_empty() {
            index += 1;
            continue;
        }

        let count = missing.len();
        for (offset, id) in missing.into_iter().enumerate() {
            messages.insert(
                insert_at + offset,
                json!({
                    "role": role::TOOL,
                    "tool_call_id": id,
                    "content": MISSING_TOOL_RESPONSE,
                }),
            );
        }
        index = insert_at + count;
    }
}

/// Claude `tool_choice` -> OpenAI `tool_choice` (upstream `convertToolChoice`).
fn convert_tool_choice(choice: &Value) -> Value {
    if let Some(text) = choice.as_str() {
        return json!(text);
    }
    match choice.get("type").and_then(Value::as_str) {
        Some("any") => json!("required"),
        Some("tool") => json!({
            "type": openai_block::FUNCTION,
            "function": { "name": choice.get("name").cloned().unwrap_or(Value::Null) },
        }),
        // "auto" and anything unrecognized both mean auto.
        _ => json!("auto"),
    }
}
