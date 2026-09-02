//! Building Cursor's `StreamUnifiedChatRequestWithTools` protobuf.
//!
//! Ports the request half of `open-sse/utils/cursorProtobuf.js`. Cursor's schema is not published, so
//! every field number here was established by observing the IDE. Several carry no known meaning and are
//! named for their number — they are sent because the endpoint rejects a request without them, which is
//! the only fact available about them. Guessing a name would claim knowledge this port does not have.

use serde_json::Value;

use super::protobuf::{bytes_field, put_bool, put_bytes, put_str, put_uint, put_varint};

/// Field numbers in `StreamUnifiedChatRequest`.
mod field {
    pub(super) const MESSAGES: u32 = 1;
    pub(super) const UNKNOWN_2: u32 = 2;
    pub(super) const INSTRUCTION: u32 = 3;
    pub(super) const UNKNOWN_4: u32 = 4;
    pub(super) const MODEL: u32 = 5;
    pub(super) const WEB_TOOL: u32 = 8;
    pub(super) const UNKNOWN_13: u32 = 13;
    pub(super) const CURSOR_SETTING: u32 = 15;
    pub(super) const UNKNOWN_19: u32 = 19;
    pub(super) const CONVERSATION_ID: u32 = 23;
    pub(super) const METADATA: u32 = 26;
    pub(super) const IS_AGENTIC: u32 = 27;
    pub(super) const SUPPORTED_TOOLS: u32 = 29;
    pub(super) const MESSAGE_IDS: u32 = 30;
    pub(super) const MCP_TOOLS: u32 = 34;
    pub(super) const LARGE_CONTEXT: u32 = 35;
    pub(super) const UNKNOWN_38: u32 = 38;
    pub(super) const UNIFIED_MODE: u32 = 46;
    pub(super) const UNKNOWN_47: u32 = 47;
    pub(super) const SHOULD_DISABLE_TOOLS: u32 = 48;
    pub(super) const THINKING_LEVEL: u32 = 49;
    pub(super) const UNKNOWN_51: u32 = 51;
    pub(super) const UNKNOWN_53: u32 = 53;
    pub(super) const UNIFIED_MODE_NAME: u32 = 54;
}

/// Field numbers in `ConversationMessage`.
mod message {
    pub(super) const CONTENT: u32 = 1;
    pub(super) const ROLE: u32 = 2;
    pub(super) const ID: u32 = 13;
    pub(super) const IS_AGENTIC: u32 = 29;
    pub(super) const UNIFIED_MODE: u32 = 47;
    pub(super) const SUPPORTED_TOOLS: u32 = 51;
}

/// The wrapper field: `StreamUnifiedChatRequestWithTools.request`.
const WITH_TOOLS_REQUEST: u32 = 1;

/// `ConversationMessage.role`.
const ROLE_USER: u64 = 1;
const ROLE_ASSISTANT: u64 = 2;

/// `UnifiedMode`.
const MODE_CHAT: u64 = 1;
const MODE_AGENT: u64 = 2;

/// `ThinkingLevel`.
const THINKING_UNSPECIFIED: u64 = 0;
const THINKING_MEDIUM: u64 = 1;
const THINKING_HIGH: u64 = 2;

/// The text of a message's content, whether a string or a content-part array.
pub(crate) fn text_from_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _absent_or_other => String::new(),
    }
}

/// Whether a conversation is plain text: no tool calls and no tool results anywhere.
///
/// Upstream routes these to `AgentService`, because Cursor's `ChatService` has been retired and rejects a
/// request carrying tool schemas — which many clients attach to every turn, including plain ones.
pub(crate) fn is_plain_text(messages: &[Value]) -> bool {
    messages.iter().all(|message| {
        if message.get("role").and_then(Value::as_str) == Some("tool") {
            return false;
        }
        if message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty())
        {
            return false;
        }
        match message.get("content") {
            Some(Value::String(_text)) => true,
            None => true,
            Some(Value::Array(parts)) => parts
                .iter()
                .all(|part| part.get("type").and_then(Value::as_str) == Some("text")),
            Some(_other) => false,
        }
    })
}

/// `CursorSetting`, sent with the settings path the IDE reports.
///
/// The backslash in `cursor\aisettings` is Cursor's own spelling on every platform, not a Windows path
/// this port should normalise.
fn cursor_setting() -> Vec<u8> {
    let mut unknown6 = Vec::new();
    put_bytes(&mut unknown6, 1, &[]);
    put_bytes(&mut unknown6, 2, &[]);

    let mut out = Vec::new();
    put_str(&mut out, 1, r"cursor\aisettings");
    put_bytes(&mut out, 3, &[]);
    put_bytes(&mut out, 6, &unknown6);
    put_uint(&mut out, 8, 1);
    put_uint(&mut out, 9, 1);
    out
}

/// `Metadata`: the client's platform, version, working directory, and a timestamp.
///
/// Upstream reports the Node process's real values, including `process.cwd()`. This port sends a fixed
/// `/` instead: the working directory of the router process says nothing about the user's request and
/// discloses the server's filesystem layout to Cursor. The rest is reported honestly.
fn metadata(timestamp: &str) -> Vec<u8> {
    let (platform, arch) = if cfg!(target_os = "windows") {
        ("win32", "x64")
    } else if cfg!(target_os = "macos") {
        ("darwin", "arm64")
    } else {
        ("linux", "x64")
    };
    let mut out = Vec::new();
    put_str(&mut out, 1, platform);
    put_str(&mut out, 2, arch);
    put_str(
        &mut out,
        3,
        concat!("nullrouter/", env!("CARGO_PKG_VERSION")),
    );
    put_str(&mut out, 4, "/");
    put_str(&mut out, 5, timestamp);
    out
}

/// `Model`: the name, plus an empty field 4 the endpoint expects to be present.
fn model_message(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, name);
    put_bytes(&mut out, 4, &[]);
    out
}

/// One `ConversationMessage`.
fn conversation_message(
    content: &str,
    role: u64,
    id: &str,
    is_last: bool,
    has_tools: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, message::CONTENT, content);
    put_uint(&mut out, message::ROLE, role);
    put_str(&mut out, message::ID, id);
    put_bool(&mut out, message::IS_AGENTIC, has_tools);
    put_uint(
        &mut out,
        message::UNIFIED_MODE,
        if has_tools { MODE_AGENT } else { MODE_CHAT },
    );
    if is_last && has_tools {
        // A packed varint list holding a single tool id.
        let mut packed = Vec::new();
        put_varint(&mut packed, 1);
        put_bytes(&mut out, message::SUPPORTED_TOOLS, &packed);
    }
    out
}

/// One `MessageId`.
fn message_id(id: &str, role: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, id);
    put_uint(&mut out, 3, role);
    out
}

/// One `MCPTool`, from an OpenAI tool declaration.
fn mcp_tool(tool: &Value) -> Vec<u8> {
    let function = tool.get("function").unwrap_or(tool);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let description = function
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let schema = function
        .get("parameters")
        .or_else(|| tool.get("input_schema"));

    let mut out = Vec::new();
    if !name.is_empty() {
        put_str(&mut out, 1, name);
    }
    if !description.is_empty() {
        put_str(&mut out, 2, description);
    }
    if let Some(schema) =
        schema.filter(|schema| schema.as_object().is_some_and(|object| !object.is_empty()))
    {
        put_str(&mut out, 3, &schema.to_string());
    }
    // Every tool this router forwards is declared by the client, which Cursor sees as a custom server.
    put_str(&mut out, 4, "custom");
    out
}

/// What a request needs that cannot be derived from the body.
///
/// Passed in rather than generated inside, so a test can assert on the bytes: a request carrying fresh
/// uuids differs on every call and could only be asserted loosely.
#[derive(Debug, Clone)]
pub(crate) struct Ids {
    /// One id per message, in the conversation's order.
    pub(crate) per_message: Vec<String>,
    /// This conversation's id.
    pub(crate) conversation_id: String,
    /// An ISO-8601 timestamp for the metadata block.
    pub(crate) timestamp: String,
}

/// Build the framed request body.
///
/// `force_agent` is set when the caller is an agentic client that sends no tool schemas of its own;
/// upstream keys that off the inbound `user-agent`, which reaches this port as an explicit flag instead.
pub(crate) fn build(
    messages: &[Value],
    model: &str,
    tools: &[Value],
    reasoning_effort: Option<&str>,
    force_agent: bool,
    ids: &Ids,
) -> Vec<u8> {
    let has_tools = !tools.is_empty();
    let is_agentic = has_tools || force_agent;
    let last_index = messages.len().saturating_sub(1);

    let mut body = Vec::new();

    for (index, entry) in messages.iter().enumerate() {
        let role = match entry.get("role").and_then(Value::as_str) {
            Some("user") => ROLE_USER,
            // Cursor's ConversationMessage has two roles. A system turn is carried as an assistant one,
            // which is how upstream maps everything that is not a user.
            _assistant_or_system => ROLE_ASSISTANT,
        };
        let content = text_from_content(entry.get("content"));
        let id = ids
            .per_message
            .get(index)
            .map(String::as_str)
            .unwrap_or_default();
        put_bytes(
            &mut body,
            field::MESSAGES,
            &conversation_message(&content, role, id, index == last_index, has_tools),
        );
    }

    put_uint(&mut body, field::UNKNOWN_2, 1);
    // An empty `Instruction`. The system prompt travels as a message, not here.
    put_bytes(&mut body, field::INSTRUCTION, &[]);
    put_uint(&mut body, field::UNKNOWN_4, 1);
    put_bytes(&mut body, field::MODEL, &model_message(model));
    put_str(&mut body, field::WEB_TOOL, "");
    put_uint(&mut body, field::UNKNOWN_13, 1);
    put_bytes(&mut body, field::CURSOR_SETTING, &cursor_setting());
    put_uint(&mut body, field::UNKNOWN_19, 1);
    put_str(&mut body, field::CONVERSATION_ID, &ids.conversation_id);
    put_bytes(&mut body, field::METADATA, &metadata(&ids.timestamp));

    put_bool(&mut body, field::IS_AGENTIC, is_agentic);
    if is_agentic {
        let mut packed = Vec::new();
        put_varint(&mut packed, 1);
        put_bytes(&mut body, field::SUPPORTED_TOOLS, &packed);
    }

    for (index, entry) in messages.iter().enumerate() {
        let role = match entry.get("role").and_then(Value::as_str) {
            Some("user") => ROLE_USER,
            _assistant_or_system => ROLE_ASSISTANT,
        };
        let id = ids
            .per_message
            .get(index)
            .map(String::as_str)
            .unwrap_or_default();
        put_bytes(&mut body, field::MESSAGE_IDS, &message_id(id, role));
    }

    for tool in tools {
        put_bytes(&mut body, field::MCP_TOOLS, &mcp_tool(tool));
    }

    put_uint(&mut body, field::LARGE_CONTEXT, 0);
    put_uint(&mut body, field::UNKNOWN_38, 0);
    put_uint(
        &mut body,
        field::UNIFIED_MODE,
        if is_agentic { MODE_AGENT } else { MODE_CHAT },
    );
    put_str(&mut body, field::UNKNOWN_47, "");
    // Inverted on purpose: tools are disabled exactly when the request is not agentic.
    put_bool(&mut body, field::SHOULD_DISABLE_TOOLS, !is_agentic);
    put_uint(
        &mut body,
        field::THINKING_LEVEL,
        match reasoning_effort {
            Some("medium") => THINKING_MEDIUM,
            Some("high") => THINKING_HIGH,
            // Cursor has no low level, so a low or absent effort is unspecified rather than invented.
            _none_or_low => THINKING_UNSPECIFIED,
        },
    );
    put_uint(&mut body, field::UNKNOWN_51, 0);
    put_uint(&mut body, field::UNKNOWN_53, 1);
    put_str(
        &mut body,
        field::UNIFIED_MODE_NAME,
        if is_agentic { "Agent" } else { "Ask" },
    );

    super::protobuf::frame(&bytes_field(WITH_TOOLS_REQUEST, &body))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::protobuf::{FieldValue, Message, frames};
    use super::{Ids, build, is_plain_text, text_from_content};

    fn ids(count: usize) -> Ids {
        Ids {
            per_message: (0..count).map(|index| format!("msg-{index}")).collect(),
            conversation_id: "conv-1".to_owned(),
            timestamp: "2026-09-01T00:00:00.000Z".to_owned(),
        }
    }

    /// Decode a built body back to the inner `StreamUnifiedChatRequest`.
    fn decode(body: &[u8]) -> Message {
        let (read, _consumed) = frames(body);
        let frame = read.first().expect("one frame");
        let outer = Message::decode(&frame.payload);
        outer.nested(1).expect("the request wrapper")
    }

    #[test]
    fn content_parts_are_flattened_to_text() {
        assert_eq!(text_from_content(Some(&json!("plain"))), "plain");
        assert_eq!(
            text_from_content(Some(&json!([
                { "type": "text", "text": "one" },
                { "type": "image_url", "image_url": { "url": "…" } },
                { "type": "text", "text": "two" },
            ]))),
            "one\ntwo"
        );
        assert_eq!(text_from_content(None), "");
    }

    #[test]
    fn a_plain_conversation_is_told_apart_from_a_tool_one() {
        // Cursor's ChatService is retired and rejects a request carrying tool schemas, so a plain turn has
        // to be recognised even when the client attached its whole toolbox.
        assert!(is_plain_text(&[json!({ "role": "user", "content": "hi" })]));
        assert!(is_plain_text(&[
            json!({ "role": "user", "content": [{ "type": "text", "text": "hi" }] }),
            json!({ "role": "assistant", "content": "hello" }),
        ]));
        assert!(!is_plain_text(&[
            json!({ "role": "tool", "content": "result" })
        ]));
        assert!(!is_plain_text(&[json!({
            "role": "assistant",
            "tool_calls": [{ "id": "call_1", "function": { "name": "f", "arguments": "{}" } }],
        })]));
        // An image part is not text, so it is not a plain-text conversation.
        assert!(!is_plain_text(&[json!({
            "role": "user",
            "content": [{ "type": "image_url", "image_url": { "url": "…" } }],
        })]));
    }

    #[test]
    fn a_chat_request_carries_its_messages_model_and_mode() {
        let messages = vec![
            json!({ "role": "user", "content": "first" }),
            json!({ "role": "assistant", "content": "reply" }),
            json!({ "role": "user", "content": "second" }),
        ];
        let body = build(&messages, "claude-4.5-sonnet", &[], None, false, &ids(3));
        let request = decode(&body);

        // The model sits in a nested `Model` message with an empty field 4 the endpoint expects.
        let model = request.nested(5).expect("a model message");
        assert_eq!(model.text(1).as_deref(), Some("claude-4.5-sonnet"));
        assert_eq!(model.get(4), Some(&FieldValue::Bytes(Vec::new())));

        // Without tools the request is a chat, and tools are disabled — field 48 is inverted.
        assert_eq!(request.get(27), Some(&FieldValue::Uint(0)), "is_agentic");
        assert_eq!(
            request.get(48),
            Some(&FieldValue::Uint(1)),
            "should_disable_tools is the inverse of agentic"
        );
        assert_eq!(
            request.get(46),
            Some(&FieldValue::Uint(1)),
            "unified mode chat"
        );
        assert_eq!(request.text(54).as_deref(), Some("Ask"));
        assert_eq!(request.text(23).as_deref(), Some("conv-1"));

        // The first message decodes with its role and id.
        let first = request.nested(1).expect("a message");
        assert_eq!(first.text(1).as_deref(), Some("first"));
        assert_eq!(first.get(2), Some(&FieldValue::Uint(1)), "user role");
        assert_eq!(first.text(13).as_deref(), Some("msg-0"));
    }

    #[test]
    fn every_message_gets_an_id_entry_in_conversation_order() {
        let messages = vec![
            json!({ "role": "user", "content": "a" }),
            json!({ "role": "assistant", "content": "b" }),
        ];
        let body = build(&messages, "m", &[], None, false, &ids(2));
        let (read, _consumed) = frames(&body);
        let payload = read.first().map(|f| f.payload.clone()).expect("a frame");
        let inner = Message::decode(&payload)
            .nested(1)
            .expect("the request wrapper");

        // Field 30 repeats once per message. `get` returns the first, which must be message 0's.
        let first_id = inner.nested(30).expect("a message id");
        assert_eq!(first_id.text(1).as_deref(), Some("msg-0"));
        assert_eq!(first_id.get(3), Some(&FieldValue::Uint(1)), "user role");
    }

    #[test]
    fn declared_tools_turn_the_request_agentic() {
        let messages = vec![json!({ "role": "user", "content": "read the file" })];
        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": { "type": "object", "properties": { "path": { "type": "string" } } },
            },
        })];
        let body = build(&messages, "m", &tools, None, false, &ids(1));
        let request = decode(&body);

        assert_eq!(request.get(27), Some(&FieldValue::Uint(1)), "is_agentic");
        assert_eq!(
            request.get(48),
            Some(&FieldValue::Uint(0)),
            "tools stay enabled"
        );
        assert_eq!(request.get(46), Some(&FieldValue::Uint(2)), "agent mode");
        assert_eq!(request.text(54).as_deref(), Some("Agent"));

        let tool = request.nested(34).expect("an mcp tool");
        assert_eq!(tool.text(1).as_deref(), Some("read_file"));
        assert_eq!(tool.text(2).as_deref(), Some("Read a file"));
        // The schema travels as JSON text, and the server is always `custom` — every tool here was
        // declared by the client rather than by a Cursor-side MCP server.
        assert!(
            tool.text(3)
                .is_some_and(|schema| schema.contains("\"path\"")),
            "the schema should be JSON"
        );
        assert_eq!(tool.text(4).as_deref(), Some("custom"));
    }

    #[test]
    fn a_tool_less_agent_client_can_still_force_agent_mode() {
        // Upstream keys this off the inbound user agent. Some agentic clients send no schemas of their own
        // and still need Agent mode, or Cursor answers as a chat.
        let messages = vec![json!({ "role": "user", "content": "hi" })];
        let forced = decode(&build(&messages, "m", &[], None, true, &ids(1)));
        assert_eq!(forced.get(27), Some(&FieldValue::Uint(1)));
        assert_eq!(forced.text(54).as_deref(), Some("Agent"));
        // But no MCP tool is invented for it.
        assert!(forced.get(34).is_none());
    }

    #[test]
    fn reasoning_effort_maps_onto_the_three_thinking_levels() {
        let messages = vec![json!({ "role": "user", "content": "hi" })];
        let level = |effort: Option<&str>| {
            decode(&build(&messages, "m", &[], effort, false, &ids(1)))
                .get(49)
                .cloned()
        };
        assert_eq!(level(Some("high")), Some(FieldValue::Uint(2)));
        assert_eq!(level(Some("medium")), Some(FieldValue::Uint(1)));
        // Cursor has no low level, so low is unspecified rather than invented as medium.
        assert_eq!(level(Some("low")), Some(FieldValue::Uint(0)));
        assert_eq!(level(None), Some(FieldValue::Uint(0)));
    }

    #[test]
    fn the_metadata_block_does_not_disclose_the_servers_filesystem() {
        // Upstream sends `process.cwd()`. The router's working directory says nothing about the request
        // and discloses the server's layout to Cursor.
        let messages = vec![json!({ "role": "user", "content": "hi" })];
        let request = decode(&build(&messages, "m", &[], None, false, &ids(1)));
        let metadata = request.nested(26).expect("a metadata block");
        assert_eq!(metadata.text(4).as_deref(), Some("/"));
        assert!(
            metadata
                .text(3)
                .is_some_and(|version| version.starts_with("nullrouter/")),
            "the client version should identify this port"
        );
        assert_eq!(
            metadata.text(5).as_deref(),
            Some("2026-09-01T00:00:00.000Z")
        );
    }

    #[test]
    fn the_body_is_one_connect_frame_wrapping_one_field() {
        let messages = vec![json!({ "role": "user", "content": "hi" })];
        let body = build(&messages, "m", &[], None, false, &ids(1));
        let (read, consumed) = frames(&body);
        assert_eq!(read.len(), 1);
        assert_eq!(consumed, body.len(), "no trailing bytes");
        assert_eq!(
            read.first().map(|frame| frame.flags),
            Some(0),
            "Cursor rejects a compressed request"
        );
        // The frame's payload is `StreamUnifiedChatRequestWithTools`, whose only field is the request.
        let outer = Message::decode(&read.first().expect("a frame").payload);
        assert_eq!(outer.field_numbers(), vec![1]);
    }

    #[test]
    fn a_system_message_is_carried_as_an_assistant_turn() {
        // Cursor's ConversationMessage has two roles. Dropping a system turn would lose the prompt.
        let messages = vec![
            json!({ "role": "system", "content": "be terse" }),
            json!({ "role": "user", "content": "hi" }),
        ];
        let request = decode(&build(&messages, "m", &[], None, false, &ids(2)));
        let first = request.nested(1).expect("a message");
        assert_eq!(first.text(1).as_deref(), Some("be terse"));
        assert_eq!(first.get(2), Some(&FieldValue::Uint(2)), "assistant role");
    }

    #[test]
    fn an_empty_conversation_still_produces_a_well_formed_request() {
        let request = decode(&build(&[], "m", &[], None, false, &ids(0)));
        assert!(request.get(1).is_none(), "no messages");
        // The static fields the endpoint requires are still there.
        assert_eq!(request.text(54).as_deref(), Some("Ask"));
        assert!(request.nested(15).is_some(), "the cursor setting block");
    }
}
