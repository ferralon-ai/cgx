//! Plan-time reject contract (design §4/§5): recognised-but-unbuilt features and
//! properties with no backing field are `Plan` errors naming the feature, never a
//! silent wrong answer or a generic syntax error.

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
    let err = run(&view(), q).expect_err("expected a Plan reject");
    assert_eq!(err.kind, ErrorKind::Plan, "query: {q}");
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
