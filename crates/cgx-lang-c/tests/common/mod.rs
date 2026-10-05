//! Shared test helpers for the C frontend suite.
#![allow(dead_code)]

use cgx_core::condition::EdgeCondition;
use cgx_frontend::{CutHint, FileCtx, FileFacts, LanguageFrontend, RawRef, RefKind, SymbolDef};
use cgx_lang_c::CFrontend;
use cgx_resolve::{link, FileInput, LinkOpts, ResolvedGraph};

/// Extract canonical facts from an inline C source string at a repo-relative path.
pub fn extract(rel_path: &str, src: &str) -> FileFacts {
    let fe = CFrontend::new();
    let mut facts = fe
        .extract(src.as_bytes(), &FileCtx::new(rel_path, format!("oid-{rel_path}")))
        .expect("c extract must not error on valid source");
    facts.canonicalize();
    facts
}

/// Link a set of `(path, facts)` into a resolved graph with the production
/// default options (name+arity fallback on — same as `cgx index`).
pub fn link_files<'a>(files: &'a [(&str, &'a FileFacts)]) -> ResolvedGraph {
    let inputs: Vec<FileInput<'a>> = files
        .iter()
        .map(|(path, facts)| FileInput::new(format!("blob-{path}"), *path, "c", *facts))
        .collect();
    link(&inputs, &LinkOpts::default())
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

pub fn has_def(facts: &FileFacts, fqn: &str) -> bool {
    facts.defs.iter().any(|d| d.fqn == fqn)
}

/// The first ref whose callee last-segment matches `callee_last`.
pub fn find_ref<'a>(facts: &'a FileFacts, callee_last: &str) -> Option<&'a RawRef> {
    facts
        .refs
        .iter()
        .find(|r| r.name_path.last().map(String::as_str) == Some(callee_last))
}

pub fn has_ref(facts: &FileFacts, callee_last: &str, kind: RefKind, cond: EdgeCondition) -> bool {
    facts.refs.iter().any(|r| {
        r.name_path.last().map(String::as_str) == Some(callee_last)
            && r.kind == kind
            && r.edge_condition == cond
    })
}

pub fn cut_hints<'a>(facts: &'a FileFacts) -> &'a [CutHint] {
    &facts.cut_hints
}
