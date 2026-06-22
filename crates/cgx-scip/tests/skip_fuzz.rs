//! Property test: arbitrary unknown fields interleaved with the fields cgx
//! cares about are skipped without error, and the known fields still decode.

use cgx_scip::ScipIndex;
use proptest::prelude::*;

const WIRE_VARINT: u64 = 0;
const WIRE_LEN: u64 = 2;
const WIRE_FIXED64: u64 = 1;
const WIRE_FIXED32: u64 = 5;

fn varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

fn tag(num: u32, wire: u64, out: &mut Vec<u8>) {
    varint((u64::from(num) << 3) | wire, out);
}

/// One arbitrary unknown field with a field number outside cgx's known set
/// (we keep numbers >= 100 so they never collide with Index's 1/2/3).
#[derive(Debug, Clone)]
enum UnknownField {
    Varint(u32, u64),
    Len(u32, Vec<u8>),
    Fixed64(u32, u64),
    Fixed32(u32, u32),
}

fn unknown_field_strategy() -> impl Strategy<Value = UnknownField> {
    let num = 100u32..1000;
    prop_oneof![
        (num.clone(), any::<u64>()).prop_map(|(n, v)| UnknownField::Varint(n, v)),
        (num.clone(), prop::collection::vec(any::<u8>(), 0..32))
            .prop_map(|(n, b)| UnknownField::Len(n, b)),
        (num.clone(), any::<u64>()).prop_map(|(n, v)| UnknownField::Fixed64(n, v)),
        (num, any::<u32>()).prop_map(|(n, v)| UnknownField::Fixed32(n, v)),
    ]
}

fn write_unknown(f: &UnknownField, out: &mut Vec<u8>) {
    match f {
        UnknownField::Varint(n, v) => {
            tag(*n, WIRE_VARINT, out);
            varint(*v, out);
        }
        UnknownField::Len(n, b) => {
            tag(*n, WIRE_LEN, out);
            varint(b.len() as u64, out);
            out.extend_from_slice(b);
        }
        UnknownField::Fixed64(n, v) => {
            tag(*n, WIRE_FIXED64, out);
            out.extend_from_slice(&v.to_le_bytes());
        }
        UnknownField::Fixed32(n, v) => {
            tag(*n, WIRE_FIXED32, out);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

proptest! {
    #[test]
    fn unknown_fields_skipped_known_fields_survive(
        unknowns in prop::collection::vec(unknown_field_strategy(), 0..20),
    ) {
        // Build an Index with: a project_root (known), arbitrary unknown fields
        // sprinkled in, and one document with a known relative_path.
        let mut meta = Vec::new();
        tag(3, WIRE_LEN, &mut meta); // Metadata.project_root
        varint(5, &mut meta);
        meta.extend_from_slice(b"/root");

        let mut doc = Vec::new();
        tag(1, WIRE_LEN, &mut doc); // Document.relative_path
        varint(4, &mut doc);
        doc.extend_from_slice(b"x.rs");

        let mut idx = Vec::new();
        // Interleave unknown fields before, between, and after known ones.
        let mid = unknowns.len() / 2;
        for f in &unknowns[..mid] {
            write_unknown(f, &mut idx);
        }
        tag(1, WIRE_LEN, &mut idx); // Index.metadata
        varint(meta.len() as u64, &mut idx);
        idx.extend_from_slice(&meta);
        for f in &unknowns[mid..] {
            write_unknown(f, &mut idx);
        }
        tag(2, WIRE_LEN, &mut idx); // Index.documents
        varint(doc.len() as u64, &mut idx);
        idx.extend_from_slice(&doc);

        let index = ScipIndex::parse(&idx).expect("unknown fields must not break parse");
        prop_assert_eq!(index.metadata.project_root, "/root");
        prop_assert_eq!(index.documents.len(), 1);
        prop_assert_eq!(&index.documents[0].relative_path, "x.rs");
    }

    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        // The decoder must return Ok or Err, never panic, on arbitrary input.
        let _ = ScipIndex::parse(&bytes);
    }
}
