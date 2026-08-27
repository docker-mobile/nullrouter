//! OpenAI Responses API request -> OpenAI Chat Completions request.
//!
//! Ports `open-sse/translator/request/openai-responses.js`.
//!
//! The Responses API replaces `messages[]` with a flat `input[]` of typed items
//! (`message`, `function_call`, `function_call_output`, `reasoning`, ...) plus a
//! separate `instructions` string. Items must be regrouped into chat turns:
//! consecutive `function_call` items collapse into one assistant message with
//! `tool_calls`, and each output becomes its own `tool` message.

use serde_json::{Map, Value, json};

use crate::schema::{openai_block, role};

/// Responses API item type discriminators.
mod item {
    pub(super) const MESSAGE: &str = "message";
    pub(super) const FUNCTION_CALL: &str = "function_call";
    pub(super) const FUNCTION_CALL_OUTPUT: &str = "function_call_output";
    pub(super) const CUSTOM_TOOL_CALL: &str = "custom_tool_call";
    pub(super) const CUSTOM_TOOL_CALL_OUTPUT: &str = "custom_tool_call_output";
    pub(super) const ADDITIONAL_TOOLS: &str = "additional_tools";
    pub(super) const REASONING: &str = "reasoning";
    pub(super) const OUTPUT_TEXT: &str = "output_text";
    pub(super) const INPUT_TEXT: &str = "input_text";
    pub(super) const INPUT_IMAGE: &str = "input_image";
}

/// The Responses API rejects `call_id` values longer than this.
const MAX_CALL_ID_LEN: usize = 64;

/// Truncate an over-long call id rather than letting upstream reject the request.
fn clamp_call_id(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    value.as_str().map_or_else(
        || value.clone(),
        |id| {
            if id.len() > MAX_CALL_ID_LEN {
                Value::String(id.chars().take(MAX_CALL_ID_LEN).collect())
            } else {
                Value::String(id.to_owned())
            }
        },
    )
}

/// Normalize `input` to an item array (upstream `normalizeResponsesInput`).
///
/// A string becomes one user message. An empty array becomes a placeholder
/// message: every provider rejects an empty `messages[]`.
fn normalize_input(input: &Value) -> Option<Vec<Value>> {
    match input {
        Value::String(text) => {
            let text = if text.trim().is_empty() { "..." } else { text };
            Some(vec![json!({
                "type": item::MESSAGE,
                "role": role::USER,
                "content": [{ "type": item::INPUT_TEXT, "text": text }],
            })])
        }
        Value::Array(items) if items.is_empty() => Some(vec![json!({
            "type": item::MESSAGE,
            "role": role::USER,
            "content": [{ "type": item::INPUT_TEXT, "text": "..." }],
        })]),
        Value::Array(items) => Some(items.clone()),
        _ => None,
    }
}

/// Reasoning text from a `reasoning` item's `summary[]` or `content[]`.
fn reasoning_text(entry: &Value) -> String {
    for key in ["summary", "content"] {
        if let Some(parts) = entry.get(key).and_then(Value::as_array) {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

/// Accumulated reasoning waiting to attach to the next assistant turn.
#[derive(Default)]
struct PendingReasoning {
    text: String,
    /// `encrypted_content`, needed for `store: false` multi-turn continuity.
    encrypted: String,
}

impl PendingReasoning {
    /// Attach and clear, so reasoning lands on exactly one assistant message.
    fn attach(&mut self, message: &mut Map<String, Value>) {
        if !self.text.is_empty() {
            message.insert("reasoning_content".to_owned(), json!(self.text));
        }
        if !self.encrypted.is_empty() {
            message.insert("encrypted_content".to_owned(), json!(self.encrypted));
        }
        self.clear();
    }

    fn clear(&mut self) {
        self.text.clear();
        self.encrypted.clear();
    }
}

/// Translate a Responses API request body into Chat Completions shape.
///
/// Returns the body unchanged when it carries no `input`, matching upstream.
#[allow(
    clippy::too_many_lines,
    reason = "one linear regrouping pass over input[]; splitting it would obscure the item ordering rules it encodes"
)]
pub fn translate(model: &str, body: &Value, stream: bool) -> Value {
    let Some(input) = body.get("input") else {
        return body.clone();
    };
    let Some(items) = normalize_input(input) else {
        return body.clone();
    };

    let Some(source) = body.as_object() else {
        return body.clone();
    };
    let mut result = source.clone();
    let mut messages: Vec<Value> = Vec::new();

    // `instructions` is the Responses API's system prompt.
    if let Some(instructions) = body
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        messages.push(json!({ "role": role::SYSTEM, "content": instructions }));
    }

    let mut current_assistant: Option<Map<String, Value>> = None;
    let mut pending = PendingReasoning::default();
    let mut additional_tools: Vec<Value> = Vec::new();
    let mut custom_tool_names: Vec<String> = Vec::new();

    for entry in &items {
        // Some clients omit `type` and rely on `role` alone.
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| entry.get("role").map(|_| item::MESSAGE))
            .unwrap_or_default();

        match kind {
            item::MESSAGE => {
                if let Some(assistant) = current_assistant.take() {
                    messages.push(Value::Object(assistant));
                }
                let mut message = Map::new();
                message.insert(
                    "role".to_owned(),
                    entry
                        .get("role")
                        .cloned()
                        .unwrap_or_else(|| json!(role::USER)),
                );
                message.insert("content".to_owned(), convert_content(entry.get("content")));
                if entry.get("role").and_then(Value::as_str) == Some(role::ASSISTANT) {
                    pending.attach(&mut message);
                } else {
                    // Reasoning only belongs to an assistant turn.
                    pending.clear();
                }
                messages.push(Value::Object(message));
            }
            item::FUNCTION_CALL | item::CUSTOM_TOOL_CALL => {
                // A nameless tool call is rejected upstream, so it is skipped
                // before any assistant turn is opened for it.
                let Some(name) = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                else {
                    continue;
                };

                // Consecutive calls accumulate into one assistant turn.
                let is_new_turn = current_assistant.is_none();
                let assistant = current_assistant.get_or_insert_with(|| {
                    let mut message = Map::new();
                    message.insert("role".to_owned(), json!(role::ASSISTANT));
                    message.insert("content".to_owned(), Value::Null);
                    message.insert("tool_calls".to_owned(), Value::Array(Vec::new()));
                    message
                });
                // Buffered reasoning belongs to the turn that carries the call,
                // and only to the first call in it.
                if is_new_turn {
                    pending.attach(assistant);
                }

                let is_custom = kind == item::CUSTOM_TOOL_CALL;
                if is_custom && !custom_tool_names.iter().any(|known| known == name) {
                    custom_tool_names.push(name.to_owned());
                }

                // Chat Completions has no freeform custom tool, so its raw input
                // is wrapped as a single `input` argument.
                let arguments = if is_custom {
                    let raw = entry.get("input").map_or_else(String::new, |value| {
                        value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_owned)
                    });
                    serde_json::to_string(&json!({ "input": raw }))
                        .unwrap_or_else(|_| "{}".to_owned())
                } else {
                    entry
                        .get("arguments")
                        .map_or_else(|| "{}".to_owned(), stringify_arguments)
                };

                if let Some(calls) = assistant
                    .get_mut("tool_calls")
                    .and_then(Value::as_array_mut)
                {
                    calls.push(json!({
                        "id": clamp_call_id(entry.get("call_id")),
                        "type": openai_block::FUNCTION,
                        "function": { "name": name, "arguments": arguments },
                    }));
                }
            }
            item::FUNCTION_CALL_OUTPUT | item::CUSTOM_TOOL_CALL_OUTPUT => {
                if let Some(assistant) = current_assistant.take() {
                    messages.push(Value::Object(assistant));
                }
                let output = entry.get("output").map_or_else(String::new, |value| {
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned)
                });
                messages.push(json!({
                    "role": role::TOOL,
                    "tool_call_id": clamp_call_id(entry.get("call_id")),
                    "content": output,
                }));
            }
            item::ADDITIONAL_TOOLS => {
                if let Some(tools) = entry.get("tools").and_then(Value::as_array) {
                    additional_tools.extend(tools.iter().cloned());
                }
            }
            item::REASONING => {
                let text = reasoning_text(entry);
                if !text.is_empty() {
                    if pending.text.is_empty() {
                        pending.text = text;
                    } else {
                        pending.text.push('\n');
                        pending.text.push_str(&text);
                    }
                }
                if let Some(encrypted) = entry
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    encrypted.clone_into(&mut pending.encrypted);
                }
            }
            _ => {}
        }
    }

    if let Some(assistant) = current_assistant.take() {
        messages.push(Value::Object(assistant));
    }

    result.insert("model".to_owned(), json!(model));
    result.insert("messages".to_owned(), Value::Array(messages));
    result.insert("stream".to_owned(), json!(stream));

    // Tools: Responses declares `{type, name, parameters}`; Chat wants
    // `{type: "function", function: {...}}`. Hosted tools carry no name and
    // cannot be represented, so they are dropped.
    let declared = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let all_tools: Vec<Value> = declared.into_iter().chain(additional_tools).collect();
    if all_tools.is_empty() {
        result.remove("tools");
    } else {
        let converted: Vec<Value> = all_tools
            .iter()
            .filter_map(|tool| convert_tool(tool, &mut custom_tool_names))
            .collect();
        if converted.is_empty() {
            result.remove("tools");
        } else {
            result.insert("tools".to_owned(), Value::Array(converted));
        }
    }

    // `max_output_tokens` is Responses-only; map it rather than leaking it.
    if let Some(max_output) = result.remove("max_output_tokens")
        && !result.contains_key("max_tokens")
    {
        result.insert("max_tokens".to_owned(), max_output);
    }

    // `reasoning.effort` becomes the Chat-level field.
    if let Some(effort) = body
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
    {
        result.insert("reasoning_effort".to_owned(), json!(effort));
    }

    // Strip Responses-only fields so they never reach a Chat provider.
    for field in [
        "input",
        "instructions",
        "include",
        "prompt_cache_key",
        "store",
        "reasoning",
        "client_metadata",
    ] {
        result.remove(field);
    }

    Value::Object(result)
}

/// Serialize a tool-call `arguments` value, which may already be a string.
fn stringify_arguments(value: &Value) -> String {
    value.as_str().map_or_else(
        || serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned()),
        str::to_owned,
    )
}

/// Convert Responses content parts to Chat content parts.
fn convert_content(content: Option<&Value>) -> Value {
    let Some(Value::Array(parts)) = content else {
        return content.cloned().unwrap_or(Value::Null);
    };
    let converted: Vec<Value> = parts
        .iter()
        .map(|part| match part.get("type").and_then(Value::as_str) {
            // Both input_text and output_text flatten to a plain text part.
            Some(item::INPUT_TEXT | item::OUTPUT_TEXT) => json!({
                "type": openai_block::TEXT,
                "text": part.get("text").cloned().unwrap_or(Value::Null),
            }),
            Some(item::INPUT_IMAGE) => {
                let url = part
                    .get("image_url")
                    .or_else(|| part.get("file_id"))
                    .cloned()
                    .unwrap_or_else(|| json!(""));
                let detail = part.get("detail").cloned().unwrap_or_else(|| json!("auto"));
                json!({
                    "type": openai_block::IMAGE_URL,
                    "image_url": { "url": url, "detail": detail },
                })
            }
            // Anything else passes through untouched.
            _ => part.clone(),
        })
        .collect();
    Value::Array(converted)
}

/// Convert one Responses tool declaration to Chat shape.
fn convert_tool(tool: &Value, custom_tool_names: &mut Vec<String>) -> Option<Value> {
    // Already Chat-shaped.
    if tool.get("function").is_some() {
        return Some(tool.clone());
    }
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())?;

    if tool.get("type").and_then(Value::as_str) == Some("custom") {
        if !custom_tool_names.iter().any(|known| known == name) {
            custom_tool_names.push(name.to_owned());
        }
        let format_hint = ["syntax", "definition"]
            .iter()
            .filter_map(|key| {
                tool.get("format")
                    .and_then(|format| format.get(*key))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let description = [
            tool.get("description")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            &format_hint,
        ]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n");

        return Some(json!({
            "type": openai_block::FUNCTION,
            "function": {
                "name": name,
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "input": {
                            "type": "string",
                            "description": "Raw freeform input for this custom tool",
                        },
                    },
                    "required": ["input"],
                    "additionalProperties": false,
                },
            },
        }));
    }

    let mut function = Map::new();
    function.insert("name".to_owned(), json!(name));
    function.insert(
        "description".to_owned(),
        json!(
            tool.get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
    );
    function.insert(
        "parameters".to_owned(),
        normalize_tool_parameters(tool.get("parameters")),
    );
    if let Some(strict) = tool.get("strict") {
        function.insert("strict".to_owned(), strict.clone());
    }
    Some(json!({ "type": openai_block::FUNCTION, "function": function }))
}

/// An object schema must always carry `properties` (upstream
/// `normalizeToolParameters`).
fn normalize_tool_parameters(params: Option<&Value>) -> Value {
    let Some(params) = params.filter(|value| !value.is_null()) else {
        return json!({ "type": "object", "properties": {} });
    };
    if params.get("type").and_then(Value::as_str) == Some("object")
        && params.get("properties").is_none()
    {
        let mut filled = params.as_object().cloned().unwrap_or_default();
        filled.insert("properties".to_owned(), json!({}));
        return Value::Object(filled);
    }
    params.clone()
}
