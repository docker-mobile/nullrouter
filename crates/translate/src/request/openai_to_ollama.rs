//! OpenAI request -> Ollama request.
//!
//! Ports `openaiToOllamaRequest` from
//! `open-sse/translator/request/openai-to-ollama.js`.
//!
//! Ollama's `/api/chat` differs from OpenAI in three ways that matter:
//!
//! * `content` must be a string. A multimodal content array is flattened to its
//!   text blocks, and any images move to `message.images[]` as raw base64 with the
//!   `data:` prefix removed.
//! * Sampling parameters live under `options`, and `max_tokens` is spelled
//!   `num_predict`.
//! * A tool result is addressed by `tool_name`, not `tool_call_id`, so the name is
//!   recovered from the assistant turn that requested the call.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::schema::{openai_block, role, safe_parse_json};

/// Translate an OpenAI request body into Ollama shape.
pub fn translate(model: &str, body: &Value, stream: bool) -> Value {
    let mut result = Map::new();
    result.insert("model".to_owned(), json!(model));
    result.insert("messages".to_owned(), normalize_messages(body));
    result.insert("stream".to_owned(), json!(stream));

    // Sampling options are nested, and only the ones the client sent are carried:
    // sending `num_predict: null` would cap generation at nothing.
    let mut options = Map::new();
    for (source, target) in [
        ("temperature", "temperature"),
        ("max_tokens", "num_predict"),
        ("top_p", "top_p"),
    ] {
        if let Some(value) = body.get(source).filter(|value| !value.is_null()) {
            options.insert(target.to_owned(), value.clone());
        }
    }
    if !options.is_empty() {
        result.insert("options".to_owned(), Value::Object(options));
    }

    // Ollama accepts tool declarations in OpenAI's own shape.
    if let Some(tools) = body.get("tools").filter(|tools| tools.is_array()) {
        result.insert("tools".to_owned(), tools.clone());
    }
    if let Some(choice) = body.get("tool_choice").filter(|value| !value.is_null()) {
        result.insert("tool_choice".to_owned(), choice.clone());
    }

    Value::Object(result)
}

/// Rewrite `messages[]` into Ollama's shape.
///
/// A non-array `messages` is passed through unchanged, matching upstream: this
/// translator is not the place to reject a malformed body, and the provider's own
/// error is more useful than a synthesised one.
fn normalize_messages(body: &Value) -> Value {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return body.get("messages").cloned().unwrap_or(Value::Null);
    };

    // First pass: tool_call_id -> function name, so a tool result can be named.
    let mut call_names: BTreeMap<String, String> = BTreeMap::new();
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some(role::ASSISTANT) {
            continue;
        }
        let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for call in calls {
            let id = call.get("id").and_then(Value::as_str);
            let name = call
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str);
            if let (Some(id), Some(name)) = (id, name) {
                call_names.insert(id.to_owned(), name.to_owned());
            }
        }
    }

    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    for message in messages {
        let message_role = message.get("role").and_then(Value::as_str).unwrap_or("");

        if message_role == role::TOOL {
            let content = normalize_content(message.get("content"));
            // A tool result with no content carries nothing the model can use.
            if content.is_empty() {
                continue;
            }
            // The id is not part of Ollama's wire shape, so the name is looked up.
            // `unknown_tool` matches upstream: a result the model cannot attribute
            // is still better than dropping the turn and leaving a dangling call.
            let name = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .and_then(|id| call_names.get(id))
                .map(String::as_str)
                .or_else(|| message.get("name").and_then(Value::as_str))
                .unwrap_or("unknown_tool");
            out.push(json!({
                "role": role::TOOL,
                "tool_name": name,
                "content": content,
            }));
            continue;
        }

        if message_role == role::ASSISTANT
            && let Some(calls) = message.get("tool_calls").and_then(Value::as_array)
        {
            out.push(json!({
                "role": role::ASSISTANT,
                "content": normalize_content(message.get("content")),
                "tool_calls": calls.iter().map(ollama_tool_call).collect::<Vec<_>>(),
            }));
            continue;
        }

        let content = normalize_content(message.get("content"));
        let images = extract_images(message.get("content"));
        // An empty non-assistant turn is dropped; an empty assistant turn is kept,
        // because it may be the turn a tool call belongs to.
        if content.is_empty() && images.is_empty() && message_role != role::ASSISTANT {
            continue;
        }

        let mut entry = Map::new();
        entry.insert("role".to_owned(), json!(message_role));
        entry.insert("content".to_owned(), json!(content));
        if !images.is_empty() {
            entry.insert("images".to_owned(), json!(images));
        }
        out.push(Value::Object(entry));
    }

    Value::Array(out)
}

/// One OpenAI tool call in Ollama's shape.
///
/// Ollama nests `index` inside `function` and takes `arguments` as an object, where
/// OpenAI puts `index` beside it and sends arguments as a JSON string.
fn ollama_tool_call(call: &Value) -> Value {
    let function = call.get("function");
    let name = function
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let raw = function.and_then(|function| function.get("arguments"));
    let arguments = match raw {
        Some(Value::String(text)) => {
            let parsed = safe_parse_json(text);
            // `safe_parse_json` returns the raw string when it is not JSON. Ollama
            // wants an object, so an unparseable argument string becomes empty
            // rather than a string where an object belongs.
            if parsed.is_object() {
                parsed
            } else {
                json!({})
            }
        }
        Some(other) => other.clone(),
        None => json!({}),
    };
    json!({
        "type": openai_block::FUNCTION,
        "function": {
            "index": call.get("index").and_then(Value::as_u64).unwrap_or(0),
            "name": name,
            "arguments": arguments,
        },
    })
}

/// Flatten content to the string Ollama requires.
fn normalize_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some(openai_block::TEXT))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Pull base64 images out of OpenAI multimodal blocks.
///
/// Ollama wants raw base64 in `images[]`, so the `data:` URI wrapper is stripped. A
/// remote URL is skipped rather than forwarded: Ollama would treat it as base64 and
/// fail on it.
fn extract_images(content: Option<&Value>) -> Vec<String> {
    let Some(blocks) = content.and_then(Value::as_array) else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some(openai_block::IMAGE_URL))
        .filter_map(|block| {
            let url = block.get("image_url");
            match url {
                Some(Value::String(text)) => Some(text.clone()),
                Some(object) => object.get("url").and_then(Value::as_str).map(str::to_owned),
                None => None,
            }
        })
        .filter_map(|url| base64_from_data_uri(&url))
        .collect()
}

/// The base64 payload of a `data:` URI, or `None` for anything else.
fn base64_from_data_uri(url: &str) -> Option<String> {
    let rest = url.strip_prefix("data:")?;
    let (_mime, payload) = rest.split_once(";base64,")?;
    (!payload.is_empty()).then(|| payload.to_owned())
}

#[cfg(test)]
mod tests {
    use super::translate;
    use serde_json::{Value, json};

    #[test]
    fn sampling_options_are_nested_and_renamed() {
        let body = json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "temperature": 0.4,
            "max_tokens": 128,
            "top_p": 0.9,
        });
        let out = translate("llama3.2", &body, true);

        assert_eq!(out.get("model").and_then(Value::as_str), Some("llama3.2"));
        assert_eq!(out.get("stream").and_then(Value::as_bool), Some(true));
        // `max_tokens` is `num_predict` here; sending the OpenAI spelling would
        // leave generation uncapped.
        assert_eq!(out.pointer("/options/num_predict"), Some(&json!(128)));
        assert_eq!(out.pointer("/options/temperature"), Some(&json!(0.4)));
        assert_eq!(out.pointer("/options/top_p"), Some(&json!(0.9)));
        // Nothing sampling-related is left at the top level.
        assert!(out.get("max_tokens").is_none());
        assert!(out.get("temperature").is_none());
    }

    #[test]
    fn a_body_with_no_sampling_parameters_sends_no_options() {
        let body = json!({ "messages": [{ "role": "user", "content": "hi" }] });
        let out = translate("llama3.2", &body, false);
        // An empty `options` is not sent: `num_predict: null` would cap output at
        // nothing on some builds.
        assert!(out.get("options").is_none(), "{out}");
    }

    #[test]
    fn multimodal_content_is_split_into_text_and_raw_base64_images() {
        let body = json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "what is this" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAB" } },
                    { "type": "text", "text": "in one word" },
                ],
            }],
        });
        let out = translate("llava", &body, false);
        let message = out.pointer("/messages/0").expect("message");

        // Content is a string, never an array.
        assert_eq!(
            message.get("content").and_then(Value::as_str),
            Some("what is this\nin one word")
        );
        // Images are raw base64 with the data: wrapper removed.
        assert_eq!(message.pointer("/images/0"), Some(&json!("AAAB")));
    }

    #[test]
    fn a_remote_image_url_is_dropped_rather_than_sent_as_base64() {
        let body = json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "look" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/cat.png" } },
                ],
            }],
        });
        let out = translate("llava", &body, false);
        // Forwarding the URL would have Ollama decode "https://…" as base64.
        assert!(
            out.pointer("/messages/0/images").is_none(),
            "a URL was forwarded as image data: {out}"
        );
        assert_eq!(
            out.pointer("/messages/0/content").and_then(Value::as_str),
            Some("look")
        );
    }

    #[test]
    fn a_tool_result_is_addressed_by_name_recovered_from_the_call() {
        let body = json!({
            "messages": [
                { "role": "user", "content": "weather?" },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "get_weather", "arguments": "{\"city\":\"Oslo\"}" },
                    }],
                },
                { "role": "tool", "tool_call_id": "call_1", "content": "12C" },
            ],
        });
        let out = translate("llama3.2", &body, false);

        // The assistant turn keeps its call, with arguments as an object.
        let call = out.pointer("/messages/1/tool_calls/0").expect("call");
        assert_eq!(call.pointer("/function/name"), Some(&json!("get_weather")));
        assert_eq!(
            call.pointer("/function/arguments"),
            Some(&json!({ "city": "Oslo" })),
            "arguments must be an object, not a JSON string"
        );
        // An empty assistant turn survives, because it owns the call.
        assert_eq!(
            out.pointer("/messages/1/content").and_then(Value::as_str),
            Some("")
        );

        // The result names the tool: `tool_call_id` is not part of Ollama's shape.
        let result = out.pointer("/messages/2").expect("tool result");
        assert_eq!(result.get("tool_name"), Some(&json!("get_weather")));
        assert!(result.get("tool_call_id").is_none());
        assert_eq!(result.get("content"), Some(&json!("12C")));
    }

    #[test]
    fn an_unmatched_tool_result_falls_back_to_a_name_rather_than_being_dropped() {
        let body = json!({
            "messages": [
                // No assistant turn to recover a name from.
                { "role": "tool", "tool_call_id": "call_gone", "content": "42" },
                { "role": "tool", "name": "explicit_name", "content": "43" },
            ],
        });
        let out = translate("llama3.2", &body, false);
        // Dropping these would leave the conversation missing a turn.
        assert_eq!(
            out.pointer("/messages/0/tool_name"),
            Some(&json!("unknown_tool"))
        );
        // An explicit `name` is preferred over the fallback.
        assert_eq!(
            out.pointer("/messages/1/tool_name"),
            Some(&json!("explicit_name"))
        );
    }

    #[test]
    fn tool_declarations_pass_through_unchanged() {
        let tools = json!([{
            "type": "function",
            "function": { "name": "get_weather", "parameters": { "type": "object" } },
        }]);
        let body = json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": tools,
            "tool_choice": "auto",
        });
        let out = translate("llama3.2", &body, false);
        // Ollama reads OpenAI's own tool shape, so reshaping would be wrong.
        assert_eq!(out.get("tools"), Some(&tools));
        assert_eq!(out.get("tool_choice"), Some(&json!("auto")));
    }
}
