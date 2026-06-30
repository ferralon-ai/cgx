//! Shared test helpers for the Python frontend suite.
#![allow(dead_code)]

use cgx_core::condition::EdgeCondition;
use cgx_frontend::{FileCtx, FileFacts, LanguageFrontend, RefKind, RelationKind, SymbolDef};
use cgx_lang_python::PythonFrontend;
use std::path::PathBuf;

/// Extract canonical facts from an inline Python source string at a repo-relative
/// path.
pub fn extract(rel_path: &str, src: &str) -> FileFacts {
    let fe = PythonFrontend::new();
    let mut facts = fe
        .extract(src.as_bytes(), &FileCtx::new(rel_path, "test-oid"))
        .expect("python extract must not error on valid source");
    facts.canonicalize();
    facts
}

pub fn fixtures_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/python")
        .canonicalize()
        .expect("fixtures/python must exist")
}

/// Extract a fixture file by repo-relative subpath (e.g. "shapes.py").
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

/// Whether a ref exists whose callee last segment, kind, and edge condition match.
pub fn has_ref(facts: &FileFacts, callee_last: &str, kind: RefKind, cond: EdgeCondition) -> bool {
    facts.refs.iter().any(|r| {
        r.name_path.last().map(String::as_str) == Some(callee_last)
            && r.kind == kind
            && r.edge_condition == cond
    })
}

/// Whether an `Inherits`/`Implements` relation exists with the given subject and
/// object (each compared by `::`-joined name path).
pub fn has_relation(facts: &FileFacts, kind: RelationKind, subject: &str, object: &str) -> bool {
    facts.impl_relations.iter().any(|r| {
        r.kind == kind
            && r.subject.join("::") == subject
            && r.object.join("::") == object
    })
}

/// Whether an `Overrides` relation exists whose subject FQN and object `[Base,
/// method]` path match (object compared by `::`-joined name path).
pub fn has_override(facts: &FileFacts, subject_fqn: &str, object: &str) -> bool {
    facts.impl_relations.iter().any(|r| {
        r.kind == RelationKind::Overrides
            && r.subject.join("::") == subject_fqn
            && r.object.join("::") == object
    })
}

/// Count of `Overrides` relations whose subject FQN matches.
pub fn override_count(facts: &FileFacts, subject_fqn: &str) -> usize {
    facts
        .impl_relations
        .iter()
        .filter(|r| r.kind == RelationKind::Overrides && r.subject.join("::") == subject_fqn)
        .count()
}
