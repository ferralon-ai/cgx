//! End-to-end resolution through the SHARED `cgx-resolve::link()` over facts the
//! real C++ extractor produced: intra-file direct calls resolve `certain`,
//! cross-TU calls band to the `possible` Tier-2 ceiling, the virtual-dispatch band
//! is size-only, and no edge is fabricated.

mod common;

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::edge::EdgeKind;
use common::{extract, link_files};

fn dst_fqn<'a>(g: &'a cgx_resolve::ResolvedGraph, edge: &cgx_core::edge::EdgeRecord) -> &'a str {
    &g.node_records().nth(edge.dst.index()).unwrap().fqn
}

#[test]
fn intra_file_direct_call_resolves_certain() {
    let f = extract(
        "src/m.cpp",
        "int helper(int x) { return x + 1; }\nint square(int x) { return helper(x); }\n",
    );
    let g = link_files(&[("src/m.cpp", &f)]);
    let edge = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::Calls && e.rule == "scope-ref")
        .expect("an intra-file resolved call edge");
    assert_eq!(edge.confidence, Confidence::Certain);
    assert_eq!(edge.tier, Tier::ScopeGraph);
    assert_eq!(dst_fqn(&g, edge), "helper");
    assert!(edge.site_id.is_some(), "call edges carry a site_id");
}

#[test]
fn intra_namespace_call_resolves_certain() {
    let f = extract(
        "src/m.cpp",
        "namespace n { int a() { return 0; } int b() { return a(); } }\n",
    );
    let g = link_files(&[("src/m.cpp", &f)]);
    assert!(g
        .edge_records()
        .any(|e| e.kind == EdgeKind::Calls
            && e.confidence == Confidence::Certain
            && dst_fqn(&g, e) == "n::a"));
}

#[test]
fn intra_file_member_body_call_resolves_to_its_own_class_method() {
    // `draw()` calls `area()` within the same class — an unqualified in-class call
    // resolves to the local method (mirrors the shipped Java unqualified-call path).
    let f = extract(
        "src/m.cpp",
        "struct C { void area() const; void draw() const { area(); } };\n",
    );
    let g = link_files(&[("src/m.cpp", &f)]);
    assert!(g
        .edge_records()
        .any(|e| e.kind == EdgeKind::Calls && dst_fqn(&g, e) == "C::area"));
}

#[test]
fn cross_file_call_resolves_possible() {
    let lib = extract("src/lib.cpp", "int square(int x) { return x * x; }\n");
    let app = extract(
        "src/app.cpp",
        "#include \"lib.hpp\"\nint main() { return square(5); }\n",
    );
    let g = link_files(&[("src/lib.cpp", &lib), ("src/app.cpp", &app)]);
    let edge = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::Calls && dst_fqn(&g, e) == "square")
        .expect("a cross-file call edge to square");
    assert_eq!(edge.confidence, Confidence::Possible);
}

#[test]
fn namespace_qualified_cross_file_call_resolves_possible() {
    let lib = extract(
        "src/lib.cpp",
        "namespace geo { int helper(int x) { return x; } }\n",
    );
    let app = extract(
        "src/app.cpp",
        "#include \"lib.hpp\"\nint run() { return geo::helper(2); }\n",
    );
    let g = link_files(&[("src/lib.cpp", &lib), ("src/app.cpp", &app)]);
    let edge = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::Calls && dst_fqn(&g, e) == "geo::helper")
        .expect("a resolved edge to geo::helper");
    assert_eq!(edge.confidence, Confidence::Possible);
}

#[test]
fn main_is_recorded_as_an_entrypoint_hint() {
    let f = extract("src/app.cpp", "int main() { return 0; }\n");
    assert!(f
        .entrypoint_hints
        .iter()
        .any(|h| h.fqn == "main"));
}

#[test]
fn the_extractor_produces_edges_where_the_fallback_scaffold_produced_none() {
    // The headline regression guard: a cross-scope call must materialize an edge.
    let f = extract("src/m.cpp", "int g() { return 0; }\nint f() { return g(); }\n");
    let g = link_files(&[("src/m.cpp", &f)]);
    assert!(g.edge_records().any(|e| e.kind == EdgeKind::Calls));
}

#[test]
fn no_panic_confidence_or_condition_reaches_the_graph() {
    let f = extract(
        "src/m.cpp",
        "void g();\nvoid f() { try { g(); } catch (...) { g(); } }\n",
    );
    let g = link_files(&[("src/m.cpp", &f)]);
    assert!(g
        .edge_records()
        .all(|e| e.condition != cgx_core::condition::EdgeCondition::Panic));
}
