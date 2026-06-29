//! Shared test helpers for the Go frontend suite.
#![allow(dead_code)]

use cgx_core::condition::EdgeCondition;
use cgx_frontend::{FileCtx, FileFacts, LanguageFrontend, RefKind, SymbolDef};
use cgx_lang_go::GoFrontend;
use std::path::PathBuf;

/// Extract canonical facts from an inline Go source string at a repo-relative path.
pub fn extract(rel_path: &str, src: &str) -> FileFacts {
    let fe = GoFrontend::new();
    let mut facts = fe
        .extract(src.as_bytes(), &FileCtx::new(rel_path, "test-oid"))
        .expect("go extract must not error on valid source");
    facts.canonicalize();
    facts
}

pub fn fixtures_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/go")
        .canonicalize()
        .expect("fixtures/go must exist")
}

/// Extract a fixture file by repo-relative subpath (e.g. "interfaces.go").
pub fn extract_fixture(rel: &str) -> FileFacts {
    let file = fixtures_src().join(rel);
    let src =
        std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    // Fixture FQNs use the module short-name "go_sample".
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

pub fn has_ref(facts: &FileFacts, callee_last: &str, kind: RefKind, cond: EdgeCondition) -> bool {
    facts.refs.iter().any(|r| {
        r.name_path.last().map(String::as_str) == Some(callee_last)
            && r.kind == kind
            && r.edge_condition == cond
    })
}
