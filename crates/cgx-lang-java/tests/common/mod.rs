//! Shared test helpers for the Java frontend suite.
#![allow(dead_code)]

use cgx_core::condition::EdgeCondition;
use cgx_frontend::{FileCtx, FileFacts, LanguageFrontend, RefKind, SymbolDef};
use cgx_lang_java::JavaFrontend;
use std::path::PathBuf;

/// Extract canonical facts from an inline Java source string at a repo-relative path.
///
/// The Java FQN prefix is read from the source `package_declaration`, so the
/// `rel_path` only labels spans — it does not drive the FQN like the Go adapter.
pub fn extract(rel_path: &str, src: &str) -> FileFacts {
    let fe = JavaFrontend::new();
    let mut facts = fe
        .extract(src.as_bytes(), &FileCtx::new(rel_path, "test-oid"))
        .expect("java extract must not error on valid source");
    facts.canonicalize();
    facts
}

pub fn fixtures_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/java")
        .canonicalize()
        .expect("fixtures/java must exist")
}

/// Extract a fixture file by name (e.g. "Shapes.java").
pub fn extract_fixture(rel: &str) -> FileFacts {
    let file = fixtures_src().join(rel);
    let src =
        std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    extract(rel, &src)
}

pub fn def_fqns(facts: &FileFacts) -> Vec<&str> {
    facts.defs.iter().map(|d| d.fqn.as_str()).collect()
}

pub fn def<'a>(facts: &'a FileFacts, fqn: &str) -> &'a SymbolDef {
    facts
        .defs
        .iter()
        .find(|d| d.fqn == fqn)
        .unwrap_or_else(|| panic!("no def {fqn:?}; have {:?}", def_fqns(facts)))
}

/// Whether a ref to `callee_last` (the final `name_path` segment) of the given
/// kind and edge condition was emitted.
pub fn has_ref(facts: &FileFacts, callee_last: &str, kind: RefKind, cond: EdgeCondition) -> bool {
    facts.refs.iter().any(|r| {
        r.name_path.last().map(String::as_str) == Some(callee_last)
            && r.kind == kind
            && r.edge_condition == cond
    })
}
