mod common;

use cgx_core::cut::CutMarker;
use cgx_core::transform::Transform;
use cgx_frontend::DataFlowFact;
use common::extract;

const FILE: &str = "app/df.go";

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
fn short_var_copy_emits_copy_flow() {
    let f = flows("package df\nfunc g(a int) int { b := a; return b }\n");
    assert!(has_flow(&f, "b", "a", Transform::Copy), "got {f:?}");
}

#[test]
fn binary_emits_arith_per_operand() {
    let f = flows("package df\nfunc g(a int, b int) int { c := a + b; return c }\n");
    assert!(has_flow(&f, "c", "a", Transform::Arith), "got {f:?}");
    assert!(has_flow(&f, "c", "b", Transform::Arith), "got {f:?}");
}

#[test]
fn field_projection_emits_projection_flow() {
    let f =
        flows("package df\ntype U struct{ name int }\nfunc g(u U) int { n := u.name; return n }\n");
    assert!(
        has_flow(&f, "n", "name", Transform::Projection),
        "got {f:?}"
    );
}

#[test]
fn deep_field_truncates_access_path() {
    let f = flows(
        "package df\ntype U struct{ c C }\ntype C struct{ t int }\nfunc g(u U) int { n := u.c.t; return n }\n",
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
    // Truncated to the base identifier `u`.
    assert_eq!(proj.source.last().map(String::as_str), Some("u"));
}

#[test]
fn call_result_is_opaque_with_callee_and_args() {
    let f = flows(
        "package df\nfunc g(a int) int { r := h(a); return r }\nfunc h(x int) int { return x }\n",
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
fn reassignment_bumps_derived_version() {
    let f = flows("package df\nfunc g(a int, b int) int { x := a; x = b; return x }\n");
    let mut versions: Vec<u32> = f
        .iter()
        .filter(|d| d.derived.last().map(String::as_str) == Some("x"))
        .map(|d| d.derived_version)
        .collect();
    versions.sort_unstable();
    assert_eq!(versions, vec![1, 2], "got {f:?}");
}
