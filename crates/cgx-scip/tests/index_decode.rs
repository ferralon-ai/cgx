//! Integration tests for the typed `ScipIndex` model and `ScipResolver`,
//! driven entirely by hand-authored protobuf wire messages (decision R7 — no
//! external toolchain needed). A small encoder mirrors the SCIP `.proto` field
//! numbers so we control every byte.

use cgx_scip::{ScipIndex, ScipResolver, SymClass};

mod enc {
    //! Minimal protobuf encoder for SCIP messages — test-only mirror of the
    //! field numbers in `cgx_scip::lib`. We hand-build bytes the decoder reads.

    pub const WIRE_VARINT: u64 = 0;
    pub const WIRE_LEN: u64 = 2;

    pub fn varint(mut v: u64, out: &mut Vec<u8>) {
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

    pub fn tag(num: u32, wire: u64, out: &mut Vec<u8>) {
        varint((u64::from(num) << 3) | wire, out);
    }

    pub fn varint_field(num: u32, v: u64, out: &mut Vec<u8>) {
        tag(num, WIRE_VARINT, out);
        varint(v, out);
    }

    pub fn len_field(num: u32, payload: &[u8], out: &mut Vec<u8>) {
        tag(num, WIRE_LEN, out);
        varint(payload.len() as u64, out);
        out.extend_from_slice(payload);
    }

    pub fn str_field(num: u32, s: &str, out: &mut Vec<u8>) {
        len_field(num, s.as_bytes(), out);
    }

    /// Build an Occurrence sub-message: range (packed int32, field 1), symbol
    /// (field 2), symbol_roles (field 3).
    pub fn occurrence(symbol: &str, roles: i64, range: &[i32]) -> Vec<u8> {
        let mut occ = Vec::new();
        let mut packed = Vec::new();
        for &i in range {
            varint(i as u32 as u64, &mut packed);
        }
        len_field(1, &packed, &mut occ);
        str_field(2, symbol, &mut occ);
        varint_field(3, roles as u64, &mut occ);
        occ
    }

    /// Build a SymbolInformation sub-message: symbol (1), kind (5),
    /// enclosing_symbol (8).
    pub fn symbol_info(symbol: &str, kind: i64, enclosing: &str) -> Vec<u8> {
        let mut si = Vec::new();
        str_field(1, symbol, &mut si);
        varint_field(5, kind as u64, &mut si);
        if !enclosing.is_empty() {
            str_field(8, enclosing, &mut si);
        }
        si
    }

    /// Build a ToolInfo with just a name (field 1).
    pub fn tool_info(name: &str) -> Vec<u8> {
        let mut ti = Vec::new();
        str_field(1, name, &mut ti);
        ti
    }

    /// Build a Metadata with tool_info (field 2) + project_root (field 3).
    pub fn metadata(tool: &str, root: &str) -> Vec<u8> {
        let mut m = Vec::new();
        len_field(2, &tool_info(tool), &mut m);
        str_field(3, root, &mut m);
        m
    }
}

const DEF: i64 = 0x1;
const REF: i64 = 0x0;
const KIND_TRAIT: i64 = 64;

#[test]
fn parses_metadata_documents_and_external_symbols() {
    let mut idx = Vec::new();
    enc::len_field(1, &enc::metadata("rust-analyzer", "/repo"), &mut idx);

    // Document a.rs with one definition + one reference occurrence and a symbol.
    let mut doc = Vec::new();
    enc::str_field(1, "a.rs", &mut doc);
    enc::len_field(
        2,
        &enc::occurrence(
            "rust-analyzer cargo mycrate 1.0.0 parse().",
            DEF,
            &[10, 4, 9],
        ),
        &mut doc,
    );
    enc::len_field(
        2,
        &enc::occurrence(
            "rust-analyzer cargo mycrate 1.0.0 parse().",
            REF,
            &[20, 8, 13],
        ),
        &mut doc,
    );
    enc::len_field(
        3,
        &enc::symbol_info("rust-analyzer cargo mycrate 1.0.0 parse().", 0, ""),
        &mut doc,
    );
    enc::len_field(2, &doc, &mut idx);

    // External symbol.
    enc::len_field(
        3,
        &enc::symbol_info("rust-analyzer cargo serde 1.0.0 Deserialize#", KIND_TRAIT, ""),
        &mut idx,
    );

    let index = ScipIndex::parse(&idx).expect("parse");
    assert_eq!(index.metadata.tool, "rust-analyzer");
    assert_eq!(index.metadata.project_root, "/repo");
    assert_eq!(index.documents.len(), 1);
    assert_eq!(index.documents[0].relative_path, "a.rs");
    assert_eq!(index.documents[0].occurrences.len(), 2);
    assert!(index.documents[0].occurrences[0].is_definition());
    assert!(!index.documents[0].occurrences[1].is_definition());
    assert_eq!(index.external_symbols.len(), 1);
    assert_eq!(index.external_symbols[0].kind as i64, KIND_TRAIT);
}

#[test]
fn resolver_exposes_def_sites_refs_and_classify() {
    let mut idx = Vec::new();

    let mut doc = Vec::new();
    enc::str_field(1, "lib.rs", &mut doc);
    // free fn definition + reference
    enc::len_field(
        2,
        &enc::occurrence("rust-analyzer cargo mycrate 1.0.0 parse().", DEF, &[1, 0, 5]),
        &mut doc,
    );
    enc::len_field(
        2,
        &enc::occurrence("rust-analyzer cargo mycrate 1.0.0 parse().", REF, &[9, 4, 9]),
        &mut doc,
    );
    // trait method reference; the trait type is recorded as a Trait symbol.
    enc::len_field(
        2,
        &enc::occurrence(
            "rust-analyzer cargo mycrate 1.0.0 shape/Shape#area().",
            REF,
            &[12, 6, 12],
        ),
        &mut doc,
    );
    enc::len_field(
        3,
        &enc::symbol_info("rust-analyzer cargo mycrate 1.0.0 shape/Shape#", KIND_TRAIT, ""),
        &mut doc,
    );
    enc::len_field(2, &doc, &mut idx);

    let resolver = ScipResolver::from_bytes(&idx).expect("parse");

    // def_sites: parse is defined exactly once → unique.
    let defs = resolver.def_sites("mycrate::parse");
    assert_eq!(defs.len(), 1, "free fn should have one def-site");
    assert_eq!(defs[0].relative_path, "lib.rs");

    // refs_in_doc: two references (parse ref + Shape::area ref), sorted.
    let refs = resolver.refs_in_doc("lib.rs");
    assert_eq!(refs.len(), 2);
    // sorted by range first; (9,4,..) < (12,6,..)
    assert_eq!(refs[0].0.start_line, 9);
    assert_eq!(refs[1].0.start_line, 12);

    // classify: free fn vs trait member.
    assert_eq!(
        resolver.classify("rust-analyzer cargo mycrate 1.0.0 parse()."),
        SymClass::FreeOrInherent
    );
    assert_eq!(
        resolver.classify("rust-analyzer cargo mycrate 1.0.0 shape/Shape#area()."),
        SymClass::TraitMember
    );
}

#[test]
fn collision_18772_flags_non_unique_def_site() {
    // Same qname defined twice (two inherent impl blocks → #18772) → def_sites
    // returns len 2; downstream caps at probable.
    let mut idx = Vec::new();
    let mut doc = Vec::new();
    enc::str_field(1, "a.rs", &mut doc);
    enc::len_field(
        2,
        &enc::occurrence("rust-analyzer cargo mycrate 1.0.0 Foo#bar().", DEF, &[1, 0, 3]),
        &mut doc,
    );
    enc::len_field(2, &doc, &mut idx);

    let mut doc2 = Vec::new();
    enc::str_field(1, "b.rs", &mut doc2);
    enc::len_field(
        2,
        &enc::occurrence("rust-analyzer cargo mycrate 1.0.0 Foo#bar().", DEF, &[5, 0, 3]),
        &mut doc2,
    );
    enc::len_field(2, &doc2, &mut idx);

    let resolver = ScipResolver::from_bytes(&idx).expect("parse");
    let defs = resolver.def_sites("mycrate::Foo::bar");
    assert_eq!(defs.len(), 2, "#18772 collision must surface >1 def-site");
}

#[test]
fn parses_multiline_range() {
    let mut idx = Vec::new();
    let mut doc = Vec::new();
    enc::str_field(1, "a.rs", &mut doc);
    enc::len_field(
        2,
        // 4-int packed range → multi-line [sl, sc, el, ec]
        &enc::occurrence("rust-analyzer cargo c 1.0.0 f().", REF, &[3, 2, 7, 9]),
        &mut doc,
    );
    enc::len_field(2, &doc, &mut idx);

    let index = ScipIndex::parse(&idx).expect("parse");
    let occ = &index.documents[0].occurrences[0];
    assert_eq!(occ.range.start_line, 3);
    assert_eq!(occ.range.start_char, 2);
    assert_eq!(occ.range.end_line, 7);
    assert_eq!(occ.range.end_char, 9);
}

#[test]
fn real_scip_fixture_when_present() {
    // Gated behind the committed blob's presence (decision R7); the suite is
    // green without rust-analyzer / the binary fixture.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/scip-sample.scip");
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return, // fixture not generated yet — skip.
    };
    let resolver = ScipResolver::from_bytes(&bytes).expect("real fixture should parse");
    assert!(
        !resolver.def_sites("scip-sample::parse").is_empty(),
        "expected free fn `parse` definition in the real fixture"
    );
}
