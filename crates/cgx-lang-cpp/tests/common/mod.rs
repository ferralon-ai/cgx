//! Shared test helpers for the C++ frontend suite.
#![allow(dead_code)]

use cgx_core::condition::EdgeCondition;
use cgx_core::confidence::Confidence;
use cgx_core::edge::EdgeKind;
use cgx_frontend::{CutHint, FileCtx, FileFacts, LanguageFrontend, RawRef, RefKind, SymbolDef};
use cgx_lang_cpp::CppFrontend;
use cgx_resolve::{link, FileInput, LinkOpts, ResolvedGraph};

/// Extract canonical facts from an inline C++ source string at a repo-relative path.
pub fn extract(rel_path: &str, src: &str) -> FileFacts {
    let fe = CppFrontend::new();
    let mut facts = fe
        .extract(src.as_bytes(), &FileCtx::new(rel_path, format!("oid-{rel_path}")))
        .expect("cpp extract must not error on valid source");
    facts.canonicalize();
    facts
}

/// Link a set of `(path, facts)` into a resolved graph with the production
/// default options (name+arity fallback on — same as `cgx index`).
pub fn link_files<'a>(files: &'a [(&str, &'a FileFacts)]) -> ResolvedGraph {
    let inputs: Vec<FileInput<'a>> = files
        .iter()
        .map(|(path, facts)| FileInput::new(format!("blob-{path}"), *path, "cpp", *facts))
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

pub fn cut_hints(facts: &FileFacts) -> &[CutHint] {
    &facts.cut_hints
}

// --- resolved-graph helpers (confidence / edge-kind pins) ---

fn short(fqn: &str) -> &str {
    fqn.rsplit("::").next().unwrap_or(fqn)
}

/// FQN of the node at a given index.
fn fqn_at(graph: &ResolvedGraph, idx: usize) -> String {
    graph
        .node_records()
        .nth(idx)
        .map(|n| n.fqn.clone())
        .unwrap_or_default()
}

/// The confidence(s) of edges from `caller_short`'s body to `callee_short`.
pub fn confidences_from_to(
    graph: &ResolvedGraph,
    caller_short: &str,
    callee_short: &str,
) -> Vec<Confidence> {
    graph
        .edge_records()
        .filter(|e| {
            short(&fqn_at(graph, e.src.index())) == caller_short
                && short(&fqn_at(graph, e.dst.index())) == callee_short
        })
        .map(|e| e.confidence)
        .collect()
}

/// The `(edge_kind, confidence)` of every edge whose destination short name is
/// `callee_short`.
pub fn edges_to_short(graph: &ResolvedGraph, callee_short: &str) -> Vec<(EdgeKind, Confidence)> {
    graph
        .edge_records()
        .filter(|e| short(&fqn_at(graph, e.dst.index())) == callee_short)
        .map(|e| (e.kind, e.confidence))
        .collect()
}

pub fn count_nodes_short(graph: &ResolvedGraph, s: &str) -> usize {
    graph
        .node_records()
        .filter(|n| short(&n.fqn) == s)
        .count()
}

pub fn has_structural_edge(
    graph: &ResolvedGraph,
    kind: EdgeKind,
    src_short: &str,
    dst_short: &str,
) -> bool {
    graph.edge_records().any(|e| {
        e.kind == kind
            && short(&fqn_at(graph, e.src.index())) == src_short
            && short(&fqn_at(graph, e.dst.index())) == dst_short
    })
}
