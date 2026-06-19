//! Test-only SCIP `.scip` encoder, gated behind the `test-support` feature.
//!
//! This is **not** part of the production API — `cgx-scip` is read-only in prod
//! (decision R1). The encoder exists so downstream crates' hermetic tests can
//! author a synthetic-but-valid `.scip` byte stream **programmatically**, without
//! shelling out to `rust-analyzer` (which is not available in CI). It is the
//! exact inverse of the [`crate::wire`] read path: it emits the same narrow field
//! subset the decoder consumes, in standard protobuf encoding.
//!
//! Enable it from a consumer's `[dev-dependencies]`:
//!
//! ```toml
//! cgx-scip = { workspace = true, features = ["test-support"] }
//! ```

use crate::wire::WireType;

/// Encode an unsigned value as a protobuf base-128 varint.
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

/// Build a field tag from a field number and wire type.
fn tag(number: u32, wire_type: WireType) -> u64 {
    let w = match wire_type {
        WireType::Varint => 0,
        WireType::Fixed64 => 1,
        WireType::Len => 2,
        WireType::Fixed32 => 5,
    };
    (u64::from(number) << 3) | w
}

/// A minimal protobuf message builder. Fields are appended in call order; the
/// `.scip` decoder is order-insensitive within a message, so callers may build
/// fields in any order.
#[derive(Debug, Default, Clone)]
pub struct Msg(Vec<u8>);

impl Msg {
    /// A fresh, empty message.
    pub fn new() -> Self {
        Msg(Vec::new())
    }

    /// Append a varint (wire type 0) field.
    pub fn varint(mut self, number: u32, value: i64) -> Self {
        encode_varint(tag(number, WireType::Varint), &mut self.0);
        encode_varint(value as u64, &mut self.0);
        self
    }

    /// Append a length-delimited (wire type 2) field carrying a UTF-8 string.
    pub fn string(self, number: u32, s: &str) -> Self {
        self.bytes(number, s.as_bytes())
    }

    /// Append a length-delimited (wire type 2) field carrying raw bytes (a nested
    /// message or a packed repeated scalar).
    pub fn bytes(mut self, number: u32, payload: &[u8]) -> Self {
        encode_varint(tag(number, WireType::Len), &mut self.0);
        encode_varint(payload.len() as u64, &mut self.0);
        self.0.extend_from_slice(payload);
        self
    }

    /// Append an embedded sub-message under `number`.
    pub fn message(self, number: u32, sub: Msg) -> Self {
        self.bytes(number, &sub.into_bytes())
    }

    /// Append a packed `repeated int32` field (the SCIP range encoding).
    pub fn packed_i32(self, number: u32, ints: &[i32]) -> Self {
        let mut packed = Vec::new();
        for &i in ints {
            encode_varint(i as u64, &mut packed);
        }
        self.bytes(number, &packed)
    }

    /// Finish, returning the encoded bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

// --- High-level SCIP-shaped builders -----------------------------------------
//
// These mirror the field numbers in `crate` (the `F_*` constants) so a synthetic
// index round-trips through `ScipIndex::parse` / `ScipResolver`.

/// `Occurrence.symbol_roles` bit for a definition (matches
/// [`crate::SYMBOL_ROLE_DEFINITION`]).
pub const ROLE_DEFINITION: i64 = 0x1;

/// SCIP `SymbolInformation.Kind::Trait` (used to flag a trait-member ceiling).
/// Matches [`crate::symbol::SCIP_KIND_TRAIT`].
pub const KIND_TRAIT: i64 = 64;
/// SCIP `SymbolInformation.Kind::Method`.
pub const KIND_METHOD: i64 = 26;

/// Build an `Occurrence` message: `range=1` (packed int32), `symbol=2`,
/// `symbol_roles=3`.
pub fn occurrence(range: &[i32], symbol: &str, roles: i64) -> Msg {
    let mut m = Msg::new().packed_i32(1, range).string(2, symbol);
    if roles != 0 {
        m = m.varint(3, roles);
    }
    m
}

/// Build a `SymbolInformation` message: `symbol=1`, `kind=5`, `enclosing=8`.
pub fn symbol_info(symbol: &str, kind: i64, enclosing: &str) -> Msg {
    let mut m = Msg::new().string(1, symbol);
    if kind != 0 {
        m = m.varint(5, kind);
    }
    if !enclosing.is_empty() {
        m = m.string(8, enclosing);
    }
    m
}

/// Build a `Document` message: `relative_path=1`, `occurrences=2` (repeated),
/// `symbols=3` (repeated).
pub fn document(relative_path: &str, occurrences: Vec<Msg>, symbols: Vec<Msg>) -> Msg {
    let mut m = Msg::new().string(1, relative_path);
    for occ in occurrences {
        m = m.message(2, occ);
    }
    for sym in symbols {
        m = m.message(3, sym);
    }
    m
}

/// Build a top-level `Index` message: `metadata=1` (project_root + tool name),
/// `documents=2` (repeated), `external_symbols=3` (repeated).
pub fn index(project_root: &str, tool: &str, documents: Vec<Msg>, external_symbols: Vec<Msg>) -> Vec<u8> {
    // Metadata { project_root=3, tool_info=2 { name=1 } }
    let tool_info = Msg::new().string(1, tool);
    let metadata = Msg::new().string(3, project_root).message(2, tool_info);

    let mut m = Msg::new().message(1, metadata);
    for doc in documents {
        m = m.message(2, doc);
    }
    for ext in external_symbols {
        m = m.message(3, ext);
    }
    m.into_bytes()
}
