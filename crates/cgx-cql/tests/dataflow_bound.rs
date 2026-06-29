//! Guards on unbounded variable-length (`DATA_FLOW*` / `CALLS*`) traversal.
//!
//! Regression coverage for the SOTU punch-list risk: a bare `*` var-length walk
//! over a dense dataflow graph can expand without bound (full transitive closure
//! from every anchor), blowing up time/memory. The guard adds:
//!   * a default upper hop bound (`DEFAULT_VAR_LENGTH_DEPTH`) for an unbounded `*`,
//!   * a result-row cap (`DEFAULT_VAR_LENGTH_ROWS`) on the peer-set branch,
//!
//! both *truncating* with a surfaced `TruncationReason::PathCap` marker rather than
//! erroring (the CutMarker honesty idiom).
//!
//! Source: internal design notes
//!         findings/C-health.md — structural risk #2.

mod common;

use cgx_core::SymbolKind::Function;
use cgx_cql::eval::{DEFAULT_VAR_LENGTH_DEPTH, DEFAULT_VAR_LENGTH_ROWS};
use cgx_cql::{run, TruncationReason, Value};
use cgx_query::GraphView;

use common::GraphBuilder;

/// A linear DATA_FLOW chain `n0 -> n1 -> ... -> n{len-1}` (each `data_flow` edge
/// points `src` -> `dst`, i.e. forward derivation). `n0` is the anchor source.
fn linear_chain(len: usize) -> GraphView {
    let mut b = GraphBuilder::new();
    for i in 0..len {
        b = b.sym(&format!("n{i}"), Function, "src/chain.rs", (i + 1) as u32);
    }
    for i in 0..len - 1 {
        let (from, to) = (format!("n{i}"), format!("n{}", i + 1));
        b = b.data_flow(&from, &to);
    }
    b.view()
}

/// (a) BOUND-HIT: a bare `DATA_FLOW*` over a chain longer than the default depth
/// only reaches nodes within `DEFAULT_VAR_LENGTH_DEPTH` hops of the anchor — the
/// deeper nodes are excluded by the default cap, so the walk terminates bounded.
#[test]
fn unbounded_star_caps_at_default_depth() {
    // Chain of 20 nodes: n0..n19. A bare `*` from n0 must stop at n{DEFAULT}.
    let view = linear_chain(20);
    let q = r#"MATCH (s{name:"n0"})-[:DATA_FLOW*]->(d) RETURN d.name AS reached"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    let reached: Vec<&str> = table
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::Str(s) => s.as_str(),
            other => panic!("expected a string, got {other:?}"),
        })
        .collect();

    // Every reached node is within DEFAULT_VAR_LENGTH_DEPTH hops; the farthest
    // reachable node is exactly n{DEFAULT_VAR_LENGTH_DEPTH}, and n{DEFAULT+1}.. are
    // beyond the default cap and must NOT appear.
    let deepest = format!("n{DEFAULT_VAR_LENGTH_DEPTH}");
    let beyond = format!("n{}", DEFAULT_VAR_LENGTH_DEPTH + 1);
    assert!(
        reached.contains(&deepest.as_str()),
        "n{DEFAULT_VAR_LENGTH_DEPTH} should be reachable within the default cap; got {reached:?}"
    );
    assert!(
        !reached.contains(&beyond.as_str()),
        "n{} is beyond the default depth cap and must be excluded; got {reached:?}",
        DEFAULT_VAR_LENGTH_DEPTH + 1
    );
    assert_eq!(
        reached.len(),
        DEFAULT_VAR_LENGTH_DEPTH as usize,
        "exactly {DEFAULT_VAR_LENGTH_DEPTH} nodes (n1..n{DEFAULT_VAR_LENGTH_DEPTH}) within the cap"
    );
}

/// (b) WITHIN-BOUND: a chain comfortably shorter than the default depth returns
/// the full reachable set with no truncation marker.
#[test]
fn within_bound_returns_full_results_untruncated() {
    // Chain of 4 nodes: n0 -> n1 -> n2 -> n3. All 3 reachable nodes are within the
    // default depth, so the result is complete.
    let view = linear_chain(4);
    let q = r#"MATCH (s{name:"n0"})-[:DATA_FLOW*]->(d) RETURN d.name AS reached"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    let mut reached: Vec<&str> = table
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::Str(s) => s.as_str(),
            other => panic!("expected a string, got {other:?}"),
        })
        .collect();
    reached.sort_unstable();

    assert_eq!(reached, vec!["n1", "n2", "n3"], "full reachable set expected");
    assert_eq!(
        table.truncation, None,
        "a within-bound walk must not surface a truncation marker"
    );
}

/// (c) ESCAPE HATCH: an explicit `*1..N` with `N` above the default depth raises
/// the bound, so a node beyond `DEFAULT_VAR_LENGTH_DEPTH` that the bare `*` would
/// exclude is now reached.
#[test]
fn explicit_range_raises_the_default_depth_bound() {
    let view = linear_chain(20);
    // Explicit upper bound of 12 (> DEFAULT 8) reaches n9..n12 that a bare `*`
    // would have excluded.
    let q = r#"MATCH (s{name:"n0"})-[:DATA_FLOW*1..12]->(d) RETURN d.name AS reached"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    let reached: Vec<&str> = table
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::Str(s) => s.as_str(),
            other => panic!("expected a string, got {other:?}"),
        })
        .collect();

    let escaped = format!("n{}", DEFAULT_VAR_LENGTH_DEPTH + 4); // n12, beyond default
    assert!(
        reached.contains(&escaped.as_str()),
        "explicit *1..12 should raise the bound and reach n12; got {reached:?}"
    );
    assert_eq!(
        reached.len(),
        12,
        "exactly 12 nodes (n1..n12) within the explicit bound"
    );
}

/// BOUND-HIT (row cap): an *unanchored* `(a)-[:DATA_FLOW*]->(b)` anchors on every
/// node, so the cross product of emitted rows is O(anchors × reachable). On a wide
/// fan this exceeds `DEFAULT_VAR_LENGTH_ROWS`; the walk must terminate (no hang)
/// and surface a `PathCap` truncation marker.
#[test]
fn unanchored_star_caps_rows_and_emits_truncation_marker() {
    // A `hub` flows into WIDTH leaf nodes, and each leaf flows into the hub's
    // SECONDARY fan — built so the total emitted-row count exceeds the row cap.
    // Simpler: a star where one source flows into many sinks AND the pattern is
    // unanchored, so each of the many nodes is an anchor with reachable peers.
    // The unanchored anchor set is every node; the emitted bindings number
    // ~WIDTH + WIDTH*(WIDTH-1)/2. WIDTH=60 -> ~60 + 1770 = 1830 bindings, well
    // above DEFAULT_VAR_LENGTH_ROWS=1024, so the row cap fires.
    const WIDTH: usize = 60;

    let mut b = GraphBuilder::new().sym("root", Function, "src/star.rs", 1);
    for i in 0..WIDTH {
        b = b.sym(&format!("a{i}"), Function, "src/star.rs", (10 + i) as u32);
    }
    // root -> a_i, and a_i -> a_{i+1} chained so every a_i reaches many peers.
    for i in 0..WIDTH {
        b = b.data_flow("root", &format!("a{i}"));
    }
    for i in 0..WIDTH - 1 {
        for j in (i + 1)..WIDTH {
            b = b.data_flow(&format!("a{i}"), &format!("a{j}"));
        }
    }
    let view = b.view();

    // Unanchored: anchor set is every node -> cross-product row blowup.
    let q = r#"MATCH (x)-[:DATA_FLOW*]->(y) RETURN y.name AS reached"#;
    // Must terminate within the test timeout (no unbounded expansion).
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    assert!(
        table.rows.len() <= DEFAULT_VAR_LENGTH_ROWS,
        "rows must be capped at DEFAULT_VAR_LENGTH_ROWS={DEFAULT_VAR_LENGTH_ROWS}, got {}",
        table.rows.len()
    );
    assert_eq!(
        table.truncation,
        Some(TruncationReason::PathCap),
        "hitting the row cap must surface a PathCap truncation marker"
    );
}
