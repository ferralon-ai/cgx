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
fn refs_imports_hints_are_identical_across_runs() {
    // A body with calls under several edge conditions, an import, an entrypoint,
    // and a cut all share the canonical sort path.
    let src = "package a.b;\nimport java.util.List;\nclass C {\n  public static void main(String[] a) {\n    g();\n    if (a.length > 0) { new Widget(); }\n    try { risky(); } catch (Exception e) { recover(); }\n    Class.forName(\"x\");\n  }\n}\n";
    let a = extract("C.java", src);
    let b = extract("C.java", src);
    assert_eq!(a, b);
    assert!(!a.refs.is_empty() && !a.imports.is_empty() && !a.entrypoint_hints.is_empty());
}

#[test]
fn inheritance_relations_are_identical_across_runs() {
    // The lattice fixture exercises Inherits/Implements/Overrides, whose
    // accumulation (BTreeMap-keyed, flushed in finish) must canonicalize to a
    // byte-identical order across runs.
    let a = extract_fixture("Lattice.java");
    let b = extract_fixture("Lattice.java");
    assert_eq!(a, b, "canonical inheritance relations must be stable");
    assert!(
        !a.impl_relations.is_empty(),
        "the lattice fixture must emit impl_relations"
    );
}

#[test]
fn effect_facts_are_identical_across_runs() {
    // The effects fixture exercises every effect kind (io.*, nondeterministic,
    // dynamic-code, blocking, spawns); the BTreeMap-keyed accumulator flushed in
    // finish must canonicalize to a byte-identical order across runs.
    let a = extract_fixture("Effects.java");
    let b = extract_fixture("Effects.java");
    assert_eq!(a, b, "canonical effect facts must be stable");
    assert!(
        !a.effects.is_empty(),
        "the effects fixture must emit effect facts"
    );
}

#[test]
fn data_flow_facts_are_identical_across_runs() {
    // The dataflow fixture exercises every lowered construct (copy chain, arith,
    // projection, deep-field truncation, opaque call, ternary, reassignment,
    // for-each binding); the BTreeMap-keyed accumulator flushed in finish + the
    // canonical sort must produce a byte-identical fact vector across runs.
    let a = extract_fixture("Dataflow.java");
    let b = extract_fixture("Dataflow.java");
    assert_eq!(a, b, "canonical dataflow facts must be stable");
    assert!(
        !a.data_flows.is_empty(),
        "the dataflow fixture must emit data_flow facts"
    );
}

#[test]
fn full_extraction_is_byte_identical_across_runs() {
    // The whole channel set in one source: defs, refs/conditions, imports,
    // entrypoints, cuts, inheritance, effects AND intraprocedural dataflow. The
    // canonicalized facts (every Vec sorted, scope ids stable) must be equal
    // across two independent extractions of the same bytes.
    let src = "package a.b;\n\
        import java.util.List;\n\
        class C extends Base implements Runnable {\n\
        \x20 private int total;\n\
        \x20 public synchronized void run() {\n\
        \x20   int a = total;\n\
        \x20   int b = a + 1;\n\
        \x20   int c = b > 0 ? a : b;\n\
        \x20   int r = helper(c);\n\
        \x20   for (String s : names()) { System.out.println(s); }\n\
        \x20   try { risky(); } catch (Exception e) { recover(); }\n\
        \x20 }\n\
        \x20 int helper(int x) { return x; }\n\
        \x20 List<String> names() { return null; }\n\
        \x20 void risky() {}\n\
        \x20 void recover() {}\n\
        }\n";
    let a = extract("C.java", src);
    let b = extract("C.java", src);
    assert_eq!(a, b, "full canonical extraction must be byte-identical");
    assert!(
        !a.defs.is_empty()
            && !a.refs.is_empty()
            && !a.data_flows.is_empty()
            && !a.effects.is_empty(),
        "the full-channel source must populate defs, refs, dataflow and effects"
    );
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
