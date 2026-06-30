//! Intraprocedural dataflow (SSA): a method/constructor body lowers each
//! production site into `DataFlowFact`s carrying SSA value nodes + `DerivesFrom`
//! edges (derived→source). Mirrors `cgx-lang-go/tests/dataflow.rs` with Java
//! fixtures; the model (fact shape, versioning, `OpaqueCall`/`TruncatedAccessPath`
//! under-approximation) is identical to the Go adapter.

mod common;

use cgx_core::cut::CutMarker;
use cgx_core::transform::Transform;
use cgx_frontend::DataFlowFact;
use common::extract;

const FILE: &str = "Df.java";

fn flows(src: &str) -> Vec<DataFlowFact> {
    extract(FILE, src).data_flows
}

/// A `derived --derives-from(transform)--> source` edge, compared on the final
/// access-path segment of each endpoint (the value-node base name).
fn has_flow(f: &[DataFlowFact], derived: &str, source: &str, t: Transform) -> bool {
    f.iter().any(|x| {
        x.derived.last().map(String::as_str) == Some(derived)
            && x.source.last().map(String::as_str) == Some(source)
            && x.transform == t
    })
}

#[test]
fn local_var_copy_emits_copy_flow() {
    let f = flows("class C {\n  int g(int a) { int b = a; return b; }\n}\n");
    assert!(has_flow(&f, "b", "a", Transform::Copy), "got {f:?}");
}

#[test]
fn assignment_to_return_flows_param_through() {
    // `b ⇝ a` (copy) and `return ⇝ b` (copy): the chain a ⇝ return is recoverable.
    let f = flows("class C {\n  int g(int a) { int b = a; return b; }\n}\n");
    assert!(
        has_flow(&f, "return", "b", Transform::Copy),
        "return must derive from b: {f:?}"
    );
}

#[test]
fn param_flows_directly_to_return() {
    let f = flows("class C {\n  int id(int a) { return a; }\n}\n");
    assert!(
        has_flow(&f, "return", "a", Transform::Copy),
        "return must derive from the returned param: {f:?}"
    );
}

#[test]
fn binary_emits_arith_per_operand() {
    let f = flows("class C {\n  int g(int a, int b) { int c = a + b; return c; }\n}\n");
    assert!(has_flow(&f, "c", "a", Transform::Arith), "got {f:?}");
    assert!(has_flow(&f, "c", "b", Transform::Arith), "got {f:?}");
}

#[test]
fn field_projection_emits_projection_flow() {
    let f = flows(
        "class C {\n  static class U { int name; }\n  int g(U u) { int n = u.name; return n; }\n}\n",
    );
    assert!(
        has_flow(&f, "n", "name", Transform::Projection),
        "got {f:?}"
    );
}

#[test]
fn deep_field_truncates_access_path() {
    let f = flows(
        "class C {\n  static class V { int t; }\n  static class U { V c; }\n  int g(U u) { int n = u.c.t; return n; }\n}\n",
    );
    let proj = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("n"))
        .expect("projection fact");
    assert_eq!(proj.transform, Transform::Projection);
    assert!(
        proj.cut_markers.contains(&CutMarker::TruncatedAccessPath),
        "got {proj:?}"
    );
    // Truncated to the base local `u`.
    assert_eq!(proj.source.last().map(String::as_str), Some("u"));
}

#[test]
fn call_result_is_opaque_with_callee_and_args() {
    let f = flows(
        "class C {\n  int g(int a) { int r = h(a); return r; }\n  int h(int x) { return x; }\n}\n",
    );
    let opaque = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("r"))
        .expect("opaque-call fact");
    assert!(
        opaque.cut_markers.contains(&CutMarker::OpaqueCall),
        "got {opaque:?}"
    );
    assert_eq!(opaque.callee_fqn.as_deref(), Some("h"));
    assert_eq!(
        opaque
            .args
            .first()
            .and_then(|a| a.last())
            .map(String::as_str),
        Some("a")
    );
    // An opaque call must NOT over-claim an intraprocedural source edge.
    assert!(
        opaque.source.is_empty(),
        "opaque source must be empty: {opaque:?}"
    );
}

#[test]
fn object_creation_is_opaque_with_type_callee() {
    let f = flows(
        "class C {\n  Object g(int a) { Object r = new Box(a); return r; }\n  static class Box { Box(int x) {} }\n}\n",
    );
    let opaque = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("r"))
        .expect("opaque new fact");
    assert!(
        opaque.cut_markers.contains(&CutMarker::OpaqueCall),
        "got {opaque:?}"
    );
    assert_eq!(opaque.callee_fqn.as_deref(), Some("Box"));
    assert_eq!(
        opaque.args.first().and_then(|a| a.last()).map(String::as_str),
        Some("a")
    );
    assert!(opaque.source.is_empty(), "got {opaque:?}");
}

#[test]
fn ternary_selects_from_both_branches() {
    let f = flows("class C {\n  int g(boolean c, int a, int b) { int x = c ? a : b; return x; }\n}\n");
    assert!(has_flow(&f, "x", "a", Transform::Branched), "got {f:?}");
    assert!(has_flow(&f, "x", "b", Transform::Branched), "got {f:?}");
}

#[test]
fn for_each_binds_loop_var_from_iterable() {
    let f = flows(
        "class C {\n  int g(int[] items) { int last = 0; for (int e : items) { last = e; } return last; }\n}\n",
    );
    // The loop var derives from the iterable (Java-specific lowering, `Other`).
    assert!(has_flow(&f, "e", "items", Transform::Other), "got {f:?}");
    // ... and `last` derives from the loop var, so last ⇝ e ⇝ items is recoverable.
    assert!(has_flow(&f, "last", "e", Transform::Copy), "got {f:?}");
}

#[test]
fn reassignment_bumps_derived_version() {
    let f = flows("class C {\n  int g(int a, int b) { int x = a; x = b; return x; }\n}\n");
    let mut versions: Vec<u32> = f
        .iter()
        .filter(|d| d.derived.last().map(String::as_str) == Some("x"))
        .map(|d| d.derived_version)
        .collect();
    versions.sort_unstable();
    assert_eq!(versions, vec![1, 2], "got {f:?}");
}

#[test]
fn for_each_over_call_result_claims_no_source() {
    // An iterable that is a call result is opaque — the loop var must NOT claim a
    // spurious intraprocedural source (under-approximation, Go parity).
    let f = flows(
        "class C {\n  void g() { for (String s : list()) { sink(s); } }\n  java.util.List<String> list() { return null; }\n}\n",
    );
    assert!(
        !f.iter()
            .any(|x| x.derived.last().map(String::as_str) == Some("s") && !x.source.is_empty()),
        "loop var over an opaque iterable must claim no source: {f:?}"
    );
}
