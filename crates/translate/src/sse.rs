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
    ///
    /// One allocation per line, and one buffer shift per chunk. The obvious implementation —
    /// copying the line out, copying it again to strip `\r`, then rebuilding `self.buffer` from the
    /// remainder — allocates three times per line and makes the whole loop quadratic in the number
    /// of lines a single chunk carries, because each line re-copies everything after it. A
    /// 2000-frame response arriving in few chunks pays that quadratic term in full.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(chunk);

        // Find the boundaries first, so the buffer is shifted exactly once rather than per line.
        let mut lines = Vec::new();
        let mut start = 0_usize;
        while let Some(offset) = self.buffer.get(start..).and_then(|rest| rest.find('\n')) {
            let end = start + offset;
            let line = self.buffer.get(start..end).unwrap_or_default();
            // `\r\n` line endings. Stripped by slicing, not by a second copy.
            lines.push(line.strip_suffix('\r').unwrap_or(line).to_owned());
            start = end + 1;
        }
        if start > 0 {
            // `drain` moves the tail down in place; the previous version allocated a fresh String
            // for the remainder on every line.
            self.buffer.drain(..start);
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
    fn many_lines_in_one_chunk_all_come_back_in_order() {
        // The case the rewrite was for. Every line arriving in a single chunk used to re-copy the
        // whole remaining buffer, so a chunk carrying N lines did O(N^2) byte copies; a 2000-frame
        // completion delivered in a few chunks hit that squarely. Correctness first: the boundaries
        // must land in the same places whatever the chunking.
        let mut buffer = LineBuffer::new();
        let mut chunk = String::new();
        for index in 0..2000 {
            use std::fmt::Write as _;
            let _ = writeln!(chunk, "data: {{\"i\":{index}}}");
        }
        let lines = buffer.push(&chunk);
        assert_eq!(lines.len(), 2000);
        assert_eq!(
            lines
                .first()
                .and_then(|line| parse_line(line, Encoding::Sse)),
            Some(Frame::Data(json!({ "i": 0 })))
        );
        assert_eq!(
            lines
                .last()
                .and_then(|line| parse_line(line, Encoding::Sse)),
            Some(Frame::Data(json!({ "i": 1999 })))
        );
        assert!(buffer.is_empty(), "nothing should be left buffered");
    }

    #[test]
    fn the_same_bytes_split_differently_yield_the_same_lines() {
        // Chunk boundaries are the network's business, not the caller's, so the line sequence must
        // not depend on them. This is the invariant the buffer exists to provide, and the one an
        // off-by-one in the new offset arithmetic would break.
        let source = "data: one\n\ndata: two\r\ndata: [DONE]\n";
        let whole = LineBuffer::new().push(source);

        for split in 1..source.len() {
            let mut buffer = LineBuffer::new();
            let mut lines = buffer.push(source.get(..split).unwrap_or_default());
            lines.extend(buffer.push(source.get(split..).unwrap_or_default()));
            assert_eq!(lines, whole, "split at {split} changed the line sequence");
        }
    }

    #[test]
    fn a_blank_line_is_returned_as_a_line() {
        // SSE frames are terminated by a blank line, so the empty string between two `data:` lines
        // is real output and must not be silently dropped.
        let mut buffer = LineBuffer::new();
        assert_eq!(buffer.push("a\n\nb\n"), vec!["a", "", "b"]);
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
