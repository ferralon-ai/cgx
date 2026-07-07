//! Intraprocedural SSA dataflow coverage for the TypeScript/JavaScript adapter.
//!
//! Mirrors Go's / Python's `dataflow` test: each production site (declaration /
//! assignment / augmented / projection / call / return) lowers into a
//! `DataFlowFact` whose `DerivesFrom` direction is **derived → source**
//! (`b#1 → a#0`). A call result is opaque; a depth-2+ member access truncates to
//! the base local. TS type-coercions (`as T`, `!`, `await`) are transparent.

mod common;

use cgx_core::cut::CutMarker;
use cgx_core::transform::Transform;
use cgx_frontend::DataFlowFact;
use common::extract;

const FILE: &str = "src/df.ts";

fn flows(src: &str) -> Vec<DataFlowFact> {
    extract(FILE, src).data_flows
}

fn has_flow(f: &[DataFlowFact], derived: &str, source: &str, t: Transform) -> bool {
    f.iter().any(|x| {
        x.derived.last().map(String::as_str) == Some(derived)
            && x.source.last().map(String::as_str) == Some(source)
            && x.transform == t
    })
}

#[test]
fn copy_assignment_emits_copy_flow() {
    let f = flows("function g(a: number){ const b = a; return b; }\n");
    assert!(has_flow(&f, "b", "a", Transform::Copy), "got {f:?}");
}

#[test]
fn binary_emits_arith_per_operand() {
    let f = flows("function g(a: number, b: number){ const c = a + b; return c; }\n");
    assert!(has_flow(&f, "c", "a", Transform::Arith), "got {f:?}");
    assert!(has_flow(&f, "c", "b", Transform::Arith), "got {f:?}");
}

#[test]
fn unary_emits_arith() {
    let f = flows("function g(a: number){ const c = -a; return c; }\n");
    assert!(has_flow(&f, "c", "a", Transform::Arith), "got {f:?}");
}

#[test]
fn augmented_assignment_derives_from_self_and_operand() {
    // `x += y`: the new `x` derives from BOTH the prior `x` and `y`.
    let f = flows("function g(x: number, y: number){ x += y; return x; }\n");
    assert!(has_flow(&f, "x", "x", Transform::Arith), "self-dep: {f:?}");
    assert!(
        has_flow(&f, "x", "y", Transform::Arith),
        "operand-dep: {f:?}"
    );
    let aug = f
        .iter()
        .filter(|d| d.derived.last().map(String::as_str) == Some("x"))
        .map(|d| d.derived_version)
        .max();
    assert_eq!(aug, Some(1), "got {f:?}");
}

#[test]
fn member_projection_binds_to_base_local() {
    // Depth-1 `a.field` binds to the BASE local `a`, NOT the field name.
    let f = flows("function g(a: any){ const b = a.field; return b; }\n");
    let proj = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("b"))
        .expect("projection fact");
    assert_eq!(proj.transform, Transform::Projection);
    assert_eq!(
        proj.source.first().map(String::as_str),
        Some("a"),
        "base local must be the source root: {proj:?}"
    );
    assert!(
        proj.cut_markers.is_empty(),
        "depth-1 not truncated: {proj:?}"
    );
}

#[test]
fn subscript_projection_binds_to_base_local() {
    let f = flows("function g(a: any, i: number){ const b = a[i]; return b; }\n");
    let proj = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("b"))
        .expect("projection fact");
    assert_eq!(proj.transform, Transform::Projection);
    assert_eq!(
        proj.source.first().map(String::as_str),
        Some("a"),
        "{proj:?}"
    );
}

#[test]
fn deep_member_truncates_access_path() {
    let f = flows("function g(a: any){ const n = a.b.c; return n; }\n");
    let proj = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("n"))
        .expect("projection fact");
    assert_eq!(proj.transform, Transform::Projection);
    assert!(
        proj.cut_markers.contains(&CutMarker::TruncatedAccessPath),
        "got {proj:?}"
    );
    assert_eq!(proj.source.last().map(String::as_str), Some("a"));
}

#[test]
fn call_result_is_opaque_with_callee_and_args() {
    let f = flows(
        "function g(a: number){ const r = h(a); return r; }\nfunction h(x: number){ return x; }\n",
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
    assert!(
        opaque.source.is_empty(),
        "opaque source must be empty: {opaque:?}"
    );
}

#[test]
fn method_call_result_is_opaque_without_callee() {
    // `x.m(a)` is a virtual-receiver call: opaque, no syntactic callee FQN.
    let f = flows("function g(a: any, x: any){ const r = x.m(a); return r; }\n");
    let opaque = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("r"))
        .expect("opaque-call fact");
    assert!(opaque.cut_markers.contains(&CutMarker::OpaqueCall));
    assert_eq!(opaque.callee_fqn, None, "virtual call: {opaque:?}");
}

#[test]
fn as_cast_is_transparent_copy() {
    // `const b = a as Foo` is a copy from `a` (the coercion is transparent).
    let f = flows("function g(a: unknown){ const b = a as number; return b; }\n");
    assert!(has_flow(&f, "b", "a", Transform::Copy), "got {f:?}");
}

#[test]
fn await_call_is_opaque() {
    let f = flows("async function g(a: number){ const r = await h(a); return r; }\n");
    let opaque = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("r"))
        .expect("opaque await-call fact");
    assert!(
        opaque.cut_markers.contains(&CutMarker::OpaqueCall),
        "{opaque:?}"
    );
    assert_eq!(opaque.callee_fqn.as_deref(), Some("h"));
}

#[test]
fn return_references_latest_version() {
    let f = flows("function g(a: number){ const b = a; return b; }\n");
    let ret = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("return"))
        .expect("return fact");
    assert_eq!(ret.transform, Transform::Copy);
    assert_eq!(ret.source.last().map(String::as_str), Some("b"), "{ret:?}");
}

#[test]
fn reassignment_bumps_derived_version() {
    let f = flows("function g(a: number, b: number){ let x = a; x = b; return x; }\n");
    let mut versions: Vec<u32> = f
        .iter()
        .filter(|d| d.derived.last().map(String::as_str) == Some("x"))
        .map(|d| d.derived_version)
        .collect();
    versions.sort_unstable();
    assert_eq!(versions, vec![1, 2], "got {f:?}");
}

#[test]
fn array_literal_is_composed_from_elements() {
    let f = flows("function g(a: number, b: number){ const p = [a, b]; return p; }\n");
    assert!(has_flow(&f, "p", "a", Transform::Composed), "got {f:?}");
    assert!(has_flow(&f, "p", "b", Transform::Composed), "got {f:?}");
}

#[test]
fn destructuring_each_target_derives_from_rhs() {
    // `const [p, q] = f()`: best-effort — each target derives from the opaque call.
    let f = flows("function g(){ const [p, q] = f(); return p; }\n");
    let p = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("p"))
        .expect("p fact");
    let q = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("q"))
        .expect("q fact");
    assert!(p.cut_markers.contains(&CutMarker::OpaqueCall), "{p:?}");
    assert!(q.cut_markers.contains(&CutMarker::OpaqueCall), "{q:?}");
}

#[test]
fn flow_inside_conditional_carries_condition() {
    use cgx_core::condition::EdgeCondition;
    let f = flows("function g(a: number){ if (a) { const b = a; return b; } return a; }\n");
    let cond = f
        .iter()
        .find(|x| x.derived.last().map(String::as_str) == Some("b"))
        .expect("b fact");
    assert_eq!(cond.edge_condition, EdgeCondition::Conditional, "{cond:?}");
}

#[test]
fn no_phantom_param_source_facts() {
    // Parameters are SSA roots: they must never be a `derived` of a fact.
    let f = flows("function g(a: number, b: number){ const c = a + b; return c; }\n");
    assert!(
        !f.iter()
            .any(|x| { matches!(x.derived.last().map(String::as_str), Some("a") | Some("b")) }),
        "params must not be derived targets: {f:?}"
    );
}

#[test]
fn arrow_block_body_gets_dataflow() {
    // A named block-body arrow gets its own SSA pass.
    let f = flows("const g = (a: number) => { const b = a; return b; };\n");
    assert!(has_flow(&f, "b", "a", Transform::Copy), "got {f:?}");
}
