//! Canonical Example 1 (design §8, P4 proof):
//!
//! ```cypher
//! MATCH path = (src{name:"main::foo"})-[:CALLS*]->(dst{name:"vulnerable::bar"})
//! WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
//! RETURN path
//! ```
//!
//! "Call paths from `main::foo` to `vulnerable::bar` that never cross an
//! exceptional-class edge." Exercises: `path =` binding over a var-length walk,
//! witness simple-path enumeration, `relationships(path)`, the `NONE` quantifier,
//! `IN` over a list literal, and three-valued logic.

mod common;

use cgx_core::EdgeCondition;
use cgx_cql::run;

use common::GraphBuilder;

/// Two routes from `main::foo` to `vulnerable::bar`:
/// - foo → safe → bar (all `always` edges — should be returned)
/// - foo → guard → bar (guard→bar is an `exception` edge — excluded)
///
/// Plus a decoy `unrelated` that does not reach bar.
fn fixture() -> cgx_query::GraphView {
    use cgx_core::SymbolKind::Function;
    GraphBuilder::new()
        .sym("main::foo", Function, "src/main.rs", 1)
        .sym("safe", Function, "src/safe.rs", 1)
        .sym("guard", Function, "src/guard.rs", 1)
        .sym("vulnerable::bar", Function, "src/vuln.rs", 1)
        .sym("unrelated", Function, "src/u.rs", 1)
        // Non-exception route.
        .calls("main::foo", "safe")
        .calls("safe", "vulnerable::bar")
        // Exception route: the second hop is an exception edge.
        .calls("main::foo", "guard")
        .calls_cond("guard", "vulnerable::bar", EdgeCondition::Exception)
        // Decoy.
        .calls("main::foo", "unrelated")
        .view()
}

#[test]
fn non_exception_paths_only() {
    let view = fixture();
    let q = r#"MATCH path = (src{name:"main::foo"})-[:CALLS*]->(dst{name:"vulnerable::bar"})
               WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
               RETURN path"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    // Exactly one surviving path: foo → safe → bar (3 nodes, 2 hops).
    assert_eq!(table.paths.len(), 1, "rows: {:?}", table.rows);
    let p = &table.paths[0];
    assert_eq!(p.nodes.len(), 3);
    assert_eq!(p.edges.len(), 2);

    // The returned path's node fqns, resolved through the view.
    let fqns: Vec<&str> = p.nodes.iter().map(|n| view.node(*n).fqn.as_str()).collect();
    assert_eq!(fqns, vec!["main::foo", "safe", "vulnerable::bar"]);
}

#[test]
fn without_filter_both_paths_enumerate() {
    // Same pattern, no NONE filter: both routes are simple paths to bar.
    let view = fixture();
    let q = r#"MATCH path = (src{name:"main::foo"})-[:CALLS*]->(dst{name:"vulnerable::bar"})
               RETURN path"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.paths.len(), 2);
}

#[test]
fn path_functions_length_and_last_node() {
    let view = fixture();
    let q = r#"MATCH path = (src{name:"main::foo"})-[:CALLS*]->(dst{name:"vulnerable::bar"})
               WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
               RETURN length(path) AS hops, last_node(path) AS sink"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.columns, vec!["hops", "sink"]);
    assert_eq!(table.rows.len(), 1);
    assert_eq!(table.rows[0][0], cgx_cql::Value::Int(2));
    // last_node is `vulnerable::bar`.
    if let cgx_cql::Value::Node(id) = table.rows[0][1] {
        assert_eq!(view.node(id).fqn, "vulnerable::bar");
    } else {
        panic!("expected a node");
    }
}

#[test]
fn any_and_all_quantifiers() {
    let view = fixture();
    // ALL edges are `always` on the safe path → ANY exception is false.
    let q = r#"MATCH path = (src{name:"main::foo"})-[:CALLS*]->(dst{name:"vulnerable::bar"})
               WHERE ANY(r IN relationships(path) WHERE r.condition = "exception")
               RETURN path"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    // Only the exception route satisfies ANY(exception).
    assert_eq!(table.paths.len(), 1);
    let p = &table.paths[0];
    let fqns: Vec<&str> = p.nodes.iter().map(|n| view.node(*n).fqn.as_str()).collect();
    assert_eq!(fqns, vec!["main::foo", "guard", "vulnerable::bar"]);
}

#[test]
fn list_comprehension_projects_conditions() {
    let view = fixture();
    let q = r#"MATCH path = (src{name:"main::foo"})-[:CALLS*]->(dst{name:"vulnerable::bar"})
               RETURN [r IN relationships(path) | r.condition] AS conds"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.rows.len(), 2);
    // Each row is a list of condition tokens; the safe path is all "always".
    use cgx_cql::Value::{List, Str};
    let has_safe = table.rows.iter().any(|r| {
        r[0] == List(vec![Str("always".into()), Str("always".into())])
    });
    let has_exc = table.rows.iter().any(|r| {
        r[0] == List(vec![Str("always".into()), Str("exception".into())])
    });
    assert!(has_safe, "rows: {:?}", table.rows);
    assert!(has_exc, "rows: {:?}", table.rows);
}

#[test]
fn unconstrained_open_end_enumerates_all_witness_paths() {
    // Open end has no name constraint: `path = (src)-[:CALLS*1..2]->()` must
    // reconstruct a witness path to every node reachable within 2 hops, bounding
    // by the `*1..2` depth. From `main::foo`: depth-1 {safe, guard, unrelated},
    // depth-2 {bar via safe, bar via guard}. Five simple paths total.
    let view = fixture();
    let q = r#"MATCH path = (src{name:"main::foo"})-[:CALLS*1..2]->(dst)
               RETURN path"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.paths.len(), 5, "paths: {:?}", table.paths);
    // Every path starts at foo and is length 1 or 2.
    for p in &table.paths {
        assert_eq!(view.node(p.nodes[0]).fqn, "main::foo");
        assert!(p.edges.len() == 1 || p.edges.len() == 2);
    }
}

#[test]
fn min_hop_bound_excludes_short_paths() {
    // `*2..` requires at least two hops, so the direct depth-1 neighbours are
    // excluded; only the two-hop paths to bar remain.
    let view = fixture();
    let q = r#"MATCH path = (src{name:"main::foo"})-[:CALLS*2..]->(dst)
               RETURN path"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.paths.len(), 2, "paths: {:?}", table.paths);
    for p in &table.paths {
        assert_eq!(p.edges.len(), 2);
        assert_eq!(view.node(*p.nodes.last().unwrap()).fqn, "vulnerable::bar");
    }
}

#[test]
fn determinism_run_twice() {
    let view = fixture();
    let q = r#"MATCH path = (src{name:"main::foo"})-[:CALLS*]->(dst{name:"vulnerable::bar"})
               WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
               RETURN path"#;
    assert_eq!(run(&view, q).unwrap(), run(&view, q).unwrap());
}

// ── N3: var-length path walk is budget-bounded and surfaces truncation ────────

/// Dense fixture: `src` → 33 middle-a nodes → 33 middle-b nodes → `dst`.
/// There are 33 × 33 = 1089 simple paths (> DEFAULT_MAX_PATHS=1024) so the
/// path-cap fires deterministically.  The test proves:
/// (a) the walk *terminates* (no hang on a dense graph), and
/// (b) `table.truncation` is `Some(PathCap)` (honesty marker present).
#[test]
fn dense_graph_terminates_and_emits_truncation_marker() {
    use cgx_core::SymbolKind::Function;
    use cgx_query::TruncationReason;

    const MID_COUNT: usize = 33; // 33 × 33 = 1089 > DEFAULT_MAX_PATHS=1024

    // Build the graph programmatically: src → mid_a_i, mid_a_i → mid_b_j, mid_b_j → dst.
    let mut b = GraphBuilder::new()
        .sym("src", Function, "src/src.rs", 1)
        .sym("dst", Function, "src/dst.rs", 1);
    for i in 0..MID_COUNT {
        b = b.sym(&format!("mid_a_{i}"), Function, "src/mid.rs", (10 + i) as u32);
    }
    for j in 0..MID_COUNT {
        b = b.sym(&format!("mid_b_{j}"), Function, "src/mid.rs", (100 + j) as u32);
    }
    for i in 0..MID_COUNT {
        b = b.calls("src", &format!("mid_a_{i}"));
    }
    for i in 0..MID_COUNT {
        for j in 0..MID_COUNT {
            b = b.calls(&format!("mid_a_{i}"), &format!("mid_b_{j}"));
        }
    }
    for j in 0..MID_COUNT {
        b = b.calls(&format!("mid_b_{j}"), "dst");
    }
    let view = b.view();

    let q = r#"MATCH path = (s{name:"src"})-[:CALLS*]->(d{name:"dst"}) RETURN path"#;
    // Must terminate (no hang) within the test timeout.
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    // (a) Terminated with ≤ DEFAULT_MAX_PATHS=1024 paths.
    assert!(
        table.paths.len() <= 1024,
        "expected ≤ 1024 paths, got {}",
        table.paths.len()
    );
    // (b) Truncation marker is present (PathCap fired).
    assert_eq!(
        table.truncation,
        Some(TruncationReason::PathCap),
        "expected PathCap truncation marker"
    );
}
