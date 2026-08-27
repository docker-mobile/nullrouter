//! OpenAI request -> Gemini request.
//!
//! Ports `openaiToGeminiBase` / `openaiToGeminiRequest` from
//! `open-sse/translator/request/openai-to-gemini.js`. The Cloud Code envelope
//! variants (`gemini-cli`, `antigravity`) are custom-executor formats and are
//! not ported.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::request::gemini_schema::{
    clean_schema, default_safety_settings, sanitize_function_name,
};
use crate::schema::{extract_text_content, gemini_role, openai_block, role, safe_parse_json};

/// Translate an OpenAI request body into Gemini shape.
#[allow(
    clippy::too_many_lines,
    reason = "mirrors upstream openaiToGeminiBase as one linear pass; splitting it would obscure the ported order of operations"
)]
pub fn translate(model: &str, body: &Value, _stream: bool) -> Value {
    let mut result = Map::new();
    result.insert("model".to_owned(), json!(model));

    let mut generation_config = Map::new();
    for (source, target) in [
        ("temperature", "temperature"),
        ("top_p", "topP"),
        ("top_k", "topK"),
        ("max_tokens", "maxOutputTokens"),
    ] {
        if let Some(value) = body.get(source) {
            generation_config.insert(target.to_owned(), value.clone());
        }
    }

    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[][..], |messages| messages.as_slice());

    // tool_call_id -> function name, so a functionResponse can be named.
    let mut call_names: BTreeMap<String, String> = BTreeMap::new();
    // tool_call_id -> reply content.
    let mut tool_responses: BTreeMap<String, Value> = BTreeMap::new();
    for message in messages {
        let message_role = message.get("role").and_then(Value::as_str);
        if message_role == Some(role::ASSISTANT)
            && let Some(calls) = message.get("tool_calls").and_then(Value::as_array)
        {
            for call in calls {
                let is_function =
                    call.get("type").and_then(Value::as_str) == Some(openai_block::FUNCTION);
                let id = call.get("id").and_then(Value::as_str);
                let name = call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str);
                if is_function && let (Some(id), Some(name)) = (id, name) {
                    call_names.insert(id.to_owned(), name.to_owned());
                }
            }
        }
        if message_role == Some(role::TOOL)
            && let Some(id) = message.get("tool_call_id").and_then(Value::as_str)
        {
            tool_responses.insert(
                id.to_owned(),
                message.get("content").cloned().unwrap_or(Value::Null),
            );
        }
    }

    let mut contents: Vec<Value> = Vec::new();
    let mut system_instruction: Option<Value> = None;

    for message in messages {
        let message_role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = message.get("content");

        // A lone system message becomes a user turn; otherwise it is hoisted
        // into systemInstruction.
        if message_role == role::SYSTEM && messages.len() > 1 {
            let text = content.map_or_else(String::new, |content| {
                content
                    .as_str()
                    .map_or_else(|| extract_text_content(Some(content), ""), str::to_owned)
            });
            system_instruction = Some(json!({
                "role": gemini_role::USER,
                "parts": [{ "text": text }],
            }));
            continue;
        }

        if message_role == role::USER || (message_role == role::SYSTEM && messages.len() == 1) {
            let parts = content_to_parts(content);
            if !parts.is_empty() {
                contents.push(json!({ "role": gemini_role::USER, "parts": parts }));
            }
            continue;
        }

        if message_role == role::ASSISTANT {
            push_assistant_content(
                message,
                content,
                &call_names,
                &tool_responses,
                &mut contents,
            );
        }
    }

    if let Some(instruction) = system_instruction {
        result.insert("systemInstruction".to_owned(), instruction);
    }
    result.insert(
        "contents".to_owned(),
        Value::Array(normalize_contents(contents)),
    );
    result.insert(
        "generationConfig".to_owned(),
        Value::Object(generation_config),
    );
    result.insert("safetySettings".to_owned(), default_safety_settings());

    if let Some(tools) = body.get("tools").and_then(Value::as_array)
        && !tools.is_empty()
    {
        let declarations = convert_tools(tools);
        if !declarations.is_empty() {
            result.insert(
                "tools".to_owned(),
                json!([{ "functionDeclarations": declarations }]),
            );
        }
    }

    Value::Object(result)
}

fn push_assistant_content(
    message: &Value,
    content: Option<&Value>,
    call_names: &BTreeMap<String, String>,
    tool_responses: &BTreeMap<String, Value>,
    contents: &mut Vec<Value>,
) {
    let mut parts: Vec<Value> = Vec::new();

    // Reasoning becomes a thought part. The upstream thought-signature parts
    // belong to the Cloud Code variants, which are out of scope here.
    if let Some(reasoning) = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|reasoning| !reasoning.is_empty())
    {
        parts.push(json!({ "thought": true, "text": reasoning }));
    }

    if let Some(content) = content.filter(|content| !content.is_null()) {
        let text = content
            .as_str()
            .map_or_else(|| extract_text_content(Some(content), ""), str::to_owned);
        if !text.is_empty() {
            parts.push(json!({ "text": text }));
        }
    }

    let calls = message.get("tool_calls").and_then(Value::as_array);
    let Some(calls) = calls else {
        if !parts.is_empty() {
            contents.push(json!({ "role": gemini_role::MODEL, "parts": parts }));
        }
        return;
    };

    let mut call_ids: Vec<String> = Vec::new();
    for call in calls {
        if call.get("type").and_then(Value::as_str) != Some(openai_block::FUNCTION) {
            continue;
        }
        let function = call.get("function");
        let name = function
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = function
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or("{}");
        let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        parts.push(json!({
            "functionCall": {
                "id": id,
                "name": sanitize_function_name(name),
                "args": safe_parse_json(arguments),
            },
        }));
        call_ids.push(id.to_owned());
    }

    if !parts.is_empty() {
        contents.push(json!({ "role": gemini_role::MODEL, "parts": parts }));
    }

    // Only emit functionResponse parts when replies actually exist.
    let has_responses = call_ids.iter().any(|id| tool_responses.contains_key(id));
    if !has_responses {
        return;
    }

    let mut tool_parts: Vec<Value> = Vec::new();
    for id in &call_ids {
        let Some(response) = tool_responses.get(id) else {
            continue;
        };
        let name = call_names
            .get(id)
            .cloned()
            .unwrap_or_else(|| derive_name_from_id(id));
        let parsed = match response {
            Value::String(text) => {
                let parsed = safe_parse_json(text);
                if parsed.is_object() || parsed.is_array() {
                    parsed
                } else {
                    json!({ "result": parsed })
                }
            }
            Value::Null => json!({ "result": Value::Null }),
            other if other.is_object() || other.is_array() => other.clone(),
            other => json!({ "result": other }),
        };
        tool_parts.push(json!({
            "functionResponse": {
                "id": id,
                "name": sanitize_function_name(&name),
                "response": { "result": parsed },
            },
        }));
    }
    if !tool_parts.is_empty() {
        contents.push(json!({ "role": gemini_role::USER, "parts": tool_parts }));
    }
}

/// Recover a function name from a generated call id when no mapping exists
/// (upstream drops the trailing two dash-separated segments).
fn derive_name_from_id(id: &str) -> String {
    let segments: Vec<&str> = id.split('-').collect();
    if segments.len() > 2 {
        segments
            .get(..segments.len() - 2)
            .unwrap_or_default()
            .join("-")
    } else {
        id.to_owned()
    }
}

/// OpenAI content -> Gemini parts (upstream `convertOpenAIContentToParts`).
fn content_to_parts(content: Option<&Value>) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    match content {
        Some(Value::String(text)) => parts.push(json!({ "text": text })),
        Some(Value::Array(items)) => {
            for item in items {
                push_content_part(&mut parts, item);
            }
        }
        _ => {}
    }
    parts
}

fn push_content_part(parts: &mut Vec<Value>, item: &Value) {
    match item.get("type").and_then(Value::as_str) {
        Some(openai_block::TEXT) => {
            parts.push(json!({ "text": item.get("text").cloned().unwrap_or(Value::Null) }));
        }
        Some(openai_block::IMAGE_URL) => {
            let Some(url) = item
                .get("image_url")
                .and_then(|image| image.get("url"))
                .and_then(Value::as_str)
            else {
                return;
            };
            if let Some(rest) = url.strip_prefix("data:") {
                // Upstream splits on the first comma and takes the mime before `;`.
                if let Some(comma) = rest.find(',') {
                    let mime = rest
                        .get(..comma)
                        .unwrap_or_default()
                        .split(';')
                        .next()
                        .unwrap_or_default();
                    let data = rest.get(comma + 1..).unwrap_or_default();
                    parts.push(json!({
                        "inlineData": { "mime_type": mime, "data": data },
                    }));
                }
            } else if url.starts_with("http://") || url.starts_with("https://") {
                parts.push(json!({
                    "fileData": { "fileUri": url, "mimeType": "image/*" },
                }));
            }
        }
        Some(openai_block::INPUT_AUDIO) => {
            let audio = item.get("input_audio");
            let Some(data) = audio
                .and_then(|audio| audio.get("data"))
                .and_then(Value::as_str)
            else {
                return;
            };
            let format = audio
                .and_then(|audio| audio.get("format"))
                .and_then(Value::as_str)
                .unwrap_or("wav");
            let mime = if format == "mp3" {
                "audio/mpeg".to_owned()
            } else {
                format!("audio/{format}")
            };
            parts.push(json!({
                "inlineData": { "mime_type": mime, "data": data },
            }));
        }
        _ => {}
    }
}

/// Merge consecutive same-role contents (upstream `normalizeGeminiContents`).
fn normalize_contents(contents: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for content in contents {
        let role_matches = content.get("role").and_then(Value::as_str).is_some();
        let has_parts = content
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| !parts.is_empty());
        if !role_matches || !has_parts {
            continue;
        }

        let same_as_last = out
            .last()
            .and_then(|last| last.get("role"))
            .zip(content.get("role"))
            .is_some_and(|(last, current)| last == current);

        if same_as_last
            && let Some(last) = out.last_mut()
            && let Some(target) = last.get_mut("parts").and_then(Value::as_array_mut)
            && let Some(parts) = content.get("parts").and_then(Value::as_array)
        {
            target.extend(parts.iter().cloned());
            continue;
        }
        out.push(content);
    }
    out
}

fn convert_tools(tools: &[Value]) -> Vec<Value> {
    let mut declarations: Vec<Value> = Vec::new();
    for tool in tools {
        // Claude-shaped tool (name + input_schema, no type).
        if let Some(name) = tool.get("name").and_then(Value::as_str)
            && let Some(schema) = tool.get("input_schema")
        {
            declarations.push(json!({
                "name": sanitize_function_name(name),
                "description": tool.get("description").and_then(Value::as_str).unwrap_or_default(),
                "parameters": clean_schema(schema),
            }));
            continue;
        }
        // OpenAI-shaped tool.
        if tool.get("type").and_then(Value::as_str) == Some(openai_block::FUNCTION)
            && let Some(function) = tool.get("function")
        {
            let schema = function
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            declarations.push(json!({
                "name": sanitize_function_name(
                    function.get("name").and_then(Value::as_str).unwrap_or_default(),
                ),
                "description": function
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "parameters": clean_schema(&schema),
            }));
        }
    }
    declarations
}
