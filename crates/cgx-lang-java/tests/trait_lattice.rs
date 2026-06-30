//! Inheritance lattice (GM-2.2): `Inherits` / `Implements` / `Overrides`
//! relations emitted from Java's nominal `extends`/`implements` clauses and
//! method-override signal (`@Override` plus in-file name+arity matching).
//!
//! Java's nominal type model lets the adapter emit these relations directly from
//! the supertype clauses (no method-set subset guessing, unlike the Go adapter's
//! structural `Implements`). Assertions are at the adapter (pre-resolution)
//! level: `subject`/`object` are the name paths the resolver later grounds.

mod common;

use cgx_frontend::RelationKind;
use common::{extract, extract_fixture, has_override, has_relation};

// --- extends / implements edges ---

#[test]
fn class_extends_superclass_emits_inherits() {
    let facts = extract_fixture("Lattice.java");
    assert!(
        has_relation(&facts, RelationKind::Inherits, "Circle", "Base"),
        "Circle extends Base; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn class_implements_interface_emits_implements() {
    let facts = extract_fixture("Lattice.java");
    assert!(
        has_relation(&facts, RelationKind::Implements, "Circle", "Drawable"),
        "Circle implements Drawable; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn interface_extends_multiple_interfaces_emits_inherits_each() {
    let facts = extract_fixture("Lattice.java");
    assert!(
        has_relation(&facts, RelationKind::Inherits, "Drawable", "Shape"),
        "Drawable extends Shape; got {:?}",
        facts.impl_relations
    );
    assert!(
        has_relation(&facts, RelationKind::Inherits, "Drawable", "Named"),
        "Drawable extends Named; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn enum_implements_interface_emits_implements() {
    let facts = extract_fixture("Lattice.java");
    assert!(
        has_relation(&facts, RelationKind::Implements, "Mode", "Named"),
        "Mode implements Named; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn record_implements_interface_emits_implements() {
    let facts = extract_fixture("Lattice.java");
    assert!(
        has_relation(&facts, RelationKind::Implements, "Pair", "Named"),
        "Pair implements Named; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn unresolved_external_supertype_still_emits_relation_with_simple_name() {
    let facts = extract_fixture("Lattice.java");
    // `class Box extends RuntimeException` — the supertype is a JDK type the
    // single-file adapter cannot resolve; the relation is still emitted with the
    // best-available simple name and the resolver grounds no edge.
    assert!(
        has_relation(&facts, RelationKind::Inherits, "Box", "RuntimeException"),
        "Box extends RuntimeException; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn inherits_subject_is_the_full_fqn_path() {
    let facts = extract_fixture("Lattice.java");
    let rel = facts
        .impl_relations
        .iter()
        .find(|r| r.kind == RelationKind::Inherits && r.subject.last().map(String::as_str) == Some("Circle"))
        .expect("Circle Inherits relation");
    assert_eq!(
        rel.subject.as_slice(),
        ["com", "example", "lattice", "Circle"],
        "subject should be the type's full FQN segments"
    );
}

// --- overrides ---

#[test]
fn annotated_override_against_directly_declared_interface_method() {
    let facts = extract_fixture("Lattice.java");
    // Circle has @Override draw(); Drawable directly declares draw().
    assert!(
        has_override(&facts, "Drawable", "draw"),
        "Circle::draw overrides Drawable::draw; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn override_subject_is_overriding_methods_full_fqn() {
    let facts = extract_fixture("Lattice.java");
    let rel = facts
        .impl_relations
        .iter()
        .find(|r| {
            r.kind == RelationKind::Overrides
                && r.subject.last().map(String::as_str) == Some("draw")
        })
        .expect("Circle::draw Overrides relation");
    assert_eq!(
        rel.subject.as_slice(),
        ["com", "example", "lattice", "Circle", "draw"],
        "Overrides subject must be the overriding method's exact FQN segments"
    );
}

#[test]
fn unannotated_override_matched_against_in_file_superclass() {
    let facts = extract_fixture("Lattice.java");
    // Circle.describe() has no @Override but name+arity matches Base.describe().
    assert!(
        has_override(&facts, "Base", "describe"),
        "Circle::describe overrides in-file Base::describe; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn enum_annotated_override_emitted() {
    let facts = extract_fixture("Lattice.java");
    assert!(
        has_override(&facts, "Named", "label"),
        "Mode/Pair override Named::label; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn non_overriding_method_emits_no_override() {
    let facts = extract_fixture("Lattice.java");
    // radius() matches no supertype method (by name) and carries no @Override.
    assert!(
        !facts.impl_relations.iter().any(|r| r.kind == RelationKind::Overrides
            && r.subject.last().map(String::as_str) == Some("radius")),
        "radius() is not an override; got {:?}",
        facts.impl_relations
    );
}

#[test]
fn type_without_supertypes_emits_no_relations() {
    let src = "package p;\nclass Solo {\n  @Override\n  public String toString() { return \"x\"; }\n  void plain() {}\n}\n";
    let facts = extract("Solo.java", src);
    // No declared supertype clause -> no Inherits/Implements, and the @Override
    // toString has no in-file supertype to ground against, so no Overrides.
    assert!(
        facts.impl_relations.is_empty(),
        "a type with no extends/implements clause emits no lattice relations; got {:?}",
        facts.impl_relations
    );
}
