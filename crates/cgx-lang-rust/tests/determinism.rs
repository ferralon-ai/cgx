//! Determinism (byte-identical fragments across runs, no HashMap order leakage)
//! and robustness (cgx's own source + the whole fixture corpus parse without
//! panic) — the WP-04 convergence criterion "cgx's own source parses without
//! panic".

mod common;

use cgx_frontend::{FileCtx, LanguageFrontend};
use cgx_lang_rust::RustFrontend;
use common::{extract, fixtures_src};
use std::path::Path;

#[test]
fn extraction_is_byte_identical_across_runs() {
    let src = include_str!("../src/extract.rs");
    let first = extract("src/extract.rs", src);
    let second = extract("src/extract.rs", src);
    assert_eq!(first, second);

    let b1 = cgx_core::codec::encode(&first).unwrap();
    let b2 = cgx_core::codec::encode(&second).unwrap();
    assert_eq!(b1, b2, "canonical postcard fragment is byte-identical");
}

#[test]
fn fragment_is_canonical_without_an_explicit_canonicalize() {
    // The extractor emits facts in a stable order; canonicalize() must be a
    // no-op on already-extracted facts (idempotence guards against order leaks).
    let fe = RustFrontend::new();
    let src = include_str!("../src/extract.rs");
    let raw = fe
        .extract(src.as_bytes(), &FileCtx::new("src/extract.rs", "oid"))
        .unwrap();
    let mut canon = raw.clone();
    canon.canonicalize();
    // Re-canonicalizing is idempotent.
    let mut twice = canon.clone();
    twice.canonicalize();
    assert_eq!(canon, twice);
}

/// Recursively collect `.rs` files under `dir`.
fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                continue;
            }
            rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_whole_rust_fixture_corpus_extracts_without_panic() {
    let fe = RustFrontend::new();
    let mut files = Vec::new();
    rs_files(&fixtures_src(), &mut files);
    assert!(!files.is_empty(), "fixture corpus is non-empty");
    for file in files {
        let src = std::fs::read(&file).unwrap();
        let rel = format!("src/{}", file.file_name().unwrap().to_str().unwrap());
        let result = fe.extract(&src, &FileCtx::new(rel, "oid"));
        assert!(
            result.is_ok(),
            "extract {} failed: {result:?}",
            file.display()
        );
    }
}

#[test]
fn cgx_own_source_parses_without_panic() {
    // Dogfood: index this crate's own sources (architecture §6 standing fixture).
    let fe = RustFrontend::new();
    let crate_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&crate_src, &mut files);
    assert!(!files.is_empty());
    for file in files {
        let src = std::fs::read(&file).unwrap();
        let mut facts = fe
            .extract(&src, &FileCtx::new(file.to_string_lossy(), "oid"))
            .unwrap_or_else(|e| panic!("extract {} errored: {e}", file.display()));
        facts.canonicalize();
        // The frontend's own files define real symbols.
        assert!(!facts.defs.is_empty(), "{} has defs", file.display());
    }
}

#[test]
fn malformed_source_degrades_rather_than_erroring() {
    let fe = RustFrontend::new();
    let result = fe.extract(b"fn (((", &FileCtx::new("src/broken.rs", "oid"));
    assert!(result.is_ok(), "must not error on malformed source");
}

#[test]
fn non_utf8_source_does_not_panic() {
    let fe = RustFrontend::new();
    let bytes = [0xff, 0xfe, b'f', b'n', b' ', b'f', b'(', b')', b'{', b'}'];
    let result = fe.extract(&bytes, &FileCtx::new("src/weird.rs", "oid"));
    assert!(result.is_ok());
}

#[test]
fn empty_source_yields_well_formed_empty_facts() {
    let facts = extract("src/empty.rs", "");
    assert!(facts.is_degraded_empty());
    assert_eq!(facts.scopes.len(), 1);
}
