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
use cgx_resolve::{link, propagate_dirty, FileInput, LinkOpts};

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

// --- v0.3 SC3 incremental substrate (resolver-level) -------------------------

/// A file with caller `m::g` (whose body holds the supplied facts) and a leaf
/// callee `m::h`, so an opaque call to `h` grounds to a real FQN.
fn fn_g_calling_h<F: FnOnce(&mut FileBuilder, ScopeId)>(f: F) -> FileFacts {
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
    b.def(
        "m::h",
        SymbolKind::Function,
        ScopeId::ROOT,
        10,
        cgx_core::node::Visibility::Public,
        false,
        Some(1),
    );
    f(&mut b, body);
    b.build()
}

#[test]
fn opaque_call_records_resolved_summary_dep() {
    // An opaque call `let r = h(a)` carrying callee `h` grounds to the real FQN
    // `m::h` in summary_deps.
    let facts = fn_g_calling_h(|b, body| {
        b.data_flow(&["r"], 1, &[], body, Transform::Other, 2);
        b.last_data_flow_cut(cgx_core::cut::CutMarker::OpaqueCall);
        b.last_data_flow_callee("h");
    });
    let g = link(&[input(&facts)], &dataflow_opts());
    assert!(
        g.dataflow
            .summary_deps
            .contains(&("m::g".to_string(), "m::h".to_string())),
        "expected a resolved (m::g -> m::h) dep, got {:?}",
        g.dataflow.summary_deps
    );
}

#[test]
fn opaque_call_with_unknown_callee_records_wildcard_dep() {
    // Criterion 6 (substrate): an opaque call whose callee carries no name (a
    // virtual/duck-typed receiver call) records the conservative `*` wildcard.
    let facts = fn_g_calling_h(|b, body| {
        b.data_flow(&["r"], 1, &[], body, Transform::Other, 2);
        b.last_data_flow_cut(cgx_core::cut::CutMarker::OpaqueCall);
        // No callee name set → wildcard.
    });
    let g = link(&[input(&facts)], &dataflow_opts());
    assert!(
        g.dataflow
            .summary_deps
            .contains(&("m::g".to_string(), "*".to_string())),
        "expected a wildcard dep, got {:?}",
        g.dataflow.summary_deps
    );
}

#[test]
fn cold_link_recomputes_every_function() {
    // An empty prior cache (cold build) marks every function as recomputed.
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["b"], 1, &["a"], body, Transform::Copy, 2);
    });
    let g = link(&[input(&facts)], &dataflow_opts());
    assert_eq!(g.dataflow.stats.functions_recomputed, 1);
    assert_eq!(g.dataflow.stats.functions_reused, 0);
    assert_eq!(g.dataflow.changed_fns, vec!["m::g".to_string()]);
}

#[test]
fn unchanged_function_with_matching_prior_cache_is_reused() {
    // Criterion 1 (substrate): a function whose facts_hash matches the prior
    // cache is reused, not recomputed.
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["b"], 1, &["a"], body, Transform::Copy, 2);
    });
    // First (cold) link to learn the hash the resolver computes.
    let cold = link(&[input(&facts)], &dataflow_opts());
    let prior: std::collections::HashMap<(String, String), String> =
        cold.dataflow.fn_intraproc_cache.iter().cloned().collect();

    let warm_opts = LinkOpts {
        dataflow: true,
        prior_fn_cache: prior,
        ..LinkOpts::default()
    };
    let warm = link(&[input(&facts)], &warm_opts);
    assert_eq!(warm.dataflow.stats.functions_recomputed, 0);
    assert_eq!(warm.dataflow.stats.functions_reused, 1);
    assert!(warm.dataflow.changed_fns.is_empty());
}

#[test]
fn propagate_dirty_reaches_transitive_callers() {
    // Criterion 3: B calls A, C calls B, D is independent. A dirty → B, C dirty;
    // D stays clean.
    let deps = vec![
        ("B".to_string(), "A".to_string()),
        ("C".to_string(), "B".to_string()),
    ];
    let mut closed = propagate_dirty(&["A".to_string()], &deps);
    closed.sort();
    assert_eq!(closed, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
}

#[test]
fn propagate_dirty_terminates_on_mutual_recursion_cycle() {
    // The visited set makes a cycle A<->B terminate rather than loop forever.
    let deps = vec![
        ("A".to_string(), "B".to_string()),
        ("B".to_string(), "A".to_string()),
    ];
    let mut closed = propagate_dirty(&["A".to_string()], &deps);
    closed.sort();
    assert_eq!(closed, vec!["A".to_string(), "B".to_string()]);
}

#[test]
fn propagate_dirty_wildcard_dependent_recomputes_on_any_change() {
    // Criterion 6: a function with a `*` dep recomputes when ANY function changed.
    let deps = vec![("W".to_string(), "*".to_string())];
    let closed = propagate_dirty(&["X".to_string()], &deps);
    assert!(closed.contains(&"W".to_string()), "wildcard dependent must be dirty");
    // No change → wildcard dependent stays clean.
    let none = propagate_dirty(&[], &deps);
    assert!(none.is_empty(), "no seed change must leave the wildcard clean");
}

// --- v0.3 SC4 IFDS interprocedural summaries --------------------------------

use cgx_core::confidence::Confidence;
use cgx_core::cut::CutMarker;
use common::FileBuilder as FB;

/// Build a callee `m::h(x)` whose body returns its parameter through `transform`
/// (so it has a summary `formal_in_0 ⇝ return`), and a caller `m::g(a)` whose
/// body holds `let r = h(a); let b = r;` so the interproc edge resolves `r ⇝ a`.
fn caller_callee_fixture(callee_transform: Transform) -> FileFacts {
    let mut b = FB::new();
    let g_body = b.scope(ScopeId::ROOT, Some("m::g"));
    let h_body = b.scope(ScopeId::ROOT, Some("m::h"));
    b.def("m::g", SymbolKind::Function, ScopeId::ROOT, 1,
        cgx_core::node::Visibility::Public, false, Some(1));
    b.def("m::h", SymbolKind::Function, ScopeId::ROOT, 10,
        cgx_core::node::Visibility::Public, false, Some(1));
    // m::h body: `return <transform>(x)` — formal-in p0 (named "p0" by the
    // builder's signature synthesis) flows to the return.
    b.data_flow(&["m::h", "return"], 1, &["p0"], h_body, callee_transform, 11);
    // m::g body: `let r = h(a)` (opaque call carrying callee + arg path), then
    // `let b = r` so a downstream intraproc edge witnesses r.
    b.data_flow(&["r"], 1, &[], g_body, Transform::Other, 2);
    b.last_data_flow_cut(CutMarker::OpaqueCall);
    b.last_data_flow_callee("h");
    b.last_data_flow_args(&[&["p0"]]); // g's own param p0 is passed as h's arg 0.
    b.data_flow(&["b"], 1, &["r"], g_body, Transform::Copy, 3);
    b.build()
}

#[test]
fn interproc_flow_resolves_through_summary() {
    // Criterion 1: `a -> g(a) -> return -> b` yields a DerivesFrom path b ⇝ a
    // through g's summary; the interproc edge is probable + tagged interprocedural.
    let facts = caller_callee_fixture(Transform::Copy);
    let g = link(&[input(&facts)], &dataflow_opts());

    // The interproc edge: r#1 ⇝ p0#0 (g's param), rule "interprocedural".
    let interproc: Vec<_> = g
        .edge_records()
        .filter(|e| e.kind == EdgeKind::DerivesFrom && e.rule == "interprocedural")
        .collect();
    assert_eq!(interproc.len(), 1, "exactly one interproc edge, got {interproc:?}");
    let e = interproc[0];
    assert_eq!(e.confidence, Confidence::Probable, "summary flow is probable");

    // Endpoints: src = m::g::r#1 (result), dst = m::g::p0#0 (the caller's arg).
    let fqn = |id: cgx_core::id::NodeId| {
        g.node_records().find(|n| n.id == id).map(|n| n.fqn.clone()).unwrap()
    };
    assert_eq!(fqn(e.src), "m::g::r#1");
    assert_eq!(fqn(e.dst), "m::g::p0#0");
}

#[test]
fn summary_preserves_transform_across_boundary() {
    // Criterion 3: a non-copy transform on the callee's intraproc return path
    // shows on the materialized interproc edge.
    let facts = caller_callee_fixture(Transform::Parse);
    let g = link(&[input(&facts)], &dataflow_opts());
    let e = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::DerivesFrom && e.rule == "interprocedural")
        .expect("an interproc edge");
    assert_eq!(
        e.transform,
        Some(Transform::Parse),
        "the callee's Parse transform must survive the summary boundary"
    );
}

#[test]
fn self_recursion_reaches_fixpoint_no_hang() {
    // Criterion 2: a self-recursive `m::r(x) { return r(x) }` summarizes to a
    // fixpoint (formal_in_0 ⇝ return) without hanging.
    let mut b = FB::new();
    let body = b.scope(ScopeId::ROOT, Some("m::r"));
    b.def("m::r", SymbolKind::Function, ScopeId::ROOT, 1,
        cgx_core::node::Visibility::Public, false, Some(1));
    // `fn r(p0) { if base { return p0 } return r(p0) }`: a base-case return that
    // copies the param (so a non-trivial summary exists) plus the recursive call.
    b.data_flow(&["m::r", "return"], 1, &["p0"], body, Transform::Copy, 2);
    b.data_flow(&["m::r", "return"], 2, &[], body, Transform::Copy, 3);
    b.last_data_flow_cut(CutMarker::OpaqueCall);
    b.last_data_flow_callee("r");
    b.last_data_flow_args(&[&["p0"]]);
    let facts = b.build();
    let g = link(&[input(&facts)], &dataflow_opts());
    // The summary fixpoint terminates; we get a self-edge return#1 ⇝ p0#0.
    assert!(
        g.edge_records()
            .any(|e| e.kind == EdgeKind::DerivesFrom && e.rule == "interprocedural"),
        "self-recursion must still produce a summary edge at fixpoint"
    );
    assert_eq!(g.dataflow.ifds_stats.budget_exceeded_sccs, 0, "no budget trip");
}

#[test]
fn mutual_recursion_scc_reaches_fixpoint_no_hang() {
    // Criterion 2: f <-> g mutual recursion forms a 2-node SCC that summarizes to
    // a fixpoint without hanging.
    let mut b = FB::new();
    let f_body = b.scope(ScopeId::ROOT, Some("m::f"));
    let g_body = b.scope(ScopeId::ROOT, Some("m::g"));
    b.def("m::f", SymbolKind::Function, ScopeId::ROOT, 1,
        cgx_core::node::Visibility::Public, false, Some(1));
    b.def("m::g", SymbolKind::Function, ScopeId::ROOT, 10,
        cgx_core::node::Visibility::Public, false, Some(1));
    // m::f: base-case `return p0` + recursive `return g(p0)`.
    b.data_flow(&["m::f", "return"], 1, &["p0"], f_body, Transform::Copy, 2);
    b.data_flow(&["m::f", "return"], 2, &[], f_body, Transform::Copy, 3);
    b.last_data_flow_cut(CutMarker::OpaqueCall);
    b.last_data_flow_callee("g");
    b.last_data_flow_args(&[&["p0"]]);
    // m::g: base-case `return p0` + recursive `return f(p0)`.
    b.data_flow(&["m::g", "return"], 1, &["p0"], g_body, Transform::Copy, 11);
    b.data_flow(&["m::g", "return"], 2, &[], g_body, Transform::Copy, 12);
    b.last_data_flow_cut(CutMarker::OpaqueCall);
    b.last_data_flow_callee("f");
    b.last_data_flow_args(&[&["p0"]]);
    let facts = b.build();
    let g = link(&[input(&facts)], &dataflow_opts());
    assert_eq!(g.dataflow.ifds_stats.budget_exceeded_sccs, 0, "no budget trip on f<->g");
    // Both functions reach a return⇝param summary at fixpoint.
    assert!(g.dataflow.ifds_stats.summaries_computed >= 2);
}

#[test]
fn interproc_link_is_byte_identical_across_runs() {
    // Criterion 7: two links of the same caller+callee facts are byte-identical
    // (deterministic worklist order, content-derived summaries).
    let facts = caller_callee_fixture(Transform::Copy);
    let g1 = link(&[input(&facts)], &dataflow_opts());
    let g2 = link(&[input(&facts)], &dataflow_opts());
    let e1: Vec<(u32, u32, String)> = g1
        .edge_records()
        .map(|e| (e.src.0, e.dst.0, e.rule.clone()))
        .collect();
    let e2: Vec<(u32, u32, String)> = g2
        .edge_records()
        .map(|e| (e.src.0, e.dst.0, e.rule.clone()))
        .collect();
    assert_eq!(e1, e2, "interproc edge set must be byte-identical across runs");
    // The persisted summaries are also identical.
    assert_eq!(g1.dataflow.fn_summaries, g2.dataflow.fn_summaries);
}

#[test]
fn work_budget_backstop_fires_on_dense_scc() {
    // Criterion 4: an adversarial dense mutual-recursion clique with a TINY work
    // budget trips the per-SCC cap and records a `summary-budget-exceeded` cut
    // marker instead of hanging. Bounded by construction (small N + small budget)
    // so CI never actually hangs; the assertion proves the backstop engaged.
    const N: usize = 8;
    let mut b = FB::new();
    let mut bodies = Vec::new();
    for i in 0..N {
        let fqn = format!("m::f{i}");
        let body = b.scope(ScopeId::ROOT, Some(&fqn));
        b.def(&fqn, SymbolKind::Function, ScopeId::ROOT, (i as u32) * 10 + 1,
            cgx_core::node::Visibility::Public, false, Some(1));
        bodies.push((fqn, body));
    }
    // Every f_i has a base-case `return p0` and calls every f_j (a dense clique →
    // one big SCC whose round-robin fixpoint does O(N^2) recomputes).
    for (i, (_fqn, body)) in bodies.iter().enumerate() {
        b.data_flow(&["return"], 1, &["p0"], *body, Transform::Copy, (i as u32) * 10 + 2);
        for j in 0..N {
            if i == j {
                continue;
            }
            b.data_flow(&["return"], 2 + j as u32, &[], *body, Transform::Copy, (i as u32) * 10 + 3 + j as u32);
            b.last_data_flow_cut(CutMarker::OpaqueCall);
            b.last_data_flow_callee(&format!("f{j}"));
            b.last_data_flow_args(&[&["p0"]]);
        }
    }
    let facts = b.build();
    let opts = LinkOpts {
        dataflow: true,
        max_summary_edges: Some(4), // tiny: per-SCC cap = max(4/10,1) = 1.
        ..LinkOpts::default()
    };
    let g = link(&[input(&facts)], &opts);
    assert!(
        g.dataflow.ifds_stats.budget_exceeded_sccs >= 1,
        "the dense SCC must trip the work-budget cap"
    );
    // At least one materialized interproc edge carries the honest cut marker.
    assert!(
        g.edge_records().any(|e| e.kind == EdgeKind::DerivesFrom
            && e.cut_markers.contains(CutMarker::SummaryBudgetExceeded)),
        "a summary-budget-exceeded cut marker must be recorded"
    );
}

#[test]
fn unread_param_is_minted_as_formal_in() {
    // Decision C: a parameter never read as an intraproc source still gets a
    // formal-in value node `<fn>::p0#0` so a summary can source it.
    let mut b = FB::new();
    let _body = b.scope(ScopeId::ROOT, Some("m::h"));
    b.def("m::h", SymbolKind::Function, ScopeId::ROOT, 1,
        cgx_core::node::Visibility::Public, false, Some(1));
    // No data_flow facts that read p0 at all.
    let facts = b.build();
    let g = link(&[input(&facts)], &dataflow_opts());
    assert!(
        g.node_records()
            .any(|n| n.kind == SymbolKind::Variable && n.fqn == "m::h::p0#0"),
        "every param must be minted as a formal-in node"
    );
}

// --- v0.3 SC6 regression: order-independent source resolution + field base ----
//
// These guard the Gap-A (canonicalize sorts by derived NAME but resolution must
// use PROGRAM order) and Gap-2 (depth-1 field projection must flow from the BASE
// local, not the field tail) fixes verified in the cycle
// `2026-06-24_1831_cgx-v0.3-sc6-on-by-default` (execution/verify-ssa-recall.md).

/// Whether a backward DerivesFrom walk from the value node `from_fqn` reaches
/// `to_fqn` (the source the chain should bottom out at). Follows edges by
/// node FQN so it is independent of dense id assignment.
fn derives_from_reaches(g: &cgx_resolve::ResolvedGraph, from_fqn: &str, to_fqn: &str) -> bool {
    let id_of = |fqn: &str| g.node_records().find(|n| n.fqn == fqn).map(|n| n.id);
    let fqn_of = |id: cgx_core::id::NodeId| {
        g.node_records().find(|n| n.id == id).map(|n| n.fqn.clone())
    };
    let Some(start) = id_of(from_fqn) else {
        return false;
    };
    let mut stack = vec![start];
    let mut seen = std::collections::HashSet::new();
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        if fqn_of(cur).as_deref() == Some(to_fqn) {
            return true;
        }
        for e in g.edge_records() {
            if e.kind == EdgeKind::DerivesFrom && e.src == cur {
                stack.push(e.dst);
            }
        }
    }
    false
}

#[test]
fn tail_return_reaches_source_through_reordered_bindings() {
    // Gap A: `fn order_test(p) { let z = p; let a = z; a }`. The derived names
    // a < return < z, so canonicalize() stores them in an order where the binding
    // `a = z` precedes z's def — yet resolution must use program order so the
    // chain return ⇝ a ⇝ z ⇝ p stays connected (pre-fix `a#1 -> z#0` was a dead
    // phantom and return did NOT reach p).
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["z"], 1, &["p0"], body, Transform::Copy, 2);
        b.data_flow(&["a"], 1, &["z"], body, Transform::Copy, 3);
        b.data_flow(&["m::g", "return"], 1, &["a"], body, Transform::Copy, 4);
    });
    let g = link(&[input(&facts)], &dataflow_opts());

    // No source resolves to a phantom #0 of a local that is actually re-defined.
    assert!(
        g.node_records().any(|n| n.fqn == "m::g::z#1"),
        "z's real def z#1 must exist as a node"
    );
    assert!(
        !g.edge_records().any(|e| {
            e.kind == EdgeKind::DerivesFrom
                && g.node_records().any(|n| n.id == e.dst && n.fqn == "m::g::z#0")
        }),
        "no edge may resolve z to the dead phantom z#0"
    );
    // End-to-end: the tail return reaches the parameter through the full chain.
    assert!(
        derives_from_reaches(&g, "m::g::return#1", "m::g::p0#0"),
        "return ⇝ p0 must be connected through z#1 and a#1"
    );
}

#[test]
fn interproc_tail_return_reaches_caller_arg_across_call() {
    // Gap A across a call boundary: the callee's tail-return summary plus the
    // caller's reordered bindings must compose so the caller's downstream value
    // reaches its own param. `caller_callee_fixture` builds g(a){ r=h(a); b=r }
    // and h(p0){ return p0 }; b ⇝ p0 must hold via the interproc edge r ⇝ p0.
    let facts = caller_callee_fixture(Transform::Copy);
    let g = link(&[input(&facts)], &dataflow_opts());
    assert!(
        derives_from_reaches(&g, "m::g::b#1", "m::g::p0#0"),
        "b ⇝ p0 must be connected through the interproc summary edge r ⇝ p0"
    );
}

#[test]
fn depth1_field_projection_flows_from_base_local() {
    // Gap 2: `let x = p.lo;` emits source path ["p","lo"]. The resolver must bind
    // x to the BASE local p (with a Projection transform), not to a phantom local
    // named after the field `lo` (pre-fix edge was `x#1 -> lo#0`).
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["x"], 1, &["p0", "lo"], body, Transform::Projection, 2);
    });
    let g = link(&[input(&facts)], &dataflow_opts());

    let proj = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::DerivesFrom && e.transform == Some(Transform::Projection))
        .expect("a Projection DerivesFrom edge");
    let dst_fqn = g.node_records().find(|n| n.id == proj.dst).map(|n| n.fqn.clone());
    assert_eq!(
        dst_fqn.as_deref(),
        Some("m::g::p0#0"),
        "x must flow from the base local p0, not the field tail `lo`"
    );
    assert!(
        !g.node_records().any(|n| n.fqn.contains("::lo#")),
        "no phantom value node named after the field `lo` may be minted"
    );
    assert!(
        derives_from_reaches(&g, "m::g::x#1", "m::g::p0#0"),
        "flows-from x must reach p0 through the projection"
    );
}

#[test]
fn if_select_captures_both_arms_and_condition() {
    // Gap 3 (REFUTED as a bug — guard it stays correct): a φ-join
    // `let y = if cond { a } else { b }` captures both arms. Modelled as three
    // Branched facts at the same site (one per source), and the tail return
    // reaches each arm. The over-capture of `cond` is by current design.
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["a"], 1, &["p0"], body, Transform::Copy, 2);
        b.data_flow(&["bb"], 1, &["p0"], body, Transform::Copy, 3);
        b.data_flow(&["y"], 1, &["a"], body, Transform::Branched, 4);
        b.data_flow(&["y"], 1, &["bb"], body, Transform::Branched, 4);
        b.data_flow(&["m::g", "return"], 1, &["y"], body, Transform::Copy, 5);
    });
    let g = link(&[input(&facts)], &dataflow_opts());
    let branched: Vec<_> = g
        .edge_records()
        .filter(|e| e.kind == EdgeKind::DerivesFrom && e.transform == Some(Transform::Branched))
        .collect();
    assert_eq!(branched.len(), 2, "both if-arms must produce a Branched edge");
    assert!(
        derives_from_reaches(&g, "m::g::return#1", "m::g::a#1"),
        "return ⇝ y ⇝ a (then-arm) must be connected"
    );
    assert!(
        derives_from_reaches(&g, "m::g::return#1", "m::g::bb#1"),
        "return ⇝ y ⇝ bb (else-arm) must be connected"
    );
}

#[test]
fn binary_arith_links_both_operands_to_return() {
    // Gap 4 (REFUTED as a bug — guard it stays correct): `let c = a + b` links
    // BOTH operands at their live versions, and the tail return reaches both
    // through the full chain c ⇝ {a,b} ⇝ {p,q}.
    let facts = fn_g_with(|b, body| {
        b.data_flow(&["a"], 1, &["p0"], body, Transform::Copy, 2);
        b.data_flow(&["bb"], 1, &["q0"], body, Transform::Copy, 3);
        b.data_flow(&["c"], 1, &["a"], body, Transform::Arith, 4);
        b.data_flow(&["c"], 1, &["bb"], body, Transform::Arith, 4);
        b.data_flow(&["m::g", "return"], 1, &["c"], body, Transform::Copy, 5);
    });
    let g = link(&[input(&facts)], &dataflow_opts());
    assert!(
        derives_from_reaches(&g, "m::g::return#1", "m::g::p0#0"),
        "return ⇝ c ⇝ a ⇝ p0 (first operand) must be connected"
    );
    assert!(
        derives_from_reaches(&g, "m::g::return#1", "m::g::q0#0"),
        "return ⇝ c ⇝ bb ⇝ q0 (second operand) must be connected — no phantom #0"
    );
}

// KNOWN LIMIT (Gap 1, deferred by design — do NOT try to fix here): arithmetic
// THROUGH a builtin/opaque method call, e.g. `let c = a.wrapping_add(b)`, yields
// NO DerivesFrom edge. The call is classified OpaqueCall with no grounded callee
// summary (a builtin), and a method call's receiver `a` is not captured as an
// arg, so IFDS materializes zero edges. This is the documented OpaqueCall
// deferral (design 04 DF lattice), not a regression — see verify-ssa-recall.md
// Gap 1. There is intentionally no passing-reachability test for this shape.

#[test]
fn resolution_is_independent_of_canonical_fact_order() {
    // Gap A invariant: the resolved value-node + edge set must be identical no
    // matter what order the facts arrive in, because resolution uses program
    // (span) order, not the canonical derived-name storage order. Feed the same
    // facts twice — once name-sorted (via build()) and once reversed — and assert
    // the resolved edge FQN set is byte-identical.
    let mut forward = FB::new();
    let f_body = forward.scope(ScopeId::ROOT, Some("m::g"));
    forward.def("m::g", SymbolKind::Function, ScopeId::ROOT, 1,
        cgx_core::node::Visibility::Public, false, Some(1));
    forward.data_flow(&["z"], 1, &["p0"], f_body, Transform::Copy, 2);
    forward.data_flow(&["a"], 1, &["z"], f_body, Transform::Copy, 3);
    forward.data_flow(&["m::g", "return"], 1, &["a"], f_body, Transform::Copy, 4);
    let facts_a = forward.build(); // canonicalize() sorts by derived name.

    // Same facts, but reverse the data_flows after build to prove the resolver
    // re-derives program order from spans regardless of vec order.
    let mut facts_b = facts_a.clone();
    facts_b.data_flows.reverse();

    let g_a = link(&[input(&facts_a)], &dataflow_opts());
    let g_b = link(&[input(&facts_b)], &dataflow_opts());

    let edge_fqns = |g: &cgx_resolve::ResolvedGraph| -> Vec<(String, String)> {
        let fqn = |id: cgx_core::id::NodeId| {
            g.node_records().find(|n| n.id == id).map(|n| n.fqn.clone()).unwrap_or_default()
        };
        let mut v: Vec<(String, String)> = g
            .edge_records()
            .filter(|e| e.kind == EdgeKind::DerivesFrom)
            .map(|e| (fqn(e.src), fqn(e.dst)))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        edge_fqns(&g_a),
        edge_fqns(&g_b),
        "resolved DerivesFrom edge set must not depend on canonical fact order"
    );
}
