//! Plan-time reject contract (design §4/§5): recognised-but-unbuilt features and
//! properties with no backing field are `Plan` errors naming the feature, never a
//! silent wrong answer or a generic syntax error.
//!
//! Coverage matrix — one test per entry from the §4 reject tables + §2 "Coverage OUT":
//!
//! Edge types (Theme-13): CALLS:super, RESOLVES_TO, PROVIDES_BODY, SHADOWS_FIELD, FULFILLS
//! Edge types (other deferred): COMPATIBLE_WITH
//! Node properties (unbacked): confidence, col, scope, type, introduced_in_branch,
//!   is_exit, in_degree, out_degree
//! Edge properties (unbacked): transformation, via, taint_label, site
//! Deferred clauses: IS EMPTY, MATCH ALL…MUST PASS THROUGH/AVOIDING, CREATE/SET/DELETE,
//!   UNWIND, OPTIONAL MATCH, UNION
//! Deferred syntax: query parameters ($x), arithmetic expressions (+)

mod common;

use cgx_core::SymbolKind;
use cgx_cql::{run, ErrorKind};

use common::GraphBuilder;

fn view() -> cgx_query::GraphView {
    GraphBuilder::new()
        .sym("a", SymbolKind::Function, "src/lib.rs", 10)
        .sym("b", SymbolKind::Function, "src/lib.rs", 20)
        .calls("a", "b")
        .view()
}

fn reject(q: &str) -> String {
    let err = run(&view(), q).expect_err(&format!("expected a Plan reject for: {q}"));
    assert_eq!(err.kind, ErrorKind::Plan, "expected Plan error, got {:?} for: {q}", err.kind);
    err.message
}

#[test]
fn rejects_node_confidence() {
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.confidence = "certain" RETURN a.name"#);
    assert!(m.contains("confidence"), "{m}");
    assert!(m.contains("edge attribute"), "{m}");
}

#[test]
fn rejects_node_scope() {
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.scope = "x" RETURN a.name"#);
    assert!(m.contains("scope"), "{m}");
}

#[test]
fn rejects_edge_transformation() {
    let m =
        reject(r#"MATCH (a)-[r:DATA_FLOW]->(b) RETURN r.transformation"#);
    assert!(m.contains("transformation"), "{m}");
    assert!(m.contains("deferred"), "{m}");
}

#[test]
fn rejects_deferred_edge_type() {
    let m = reject(r#"MATCH (a)-[:RESOLVES_TO]->(b) RETURN a.name"#);
    assert!(m.contains("RESOLVES_TO"), "{m}");
    assert!(m.contains("Theme-13"), "{m}");
}

#[test]
fn rejects_compatible_with() {
    let m = reject(r#"MATCH (a)-[:COMPATIBLE_WITH]->(b) RETURN a.name"#);
    assert!(m.contains("COMPATIBLE_WITH"), "{m}");
}

#[test]
fn rejects_unknown_edge_type() {
    let m = reject(r#"MATCH (a)-[:FROBNICATE]->(b) RETURN a.name"#);
    assert!(m.contains("FROBNICATE"), "{m}");
}

#[test]
fn rejects_unknown_node_property() {
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) RETURN a.wibble"#);
    assert!(m.contains("wibble"), "{m}");
}

#[test]
fn rejects_anonymous_interior_node_in_chain() {
    // A chained pattern joins on the interior node's variable; an anonymous
    // interior node has nothing to join on, so it is a Plan reject.
    let m = reject(r#"MATCH (a)-[:CALLS]->()-[:CALLS]->(c) RETURN c.name"#);
    assert!(m.contains("interior"), "{m}");
    assert!(m.contains("named"), "{m}");
}

#[test]
fn rejects_path_binding_over_multi_relationship() {
    // A `path =` binding spanning multiple relationships is not yet supported.
    let m = reject(r#"MATCH p = (a)-[:CALLS]->(b)-[:CALLS]->(c) RETURN p"#);
    assert!(m.contains("path"), "{m}");
    assert!(m.contains("multi-relationship"), "{m}");
}

// ── Theme-13 edge types (each individually) ──────────────────────────────────

#[test]
fn rejects_calls_super() {
    // `CALLS:super` uses the sub-type qualifier syntax; recognised as Theme-13.
    let m = reject(r#"MATCH (a)-[:CALLS:super]->(b) RETURN a.name"#);
    assert!(m.contains("Theme-13") || m.contains("CALLS:") || m.contains("sub-type"), "{m}");
    assert!(m.contains("deferred"), "{m}");
}

#[test]
fn rejects_provides_body() {
    let m = reject(r#"MATCH (a)-[:PROVIDES_BODY]->(b) RETURN a.name"#);
    assert!(m.contains("PROVIDES_BODY"), "{m}");
    assert!(m.contains("Theme-13"), "{m}");
}

#[test]
fn rejects_shadows_field() {
    let m = reject(r#"MATCH (a)-[:SHADOWS_FIELD]->(b) RETURN a.name"#);
    assert!(m.contains("SHADOWS_FIELD"), "{m}");
    assert!(m.contains("Theme-13"), "{m}");
}

#[test]
fn rejects_fulfills() {
    let m = reject(r#"MATCH (a)-[:FULFILLS]->(b) RETURN a.name"#);
    assert!(m.contains("FULFILLS"), "{m}");
    assert!(m.contains("Theme-13"), "{m}");
}

// ── Unbacked node properties ──────────────────────────────────────────────────

#[test]
fn rejects_node_col() {
    // `col` has no backing field on symbol nodes (ADR-01).
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.col = 1 RETURN a.name"#);
    assert!(m.contains("col"), "{m}");
}

#[test]
fn rejects_node_type() {
    // `type` has no backing field (Canonical Example 3b substitution, design §5).
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.type = "Foo" RETURN a.name"#);
    assert!(m.contains("type"), "{m}");
}

#[test]
fn rejects_node_introduced_in_branch() {
    let m = reject(
        r#"MATCH (a)-[:CALLS]->(b) WHERE a.introduced_in_branch = "main" RETURN a.name"#,
    );
    assert!(m.contains("introduced_in_branch"), "{m}");
}

#[test]
fn rejects_node_is_exit() {
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.is_exit = true RETURN a.name"#);
    assert!(m.contains("is_exit"), "{m}");
}

#[test]
fn rejects_node_in_degree() {
    // `in_degree` is not materialized on the node record (GM-1.3).
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.in_degree = 1 RETURN a.name"#);
    assert!(m.contains("in_degree"), "{m}");
}

#[test]
fn rejects_node_out_degree() {
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.out_degree = 0 RETURN a.name"#);
    assert!(m.contains("out_degree"), "{m}");
}

// ── Unbacked edge properties ──────────────────────────────────────────────────

#[test]
fn rejects_edge_via() {
    let m = reject(r#"MATCH (a)-[r:DATA_FLOW]->(b) RETURN r.via"#);
    assert!(m.contains("via"), "{m}");
}

#[test]
fn rejects_edge_taint_label() {
    let m = reject(r#"MATCH (a)-[r:DATA_FLOW]->(b) RETURN r.taint_label"#);
    assert!(m.contains("taint_label"), "{m}");
}

#[test]
fn rejects_edge_site() {
    // `site.*` — the site sub-properties are not queryable (ADR-01).
    let m = reject(r#"MATCH (a)-[r:CALLS]->(b) RETURN r.site"#);
    assert!(m.contains("site"), "{m}");
}

// ── Deferred clause-level features ───────────────────────────────────────────

#[test]
fn rejects_is_empty() {
    // `IS EMPTY` requires type reconstruction; deferred.
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.name IS EMPTY RETURN a.name"#);
    assert!(m.contains("IS EMPTY") || m.contains("IS"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}

#[test]
fn rejects_match_all_guarded_cut() {
    // Q-20: `MATCH ALL … MUST PASS THROUGH/AVOIDING` is deferred.
    let m = reject(r#"MATCH ALL (a)-[:CALLS]->(b) MUST PASS THROUGH (c) RETURN a.name"#);
    assert!(m.contains("MATCH ALL") || m.contains("guarded"), "{m}");
    assert!(m.contains("deferred"), "{m}");
}

#[test]
fn rejects_create_clause() {
    // cgx query is read-only; write clauses are deferred.
    let m = reject(r#"CREATE (a {name: "x"}) RETURN a.name"#);
    assert!(m.contains("CREATE") || m.contains("read-only"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}

#[test]
fn rejects_set_clause() {
    // `SET` is a write clause; deferred.
    let m = reject(r#"SET a.name = "x" RETURN a.name"#);
    assert!(m.contains("SET") || m.contains("read-only"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}

#[test]
fn rejects_delete_clause() {
    let m = reject(r#"DELETE a RETURN a.name"#);
    assert!(m.contains("DELETE") || m.contains("read-only"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}

#[test]
fn rejects_unwind_clause() {
    let m = reject(r#"UNWIND [1,2,3] AS x RETURN x"#);
    assert!(m.contains("UNWIND"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}

#[test]
fn rejects_optional_match() {
    let m = reject(r#"OPTIONAL MATCH (a)-[:CALLS]->(b) RETURN a.name"#);
    assert!(m.contains("OPTIONAL"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}

#[test]
fn rejects_union() {
    let m = reject(
        r#"MATCH (a)-[:CALLS]->(b) RETURN a.name UNION MATCH (c)-[:CALLS]->(d) RETURN c.name"#,
    );
    assert!(m.contains("UNION"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}

// ── Deferred syntax features ──────────────────────────────────────────────────

#[test]
fn rejects_parameters() {
    // Query parameters (`$name`) are deferred.
    let m = reject(r#"MATCH (a {name: $node_name})-[:CALLS]->(b) RETURN a.name"#);
    assert!(m.contains("parameter") || m.contains("$"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}

#[test]
fn rejects_arithmetic_addition() {
    // Arithmetic expressions are deferred; `+` gives a Plan error at lex time.
    let m = reject(r#"MATCH (a)-[:CALLS]->(b) WHERE a.line + 1 = 5 RETURN a.name"#);
    assert!(m.contains("arithmetic") || m.contains("+"), "{m}");
    assert!(m.contains("deferred") || m.contains("not supported"), "{m}");
}
