//! Cross-format concerns: usage arithmetic, finish-reason mapping, reasoning
//! extraction, and OpenAI chunk construction.
//!
//! Ports `open-sse/translator/concerns/{usage,finishReason,reasoning,chunk}.js`.

use serde_json::{Map, Value, json};

use crate::schema::{claude_stop, openai_finish, role};

/// Token counts in OpenAI shape, before serialization.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
}

impl Usage {
    /// Serialize to the OpenAI `usage` object (upstream `buildUsage`).
    ///
    /// Detail sub-objects appear only when their counts are non-zero.
    pub fn to_value(self) -> Value {
        let mut usage = Map::new();
        usage.insert("prompt_tokens".to_owned(), json!(self.prompt_tokens));
        usage.insert(
            "completion_tokens".to_owned(),
            json!(self.completion_tokens),
        );
        usage.insert("total_tokens".to_owned(), json!(self.total_tokens));

        if self.cached_tokens > 0 || self.cache_creation_tokens > 0 {
            let mut details = Map::new();
            if self.cached_tokens > 0 {
                details.insert("cached_tokens".to_owned(), json!(self.cached_tokens));
            }
            if self.cache_creation_tokens > 0 {
                details.insert(
                    "cache_creation_tokens".to_owned(),
                    json!(self.cache_creation_tokens),
                );
            }
            usage.insert("prompt_tokens_details".to_owned(), Value::Object(details));
        }

        if self.reasoning_tokens > 0 {
            usage.insert(
                "completion_tokens_details".to_owned(),
                json!({ "reasoning_tokens": self.reasoning_tokens }),
            );
        }

        Value::Object(usage)
    }
}

/// Read a numeric field, treating anything non-numeric as 0 (upstream `n()`).
fn num(raw: &Value, key: &str) -> u64 {
    raw.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Provider-native usage payload kinds with distinct token field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageKind {
    Claude,
    Gemini,
    Kiro,
    Ollama,
    CommandCode,
}

/// Convert a provider-native usage object to OpenAI shape
/// (upstream `toOpenAIUsage`). Each provider keeps its exact arithmetic.
pub fn to_openai_usage(raw: &Value, kind: UsageKind) -> Option<Usage> {
    if !raw.is_object() {
        return None;
    }
    Some(match kind {
        UsageKind::Claude => {
            let input = num(raw, "input_tokens");
            let output = num(raw, "output_tokens");
            let cache_read = num(raw, "cache_read_input_tokens");
            let cache_create = num(raw, "cache_creation_input_tokens");
            // Claude folds all prompt-side tokens into prompt_tokens.
            let prompt = input + cache_read + cache_create;
            Usage {
                prompt_tokens: prompt,
                completion_tokens: output,
                total_tokens: prompt + output,
                cached_tokens: cache_read,
                cache_creation_tokens: cache_create,
                reasoning_tokens: 0,
            }
        }
        UsageKind::Gemini => {
            let cached = num(raw, "cachedContentTokenCount");
            let prompt = num(raw, "promptTokenCount");
            let thoughts = num(raw, "thoughtsTokenCount");
            let total = num(raw, "totalTokenCount");
            let mut candidates = num(raw, "candidatesTokenCount");
            // Derive candidates when upstream omits it.
            if candidates == 0 && total > 0 {
                candidates = total.saturating_sub(prompt).saturating_sub(thoughts);
            }
            Usage {
                prompt_tokens: prompt,
                completion_tokens: candidates + thoughts,
                total_tokens: total,
                cached_tokens: cached,
                cache_creation_tokens: 0,
                reasoning_tokens: thoughts,
            }
        }
        UsageKind::Kiro => {
            let input = num(raw, "inputTokens");
            let output = num(raw, "outputTokens");
            let cached = [
                num(raw, "cache_read_input_tokens"),
                num(raw, "cachedTokens"),
                num(raw, "cached_tokens"),
            ]
            .into_iter()
            .find(|value| *value > 0)
            .unwrap_or(0);
            Usage {
                prompt_tokens: input,
                completion_tokens: output,
                total_tokens: input + output,
                cached_tokens: cached,
                cache_creation_tokens: num(raw, "cache_creation_input_tokens"),
                reasoning_tokens: 0,
            }
        }
        UsageKind::Ollama => {
            let input = num(raw, "prompt_eval_count");
            let output = num(raw, "eval_count");
            Usage {
                prompt_tokens: input,
                completion_tokens: output,
                total_tokens: input + output,
                ..Usage::default()
            }
        }
        UsageKind::CommandCode => {
            let input = num(raw, "inputTokens");
            let output = num(raw, "outputTokens");
            let total = raw
                .get("totalTokens")
                .and_then(Value::as_u64)
                .unwrap_or(input + output);
            Usage {
                prompt_tokens: input,
                completion_tokens: output,
                total_tokens: total,
                ..Usage::default()
            }
        }
    })
}

/// Map an upstream stop reason onto an OpenAI `finish_reason`
/// (upstream `toOpenAIFinish`).
pub fn to_openai_finish(reason: &str, format: &str) -> String {
    match format {
        "claude" => match reason {
            claude_stop::MAX_TOKENS => openai_finish::LENGTH,
            claude_stop::TOOL_USE => openai_finish::TOOL_CALLS,
            // end_turn, stop_sequence, and anything unknown map to stop.
            _ => openai_finish::STOP,
        }
        .to_owned(),
        "commandcode" => match reason {
            "length" => openai_finish::LENGTH.to_owned(),
            "tool-calls" | "tool_use" => openai_finish::TOOL_CALLS.to_owned(),
            "content-filter" => openai_finish::CONTENT_FILTER.to_owned(),
            // `error` and an absent reason both collapse to `stop` upstream.
            "stop" | "error" | "" => openai_finish::STOP.to_owned(),
            // Anything else passes through verbatim.
            other => other.to_owned(),
        },
        "gemini" => match reason.to_uppercase().as_str() {
            "MAX_TOKENS" => openai_finish::LENGTH,
            "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" => {
                openai_finish::CONTENT_FILTER
            }
            _ => openai_finish::STOP,
        }
        .to_owned(),
        "kiro" | "ollama" => match reason {
            "tool_calls" | "tool_use" => openai_finish::TOOL_CALLS,
            "length" | "max_tokens" => openai_finish::LENGTH,
            _ => openai_finish::STOP,
        }
        .to_owned(),
        _ if reason.is_empty() => openai_finish::STOP.to_owned(),
        _ => reason.to_owned(),
    }
}

/// Map an OpenAI `finish_reason` back to an upstream stop reason
/// (upstream `fromOpenAIFinish`).
pub fn from_openai_finish(reason: &str, format: &str) -> String {
    if format == "claude" {
        return match reason {
            openai_finish::LENGTH => claude_stop::MAX_TOKENS,
            openai_finish::TOOL_CALLS => claude_stop::TOOL_USE,
            _ => claude_stop::END_TURN,
        }
        .to_owned();
    }
    reason.to_owned()
}

/// Build an OpenAI delta carrying `reasoning_content`
/// (upstream `reasoningDelta`).
pub fn reasoning_delta(text: &str, with_role: bool) -> Value {
    if with_role {
        json!({ "role": role::ASSISTANT, "reasoning_content": text })
    } else {
        json!({ "reasoning_content": text })
    }
}

/// Extract reasoning text from a streamed delta across vendor shapes
/// (upstream `extractReasoningText`): `reasoning_content`, `reasoning`, or
/// `reasoning_details[]`.
pub fn extract_reasoning_text(delta: Option<&Value>) -> String {
    let Some(delta) = delta.filter(|value| value.is_object()) else {
        return String::new();
    };

    if let Some(text) = delta.get("reasoning_content").and_then(Value::as_str)
        && !text.is_empty()
    {
        return text.to_owned();
    }
    if let Some(text) = delta.get("reasoning").and_then(Value::as_str)
        && !text.is_empty()
    {
        return text.to_owned();
    }
    if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
        return details
            .iter()
            .map(|detail| {
                detail.as_str().map_or_else(
                    || {
                        detail
                            .get("text")
                            .or_else(|| detail.get("content"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    },
                    |text| text,
                )
            })
            .collect();
    }
    String::new()
}

/// Identity of the chunk stream being built.
#[derive(Debug, Clone)]
pub struct ChunkMeta {
    pub id: String,
    pub created: u64,
    pub model: String,
}

/// Build an OpenAI `chat.completion.chunk` (upstream `buildChunk`).
///
/// Key order matches upstream: id, object, created, model, choices.
#[allow(
    clippy::needless_pass_by_value,
    reason = "delta is moved into the returned chunk"
)]
pub fn build_chunk(meta: &ChunkMeta, delta: Value, finish_reason: Option<&str>) -> Value {
    json!({
        "id": meta.id,
        "object": "chat.completion.chunk",
        "created": meta.created,
        "model": meta.model,
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": finish_reason,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ChunkMeta, Usage, UsageKind, build_chunk, extract_reasoning_text, from_openai_finish,
        to_openai_finish, to_openai_usage,
    };
    use serde_json::json;

    #[test]
    fn claude_usage_folds_cache_into_prompt_tokens() {
        let raw = json!({
            "input_tokens": 100,
            "output_tokens": 20,
            "cache_read_input_tokens": 30,
            "cache_creation_input_tokens": 5,
        });
        let usage = to_openai_usage(&raw, UsageKind::Claude).expect("claude usage parses");
        assert_eq!(usage.prompt_tokens, 135);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.total_tokens, 155);

        let value = usage.to_value();
        assert_eq!(
            value.pointer("/prompt_tokens_details/cached_tokens"),
            Some(&json!(30))
        );
        assert_eq!(
            value.pointer("/prompt_tokens_details/cache_creation_tokens"),
            Some(&json!(5))
        );
    }

    #[test]
    fn zero_details_are_omitted_from_usage() {
        let usage = Usage {
            prompt_tokens: 10,
            completion_tokens: 2,
            total_tokens: 12,
            ..Usage::default()
        };
        let value = usage.to_value();
        assert!(value.get("prompt_tokens_details").is_none());
        assert!(value.get("completion_tokens_details").is_none());
    }

    #[test]
    fn gemini_usage_derives_candidates_when_missing() {
        let raw = json!({
            "promptTokenCount": 100,
            "thoughtsTokenCount": 10,
            "totalTokenCount": 150,
        });
        let usage = to_openai_usage(&raw, UsageKind::Gemini).expect("gemini usage parses");
        // candidates = 150 - 100 - 10 = 40; completion folds in thoughts.
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.reasoning_tokens, 10);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn gemini_usage_never_underflows() {
        // Inconsistent upstream numbers must not wrap around.
        let raw = json!({
            "promptTokenCount": 200,
            "thoughtsTokenCount": 50,
            "totalTokenCount": 100,
        });
        let usage = to_openai_usage(&raw, UsageKind::Gemini).expect("parses");
        assert_eq!(usage.completion_tokens, 50);
    }

    #[test]
    fn ollama_and_commandcode_usage_shapes() {
        let ollama = to_openai_usage(
            &json!({ "prompt_eval_count": 7, "eval_count": 3 }),
            UsageKind::Ollama,
        )
        .expect("parses");
        assert_eq!(ollama.total_tokens, 10);

        let commandcode = to_openai_usage(
            &json!({ "inputTokens": 4, "outputTokens": 6, "totalTokens": 99 }),
            UsageKind::CommandCode,
        )
        .expect("parses");
        assert_eq!(commandcode.total_tokens, 99);
    }

    #[test]
    fn non_object_usage_is_rejected() {
        assert!(to_openai_usage(&json!(null), UsageKind::Claude).is_none());
        assert!(to_openai_usage(&json!(5), UsageKind::Claude).is_none());
    }

    #[test]
    fn finish_reasons_map_both_directions() {
        assert_eq!(to_openai_finish("max_tokens", "claude"), "length");
        assert_eq!(to_openai_finish("tool_use", "claude"), "tool_calls");
        assert_eq!(to_openai_finish("end_turn", "claude"), "stop");
        assert_eq!(to_openai_finish("stop_sequence", "claude"), "stop");

        assert_eq!(to_openai_finish("MAX_TOKENS", "gemini"), "length");
        assert_eq!(to_openai_finish("SAFETY", "gemini"), "content_filter");
        // Gemini reasons are compared case-insensitively.
        assert_eq!(to_openai_finish("max_tokens", "gemini"), "length");

        assert_eq!(from_openai_finish("length", "claude"), "max_tokens");
        assert_eq!(from_openai_finish("tool_calls", "claude"), "tool_use");
        assert_eq!(from_openai_finish("stop", "claude"), "end_turn");
        // Unknown OpenAI reasons collapse to end_turn for Claude.
        assert_eq!(from_openai_finish("weird", "claude"), "end_turn");
        // Non-Claude formats pass through.
        assert_eq!(from_openai_finish("stop", "openai"), "stop");
    }

    #[test]
    fn reasoning_text_extracted_across_vendor_shapes() {
        assert_eq!(
            extract_reasoning_text(Some(&json!({ "reasoning_content": "abc" }))),
            "abc"
        );
        assert_eq!(
            extract_reasoning_text(Some(&json!({ "reasoning": "xyz" }))),
            "xyz"
        );
        assert_eq!(
            extract_reasoning_text(Some(&json!({
                "reasoning_details": [{ "text": "a" }, { "content": "b" }, "c"],
            }))),
            "abc"
        );
        assert_eq!(
            extract_reasoning_text(Some(&json!({ "content": "no" }))),
            ""
        );
        assert_eq!(extract_reasoning_text(None), "");
    }

    #[test]
    fn chunks_carry_openai_envelope() {
        let meta = ChunkMeta {
            id: "chatcmpl-1".to_owned(),
            created: 42,
            model: "gpt-5".to_owned(),
        };
        let chunk = build_chunk(&meta, json!({ "content": "hi" }), None);
        assert_eq!(chunk.get("object"), Some(&json!("chat.completion.chunk")));
        assert_eq!(
            chunk.pointer("/choices/0/delta/content"),
            Some(&json!("hi"))
        );
        assert_eq!(
            chunk.pointer("/choices/0/finish_reason"),
            Some(&json!(null))
        );

        let final_chunk = build_chunk(&meta, json!({}), Some("stop"));
        assert_eq!(
            final_chunk.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
    }
}
