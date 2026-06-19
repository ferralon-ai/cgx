//! Hand-rolled, read-only protobuf wire decoder for the narrow field subset
//! cgx needs from a `.scip` index (decision R1 — no `prost`/`protobuf`/`scip`
//! crate). It understands only two wire types:
//!
//! - **Wire type 0** (`varint`): tags, enums, `int32`/`bool` scalars.
//! - **Wire type 2** (`len`): length-delimited bytes — strings, packed
//!   repeated scalars, and embedded sub-messages.
//!
//! Wire types 1 (`fixed64`), 5 (`fixed32`), and the deprecated group types
//! (3/4) never appear on SCIP's hot path; if one is encountered on an *unknown*
//! field it is skipped, and on a *known* field it is a decode error.
//!
//! The decoder never allocates beyond the borrowed input: every length-delimited
//! field is returned as a `&[u8]` slice into the original buffer. Unknown fields
//! are skipped without error so the reader tolerates schema additions.

use std::fmt;

/// A protobuf wire type (the low 3 bits of a field tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireType {
    /// Base-128 varint (wire type 0).
    Varint,
    /// 64-bit fixed (wire type 1).
    Fixed64,
    /// Length-delimited bytes (wire type 2).
    Len,
    /// 32-bit fixed (wire type 5).
    Fixed32,
}

/// Errors the wire reader can surface. All are recoverable at the caller level
/// (a malformed `.scip` is rejected, never panics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The buffer ended in the middle of a field.
    UnexpectedEof,
    /// A varint ran past 10 bytes (more than a u64 can hold).
    VarintOverflow,
    /// A field tag carried a wire type this decoder does not implement on a
    /// *known* field (groups, or a fixed type where a scalar was expected).
    UnsupportedWireType(u8),
    /// A length-delimited field claimed more bytes than remain in the buffer.
    LengthOutOfRange,
    /// A UTF-8 string field was not valid UTF-8.
    InvalidUtf8,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::UnexpectedEof => write!(f, "unexpected end of protobuf buffer"),
            WireError::VarintOverflow => write!(f, "varint exceeds 64 bits"),
            WireError::UnsupportedWireType(w) => write!(f, "unsupported wire type {w}"),
            WireError::LengthOutOfRange => write!(f, "length-delimited field out of range"),
            WireError::InvalidUtf8 => write!(f, "invalid UTF-8 in string field"),
        }
    }
}

impl std::error::Error for WireError {}

/// A cursor over a borrowed protobuf message body.
///
/// Construct one with [`Reader::new`], then drive it with [`Reader::next_field`]
/// in a `while let` loop. Field payloads are read with the typed accessors
/// ([`Reader::read_varint`], [`Reader::read_len`]); unknown fields are dropped
/// with [`Reader::skip`].
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

/// A decoded field header: its field number and wire type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldHeader {
    /// The protobuf field number (tag >> 3).
    pub number: u32,
    /// The wire type (tag & 0x7).
    pub wire_type: WireType,
}

impl<'a> Reader<'a> {
    /// Create a reader over an entire message body.
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    /// True once every byte has been consumed.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }

    /// Read the next field header, or `None` at end of buffer.
    ///
    /// Returns an error only when a tag is malformed; an *unknown* wire type on
    /// the tag is reported faithfully so the caller can decide to [`skip`] it.
    ///
    /// [`skip`]: Reader::skip
    pub fn next_field(&mut self) -> Result<Option<FieldHeader>, WireError> {
        if self.is_empty() {
            return Ok(None);
        }
        let tag = self.read_raw_varint()?;
        let number = (tag >> 3) as u32;
        let wire_type = match tag & 0x7 {
            0 => WireType::Varint,
            1 => WireType::Fixed64,
            2 => WireType::Len,
            5 => WireType::Fixed32,
            other => return Err(WireError::UnsupportedWireType(other as u8)),
        };
        Ok(Some(FieldHeader { number, wire_type }))
    }

    /// Read a varint payload (wire type 0) as a `u64`.
    pub fn read_varint(&mut self) -> Result<u64, WireError> {
        self.read_raw_varint()
    }

    /// Read a varint payload as an `i32` (protobuf `int32`/enum). Values are
    /// truncated to the low 32 bits, matching protobuf semantics.
    pub fn read_i32(&mut self) -> Result<i32, WireError> {
        Ok(self.read_raw_varint()? as u32 as i32)
    }

    /// Read a length-delimited payload (wire type 2) as a borrowed slice.
    pub fn read_len(&mut self) -> Result<&'a [u8], WireError> {
        let len = self.read_raw_varint()? as usize;
        let end = self.pos.checked_add(len).ok_or(WireError::LengthOutOfRange)?;
        if end > self.buf.len() {
            return Err(WireError::LengthOutOfRange);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Read a length-delimited payload as a UTF-8 `&str`.
    pub fn read_str(&mut self) -> Result<&'a str, WireError> {
        let bytes = self.read_len()?;
        std::str::from_utf8(bytes).map_err(|_| WireError::InvalidUtf8)
    }

    /// Read a length-delimited payload and hand back a sub-message [`Reader`].
    pub fn read_message(&mut self) -> Result<Reader<'a>, WireError> {
        Ok(Reader::new(self.read_len()?))
    }

    /// Skip the payload of a field whose header was already read, regardless of
    /// wire type. This is what makes the decoder forward-compatible: an unknown
    /// field number is consumed without interpretation.
    pub fn skip(&mut self, wire_type: WireType) -> Result<(), WireError> {
        match wire_type {
            WireType::Varint => {
                self.read_raw_varint()?;
            }
            WireType::Len => {
                let _ = self.read_len()?;
            }
            WireType::Fixed64 => self.advance(8)?,
            WireType::Fixed32 => self.advance(4)?,
        }
        Ok(())
    }

    fn advance(&mut self, n: usize) -> Result<(), WireError> {
        let end = self.pos.checked_add(n).ok_or(WireError::LengthOutOfRange)?;
        if end > self.buf.len() {
            return Err(WireError::UnexpectedEof);
        }
        self.pos = end;
        Ok(())
    }

    fn read_raw_varint(&mut self) -> Result<u64, WireError> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = *self.buf.get(self.pos).ok_or(WireError::UnexpectedEof)?;
            self.pos += 1;
            // A varint is at most 10 bytes (64 bits / 7 bits-per-byte, rounded up).
            if shift >= 64 {
                return Err(WireError::VarintOverflow);
            }
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }
}

/// Encode an unsigned value as a protobuf base-128 varint. Test-only inverse of
/// [`Reader::read_varint`], kept here so the round-trip stays in one place.
#[cfg(test)]
fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Build a field tag from a field number and wire type. Test-only helper.
#[cfg(test)]
fn tag(number: u32, wire_type: WireType) -> u64 {
    let w = match wire_type {
        WireType::Varint => 0,
        WireType::Fixed64 => 1,
        WireType::Len => 2,
        WireType::Fixed32 => 5,
    };
    (u64::from(number) << 3) | w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny encoder used only to build hand-authored fixtures for the decoder
    /// tests. We control every byte; this is the inverse of the read path.
    struct Enc(Vec<u8>);
    impl Enc {
        fn new() -> Self {
            Enc(Vec::new())
        }
        fn varint_field(mut self, number: u32, value: u64) -> Self {
            encode_varint(tag(number, WireType::Varint), &mut self.0);
            encode_varint(value, &mut self.0);
            self
        }
        fn len_field(mut self, number: u32, payload: &[u8]) -> Self {
            encode_varint(tag(number, WireType::Len), &mut self.0);
            encode_varint(payload.len() as u64, &mut self.0);
            self.0.extend_from_slice(payload);
            self
        }
        fn fixed32_field(mut self, number: u32, value: u32) -> Self {
            encode_varint(tag(number, WireType::Fixed32), &mut self.0);
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }
        fn done(self) -> Vec<u8> {
            self.0
        }
    }

    #[test]
    fn decodes_single_varint_field() {
        let bytes = Enc::new().varint_field(3, 300).done();
        let mut r = Reader::new(&bytes);
        let h = r.next_field().unwrap().unwrap();
        assert_eq!(h.number, 3);
        assert_eq!(h.wire_type, WireType::Varint);
        assert_eq!(r.read_varint().unwrap(), 300);
        assert!(r.next_field().unwrap().is_none());
    }

    #[test]
    fn decodes_length_delimited_string() {
        let bytes = Enc::new().len_field(1, b"cgx_core").done();
        let mut r = Reader::new(&bytes);
        let h = r.next_field().unwrap().unwrap();
        assert_eq!(h.number, 1);
        assert_eq!(h.wire_type, WireType::Len);
        assert_eq!(r.read_str().unwrap(), "cgx_core");
    }

    #[test]
    fn decodes_embedded_sub_message() {
        let inner = Enc::new().len_field(1, b"hello").done();
        let outer = Enc::new().len_field(2, &inner).done();
        let mut r = Reader::new(&outer);
        let h = r.next_field().unwrap().unwrap();
        assert_eq!(h.number, 2);
        let mut sub = r.read_message().unwrap();
        let sh = sub.next_field().unwrap().unwrap();
        assert_eq!(sh.number, 1);
        assert_eq!(sub.read_str().unwrap(), "hello");
    }

    #[test]
    fn decodes_repeated_fields_in_order() {
        let bytes = Enc::new()
            .len_field(2, b"a.rs")
            .len_field(2, b"b.rs")
            .len_field(2, b"c.rs")
            .done();
        let mut r = Reader::new(&bytes);
        let mut paths = Vec::new();
        while let Some(h) = r.next_field().unwrap() {
            assert_eq!(h.number, 2);
            paths.push(r.read_str().unwrap().to_string());
        }
        assert_eq!(paths, ["a.rs", "b.rs", "c.rs"]);
    }

    #[test]
    fn skips_unknown_varint_field() {
        // Field 99 (unknown, varint) sits between two fields we care about.
        let bytes = Enc::new()
            .len_field(1, b"first")
            .varint_field(99, 123_456)
            .len_field(3, b"third")
            .done();
        let mut r = Reader::new(&bytes);
        let mut seen = Vec::new();
        while let Some(h) = r.next_field().unwrap() {
            match h.number {
                1 | 3 => seen.push(r.read_str().unwrap().to_string()),
                _ => r.skip(h.wire_type).unwrap(),
            }
        }
        assert_eq!(seen, ["first", "third"]);
    }

    #[test]
    fn skips_unknown_length_and_fixed32_fields() {
        let bytes = Enc::new()
            .len_field(50, b"ignore me")
            .fixed32_field(51, 0xdead_beef)
            .varint_field(2, 7)
            .done();
        let mut r = Reader::new(&bytes);
        let mut value = None;
        while let Some(h) = r.next_field().unwrap() {
            match h.number {
                2 => value = Some(r.read_varint().unwrap()),
                _ => r.skip(h.wire_type).unwrap(),
            }
        }
        assert_eq!(value, Some(7));
    }

    #[test]
    fn truncated_varint_is_eof_not_panic() {
        // 0x80 sets the continuation bit but the buffer ends.
        let bytes = [0x08u8, 0x80];
        let mut r = Reader::new(&bytes);
        let h = r.next_field().unwrap().unwrap();
        assert_eq!(h.wire_type, WireType::Varint);
        assert_eq!(r.read_varint(), Err(WireError::UnexpectedEof));
    }

    #[test]
    fn length_past_end_is_error_not_panic() {
        // Field 1, len-delimited, claims 200 bytes but only 1 follows.
        let mut bytes = Vec::new();
        encode_varint(tag(1, WireType::Len), &mut bytes);
        encode_varint(200, &mut bytes);
        bytes.push(b'x');
        let mut r = Reader::new(&bytes);
        r.next_field().unwrap().unwrap();
        assert_eq!(r.read_len(), Err(WireError::LengthOutOfRange));
    }

    #[test]
    fn varint_overflow_is_rejected() {
        // 11 continuation bytes — more than a u64 can hold.
        let bytes = [0x08u8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        let mut r = Reader::new(&bytes);
        r.next_field().unwrap().unwrap();
        assert_eq!(r.read_varint(), Err(WireError::VarintOverflow));
    }

    #[test]
    fn roundtrip_varint_values() {
        for v in [0u64, 1, 127, 128, 300, 16_384, u32::MAX as u64, u64::MAX] {
            let mut out = Vec::new();
            encode_varint(v, &mut out);
            let mut r = Reader::new(&out);
            assert_eq!(r.read_varint().unwrap(), v, "roundtrip failed for {v}");
            assert!(r.is_empty());
        }
    }
}
