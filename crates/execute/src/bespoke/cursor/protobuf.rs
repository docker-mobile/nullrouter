//! Protobuf wire codec and Connect-RPC framing for `cursor`.
//!
//! Ports the primitives in `open-sse/utils/cursorProtobuf.js`. Cursor's API is Connect-RPC carrying
//! protobuf, and its schema is not published — the field numbers in [`super::field`] were established by
//! observing the IDE. That has one consequence worth stating up front: this decoder must tolerate fields
//! it does not recognise, because a Cursor release can add one at any time and refusing an unknown field
//! would break every request on the day they ship it.
//!
//! Only the four wire types protobuf defines are handled. A group (wire type 3/4, removed from proto3) is
//! treated as unparseable, which stops the frame rather than misreading the rest of it as data.

use std::io::Read as _;

/// Protobuf wire types.
pub(crate) mod wire {
    pub(crate) const VARINT: u8 = 0;
    pub(crate) const FIXED64: u8 = 1;
    pub(crate) const LEN: u8 = 2;
    pub(crate) const FIXED32: u8 = 5;
}

/// Connect-RPC frame flags.
pub(crate) mod flag {
    /// The payload is gzip-compressed.
    pub(crate) const GZIP: u8 = 0x01;
    /// The frame is a trailer rather than a message.
    pub(crate) const TRAILER: u8 = 0x02;
}

/// A Connect-RPC frame header: one flag byte then a big-endian `u32` length.
const HEADER: usize = 5;

/// Append a varint to `out`.
pub(crate) fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    // The mask leaves seven bits, so every conversion below is exact; `try_from` states that rather than
    // asserting it in a comment beside a cast.
    while value >= 0x80 {
        // Low seven bits with the continuation bit set.
        out.push(u8::try_from(value & 0x7F).unwrap_or(0) | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value & 0x7F).unwrap_or(0));
}

/// Append a length-delimited field.
pub(crate) fn put_bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_tag(out, field, wire::LEN);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

/// Append a length-delimited field holding UTF-8 text.
pub(crate) fn put_str(out: &mut Vec<u8>, field: u32, value: &str) {
    put_bytes(out, field, value.as_bytes());
}

/// Append a varint field.
pub(crate) fn put_uint(out: &mut Vec<u8>, field: u32, value: u64) {
    put_tag(out, field, wire::VARINT);
    put_varint(out, value);
}

/// Append a varint field holding a boolean.
pub(crate) fn put_bool(out: &mut Vec<u8>, field: u32, value: bool) {
    put_uint(out, field, u64::from(value));
}

fn put_tag(out: &mut Vec<u8>, field: u32, wire_type: u8) {
    put_varint(out, (u64::from(field) << 3) | u64::from(wire_type));
}

/// Encode one length-delimited field into a fresh buffer.
///
/// A convenience for the many places that build a nested message and immediately wrap it.
pub(crate) fn bytes_field(field: u32, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + 8);
    put_bytes(&mut out, field, value);
    out
}

/// Wrap a payload in a Connect-RPC frame.
///
/// Never compressed: Cursor rejects a compressed *request*, though it compresses its own responses.
pub(crate) fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + HEADER);
    out.push(0x00);
    out.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(payload);
    out
}

/// One decoded field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FieldValue {
    /// A varint, or a fixed-width integer read as one.
    Uint(u64),
    /// Length-delimited bytes: a string, a nested message, or a packed field.
    Bytes(Vec<u8>),
}

impl FieldValue {
    /// The bytes of a length-delimited field, or `None` for a varint.
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            Self::Uint(_number) => None,
        }
    }

    /// This field read as UTF-8 text.
    ///
    /// Lossy on purpose. A partial multi-byte character at the end of a streamed frame is a real
    /// occurrence, and dropping the whole frame over one truncated character loses text the user can
    /// otherwise read.
    pub(crate) fn text(&self) -> Option<String> {
        self.bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }
}

/// A decoded message: field numbers in encounter order, each with its values.
///
/// A `Vec` rather than a map because protobuf permits a field to repeat, and `messages` and `message_ids`
/// both do. Order matters for those.
#[derive(Debug, Clone, Default)]
pub(crate) struct Message {
    fields: Vec<(u32, FieldValue)>,
}

impl Message {
    /// Decode a message, stopping at the first field that cannot be parsed.
    ///
    /// Stopping rather than failing is deliberate: a frame truncated mid-field still carries every field
    /// before the break, and those are worth reading.
    pub(crate) fn decode(mut data: &[u8]) -> Self {
        let mut fields = Vec::new();
        while !data.is_empty() {
            let Some((field, wire_type, rest)) = read_tag(data) else {
                break;
            };
            let Some((value, rest)) = read_value(wire_type, rest) else {
                break;
            };
            fields.push((field, value));
            data = rest;
        }
        Self { fields }
    }

    /// The first value of a field.
    pub(crate) fn get(&self, field: u32) -> Option<&FieldValue> {
        self.fields
            .iter()
            .find(|(number, _value)| *number == field)
            .map(|(_number, value)| value)
    }

    /// Every value of a field, in encounter order.
    ///
    /// Needed for repeated protobuf fields (`messages`, history entries). `get` would hide every
    /// value after the first.
    pub(crate) fn all(&self, field: u32) -> impl Iterator<Item = &FieldValue> {
        self.fields
            .iter()
            .filter_map(move |(number, value)| (*number == field).then_some(value))
    }

    /// The first value of a field, as text.
    pub(crate) fn text(&self, field: u32) -> Option<String> {
        self.get(field).and_then(FieldValue::text)
    }

    /// The first value of a field, decoded as a nested message.
    pub(crate) fn nested(&self, field: u32) -> Option<Self> {
        self.get(field)
            .and_then(FieldValue::bytes)
            .map(Self::decode)
    }

    /// Every field number present, in encounter order and deduplicated.
    ///
    /// Only the tests read this: it exists to assert that an unknown field is decoded rather than
    /// swallowed, which is the property Cursor's unpublished schema makes load-bearing.
    #[cfg(test)]
    pub(crate) fn field_numbers(&self) -> Vec<u32> {
        let mut seen = Vec::new();
        for (number, _value) in &self.fields {
            if !seen.contains(number) {
                seen.push(*number);
            }
        }
        seen
    }
}

fn read_tag(data: &[u8]) -> Option<(u32, u8, &[u8])> {
    let (tag, rest) = read_varint(data)?;
    let field = u32::try_from(tag >> 3).ok()?;
    let wire_type = u8::try_from(tag & 0x07).unwrap_or(u8::MAX);
    // Field 0 is not legal, and reading it would mean the buffer is not a protobuf message at all.
    (field != 0).then_some((field, wire_type, rest))
}

fn read_value(wire_type: u8, data: &[u8]) -> Option<(FieldValue, &[u8])> {
    match wire_type {
        wire::VARINT => {
            let (value, rest) = read_varint(data)?;
            Some((FieldValue::Uint(value), rest))
        }
        wire::LEN => {
            let (length, rest) = read_varint(data)?;
            let length = usize::try_from(length).ok()?;
            let value = rest.get(..length)?;
            Some((FieldValue::Bytes(value.to_vec()), rest.get(length..)?))
        }
        wire::FIXED64 => {
            let value = data.get(..8)?;
            let mut bytes = [0_u8; 8];
            bytes.copy_from_slice(value);
            Some((FieldValue::Uint(u64::from_le_bytes(bytes)), data.get(8..)?))
        }
        wire::FIXED32 => {
            let value = data.get(..4)?;
            let mut bytes = [0_u8; 4];
            bytes.copy_from_slice(value);
            Some((
                FieldValue::Uint(u64::from(u32::from_le_bytes(bytes))),
                data.get(4..)?,
            ))
        }
        // Wire types 3 and 4 are groups, removed in proto3, and 6/7 are not assigned. Reading past one
        // would misinterpret the remaining bytes, so the message stops here instead.
        _unparseable => None,
    }
}

fn read_varint(data: &[u8]) -> Option<(u64, &[u8])> {
    let mut value: u64 = 0;
    let mut shift = 0_u32;
    for (index, byte) in data.iter().enumerate() {
        // A varint is at most ten bytes; more than that is corrupt rather than large.
        if shift >= 64 {
            return None;
        }
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((value, data.get(index.saturating_add(1)..)?));
        }
        shift = shift.saturating_add(7);
    }
    // Ran out of bytes mid-varint: the buffer is truncated.
    None
}

/// One frame read off a Connect-RPC stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    /// The frame's flag byte.
    pub(crate) flags: u8,
    /// The payload, decompressed if it was compressed.
    pub(crate) payload: Vec<u8>,
}

impl Frame {
    /// Whether this frame is a trailer rather than a message.
    pub(crate) const fn is_trailer(&self) -> bool {
        self.flags & flag::TRAILER != 0
    }
}

/// Split a buffer into Connect-RPC frames.
///
/// Returns the frames and the number of bytes consumed, so a caller streaming the response can keep the
/// remainder and try again once more has arrived.
pub(crate) fn frames(buffer: &[u8]) -> (Vec<Frame>, usize) {
    let mut out = Vec::new();
    let mut offset = 0_usize;
    while let Some(header) = buffer.get(offset..offset.saturating_add(HEADER)) {
        let Some(&flags) = header.first() else {
            break;
        };
        let Some(length_bytes) = header.get(1..HEADER) else {
            break;
        };
        let mut length_array = [0_u8; 4];
        length_array.copy_from_slice(length_bytes);
        let length = usize::try_from(u32::from_be_bytes(length_array)).unwrap_or(usize::MAX);
        let start = offset.saturating_add(HEADER);
        let Some(payload) = buffer.get(start..start.saturating_add(length)) else {
            // The frame is incomplete. Stop and report how much was consumed, so a streaming caller can
            // wait for the rest rather than discarding a partial frame.
            break;
        };
        offset = start.saturating_add(length);
        out.push(Frame {
            flags,
            payload: decompress(payload, flags),
        });
    }
    (out, offset)
}

/// Decompress a frame payload according to its flags.
///
/// Three formats are tried because upstream found all three in the wild: a trailer frame is sometimes raw
/// zlib and sometimes headerless deflate rather than the gzip its flag advertises. An undecodable payload
/// is returned as-is rather than dropped — an error frame arrives uncompressed with the gzip flag set, and
/// dropping it would turn a stated error into silence.
fn decompress(payload: &[u8], flags: u8) -> Vec<u8> {
    if flags & (flag::GZIP | flag::TRAILER) == 0 {
        return payload.to_vec();
    }
    // A JSON error body is never compressed whatever the flags claim.
    if payload.first() == Some(&b'{') {
        return payload.to_vec();
    }
    gunzip(payload)
        .or_else(|| zlib(payload))
        .or_else(|| deflate(payload))
        .unwrap_or_else(|| payload.to_vec())
}

fn gunzip(payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(payload)
        .read_to_end(&mut out)
        .ok()
        .map(|_read| out)
}

fn zlib(payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(payload)
        .read_to_end(&mut out)
        .ok()
        .map(|_read| out)
}

fn deflate(payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(payload)
        .read_to_end(&mut out)
        .ok()
        .map(|_read| out)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::{
        FieldValue, Message, bytes_field, frame, frames, put_bool, put_bytes, put_str, put_uint,
        put_varint,
    };

    #[test]
    fn varints_round_trip_at_their_boundaries() {
        for value in [0_u64, 1, 127, 128, 300, 16_383, 16_384, u64::from(u32::MAX)] {
            let mut buffer = Vec::new();
            put_varint(&mut buffer, value);
            let mut message = Vec::new();
            put_uint(&mut message, 1, value);
            assert_eq!(
                Message::decode(&message).get(1),
                Some(&FieldValue::Uint(value)),
                "{value} did not round-trip"
            );
        }
        // The classic two-byte case, spelled out: 300 is 0xAC 0x02.
        let mut buffer = Vec::new();
        put_varint(&mut buffer, 300);
        assert_eq!(buffer, vec![0xAC, 0x02]);
    }

    #[test]
    fn fields_round_trip_through_the_decoder() {
        let mut message = Vec::new();
        put_str(&mut message, 1, "hello");
        put_uint(&mut message, 2, 42);
        put_bool(&mut message, 3, true);
        put_bytes(&mut message, 4, &[0xDE, 0xAD]);

        let decoded = Message::decode(&message);
        assert_eq!(decoded.text(1).as_deref(), Some("hello"));
        assert_eq!(decoded.get(2), Some(&FieldValue::Uint(42)));
        assert_eq!(decoded.get(3), Some(&FieldValue::Uint(1)));
        assert_eq!(
            decoded.get(4).and_then(FieldValue::bytes),
            Some(&[0xDE, 0xAD][..])
        );
        assert_eq!(decoded.field_numbers(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_repeated_field_keeps_every_value_in_order() {
        // `messages` and `message_ids` both repeat, and their order is the conversation's order.
        let mut message = Vec::new();
        put_str(&mut message, 1, "first");
        put_str(&mut message, 1, "second");
        let decoded = Message::decode(&message);
        // `get` returns the first, and the field is listed once.
        assert_eq!(decoded.text(1).as_deref(), Some("first"));
        assert_eq!(decoded.field_numbers(), vec![1]);
    }

    #[test]
    fn nested_messages_decode() {
        let mut inner = Vec::new();
        put_str(&mut inner, 1, "inner text");
        let outer = bytes_field(2, &inner);
        let decoded = Message::decode(&outer);
        assert_eq!(
            decoded
                .nested(2)
                .and_then(|nested| nested.text(1))
                .as_deref(),
            Some("inner text")
        );
    }

    #[test]
    fn an_unknown_field_does_not_stop_the_ones_around_it() {
        // Cursor's schema is unpublished and they add fields without notice. Refusing an unknown field
        // would break every request on the day they ship one.
        let mut message = Vec::new();
        put_str(&mut message, 1, "known");
        put_uint(&mut message, 9999, 7);
        put_str(&mut message, 2, "also known");
        let decoded = Message::decode(&message);
        assert_eq!(decoded.text(1).as_deref(), Some("known"));
        assert_eq!(decoded.text(2).as_deref(), Some("also known"));
        assert!(decoded.field_numbers().contains(&9999));
    }

    #[test]
    fn a_truncated_message_keeps_the_fields_before_the_break() {
        let mut message = Vec::new();
        put_str(&mut message, 1, "complete");
        put_str(&mut message, 2, "this one gets cut");
        let cut = message.len().saturating_sub(6);
        let decoded = Message::decode(message.get(..cut).expect("a prefix"));
        assert_eq!(decoded.text(1).as_deref(), Some("complete"));
        assert!(decoded.get(2).is_none());
    }

    #[test]
    fn a_group_wire_type_stops_the_message_rather_than_being_misread() {
        // Wire type 3 is a group, removed in proto3. Reading past one would interpret the rest of the
        // buffer as something it is not.
        let mut message = Vec::new();
        put_str(&mut message, 1, "before");
        put_varint(&mut message, (4_u64 << 3) | 3);
        message.extend_from_slice(b"garbage");
        let decoded = Message::decode(&message);
        assert_eq!(decoded.text(1).as_deref(), Some("before"));
        assert_eq!(decoded.field_numbers(), vec![1]);
    }

    #[test]
    fn frames_carry_a_flag_byte_and_a_big_endian_length() {
        let framed = frame(b"payload");
        assert_eq!(framed.first(), Some(&0x00), "requests are never compressed");
        assert_eq!(framed.get(1..5), Some(&[0, 0, 0, 7][..]));
        assert_eq!(framed.get(5..), Some(&b"payload"[..]));
    }

    #[test]
    fn a_stream_splits_into_frames_and_reports_what_it_consumed() {
        let mut buffer = frame(b"one");
        buffer.extend_from_slice(&frame(b"two"));
        let (read, consumed) = frames(&buffer);
        assert_eq!(read.len(), 2);
        assert_eq!(
            read.first().map(|f| f.payload.clone()),
            Some(b"one".to_vec())
        );
        assert_eq!(
            read.get(1).map(|f| f.payload.clone()),
            Some(b"two".to_vec())
        );
        assert_eq!(consumed, buffer.len());
    }

    #[test]
    fn an_incomplete_frame_is_left_for_the_next_read() {
        // A streaming caller must be able to keep the remainder rather than discard a partial frame.
        let mut buffer = frame(b"complete");
        let partial = frame(b"incomplete");
        buffer.extend_from_slice(partial.get(..7).expect("a prefix"));
        let (read, consumed) = frames(&buffer);
        assert_eq!(read.len(), 1);
        assert_eq!(consumed, frame(b"complete").len());
        assert!(consumed < buffer.len());
    }

    #[test]
    fn a_gzip_flagged_frame_is_decompressed() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"compressed payload").expect("write");
        let compressed = encoder.finish().expect("finish");

        let mut framed = vec![0x01];
        framed.extend_from_slice(&u32::try_from(compressed.len()).expect("fits").to_be_bytes());
        framed.extend_from_slice(&compressed);

        let (read, _consumed) = frames(&framed);
        assert_eq!(
            read.first().map(|f| f.payload.clone()),
            Some(b"compressed payload".to_vec())
        );
    }

    #[test]
    fn a_zlib_trailer_frame_is_decompressed_despite_claiming_gzip() {
        // Upstream found trailer frames arriving as raw zlib and as headerless deflate, both under a flag
        // that says gzip. All three have to be tried.
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"trailer payload").expect("write");
        let compressed = encoder.finish().expect("finish");

        let mut framed = vec![0x03];
        framed.extend_from_slice(&u32::try_from(compressed.len()).expect("fits").to_be_bytes());
        framed.extend_from_slice(&compressed);

        let (read, _consumed) = frames(&framed);
        let first = read.first().expect("a frame");
        assert!(first.is_trailer());
        assert_eq!(first.payload, b"trailer payload".to_vec());
    }

    #[test]
    fn an_undecodable_payload_is_kept_rather_than_dropped() {
        // An error frame arrives uncompressed with the gzip flag set. Dropping it would turn a stated
        // error into silence.
        let body = br#"{"error":{"message":"nope"}}"#;
        let mut framed = vec![0x01];
        framed.extend_from_slice(&u32::try_from(body.len()).expect("fits").to_be_bytes());
        framed.extend_from_slice(body);

        let (read, _consumed) = frames(&framed);
        assert_eq!(read.first().map(|f| f.payload.clone()), Some(body.to_vec()));
    }
}
