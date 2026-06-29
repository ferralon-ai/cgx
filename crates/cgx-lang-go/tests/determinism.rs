mod common;

use common::extract_fixture;

#[test]
fn extraction_is_byte_identical_across_runs() {
    let a = extract_fixture("interfaces.go");
    let b = extract_fixture("interfaces.go");
    assert_eq!(a, b, "canonical facts must be identical across extractions");
}

#[test]
fn fixture_defs_present() {
    let facts = extract_fixture("interfaces.go");
    let fqns: Vec<&str> = facts.defs.iter().map(|d| d.fqn.as_str()).collect();
    assert!(fqns.iter().any(|f| f.ends_with("::Reader")), "{fqns:?}");
    assert!(
        fqns.iter().any(|f| f.ends_with("::(*File)::Read")),
        "{fqns:?}"
    );
    assert!(fqns.iter().any(|f| f.ends_with("::Consume")), "{fqns:?}");
}

#[test]
fn fixture_imports_present_by_specifier_not_order() {
    // `gofmt` alphabetizes the import block in `imports.go`, so assert on the
    // specifier set rather than positional order.
    let facts = extract_fixture("imports.go");
    let specs: Vec<&str> = facts.imports.iter().map(|i| i.specifier.as_str()).collect();
    assert!(specs.contains(&"math"), "{specs:?}");
    assert!(specs.contains(&"net/http"), "{specs:?}");
    assert!(specs.contains(&"net/http/pprof"), "{specs:?}");
    // The dot-import (`. "math"`) is a glob; the blank import binds no names.
    let math = facts
        .imports
        .iter()
        .find(|i| i.specifier == "math")
        .unwrap();
    assert!(math.glob, "dot-import must be glob: {math:?}");
    let pprof = facts
        .imports
        .iter()
        .find(|i| i.specifier == "net/http/pprof")
        .unwrap();
    assert!(
        pprof.names.is_empty(),
        "blank import has no names: {pprof:?}"
    );
}

#[test]
fn fixture_dataflow_chain_present() {
    // `dataflow.go`'s `Transform`: a → b (copy), b → c (arith), c → return.
    use cgx_core::transform::Transform;
    let facts = extract_fixture("dataflow.go");
    let f = &facts.data_flows;
    let copy_ba = f.iter().any(|x| {
        x.derived.last().map(String::as_str) == Some("b")
            && x.source.last().map(String::as_str) == Some("a")
            && x.transform == Transform::Copy
    });
    let arith_cb = f.iter().any(|x| {
        x.derived.last().map(String::as_str) == Some("c")
            && x.source.last().map(String::as_str) == Some("b")
            && x.transform == Transform::Arith
    });
    let ret_c = f.iter().any(|x| {
        x.derived.last().map(String::as_str) == Some("return")
            && x.source.last().map(String::as_str) == Some("c")
    });
    assert!(copy_ba, "missing b<-a copy: {f:?}");
    assert!(arith_cb, "missing c<-b arith: {f:?}");
    assert!(ret_c, "missing return<-c: {f:?}");
}
