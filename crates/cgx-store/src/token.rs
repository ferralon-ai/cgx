//! A minimal serde serializer that extracts the string token of a unit enum
//! variant, without depending on `serde_json`.
//!
//! The `cgx-core` model enums (`SymbolKind`, `EdgeKind`, `EdgeCondition`, …) all
//! `#[serde(rename_all = ...)]` to unit string variants. The denormalized view
//! columns store that exact token, so `--sql` against the `v_*` views speaks the
//! same vocabulary as JSON output. This serializer captures only the unit-variant
//! name and rejects anything else (which never occurs for these enums).

use serde::ser::{self, Serialize};

/// Serialize `value` to its unit-variant string token.
///
/// Panics only on a programming error: calling it on a non-unit-enum value. All
/// call sites pass `cgx-core` model enums, which are unit-variant.
pub fn to_token<T: Serialize>(value: &T) -> String {
    let mut s = TokenSerializer { out: None };
    value
        .serialize(&mut s)
        .expect("model enums serialize as unit variants");
    s.out.expect("a token was produced")
}

struct TokenSerializer {
    out: Option<String>,
}

/// Error for the token serializer.
#[derive(Debug)]
struct TokenError(String);

impl core::fmt::Display for TokenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TokenError {}

impl ser::Error for TokenError {
    fn custom<T: core::fmt::Display>(msg: T) -> Self {
        TokenError(msg.to_string())
    }
}

fn unsupported(what: &str) -> TokenError {
    TokenError(format!(
        "to_token only supports unit enum variants, got {what}"
    ))
}

impl ser::Serializer for &mut TokenSerializer {
    type Ok = ();
    type Error = TokenError;
    type SerializeSeq = ser::Impossible<(), TokenError>;
    type SerializeTuple = ser::Impossible<(), TokenError>;
    type SerializeTupleStruct = ser::Impossible<(), TokenError>;
    type SerializeTupleVariant = ser::Impossible<(), TokenError>;
    type SerializeMap = ser::Impossible<(), TokenError>;
    type SerializeStruct = ser::Impossible<(), TokenError>;
    type SerializeStructVariant = ser::Impossible<(), TokenError>;

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), TokenError> {
        self.out = Some(variant.to_owned());
        Ok(())
    }

    fn serialize_str(self, v: &str) -> Result<(), TokenError> {
        self.out = Some(v.to_owned());
        Ok(())
    }

    fn serialize_bool(self, _v: bool) -> Result<(), TokenError> {
        Err(unsupported("bool"))
    }
    fn serialize_i8(self, _v: i8) -> Result<(), TokenError> {
        Err(unsupported("i8"))
    }
    fn serialize_i16(self, _v: i16) -> Result<(), TokenError> {
        Err(unsupported("i16"))
    }
    fn serialize_i32(self, _v: i32) -> Result<(), TokenError> {
        Err(unsupported("i32"))
    }
    fn serialize_i64(self, _v: i64) -> Result<(), TokenError> {
        Err(unsupported("i64"))
    }
    fn serialize_u8(self, _v: u8) -> Result<(), TokenError> {
        Err(unsupported("u8"))
    }
    fn serialize_u16(self, _v: u16) -> Result<(), TokenError> {
        Err(unsupported("u16"))
    }
    fn serialize_u32(self, _v: u32) -> Result<(), TokenError> {
        Err(unsupported("u32"))
    }
    fn serialize_u64(self, _v: u64) -> Result<(), TokenError> {
        Err(unsupported("u64"))
    }
    fn serialize_f32(self, _v: f32) -> Result<(), TokenError> {
        Err(unsupported("f32"))
    }
    fn serialize_f64(self, _v: f64) -> Result<(), TokenError> {
        Err(unsupported("f64"))
    }
    fn serialize_char(self, _v: char) -> Result<(), TokenError> {
        Err(unsupported("char"))
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<(), TokenError> {
        Err(unsupported("bytes"))
    }
    fn serialize_none(self) -> Result<(), TokenError> {
        Err(unsupported("none"))
    }
    fn serialize_some<T: ?Sized + Serialize>(self, _v: &T) -> Result<(), TokenError> {
        Err(unsupported("some"))
    }
    fn serialize_unit(self) -> Result<(), TokenError> {
        Err(unsupported("unit"))
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), TokenError> {
        Err(unsupported("unit struct"))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<(), TokenError> {
        Err(unsupported("newtype struct"))
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<(), TokenError> {
        Err(unsupported("newtype variant"))
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, TokenError> {
        Err(unsupported("seq"))
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, TokenError> {
        Err(unsupported("tuple"))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, TokenError> {
        Err(unsupported("tuple struct"))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, TokenError> {
        Err(unsupported("tuple variant"))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, TokenError> {
        Err(unsupported("map"))
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, TokenError> {
        Err(unsupported("struct"))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, TokenError> {
        Err(unsupported("struct variant"))
    }
}
