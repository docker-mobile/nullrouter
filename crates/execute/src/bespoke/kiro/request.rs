//! Building `kiro`'s `CodeWhisperer` request document.
//!
//! Ports `open-sse/translator/request/openai-to-kiro.js`. The wire shape is a `conversationState`
//! carrying a history of alternating turns plus the current one, which is *not* part of the history — it is
//! lifted out of it. Three of its rules exist because `CodeWhisperer` answers
//! `400 REQUEST_BODY_INVALID` rather than degrading:
//!
//! * **Turns must alternate.** Two user turns in a row is a rejection, so consecutive ones are merged —
//!   and their contexts merged with them, or a tool result from the second is silently dropped.
//! * **The current message is the last *user* turn**, removed from the history rather than duplicated.
//! * **A tool result belongs to a user turn.** OpenAI puts it on a `tool` role, which has no equivalent
//!   here, so it becomes a `userInputMessageContext.toolResults` entry.
//!
//! The `profileArn` rule is not about shape but about accounts: an account-bound credential must never be
//! sent the shared default ARN, because it belongs to a different account and draws
//! `403 bearer token invalid`.

use serde_json::{Map, Value, json};

/// Kiro's own output ceiling, which upstream sends unconditionally.
const MAX_TOKENS: u64 = 32_000;

/// Auth methods whose credential is bound to a specific account.
///
/// These must never receive the shared default profile ARN. An empty string is sent instead, which tells
/// `CodeWhisperer` to use the token's own default profile.
const ACCOUNT_BOUND: [&str; 3] = ["api_key", "idc", "external_idp"];

/// A turn in the conversation, before it is rendered.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Turn {
    /// A user turn: text, any images, and any tool results it carries.
    User {
        content: String,
        images: Vec<Value>,
        tool_results: Vec<Value>,
    },
    /// An assistant turn: text plus any tool calls it made.
    Assistant {
        content: String,
        tool_uses: Vec<Value>,
    },
}

/// A conversation split the way `conversationState` wants it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Conversation {
    /// The turns before the current one, alternating.
    pub(crate) history: Vec<Turn>,
    /// The last user turn, lifted out of the history.
    pub(crate) current: Turn,
}

/// Split an OpenAI message list into Kiro's history and current turn.
pub(crate) fn convert_messages(messages: &[Value]) -> Conversation {
    let mut turns: Vec<Turn> = Vec::new();

    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        match role {
            "assistant" => {
                let (content, mut tool_uses) = assistant_parts(message);
                if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                    // `tool_calls` is the OpenAI spelling and wins over content blocks, as upstream has it.
                    tool_uses = calls.iter().filter_map(tool_use_from_call).collect();
                }
                // Kept even when empty. An assistant turn with no text renders as `...`, and dropping it
                // instead would let the user turns on either side merge — which changes the conversation
                // rather than tidying it.
                turns.push(Turn::Assistant { content, tool_uses });
            }
            // A tool result has no turn of its own here: it rides on a user turn.
            "tool" => turns.push(Turn::User {
                content: String::new(),
                images: Vec::new(),
                tool_results: vec![tool_result_from_message(message)],
            }),
            // A system turn becomes a user turn wrapped in `<instructions>`. Claude models reached through
            // Kiro treat that tag as authoritative, which is the nearest thing to a system role the
            // protocol offers.
            "system" => {
                let (content, images, tool_results) = user_parts(message);
                let wrapped = if content.is_empty() {
                    content
                } else {
                    format!("<instructions>\n{content}\n</instructions>")
                };
                turns.push(Turn::User {
                    content: wrapped,
                    images,
                    tool_results,
                });
            }
            _user => {
                let (content, images, tool_results) = user_parts(message);
                turns.push(Turn::User {
                    content,
                    images,
                    tool_results,
                });
            }
        }
    }

    // The current message is the last *user* turn, removed from the history rather than repeated in it.
    let current_index = turns
        .iter()
        .rposition(|turn| matches!(turn, Turn::User { .. }));
    // A conversation of only assistant turns has no current message, so a minimal one is made rather than
    // sending a document with an absent required field.
    let current = current_index.map_or_else(
        || Turn::User {
            content: String::new(),
            images: Vec::new(),
            tool_results: Vec::new(),
        },
        |index| turns.remove(index),
    );

    Conversation {
        history: merge_consecutive_users(turns),
        current,
    }
}

/// Merge consecutive user turns, contexts included.
///
/// `CodeWhisperer` requires alternating turns. Merging only the text would drop a tool result or an image
/// that arrived on the second of two adjacent user turns.
fn merge_consecutive_users(turns: Vec<Turn>) -> Vec<Turn> {
    let mut merged: Vec<Turn> = Vec::new();
    for turn in turns {
        let Turn::User {
            content,
            images,
            tool_results,
        } = turn
        else {
            merged.push(turn);
            continue;
        };
        match merged.last_mut() {
            Some(Turn::User {
                content: previous,
                images: previous_images,
                tool_results: previous_results,
            }) => {
                if !content.is_empty() {
                    if !previous.is_empty() {
                        previous.push_str("\n\n");
                    }
                    previous.push_str(&content);
                }
                previous_images.extend(images);
                previous_results.extend(tool_results);
            }
            _not_a_user_turn => merged.push(Turn::User {
                content,
                images,
                tool_results,
            }),
        }
    }
    merged
}

/// A user message's text, images, and tool results.
fn user_parts(message: &Value) -> (String, Vec<Value>, Vec<Value>) {
    match message.get("content") {
        Some(Value::String(text)) => (text.clone(), Vec::new(), Vec::new()),
        Some(Value::Array(parts)) => {
            let mut text = Vec::new();
            let mut images = Vec::new();
            let mut results = Vec::new();
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(value) = part.get("text").and_then(Value::as_str) {
                            text.push(value.to_owned());
                        }
                    }
                    Some("image_url") => {
                        let url = part
                            .pointer("/image_url/url")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if let Some(image) = image_from_data_uri(url) {
                            images.push(image);
                        } else if url.starts_with("http://") || url.starts_with("https://") {
                            // Kiro accepts base64 only. A remote URL is named in the text rather than
                            // dropped, so the model can at least see that an image was meant.
                            text.push(format!("[Image: {url}]"));
                        }
                    }
                    // Claude's own image block, which reaches here when a Claude client is the source.
                    Some("image") => {
                        if let Some(image) = image_from_claude_block(part) {
                            images.push(image);
                        }
                    }
                    Some("tool_result") => results.push(tool_result_from_block(part)),
                    _other => {
                        if let Some(value) = part.get("text").and_then(Value::as_str) {
                            text.push(value.to_owned());
                        }
                    }
                }
            }
            (text.join("\n"), images, results)
        }
        _absent => (String::new(), Vec::new(), Vec::new()),
    }
}

/// An assistant message's text and any tool-use blocks in its content.
fn assistant_parts(message: &Value) -> (String, Vec<Value>) {
    match message.get("content") {
        Some(Value::String(text)) => (text.trim().to_owned(), Vec::new()),
        Some(Value::Array(parts)) => {
            let text = parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            let uses = parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_use"))
                .map(|part| {
                    json!({
                        "toolUseId": part.get("id").and_then(Value::as_str).unwrap_or_default(),
                        "name": part.get("name").and_then(Value::as_str).unwrap_or_default(),
                        "input": part.get("input").cloned().unwrap_or_else(|| json!({})),
                    })
                })
                .collect();
            (text.trim().to_owned(), uses)
        }
        _absent => (String::new(), Vec::new()),
    }
}

/// A `toolUses` entry from an OpenAI `tool_calls` entry.
fn tool_use_from_call(call: &Value) -> Option<Value> {
    let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
    let function = call.get("function")?;
    let name = function.get("name").and_then(Value::as_str)?;
    // The arguments are a JSON *string* on the wire and an object here. An unparseable one becomes an
    // empty object rather than failing the whole request: the call still happened, and dropping it would
    // leave the matching tool result orphaned, which is itself a rejection.
    let input = function
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    Some(json!({ "toolUseId": id, "name": name, "input": input }))
}

/// A `toolResults` entry from an OpenAI `tool` role message.
fn tool_result_from_message(message: &Value) -> Value {
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let errored = message.get("is_error").and_then(Value::as_bool) == Some(true)
        || message.get("status").and_then(Value::as_str) == Some("error");
    json!({
        "toolUseId": message.get("tool_call_id").and_then(Value::as_str).unwrap_or_default(),
        "status": if errored { "error" } else { "success" },
        "content": [{ "text": text }],
    })
}

/// A `toolResults` entry from a Claude `tool_result` content block.
fn tool_result_from_block(block: &Value) -> Value {
    let text = match block.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _absent => String::new(),
    };
    json!({
        "toolUseId": block.get("tool_use_id").and_then(Value::as_str).unwrap_or_default(),
        "status": if block.get("is_error").and_then(Value::as_bool) == Some(true) {
            "error"
        } else {
            "success"
        },
        "content": [{ "text": text }],
    })
}

/// An `images` entry from a `data:` URI.
fn image_from_data_uri(url: &str) -> Option<Value> {
    let rest = url.strip_prefix("data:")?;
    let (mime, payload) = rest.split_once(',')?;
    let mime = mime.strip_suffix(";base64")?;
    // Kiro names the format by the subtype: `image/png` is `png`.
    let format = mime.rsplit('/').next().unwrap_or(mime);
    Some(json!({ "format": format, "source": { "bytes": payload } }))
}

/// An `images` entry from a Claude image block.
fn image_from_claude_block(part: &Value) -> Option<Value> {
    let source = part.get("source")?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    let data = source.get("data").and_then(Value::as_str)?;
    let media_type = source
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or("image/png");
    let format = media_type.rsplit('/').next().unwrap_or(media_type);
    Some(json!({ "format": format, "source": { "bytes": data } }))
}

/// Render a turn as its `conversationState` entry.
fn render(turn: &Turn, model: &str) -> Value {
    match turn {
        Turn::User {
            content,
            images,
            tool_results,
        } => {
            let mut inner = Map::new();
            // An empty user turn is rejected, so it carries `continue` — the same placeholder upstream
            // uses, and the least directive thing that is not empty.
            inner.insert(
                "content".to_owned(),
                json!(if content.is_empty() {
                    "continue"
                } else {
                    content.as_str()
                }),
            );
            inner.insert("modelId".to_owned(), json!(model));
            if !images.is_empty() {
                inner.insert("images".to_owned(), Value::Array(images.clone()));
            }
            if !tool_results.is_empty() {
                // An empty context object is rejected, so it is only present when it has something in it.
                inner.insert(
                    "userInputMessageContext".to_owned(),
                    json!({ "toolResults": tool_results }),
                );
            }
            json!({ "userInputMessage": Value::Object(inner) })
        }
        Turn::Assistant { content, tool_uses } => {
            let mut inner = Map::new();
            // An assistant turn with no text is rejected too. Upstream sends an ellipsis; the same is done
            // here rather than inventing a sentence the model never said.
            inner.insert(
                "content".to_owned(),
                json!(if content.is_empty() {
                    "..."
                } else {
                    content.as_str()
                }),
            );
            if !tool_uses.is_empty() {
                inner.insert("toolUses".to_owned(), Value::Array(tool_uses.clone()));
            }
            json!({ "assistantResponseMessage": Value::Object(inner) })
        }
    }
}

/// What a request needs from its connection.
#[derive(Debug, Clone)]
pub(crate) struct Context<'a> {
    /// The upstream model id, suffixes already stripped.
    pub(crate) model: &'a str,
    /// The connection's auth method, which decides the profile-ARN rule.
    pub(crate) auth_method: Option<&'a str>,
    /// The profile ARN resolved for this connection, if any.
    pub(crate) profile_arn: Option<&'a str>,
    /// This conversation's id.
    pub(crate) conversation_id: &'a str,
    /// The agent continuation id, which ties a multi-turn agent run together.
    pub(crate) continuation_id: &'a str,
    /// An ISO-8601 timestamp for the time context.
    pub(crate) timestamp: &'a str,
}

/// The shared default profile ARN, used only by OAuth and social connections.
///
/// Sent for those because their tokens accept it, and withheld from every account-bound method because it
/// belongs to a different account and draws `403 bearer token invalid`.
const DEFAULT_PROFILE_ARN: &str =
    "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";

/// Build the `CodeWhisperer` request document.
pub(crate) fn build(body: &Value, context: &Context<'_>) -> Value {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let conversation = convert_messages(messages);

    // The current turn carries a time context. A model with no clock otherwise answers "today" wrongly,
    // and upstream found this the least intrusive place to put it.
    let time_context = format!("[Context: Current time is {}]", context.timestamp);
    let current = match &conversation.current {
        Turn::User {
            content,
            images,
            tool_results,
        } => Turn::User {
            content: if content.is_empty() {
                time_context
            } else {
                format!("{time_context}\n\n{content}")
            },
            images: images.clone(),
            tool_results: tool_results.clone(),
        },
        assistant @ Turn::Assistant { .. } => assistant.clone(),
    };

    let mut state = Map::new();
    state.insert("chatTriggerType".to_owned(), json!("MANUAL"));
    state.insert("conversationId".to_owned(), json!(context.conversation_id));
    state.insert(
        "agentContinuationId".to_owned(),
        json!(context.continuation_id),
    );
    state.insert("agentTaskType".to_owned(), json!("vibe"));
    state.insert("currentMessage".to_owned(), render(&current, context.model));
    state.insert(
        "history".to_owned(),
        Value::Array(
            conversation
                .history
                .iter()
                .map(|turn| render(turn, context.model))
                .collect(),
        ),
    );

    let mut payload = Map::new();
    payload.insert("conversationState".to_owned(), Value::Object(state));
    payload.insert("agentMode".to_owned(), json!("vibe"));

    // An account-bound credential gets the resolved ARN or an empty string — never the shared default.
    let account_bound = context
        .auth_method
        .is_some_and(|method| ACCOUNT_BOUND.contains(&method));
    let profile = context
        .profile_arn
        .filter(|arn| !arn.is_empty())
        .map(str::to_owned)
        .or_else(|| (!account_bound).then(|| DEFAULT_PROFILE_ARN.to_owned()));
    if let Some(arn) = profile.filter(|arn| !arn.is_empty()) {
        payload.insert("profileArn".to_owned(), json!(arn));
    }

    let mut inference = Map::new();
    inference.insert("maxTokens".to_owned(), json!(MAX_TOKENS));
    if let Some(temperature) = body.get("temperature").filter(|value| value.is_number()) {
        inference.insert("temperature".to_owned(), temperature.clone());
    }
    if let Some(top_p) = body.get("top_p").filter(|value| value.is_number()) {
        inference.insert("topP".to_owned(), top_p.clone());
    }
    payload.insert("inferenceConfig".to_owned(), Value::Object(inference));

    Value::Object(payload)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Context, Turn, build, convert_messages};

    fn context() -> Context<'static> {
        Context {
            model: "claude-sonnet-4",
            auth_method: None,
            profile_arn: None,
            conversation_id: "conv-1",
            continuation_id: "cont-1",
            timestamp: "2026-09-01T00:00:00.000Z",
        }
    }

    #[test]
    fn the_last_user_turn_becomes_the_current_message_and_leaves_the_history() {
        // Repeating it in the history would send the question twice.
        let conversation = convert_messages(&[
            json!({ "role": "user", "content": "first" }),
            json!({ "role": "assistant", "content": "reply" }),
            json!({ "role": "user", "content": "second" }),
        ]);
        assert_eq!(
            conversation.current,
            Turn::User {
                content: "second".to_owned(),
                images: Vec::new(),
                tool_results: Vec::new(),
            }
        );
        assert_eq!(conversation.history.len(), 2);
        assert!(matches!(
            conversation.history.last(),
            Some(Turn::Assistant { .. })
        ));
    }

    #[test]
    fn a_trailing_assistant_turn_stays_in_the_history() {
        // The current message must be a *user* turn; the search skips past a trailing assistant one.
        let conversation = convert_messages(&[
            json!({ "role": "user", "content": "question" }),
            json!({ "role": "assistant", "content": "answer" }),
        ]);
        assert_eq!(
            conversation.current,
            Turn::User {
                content: "question".to_owned(),
                images: Vec::new(),
                tool_results: Vec::new(),
            }
        );
        assert!(matches!(
            conversation.history.first(),
            Some(Turn::Assistant { .. })
        ));
    }

    #[test]
    fn consecutive_user_turns_are_merged_with_their_contexts() {
        // CodeWhisperer rejects two user turns in a row. Merging only the text would drop a tool result
        // that arrived on the second one.
        let conversation = convert_messages(&[
            json!({ "role": "user", "content": "part one" }),
            json!({ "role": "tool", "tool_call_id": "call_1", "content": "the result" }),
            json!({ "role": "user", "content": "part two" }),
            json!({ "role": "assistant", "content": "ok" }),
            json!({ "role": "user", "content": "the question" }),
        ]);
        assert_eq!(conversation.history.len(), 2, "{:?}", conversation.history);
        let Some(Turn::User {
            content,
            tool_results,
            ..
        }) = conversation.history.first()
        else {
            panic!("the first history turn should be a merged user turn");
        };
        assert_eq!(content, "part one\n\npart two");
        assert_eq!(tool_results.len(), 1, "the tool result must survive");
        assert_eq!(
            tool_results
                .first()
                .and_then(|result| result.get("toolUseId")),
            Some(&json!("call_1"))
        );
    }

    #[test]
    fn a_system_turn_is_wrapped_in_instructions() {
        // Kiro has no system role. Claude models reached through it treat this tag as authoritative, which
        // is the nearest equivalent the protocol offers.
        let conversation = convert_messages(&[
            json!({ "role": "system", "content": "be terse" }),
            json!({ "role": "assistant", "content": "understood" }),
            json!({ "role": "user", "content": "hi" }),
        ]);
        let Some(Turn::User { content, .. }) = conversation.history.first() else {
            panic!("the system turn should become a user turn");
        };
        assert_eq!(content, "<instructions>\nbe terse\n</instructions>");
    }

    #[test]
    fn a_tool_call_and_its_result_become_a_tool_use_and_a_tool_result() {
        let conversation = convert_messages(&[
            json!({ "role": "user", "content": "read it" }),
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_7",
                    "type": "function",
                    "function": { "name": "read_file", "arguments": "{\"path\":\"a.txt\"}" },
                }],
            }),
            json!({ "role": "tool", "tool_call_id": "call_7", "content": "file contents" }),
        ]);

        let Some(Turn::Assistant { tool_uses, .. }) = conversation.history.get(1) else {
            panic!("the assistant turn should carry a tool use: {conversation:?}");
        };
        let use_entry = tool_uses.first().expect("a tool use");
        assert_eq!(use_entry.get("toolUseId"), Some(&json!("call_7")));
        assert_eq!(use_entry.get("name"), Some(&json!("read_file")));
        // The arguments are a JSON string on the wire and an object here.
        assert_eq!(use_entry.pointer("/input/path"), Some(&json!("a.txt")));

        // The result rides on the current user turn, since a `tool` role has no turn of its own.
        let Turn::User { tool_results, .. } = &conversation.current else {
            panic!("the current turn should be a user turn");
        };
        assert_eq!(
            tool_results.first().and_then(|result| result.get("status")),
            Some(&json!("success"))
        );
    }

    #[test]
    fn unparseable_tool_arguments_become_an_empty_object_rather_than_dropping_the_call() {
        // Dropping it would orphan the matching tool result, which is itself a rejection.
        let conversation = convert_messages(&[json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "function": { "name": "f", "arguments": "{not json" },
            }],
        })]);
        let Some(Turn::Assistant { tool_uses, .. }) = conversation.history.first() else {
            panic!("the call should survive: {conversation:?}");
        };
        assert_eq!(
            tool_uses.first().and_then(|entry| entry.get("input")),
            Some(&json!({}))
        );
    }

    #[test]
    fn an_errored_tool_result_says_so() {
        let conversation = convert_messages(&[json!({
            "role": "tool",
            "tool_call_id": "call_2",
            "content": "boom",
            "is_error": true,
        })]);
        let Turn::User { tool_results, .. } = &conversation.current else {
            panic!("the result should ride a user turn");
        };
        assert_eq!(
            tool_results.first().and_then(|result| result.get("status")),
            Some(&json!("error"))
        );
    }

    #[test]
    fn a_base64_image_is_carried_and_a_remote_one_is_named() {
        // Kiro accepts base64 only. Silently dropping a remote image would leave the model answering a
        // question about a picture it was never shown.
        let conversation = convert_messages(&[json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "what is this?" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAB" } },
                { "type": "image_url", "image_url": { "url": "https://example.com/cat.jpg" } },
            ],
        })]);
        let Turn::User {
            content, images, ..
        } = &conversation.current
        else {
            panic!("a user turn");
        };
        assert_eq!(images.len(), 1);
        assert_eq!(
            images.first().and_then(|image| image.get("format")),
            Some(&json!("png"))
        );
        assert_eq!(
            images
                .first()
                .and_then(|image| image.pointer("/source/bytes")),
            Some(&json!("AAAB"))
        );
        assert!(
            content.contains("[Image: https://example.com/cat.jpg]"),
            "{content}"
        );
    }

    #[test]
    fn a_claude_image_block_is_read_too() {
        // A Claude client's own block shape reaches here when it is the source format.
        let conversation = convert_messages(&[json!({
            "role": "user",
            "content": [{
                "type": "image",
                "source": { "type": "base64", "media_type": "image/jpeg", "data": "QUJD" },
            }],
        })]);
        let Turn::User { images, .. } = &conversation.current else {
            panic!("a user turn");
        };
        assert_eq!(
            images.first().and_then(|image| image.get("format")),
            Some(&json!("jpeg"))
        );
    }

    #[test]
    fn the_document_has_the_shape_codewhisperer_requires() {
        let body = json!({
            "model": "claude-sonnet-4",
            "messages": [{ "role": "user", "content": "hello" }],
        });
        let document = build(&body, &context());

        assert_eq!(
            document.pointer("/conversationState/chatTriggerType"),
            Some(&json!("MANUAL"))
        );
        assert_eq!(
            document.pointer("/conversationState/conversationId"),
            Some(&json!("conv-1"))
        );
        assert_eq!(
            document.pointer("/conversationState/agentContinuationId"),
            Some(&json!("cont-1"))
        );
        assert_eq!(document.get("agentMode"), Some(&json!("vibe")));
        // The current turn carries the model id and the time context.
        let content = document
            .pointer("/conversationState/currentMessage/userInputMessage/content")
            .and_then(Value::as_str)
            .expect("the current content");
        assert!(
            content.contains("[Context: Current time is 2026-09-01"),
            "{content}"
        );
        assert!(content.contains("hello"), "{content}");
        assert_eq!(
            document.pointer("/conversationState/currentMessage/userInputMessage/modelId"),
            Some(&json!("claude-sonnet-4"))
        );
        // History is empty when the only turn became the current message.
        assert_eq!(
            document.pointer("/conversationState/history"),
            Some(&json!([]))
        );
        assert_eq!(
            document.pointer("/inferenceConfig/maxTokens"),
            Some(&json!(32_000))
        );
    }

    #[test]
    fn an_account_bound_credential_never_gets_the_shared_default_profile() {
        // The shared ARN belongs to a different account, and sending it draws `403 bearer token invalid`.
        let body = json!({ "messages": [{ "role": "user", "content": "hi" }] });
        for method in ["api_key", "idc", "external_idp"] {
            let document = build(
                &body,
                &Context {
                    auth_method: Some(method),
                    ..context()
                },
            );
            assert!(
                document.get("profileArn").is_none(),
                "{method} must not receive the default ARN"
            );
        }

        // With a resolved ARN it is sent as-is.
        let resolved = build(
            &body,
            &Context {
                auth_method: Some("api_key"),
                profile_arn: Some("arn:aws:codewhisperer:us-east-1:1234:profile/OWN"),
                ..context()
            },
        );
        assert_eq!(
            resolved.get("profileArn"),
            Some(&json!("arn:aws:codewhisperer:us-east-1:1234:profile/OWN"))
        );

        // An OAuth or social connection keeps the shared default, which its token accepts.
        let social = build(
            &body,
            &Context {
                auth_method: Some("social"),
                ..context()
            },
        );
        assert!(
            social
                .get("profileArn")
                .and_then(Value::as_str)
                .is_some_and(|arn| arn.contains("699475941385")),
            "an OAuth connection should keep the shared default"
        );
    }

    #[test]
    fn empty_turns_carry_the_placeholders_the_endpoint_requires() {
        // An empty user or assistant turn is rejected outright.
        let body = json!({
            "messages": [
                { "role": "user", "content": "q" },
                { "role": "assistant", "content": "" },
                { "role": "user", "content": "" },
            ],
        });
        let document = build(&body, &context());
        let history = document
            .pointer("/conversationState/history")
            .and_then(Value::as_array)
            .expect("a history");
        assert!(
            history.iter().any(
                |turn| turn.pointer("/assistantResponseMessage/content") == Some(&json!("..."))
            ),
            "{history:?}"
        );
        // And the current turn, empty but for the time context, still has content.
        let current = document
            .pointer("/conversationState/currentMessage/userInputMessage/content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(!current.is_empty());
    }

    #[test]
    fn an_empty_context_object_is_omitted_rather_than_sent_empty() {
        let body = json!({ "messages": [{ "role": "user", "content": "hi" }] });
        let document = build(&body, &context());
        assert!(
            document
                .pointer(
                    "/conversationState/currentMessage/userInputMessage/userInputMessageContext"
                )
                .is_none(),
            "an empty context object is rejected"
        );
    }

    #[test]
    fn sampling_parameters_are_forwarded_only_when_given() {
        let with_params = build(
            &json!({
                "messages": [{ "role": "user", "content": "hi" }],
                "temperature": 0.3,
                "top_p": 0.9,
            }),
            &context(),
        );
        assert_eq!(
            with_params.pointer("/inferenceConfig/temperature"),
            Some(&json!(0.3))
        );
        assert_eq!(
            with_params.pointer("/inferenceConfig/topP"),
            Some(&json!(0.9))
        );

        let without = build(
            &json!({ "messages": [{ "role": "user", "content": "hi" }] }),
            &context(),
        );
        assert!(without.pointer("/inferenceConfig/temperature").is_none());
        assert!(without.pointer("/inferenceConfig/topP").is_none());
    }

    #[test]
    fn a_conversation_of_only_assistant_turns_still_gets_a_current_message() {
        // Otherwise the document is missing a field the endpoint requires.
        let document = build(
            &json!({ "messages": [{ "role": "assistant", "content": "unprompted" }] }),
            &context(),
        );
        assert!(
            document
                .pointer("/conversationState/currentMessage/userInputMessage")
                .is_some()
        );
        assert_eq!(
            document
                .pointer("/conversationState/history")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
    }
}
