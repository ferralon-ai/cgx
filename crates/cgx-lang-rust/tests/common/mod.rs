//! Shared test helpers for the Rust frontend suite.
//!
//! Each integration-test file pulls in this module via `mod common;` but uses
//! only a subset of the helpers, so unused-warnings are expected and silenced.
#![allow(dead_code)]

use cgx_core::condition::EdgeCondition;
use cgx_core::effect::EffectSet;
use cgx_frontend::{
    FileCtx, FileFacts, LanguageFrontend, RawRef, RefKind, RelationKind, SymbolDef,
};
use cgx_lang_rust::RustFrontend;
use std::path::PathBuf;

/// Extract canonical facts from an inline Rust source string at a given
/// repo-relative path.
pub fn extract(rel_path: &str, src: &str) -> FileFacts {
    let fe = RustFrontend::new();
    let mut facts = fe
        .extract(src.as_bytes(), &FileCtx::new(rel_path, "test-oid"))
        .expect("rust extract must not error on valid source");
    facts.canonicalize();
    facts
}

/// The path to the WP-02 Rust fixture source tree.
pub fn fixtures_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/rust-sample/src")
        .canonicalize()
        .expect("fixtures/rust-sample/src must exist")
}

/// Extract facts from a fixture file by stem (e.g. `"errors"` → `errors.rs`),
/// using the repo-relative path the goldens expect (`src/<stem>.rs`).
pub fn extract_fixture(stem: &str) -> FileFacts {
    let file = fixtures_src().join(format!("{stem}.rs"));
    let src =
        std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    extract(&format!("src/{stem}.rs"), &src)
}

/// All def FQNs in canonical order.
pub fn def_fqns(facts: &FileFacts) -> Vec<&str> {
    facts.defs.iter().map(|d| d.fqn.as_str()).collect()
}

/// The single def with the given FQN.
pub fn def<'a>(facts: &'a FileFacts, fqn: &str) -> &'a SymbolDef {
    facts
        .defs
        .iter()
        .find(|d| d.fqn == fqn)
        .unwrap_or_else(|| panic!("no def {fqn:?}; have {:?}", def_fqns(facts)))
}

/// Whether any ref matches the given callee short-name, kind, and condition.
pub fn has_ref(facts: &FileFacts, callee_last: &str, kind: RefKind, cond: EdgeCondition) -> bool {
    facts.refs.iter().any(|r| {
        r.name_path.last().map(String::as_str) == Some(callee_last)
            && r.kind == kind
            && r.edge_condition == cond
    })
}

/// All refs whose callee short-name matches.
pub fn refs_to<'a>(facts: &'a FileFacts, callee_last: &str) -> Vec<&'a RawRef> {
    facts
        .refs
        .iter()
        .filter(|r| r.name_path.last().map(String::as_str) == Some(callee_last))
        .collect()
}

/// Whether an `impl_relations` entry of the given kind has the given subject and
/// object name paths (joined with `::`).
pub fn has_relation(facts: &FileFacts, kind: RelationKind, subject: &str, object: &str) -> bool {
    facts.impl_relations.iter().any(|r| {
        r.kind == kind && r.subject.join("::") == subject && r.object.join("::") == object
    })
}

/// Whether an Overrides relation exists whose subject FQN ends with
/// `subject_suffix` and whose object equals `object` (the trait method path).
/// The subject is the impl method's full FQN (module-prefixed), so callers match
/// the trailing `Type::method` segment without pinning the module derivation.
pub fn has_override_suffix(facts: &FileFacts, subject_suffix: &str, object: &str) -> bool {
    facts.impl_relations.iter().any(|r| {
        r.kind == RelationKind::Overrides
            && r.subject.join("::").ends_with(subject_suffix)
            && r.object.join("::") == object
    })
}

/// All relations of a given kind, as `(subject, object)` joined-name pairs.
pub fn relations_of(facts: &FileFacts, kind: RelationKind) -> Vec<(String, String)> {
    facts
        .impl_relations
        .iter()
        .filter(|r| r.kind == kind)
        .map(|r| (r.subject.join("::"), r.object.join("::")))
        .collect()
}

/// The detected own-effects for the symbol with the given FQN. Empty set when
/// the frontend recorded no effect fact for that FQN.
pub fn effects_of(facts: &FileFacts, fqn: &str) -> EffectSet {
    facts
        .effects
        .iter()
        .find(|e| e.fqn == fqn)
        .map(|e| e.effects)
        .unwrap_or_default()
}

/// All Instantiate refs' target name paths (joined with `::`).
pub fn instantiations(facts: &FileFacts) -> Vec<String> {
    facts
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::Instantiate)
        .map(|r| r.name_path.join("::"))
        .collect()
}
