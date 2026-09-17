#![allow(clippy::doc_markdown)]
//! Config-driven param stripping.
//!
//! Remove fields a provider rejects upstream,
//! flatten content arrays, and clamp max_tokens.
//!
//! Ports `open-sse/translator/concerns/paramSupport.js`. Adding a rule here
//! instead of scattering `delete body.x` across executors keeps the strip
//! logic centralized and testable.

use serde_json::{Map, Value};

/// One stripping rule. A rule matches when the provider (if specified) equals
/// the connection's provider AND the model matches the rule's pattern.
struct StripRule {
    /// `None` = all providers.
    provider: Option<&'static str>,
    /// Regex or predicate-equivalent on the model id. `None` = all models.
    matches: Option<fn(&str) -> bool>,
    /// Params to remove from the body root.
    drop: &'static [&'static str],
    /// Flatten `messages[].content` from array to plain string.
    flatten_content: bool,
    /// Clamp `max_tokens/max_completion_tokens/max_output_tokens` to the
    /// model's capability ceiling.
    clamp_to_model_max: bool,
    /// Hard cap on max output tokens, overriding the model ceiling when lower.
    max_output_cap: Option<u64>,
}

/// All Claude models reject `temperature` upstream (Anthropic 400). #1748
fn is_claude(model: &str) -> bool {
    model.to_ascii_lowercase().contains("claude")
}

/// GitHub Copilot Claude models (except opus/sonnet 4.6) reject `thinking`
/// and `reasoning_effort`. #713
fn is_github_claude_except_46(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    let is_claude = lower.contains("claude");
    let is_46 = lower.contains("4.6") || lower.contains("4-6");
    let is_opus_or_sonnet_46 = (lower.contains("opus") || lower.contains("sonnet")) && is_46;
    is_claude && !is_opus_or_sonnet_46
}

/// GitHub Copilot gpt-5.4 rejects `temperature`.
fn is_gpt54(model: &str) -> bool {
    model.to_ascii_lowercase().contains("gpt-5.4") || model.to_ascii_lowercase().contains("gpt-5-4")
}

/// Volcengine Ark GLM-5 needs `max_tokens` clamped.
fn is_glm5(model: &str) -> bool {
    model.to_ascii_lowercase().contains("glm-5") || model.to_ascii_lowercase().contains("glm5")
}

/// Volcengine Ark Kimi family: `max_tokens` capped at 32768.
fn is_kimi(model: &str) -> bool {
    model.to_ascii_lowercase().contains("kimi")
}

/// Xiaomi `MiMo` preview models: flatten content array.
fn is_mimo_preview(model: &str) -> bool {
    model.to_ascii_lowercase().contains("preview")
}

const RULES: &[StripRule] = &[
    StripRule {
        provider: None,
        matches: Some(is_claude),
        drop: &["temperature"],
        flatten_content: false,
        clamp_to_model_max: false,
        max_output_cap: None,
    },
    StripRule {
        provider: Some("github"),
        matches: Some(is_gpt54),
        drop: &["temperature"],
        flatten_content: false,
        clamp_to_model_max: false,
        max_output_cap: None,
    },
    StripRule {
        provider: Some("github"),
        matches: Some(is_github_claude_except_46),
        drop: &["thinking", "reasoning_effort"],
        flatten_content: false,
        clamp_to_model_max: false,
        max_output_cap: None,
    },
    StripRule {
        provider: Some("cloudflare-ai"),
        matches: None,
        drop: &[],
        flatten_content: true,
        clamp_to_model_max: false,
        max_output_cap: None,
    },
    StripRule {
        provider: Some("xiaomi-mimo"),
        matches: Some(is_mimo_preview),
        drop: &[],
        flatten_content: true,
        clamp_to_model_max: false,
        max_output_cap: None,
    },
    StripRule {
        provider: Some("volcengine-ark"),
        matches: Some(is_glm5),
        drop: &[],
        flatten_content: false,
        clamp_to_model_max: true,
        max_output_cap: None,
    },
    StripRule {
        provider: Some("volcengine-ark"),
        matches: Some(is_kimi),
        drop: &[],
        flatten_content: false,
        clamp_to_model_max: true,
        max_output_cap: Some(32768),
    },
];

/// Remove unsupported params from the body in place.
///
/// Call this after `translate_request` completes, before dispatching to the
/// upstream provider. `provider` is the connection's provider id (e.g.
/// `"anthropic"`, `"github"`), `model` is the upstream model id, and
/// `model_max_output` is the model's capability ceiling (0 = unknown).
#[inline]
pub fn strip_unsupported_params(
    provider: &str,
    model: &str,
    body: &mut Value,
    model_max_output: u64,
) {
    let Some(object) = body.as_object_mut() else {
        return;
    };

    for rule in RULES {
        if let Some(rule_provider) = rule.provider
            && rule_provider != provider
        {
            continue;
        }
        if let Some(matches) = rule.matches
            && !matches(model)
        {
            continue;
        }

        for key in rule.drop {
            object.remove(*key);
        }

        if rule.flatten_content {
            flatten_content_arrays(object);
        }

        if rule.clamp_to_model_max || rule.max_output_cap.is_some() {
            let mut candidates: Vec<u64> = Vec::new();
            if rule.clamp_to_model_max && model_max_output > 0 {
                candidates.push(model_max_output);
            }
            if let Some(cap) = rule.max_output_cap {
                candidates.push(cap);
            }
            if let Some(&ceiling) = candidates.iter().min() {
                clamp_number(object, "max_tokens", ceiling);
                clamp_number(object, "max_completion_tokens", ceiling);
                clamp_number(object, "max_output_tokens", ceiling);
            }
        }
    }
}

/// Flatten `messages[].content` from an array of content parts to a plain
/// string. Some providers (Cloudflare Workers AI, Xiaomi `MiMo` Preview) only
/// accept a string, not the OpenAI content-part array.
fn flatten_content_arrays(object: &mut Map<String, Value>) {
    let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for msg in messages.iter_mut() {
        let Some(msg_obj) = msg.as_object_mut() else {
            continue;
        };
        if let Some(content) = msg_obj.get_mut("content")
            && content.is_array()
        {
            let flattened = content
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| {
                            part.get("type")
                                .and_then(Value::as_str)
                                .filter(|t| *t == "text")
                                .and_then(|_| part.get("text").and_then(Value::as_str))
                        })
                        .collect::<Vec<&str>>()
                        .join("")
                })
                .unwrap_or_default();
            *content = Value::String(flattened);
        }
    }
}

fn clamp_number(object: &mut Map<String, Value>, key: &str, ceiling: u64) {
    if let Some(Value::Number(n)) = object.get(key)
        && let Some(val) = n.as_u64()
        && val > ceiling
    {
        object.insert(key.to_owned(), Value::from(ceiling));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_temperature_for_claude() {
        let mut body = json!({"model": "claude-sonnet-4-6", "temperature": 0.7, "messages": []});
        strip_unsupported_params("anthropic", "claude-sonnet-4-6", &mut body, 0);
        assert!(
            body.get("temperature").is_none(),
            "temperature should be stripped"
        );
    }

    #[test]
    fn does_not_strip_temperature_for_non_claude() {
        let mut body = json!({"model": "gpt-4o", "temperature": 0.7, "messages": []});
        strip_unsupported_params("openai", "gpt-4o", &mut body, 0);
        assert_eq!(body["temperature"], json!(0.7));
    }

    #[test]
    fn strips_thinking_for_github_claude_except_46() {
        let mut body = json!({"model": "claude-sonnet-4-5", "thinking": {"type": "enabled"}, "reasoning_effort": "high"});
        strip_unsupported_params("github", "claude-sonnet-4-5", &mut body, 0);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn does_not_strip_thinking_for_github_claude_46() {
        let mut body = json!({"model": "claude-opus-4-6", "thinking": {"type": "enabled"}});
        strip_unsupported_params("github", "claude-opus-4-6", &mut body, 0);
        assert!(body.get("thinking").is_some(), "4.6 should keep thinking");
    }

    #[test]
    fn flattens_content_for_cloudflare() {
        let mut body = json!({
            "model": "llama-3",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hello"}, {"type": "text", "text": " world"}]}
            ]
        });
        strip_unsupported_params("cloudflare-ai", "llama-3", &mut body, 0);
        assert_eq!(body["messages"][0]["content"], json!("hello world"));
    }

    #[test]
    fn clamps_max_tokens_for_volcengine_kimi() {
        let mut body = json!({"model": "kimi-k2", "max_tokens": 262144});
        strip_unsupported_params("volcengine-ark", "kimi-k2", &mut body, 262144);
        assert_eq!(body["max_tokens"], json!(32768), "should clamp to 32768");
    }

    #[test]
    fn clamps_max_tokens_for_volcengine_glm5() {
        let mut body = json!({"model": "glm-5.2", "max_tokens": 200000});
        strip_unsupported_params("volcengine-ark", "glm-5.2", &mut body, 128000);
        assert_eq!(
            body["max_tokens"],
            json!(128000),
            "should clamp to model ceiling"
        );
    }

    #[test]
    fn does_not_clamp_when_below_ceiling() {
        let mut body = json!({"model": "kimi-k2", "max_tokens": 8000});
        strip_unsupported_params("volcengine-ark", "kimi-k2", &mut body, 262144);
        assert_eq!(
            body["max_tokens"],
            json!(8000),
            "should not clamp when below"
        );
    }

    #[test]
    fn no_rules_for_unknown_provider() {
        let mut body = json!({"model": "test", "temperature": 0.5, "thinking": {}});
        strip_unsupported_params("unknown-provider", "test-model", &mut body, 0);
        assert_eq!(body["temperature"], json!(0.5));
        assert!(body.get("thinking").is_some());
    }
}
