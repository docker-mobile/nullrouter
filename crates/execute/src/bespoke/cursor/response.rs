//! Turning Cursor's protobuf response frames into OpenAI chat-completion chunks.
//!
//! Ports `transformProtobufToSSE` and the response half of `cursorProtobuf.js`. The decoded frames come out
//! as OpenAI-shaped chunks so the existing translators can carry them on to whatever the client asked for;
//! nothing here knows about Claude or Gemini.
//!
//! Cursor streams deltas rather than whole answers, so unlike perplexity no high-water mark is needed.
//! What it does need is tool-call accumulation: a call's arguments arrive across several frames under the
//! same id, and each fragment has to be emitted against the index that id was first assigned.

use serde_json::{Value, json};

use super::protobuf::Message;

/// Field numbers in `StreamUnifiedChatResponseWithTools`.
mod envelope {
    pub(super) const TOOL_CALL: u32 = 1;
    pub(super) const BODY: u32 = 2;
}

/// Field numbers in `ClientSideToolV2Call`.
mod call {
    pub(super) const ID: u32 = 3;
    pub(super) const NAME: u32 = 9;
    pub(super) const RAW_ARGS: u32 = 10;
    pub(super) const MCP_PARAMS: u32 = 27;
}

/// Field numbers in `StreamUnifiedChatResponse`.
mod body {
    pub(super) const TEXT: u32 = 1;
    pub(super) const THINKING: u32 = 25;
}

/// `MCPParams.tools`, and the nested tool's name and params.
const MCP_TOOLS_LIST: u32 = 1;
const MCP_NESTED_NAME: u32 = 1;
const MCP_NESTED_PARAMS: u32 = 3;

/// `Thinking.text`.
const THINKING_TEXT: u32 = 1;

/// What one frame carried.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Decoded {
    /// Answer text.
    pub(crate) text: Option<String>,
    /// Reasoning text.
    pub(crate) thinking: Option<String>,
    /// A tool call, or a fragment of one.
    pub(crate) tool_call: Option<ToolCall>,
}

/// A tool call as Cursor reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolCall {
    /// The call's id, stable across the fragments that make it up.
    pub(crate) id: String,
    /// The function's name.
    pub(crate) name: String,
    /// This fragment of the arguments.
    pub(crate) arguments: String,
}

/// Decode one frame's payload.
pub(crate) fn decode_frame(payload: &[u8]) -> Decoded {
    let fields = Message::decode(payload);

    if let Some(call) = fields
        .nested(envelope::TOOL_CALL)
        .and_then(|call| decode_tool_call(&call))
    {
        return Decoded {
            tool_call: Some(call),
            ..Decoded::default()
        };
    }

    if let Some(inner) = fields.nested(envelope::BODY) {
        let text = inner.text(body::TEXT).filter(|text| !text.is_empty());
        let thinking = inner
            .nested(body::THINKING)
            .and_then(|thinking| thinking.text(THINKING_TEXT))
            .filter(|text| !text.is_empty());
        return Decoded {
            text,
            thinking,
            tool_call: None,
        };
    }

    Decoded::default()
}

fn decode_tool_call(call: &Message) -> Option<ToolCall> {
    let id = call.text(call::ID).unwrap_or_default();
    // The name and arguments arrive either flat or nested under MCP params, depending on how the tool was
    // declared. Both spellings are read, flat first.
    let mut name = call.text(call::NAME).unwrap_or_default();
    let mut arguments = call.text(call::RAW_ARGS).unwrap_or_default();

    if let Some(nested) = call
        .nested(call::MCP_PARAMS)
        .and_then(|params| params.nested(MCP_TOOLS_LIST))
    {
        if name.is_empty() {
            name = nested.text(MCP_NESTED_NAME).unwrap_or_default();
        }
        if arguments.is_empty() {
            arguments = nested.text(MCP_NESTED_PARAMS).unwrap_or_default();
        }
    }

    // An id with no name is a continuation fragment carrying only arguments; upstream requires both before
    // it reports a call, and so does this.
    (!id.is_empty() && !name.is_empty()).then_some(ToolCall {
        id,
        name,
        arguments,
    })
}

/// A JSON error body carried in a frame, if that is what this frame is.
///
/// Cursor reports rate limits and rejections as a JSON frame in the middle of a protobuf stream.
pub(crate) fn error_frame(payload: &[u8]) -> Option<Value> {
    if payload.first() != Some(&b'{') {
        return None;
    }
    let text = std::str::from_utf8(payload).ok()?;
    if !text.contains("\"error\"") {
        return None;
    }
    serde_json::from_str(text).ok()
}

/// The message and kind of a Cursor error body.
///
/// Upstream digs through `details[0].debug.details` first: the useful sentence is there, and
/// `error.message` is often just the status name.
pub(crate) fn error_detail(error: &Value) -> (String, bool) {
    let debug = error.pointer("/error/details/0/debug/details");
    let message = debug
        .and_then(|details| details.get("title"))
        .and_then(Value::as_str)
        .or_else(|| {
            debug
                .and_then(|details| details.get("detail"))
                .and_then(Value::as_str)
        })
        .or_else(|| error.pointer("/error/message").and_then(Value::as_str))
        .unwrap_or("API Error");
    let rate_limited =
        error.pointer("/error/code").and_then(Value::as_str) == Some("resource_exhausted");
    (message.to_owned(), rate_limited)
}

/// Accumulates decoded frames into OpenAI chunks.
#[derive(Debug, Clone)]
pub(crate) struct Stream {
    id: String,
    created: u64,
    model: String,
    /// Tool call ids in the order first seen; position is the OpenAI `index`.
    tool_ids: Vec<String>,
    /// Whether a role has been announced yet.
    opened: bool,
    /// Accumulated thinking text, for composer models whose answer is embedded in it.
    thinking: String,
    /// How much of a composer model's visible answer has already been emitted.
    emitted_visible: usize,
    /// Whether this model hides its answer inside the thinking stream.
    composer: bool,
}

impl Stream {
    /// Start a stream for one response.
    pub(crate) fn new(id: String, created: u64, model: String) -> Self {
        let composer = is_composer(&model);
        Self {
            id,
            created,
            model,
            tool_ids: Vec::new(),
            opened: false,
            thinking: String::new(),
            emitted_visible: 0,
            composer,
        }
    }

    /// Whether any content has been produced, which decides whether a late error frame is fatal.
    pub(crate) const fn has_content(&self) -> bool {
        self.opened || !self.tool_ids.is_empty()
    }

    /// The chunks one frame produces.
    pub(crate) fn push(&mut self, decoded: &Decoded) -> Vec<Value> {
        let mut chunks = Vec::new();

        if let Some(call) = decoded.tool_call.as_ref() {
            // A tool call opens the message with an empty assistant delta, as upstream does, so a client
            // sees the role before the call.
            if !self.opened {
                self.opened = true;
                chunks.push(self.chunk(&json!({ "role": "assistant", "content": "" })));
            }
            let index = if let Some(index) = self.tool_ids.iter().position(|id| *id == call.id) {
                index
            } else {
                self.tool_ids.push(call.id.clone());
                self.tool_ids.len().saturating_sub(1)
            };
            chunks.push(self.chunk(&json!({
                "tool_calls": [{
                    "index": index,
                    "id": call.id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": call.arguments },
                }],
            })));
        }

        if let Some(text) = decoded.text.as_ref() {
            chunks.push(self.content_delta(text));
        }

        // A composer model puts its answer *inside* the thinking stream, after a `</think>` marker. The
        // text before that marker is reasoning; what follows is the answer, and it is not repeated in a
        // text field. Without this, a composer model returns an empty reply.
        if self.composer
            && let Some(thinking) = decoded.thinking.as_ref()
        {
            self.thinking.push_str(thinking);
            let visible = visible_after_think(&self.thinking);
            if visible.len() > self.emitted_visible {
                let delta = visible
                    .get(self.emitted_visible..)
                    .unwrap_or_default()
                    .to_owned();
                self.emitted_visible = visible.len();
                if !delta.is_empty() {
                    chunks.push(self.content_delta(&delta));
                }
            }
        } else if let Some(thinking) = decoded.thinking.as_ref().filter(|_| !self.composer) {
            // Every other model reports reasoning separately, so it is passed through as such.
            let opened = self.opened;
            self.opened = true;
            let delta = if opened {
                json!({ "reasoning_content": thinking })
            } else {
                json!({ "role": "assistant", "reasoning_content": thinking })
            };
            chunks.push(self.chunk(&delta));
        }

        chunks
    }

    fn content_delta(&mut self, text: &str) -> Value {
        let opened = self.opened;
        self.opened = true;
        if opened {
            self.chunk(&json!({ "content": text }))
        } else {
            self.chunk(&json!({ "role": "assistant", "content": text }))
        }
    }

    /// The terminal chunk.
    ///
    /// No usage is reported. Upstream estimates it from the answer's character count; a made-up number is
    /// worse than an absent one, since a client cannot tell it was estimated.
    pub(crate) fn finish(&mut self) -> Vec<Value> {
        let mut chunks = Vec::new();
        // A response that produced nothing still needs a message, or a client sees a stream that opened and
        // closed with no assistant turn at all.
        if !self.has_content() {
            self.opened = true;
            chunks.push(self.chunk(&json!({ "role": "assistant", "content": "" })));
        }
        let reason = if self.tool_ids.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        chunks.push(json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }],
        }));
        chunks
    }

    fn chunk(&self, delta: &Value) -> Value {
        json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": Value::Null }],
        })
    }
}

/// Whether a model embeds its answer in the thinking stream.
fn is_composer(model: &str) -> bool {
    let tail = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    tail == "composer" || tail.starts_with("composer-")
}

/// The visible answer inside a composer model's thinking text: everything after the last `</think>`.
fn visible_after_think(thinking: &str) -> &str {
    const END: &str = "</think>";
    thinking
        .rfind(END)
        .and_then(|index| thinking.get(index.saturating_add(END.len())..))
        .map_or("", str::trim_start)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::protobuf::{Frame, bytes_field, frame, frames, put_bytes, put_str};
    use super::{
        Decoded, Stream, ToolCall, decode_frame, error_detail, error_frame, visible_after_think,
    };

    /// A response frame carrying answer text.
    fn text_payload(text: &str) -> Vec<u8> {
        let mut inner = Vec::new();
        put_str(&mut inner, 1, text);
        bytes_field(2, &inner)
    }

    /// A response frame carrying reasoning text.
    fn thinking_payload(text: &str) -> Vec<u8> {
        let mut thinking = Vec::new();
        put_str(&mut thinking, 1, text);
        let mut inner = Vec::new();
        put_bytes(&mut inner, 25, &thinking);
        bytes_field(2, &inner)
    }

    /// A response frame carrying a tool call.
    fn tool_payload(id: &str, name: &str, arguments: &str) -> Vec<u8> {
        let mut call = Vec::new();
        if !id.is_empty() {
            put_str(&mut call, 3, id);
        }
        if !name.is_empty() {
            put_str(&mut call, 9, name);
        }
        if !arguments.is_empty() {
            put_str(&mut call, 10, arguments);
        }
        bytes_field(1, &call)
    }

    fn stream() -> Stream {
        Stream::new(
            "chatcmpl-1".to_owned(),
            1_700_000_000,
            "cursor-model".to_owned(),
        )
    }

    fn deltas(chunks: &[Value]) -> Vec<Value> {
        chunks
            .iter()
            .filter_map(|chunk| chunk.pointer("/choices/0/delta").cloned())
            .collect()
    }

    #[test]
    fn a_text_frame_decodes_to_its_text() {
        assert_eq!(
            decode_frame(&text_payload("hello")),
            Decoded {
                text: Some("hello".to_owned()),
                ..Decoded::default()
            }
        );
    }

    #[test]
    fn a_thinking_frame_decodes_to_reasoning() {
        assert_eq!(
            decode_frame(&thinking_payload("let me think")),
            Decoded {
                thinking: Some("let me think".to_owned()),
                ..Decoded::default()
            }
        );
    }

    #[test]
    fn a_tool_frame_decodes_to_a_call() {
        assert_eq!(
            decode_frame(&tool_payload("call_1", "read_file", r#"{"path":"a"}"#)).tool_call,
            Some(ToolCall {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                arguments: r#"{"path":"a"}"#.to_owned(),
            })
        );
    }

    #[test]
    fn a_tool_name_nested_under_mcp_params_is_still_found() {
        // Depending on how a tool was declared, the name and arguments arrive flat or nested. Reading only
        // the flat spelling loses every call from the other kind.
        let mut nested = Vec::new();
        put_str(&mut nested, 1, "mcp_tool");
        put_str(&mut nested, 3, r#"{"q":1}"#);
        let params = bytes_field(1, &nested);
        let mut call = Vec::new();
        put_str(&mut call, 3, "call_2");
        put_bytes(&mut call, 27, &params);

        assert_eq!(
            decode_frame(&bytes_field(1, &call)).tool_call,
            Some(ToolCall {
                id: "call_2".to_owned(),
                name: "mcp_tool".to_owned(),
                arguments: r#"{"q":1}"#.to_owned(),
            })
        );
    }

    #[test]
    fn an_id_with_no_name_is_not_reported_as_a_call() {
        // That frame is a continuation carrying only arguments; reporting it as a call would invent a
        // nameless function.
        assert!(
            decode_frame(&tool_payload("call_1", "", "{\"a\":1}"))
                .tool_call
                .is_none()
        );
    }

    #[test]
    fn text_deltas_stream_with_the_role_announced_once() {
        let mut stream = stream();
        let first = stream.push(&decode_frame(&text_payload("Hel")));
        let second = stream.push(&decode_frame(&text_payload("lo")));
        assert_eq!(
            deltas(&first),
            vec![json!({ "role": "assistant", "content": "Hel" })]
        );
        // The role is announced once; the rest are bare content deltas.
        assert_eq!(deltas(&second), vec![json!({ "content": "lo" })]);
    }

    #[test]
    fn a_tool_calls_fragments_accumulate_against_one_index() {
        // Arguments arrive across frames under the same id. Each fragment must carry the index that id was
        // first assigned, or a client cannot reassemble them.
        let mut stream = stream();
        let opening = stream.push(&decode_frame(&tool_payload("call_1", "read_file", "{\"pa")));
        let continuing = stream.push(&decode_frame(&tool_payload(
            "call_1",
            "read_file",
            "th\":\"a\"}",
        )));
        let second_call = stream.push(&decode_frame(&tool_payload("call_2", "write_file", "{}")));

        // A tool call opens the message with an empty assistant delta first.
        assert_eq!(
            deltas(&opening).first(),
            Some(&json!({ "role": "assistant", "content": "" }))
        );
        let index_of = |chunks: &[Value]| {
            deltas(chunks)
                .iter()
                .find_map(|delta| delta.pointer("/tool_calls/0/index").and_then(Value::as_u64))
        };
        assert_eq!(index_of(&opening), Some(0));
        assert_eq!(
            index_of(&continuing),
            Some(0),
            "the same id keeps its index"
        );
        assert_eq!(
            index_of(&second_call),
            Some(1),
            "a new id gets the next one"
        );
    }

    #[test]
    fn reasoning_is_passed_through_separately_for_a_normal_model() {
        let mut stream = stream();
        let chunks = stream.push(&decode_frame(&thinking_payload("thinking out loud")));
        assert_eq!(
            deltas(&chunks),
            vec![json!({ "role": "assistant", "reasoning_content": "thinking out loud" })]
        );
    }

    #[test]
    fn a_composer_models_answer_is_extracted_from_its_thinking_stream() {
        // A composer model puts the answer *inside* the thinking text, after `</think>`, and does not
        // repeat it in a text field. Treating that as reasoning returns an empty reply.
        let mut stream = Stream::new("id".to_owned(), 1, "composer-1".to_owned());
        let first = stream.push(&decode_frame(&thinking_payload("reasoning...</think>The ")));
        let second = stream.push(&decode_frame(&thinking_payload("answer.")));

        assert_eq!(
            deltas(&first),
            vec![json!({ "role": "assistant", "content": "The " })]
        );
        assert_eq!(deltas(&second), vec![json!({ "content": "answer." })]);
        // And the reasoning before the marker is not emitted as content.
        assert!(
            !deltas(&first)
                .iter()
                .any(|delta| delta.to_string().contains("reasoning...")),
        );
    }

    #[test]
    fn the_visible_part_of_a_composer_stream_is_what_follows_the_last_marker() {
        assert_eq!(visible_after_think("a</think>b"), "b");
        // The last marker wins, and leading whitespace after it is trimmed.
        assert_eq!(visible_after_think("a</think>b</think>  c"), "c");
        // No marker yet means nothing is visible.
        assert_eq!(visible_after_think("still thinking"), "");
    }

    #[test]
    fn a_finished_stream_reports_why_it_stopped() {
        let mut text_only = stream();
        let _unused = text_only.push(&decode_frame(&text_payload("hi")));
        let finished = text_only.finish();
        assert_eq!(
            finished.last().and_then(|chunk| chunk
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)),
            Some("stop")
        );

        let mut with_tools = stream();
        let _unused = with_tools.push(&decode_frame(&tool_payload("c", "f", "{}")));
        assert_eq!(
            with_tools.finish().last().and_then(|chunk| chunk
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)),
            Some("tool_calls")
        );
    }

    #[test]
    fn no_usage_is_reported_because_none_was_measured() {
        // Upstream estimates it from the answer's length. A client cannot tell an estimate from a count, so
        // an absent number is more honest than a made-up one.
        let mut stream = stream();
        let _unused = stream.push(&decode_frame(&text_payload("hi")));
        assert!(
            stream
                .finish()
                .iter()
                .all(|chunk| chunk.get("usage").is_none())
        );
    }

    #[test]
    fn an_empty_response_still_produces_an_assistant_message() {
        // Otherwise a client sees a stream that opened and closed with no turn in it.
        let mut stream = stream();
        let chunks = stream.finish();
        assert_eq!(
            deltas(&chunks).first(),
            Some(&json!({ "role": "assistant", "content": "" }))
        );
    }

    #[test]
    fn a_json_error_frame_is_recognised_and_read_for_its_detail() {
        let body = br#"{"error":{"code":"resource_exhausted","message":"slow down","details":[{"debug":{"details":{"title":"Rate limit reached"}}}]}}"#;
        let error = error_frame(body).expect("an error body");
        let (message, rate_limited) = error_detail(&error);
        // The useful sentence is in `details[0].debug.details`, not `error.message`.
        assert_eq!(message, "Rate limit reached");
        assert!(rate_limited);

        // Falls back to `error.message` when the debug block has nothing.
        let plain = error_frame(br#"{"error":{"message":"bad request"}}"#).expect("an error body");
        assert_eq!(error_detail(&plain), ("bad request".to_owned(), false));

        // A protobuf frame is not mistaken for one.
        assert!(error_frame(&text_payload("hi")).is_none());
    }

    #[test]
    fn whether_an_error_is_fatal_depends_on_what_came_before_it() {
        // The precedence the stream pump relies on: before any content, the error *is* the response;
        // after content, the text already delivered is real and turning it into an error discards it.
        // `has_content` is what decides, so it is asserted directly.
        let mut fresh = stream();
        assert!(!fresh.has_content(), "nothing decoded yet");

        let _unused = fresh.push(&decode_frame(&text_payload("here is the answer")));
        assert!(fresh.has_content(), "text counts as content");

        // A tool call counts too, even though it produces no text.
        let mut with_tool = stream();
        let _unused = with_tool.push(&decode_frame(&tool_payload("c", "f", "{}")));
        assert!(with_tool.has_content());

        // And the error body is read the same way in both cases.
        let error = error_frame(br#"{"error":{"message":"nope"}}"#).expect("an error body");
        assert_eq!(error_detail(&error).0, "nope");
    }

    #[test]
    fn a_trailer_frame_is_told_apart_from_a_message() {
        // A trailer carries grpc status, not content, and contributes nothing to the answer.
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&frame(&text_payload("hi")));
        let mut trailer = vec![0x02];
        let status = b"grpc-status: 0";
        trailer.extend_from_slice(&u32::try_from(status.len()).expect("fits").to_be_bytes());
        trailer.extend_from_slice(status);
        buffer.extend_from_slice(&trailer);

        let (read, _consumed) = frames(&buffer);
        assert_eq!(read.len(), 2);
        assert_eq!(read.first().map(Frame::is_trailer), Some(false));
        assert_eq!(read.get(1).map(Frame::is_trailer), Some(true));
        // The message frame decodes; nothing asks the trailer to.
        assert_eq!(
            read.first().map(|frame| decode_frame(&frame.payload).text),
            Some(Some("hi".to_owned()))
        );
    }
}
