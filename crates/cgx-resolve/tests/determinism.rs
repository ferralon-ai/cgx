//! Determinism: the linked graph is a pure function of the input *set*,
//! independent of the order files are supplied (WP-06 convergence criterion).

mod common;

use cgx_core::node::{SymbolKind, Visibility};
use cgx_frontend::facts::{FileFacts, ScopeId};
use cgx_resolve::{link, FileInput, LinkOpts};
use proptest::prelude::*;

use common::FileBuilder;

const PUB: Visibility = Visibility::Public;

/// Build a small multi-file program: three files, cross-file calls and a
/// virtual-dispatch candidate set, so node ids, edge ids, and candidate groups
/// all have to be order-stable.
fn program() -> Vec<FileFacts> {
    // direct.rs
    let mut d = FileBuilder::new();
    d.def(
        "crate::direct::add",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(2),
    );
    let direct = d.build();

    // v.rs: two speak methods + a chorus that dispatches virtually.
    let mut v = FileBuilder::new();
    let cs = v.scope(ScopeId::ROOT, Some("crate::v::chorus"));
    v.def(
        "crate::v::Dog::speak",
        SymbolKind::Method,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    v.def(
        "crate::v::Cat::speak",
        SymbolKind::Method,
        ScopeId::ROOT,
        5,
        PUB,
        false,
        Some(0),
    );
    v.def(
        "crate::v::chorus",
        SymbolKind::Function,
        ScopeId::ROOT,
        9,
        PUB,
        false,
        Some(0),
    );
    v.vcall(&["a", "speak"], cs, 10);
    let vv = v.build();

    // imports.rs: imports add, calls it.
    let mut i = FileBuilder::new();
    let f = i.scope(ScopeId::ROOT, Some("crate::imports::run"));
    i.import("crate::direct", "add", None, false, ScopeId::ROOT);
    i.def(
        "crate::imports::run",
        SymbolKind::Function,
        ScopeId::ROOT,
        3,
        PUB,
        false,
        Some(0),
    );
    i.call(&["add"], f, 4);
    let imports = i.build();

    vec![direct, vv, imports]
}

fn inputs<'a>(files: &'a [FileFacts], order: &[usize]) -> Vec<FileInput<'a>> {
    let paths = ["src/direct.rs", "src/v.rs", "src/imports.rs"];
    let langs = ["rust", "rust", "rust"];
    order
        .iter()
        .map(|&i| FileInput::new(format!("blob{i}"), paths[i], langs[i], &files[i]))
        .collect()
}

#[test]
fn graph_is_independent_of_file_order() {
    let files = program();
    let g1 = link(&inputs(&files, &[0, 1, 2]), &LinkOpts::default());
    let g2 = link(&inputs(&files, &[2, 1, 0]), &LinkOpts::default());
    let g3 = link(&inputs(&files, &[1, 0, 2]), &LinkOpts::default());

    assert_eq!(
        g1.nodes, g2.nodes,
        "node ids/order stable across input order"
    );
    assert_eq!(g1.nodes, g3.nodes);
    assert_eq!(
        g1.edges, g2.edges,
        "edge ids/order stable across input order"
    );
    assert_eq!(g1.edges, g3.edges);
    assert_eq!(g1.candidates, g2.candidates, "candidate groups stable");
    assert_eq!(g1.candidates, g3.candidates);
}

#[test]
fn repeated_link_is_byte_identical() {
    let files = program();
    let order = [0, 1, 2];
    let a = link(&inputs(&files, &order), &LinkOpts::default());
    let b = link(&inputs(&files, &order), &LinkOpts::default());
    assert_eq!(
        a, b,
        "two runs of the same input produce an identical graph"
    );
}

proptest! {
    /// Any permutation of the input files yields a graph byte-identical to the
    /// canonical order (WP-06 convergence: deterministic across shuffled input).
    #[test]
    fn any_permutation_yields_the_canonical_graph(perm in Just(vec![0usize, 1, 2]).prop_shuffle()) {
        let files = program();
        let reference = link(&inputs(&files, &[0, 1, 2]), &LinkOpts::default());
        let shuffled = link(&inputs(&files, &perm), &LinkOpts::default());
        prop_assert_eq!(reference, shuffled);
    }
}

#[test]
fn node_ids_are_dense_and_sorted() {
    let files = program();
    let g = link(&inputs(&files, &[0, 1, 2]), &LinkOpts::default());
    for (i, n) in g.node_records().enumerate() {
        assert_eq!(n.id.0 as usize, i, "dense node ids by position");
    }
    // Canonical order is (file, line_start, fqn): files sort before each other.
    let files_in_order: Vec<&str> = g.node_records().map(|n| n.file.as_str()).collect();
    let mut sorted = files_in_order.clone();
    sorted.sort();
    assert_eq!(
        files_in_order, sorted,
        "nodes grouped by file in sorted order"
    );
}
