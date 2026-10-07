//! Inheritance lattice + CHA virtual dispatch. The banding is the shared,
//! language-agnostic size-only rule (`cgx-resolve::link.rs::
//! canonicalize_candidate_dsts`): a single-candidate method → `probable`, a
//! multi-candidate one → `possible`. These pins mirror the shipped Java CHA tests,
//! proving C++ inherits the band by construction (no C++-specific band).

mod common;

use cgx_core::confidence::Confidence;
use cgx_core::edge::EdgeKind;
use cgx_frontend::RelationKind;
use common::{
    confidences_from_to, count_nodes_short, extract, has_structural_edge, link_files,
};

#[test]
fn base_class_clause_emits_an_inherits_relation() {
    let f = extract(
        "src/a.cpp",
        "class Base {};\nclass Derived : public Base {};\n",
    );
    assert!(f
        .impl_relations
        .iter()
        .any(|r| r.kind == RelationKind::Inherits
            && r.subject.last() == Some(&"Derived".to_string())
            && r.object.last() == Some(&"Base".to_string())));
}

#[test]
fn multiple_bases_each_emit_an_inherits_relation() {
    let f = extract(
        "src/a.cpp",
        "class A {};\nclass B {};\nclass C : public A, public B {};\n",
    );
    let inherits = f
        .impl_relations
        .iter()
        .filter(|r| r.kind == RelationKind::Inherits)
        .count();
    assert_eq!(inherits, 2);
}

#[test]
fn override_keyword_emits_an_overrides_relation() {
    let f = extract(
        "src/a.cpp",
        "class B { public: virtual void m(); };\n\
         class D : public B { public: void m() override; };\n",
    );
    assert!(f
        .impl_relations
        .iter()
        .any(|r| r.kind == RelationKind::Overrides
            && r.subject.last() == Some(&"m".to_string())));
}

#[test]
fn unannotated_name_arity_match_emits_an_overrides_relation() {
    // No `override` keyword, but the name+arity matches the in-file base method.
    let f = extract(
        "src/a.cpp",
        "class B { public: virtual void m(); };\n\
         class D : public B { public: void m(); };\n",
    );
    assert!(f
        .impl_relations
        .iter()
        .any(|r| r.kind == RelationKind::Overrides));
}

#[test]
fn inherits_relation_resolves_to_a_structural_edge() {
    let f = extract(
        "src/a.cpp",
        "class Base {};\nclass Derived : public Base {};\n",
    );
    let g = link_files(&[("src/a.cpp", &f)]);
    assert!(has_structural_edge(
        &g,
        EdgeKind::Inherits,
        "Derived",
        "Base"
    ));
}

#[test]
fn single_candidate_virtual_call_is_probable() {
    // `draw` is defined in exactly one class → a single-candidate CHA set →
    // probable (the shared size-only rule, singleton ⇒ probable).
    let lib = extract(
        "src/lib.cpp",
        "struct Canvas { void draw() const; };\n",
    );
    let app = extract(
        "src/app.cpp",
        "void run(Canvas* c) { c->draw(); }\n",
    );
    let g = link_files(&[("src/lib.cpp", &lib), ("src/app.cpp", &app)]);
    assert_eq!(count_nodes_short(&g, "draw"), 1);
    let confs = confidences_from_to(&g, "run", "draw");
    assert_eq!(confs, vec![Confidence::Probable]);
}

#[test]
fn multi_candidate_virtual_call_is_possible() {
    // `area` is declared in a base and overridden in a derived → two candidates →
    // possible (the shared size-only rule, multi ⇒ possible). Openness does not
    // demote; set SIZE does.
    let lib = extract(
        "src/lib.cpp",
        "struct Shape { virtual double area() const; };\n\
         struct Circle : Shape { double area() const override; };\n",
    );
    let app = extract(
        "src/app.cpp",
        "double total(Shape* s) { return s->area(); }\n",
    );
    let g = link_files(&[("src/lib.cpp", &lib), ("src/app.cpp", &app)]);
    assert_eq!(count_nodes_short(&g, "area"), 2);
    let confs = confidences_from_to(&g, "total", "area");
    assert!(!confs.is_empty(), "the virtual call must materialize edges");
    assert!(
        confs.iter().all(|c| *c == Confidence::Possible),
        "a multi-candidate virtual call bands to possible, got {confs:?}"
    );
}

#[test]
fn virtual_call_edge_kind_is_calls_virtual_when_unnarrowed() {
    let lib = extract(
        "src/lib.cpp",
        "struct Shape { virtual double area() const; };\n\
         struct Circle : Shape { double area() const override; };\n",
    );
    let app = extract("src/app.cpp", "double t(Shape* s) { return s->area(); }\n");
    let g = link_files(&[("src/lib.cpp", &lib), ("src/app.cpp", &app)]);
    assert!(g
        .edge_records()
        .any(|e| e.kind == EdgeKind::CallsVirtual));
}

#[test]
fn diamond_bases_all_emit_inherits() {
    let f = extract(
        "src/a.cpp",
        "struct A {};\nstruct B : A {};\nstruct C : A {};\nstruct D : B, C {};\n",
    );
    let g = link_files(&[("src/a.cpp", &f)]);
    assert!(has_structural_edge(&g, EdgeKind::Inherits, "D", "B"));
    assert!(has_structural_edge(&g, EdgeKind::Inherits, "D", "C"));
    assert!(has_structural_edge(&g, EdgeKind::Inherits, "B", "A"));
}

#[test]
fn qualified_base_class_name_resolves_to_its_simple_name() {
    let f = extract(
        "src/a.cpp",
        "namespace geo { struct Shape {}; }\nstruct Circle : geo::Shape {};\n",
    );
    assert!(f.impl_relations.iter().any(|r| r.kind
        == RelationKind::Inherits
        && r.object.last() == Some(&"Shape".to_string())));
}
