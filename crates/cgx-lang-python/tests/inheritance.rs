//! Cycle-3 inheritance / overrides: `Inherits` per base class and `Overrides`
//! per in-file name-matched (or PEP-698 `@override`-marked) method.

mod common;

use cgx_frontend::RelationKind;
use common::{extract, extract_fixture, has_override, has_relation, override_count};

#[test]
fn single_base_emits_one_inherits() {
    let facts = extract_fixture("lattice.py");
    assert!(
        has_relation(&facts, RelationKind::Inherits, "Widget", "Base"),
        "Widget(Base) must emit Inherits Widget→Base"
    );
}

#[test]
fn multiple_inheritance_emits_one_inherits_per_base() {
    let facts = extract_fixture("lattice.py");
    assert!(has_relation(&facts, RelationKind::Inherits, "Panel", "Base"));
    assert!(has_relation(&facts, RelationKind::Inherits, "Panel", "Mixin"));
}

#[test]
fn dotted_external_base_emits_best_name() {
    let facts = extract_fixture("lattice.py");
    // `collections.abc.Sequence` → best-name `Sequence`; resolver drops it later.
    assert!(has_relation(
        &facts,
        RelationKind::Inherits,
        "Seq",
        "Sequence"
    ));
    // `unittest.TestCase` → best-name `TestCase`.
    assert!(has_relation(
        &facts,
        RelationKind::Inherits,
        "CaseTest",
        "TestCase"
    ));
}

#[test]
fn keyword_arg_in_superclasses_is_filtered() {
    let facts = extract_fixture("lattice.py");
    // `class WithMeta(Base, metaclass=type)` → only Base is a base.
    assert!(has_relation(
        &facts,
        RelationKind::Inherits,
        "WithMeta",
        "Base"
    ));
    assert!(
        !has_relation(&facts, RelationKind::Inherits, "WithMeta", "type"),
        "metaclass keyword arg must not be an Inherits base"
    );
}

#[test]
fn class_without_supertype_emits_no_inherits() {
    let facts = extract_fixture("lattice.py");
    assert!(
        !facts
            .impl_relations
            .iter()
            .any(|r| r.kind == RelationKind::Inherits && r.subject.join("::") == "Lonely"),
        "Lonely has no bases → no Inherits"
    );
}

#[test]
fn override_by_name_match_against_in_file_base() {
    let facts = extract_fixture("lattice.py");
    // Widget.render overrides Base.render (same name on a declared in-file base).
    assert!(has_override(
        &facts,
        "lattice::Widget::render",
        "Base::render"
    ));
}

#[test]
fn override_against_each_matching_base_in_multiple_inheritance() {
    let facts = extract_fixture("lattice.py");
    // Panel.serialize matches Mixin.serialize; Panel.describe matches Base.describe.
    assert!(has_override(
        &facts,
        "lattice::Panel::serialize",
        "Mixin::serialize"
    ));
    assert!(has_override(
        &facts,
        "lattice::Panel::describe",
        "Base::describe"
    ));
    // serialize does NOT match Base (Base has no serialize), so no Base::serialize.
    assert!(!has_override(
        &facts,
        "lattice::Panel::serialize",
        "Base::serialize"
    ));
}

#[test]
fn non_overriding_method_emits_nothing() {
    let facts = extract_fixture("lattice.py");
    // Widget.extra has no same-named method on Base → no Overrides.
    assert_eq!(override_count(&facts, "lattice::Widget::extra"), 0);
    // Lonely.solo has no supertype at all.
    assert_eq!(override_count(&facts, "lattice::Lonely::solo"), 0);
}

#[test]
fn method_matching_external_base_only_emits_no_override() {
    let facts = extract_fixture("lattice.py");
    // CaseTest.test_it / Seq.__len__ match methods on EXTERNAL bases not visible
    // in-file → no in-file override signal (left to the CHA/resolve layer).
    assert_eq!(override_count(&facts, "lattice::CaseTest::test_it"), 0);
    assert_eq!(override_count(&facts, "lattice::Seq::__len__"), 0);
}

#[test]
fn override_subject_is_full_method_fqn() {
    let facts = extract_fixture("lattice.py");
    let rel = facts
        .impl_relations
        .iter()
        .find(|r| r.kind == RelationKind::Overrides && r.object.join("::") == "Base::render")
        .expect("Widget render override must exist");
    // The resolver rejoins subject for an exact `def_to_node` lookup, so it must
    // be the full FQN segments, not a bare method name.
    assert_eq!(rel.subject.join("::"), "lattice::Widget::render");
}

#[test]
fn explicit_override_decorator_emits_against_declared_base() {
    // PEP 698 `@override` is authoritative even when the base method set is not
    // matched by name in-file — emit against every declared in-file base.
    let src = "\
from typing import override


class P:
    pass


class C(P):
    @override
    def run(self):
        return 1
";
    let facts = extract("app/dec.py", src);
    assert!(
        has_override(&facts, "app::dec::C::run", "P::run"),
        "@override on C.run must emit Overrides against declared base P even though P has no run"
    );
}

#[test]
fn nested_function_is_not_a_class_method() {
    // A function nested inside a method must not register as an overridable member
    // of the enclosing class.
    let src = "\
class B:
    def helper(self):
        return 0


class D(B):
    def m(self):
        def helper():
            return 1

        return helper()
";
    let facts = extract("app/nest.py", src);
    // D.m is not named `helper`, and the nested `helper` is not a class member, so
    // there is no D.helper overriding B.helper.
    assert_eq!(override_count(&facts, "app::nest::D::m"), 0);
    assert!(!has_override(&facts, "app::nest::D::helper", "B::helper"));
}
