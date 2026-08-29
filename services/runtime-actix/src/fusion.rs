//! Fusion combos: ask every model, then have a judge write one answer.
//!
//! Ports the fusion half of `open-sse/services/combo.js`. A fusion combo fans the
//! prompt out to every panel model in parallel, collects their prose, and hands all
//! of it to a judge model that writes the final reply.
//!
//! This file is the shape of that request: which body each panel gets, how their
//! answers are read back out of four different response formats, and what the judge
//! is told. The execution and timing live in [`crate::pipeline`].
//!
//! Two decisions here are load-bearing rather than cosmetic:
//!
//! * Panel calls are forced non-streaming with tools stripped. A judge needs
//!   complete prose; a panel that emitted `tool_calls` would hand the judge a
//!   half-finished turn it cannot use, and the client never sees panel output
//!   anyway.
//! * Panel answers are anonymised as "Source N". Naming the models would invite the
//!   judge to weigh brand reputation instead of the substance in front of it.

use nullrouter_translate::schema::extract_text_content;
use serde_json::{Map, Value, json};

/// Prefix used when flattening an assistant turn's tool calls into prose.
const TOOL_CALL_PREFIX: &str = "[Called tools: ";

/// Prefix used when flattening a tool result into prose.
const TOOL_RESULT_PREFIX: &str = "[Tool result: ";

/// One panel model's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelAnswer {
    /// The model that produced it, for logging only — never shown to the judge.
    pub model: String,
    pub text: String,
}

/// Fusion tuning, overridable per combo upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FusionTuning {
    /// Answers needed before stragglers are put on a clock.
    pub min_panel: usize,
    /// How long the remaining models get once quorum is reached.
    pub straggler_grace_ms: u64,
    /// Absolute cap, so one hung model cannot stall the request.
    pub panel_hard_timeout_ms: u64,
}

impl Default for FusionTuning {
    fn default() -> Self {
        // Upstream `FUSION_DEFAULTS`.
        Self {
            min_panel: 2,
            straggler_grace_ms: 8_000,
            panel_hard_timeout_ms: 90_000,
        }
    }
}

impl FusionTuning {
    /// Quorum for a panel of `panel_size`, never above the panel itself.
    ///
    /// A quorum larger than the panel could never be reached, so the grace window
    /// would never start and every request would wait out the hard timeout.
    pub(crate) const fn quorum(self, panel_size: usize) -> usize {
        let floor = if self.min_panel < 2 {
            2
        } else {
            self.min_panel
        };
        if floor > panel_size {
            panel_size
        } else {
            floor
        }
    }
}

/// Flatten tool turns in a message array into plain prose.
///
/// Panel models are asked for prose, so the history they see must not contain
/// structured tool turns: a model handed a `tool` role with its tools stripped can
/// loop trying to answer it. The content is preserved as text rather than dropped,
/// because the tool results are often the substance of the conversation.
pub(crate) fn flatten_tool_history(messages: &[Value]) -> Vec<Value> {
    messages.iter().map(flatten_message).collect()
}

fn flatten_message(message: &Value) -> Value {
    let Some(object) = message.as_object() else {
        return message.clone();
    };
    let role = object
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();

    // A tool result becomes an assistant statement of what came back.
    if role == "tool" || role == "function" {
        let text = content_as_text(object.get("content"));
        return json!({
            "role": "assistant",
            "content": format!("{TOOL_RESULT_PREFIX}{text}]"),
        });
    }

    // An assistant turn's structured calls become a named list.
    if role == "assistant"
        && let Some(calls) = object.get("tool_calls").and_then(Value::as_array)
    {
        let names: Vec<String> = calls.iter().map(tool_call_name).collect();
        let base = content_as_text(object.get("content"));
        let mut reduced = object.clone();
        reduced.remove("tool_calls");
        reduced.insert(
            "content".to_owned(),
            Value::from(join_with_newline(
                &base,
                &format!("{TOOL_CALL_PREFIX}{}]", names.join(", ")),
            )),
        );
        return Value::Object(reduced);
    }

    // Claude-shaped blocks carry tool use and results inside `content`.
    if let Some(blocks) = object.get("content").and_then(Value::as_array)
        && blocks.iter().any(is_tool_block)
    {
        return flatten_blocks(object, blocks);
    }

    message.clone()
}

/// Whether a content block is a tool use or a tool result.
fn is_tool_block(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("tool_use" | "tool_result")
    )
}

/// Rebuild a Claude-shaped message with its tool blocks reduced to prose.
fn flatten_blocks(object: &Map<String, Value>, blocks: &[Value]) -> Value {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_names: Vec<String> = Vec::new();
    let mut tool_results: Vec<String> = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    text_parts.push(text.to_owned());
                }
            }
            Some("tool_use") => tool_names.push(
                block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_owned(),
            ),
            Some("tool_result") => tool_results.push(content_as_text(block.get("content"))),
            _ => {}
        }
    }

    let mut content = text_parts.join("\n");
    if !tool_names.is_empty() {
        content = join_with_newline(
            &content,
            &format!("{TOOL_CALL_PREFIX}{}]", tool_names.join(", ")),
        );
    }
    if !tool_results.is_empty() {
        content = join_with_newline(
            &content,
            &format!("{TOOL_RESULT_PREFIX}{}]", tool_results.join("\n")),
        );
    }
    let mut reduced = object.clone();
    reduced.insert("content".to_owned(), Value::from(content));
    Value::Object(reduced)
}

/// A tool call's function name, or a generic label.
fn tool_call_name(call: &Value) -> String {
    call.get("function")
        .and_then(|function| function.get("name"))
        .or_else(|| call.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_owned()
}

/// Content as text, whether it is a string or a block array.
fn content_as_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(value) => {
            let extracted = extract_text_content(Some(value), "\n");
            if extracted.is_empty() && !value.is_null() {
                // A shape the extractor does not know is still better rendered than
                // silently dropped: the judge loses the turn otherwise.
                value.to_string()
            } else {
                extracted
            }
        }
        None => String::new(),
    }
}

fn join_with_newline(base: &str, addition: &str) -> String {
    if base.is_empty() {
        return addition.to_owned();
    }
    format!("{base}\n{addition}")
}

/// The body each panel model is called with.
///
/// Non-streaming with `tools`, `tool_choice` and `stream_options` removed. Dropping
/// `stream_options` matters on its own: some providers reject it outright when
/// `stream` is false.
pub(crate) fn panel_body(body: &Value) -> Value {
    let mut panel = body.clone();
    let Some(object) = panel.as_object_mut() else {
        return panel;
    };
    object.remove("tools");
    object.remove("tool_choice");
    object.remove("stream_options");
    object.insert("stream".to_owned(), Value::Bool(false));

    // Flatten whichever message array this format uses.
    for key in ["messages", "input"] {
        if let Some(messages) = object.get(key).and_then(Value::as_array) {
            let flattened = flatten_tool_history(messages);
            object.insert(key.to_owned(), Value::Array(flattened));
        }
    }
    panel
}

/// Read the assistant text out of a completed response, in any of the four formats.
///
/// Panel replies have already been translated back to the client's format, so all
/// four shapes are possible here.
pub(crate) fn extract_panel_text(body: &Value) -> String {
    let Some(object) = body.as_object() else {
        return String::new();
    };

    // OpenAI chat completion.
    if let Some(choice) = object
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    {
        let message = choice.get("message").or_else(|| choice.get("delta"));
        let text = content_as_text(message.and_then(|message| message.get("content")));
        if !text.trim().is_empty() {
            return text;
        }
        if let Some(text) = choice.get("text").and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return text.to_owned();
        }
    }

    // Claude messages: text blocks share OpenAI's `{type:"text"}` shape.
    let claude = extract_text_content(object.get("content"), "\n");
    if !claude.trim().is_empty() {
        return claude;
    }

    // Gemini: parts carry `.text` with no type discriminator.
    if let Some(parts) = object
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    {
        let text: String = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect();
        if !text.trim().is_empty() {
            return text;
        }
    }

    // OpenAI Responses API.
    if let Some(output) = object.get("output").and_then(Value::as_array) {
        let text: String = output
            .iter()
            .filter_map(|item| item.get("content").and_then(Value::as_array))
            .flatten()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect();
        if !text.trim().is_empty() {
            return text;
        }
    }

    String::new()
}

/// The directive the judge is given.
///
/// Per the design upstream follows, the judge does not merge: it analyses the panel
/// for consensus, contradictions, partial coverage, unique insight and blind spots,
/// then writes one grounded answer. Most of fusion's quality comes from that step
/// rather than from having several answers.
///
/// Sources are numbered rather than named so the judge weighs substance, not the
/// reputation of a model brand.
pub(crate) fn judge_prompt(answers: &[PanelAnswer]) -> String {
    let panel = answers
        .iter()
        .enumerate()
        .map(|(index, answer)| format!("[Source {}]\n{}", index + 1, answer.text))
        .collect::<Vec<_>>()
        .join("\n\n");

    [
        format!(
            "You are the JUDGE in a model-fusion panel. {} expert models independently answered the user's most recent request. Their responses are below, anonymized by source.",
            answers.len()
        ),
        String::new(),
        String::from(
            "Do NOT mention that multiple models were used, and do NOT refer to the sources. Produce ONE authoritative final answer addressed directly to the user.",
        ),
        String::new(),
        String::from(
            "First, internally analyze the panel along these dimensions: consensus (points most sources agree on — treat as higher-confidence), contradictions (where they disagree — resolve with your own judgment), partial coverage, unique insights only one source surfaced, and blind spots every source missed. Then write the best possible final answer grounded in that analysis — more complete and correct than any single response, with no filler.",
        ),
        String::new(),
        String::from("=== PANEL RESPONSES ==="),
        panel,
        String::from("=== END PANEL RESPONSES ==="),
        String::new(),
        String::from("Now write the final answer to the user's original request."),
    ]
    .join("\n")
}

/// The judge's request body: the original conversation plus the directive.
///
/// The client's `stream` flag and `tools` are kept, so streaming and downstream
/// tool use still work — only the panel calls were degraded to prose.
pub(crate) fn judge_body(body: &Value, answers: &[PanelAnswer]) -> Value {
    append_user_turn(body, &judge_prompt(answers))
}

/// Append a user turn to whichever message array the request format uses.
///
/// The original conversation and system prompt are preserved so the judge answers
/// the user's actual question rather than the directive in isolation.
fn append_user_turn(body: &Value, text: &str) -> Value {
    let mut next = body.clone();
    let Some(object) = next.as_object_mut() else {
        return json!({ "messages": [{ "role": "user", "content": text }] });
    };

    if let Some(messages) = object.get("messages").and_then(Value::as_array) {
        let mut extended = messages.clone();
        extended.push(json!({ "role": "user", "content": text }));
        object.insert("messages".to_owned(), Value::Array(extended));
        return next;
    }
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        let mut extended = input.clone();
        extended.push(json!({ "role": "user", "content": text }));
        object.insert("input".to_owned(), Value::Array(extended));
        return next;
    }
    if let Some(contents) = object.get("contents").and_then(Value::as_array) {
        let mut extended = contents.clone();
        extended.push(json!({ "role": "user", "parts": [{ "text": text }] }));
        object.insert("contents".to_owned(), Value::Array(extended));
        return next;
    }
    // No recognised array: start one, as upstream does.
    object.insert(
        "messages".to_owned(),
        json!([{ "role": "user", "content": text }]),
    );
    next
}
