//! v0.3 DATA_FLOW SC2: resolver mapping of `DataFlowFact`s into SSA value nodes
//! and `DerivesFrom` edges.
//!
//! These tests drive the real `link()` over hand-built `FileFacts` (the WP-06
//! pattern), asserting the convergence criteria: value nodes + DerivesFrom edges
//! appear under `--dataflow`; re-assignment yields distinct value nodes; the base
//! index (no flag) is unchanged; an opaque call emits no DerivesFrom edge.

mod common;

use cgx_core::edge::EdgeKind;
use cgx_core::node::SymbolKind;
use cgx_core::transform::Transform;
use cgx_frontend::facts::{FileFacts, ScopeId};
use cgx_resolve::{link, FileInput, LinkOpts};

use common::FileBuilder;

/// One function `g` with a body scope, and the dataflow facts the caller adds.
fn fn_g_with<F: FnOnce(&mut FileBuilder, ScopeId)>(f: F) -> FileFacts {
    let mut b = FileBuilder::new();
    let body = b.scope(ScopeId::ROOT, Some("m::g"));
    b.def(
        "m::g",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        cgx_core::node::Visibility::Public,
        false,
        Some(1),
    );
    f(&mut b, body);
    b.build()
}

fn input<'a>(facts: &'a FileFacts) -> FileInput<'a> {
    FileInput::new("blob-df", "src/m.rs", "rust", facts)
}

fn dataflow_opts() -> LinkOpts {
    LinkOpts {
        dataflow: true,
        ..LinkOpts::default()
    }
}

#[test]
fn dataflow_off_emits_no_value_nodes_or_derives_from() {
    // Criterion 5: base index (no flag) has zero value nodes, zero DerivesFrom.
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["b"], 1, &["a"], body, Transform::Copy, 2);
    });
    let g = link(&[input(&facts)], &LinkOpts::default());
    assert!(
        g.edge_records().all(|e| e.kind != EdgeKind::DerivesFrom),
        "base index must emit no DerivesFrom edges"
    );
    // The only node is the function def itself — no Variable value nodes.
    assert!(
        g.node_records()
            .all(|n| n.kind != SymbolKind::Variable),
        "base index must emit no SSA value nodes"
    );
}

#[test]
fn dataflow_on_emits_value_nodes_and_derives_from() {
    // Criterion 1: SSA value nodes + DerivesFrom edges appear with --dataflow.
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["b"], 1, &["a"], body, Transform::Copy, 2);
    });
    let g = link(&[input(&facts)], &dataflow_opts());

    let value_nodes: Vec<_> = g
        .node_records()
        .filter(|n| n.kind == SymbolKind::Variable)
        .collect();
    assert!(!value_nodes.is_empty(), "expected SSA value nodes");

    let df: Vec<_> = g
        .edge_records()
        .filter(|e| e.kind == EdgeKind::DerivesFrom)
        .collect();
    assert_eq!(df.len(), 1, "one DerivesFrom edge for `let b = a`");
    assert_eq!(df[0].transform, Some(Transform::Copy));
}

#[test]
fn reassignment_yields_two_distinct_value_nodes() {
    // Criterion 2: `b = a; b = b + 1;` → two distinct value nodes for b.
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["b"], 1, &["a"], body, Transform::Copy, 2);
        b.data_flow(&["b"], 2, &["b"], body, Transform::Arith, 3);
    });
    let g = link(&[input(&facts)], &dataflow_opts());

    let b_nodes: Vec<&str> = g
        .node_records()
        .filter(|n| n.kind == SymbolKind::Variable && n.fqn.contains("::b#"))
        .map(|n| n.fqn.as_str())
        .collect();
    assert!(
        b_nodes.contains(&"m::g::b#1") && b_nodes.contains(&"m::g::b#2"),
        "expected b#1 and b#2 as distinct nodes, got {b_nodes:?}"
    );
    // The arith edge for b#2 sources b#1 (the prior version), not b#2 (no self).
    let arith = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::DerivesFrom && e.transform == Some(Transform::Arith))
        .expect("an arith DerivesFrom edge");
    assert_ne!(arith.src, arith.dst, "b#2 must derive from a distinct b#1");
}

#[test]
fn opaque_call_emits_no_derives_from_edge() {
    // opaque-call: `let r = h(a)` records the cut, NO DerivesFrom through h.
    let facts = fn_g_with(|b, body| {
        // The frontend records an empty-source fact carrying OpaqueCall.
        b.data_flow(&["r"], 1, &[], body, Transform::Other, 2);
        b.last_data_flow_cut(cgx_core::cut::CutMarker::OpaqueCall);
    });
    let g = link(&[input(&facts)], &dataflow_opts());
    assert!(
        g.edge_records().all(|e| e.kind != EdgeKind::DerivesFrom),
        "an opaque call must emit no DerivesFrom edge"
    );
}

#[test]
fn return_flow_targets_fn_return_node() {
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["m::g", "return"], 1, &["a"], body, Transform::Copy, 3);
    });
    let g = link(&[input(&facts)], &dataflow_opts());
    assert!(
        g.node_records()
            .any(|n| n.kind == SymbolKind::Variable && n.fqn.contains("return#1")),
        "a return value node must exist"
    );
    assert_eq!(
        g.edge_records()
            .filter(|e| e.kind == EdgeKind::DerivesFrom)
            .count(),
        1
    );
}

#[test]
fn truncated_access_path_drops_to_possible() {
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["t"], 1, &["u"], body, Transform::Projection, 2);
        b.last_data_flow_cut(cgx_core::cut::CutMarker::TruncatedAccessPath);
    });
    let g = link(&[input(&facts)], &dataflow_opts());
    let e = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::DerivesFrom)
        .expect("a DerivesFrom edge");
    assert_eq!(e.confidence, cgx_core::confidence::Confidence::Possible);
}

#[test]
fn link_with_dataflow_is_byte_identical_across_runs() {
    // Criterion 4 (resolver-level): two links of the same facts are identical.
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["b"], 1, &["a"], body, Transform::Copy, 2);
        b.data_flow(&["b"], 2, &["b"], body, Transform::Arith, 3);
        b.data_flow(&["m::g", "return"], 1, &["b"], body, Transform::Copy, 4);
    });
    let g1 = link(&[input(&facts)], &dataflow_opts());
    let g2 = link(&[input(&facts)], &dataflow_opts());

    let ids1: Vec<(u32, u32, u32)> = g1
        .edge_records()
        .map(|e| (e.src.0, e.dst.0, e.kind as u32))
        .collect();
    let ids2: Vec<(u32, u32, u32)> = g2
        .edge_records()
        .map(|e| (e.src.0, e.dst.0, e.kind as u32))
        .collect();
    assert_eq!(ids1, ids2, "edge order/ids must be byte-identical across runs");

    let nodes1: Vec<&str> = g1.node_records().map(|n| n.fqn.as_str()).collect();
    let nodes2: Vec<&str> = g2.node_records().map(|n| n.fqn.as_str()).collect();
    assert_eq!(nodes1, nodes2, "node ids/fqns must be identical across runs");
}
