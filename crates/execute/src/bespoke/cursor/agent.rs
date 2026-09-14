//! Cursor `AgentService` (`agent.v1.AgentService/Run`) request and duplex decode.
//!
//! Ports the AgentService half of `open-sse/executors/cursor.js`. Cursor retired `ChatService` for
//! plain-text turns; those now go to this endpoint. The wire is still Connect-RPC protobuf, so this
//! module reuses [`super::protobuf`] rather than inventing a second codec.
//!
//! Field numbers were established against Cursor's `agent.proto` (the same numbers the reference
//! executor encodes). A Cursor release can add a field at any time, so decode of unknown fields is
//! the same as everywhere else in this crate: keep them, do not refuse.
//!
//! # Duplex
//!
//! The server asks for IDE file context mid-stream (`ExecServerMessage` field 10) and the client
//! must answer on the *same open stream* before the answer continues. This module produces that
//! empty-context acknowledgement; the caller is responsible for writing it back. A router has no
//! editor and no open files, so the acknowledgement is always empty rather than fabricated file
//! contents — inventing context here would put text into the model's prompt that the user never
//! wrote.

use serde_json::Value;

use super::protobuf::{self, Message, bytes_field, frame, put_bool, put_str};
use super::request;

/// RPC path Cursor's AgentService listens on.
pub const RUN_PATH: &str = "/agent.v1.AgentService/Run";

/// `agent.v1.AgentClientMessage.run_request`.
const CLIENT_RUN_REQUEST: u32 = 1;
/// `agent.v1.AgentClientMessage.exec_client_message`.
const CLIENT_EXEC: u32 = 2;

/// `agent.v1.AgentServerMessage.interaction_update`.
const SERVER_INTERACTION: u32 = 1;
/// `agent.v1.AgentServerMessage.exec_request`.
const SERVER_EXEC: u32 = 2;

/// `UserMessageAction.user_message`.
const USER_MESSAGE: u32 = 1;
/// `UserMessageAction.conversation_history`.
const USER_HISTORY: u32 = 7;
/// `ConversationAction.user_action`.
const CONVERSATION_USER: u32 = 1;
/// `RunRequest.conversation_state`.
const RUN_STATE: u32 = 1;
/// `RunRequest.conversation_action`.
const RUN_ACTION: u32 = 2;
/// `RunRequest.system`.
const RUN_SYSTEM: u32 = 8;
/// `RunRequest.requested_model`.
const RUN_MODEL: u32 = 9;

/// `RequestedModel.name`.
const MODEL_NAME: u32 = 1;
/// `RequestedModel.max_mode` (upstream always sets this true).
const MODEL_MAX: u32 = 7;

/// `ConversationHistoryMessage.user` / `.assistant`.
const HISTORY_USER: u32 = 1;
const HISTORY_ASSISTANT: u32 = 2;

/// `ExecClientMessage.request_context_result`.
const EXEC_CLIENT_CONTEXT: u32 = 10;
/// `ExecServerMessage.request_context`.
const EXEC_SERVER_CONTEXT: u32 = 10;

/// `InteractionUpdate.text_delta`.
const UPDATE_TEXT: u32 = 1;
/// `InteractionUpdate.turn_ended`.
const UPDATE_ENDED: u32 = 14;

/// One event decoded from an AgentService server frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Assistant text, already UTF-8.
    Text(String),
    /// The server asked for IDE context. The caller must write [`context_ack`] on the same stream.
    RequestContext,
    /// The turn ended (`turn_ended` field 14).
    Done,
    /// An editor-backed tool this router cannot service.
    UnsupportedExec,
}

/// Build the Connect-RPC framed `AgentClientMessage.run_request` for a chat body.
///
/// `message_id` is the UUID Cursor puts on the current user message. Tests pin it; production
/// generates one per request.
pub fn run_frame(body: &Value, message_id: &str) -> Vec<u8> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    frame(&bytes_field(
        CLIENT_RUN_REQUEST,
        &run_request(messages, model, message_id),
    ))
}

/// The empty-context acknowledgement AgentService waits for before producing the answer.
pub fn context_ack() -> Vec<u8> {
    // agent.v1.RequestContextSuccess (empty) → RequestContextResult.success →
    // ExecClientMessage.request_context_result → AgentClientMessage.exec_client_message.
    let success = Vec::new();
    let result = bytes_field(1, &success);
    let exec = bytes_field(EXEC_CLIENT_CONTEXT, &result);
    frame(&bytes_field(CLIENT_EXEC, &exec))
}

/// Decode one AgentService *payload* (the Connect frame's inner protobuf) into zero or more events.
pub fn decode_payload(payload: &[u8]) -> Vec<Event> {
    let server = Message::decode(payload);
    let mut events = Vec::new();
    if let Some(update) = server.nested(SERVER_INTERACTION) {
        if let Some(text) = update
            .nested(UPDATE_TEXT)
            .and_then(|delta| delta.text(1))
            .filter(|text| !text.is_empty())
        {
            events.push(Event::Text(text));
        }
        if update.get(UPDATE_ENDED).is_some() {
            events.push(Event::Done);
        }
    }
    if let Some(exec) = server.nested(SERVER_EXEC) {
        if exec.get(EXEC_SERVER_CONTEXT).is_some() {
            events.push(Event::RequestContext);
        } else {
            events.push(Event::UnsupportedExec);
        }
    }
    events
}

/// Split a byte stream into AgentService events, returning leftover incomplete-frame bytes.
pub fn decode_stream(buffer: &[u8]) -> (Vec<Event>, usize) {
    let (frames, consumed) = protobuf::frames(buffer);
    let mut events = Vec::new();
    for frame in frames {
        if frame.is_trailer() {
            continue;
        }
        events.extend(decode_payload(&frame.payload));
    }
    (events, consumed)
}

fn run_request(messages: &[Value], model: &str, message_id: &str) -> Vec<u8> {
    let system = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("system"))
        .map(|message| request::text_from_content(message.get("content")))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let chat: Vec<&Value> = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) != Some("system"))
        .collect();
    let current_index = chat
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"));
    let current = current_index
        .and_then(|index| chat.get(index).copied())
        .or_else(|| chat.last().copied());
    let history_end = current_index.unwrap_or_else(|| chat.len().saturating_sub(1));
    let history: Vec<Vec<u8>> = chat
        .iter()
        .take(history_end)
        .filter_map(|message| history_entry(message))
        .collect();

    let user_text = current
        .map(|message| request::text_from_content(message.get("content")))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Continue.".to_owned());

    let mut user_message = Vec::new();
    put_str(&mut user_message, 1, &user_text);
    put_str(&mut user_message, 2, message_id);

    let mut user_action = bytes_field(USER_MESSAGE, &user_message);
    if !history.is_empty() {
        let mut conversation_history = Vec::new();
        for entry in history {
            conversation_history.extend(bytes_field(1, &entry));
        }
        user_action.extend(bytes_field(USER_HISTORY, &conversation_history));
    }
    let conversation_action = bytes_field(CONVERSATION_USER, &user_action);

    let mut requested_model = Vec::new();
    put_str(&mut requested_model, MODEL_NAME, model);
    put_bool(&mut requested_model, MODEL_MAX, true);

    let mut out = Vec::new();
    // An empty ConversationStateStructure starts a fresh local agent session.
    out.extend(bytes_field(RUN_STATE, &[]));
    out.extend(bytes_field(RUN_ACTION, &conversation_action));
    if !system.is_empty() {
        put_str(&mut out, RUN_SYSTEM, &system);
    }
    out.extend(bytes_field(RUN_MODEL, &requested_model));
    out
}

fn history_entry(message: &Value) -> Option<Vec<u8>> {
    let content = request::text_from_content(message.get("content"));
    if content.is_empty() {
        return None;
    }
    let text = bytes_field(1, content.as_bytes());
    let inner = bytes_field(1, &bytes_field(1, &text));
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let field = if role == "assistant" {
        HISTORY_ASSISTANT
    } else {
        HISTORY_USER
    };
    Some(bytes_field(field, &inner))
}

/// Encode an `interaction_update` carrying a text delta, as the server does. Tests use this to
/// speak the protocol without a live Cursor.
pub fn encode_text_delta(text: &str) -> Vec<u8> {
    let delta = bytes_field(1, text.as_bytes());
    let update = bytes_field(UPDATE_TEXT, &delta);
    frame(&bytes_field(SERVER_INTERACTION, &update))
}

/// Encode the mid-stream RequestContext ask.
pub fn encode_request_context() -> Vec<u8> {
    let exec = bytes_field(EXEC_SERVER_CONTEXT, &[]);
    frame(&bytes_field(SERVER_EXEC, &exec))
}

/// Encode the turn-ended marker.
pub fn encode_turn_ended() -> Vec<u8> {
    let mut update = Vec::new();
    put_bool(&mut update, UPDATE_ENDED, true);
    frame(&bytes_field(SERVER_INTERACTION, &update))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "a frame header that is not five bytes is a test failure, which is what the panic reports"
)]
mod tests {
    use serde_json::json;

    use super::{
        Event, RUN_PATH, context_ack, decode_payload, decode_stream, encode_request_context,
        encode_text_delta, encode_turn_ended, run_frame,
    };
    use crate::bespoke::cursor::protobuf::{Message, bytes_field};

    fn unwrap_run(bytes: &[u8]) -> Message {
        // Skip the 5-byte Connect header.
        let payload = bytes.get(5..).expect("framed");
        Message::decode(payload)
            .nested(1)
            .expect("AgentClientMessage.run_request")
    }

    #[test]
    fn the_rpc_path_is_the_reference_one() {
        assert_eq!(RUN_PATH, "/agent.v1.AgentService/Run");
    }

    #[test]
    fn a_plain_turn_puts_the_user_text_and_model_on_the_run_request() {
        let body = json!({
            "model": "claude-4.5-sonnet",
            "messages": [{ "role": "user", "content": "hello agent" }],
        });
        let run = unwrap_run(&run_frame(&body, "msg-fixed"));
        // Field 1 is the empty ConversationStateStructure.
        assert!(run.get(1).is_some());
        let action = run.nested(2).expect("conversation_action");
        let user_action = action.nested(1).expect("user_action");
        let user_message = user_action.nested(1).expect("user_message");
        assert_eq!(user_message.text(1).as_deref(), Some("hello agent"));
        assert_eq!(user_message.text(2).as_deref(), Some("msg-fixed"));
        let model = run.nested(9).expect("requested_model");
        assert_eq!(model.text(1).as_deref(), Some("claude-4.5-sonnet"));
        assert_eq!(
            model.get(7).and_then(|value| match value {
                crate::bespoke::cursor::protobuf::FieldValue::Uint(flag) => Some(*flag),
                crate::bespoke::cursor::protobuf::FieldValue::Bytes(_) => None,
            }),
            Some(1)
        );
    }

    #[test]
    fn system_text_is_lifted_out_of_the_history() {
        let body = json!({
            "model": "gpt",
            "messages": [
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hi" },
            ],
        });
        let run = unwrap_run(&run_frame(&body, "id"));
        assert_eq!(run.text(8).as_deref(), Some("be brief"));
        let user_action = run
            .nested(2)
            .and_then(|action| action.nested(1))
            .expect("user");
        // History is field 7 and must be absent: the only chat turn is the current user one.
        assert!(user_action.get(7).is_none());
    }

    #[test]
    fn prior_turns_land_in_conversation_history_under_the_right_role() {
        let body = json!({
            "model": "gpt",
            "messages": [
                { "role": "user", "content": "first" },
                { "role": "assistant", "content": "second" },
                { "role": "user", "content": "third" },
            ],
        });
        let run = unwrap_run(&run_frame(&body, "id"));
        let user_action = run
            .nested(2)
            .and_then(|action| action.nested(1))
            .expect("user");
        let history = user_action.nested(7).expect("conversation_history");
        assert_eq!(history.all(1).count(), 2, "user then assistant prior turns");
        let current = user_action.nested(1).expect("current user_message");
        assert_eq!(current.text(1).as_deref(), Some("third"));
    }

    #[test]
    fn an_empty_user_turn_is_sent_as_continue_rather_than_dropped() {
        // Dropping it would make the previous assistant turn look like the current message.
        let body = json!({
            "model": "gpt",
            "messages": [
                { "role": "assistant", "content": "already said" },
                { "role": "user", "content": "" },
            ],
        });
        let run = unwrap_run(&run_frame(&body, "id"));
        let user_message = run
            .nested(2)
            .and_then(|action| action.nested(1))
            .and_then(|user| user.nested(1))
            .expect("user_message");
        assert_eq!(user_message.text(1).as_deref(), Some("Continue."));
    }

    #[test]
    fn the_context_ack_is_an_empty_request_context_on_exec_client_message() {
        let bytes = context_ack();
        let payload = bytes.get(5..).expect("framed");
        let client = Message::decode(payload);
        let exec = client.nested(2).expect("exec_client_message");
        assert!(exec.nested(10).is_some(), "request_context_result");
    }

    #[test]
    fn a_text_delta_and_a_context_ask_decode_from_the_server_frames() {
        let mut stream = encode_request_context();
        stream.extend(encode_text_delta("hello "));
        stream.extend(encode_text_delta("world"));
        stream.extend(encode_turn_ended());
        let (events, consumed) = decode_stream(&stream);
        assert_eq!(consumed, stream.len());
        assert_eq!(
            events,
            vec![
                Event::RequestContext,
                Event::Text("hello ".to_owned()),
                Event::Text("world".to_owned()),
                Event::Done,
            ]
        );
    }

    #[test]
    fn an_editor_tool_is_named_unsupported_rather_than_narrated() {
        // Field 2 (exec_request) without field 10 is a shell/read/write the router cannot service.
        let exec = bytes_field(1, b"not-context");
        let payload = bytes_field(2, &exec);
        assert_eq!(decode_payload(&payload), vec![Event::UnsupportedExec]);
    }

    /// A loopback duplex: the "server" asks for context, waits until the ack arrives on the same
    /// stream, *then* writes the answer. That is the load-bearing order of `AgentService` — answering
    /// after the text would be a `ChatService` conversation, not this one.
    #[tokio::test]
    async fn a_loopback_duplex_answers_the_context_ask_before_the_text_continues() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let (mut client, mut server) = tokio::io::duplex(8 * 1024);
        let body = json!({
            "model": "claude-4.5-sonnet",
            "messages": [{ "role": "user", "content": "say hi" }],
        });
        let run = run_frame(&body, "loopback-id");

        let server_task = tokio::spawn(async move {
            // Read the run frame (5-byte header + payload).
            let mut header = [0_u8; 5];
            server.read_exact(&mut header).await.expect("run header");
            let length = usize::try_from(u32::from_be_bytes([
                header[1], header[2], header[3], header[4],
            ]))
            .expect("frame length");
            let mut payload = vec![0_u8; length];
            server.read_exact(&mut payload).await.expect("run payload");
            let run = Message::decode(&payload).nested(1).expect("run_request");
            assert_eq!(
                run.nested(2)
                    .and_then(|action| action.nested(1))
                    .and_then(|user| user.nested(1))
                    .and_then(|message| message.text(1))
                    .as_deref(),
                Some("say hi")
            );

            // Ask for context *before* producing any text.
            server
                .write_all(&encode_request_context())
                .await
                .expect("ask");

            // Block until the empty context lands on this same stream.
            let mut ack_header = [0_u8; 5];
            server
                .read_exact(&mut ack_header)
                .await
                .expect("ack header");
            let ack_len = usize::try_from(u32::from_be_bytes([
                ack_header[1],
                ack_header[2],
                ack_header[3],
                ack_header[4],
            ]))
            .expect("ack length");
            let mut ack_payload = vec![0_u8; ack_len];
            server.read_exact(&mut ack_payload).await.expect("ack body");
            let ack = Message::decode(&ack_payload);
            assert!(
                ack.nested(2).and_then(|exec| exec.nested(10)).is_some(),
                "the ack must be request_context_result"
            );

            server
                .write_all(&encode_text_delta("hello from agent"))
                .await
                .expect("text");
            server.write_all(&encode_turn_ended()).await.expect("done");
        });

        client.write_all(&run).await.expect("send run");

        let mut incoming = Vec::new();
        let mut saw_text = false;
        let mut answered_context = false;
        // Read until Done. Bound the loop so a protocol bug cannot hang the suite.
        for _ in 0..32 {
            let mut buf = [0_u8; 1024];
            let n = client.read(&mut buf).await.expect("read");
            if n == 0 {
                break;
            }
            incoming.extend_from_slice(&buf[..n]);
            let (events, consumed) = decode_stream(&incoming);
            incoming.drain(..consumed);
            for event in events {
                match event {
                    Event::RequestContext => {
                        client.write_all(&context_ack()).await.expect("ack");
                        answered_context = true;
                    }
                    Event::Text(text) => {
                        assert!(
                            answered_context,
                            "text arrived before the context ask was answered: {text}"
                        );
                        assert_eq!(text, "hello from agent");
                        saw_text = true;
                    }
                    Event::Done => {
                        server_task.await.expect("server");
                        assert!(saw_text, "the turn ended with no assistant text");
                        assert!(answered_context);
                        return;
                    }
                    Event::UnsupportedExec => panic!("loopback must not request an IDE tool"),
                }
            }
        }
        panic!("the duplex exchange never reached Done");
    }
}
