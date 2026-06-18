//! P5 proof tests (design §8): aggregates, implicit grouping, ORDER BY, and
//! determinism.
//!
//! - A reachability-matrix `count(*)` over a `CALLS*` walk that includes a
//!   mutually-recursive pair, proving BFS simple-path semantics count each
//!   distinct reachable node once (never infinitely).
//! - Determinism: a canonical query run twice is byte-identical, and a
//!   shuffled-edge-input fixture produces an identical result (no hash-order
//!   leak — the store canonicalises edge order, the walker sorts neighbours,
//!   and the result is ordered, so input order cannot reach a row).

mod common;

use cgx_core::SymbolKind::Function;
use cgx_cql::{run, Value};

use common::GraphBuilder;

/// Reachability fixture with a mutual recursion `f <-> g` plus a sink `h`:
///   f -> g,  g -> f,  f -> h
/// Distinct nodes reachable from `f` over `CALLS*` are exactly {g, h} (each
/// visited once; the f<->g cycle does not loop forever). `count(*)` over the
/// reachable peer set is therefore 2.
fn recursion_fixture() -> cgx_query::GraphView {
    GraphBuilder::new()
        .sym("f", Function, "src/a.rs", 1)
        .sym("g", Function, "src/b.rs", 1)
        .sym("h", Function, "src/c.rs", 1)
        .calls("f", "g")
        .calls("g", "f")
        .calls("f", "h")
        .view()
}

#[test]
fn reachability_matrix_count_is_distinct_and_finite() {
    let view = recursion_fixture();
    // No grouping key ⇒ the whole stream collapses to a single `count` row.
    let q = r#"MATCH (f{name:"f"})-[:CALLS*]->(m) RETURN count(*) AS reached"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.columns, vec!["reached"]);
    assert_eq!(table.rows, vec![vec![Value::Int(2)]]);
}

#[test]
fn count_of_empty_stream_is_zero() {
    // `h` calls nothing, so the reachable set is empty; `count(*)` collapses the
    // empty stream to a single `0` row (Cypher whole-stream aggregation).
    let view = recursion_fixture();
    let q = r#"MATCH (start{name:"h"})-[:CALLS*]->(m) RETURN count(*) AS reached"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.rows, vec![vec![Value::Int(0)]]);
}

/// Methods of a type, some called once / some twice, for grouped aggregation.
fn grouping_fixture() -> cgx_query::GraphView {
    use cgx_core::SymbolKind::{Method, Type};
    GraphBuilder::new()
        .sym("app::S", Type, "src/lib.rs", 1)
        .sym("app::S::a", Method, "src/lib.rs", 10)
        .sym("app::S::b", Method, "src/lib.rs", 20)
        .sym("app::c1", Function, "src/lib.rs", 30)
        .sym("app::c2", Function, "src/lib.rs", 40)
        // `a` is called twice, `b` once.
        .calls("app::c1", "app::S::a")
        .calls("app::c2", "app::S::a")
        .calls("app::c1", "app::S::b")
        .view()
}

#[test]
fn implicit_grouping_by_non_aggregate_key() {
    // `RETURN m.name, count(*)` groups by the non-aggregate key `m.name`, with
    // one row per callee and the count of incoming CALLS edges.
    let view = grouping_fixture();
    let q = r#"MATCH (caller)-[:CALLS]->(m:method)
               RETURN m.name AS callee, count(*) AS calls
               ORDER BY m.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.columns, vec!["callee", "calls"]);
    assert_eq!(
        table.rows,
        vec![
            vec![Value::Str("app::S::a".into()), Value::Int(2)],
            vec![Value::Str("app::S::b".into()), Value::Int(1)],
        ]
    );
}

#[test]
fn collect_preserves_deterministic_order() {
    // `collect(m.name)` with no grouping key gathers every callee in IF-8
    // (node-id) order: a (line 10) before b (line 20).
    let view = grouping_fixture();
    let q = r#"MATCH (caller)-[:CALLS]->(m:method) RETURN collect(m.name) AS callees"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.rows.len(), 1);
    match &table.rows[0][0] {
        Value::List(items) => {
            // a appears twice (called twice), b once; order is a,a,b by node id.
            let names: Vec<&str> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => s.as_str(),
                    _ => panic!("expected string"),
                })
                .collect();
            assert_eq!(names, vec!["app::S::a", "app::S::a", "app::S::b"]);
        }
        other => panic!("expected list, got {other:?}"),
    }
}

/// MIN/MAX over a per-path list comprehension, exercising the confidence
/// domain order (certain > probable > possible).
fn confidence_fixture() -> cgx_query::GraphView {
    use cgx_core::{Confidence, EdgeCondition, EdgeKind};
    GraphBuilder::new()
        .sym("a", Function, "src/a.rs", 1)
        .sym("b", Function, "src/b.rs", 1)
        .sym("c", Function, "src/c.rs", 1)
        // a -> b certain, b -> c possible: the weakest on the path is `possible`.
        .edge("a", "b", EdgeKind::Calls, EdgeCondition::Always, Confidence::Certain)
        .edge("b", "c", EdgeKind::Calls, EdgeCondition::Always, Confidence::Possible)
        .view()
}

#[test]
fn min_max_use_confidence_domain_order() {
    let view = confidence_fixture();
    // MIN over the path's edge confidences = the weakest link = "possible";
    // MAX = "certain". Lexicographically "certain" < "possible" < "probable",
    // so a wrong (lexicographic) MIN would yield "certain" — this proves the
    // domain order is used.
    let q = r#"MATCH path = (a{name:"a"})-[:CALLS*]->(c{name:"c"})
               RETURN MIN([r IN relationships(path) | r.confidence]) AS weakest,
                      MAX([r IN relationships(path) | r.confidence]) AS strongest"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.columns, vec!["weakest", "strongest"]);
    assert_eq!(
        table.rows,
        vec![vec![
            Value::Str("possible".into()),
            Value::Str("certain".into()),
        ]]
    );
}

#[test]
fn determinism_run_twice_byte_identical() {
    let view = recursion_fixture();
    let q = r#"MATCH (f{name:"f"})-[:CALLS*]->(m) RETURN count(*) AS reached"#;
    assert_eq!(run(&view, q).unwrap(), run(&view, q).unwrap());
}

#[test]
fn determinism_under_shuffled_edge_input() {
    // Same logical graph, edges fed to the builder in two different orders. The
    // store canonicalises edge order and the walker sorts neighbours, so the
    // grouped+ordered result must be identical regardless of input order.
    use cgx_core::SymbolKind::{Method, Type};

    let forward = GraphBuilder::new()
        .sym("app::S", Type, "src/lib.rs", 1)
        .sym("app::S::a", Method, "src/lib.rs", 10)
        .sym("app::S::b", Method, "src/lib.rs", 20)
        .sym("app::c1", Function, "src/lib.rs", 30)
        .sym("app::c2", Function, "src/lib.rs", 40)
        .calls("app::c1", "app::S::a")
        .calls("app::c2", "app::S::a")
        .calls("app::c1", "app::S::b")
        .view();

    let shuffled = GraphBuilder::new()
        .sym("app::S", Type, "src/lib.rs", 1)
        .sym("app::S::a", Method, "src/lib.rs", 10)
        .sym("app::S::b", Method, "src/lib.rs", 20)
        .sym("app::c1", Function, "src/lib.rs", 30)
        .sym("app::c2", Function, "src/lib.rs", 40)
        // Edges inserted in a different order.
        .calls("app::c1", "app::S::b")
        .calls("app::c2", "app::S::a")
        .calls("app::c1", "app::S::a")
        .view();

    let q = r#"MATCH (caller)-[:CALLS]->(m:method)
               RETURN m.name AS callee, count(*) AS calls ORDER BY m.name"#;
    let a = run(&forward, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    let b = run(&shuffled, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(a, b);
}

#[test]
fn default_if8_ordering_without_order_by() {
    // No ORDER BY: results come back in IF-8 (file, line, fqn) order, which for
    // this fixture is a (line 10) then b (line 20).
    let view = grouping_fixture();
    let q = r#"MATCH (caller)-[:CALLS]->(m:method) RETURN DISTINCT m.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    let names: Vec<&str> = table
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::Str(s) => s.as_str(),
            _ => panic!("expected string"),
        })
        .collect();
    assert_eq!(names, vec!["app::S::a", "app::S::b"]);
}
