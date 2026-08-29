//! Incremental SSE / NDJSON line parsing and frame serialization.
//!
//! Ports `parseSSELine` from `open-sse/utils/streamHelpers.js` and the frame
//! builders in `open-sse/utils/sse.js`, plus the byte-level buffering that the
//! JS runtime gets from its stream reader.

use serde_json::Value;

/// One parsed item from an upstream stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A decoded JSON payload.
    Data(Value),
    /// The terminal `data: [DONE]` sentinel.
    Done,
}

/// Upstream stream encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// `data: {...}` lines.
    Sse,
    /// Bare JSON objects, one per line (Ollama, `CommandCode`).
    Ndjson,
}

/// Parse a single line (upstream `parseSSELine`).
///
/// Returns `None` for comments, blank lines, `event:` lines, and undecodable
/// payloads — matching upstream's tolerant behavior.
pub fn parse_line(line: &str, encoding: Encoding) -> Option<Frame> {
    if line.is_empty() {
        return None;
    }

    if encoding == Encoding::Ndjson {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            return None;
        }
        return serde_json::from_str(trimmed).ok().map(Frame::Data);
    }

    // Upstream checks `line.charCodeAt(0) !== 100` — only lines starting with
    // 'd' are considered, so `event:`/`id:`/`:` lines are skipped.
    if !line.starts_with('d') {
        return None;
    }
    let payload = line.get(5..)?.trim();
    if payload == "[DONE]" {
        return Some(Frame::Done);
    }
    serde_json::from_str(payload).ok().map(Frame::Data)
}

/// Accumulates upstream bytes and yields complete lines.
///
/// Upstream relies on the JS stream reader for this; in Rust the chunk
/// boundaries are ours to manage, so partial lines are held until terminated.
#[derive(Debug, Default)]
pub struct LineBuffer {
    buffer: String,
}

impl LineBuffer {
    pub const fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Append a chunk and return every complete line it finished.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(chunk);
        let mut lines = Vec::new();
        while let Some(newline) = self.buffer.find('\n') {
            let line = self.buffer.get(..newline).unwrap_or_default().to_owned();
            // `\r\n` line endings.
            let line = line.strip_suffix('\r').unwrap_or(&line).to_owned();
            self.buffer = self
                .buffer
                .get(newline + 1..)
                .unwrap_or_default()
                .to_owned();
            lines.push(line);
        }
        lines
    }

    /// Take whatever remains after the stream ends, if it is not blank.
    pub fn flush(&mut self) -> Option<String> {
        let remainder = std::mem::take(&mut self.buffer);
        let trimmed = remainder.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }

    /// `true` when nothing is buffered.
    pub const fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Serialize a `data:` frame (upstream `sseChunk`).
pub fn data_frame(payload: &Value) -> String {
    let encoded = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_owned());
    format!("data: {encoded}\n\n")
}

/// Serialize a named-event frame, as the Responses API uses.
pub fn event_frame(event: &str, payload: &Value) -> String {
    let encoded = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_owned());
    format!("event: {event}\ndata: {encoded}\n\n")
}

/// The terminal frame.
pub fn done_frame() -> String {
    "data: [DONE]\n\n".to_owned()
}

#[cfg(test)]
mod tests {
    use super::{Encoding, Frame, LineBuffer, data_frame, done_frame, event_frame, parse_line};
    use serde_json::json;

    #[test]
    fn sse_data_lines_decode() {
        assert_eq!(
            parse_line(r#"data: {"a":1}"#, Encoding::Sse),
            Some(Frame::Data(json!({ "a": 1 })))
        );
        assert_eq!(parse_line("data: [DONE]", Encoding::Sse), Some(Frame::Done));
    }

    #[test]
    fn non_data_lines_are_skipped() {
        assert_eq!(parse_line("", Encoding::Sse), None);
        assert_eq!(parse_line(": keep-alive", Encoding::Sse), None);
        assert_eq!(parse_line("event: response.failed", Encoding::Sse), None);
        assert_eq!(parse_line("id: 42", Encoding::Sse), None);
        // Malformed payloads are dropped, not fatal.
        assert_eq!(parse_line("data: {not json", Encoding::Sse), None);
    }

    #[test]
    fn ndjson_lines_decode_without_prefix() {
        assert_eq!(
            parse_line(r#"{"done":false}"#, Encoding::Ndjson),
            Some(Frame::Data(json!({ "done": false })))
        );
        // NDJSON ignores anything that is not a JSON object.
        assert_eq!(parse_line("data: {}", Encoding::Ndjson), None);
        assert_eq!(parse_line("[1,2]", Encoding::Ndjson), None);
    }

    #[test]
    fn line_buffer_reassembles_split_frames() {
        let mut buffer = LineBuffer::new();
        // A frame split mid-JSON across two chunks.
        assert!(buffer.push("data: {\"a\":").is_empty());
        let lines = buffer.push("1}\ndata: [DONE]\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines
                .first()
                .and_then(|line| parse_line(line, Encoding::Sse)),
            Some(Frame::Data(json!({ "a": 1 })))
        );
        assert_eq!(
            lines
                .get(1)
                .and_then(|line| parse_line(line, Encoding::Sse)),
            Some(Frame::Done)
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn line_buffer_handles_crlf_and_trailing_data() {
        let mut buffer = LineBuffer::new();
        let lines = buffer.push("data: {\"a\":1}\r\n");
        assert_eq!(
            lines
                .first()
                .and_then(|line| parse_line(line, Encoding::Sse)),
            Some(Frame::Data(json!({ "a": 1 })))
        );

        // An unterminated final line survives until flush.
        let mut tail = LineBuffer::new();
        assert!(tail.push("data: {\"b\":2}").is_empty());
        let remainder = tail.flush().expect("trailing line is returned");
        assert_eq!(
            parse_line(&remainder, Encoding::Sse),
            Some(Frame::Data(json!({ "b": 2 })))
        );
        assert!(tail.flush().is_none());
    }

    #[test]
    fn frames_serialize_with_blank_line_terminators() {
        assert_eq!(data_frame(&json!({ "a": 1 })), "data: {\"a\":1}\n\n");
        assert_eq!(
            event_frame("response.failed", &json!({ "b": 2 })),
            "event: response.failed\ndata: {\"b\":2}\n\n"
        );
        assert_eq!(done_frame(), "data: [DONE]\n\n");
    }
}
