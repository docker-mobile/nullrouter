//! OpenAI request -> Claude request.
//!
//! Ports `open-sse/translator/request/openai-to-claude.js`.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::schema::{
    CLAUDE_SYSTEM_PROMPT, DEFAULT_MAX_TOKENS, adjust_max_tokens, claude_block,
    extract_text_content, openai_block, parse_data_uri, role, safe_parse_json,
};

/// Upstream uses an empty prefix: a `proxy_` prefix was a detectable
/// fingerprint difference against real Claude Code.
const CLAUDE_OAUTH_TOOL_PREFIX: &str = "";

/// Claude accepts only these `tool_choice` types; anything else is a 400.
const CLAUDE_TOOL_CHOICE_TYPES: [&str; 4] = ["auto", "any", "tool", "none"];

/// A translated Claude request plus the tool-name mapping needed to translate
/// the response back.
#[derive(Debug, Clone)]
pub struct TranslatedRequest {
    pub body: Value,
    /// Renamed tool name -> original name.
    pub tool_name_map: BTreeMap<String, String>,
}

/// Translate an OpenAI request body into Claude shape.
///
/// `model_ceiling` is the model's real output cap; pass
/// [`crate::schema::DEFAULT_MAX_TOKENS`] when unknown.
pub fn translate(model: &str, body: &Value, stream: bool, model_ceiling: u64) -> TranslatedRequest {
    let mut tool_name_map = BTreeMap::new();
    let mut result = Map::new();

    result.insert("model".to_owned(), json!(model));
    result.insert(
        "max_tokens".to_owned(),
        json!(adjust_max_tokens(body, model_ceiling)),
    );
    result.insert("stream".to_owned(), json!(stream));

    if let Some(temperature) = body.get("temperature") {
        result.insert("temperature".to_owned(), temperature.clone());
    }

    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    if let Some(source) = body.get("messages").and_then(Value::as_array) {
        for message in source {
            if message.get("role").and_then(Value::as_str) == Some(role::SYSTEM) {
                let content = message.get("content");
                let text = match content {
                    Some(Value::String(text)) => text.clone(),
                    other => extract_text_content(other, "\n"),
                };
                system_parts.push(text);
            }
        }

        messages = merge_messages(source, &mut tool_name_map);
        apply_cache_control(&mut messages);
    }

    result.insert("messages".to_owned(), Value::Array(messages));

    if let Some(format) = body.get("response_format")
        && let Some(instruction) = json_mode_instruction(format)
    {
        system_parts.push(instruction);
    }

    result.insert("system".to_owned(), build_system(&system_parts));

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let converted = convert_tools(tools, &mut tool_name_map);
        if !converted.is_empty() {
            result.insert("tools".to_owned(), Value::Array(converted));
        }
    }

    if let Some(choice) = body.get("tool_choice") {
        result.insert("tool_choice".to_owned(), convert_tool_choice(choice));
    }

    TranslatedRequest {
        body: Value::Object(result),
        tool_name_map,
    }
}

/// Merge consecutive same-role messages, keeping `tool_result` in its own
/// message immediately after the matching `tool_use`.
fn merge_messages(source: &[Value], tool_name_map: &mut BTreeMap<String, String>) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    let mut current_role: Option<&str> = None;
    let mut current_parts: Vec<Value> = Vec::new();

    for message in source
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) != Some(role::SYSTEM))
    {
        let message_role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let new_role = if message_role == role::USER || message_role == role::TOOL {
            role::USER
        } else {
            role::ASSISTANT
        };

        let blocks = content_blocks(message, tool_name_map);
        let has_tool_use = blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some(claude_block::TOOL_USE));
        let has_tool_result = blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some(claude_block::TOOL_RESULT)
        });

        if has_tool_result {
            let (results, others): (Vec<Value>, Vec<Value>) =
                blocks.into_iter().partition(|block| {
                    block.get("type").and_then(Value::as_str) == Some(claude_block::TOOL_RESULT)
                });

            flush(&mut messages, current_role, &mut current_parts);

            if !results.is_empty() {
                messages.push(json!({ "role": role::USER, "content": results }));
            }
            if !others.is_empty() {
                current_role = Some(new_role);
                current_parts.extend(others);
            }
            continue;
        }

        if current_role != Some(new_role) {
            flush(&mut messages, current_role, &mut current_parts);
            current_role = Some(new_role);
        }

        current_parts.extend(blocks);

        if has_tool_use {
            flush(&mut messages, current_role, &mut current_parts);
        }
    }

    flush(&mut messages, current_role, &mut current_parts);
    messages
}

/// Emit the accumulated parts as one message, if any.
///
/// Mirrors upstream `flushCurrentMessage`, which clears the parts but leaves
/// `currentRole` set.
fn flush(messages: &mut Vec<Value>, current_role: Option<&str>, parts: &mut Vec<Value>) {
    if let Some(role) = current_role
        && !parts.is_empty()
    {
        messages.push(json!({ "role": role, "content": std::mem::take(parts) }));
    }
    parts.clear();
}

/// Mark the last cacheable block of the last assistant message as ephemeral.
fn apply_cache_control(messages: &mut [Value]) {
    // thinking blocks may not carry cache_control.
    const CACHEABLE: [&str; 4] = [
        claude_block::TEXT,
        claude_block::TOOL_USE,
        claude_block::TOOL_RESULT,
        claude_block::IMAGE,
    ];

    for message in messages.iter_mut().rev() {
        if message.get("role").and_then(Value::as_str) != Some(role::ASSISTANT) {
            continue;
        }
        let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        if blocks.is_empty() {
            break;
        }
        for block in blocks.iter_mut().rev() {
            let cacheable = block
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| CACHEABLE.contains(&kind));
            if cacheable && let Some(object) = block.as_object_mut() {
                object.insert("cache_control".to_owned(), json!({ "type": "ephemeral" }));
                break;
            }
        }
        break;
    }
}

/// System blocks: the Claude Code prompt, plus the caller's system text.
fn build_system(system_parts: &[String]) -> Value {
    let prompt = json!({ "type": claude_block::TEXT, "text": CLAUDE_SYSTEM_PROMPT });
    let joined = system_parts.join("\n");
    if system_parts.is_empty() || joined.is_empty() {
        return json!([prompt]);
    }
    json!([
        prompt,
        {
            "type": claude_block::TEXT,
            "text": joined,
            "cache_control": { "type": "ephemeral", "ttl": "1h" },
        },
    ])
}

/// Turn `response_format` into a system instruction, since Claude has no
/// native JSON mode.
fn json_mode_instruction(format: &Value) -> Option<String> {
    match format.get("type").and_then(Value::as_str) {
        Some("json_schema") => {
            let schema = format
                .get("json_schema")
                .and_then(|wrapper| wrapper.get("schema"))?;
            let rendered = serde_json::to_string_pretty(schema).ok()?;
            Some(format!(
                "You must respond with valid JSON that strictly follows this JSON schema:\n\
                 ```json\n{rendered}\n```\n\
                 Respond ONLY with the JSON object, no other text."
            ))
        }
        Some("json_object") => Some(
            "You must respond with valid JSON. Respond ONLY with a JSON object, no other text."
                .to_owned(),
        ),
        _ => None,
    }
}

fn convert_tools(tools: &[Value], tool_name_map: &mut BTreeMap<String, String>) -> Vec<Value> {
    let mut converted: Vec<Value> = Vec::new();

    for tool in tools {
        let tool_type = tool.get("type").and_then(Value::as_str);
        // Built-in tools (e.g. web_search_*) pass through untouched.
        if let Some(kind) = tool_type
            && kind != openai_block::FUNCTION
        {
            converted.push(tool.clone());
            continue;
        }

        let data = if tool_type == Some(openai_block::FUNCTION) {
            tool.get("function").unwrap_or(tool)
        } else {
            tool
        };
        let original_name = data.get("name").and_then(Value::as_str).unwrap_or_default();
        let tool_name = format!("{CLAUDE_OAUTH_TOOL_PREFIX}{original_name}");
        tool_name_map.insert(tool_name.clone(), original_name.to_owned());

        converted.push(json!({
            "name": tool_name,
            "description": data.get("description").and_then(Value::as_str).unwrap_or_default(),
            "input_schema": data
                .get("parameters")
                .or_else(|| data.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({
                    "type": "object",
                    "properties": {},
                    "required": [],
                })),
        }));
    }

    if let Some(last) = converted.last_mut()
        && let Some(object) = last.as_object_mut()
    {
        object.insert(
            "cache_control".to_owned(),
            json!({ "type": "ephemeral", "ttl": "1h" }),
        );
    }

    converted
}

/// Extract Claude content blocks from one OpenAI message.
fn content_blocks(message: &Value, tool_name_map: &mut BTreeMap<String, String>) -> Vec<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    let message_role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content = message.get("content");

    if message_role == role::TOOL {
        blocks.push(json!({
            "type": claude_block::TOOL_RESULT,
            "tool_use_id": message.get("tool_call_id").cloned().unwrap_or(Value::Null),
            "content": content.cloned().unwrap_or(Value::Null),
        }));
        return blocks;
    }

    if message_role == role::USER {
        match content {
            Some(Value::String(text)) if !text.is_empty() => {
                blocks.push(json!({ "type": claude_block::TEXT, "text": text }));
            }
            Some(Value::Array(parts)) => {
                for part in parts {
                    push_user_part(&mut blocks, part);
                }
            }
            _ => {}
        }
        return blocks;
    }

    if message_role == role::ASSISTANT {
        match content {
            Some(Value::Array(parts)) => {
                for part in parts {
                    push_assistant_part(&mut blocks, part);
                }
            }
            Some(Value::String(text)) if !text.is_empty() => {
                blocks.push(json!({ "type": claude_block::TEXT, "text": text }));
            }
            Some(other) if !other.is_null() => {
                let text = extract_text_content(Some(other), "\n");
                if !text.is_empty() {
                    blocks.push(json!({ "type": claude_block::TEXT, "text": text }));
                }
            }
            _ => {}
        }

        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                if call.get("type").and_then(Value::as_str) != Some(openai_block::FUNCTION) {
                    continue;
                }
                let function = call.get("function");
                let original = function
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tool_name = format!("{CLAUDE_OAUTH_TOOL_PREFIX}{original}");
                tool_name_map.insert(tool_name.clone(), original.to_owned());
                let arguments = function
                    .and_then(|function| function.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                blocks.push(json!({
                    "type": claude_block::TOOL_USE,
                    "id": call.get("id").cloned().unwrap_or(Value::Null),
                    "name": tool_name,
                    "input": safe_parse_json(arguments),
                }));
            }
        }
    }

    blocks
}

fn push_user_part(blocks: &mut Vec<Value>, part: &Value) {
    match part.get("type").and_then(Value::as_str) {
        Some(openai_block::TEXT) => {
            if let Some(text) = part
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                blocks.push(json!({ "type": claude_block::TEXT, "text": text }));
            }
        }
        Some(claude_block::TOOL_RESULT) => {
            let mut block = json!({
                "type": claude_block::TOOL_RESULT,
                "tool_use_id": part.get("tool_use_id").cloned().unwrap_or(Value::Null),
                "content": part.get("content").cloned().unwrap_or(Value::Null),
            });
            if let Some(is_error) = part
                .get("is_error")
                .filter(|flag| flag.as_bool() == Some(true))
                && let Some(object) = block.as_object_mut()
            {
                object.insert("is_error".to_owned(), is_error.clone());
            }
            blocks.push(block);
        }
        Some(openai_block::IMAGE_URL) => {
            let Some(url) = part
                .get("image_url")
                .and_then(|image| image.get("url"))
                .and_then(Value::as_str)
            else {
                return;
            };
            if let Some(parsed) = parse_data_uri(url) {
                blocks.push(json!({
                    "type": claude_block::IMAGE,
                    "source": {
                        "type": "base64",
                        "media_type": parsed.mime_type,
                        "data": parsed.base64,
                    },
                }));
            } else if url.starts_with("http://") || url.starts_with("https://") {
                blocks.push(json!({
                    "type": claude_block::IMAGE,
                    "source": { "type": "url", "url": url },
                }));
            }
        }
        Some(openai_block::IMAGE) => {
            if let Some(source) = part.get("source") {
                blocks.push(json!({ "type": claude_block::IMAGE, "source": source }));
            }
        }
        Some(openai_block::FILE) => {
            // Claude accepts PDF documents only.
            let Some(data) = part
                .get("file")
                .and_then(|file| file.get("file_data"))
                .and_then(Value::as_str)
            else {
                return;
            };
            if let Some(parsed) = parse_data_uri(data)
                && parsed.mime_type == "application/pdf"
            {
                blocks.push(json!({
                    "type": claude_block::DOCUMENT,
                    "source": {
                        "type": "base64",
                        "media_type": parsed.mime_type,
                        "data": parsed.base64,
                    },
                }));
            }
        }
        _ => {}
    }
}

fn push_assistant_part(blocks: &mut Vec<Value>, part: &Value) {
    match part.get("type").and_then(Value::as_str) {
        Some(openai_block::TEXT) => {
            if let Some(text) = part
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                blocks.push(json!({ "type": claude_block::TEXT, "text": text }));
            }
        }
        Some(claude_block::TOOL_USE) => {
            blocks.push(json!({
                "type": claude_block::TOOL_USE,
                "id": part.get("id").cloned().unwrap_or(Value::Null),
                "name": part.get("name").cloned().unwrap_or(Value::Null),
                "input": part.get("input").cloned().unwrap_or_else(|| json!({})),
            }));
        }
        Some(claude_block::THINKING) => {
            // cache_control is not allowed on thinking blocks.
            let mut block = part.clone();
            if let Some(object) = block.as_object_mut() {
                object.remove("cache_control");
            }
            blocks.push(block);
        }
        _ => {}
    }
}

/// OpenAI `tool_choice` -> Claude `tool_choice`.
///
/// Never forwards a type Claude would reject.
fn convert_tool_choice(choice: &Value) -> Value {
    if choice.is_null() {
        return json!({ "type": "auto" });
    }
    if let Some(text) = choice.as_str() {
        return if text == "required" {
            json!({ "type": "any" })
        } else {
            json!({ "type": "auto" })
        };
    }
    if choice.is_object() {
        // The OpenAI forced-tool shape also carries type "function", which
        // Claude rejects, so this is checked before the pass-through.
        if let Some(name) = choice
            .get("function")
            .and_then(|function| function.get("name"))
            .filter(|name| !name.is_null())
        {
            return json!({ "type": "tool", "name": name });
        }
        if let Some(kind) = choice.get("type").and_then(Value::as_str)
            && CLAUDE_TOOL_CHOICE_TYPES.contains(&kind)
        {
            return choice.clone();
        }
    }
    json!({ "type": "auto" })
}

/// Ceiling used when the caller has no model-specific capability data.
pub const fn default_ceiling() -> u64 {
    DEFAULT_MAX_TOKENS
}
