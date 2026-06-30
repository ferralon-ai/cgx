//! Determinism: canonicalized extraction of the same source must be
//! byte-identical across runs (stable walk order + `BTreeMap`/scope-id
//! invariants), independent of which method scopes the walker happens to open.

mod common;

use common::{def_fqns, extract, extract_fixture};

#[test]
fn fixture_extraction_is_identical_across_runs() {
    let a = extract_fixture("Shapes.java");
    let b = extract_fixture("Shapes.java");
    assert_eq!(a, b, "canonical facts must be identical across extractions");
}

#[test]
fn inline_extraction_is_identical_across_runs() {
    // Several sibling members in one class exercise stable ordering of defs that
    // share a scope.
    let src = "package a.b;\nclass C {\n  private int x, y;\n  public void p() {}\n  private void q() {}\n  static class N { protected int z; }\n}\n";
    let a = extract("C.java", src);
    let b = extract("C.java", src);
    assert_eq!(a, b);
}

#[test]
fn fixture_def_set_is_complete_and_stable() {
    let facts = extract_fixture("Shapes.java");
    let fqns = def_fqns(&facts);
    for expected in [
        "com::example::shapes::Shapes",
        "com::example::shapes::Shapes::Shapes",
        "com::example::shapes::Shapes::count",
        "com::example::shapes::Shapes::label",
        "com::example::shapes::Shapes::note",
        "com::example::shapes::Shapes::total",
        "com::example::shapes::Shapes::reset",
        "com::example::shapes::Shapes::Helper",
        "com::example::shapes::Shapes::Helper::seed",
        "com::example::shapes::Drawable",
        "com::example::shapes::Drawable::draw",
        "com::example::shapes::Color",
        "com::example::shapes::Color::isPrimary",
        "com::example::shapes::Point",
        "com::example::shapes::Point::sum",
        "com::example::shapes::Base",
        "com::example::shapes::Base::run",
    ] {
        assert!(fqns.contains(&expected), "missing {expected}; have {fqns:?}");
    }
}
