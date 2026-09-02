//! Turning `kiro`'s event-stream frames into OpenAI chat-completion chunks.
//!
//! Ports `open-sse/translator/response/kiro-to-openai.js` and the event handling in `executors/kiro.js`.
//! The frames come off [`super::eventstream`]; this decides what each event means.
//!
//! Two things are worth stating. Kiro reports **no finish reason of its own** — `messageStopEvent` says the
//! turn ended and nothing more, so the reason is derived from whether a tool was used. And a `toolUseEvent`
//! arrives in *fragments*: the input is a JSON string built across several events under one id, so a
//! fragment cannot be parsed on its own and the call is only complete when the event says `stop`.

use serde_json::{Value, json};

use super::eventstream::Frame;

/// The event types this decoder acts on.
///
/// Others (`metadataEvent`, `meteringEvent`, `metricsEvent`, `contextUsageEvent`) are recognised as
/// legitimate and ignored: they carry accounting rather than content.
const CONTENT_EVENTS: [&str; 4] = [
    "assistantResponseEvent",
    "reasoningContentEvent",
    "codeEvent",
    "toolUseEvent",
];

/// What one frame carried.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Decoded {
    /// Answer text.
    pub(crate) text: Option<String>,
    /// Reasoning text.
    pub(crate) reasoning: Option<String>,
    /// A tool call, or a fragment of one.
    pub(crate) tool: Option<ToolFragment>,
    /// The turn ended.
    pub(crate) stopped: bool,
    /// An upstream exception, which arrives in-band rather than as a status code.
    pub(crate) exception: Option<String>,
}

/// A tool call as Kiro reports it, which may be one fragment of several.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolFragment {
    /// The call's id, stable across its fragments.
    pub(crate) id: String,
    /// The function's name. Present on the first fragment.
    pub(crate) name: String,
    /// This fragment of the input, as JSON text.
    pub(crate) input: String,
    /// Whether this fragment completes the call.
    pub(crate) last: bool,
}

/// Decode one frame.
pub(crate) fn decode(frame: &Frame) -> Decoded {
    // An exception is reported in-band, after the response headers already said 200.
    if frame.message_type() == Some("exception") {
        let detail = frame
            .payload
            .as_ref()
            .and_then(|payload| payload.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("upstream exception");
        let name = frame.exception_type().unwrap_or("Exception");
        return Decoded {
            exception: Some(format!("{name}: {detail}")),
            ..Decoded::default()
        };
    }

    let Some(payload) = frame.payload.as_ref() else {
        // A payload-less frame is only meaningful when its header says the turn stopped.
        return Decoded {
            stopped: frame.event_type() == Some("messageStopEvent"),
            ..Decoded::default()
        };
    };

    // The event type is in the header, but a payload that names itself is honoured too — upstream reads
    // both, and the CodeWhisperer surface has been seen to send only the nested form.
    let event = frame
        .event_type()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            CONTENT_EVENTS
                .iter()
                .chain(std::iter::once(&"messageStopEvent"))
                .find(|name| payload.get(*name).is_some())
                .map(|name| (*name).to_owned())
        })
        .unwrap_or_default();
    // When the payload nests the event under its own name, read from there.
    let inner = payload.get(event.as_str()).unwrap_or(payload);

    match event.as_str() {
        "assistantResponseEvent" | "codeEvent" => Decoded {
            text: inner
                .get("content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            ..Decoded::default()
        },
        "reasoningContentEvent" => Decoded {
            reasoning: inner
                .get("text")
                .or_else(|| inner.get("content"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            ..Decoded::default()
        },
        "toolUseEvent" => Decoded {
            tool: Some(ToolFragment {
                id: inner
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                name: inner
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                // The input arrives as a JSON *string* across several events. An object here is a
                // whole-call event rather than a fragment, so it is re-serialised to the same shape.
                input: match inner.get("input") {
                    Some(Value::String(text)) => text.clone(),
                    Some(value) => value.to_string(),
                    None => String::new(),
                },
                last: inner.get("stop").and_then(Value::as_bool) == Some(true),
            }),
            ..Decoded::default()
        },
        "messageStopEvent" => Decoded {
            stopped: true,
            ..Decoded::default()
        },
        // Accounting events and anything unrecognised. Kiro adds event types without notice, so an unknown
        // one is ignored rather than treated as an error.
        _ignored => Decoded::default(),
    }
}

/// Accumulates decoded frames into OpenAI chunks.
#[derive(Debug, Clone)]
pub(crate) struct Stream {
    id: String,
    created: u64,
    model: String,
    /// Whether a role has been announced.
    opened: bool,
    /// Tool call ids in first-seen order; position is the OpenAI `index`.
    tool_ids: Vec<String>,
    /// Whether any tool was used, which is the only thing the finish reason can be derived from.
    used_tool: bool,
    /// Whether the terminal chunk has been emitted.
    finished: bool,
}

impl Stream {
    /// Start a stream for one response.
    pub(crate) const fn new(id: String, created: u64, model: String) -> Self {
        Self {
            id,
            created,
            model,
            opened: false,
            tool_ids: Vec::new(),
            used_tool: false,
            finished: false,
        }
    }

    /// Whether any content has been produced.
    pub(crate) const fn has_content(&self) -> bool {
        self.opened
    }

    /// The chunks one decoded frame produces.
    pub(crate) fn push(&mut self, decoded: &Decoded) -> Vec<Value> {
        let mut chunks = Vec::new();

        if let Some(text) = decoded.text.as_ref() {
            chunks.push(self.delta(json!({ "content": text })));
        }
        if let Some(reasoning) = decoded.reasoning.as_ref() {
            chunks.push(self.delta(json!({ "reasoning_content": reasoning })));
        }
        if let Some(fragment) = decoded.tool.as_ref() {
            self.used_tool = true;
            let index = if let Some(index) = self.tool_ids.iter().position(|id| *id == fragment.id)
            {
                index
            } else {
                self.tool_ids.push(fragment.id.clone());
                self.tool_ids.len().saturating_sub(1)
            };
            // Each fragment is relayed as an argument delta against the index its id holds. A client
            // concatenates them; parsing a fragment here would fail, since half a JSON document is not one.
            chunks.push(self.delta(json!({
                "tool_calls": [{
                    "index": index,
                    "id": fragment.id,
                    "type": "function",
                    "function": { "name": fragment.name, "arguments": fragment.input },
                }],
            })));
        }
        if decoded.stopped {
            chunks.extend(self.finish());
        }

        chunks
    }

    /// The terminal chunk.
    ///
    /// Kiro sends no finish reason, so it is derived: `tool_calls` when a tool was used this turn and
    /// `stop` otherwise. No usage is reported — Kiro's metering events are billing records rather than
    /// token counts for this request, and inventing a number a client cannot distinguish from a measured
    /// one is worse than omitting it.
    pub(crate) fn finish(&mut self) -> Vec<Value> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut chunks = Vec::new();
        if !self.opened {
            // A turn that produced nothing still needs a message, or a client sees a stream that opened and
            // closed with no assistant turn in it.
            chunks.push(self.delta(json!({ "content": "" })));
        }
        let reason = if self.used_tool { "tool_calls" } else { "stop" };
        chunks.push(json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }],
        }));
        chunks
    }

    /// One delta chunk, announcing the role on the first.
    fn delta(&mut self, mut delta: Value) -> Value {
        if !self.opened {
            self.opened = true;
            if let Some(object) = delta.as_object_mut() {
                object.insert("role".to_owned(), json!("assistant"));
            }
        }
        json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": Value::Null }],
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::eventstream::{Frame, HeaderValue};
    use super::{Decoded, Stream, ToolFragment, decode};

    fn frame(event: &str, payload: Option<Value>) -> Frame {
        Frame {
            headers: vec![
                (
                    ":event-type".to_owned(),
                    HeaderValue::Text(event.to_owned()),
                ),
                (
                    ":message-type".to_owned(),
                    HeaderValue::Text("event".to_owned()),
                ),
            ],
            payload,
        }
    }

    fn stream() -> Stream {
        Stream::new(
            "chatcmpl-1".to_owned(),
            1_700_000_000,
            "kiro-model".to_owned(),
        )
    }

    fn deltas(chunks: &[Value]) -> Vec<Value> {
        chunks
            .iter()
            .filter_map(|chunk| chunk.pointer("/choices/0/delta").cloned())
            .collect()
    }

    #[test]
    fn an_assistant_response_event_decodes_to_text() {
        assert_eq!(
            decode(&frame(
                "assistantResponseEvent",
                Some(json!({ "content": "hello" }))
            )),
            Decoded {
                text: Some("hello".to_owned()),
                ..Decoded::default()
            }
        );
    }

    #[test]
    fn a_code_event_is_text_too() {
        // Kiro sends code in its own event type, but it is part of the same answer.
        assert_eq!(
            decode(&frame(
                "codeEvent",
                Some(json!({ "content": "fn main() {}" }))
            ))
            .text,
            Some("fn main() {}".to_owned())
        );
    }

    #[test]
    fn a_reasoning_event_decodes_to_reasoning_and_reads_either_field() {
        assert_eq!(
            decode(&frame(
                "reasoningContentEvent",
                Some(json!({ "text": "thinking" }))
            ))
            .reasoning,
            Some("thinking".to_owned())
        );
        // The same event has been seen with `content` instead.
        assert_eq!(
            decode(&frame(
                "reasoningContentEvent",
                Some(json!({ "content": "also thinking" }))
            ))
            .reasoning,
            Some("also thinking".to_owned())
        );
    }

    #[test]
    fn an_event_named_only_in_its_payload_is_still_recognised() {
        // The CodeWhisperer surface has been seen to send the event nested under its own name with no
        // `:event-type` header. Reading only the header would silently drop the whole answer.
        let nested = Frame {
            headers: vec![(
                ":message-type".to_owned(),
                HeaderValue::Text("event".to_owned()),
            )],
            payload: Some(json!({ "assistantResponseEvent": { "content": "nested" } })),
        };
        assert_eq!(decode(&nested).text, Some("nested".to_owned()));
    }

    #[test]
    fn a_tool_use_event_decodes_to_a_fragment() {
        assert_eq!(
            decode(&frame(
                "toolUseEvent",
                Some(json!({
                    "toolUseId": "tool_1",
                    "name": "read_file",
                    "input": "{\"path\":",
                    "stop": false,
                }))
            ))
            .tool,
            Some(ToolFragment {
                id: "tool_1".to_owned(),
                name: "read_file".to_owned(),
                input: "{\"path\":".to_owned(),
                last: false,
            })
        );
    }

    #[test]
    fn a_whole_call_event_with_an_object_input_is_serialised_to_the_same_shape() {
        // Some events carry the input as an object rather than a string fragment. A client expects
        // `arguments` to be text either way.
        let decoded = decode(&frame(
            "toolUseEvent",
            Some(json!({
                "toolUseId": "tool_2",
                "name": "write_file",
                "input": { "path": "a.txt" },
                "stop": true,
            })),
        ))
        .tool
        .expect("a fragment");
        assert_eq!(decoded.input, r#"{"path":"a.txt"}"#);
        assert!(decoded.last);
    }

    #[test]
    fn a_message_stop_event_ends_the_turn() {
        assert!(decode(&frame("messageStopEvent", Some(json!({})))).stopped);
        // And with no payload at all, which is how it usually arrives.
        assert!(decode(&frame("messageStopEvent", None)).stopped);
    }

    #[test]
    fn accounting_events_are_ignored_rather_than_treated_as_errors() {
        // Kiro adds event types without notice, so an unrecognised one must not break the stream.
        for event in [
            "metadataEvent",
            "meteringEvent",
            "metricsEvent",
            "contextUsageEvent",
            "somethingKiroAddedLastWeek",
        ] {
            assert_eq!(
                decode(&frame(event, Some(json!({ "whatever": 1 })))),
                Decoded::default(),
                "{event} should be ignored"
            );
        }
    }

    #[test]
    fn an_exception_frame_is_surfaced_with_its_name_and_message() {
        // It arrives in-band, after the response headers already said 200, so it is the one failure that
        // cannot become a status code.
        let exception = Frame {
            headers: vec![
                (
                    ":message-type".to_owned(),
                    HeaderValue::Text("exception".to_owned()),
                ),
                (
                    ":exception-type".to_owned(),
                    HeaderValue::Text("ThrottlingException".to_owned()),
                ),
            ],
            payload: Some(json!({ "message": "Too many requests" })),
        };
        assert_eq!(
            decode(&exception).exception.as_deref(),
            Some("ThrottlingException: Too many requests")
        );
    }

    #[test]
    fn text_deltas_announce_the_role_once() {
        let mut stream = stream();
        let first = stream.push(&Decoded {
            text: Some("Hel".to_owned()),
            ..Decoded::default()
        });
        let second = stream.push(&Decoded {
            text: Some("lo".to_owned()),
            ..Decoded::default()
        });
        assert_eq!(
            deltas(&first),
            vec![json!({ "content": "Hel", "role": "assistant" })]
        );
        assert_eq!(deltas(&second), vec![json!({ "content": "lo" })]);
    }

    #[test]
    fn tool_fragments_accumulate_against_one_index() {
        // The input is a JSON document split across events. A fragment cannot be parsed on its own, so each
        // is relayed as an argument delta against the index its id holds.
        let mut stream = stream();
        let fragment = |input: &str, last: bool| Decoded {
            tool: Some(ToolFragment {
                id: "tool_1".to_owned(),
                name: "read_file".to_owned(),
                input: input.to_owned(),
                last,
            }),
            ..Decoded::default()
        };
        let first = stream.push(&fragment("{\"pa", false));
        let second = stream.push(&fragment("th\":\"a\"}", true));
        let index_of = |chunks: &[Value]| {
            deltas(chunks)
                .iter()
                .find_map(|delta| delta.pointer("/tool_calls/0/index").and_then(Value::as_u64))
        };
        assert_eq!(index_of(&first), Some(0));
        assert_eq!(index_of(&second), Some(0), "one id keeps one index");
        // The fragments are relayed verbatim for the client to concatenate.
        assert_eq!(
            deltas(&second)
                .first()
                .and_then(|delta| delta.pointer("/tool_calls/0/function/arguments").cloned()),
            Some(json!("th\":\"a\"}"))
        );

        let other = stream.push(&Decoded {
            tool: Some(ToolFragment {
                id: "tool_2".to_owned(),
                name: "write_file".to_owned(),
                input: "{}".to_owned(),
                last: true,
            }),
            ..Decoded::default()
        });
        assert_eq!(index_of(&other), Some(1), "a new id gets the next index");
    }

    #[test]
    fn the_finish_reason_is_derived_because_kiro_sends_none() {
        // `messageStopEvent` says the turn ended and nothing else, so the reason comes from whether a tool
        // was used.
        let mut plain = stream();
        let _unused = plain.push(&Decoded {
            text: Some("hi".to_owned()),
            ..Decoded::default()
        });
        let chunks = plain.push(&Decoded {
            stopped: true,
            ..Decoded::default()
        });
        assert_eq!(
            chunks.last().and_then(|chunk| chunk
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)),
            Some("stop")
        );

        let mut with_tool = stream();
        let _unused = with_tool.push(&Decoded {
            tool: Some(ToolFragment {
                id: "t".to_owned(),
                name: "f".to_owned(),
                input: "{}".to_owned(),
                last: true,
            }),
            ..Decoded::default()
        });
        let stopped = with_tool.push(&Decoded {
            stopped: true,
            ..Decoded::default()
        });
        assert_eq!(
            stopped.last().and_then(|chunk| chunk
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)),
            Some("tool_calls")
        );
    }

    #[test]
    fn the_terminal_chunk_is_emitted_once_even_if_the_stream_also_ends() {
        // `messageStopEvent` finishes the stream, and the pump calls `finish` again when the body ends. A
        // second terminal chunk would look to a client like a second turn.
        let mut stream = stream();
        let stopped = stream.push(&Decoded {
            stopped: true,
            ..Decoded::default()
        });
        assert!(!stopped.is_empty());
        assert!(
            stream.finish().is_empty(),
            "the terminal chunk must not repeat"
        );
    }

    #[test]
    fn a_turn_that_produced_nothing_still_gets_an_assistant_message() {
        let mut stream = stream();
        let chunks = stream.finish();
        assert_eq!(
            deltas(&chunks).first(),
            Some(&json!({ "content": "", "role": "assistant" }))
        );
    }

    #[test]
    fn no_usage_is_reported_because_none_was_measured() {
        // Kiro's metering events are billing records, not this request's token counts.
        let mut stream = stream();
        let _unused = stream.push(&Decoded {
            text: Some("hi".to_owned()),
            ..Decoded::default()
        });
        assert!(
            stream
                .finish()
                .iter()
                .all(|chunk| chunk.get("usage").is_none())
        );
    }
}
