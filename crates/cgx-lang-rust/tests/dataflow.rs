//! v0.3 DATA_FLOW SC2: intraprocedural SSA dataflow-fact extraction.
//!
//! Each test exercises one production site (design §1.1) via a focused inline
//! source and asserts the emitted [`DataFlowFact`]s: the transform tag, the
//! source/derived name-paths, and the SSA versioning that makes re-assignment
//! flow-sensitive.

mod common;

use cgx_core::transform::Transform;
use cgx_frontend::DataFlowFact;
use common::extract;

const FILE: &str = "src/sample.rs";

/// Facts whose owning function (by enclosing-scope) short name is `name`. Since
/// the frontend keys nothing by fn name here, we filter by the `derived` head for
/// returns and by span proximity is unnecessary — tests use single-fn sources.
fn flows(src: &str) -> Vec<DataFlowFact> {
    extract(FILE, src).data_flows
}

fn transforms(facts: &[DataFlowFact]) -> Vec<Transform> {
    facts.iter().map(|f| f.transform).collect()
}

fn has_flow(facts: &[DataFlowFact], derived: &str, source: &str, t: Transform) -> bool {
    facts.iter().any(|f| {
        f.derived.last().map(String::as_str) == Some(derived)
            && f.source.last().map(String::as_str) == Some(source)
            && f.transform == t
    })
}

#[test]
fn let_copy_emits_copy_flow() {
    let f = flows("fn g(a: i32) -> i32 { let b = a; b }");
    assert!(has_flow(&f, "b", "a", Transform::Copy), "got {f:?}");
}

#[test]
fn binary_expr_emits_arith_flow_per_operand() {
    let f = flows("fn g(a: i32, b: i32) -> i32 { let c = a + b; c }");
    assert!(has_flow(&f, "c", "a", Transform::Arith), "got {f:?}");
    assert!(has_flow(&f, "c", "b", Transform::Arith), "got {f:?}");
}

#[test]
fn depth1_field_emits_projection() {
    let f = flows("fn g(u: U) -> i32 { let n = u.name; n }");
    // Projection source is the depth-1 access path; its last segment is the field.
    assert!(
        f.iter()
            .any(|x| x.transform == Transform::Projection
                && x.source.first().map(String::as_str) == Some("u")),
        "got {f:?}"
    );
}

#[test]
fn depth2_field_truncates_with_cut() {
    let f = flows("fn g(u: U) -> i32 { let t = u.cfg.timeout; t }");
    let proj = f
        .iter()
        .find(|x| x.transform == Transform::Projection)
        .expect("a projection flow");
    assert!(
        proj.cut_markers
            .contains(&cgx_core::cut::CutMarker::TruncatedAccessPath),
        "depth-2 access must carry a truncated-access-path cut: {proj:?}"
    );
    // The retained source is the base value `u`.
    assert_eq!(proj.source.last().map(String::as_str), Some("u"));
}

#[test]
fn struct_assembly_emits_composed() {
    let f = flows("fn g(a: i32, b: i32) -> P { let p = P { x: a, y: b }; p }");
    assert!(has_flow(&f, "p", "a", Transform::Composed), "got {f:?}");
    assert!(has_flow(&f, "p", "b", Transform::Composed), "got {f:?}");
}

#[test]
fn conditional_select_emits_branched() {
    let f = flows("fn g(c: bool, a: i32, b: i32) -> i32 { let v = if c { a } else { b }; v }");
    assert!(has_flow(&f, "v", "a", Transform::Branched), "got {f:?}");
    assert!(has_flow(&f, "v", "b", Transform::Branched), "got {f:?}");
}

#[test]
fn return_expr_emits_copy_to_return() {
    let f = flows("fn g(a: i32) -> i32 { return a; }");
    assert!(
        f.iter().any(|x| x.derived.last().map(String::as_str) == Some("return")
            && x.source.last().map(String::as_str) == Some("a")
            && x.transform == Transform::Copy),
        "got {f:?}"
    );
}

#[test]
fn reassignment_yields_distinct_ssa_versions() {
    // `b = a; b = b + 1;` — criterion 2: two distinct value defs of `b`.
    let f = flows("fn g(a: i32) -> i32 { let b = a; let b = b + 1; b }");
    let versions: Vec<u32> = f
        .iter()
        .filter(|x| x.derived.last().map(String::as_str) == Some("b"))
        .map(|x| x.derived_version)
        .collect();
    assert!(
        versions.contains(&1) && versions.contains(&2),
        "expected versions 1 and 2 of b, got {versions:?} from {f:?}"
    );
}

#[test]
fn call_result_records_opaque_call_no_source() {
    // `let r = h(a);` — SC2 records an opaque-call cut and emits NO source flow.
    let f = flows("fn g(a: i32) -> i32 { let r = h(a); r }");
    // No fact should claim r derives from a *through* the call.
    assert!(
        !has_flow(&f, "r", "a", Transform::Copy),
        "a call result must not assert a false cross-function flow: {f:?}"
    );
}

#[test]
fn condition_label_is_lowered_inside_if_arm() {
    // A flow inside an if-arm body is `conditional`, matching call-edge lowering.
    let f = flows("fn g(c: bool, a: i32) -> i32 { let mut b = 0; if c { b = a; } b }");
    assert!(
        f.iter().any(|x| x.derived.last().map(String::as_str) == Some("b")
            && x.source.last().map(String::as_str) == Some("a")
            && x.edge_condition == cgx_core::condition::EdgeCondition::Conditional),
        "got {f:?}"
    );
}

#[test]
fn no_dataflow_facts_for_pure_declarations() {
    // A file with no assignments emits no dataflow facts.
    let f = flows("struct S; trait T { fn m(&self); }");
    assert!(transforms(&f).is_empty(), "got {f:?}");
}
