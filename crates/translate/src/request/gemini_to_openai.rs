//! Gemini request -> OpenAI request.
//!
//! Ports `open-sse/translator/request/gemini-to-openai.js`.

use serde_json::{Map, Value, json};

use crate::schema::{
    DEFAULT_MAX_TOKENS, adjust_max_tokens, collapse_text_parts, encode_data_uri, gemini_role,
    openai_block, role,
};

/// Translate a Gemini request body into OpenAI shape.
pub fn translate(model: &str, body: &Value, stream: bool) -> Value {
    let mut result = Map::new();
    result.insert("model".to_owned(), json!(model));

    let mut messages: Vec<Value> = Vec::new();

    if let Some(config) = body.get("generationConfig") {
        if let Some(max_output) = config
            .get("maxOutputTokens")
            .and_then(Value::as_u64)
            .filter(|value| *value != 0)
        {
            // Upstream reuses adjustMaxTokens via a synthetic body so the
            // tool-present floor still applies.
            let synthetic = json!({
                "max_tokens": max_output,
                "tools": body.get("tools").cloned().unwrap_or(Value::Null),
            });
            result.insert(
                "max_tokens".to_owned(),
                json!(adjust_max_tokens(&synthetic, DEFAULT_MAX_TOKENS)),
            );
        }
        if let Some(temperature) = config.get("temperature") {
            result.insert("temperature".to_owned(), temperature.clone());
        }
        if let Some(top_p) = config.get("topP") {
            result.insert("top_p".to_owned(), top_p.clone());
        }
    }

    if let Some(instruction) = body.get("systemInstruction") {
        let text = extract_gemini_text(instruction);
        if !text.is_empty() {
            messages.push(json!({ "role": role::SYSTEM, "content": text }));
        }
    }

    if let Some(contents) = body.get("contents").and_then(Value::as_array) {
        for content in contents {
            if let Some(message) = convert_content(content) {
                messages.push(message);
            }
        }
    }

    result.insert("messages".to_owned(), Value::Array(messages));
    result.insert("stream".to_owned(), json!(stream));

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let mut converted: Vec<Value> = Vec::new();
        for tool in tools {
            let Some(declarations) = tool.get("functionDeclarations").and_then(Value::as_array)
            else {
                continue;
            };
            for function in declarations {
                converted.push(json!({
                    "type": openai_block::FUNCTION,
                    "function": {
                        "name": function.get("name").cloned().unwrap_or(Value::Null),
                        "description": function
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        "parameters": function
                            .get("parameters")
                            .cloned()
                            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                    },
                }));
            }
        }
        result.insert("tools".to_owned(), Value::Array(converted));
    }

    Value::Object(result)
}

fn convert_content(content: &Value) -> Option<Value> {
    let mapped_role = if content.get("role").and_then(Value::as_str) == Some(gemini_role::USER) {
        role::USER
    } else {
        role::ASSISTANT
    };
    let parts_source = content.get("parts").and_then(Value::as_array)?;

    let mut parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    for part in parts_source {
        if let Some(text) = part.get("text") {
            parts.push(json!({ "type": openai_block::TEXT, "text": text }));
        }

        if let Some(inline) = part.get("inlineData") {
            let mime = inline
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let data = inline
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            parts.push(json!({
                "type": openai_block::IMAGE_URL,
                "image_url": { "url": encode_data_uri(mime, data) },
            }));
        }

        if let Some(call) = part.get("functionCall") {
            let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            // Gemini has no native call id, so a deterministic one is derived
            // from the name to keep call/response pairing intact.
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| format!("call_{name}"), str::to_owned);
            tool_calls.push(json!({
                "id": id,
                "type": openai_block::FUNCTION,
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(&args)
                        .unwrap_or_else(|_| "{}".to_owned()),
                },
            }));
        }

        // A functionResponse short-circuits the whole content into a tool message.
        if let Some(response) = part.get("functionResponse") {
            let name = response
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = response
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| format!("call_{name}"), str::to_owned);
            let payload = response
                .get("response")
                .and_then(|value| value.get("result"))
                .or_else(|| response.get("response"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            return Some(json!({
                "role": role::TOOL,
                "tool_call_id": id,
                "content": serde_json::to_string(&payload)
                    .unwrap_or_else(|_| "{}".to_owned()),
            }));
        }
    }

    if !tool_calls.is_empty() {
        let mut object = Map::new();
        object.insert("role".to_owned(), json!(role::ASSISTANT));
        if !parts.is_empty() {
            // Upstream uses parts[0].text for a single part here, not
            // collapseTextParts.
            let content = if parts.len() == 1 {
                parts
                    .first()
                    .and_then(|part| part.get("text"))
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                Value::Array(parts)
            };
            object.insert("content".to_owned(), content);
        }
        object.insert("tool_calls".to_owned(), Value::Array(tool_calls));
        return Some(Value::Object(object));
    }

    if !parts.is_empty() {
        return Some(json!({
            "role": mapped_role,
            "content": collapse_text_parts(parts),
        }));
    }

    None
}

/// Concatenate the text of a Gemini content wrapper
/// (upstream `extractGeminiText`, empty separator).
fn extract_gemini_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .get("parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .map(|part| part.get("text").and_then(Value::as_str).unwrap_or_default())
                .collect::<String>()
        })
        .unwrap_or_default()
}
