//! AWS event-stream framing, as `kiro` speaks it.
//!
//! Ports `parseEventFrame` and its bounds checks from `open-sse/executors/kiro.js`. This is AWS's
//! `vnd.amazon.eventstream`: each message is a prelude (total length, headers length, prelude CRC), a
//! header block, a JSON payload, and a trailing CRC over everything before it.
//!
//! Both CRCs are checked. A frame that fails either is refused rather than read past — the length fields
//! come from the wire, and trusting a corrupt one means slicing at an arbitrary offset and then parsing
//! whatever lands there as JSON. The bounds are checked before any allocation for the same reason: a
//! declared length is not evidence that many bytes exist.

/// Largest message this will assemble. Upstream's own ceiling.
const MAX_MESSAGE: usize = 24 * 1024 * 1024;

/// Largest header block. Upstream's own ceiling.
const MAX_HEADERS: usize = 128 * 1024;

/// A prelude is 8 bytes plus its 4-byte CRC; a message adds a 4-byte trailing CRC.
const PRELUDE: usize = 12;
const MIN_FRAME: usize = 16;

/// A CRC-32 table for the IEEE polynomial, built once.
static CRC_TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
    let mut table = [0_u32; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut value = u32::try_from(index).unwrap_or(0);
        for _bit in 0..8 {
            value = (value >> 1) ^ if value & 1 == 1 { 0xEDB8_8320 } else { 0 };
        }
        *entry = value;
    }
    table
});

/// CRC-32 (IEEE) of a byte slice.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let table = &*CRC_TABLE;
    let crc = bytes.iter().fold(0xFFFF_FFFF_u32, |crc, byte| {
        let index = usize::from(u8::try_from(crc & 0xFF).unwrap_or(0) ^ *byte);
        table.get(index).copied().unwrap_or(0) ^ (crc >> 8)
    });
    crc ^ 0xFFFF_FFFF
}

/// Why a frame could not be read.
///
/// Distinguished from "not yet complete" so a streaming caller can tell a partial read from corruption:
/// the first means wait, the second means stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrameError {
    /// The prelude's CRC does not match, so its length fields cannot be trusted.
    PreludeCrc,
    /// The message CRC does not match: the frame arrived corrupt.
    MessageCrc,
    /// A length field is outside what the protocol or these ceilings allow.
    Bounds,
    /// A header could not be read within the block it declared.
    Header(String),
    /// The payload was not the JSON the protocol requires.
    Payload(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreludeCrc => write!(formatter, "event-stream prelude CRC mismatch"),
            Self::MessageCrc => write!(formatter, "event-stream message CRC mismatch"),
            Self::Bounds => write!(formatter, "event-stream frame bounds are invalid"),
            Self::Header(name) => write!(formatter, "event-stream header {name} is malformed"),
            Self::Payload(reason) => {
                write!(
                    formatter,
                    "event-stream payload is not valid JSON ({reason})"
                )
            }
        }
    }
}

/// A header value. Only the types that carry meaning here are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeaderValue {
    Bool(bool),
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    /// A timestamp or uuid: read for its length so the block stays parseable, value discarded because
    /// nothing here uses one.
    Skipped,
}

impl HeaderValue {
    /// This value as text, for the headers that name an event type.
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _other => None,
        }
    }
}

/// One decoded event-stream message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    /// The message's headers, in wire order.
    pub(crate) headers: Vec<(String, HeaderValue)>,
    /// The JSON payload, or `None` when the message carried none.
    pub(crate) payload: Option<serde_json::Value>,
}

impl Frame {
    /// A header's value by name.
    pub(crate) fn header(&self, name: &str) -> Option<&HeaderValue> {
        self.headers
            .iter()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| value)
    }

    /// The `:event-type` header, which names what this message is.
    pub(crate) fn event_type(&self) -> Option<&str> {
        self.header(":event-type").and_then(HeaderValue::text)
    }

    /// The `:message-type` header: `event`, or `exception` when the stream is reporting a failure.
    pub(crate) fn message_type(&self) -> Option<&str> {
        self.header(":message-type").and_then(HeaderValue::text)
    }

    /// The exception name, when this message is one.
    pub(crate) fn exception_type(&self) -> Option<&str> {
        self.header(":exception-type").and_then(HeaderValue::text)
    }
}

/// What a read produced.
#[derive(Debug)]
pub(crate) enum Read {
    /// A frame, and how many bytes it consumed.
    Frame(Box<Frame>, usize),
    /// Not enough bytes yet. The caller should keep them and read more.
    Incomplete,
    /// The frame is unreadable. Consuming more would be guesswork.
    Failed(FrameError),
}

/// Read one frame from the front of a buffer.
pub(crate) fn read_frame(buffer: &[u8]) -> Read {
    let Some(prelude) = buffer.get(..PRELUDE) else {
        return Read::Incomplete;
    };
    let Some(total) = read_u32(prelude, 0) else {
        return Read::Incomplete;
    };
    let Some(headers_length) = read_u32(prelude, 4) else {
        return Read::Incomplete;
    };
    let Some(prelude_crc) = read_u32(prelude, 8) else {
        return Read::Incomplete;
    };

    // The prelude's own CRC is checked *before* its lengths are used for anything. A corrupt length that
    // passed unchecked would slice at an arbitrary offset.
    if prelude_crc != crc32(prelude.get(..8).unwrap_or_default()) {
        return Read::Failed(FrameError::PreludeCrc);
    }

    let total = usize::try_from(total).unwrap_or(usize::MAX);
    let headers_length = usize::try_from(headers_length).unwrap_or(usize::MAX);
    if !(MIN_FRAME..=MAX_MESSAGE).contains(&total)
        || headers_length > MAX_HEADERS
        || headers_length > total.saturating_sub(MIN_FRAME)
    {
        return Read::Failed(FrameError::Bounds);
    }

    let Some(frame) = buffer.get(..total) else {
        // The lengths are sound but the bytes have not all arrived.
        return Read::Incomplete;
    };

    // The trailing CRC covers everything before it.
    let body = frame.get(..total.saturating_sub(4)).unwrap_or_default();
    let Some(message_crc) = read_u32(frame, total.saturating_sub(4)) else {
        return Read::Failed(FrameError::Bounds);
    };
    if message_crc != crc32(body) {
        return Read::Failed(FrameError::MessageCrc);
    }

    let header_end = PRELUDE.saturating_add(headers_length);
    let Some(header_bytes) = frame.get(PRELUDE..header_end) else {
        return Read::Failed(FrameError::Bounds);
    };
    let headers = match parse_headers(header_bytes) {
        Ok(headers) => headers,
        Err(error) => return Read::Failed(error),
    };

    let payload_bytes = frame
        .get(header_end..total.saturating_sub(4))
        .unwrap_or_default();
    let payload = if payload_bytes.iter().all(u8::is_ascii_whitespace) {
        // An empty or whitespace-only payload is a message with no body, not a malformed one.
        None
    } else {
        match serde_json::from_slice::<serde_json::Value>(payload_bytes) {
            Ok(value) => Some(value),
            Err(error) => return Read::Failed(FrameError::Payload(error.to_string())),
        }
    };

    Read::Frame(Box::new(Frame { headers, payload }), total)
}

/// Parse a header block.
fn parse_headers(mut block: &[u8]) -> Result<Vec<(String, HeaderValue)>, FrameError> {
    let mut headers: Vec<(String, HeaderValue)> = Vec::new();
    while !block.is_empty() {
        let (&name_length, rest) = block
            .split_first()
            .ok_or_else(|| FrameError::Header("<truncated>".to_owned()))?;
        let name_length = usize::from(name_length);
        let name_bytes = rest
            .get(..name_length)
            .ok_or_else(|| FrameError::Header("<truncated>".to_owned()))?;
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        let rest = rest
            .get(name_length..)
            .ok_or_else(|| FrameError::Header(name.clone()))?;
        let (&kind, rest) = rest
            .split_first()
            .ok_or_else(|| FrameError::Header(name.clone()))?;

        // A duplicate header is refused rather than resolved to one of the two. AWS does not send them,
        // so one appearing means the block is not what it claims.
        if headers.iter().any(|(existing, _value)| *existing == name) {
            return Err(FrameError::Header(name));
        }

        let (value, rest) = read_header_value(kind, rest, &name)?;
        headers.push((name, value));
        block = rest;
    }
    Ok(headers)
}

fn read_header_value<'a>(
    kind: u8,
    rest: &'a [u8],
    name: &str,
) -> Result<(HeaderValue, &'a [u8]), FrameError> {
    let malformed = || FrameError::Header(name.to_owned());
    match kind {
        // 0 and 1 are the boolean literals; they carry no bytes.
        0 => Ok((HeaderValue::Bool(true), rest)),
        1 => Ok((HeaderValue::Bool(false), rest)),
        2 => {
            let (&byte, rest) = rest.split_first().ok_or_else(malformed)?;
            // A one-byte header is signed, so the wrap to `i8` is the protocol's own reading of it.
            Ok((HeaderValue::Int(i64::from(i8::from_be_bytes([byte]))), rest))
        }
        3 => {
            let bytes = rest.get(..2).ok_or_else(malformed)?;
            let mut array = [0_u8; 2];
            array.copy_from_slice(bytes);
            Ok((
                HeaderValue::Int(i64::from(i16::from_be_bytes(array))),
                rest.get(2..).ok_or_else(malformed)?,
            ))
        }
        4 => {
            let bytes = rest.get(..4).ok_or_else(malformed)?;
            let mut array = [0_u8; 4];
            array.copy_from_slice(bytes);
            Ok((
                HeaderValue::Int(i64::from(i32::from_be_bytes(array))),
                rest.get(4..).ok_or_else(malformed)?,
            ))
        }
        // 5 is int64 and 8 is a timestamp. Both are eight bytes and neither is read here.
        5 | 8 => Ok((HeaderValue::Skipped, rest.get(8..).ok_or_else(malformed)?)),
        // 6 is a byte array, 7 a string. Both are length-prefixed.
        6 | 7 => {
            let length_bytes = rest.get(..2).ok_or_else(malformed)?;
            let mut array = [0_u8; 2];
            array.copy_from_slice(length_bytes);
            let length = usize::from(u16::from_be_bytes(array));
            let body = rest
                .get(2..2usize.saturating_add(length))
                .ok_or_else(malformed)?;
            let value = if kind == 7 {
                HeaderValue::Text(String::from_utf8_lossy(body).into_owned())
            } else {
                HeaderValue::Bytes(body.to_vec())
            };
            Ok((
                value,
                rest.get(2usize.saturating_add(length)..)
                    .ok_or_else(malformed)?,
            ))
        }
        // 9 is a uuid: sixteen bytes, unused here.
        9 => Ok((HeaderValue::Skipped, rest.get(16..).ok_or_else(malformed)?)),
        // An unknown type has an unknown length, so the block cannot be walked past it.
        _unknown => Err(malformed()),
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.saturating_add(4))?;
    let mut array = [0_u8; 4];
    array.copy_from_slice(slice);
    Some(u32::from_be_bytes(array))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{FrameError, HeaderValue, MAX_HEADERS, MAX_MESSAGE, Read, crc32, read_frame};

    /// Build a well-formed frame.
    fn build(headers: &[(&str, HeaderValue)], payload: Option<&serde_json::Value>) -> Vec<u8> {
        let mut header_block = Vec::new();
        for (name, value) in headers {
            header_block.push(u8::try_from(name.len()).unwrap_or(0));
            header_block.extend_from_slice(name.as_bytes());
            match value {
                HeaderValue::Text(text) => {
                    header_block.push(7);
                    header_block
                        .extend_from_slice(&u16::try_from(text.len()).unwrap_or(0).to_be_bytes());
                    header_block.extend_from_slice(text.as_bytes());
                }
                HeaderValue::Bool(flag) => header_block.push(u8::from(!*flag)),
                HeaderValue::Int(number) => {
                    header_block.push(4);
                    header_block
                        .extend_from_slice(&i32::try_from(*number).unwrap_or(0).to_be_bytes());
                }
                HeaderValue::Bytes(bytes) => {
                    header_block.push(6);
                    header_block
                        .extend_from_slice(&u16::try_from(bytes.len()).unwrap_or(0).to_be_bytes());
                    header_block.extend_from_slice(bytes);
                }
                HeaderValue::Skipped => {
                    header_block.push(8);
                    header_block.extend_from_slice(&[0_u8; 8]);
                }
            }
        }
        let body = payload.map(Value::to_string).unwrap_or_default();
        let total = 16_usize
            .saturating_add(header_block.len())
            .saturating_add(body.len());

        let mut frame = Vec::with_capacity(total);
        frame.extend_from_slice(&u32::try_from(total).unwrap_or(0).to_be_bytes());
        frame.extend_from_slice(&u32::try_from(header_block.len()).unwrap_or(0).to_be_bytes());
        let prelude_crc = crc32(frame.get(..8).unwrap_or_default());
        frame.extend_from_slice(&prelude_crc.to_be_bytes());
        frame.extend_from_slice(&header_block);
        frame.extend_from_slice(body.as_bytes());
        let message_crc = crc32(&frame);
        frame.extend_from_slice(&message_crc.to_be_bytes());
        frame
    }

    fn event(kind: &str, payload: &serde_json::Value) -> Vec<u8> {
        build(
            &[
                (":event-type", HeaderValue::Text(kind.to_owned())),
                (":message-type", HeaderValue::Text("event".to_owned())),
            ],
            Some(payload),
        )
    }

    #[test]
    fn the_crc_matches_the_published_check_value() {
        // CRC-32/IEEE of "123456789" is 0xCBF43926. Asserting a published vector pins the polynomial and
        // the bit order, which a self-consistent round-trip would not.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_well_formed_frame_decodes_to_its_headers_and_payload() {
        let frame = event("assistantResponseEvent", &json!({ "content": "hello" }));
        let Read::Frame(decoded, consumed) = read_frame(&frame) else {
            panic!("the frame should decode");
        };
        assert_eq!(consumed, frame.len());
        assert_eq!(decoded.event_type(), Some("assistantResponseEvent"));
        assert_eq!(decoded.message_type(), Some("event"));
        assert_eq!(
            decoded
                .payload
                .as_ref()
                .and_then(|payload| payload.get("content")),
            Some(&json!("hello"))
        );
    }

    #[test]
    fn a_partial_frame_is_incomplete_rather_than_failed() {
        // A streaming caller has to tell "wait for more" from "this is corrupt". Treating a partial read
        // as corruption would abort a healthy stream on a chunk boundary.
        let frame = event("assistantResponseEvent", &json!({ "content": "hello" }));
        for cut in [0_usize, 4, 11, 12, frame.len().saturating_sub(1)] {
            let partial = frame.get(..cut).expect("a prefix");
            assert!(
                matches!(read_frame(partial), Read::Incomplete),
                "a {cut}-byte prefix should be incomplete"
            );
        }
    }

    #[test]
    fn several_frames_are_read_one_at_a_time() {
        let mut buffer = event("assistantResponseEvent", &json!({ "content": "one" }));
        let first_length = buffer.len();
        buffer.extend_from_slice(&event(
            "assistantResponseEvent",
            &json!({ "content": "two" }),
        ));

        let Read::Frame(first, consumed) = read_frame(&buffer) else {
            panic!("the first frame should decode");
        };
        assert_eq!(consumed, first_length);
        assert_eq!(
            first.payload.as_ref().and_then(|p| p.get("content")),
            Some(&json!("one"))
        );
        let Read::Frame(second, _consumed) = read_frame(buffer.get(consumed..).expect("the rest"))
        else {
            panic!("the second frame should decode");
        };
        assert_eq!(
            second.payload.as_ref().and_then(|p| p.get("content")),
            Some(&json!("two"))
        );
    }

    #[test]
    fn a_corrupt_prelude_is_refused_before_its_lengths_are_used() {
        // The lengths come from the wire. Trusting a corrupt one means slicing at an arbitrary offset and
        // parsing whatever lands there.
        let mut frame = event("assistantResponseEvent", &json!({ "content": "hi" }));
        // Claim a total length of 4GB-ish while leaving the CRC alone.
        if let Some(slot) = frame.get_mut(..4) {
            slot.copy_from_slice(&0xFFFF_FFF0_u32.to_be_bytes());
        }
        assert!(matches!(
            read_frame(&frame),
            Read::Failed(FrameError::PreludeCrc)
        ));
    }

    #[test]
    fn a_corrupt_payload_is_caught_by_the_message_crc() {
        let mut frame = event("assistantResponseEvent", &json!({ "content": "hello" }));
        // Flip a byte inside the payload, leaving both lengths and the prelude CRC intact.
        let last = frame.len().saturating_sub(6);
        if let Some(byte) = frame.get_mut(last) {
            *byte ^= 0xFF;
        }
        assert!(matches!(
            read_frame(&frame),
            Read::Failed(FrameError::MessageCrc)
        ));
    }

    #[test]
    fn declared_lengths_beyond_the_ceilings_are_refused() {
        // A length is a claim, not evidence that many bytes exist. Both ceilings are checked before
        // anything is allocated against them.
        // Just over the 24 MiB ceiling. A value under it is `Incomplete` instead, since the length would be
        // legitimate and the bytes simply absent — which is the distinction the next assertion checks.
        let mut frame = Vec::new();
        frame.extend_from_slice(&(u32::try_from(MAX_MESSAGE).expect("fits") + 1).to_be_bytes());
        frame.extend_from_slice(&0_u32.to_be_bytes());
        frame.extend_from_slice(&crc32(frame.get(..8).expect("a prelude")).to_be_bytes());
        assert!(matches!(
            read_frame(&frame),
            Read::Failed(FrameError::Bounds)
        ));

        // A large but legal length with the bytes missing is a partial read, not corruption.
        let mut legal = Vec::new();
        legal.extend_from_slice(&1_000_000_u32.to_be_bytes());
        legal.extend_from_slice(&0_u32.to_be_bytes());
        legal.extend_from_slice(&crc32(legal.get(..8).expect("a prelude")).to_be_bytes());
        assert!(matches!(read_frame(&legal), Read::Incomplete));

        // A header block longer than the frame that contains it.
        let mut oversized = Vec::new();
        oversized.extend_from_slice(&100_u32.to_be_bytes());
        oversized.extend_from_slice(&u32::try_from(MAX_HEADERS + 1).expect("fits").to_be_bytes());
        oversized.extend_from_slice(&crc32(oversized.get(..8).expect("a prelude")).to_be_bytes());
        assert!(matches!(
            read_frame(&oversized),
            Read::Failed(FrameError::Bounds)
        ));

        // And a total below the sixteen-byte minimum.
        let mut tiny = Vec::new();
        tiny.extend_from_slice(&8_u32.to_be_bytes());
        tiny.extend_from_slice(&0_u32.to_be_bytes());
        tiny.extend_from_slice(&crc32(tiny.get(..8).expect("a prelude")).to_be_bytes());
        assert!(matches!(
            read_frame(&tiny),
            Read::Failed(FrameError::Bounds)
        ));
    }

    #[test]
    fn every_header_type_is_walked_even_when_its_value_is_not_read() {
        // An unread type still has to be stepped over by the right number of bytes, or every header after
        // it is misparsed.
        let frame = build(
            &[
                (":event-type", HeaderValue::Text("metadataEvent".to_owned())),
                ("a-timestamp", HeaderValue::Skipped),
                ("a-number", HeaderValue::Int(42)),
                ("a-flag", HeaderValue::Bool(true)),
                ("some-bytes", HeaderValue::Bytes(vec![1, 2, 3])),
                (":message-type", HeaderValue::Text("event".to_owned())),
            ],
            Some(&json!({})),
        );
        let Read::Frame(decoded, _consumed) = read_frame(&frame) else {
            panic!("the frame should decode");
        };
        // The header after the skipped one still reads correctly.
        assert_eq!(decoded.header("a-number"), Some(&HeaderValue::Int(42)));
        assert_eq!(decoded.header("a-flag"), Some(&HeaderValue::Bool(true)));
        assert_eq!(
            decoded.header("some-bytes"),
            Some(&HeaderValue::Bytes(vec![1, 2, 3]))
        );
        assert_eq!(decoded.message_type(), Some("event"));
    }

    #[test]
    fn an_unknown_header_type_stops_the_block() {
        // Its length is unknown, so there is no way to step over it.
        let mut header_block = Vec::new();
        header_block.push(4);
        header_block.extend_from_slice(b"name");
        header_block.push(200);
        let total = 16 + header_block.len();
        let mut frame = Vec::new();
        frame.extend_from_slice(&u32::try_from(total).expect("fits").to_be_bytes());
        frame.extend_from_slice(
            &u32::try_from(header_block.len())
                .expect("fits")
                .to_be_bytes(),
        );
        frame.extend_from_slice(&crc32(frame.get(..8).expect("a prelude")).to_be_bytes());
        frame.extend_from_slice(&header_block);
        let crc = crc32(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());

        assert!(matches!(
            read_frame(&frame),
            Read::Failed(FrameError::Header(_))
        ));
    }

    #[test]
    fn a_duplicate_header_is_refused() {
        // AWS does not send them, so one appearing means the block is not what it claims to be.
        let frame = build(
            &[
                (":event-type", HeaderValue::Text("a".to_owned())),
                (":event-type", HeaderValue::Text("b".to_owned())),
            ],
            Some(&json!({})),
        );
        assert!(matches!(
            read_frame(&frame),
            Read::Failed(FrameError::Header(_))
        ));
    }

    #[test]
    fn a_message_with_no_payload_is_not_an_error() {
        let frame = build(
            &[(
                ":event-type",
                HeaderValue::Text("messageStopEvent".to_owned()),
            )],
            None,
        );
        let Read::Frame(decoded, _consumed) = read_frame(&frame) else {
            panic!("the frame should decode");
        };
        assert!(decoded.payload.is_none());
        assert_eq!(decoded.event_type(), Some("messageStopEvent"));
    }

    #[test]
    fn a_payload_that_is_not_json_is_reported_as_such() {
        let mut header_block = Vec::new();
        header_block.push(11);
        header_block.extend_from_slice(b":event-type");
        header_block.push(7);
        header_block.extend_from_slice(&5_u16.to_be_bytes());
        header_block.extend_from_slice(b"weird");
        let body = b"not json at all";
        let total = 16 + header_block.len() + body.len();

        let mut frame = Vec::new();
        frame.extend_from_slice(&u32::try_from(total).expect("fits").to_be_bytes());
        frame.extend_from_slice(
            &u32::try_from(header_block.len())
                .expect("fits")
                .to_be_bytes(),
        );
        frame.extend_from_slice(&crc32(frame.get(..8).expect("a prelude")).to_be_bytes());
        frame.extend_from_slice(&header_block);
        frame.extend_from_slice(body);
        let crc = crc32(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());

        assert!(matches!(
            read_frame(&frame),
            Read::Failed(FrameError::Payload(_))
        ));
    }

    #[test]
    fn an_exception_frame_names_its_exception() {
        // Kiro reports a throttle or a rejection as an exception message rather than a status code, since
        // the response headers already said 200.
        let frame = build(
            &[
                (":message-type", HeaderValue::Text("exception".to_owned())),
                (
                    ":exception-type",
                    HeaderValue::Text("ThrottlingException".to_owned()),
                ),
            ],
            Some(&json!({ "message": "Too many requests" })),
        );
        let Read::Frame(decoded, _consumed) = read_frame(&frame) else {
            panic!("the frame should decode");
        };
        assert_eq!(decoded.message_type(), Some("exception"));
        assert_eq!(decoded.exception_type(), Some("ThrottlingException"));
    }
}
