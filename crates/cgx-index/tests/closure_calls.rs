//! TypeScript closure calls, end to end through the TS frontend, the link pass,
//! the signature post-pass and the store.
//!
//! A call through a `const` closure binding that nothing shadows has exactly one
//! possible target, so it must come out as one `certain` `calls-closure` edge to
//! that lambda and contribute nothing to the signature-compatible fan-out. A
//! call through a reassigned `let` binding keeps the fan-out over every
//! same-arity function value, as before.

mod common;

use cgx_core::confidence::Confidence;
use cgx_core::edge::EdgeKind;
use cgx_index::{default_registry, index_path};
use common::*;

const PROGRAM: &str = r#"
export function pinned(): number {
    const double = (x: number) => x * 2;
    return double(5);
}

export function rebound(): number {
    let inc = (x: number) => x + 1;
    inc = (x: number) => x + 2;
    return inc(1);
}

export function shadowed(double: (x: number) => number): number {
    return double(3);
}

export function one(x: number): number {
    return x;
}
"#;

fn index_program() -> (tempfile::TempDir, cgx_store::LinkedGraph) {
    index_source(PROGRAM)
}

fn index_source(program: &str) -> (tempfile::TempDir, cgx_store::LinkedGraph) {
    let (tmp, repo) = init_fixture_repo("ts-sample");
    let src_dir = repo.join("src");
    for entry in std::fs::read_dir(&src_dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            std::fs::remove_file(&p).unwrap();
        }
    }
    write_file(&repo, "src/closures.ts", program);
    commit_all(&repo, "closure program");

    let registry = default_registry();
    let mut store = mem_store();
    let outcome = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    let g = read_graph(&store, &outcome.graph_key);
    (tmp, g)
}

fn node_id(g: &cgx_store::LinkedGraph, suffix: &str) -> cgx_core::NodeId {
    g.nodes
        .iter()
        .find(|n| n.fqn.ends_with(suffix))
        .unwrap_or_else(|| panic!("no node ending in {suffix}"))
        .id
}

fn closure_edges(
    g: &cgx_store::LinkedGraph,
    caller: &str,
) -> Vec<cgx_core::EdgeRecord> {
    let src = node_id(g, caller);
    g.edges
        .iter()
        .filter(|e| e.src == src && e.kind == EdgeKind::CallsClosure)
        .cloned()
        .collect()
}

#[test]
fn const_closure_call_is_one_certain_edge_to_its_lambda() {
    let (_t, g) = index_program();
    let edges = closure_edges(&g, "::pinned");
    assert_eq!(edges.len(), 1, "one edge, no fan-out: {edges:?}");
    let e = &edges[0];
    assert_eq!(e.dst, node_id(&g, "::pinned::double"));
    assert_eq!(e.confidence, Confidence::Certain);
    assert_eq!(e.rule, "scope-ref");
}

#[test]
fn reassigned_and_shadowed_closure_calls_keep_the_signature_fan_out() {
    let (_t, g) = index_program();
    for caller in ["::rebound", "::shadowed"] {
        let edges = closure_edges(&g, caller);
        assert!(
            edges.len() > 1,
            "{caller}: expected the arity-1 fan-out, got {edges:?}"
        );
        assert!(edges
            .iter()
            .all(|e| e.rule == "sig-compat" && e.confidence == Confidence::Possible));
        let dsts: Vec<_> = edges.iter().map(|e| e.dst).collect();
        assert!(dsts.contains(&node_id(&g, "::one")), "{caller}");
        assert!(dsts.contains(&node_id(&g, "::pinned::double")), "{caller}");
    }
}

/// A `const` lambda declared in a `for` initializer shadows the module-level
/// one, but gets no definition of its own. The call must not be bound to the
/// outer lambda.
const FOR_INIT: &str = r#"
export const f = () => "outer";

export function g(): string {
    for (const f = () => "inner"; ; ) {
        return f();
    }
}
"#;

#[test]
fn closure_shadowed_by_a_for_initializer_is_not_bound_to_the_outer_lambda() {
    let (_t, g) = index_source(FOR_INIT);
    let outer = g
        .nodes
        .iter()
        .find(|n| n.fqn.ends_with("::closures::f"))
        .expect("outer lambda node")
        .id;
    let edges = closure_edges(&g, "::g");
    assert!(!edges.is_empty(), "the call is still recorded");
    assert!(
        !edges
            .iter()
            .any(|e| e.dst == outer && e.confidence == Confidence::Certain),
        "no certain edge to the outer lambda: {edges:?}"
    );
    assert!(edges.iter().all(|e| e.rule == "sig-compat"));
}
