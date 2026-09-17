//! Strip multimodal content blocks a model cannot read, before translation.
//!
//! Ports `open-sse/translator/concerns/modality.js`. When a combo routes
//! to a model without vision/audio/pdf support, this removes the media
//! blocks and replaces them with a short text placeholder so messages
//! never become empty.

use nullrouter_providers::{Capabilities, Format};
use serde_json::{Map, Value, json};

fn placeholder_current(cap: &str) -> &'static str {
    match cap {
        "vision" => "[image omitted: model has no vision support]",
        "audio" => "[audio omitted: model has no audio support]",
        "pdf" => "[file omitted: model has no document support]",
        _ => "[media omitted]",
    }
}

fn placeholder_prev(cap: &str) -> &'static str {
    match cap {
        "vision" => "[Previous image omitted from context.]",
        "audio" => "[Previous audio omitted from context.]",
        "pdf" => "[Previous file omitted from context.]",
        _ => "[Previous media omitted from context.]",
    }
}

fn cap_for_mime(mime: &str) -> Option<&'static str> {
    if mime.starts_with("image/") {
        Some("vision")
    } else if mime.starts_with("audio/") {
        Some("audio")
    } else if mime == "application/pdf" {
        Some("pdf")
    } else {
        None
    }
}

fn cap_for_openai_block(block: &Value) -> Option<&'static str> {
    match block.get("type").and_then(Value::as_str) {
        Some("image_url" | "image") => Some("vision"),
        Some("input_audio" | "audio_url") => Some("audio"),
        Some("file") => Some("pdf"),
        _ => None,
    }
}

fn cap_for_claude_block(block: &Value) -> Option<&'static str> {
    match block.get("type").and_then(Value::as_str) {
        Some("image") => Some("vision"),
        Some("document") => Some("pdf"),
        _ => None,
    }
}

fn lacks(caps: &Capabilities, cap: &str) -> bool {
    match cap {
        "vision" => !caps.vision,
        "audio" => !caps.audio_input,
        "pdf" => !caps.pdf,
        _ => false,
    }
}

/// Filter OpenAI content blocks: drop unsupported media, inject placeholders.
fn strip_content(
    content: &[Value],
    cap_of: fn(&Value) -> Option<&'static str>,
    caps: &Capabilities,
    is_last: bool,
) -> Vec<Value> {
    let mut out = Vec::with_capacity(content.len());
    let mut removed: Vec<&str> = Vec::new();
    for block in content {
        if let Some(cap) = cap_of(block)
            && lacks(caps, cap)
        {
            removed.push(cap);
            continue;
        }
        out.push(block.clone());
    }
    for cap in &removed {
        let ph = if is_last {
            placeholder_current(cap)
        } else {
            placeholder_prev(cap)
        };
        out.push(json!({"type": "text", "text": ph}));
    }
    out
}

/// Strip unsupported modalities from the request body in place.
///
/// Call this BEFORE translation, on the source-format body. `source` is the
/// client's wire format, `caps` is the target model's capabilities.
pub fn strip_unsupported_modalities(body: &mut Value, source: Format, caps: &Capabilities) {
    // Fast exit: model supports everything.
    if caps.vision && caps.audio_input && caps.pdf {
        return;
    }

    let Some(obj) = body.as_object_mut() else {
        return;
    };

    #[allow(
        clippy::match_same_arms,
        reason = "each arm documents a distinct format group"
    )]
    match source {
        Format::OpenAi | Format::Ollama | Format::Kiro | Format::CommandCode => {
            strip_openai_messages(obj, caps);
        }
        Format::Claude => {
            strip_claude_messages(obj, caps);
        }
        Format::OpenAiResponses => {
            strip_responses_input(obj, caps);
        }
        Format::Gemini | Format::GeminiCli | Format::Vertex => {
            strip_gemini_contents(obj, caps);
        }
        _ => {
            strip_openai_messages(obj, caps);
        }
    }
}

fn strip_openai_messages(obj: &mut Map<String, Value>, caps: &Capabilities) {
    let Some(messages) = obj.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    let last = messages.len().saturating_sub(1);
    for (i, msg) in messages.iter_mut().enumerate() {
        let Some(msg_obj) = msg.as_object_mut() else {
            continue;
        };
        if let Some(content) = msg_obj.get_mut("content")
            && let Some(arr) = content.as_array()
        {
            let filtered = strip_content(arr, cap_for_openai_block, caps, i == last);
            *content = Value::Array(filtered);
        }
    }
}

fn strip_claude_messages(obj: &mut Map<String, Value>, caps: &Capabilities) {
    let Some(messages) = obj.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    let last = messages.len().saturating_sub(1);
    for (i, msg) in messages.iter_mut().enumerate() {
        let Some(msg_obj) = msg.as_object_mut() else {
            continue;
        };
        if let Some(content) = msg_obj.get_mut("content")
            && let Some(arr) = content.as_array()
        {
            let filtered = strip_content(arr, cap_for_claude_block, caps, i == last);
            *content = Value::Array(filtered);
        }
    }
}

fn strip_responses_input(obj: &mut Map<String, Value>, caps: &Capabilities) {
    let Some(input) = obj.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    let last = input.len().saturating_sub(1);
    for (i, item) in input.iter_mut().enumerate() {
        let Some(item_obj) = item.as_object_mut() else {
            continue;
        };
        if let Some(content) = item_obj.get_mut("content").and_then(Value::as_array_mut) {
            let mut filtered = Vec::with_capacity(content.len());
            let mut removed: Vec<&str> = Vec::new();
            for block in &*content {
                let cap = match block.get("type").and_then(Value::as_str) {
                    Some("input_image") => Some("vision"),
                    Some("input_file") => Some("pdf"),
                    _ => None,
                };
                if let Some(c) = cap
                    && lacks(caps, c)
                {
                    removed.push(c);
                    continue;
                }
                filtered.push(block.clone());
            }
            for cap in &removed {
                let ph = if i == last {
                    placeholder_current(cap)
                } else {
                    placeholder_prev(cap)
                };
                filtered.push(json!({"type": "input_text", "text": ph}));
            }
            *content = filtered;
        }
    }
}

fn strip_gemini_contents(obj: &mut Map<String, Value>, caps: &Capabilities) {
    let Some(contents) = obj.get_mut("contents").and_then(Value::as_array_mut) else {
        return;
    };
    let last = contents.len().saturating_sub(1);
    for (i, c) in contents.iter_mut().enumerate() {
        let Some(c_obj) = c.as_object_mut() else {
            continue;
        };
        if let Some(parts) = c_obj.get_mut("parts").and_then(Value::as_array_mut) {
            let mut filtered = Vec::with_capacity(parts.len());
            let mut removed: Vec<&str> = Vec::new();
            for part in &*parts {
                let mime = part
                    .pointer("/inlineData/mimeType")
                    .or_else(|| part.pointer("/fileData/mimeType"))
                    .and_then(Value::as_str);
                if let Some(m) = mime
                    && let Some(cap) = cap_for_mime(m)
                    && lacks(caps, cap)
                {
                    removed.push(cap);
                    continue;
                }
                filtered.push(part.clone());
            }
            for cap in &removed {
                let ph = if i == last {
                    placeholder_current(cap)
                } else {
                    placeholder_prev(cap)
                };
                filtered.push(json!({"text": ph}));
            }
            *parts = filtered;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nullrouter_providers::Capabilities;
    use serde_json::json;

    #[test]
    fn strips_image_from_non_vision_model() {
        let mut body = json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "What is this?"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}
                ]}
            ]
        });
        let caps = Capabilities {
            vision: false,
            ..Capabilities::DEFAULT
        };
        strip_unsupported_modalities(&mut body, Format::OpenAi, &caps);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "text");
        assert!(
            content[1]["text"]
                .as_str()
                .unwrap()
                .contains("image omitted")
        );
    }

    #[test]
    fn keeps_image_for_vision_model() {
        let mut body = json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "What is this?"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}
                ]}
            ]
        });
        let caps = Capabilities {
            vision: true,
            ..Capabilities::DEFAULT
        };
        strip_unsupported_modalities(&mut body, Format::OpenAi, &caps);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image_url");
    }

    #[test]
    fn fast_exit_when_model_supports_everything() {
        let mut body = json!({"messages": [{"role": "user", "content": "hello"}]});
        let original = body.clone();
        let caps = Capabilities {
            vision: true,
            audio_input: true,
            pdf: true,
            ..Capabilities::DEFAULT
        };
        strip_unsupported_modalities(&mut body, Format::OpenAi, &caps);
        assert_eq!(
            body, original,
            "should not modify body when model supports all modalities"
        );
    }
}
