//! Upstream stream consumption and translation into client frames.
//!
//! Covers the streaming half of `open-sse/handlers/chatCore/streamingHandler.js`
//! and the SSE-to-JSON collapse in `nonStreamingHandler.js`: upstream bytes are
//! decoded into frames, translated, and re-serialized in the client's format.

use futures_util::StreamExt;
use nullrouter_providers::Format;
use nullrouter_translate::sse::{Encoding, Frame, LineBuffer, data_frame, done_frame, event_frame};
use nullrouter_translate::{StreamState, translate_response};
use reqwest::Response;
use serde_json::{Value, json};

use crate::errors::build_error_body;

/// How translated frames are serialized back to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientFraming {
    /// `data: {...}` frames terminated by `data: [DONE]` (OpenAI, Claude).
    Data,
    /// Named events, as the Responses API uses.
    ResponsesEvents,
}

impl ClientFraming {
    /// Framing a client format expects.
    pub const fn for_format(format: Format) -> Self {
        match format {
            Format::OpenAiResponses => Self::ResponsesEvents,
            _ => Self::Data,
        }
    }
}

/// Serialize one translated chunk into a client frame.
fn frame_for(chunk: &Value, framing: ClientFraming, source: Format) -> String {
    match framing {
        ClientFraming::Data => {
            // Claude clients expect the event name alongside the payload.
            if source == Format::Claude
                && let Some(event) = chunk.get("type").and_then(Value::as_str)
            {
                return event_frame(event, chunk);
            }
            data_frame(chunk)
        }
        ClientFraming::ResponsesEvents => chunk
            .get("type")
            .and_then(Value::as_str)
            .map_or_else(|| data_frame(chunk), |event| event_frame(event, chunk)),
    }
}

/// Upstream encoding for a provider format.
pub const fn upstream_encoding(target: Format) -> Encoding {
    match target {
        // Both send bare JSON objects, one per line, with no `data:` prefix.
        // grok.com streams bare JSON objects one per line, like these two.
        Format::Ollama | Format::CommandCode | Format::GrokWeb => Encoding::Ndjson,
        // Perplexity streams SSE, so it needs no entry here — listed for the reader who looks.
        _ => Encoding::Sse,
    }
}

/// Result of draining an upstream stream.
#[derive(Debug, Default)]
pub struct StreamSummary {
    /// Final usage reported by upstream, if any.
    pub usage: Option<nullrouter_translate::Usage>,
    /// Final finish reason, if upstream sent one.
    pub finish_reason: Option<String>,
    /// Concatenated assistant text, for request logging.
    pub text: String,
    /// An upstream-side conversation id, when the provider keeps the thread itself.
    ///
    /// Only perplexity sets this. It is surfaced on the summary rather than persisted inside the
    /// translator because remembering it is a decision about *this* request's conversation, which the
    /// translator has no view of.
    pub upstream_thread: Option<String>,
    /// The finished answer as the upstream reported it, for the same reason.
    pub upstream_answer: Option<String>,
    /// An upstream error that arrived before any content, stated rather than swallowed.
    ///
    /// Only the binary path sets this. Cursor reports a rejection as a JSON frame inside a protobuf
    /// stream, so the failure is discovered after the response headers have already said 200 — too late to
    /// answer with a status code, and a stream that simply stops looks to a client like a truncated answer.
    pub error: Option<String>,
}

/// Where translated frames are delivered.
///
/// Async so a sink backed by a bounded channel can apply backpressure: a client
/// slower than the provider must slow the upstream read, never lose a frame.
/// Dropping one would truncate JSON mid-object and corrupt the client's parse.
pub trait FrameSink {
    /// Deliver one frame. `Err` means the client is gone; consumption stops.
    fn send(&mut self, frame: String) -> impl Future<Output = Result<(), ()>>;
}

/// A sink backed by a closure, for collecting frames in tests.
impl<F> FrameSink for F
where
    F: FnMut(String) -> Result<(), ()>,
{
    fn send(&mut self, frame: String) -> impl Future<Output = Result<(), ()>> {
        std::future::ready(self(frame))
    }
}

/// Translate an upstream streaming response into client frames.
///
/// Frames reach `sink` as soon as each upstream chunk is parsed, so the client
/// sees output at the provider's own latency. Memory stays bounded by the sink,
/// not by the length of the completion.
pub async fn pipe_stream<S>(
    response: Response,
    target: Format,
    source: Format,
    state: &mut StreamState,
    mut sink: S,
) -> StreamSummary
where
    S: FrameSink,
{
    let framing = ClientFraming::for_format(source);
    let encoding = upstream_encoding(target);
    let mut buffer = LineBuffer::new();
    let mut summary = StreamSummary::default();
    let mut body = response.bytes_stream();
    let mut client_gone = false;

    'outer: while let Some(chunk) = body.next().await {
        let Ok(bytes) = chunk else {
            // A mid-stream transport error ends the stream; frames already
            // delivered stay valid.
            break;
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        for line in buffer.push(&text) {
            if emit_line(
                &line,
                encoding,
                target,
                source,
                framing,
                state,
                &mut summary,
                &mut sink,
            )
            .await
            .is_break()
            {
                client_gone = true;
                break 'outer;
            }
        }
    }

    // A final unterminated line still carries a frame.
    if !client_gone && let Some(line) = buffer.flush() {
        let _ = emit_line(
            &line,
            encoding,
            target,
            source,
            framing,
            state,
            &mut summary,
            &mut sink,
        )
        .await;
    }

    if !client_gone {
        // Some upstreams end their stream without ever saying it finished. grok.com just closes the
        // connection, so the finish reason a client waits for has to be synthesized here — after the
        // body is drained, and before the client format's own terminal frames.
        // The chunk comes back already converted into the client's shape: it is synthesized
        // OpenAI-side, so re-running the upstream translator over it would read an OpenAI chunk as a
        // grok event and produce nothing.
        for chunk in nullrouter_translate::finalize_upstream(target, source, state) {
            collect_text(&chunk, &mut summary);
            if sink.send(frame_for(&chunk, framing, source)).await.is_err() {
                client_gone = true;
                break;
            }
        }
    }

    if !client_gone {
        // Some client formats need terminal frames of their own: the Responses
        // API must close every open item and emit `response.completed`, or the
        // client waits forever.
        for chunk in nullrouter_translate::finalize_response(source, state) {
            if sink.send(frame_for(&chunk, framing, source)).await.is_err() {
                client_gone = true;
                break;
            }
        }
    }

    if !client_gone {
        let _ = sink.send(done_frame()).await;
    }

    // Translators populate state; on the pass-through path (source == target)
    // no translator runs, so values scraped from the chunks are used instead.
    summary.usage = state.usage.or(summary.usage);
    summary.finish_reason = state
        .finish_reason
        .clone()
        .or_else(|| summary.finish_reason.clone());
    // Perplexity's thread id and its finished answer, for a caller that wants to continue the
    // conversation on the next request.
    summary.upstream_thread = state.pplx_backend_uuid.clone();
    if !state.pplx_answer.is_empty() {
        summary.upstream_answer = Some(
            nullrouter_translate::response::perplexity_web_to_openai::clean(
                &state.pplx_answer,
                true,
            ),
        );
    }
    summary
}

/// Translate a binary upstream response into client frames.
///
/// Cursor's API is Connect-RPC carrying protobuf, so [`pipe_stream`] cannot serve it: that function
/// decodes each chunk as lossy UTF-8 and splits on newlines, which corrupts binary frames and finds
/// boundaries that are not there. This accumulates bytes, splits Connect frames as they complete, decodes
/// each to OpenAI chunks, and carries those on to the client's format.
///
/// Frames still reach the sink as they arrive rather than after the body is buffered, so a client sees
/// output at the provider's own latency — which upstream does not do: it buffers the whole response before
/// converting it.
pub async fn pipe_binary_stream<S>(
    response: Response,
    target: Format,
    source: Format,
    model: &str,
    state: &mut StreamState,
    mut sink: S,
) -> StreamSummary
where
    S: FrameSink,
{
    let framing = ClientFraming::for_format(source);
    let mut summary = StreamSummary::default();
    let mut pending: Vec<u8> = Vec::new();
    let mut body = response.bytes_stream();
    let mut client_gone = false;
    // Identity is fixed once so every chunk of one response shares it, the same way the translators do.
    let response_id = state
        .message_id
        .clone()
        .unwrap_or_else(|| format!("chatcmpl-{}", state.clock.now_millis()));
    state.message_id = Some(response_id.clone());
    let mut decoder = BinaryDecoder::new(target, response_id, state.clock.now_seconds(), model);

    'outer: while let Some(chunk) = body.next().await {
        let Ok(bytes) = chunk else {
            // A mid-stream transport error ends the stream; frames already delivered stay valid.
            break;
        };
        pending.extend_from_slice(&bytes);
        // Whatever is not consumed is an incomplete frame: it is kept for the next read rather than
        // discarded, since a frame does not have to arrive whole.
        let step = decoder.consume(&mut pending);
        if let Some(message) = step.fatal {
            // A failure that arrives before any content *is* the response. One that arrives after is a
            // truncation, and the text already delivered stays.
            summary.error = Some(message);
            break 'outer;
        }
        if step.stop_after {
            for chunk in nullrouter_translate::to_client(&step.chunks, source, state) {
                collect_text(&chunk, &mut summary);
                if sink.send(frame_for(&chunk, framing, source)).await.is_err() {
                    client_gone = true;
                    break;
                }
            }
            break 'outer;
        }
        for chunk in nullrouter_translate::to_client(&step.chunks, source, state) {
            collect_text(&chunk, &mut summary);
            if sink.send(frame_for(&chunk, framing, source)).await.is_err() {
                client_gone = true;
                break 'outer;
            }
        }
    }

    if !client_gone {
        for chunk in nullrouter_translate::to_client(&decoder.finish(), source, state) {
            collect_text(&chunk, &mut summary);
            if sink.send(frame_for(&chunk, framing, source)).await.is_err() {
                client_gone = true;
                break;
            }
        }
    }

    if !client_gone {
        for chunk in nullrouter_translate::finalize_response(source, state) {
            if sink.send(frame_for(&chunk, framing, source)).await.is_err() {
                client_gone = true;
                break;
            }
        }
    }

    if !client_gone {
        let _ = sink.send(done_frame()).await;
    }

    summary.finish_reason = state
        .finish_reason
        .clone()
        .or_else(|| summary.finish_reason.clone());
    summary
}

/// What one pass over the pending bytes produced.
struct BinaryStep {
    /// OpenAI-shaped chunks to relay.
    chunks: Vec<Value>,
    /// A failure that arrived before any content, which *is* the response.
    fatal: Option<String>,
    /// Whether the stream ends after these chunks — a truncating error, or the protocol's own terminator.
    stop_after: bool,
}

/// A decoder for one of the two binary upstream protocols.
///
/// The two share a shape — accumulate bytes, split frames, decode each to OpenAI chunks — and nothing else:
/// Cursor's frames are Connect-RPC carrying protobuf, Kiro's are CRC-checked `vnd.amazon.eventstream`
/// carrying JSON. Keeping them behind one enum is what lets the pump above stay single-purpose.
enum BinaryDecoder {
    Cursor(Box<crate::bespoke::cursor::response::Stream>),
    Kiro(Box<crate::bespoke::kiro::response::Stream>),
}

impl BinaryDecoder {
    fn new(target: Format, id: String, created: u64, model: &str) -> Self {
        if target == Format::Kiro {
            Self::Kiro(Box::new(crate::bespoke::kiro::response::Stream::new(
                id,
                created,
                model.to_owned(),
            )))
        } else {
            Self::Cursor(Box::new(crate::bespoke::cursor::response::Stream::new(
                id,
                created,
                model.to_owned(),
            )))
        }
    }

    /// Read every complete frame at the front of `pending`, draining what was consumed.
    fn consume(&mut self, pending: &mut Vec<u8>) -> BinaryStep {
        match self {
            Self::Cursor(stream) => {
                use crate::bespoke::cursor::{protobuf, response as cursor};
                let (frames, consumed) = protobuf::frames(pending);
                pending.drain(..consumed);
                let mut chunks = Vec::new();
                for frame in &frames {
                    if frame.is_trailer() {
                        continue;
                    }
                    if let Some(error) = cursor::error_frame(&frame.payload) {
                        let (message, _rate_limited) = cursor::error_detail(&error);
                        return BinaryStep {
                            chunks,
                            fatal: (!stream.has_content()).then_some(message),
                            stop_after: true,
                        };
                    }
                    let decoded = cursor::decode_frame(&frame.payload);
                    if decoded == cursor::Decoded::default() {
                        continue;
                    }
                    chunks.extend(stream.push(&decoded));
                }
                BinaryStep {
                    chunks,
                    fatal: None,
                    stop_after: false,
                }
            }
            Self::Kiro(stream) => {
                use crate::bespoke::kiro::{eventstream, response as kiro};
                let mut chunks = Vec::new();
                loop {
                    match eventstream::read_frame(pending) {
                        eventstream::Read::Incomplete => {
                            return BinaryStep {
                                chunks,
                                fatal: None,
                                stop_after: false,
                            };
                        }
                        // A CRC or bounds failure means the length fields cannot be trusted, so there is no
                        // safe offset to resume from. The stream ends rather than guessing one.
                        eventstream::Read::Failed(error) => {
                            let message = error.to_string();
                            return BinaryStep {
                                chunks,
                                fatal: (!stream.has_content()).then_some(message),
                                stop_after: true,
                            };
                        }
                        eventstream::Read::Frame(frame, consumed) => {
                            pending.drain(..consumed);
                            let decoded = kiro::decode(&frame);
                            if let Some(exception) = decoded.exception.clone() {
                                return BinaryStep {
                                    chunks,
                                    fatal: (!stream.has_content()).then_some(exception),
                                    stop_after: true,
                                };
                            }
                            if decoded == kiro::Decoded::default() {
                                continue;
                            }
                            chunks.extend(stream.push(&decoded));
                        }
                    }
                }
            }
        }
    }

    /// The terminal chunks.
    fn finish(&mut self) -> Vec<Value> {
        match self {
            Self::Cursor(stream) => stream.finish(),
            Self::Kiro(stream) => stream.finish(),
        }
    }
}

/// Drain a binary upstream body, handing decoded OpenAI chunks to `take`.
///
/// Shared with the streaming path in spirit but not in code: this one has no sink and no client format,
/// because the caller is assembling a single JSON response rather than relaying frames.
async fn collapse_binary_body<B, C, F>(
    body: &mut B,
    target: Format,
    model: &str,
    state: &mut StreamState,
    take: &mut F,
) where
    B: futures_util::Stream<Item = reqwest::Result<C>> + Unpin,
    C: AsRef<[u8]>,
    F: FnMut(Vec<Value>),
{
    let response_id = state
        .message_id
        .clone()
        .unwrap_or_else(|| format!("chatcmpl-{}", state.clock.now_millis()));
    state.message_id = Some(response_id.clone());
    let mut decoder = BinaryDecoder::new(target, response_id, state.clock.now_seconds(), model);
    let mut pending: Vec<u8> = Vec::new();

    while let Some(chunk) = body.next().await {
        let Ok(bytes) = chunk else { break };
        pending.extend_from_slice(bytes.as_ref());
        let step = decoder.consume(&mut pending);
        take(step.chunks);
        if step.stop_after {
            break;
        }
    }
    take(decoder.finish());
}

/// Whether stream consumption should continue.
enum Flow {
    Continue,
    Break,
}

impl Flow {
    const fn is_break(&self) -> bool {
        matches!(self, Self::Break)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "threads the full per-line translation context; splitting it would only move the arguments into a struct used once"
)]
async fn emit_line<S>(
    line: &str,
    encoding: Encoding,
    target: Format,
    source: Format,
    framing: ClientFraming,
    state: &mut StreamState,
    summary: &mut StreamSummary,
    sink: &mut S,
) -> Flow
where
    S: FrameSink,
{
    let Some(frame) = nullrouter_translate::sse::parse_line(line, encoding) else {
        return Flow::Continue;
    };
    // Upstream's terminator is replaced by ours after translation completes.
    let Frame::Data(payload) = frame else {
        return Flow::Continue;
    };

    // Translation is synchronous, so the borrow of `state` ends before the await.
    let translated = translate_response(target, source, &payload, state);
    for chunk in translated {
        collect_text(&chunk, summary);
        if sink.send(frame_for(&chunk, framing, source)).await.is_err() {
            return Flow::Break;
        }
    }
    Flow::Continue
}

/// Accumulate text, usage, and finish reason for logging, across both client
/// shapes.
///
/// Usage and finish reason are read here so the pass-through path (where no
/// translator runs and therefore nothing writes to `StreamState`) still reports
/// a complete summary.
fn collect_text(chunk: &Value, summary: &mut StreamSummary) {
    if let Some(content) = chunk
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
    {
        summary.text.push_str(content);
    } else if chunk.get("type").and_then(Value::as_str) == Some("content_block_delta")
        && let Some(text) = chunk.pointer("/delta/text").and_then(Value::as_str)
    {
        summary.text.push_str(text);
    }

    if let Some(reason) = chunk
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    {
        summary.finish_reason = Some(reason.to_owned());
    }
    if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
        summary.usage = Some(usage_from_openai(usage));
    }
}

/// Read an OpenAI-shaped `usage` object back into token counts.
fn usage_from_openai(usage: &Value) -> nullrouter_translate::Usage {
    let read = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let detail = |parent: &str, key: &str| {
        usage
            .get(parent)
            .and_then(|details| details.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    nullrouter_translate::Usage {
        prompt_tokens: read("prompt_tokens"),
        completion_tokens: read("completion_tokens"),
        total_tokens: read("total_tokens"),
        cached_tokens: detail("prompt_tokens_details", "cached_tokens"),
        cache_creation_tokens: detail("prompt_tokens_details", "cache_creation_tokens"),
        reasoning_tokens: detail("completion_tokens_details", "reasoning_tokens"),
    }
}

/// Collapse an upstream stream into a single non-streaming JSON body.
///
/// Used when a provider forces streaming but the client asked for JSON
/// (upstream `handleForcedSSEToJson`).
pub async fn collapse_stream_to_json(
    response: Response,
    target: Format,
    model: &str,
    state: &mut StreamState,
) -> Value {
    let encoding = upstream_encoding(target);
    let mut buffer = LineBuffer::new();
    let mut body = response.bytes_stream();

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut finish_reason: Option<String> = None;
    // Scraped from the chunks so the pass-through path still reports usage.
    let mut scraped_usage: Option<nullrouter_translate::Usage> = None;

    let mut take = |chunks: Vec<Value>| {
        for chunk in chunks {
            let delta = chunk.pointer("/choices/0/delta");
            if let Some(content) = delta
                .and_then(|delta| delta.get("content"))
                .and_then(Value::as_str)
            {
                text.push_str(content);
            }
            if let Some(thought) = delta
                .and_then(|delta| delta.get("reasoning_content"))
                .and_then(Value::as_str)
            {
                reasoning.push_str(thought);
            }
            if let Some(calls) = delta
                .and_then(|delta| delta.get("tool_calls"))
                .and_then(Value::as_array)
            {
                merge_tool_calls(&mut tool_calls, calls);
            }
            if let Some(reason) = chunk
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
            {
                finish_reason = Some(reason.to_owned());
            }
            if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
                scraped_usage = Some(usage_from_openai(usage));
            }
        }
    };

    if matches!(target, Format::Cursor | Format::Kiro) {
        // A binary upstream has no lines to parse. Its frames are decoded to OpenAI chunks directly, so
        // they are taken as they are rather than run through a translator that has no arm for this format.
        collapse_binary_body(&mut body, target, model, state, &mut take).await;
    } else {
        while let Some(chunk) = body.next().await {
            let Ok(bytes) = chunk else { break };
            let decoded = String::from_utf8_lossy(&bytes).into_owned();
            for line in buffer.push(&decoded) {
                if let Some(Frame::Data(payload)) =
                    nullrouter_translate::sse::parse_line(&line, encoding)
                {
                    // Always pivot to OpenAI: the JSON envelope below is OpenAI-shaped.
                    take(translate_response(target, Format::OpenAi, &payload, state));
                }
            }
        }
        if let Some(line) = buffer.flush()
            && let Some(Frame::Data(payload)) =
                nullrouter_translate::sse::parse_line(&line, encoding)
        {
            take(translate_response(target, Format::OpenAi, &payload, state));
        }
    }

    let mut message = json!({ "role": "assistant", "content": text });
    if let Some(object) = message.as_object_mut() {
        if !reasoning.is_empty() {
            object.insert("reasoning_content".to_owned(), json!(reasoning));
        }
        if !tool_calls.is_empty() {
            object.insert("tool_calls".to_owned(), Value::Array(tool_calls.clone()));
        }
    }

    let resolved_finish = finish_reason.unwrap_or_else(|| {
        if tool_calls.is_empty() {
            "stop".to_owned()
        } else {
            "tool_calls".to_owned()
        }
    });

    let mut envelope = json!({
        "id": format!("chatcmpl-{}", state.message_id.clone().unwrap_or_default()),
        "object": "chat.completion",
        "created": state.clock.now_seconds(),
        "model": model,
        "choices": [{ "index": 0, "message": message, "finish_reason": resolved_finish }],
    });
    if let Some(usage) = state.usage.or(scraped_usage)
        && let Some(object) = envelope.as_object_mut()
    {
        object.insert("usage".to_owned(), usage.to_value());
    }
    envelope
}

/// Merge streamed tool-call deltas into complete calls, keyed by index.
fn merge_tool_calls(accumulated: &mut Vec<Value>, deltas: &[Value]) {
    for delta in deltas {
        let index = delta.get("index").and_then(Value::as_u64).unwrap_or(0);
        let position = accumulated
            .iter()
            .position(|call| call.get("index").and_then(Value::as_u64) == Some(index));

        let Some(position) = position else {
            accumulated.push(delta.clone());
            continue;
        };
        let Some(existing) = accumulated.get_mut(position) else {
            continue;
        };

        let fragment = delta
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !fragment.is_empty() {
            // An absent `arguments` counts as empty, so a fragment is never
            // dropped just because the opening delta omitted the field.
            let existing_arguments = existing
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let combined = format!("{existing_arguments}{fragment}");
            if let Some(function) = existing.get_mut("function").and_then(Value::as_object_mut) {
                function.insert("arguments".to_owned(), json!(combined));
            }
        }
        // A later delta may be the first to carry the name.
        if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str)
            && let Some(function) = existing.get_mut("function").and_then(Value::as_object_mut)
        {
            let missing = function
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty);
            if missing {
                function.insert("name".to_owned(), json!(name));
            }
        }
    }
}

/// An error rendered as a single client frame plus a terminator.
pub fn error_stream_body(status: u16, message: &str, source: Format) -> String {
    let body = build_error_body(status, message);
    match ClientFraming::for_format(source) {
        ClientFraming::Data => format!("{}{}", data_frame(&body), done_frame()),
        ClientFraming::ResponsesEvents => {
            let event = json!({
                "type": "response.failed",
                "response": {
                    "id": "resp_error",
                    "status": "failed",
                    "error": body.get("error").cloned().unwrap_or(Value::Null),
                },
            });
            format!("{}{}", event_frame("response.failed", &event), done_frame())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientFraming, error_stream_body, frame_for, merge_tool_calls, upstream_encoding};
    use nullrouter_providers::Format;
    use nullrouter_translate::sse::Encoding;
    use serde_json::{Value, json};

    #[test]
    fn framing_follows_the_client_format() {
        assert_eq!(
            ClientFraming::for_format(Format::OpenAi),
            ClientFraming::Data
        );
        assert_eq!(
            ClientFraming::for_format(Format::Claude),
            ClientFraming::Data
        );
        assert_eq!(
            ClientFraming::for_format(Format::OpenAiResponses),
            ClientFraming::ResponsesEvents
        );
    }

    #[test]
    fn claude_clients_get_named_events() {
        let chunk = json!({ "type": "message_start", "message": {} });
        let frame = frame_for(&chunk, ClientFraming::Data, Format::Claude);
        assert!(
            frame.starts_with("event: message_start\ndata: {"),
            "got {frame}"
        );
    }

    #[test]
    fn openai_clients_get_plain_data_frames() {
        let chunk = json!({ "id": "chatcmpl-1", "choices": [] });
        let frame = frame_for(&chunk, ClientFraming::Data, Format::OpenAi);
        assert!(frame.starts_with("data: {"), "got {frame}");
        assert!(frame.ends_with("\n\n"));
    }

    #[test]
    fn ollama_upstream_is_ndjson() {
        assert_eq!(upstream_encoding(Format::Ollama), Encoding::Ndjson);
        assert_eq!(upstream_encoding(Format::OpenAi), Encoding::Sse);
    }

    #[test]
    fn error_streams_terminate_with_done() {
        let body = error_stream_body(429, "slow down", Format::OpenAi);
        assert!(body.contains("rate_limit_error"), "got {body}");
        assert!(body.ends_with("data: [DONE]\n\n"));

        let responses = error_stream_body(429, "slow down", Format::OpenAiResponses);
        assert!(
            responses.starts_with("event: response.failed"),
            "got {responses}"
        );
        assert!(responses.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn tool_call_deltas_merge_by_index() {
        let mut accumulated: Vec<Value> = Vec::new();
        merge_tool_calls(
            &mut accumulated,
            &[json!({
                "index": 0,
                "id": "call_1",
                "function": { "name": "Read", "arguments": "{\"a\"" },
            })],
        );
        merge_tool_calls(
            &mut accumulated,
            &[json!({ "index": 0, "function": { "arguments": ":1}" } })],
        );

        assert_eq!(accumulated.len(), 1);
        assert_eq!(
            accumulated
                .first()
                .and_then(|call| call.pointer("/function/arguments")),
            Some(&json!("{\"a\":1}"))
        );
        assert_eq!(
            accumulated
                .first()
                .and_then(|call| call.pointer("/function/name")),
            Some(&json!("Read"))
        );
    }

    #[test]
    fn tool_call_names_arriving_late_are_filled_in() {
        let mut accumulated: Vec<Value> = Vec::new();
        merge_tool_calls(
            &mut accumulated,
            &[json!({ "index": 0, "id": "c", "function": { "name": "", "arguments": "" } })],
        );
        merge_tool_calls(
            &mut accumulated,
            &[json!({ "index": 0, "function": { "name": "Write" } })],
        );
        assert_eq!(
            accumulated
                .first()
                .and_then(|call| call.pointer("/function/name")),
            Some(&json!("Write"))
        );
    }

    #[test]
    fn separate_indices_stay_separate() {
        let mut accumulated: Vec<Value> = Vec::new();
        merge_tool_calls(
            &mut accumulated,
            &[
                json!({ "index": 0, "id": "a", "function": { "name": "A", "arguments": "{}" } }),
                json!({ "index": 1, "id": "b", "function": { "name": "B", "arguments": "{}" } }),
            ],
        );
        assert_eq!(accumulated.len(), 2);
    }
}
