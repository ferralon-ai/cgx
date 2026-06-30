//! Definition extraction: every Cycle-1 def kind, its `SymbolKind`,
//! `Visibility`, abstract flag, FQN nesting (package prefix from the source
//! `package_declaration`, nested types under their outer type), and scope
//! opening — asserted against focused inline sources plus the shared fixture.

mod common;

use cgx_core::node::{SymbolKind, Visibility};
use common::{def, extract, extract_fixture};

const PKG: &str = "package com.example.shapes;\n";

#[test]
fn class_is_type_kind_not_abstract() {
    let facts = extract("Foo.java", &format!("{PKG}public class Foo {{}}"));
    let foo = def(&facts, "com::example::shapes::Foo");
    assert_eq!(foo.kind, SymbolKind::Type);
    assert!(!foo.is_abstract);
    assert_eq!(foo.visibility, Visibility::Public);
}

#[test]
fn abstract_class_carries_abstract_flag() {
    let facts = extract("Base.java", &format!("{PKG}abstract class Base {{}}"));
    let base = def(&facts, "com::example::shapes::Base");
    assert_eq!(base.kind, SymbolKind::Type);
    assert!(base.is_abstract);
    // No access modifier → package-private.
    assert_eq!(base.visibility, Visibility::Package);
}

#[test]
fn interface_is_type_kind_and_abstract() {
    let facts = extract("Drawable.java", &format!("{PKG}interface Drawable {{ void draw(); }}"));
    let iface = def(&facts, "com::example::shapes::Drawable");
    assert_eq!(iface.kind, SymbolKind::Type);
    assert!(iface.is_abstract, "interfaces are always abstract");
    // An interface method with no body is abstract.
    let draw = def(&facts, "com::example::shapes::Drawable::draw");
    assert_eq!(draw.kind, SymbolKind::Method);
    assert!(draw.is_abstract);
}

#[test]
fn enum_is_type_kind() {
    let facts = extract("Color.java", &format!("{PKG}enum Color {{ RED, GREEN }}"));
    let color = def(&facts, "com::example::shapes::Color");
    assert_eq!(color.kind, SymbolKind::Type);
    assert!(!color.is_abstract);
}

#[test]
fn record_is_type_kind() {
    let facts = extract("Point.java", &format!("{PKG}record Point(int x, int y) {{}}"));
    let point = def(&facts, "com::example::shapes::Point");
    assert_eq!(point.kind, SymbolKind::Type);
    assert!(!point.is_abstract);
}

#[test]
fn method_visibility_from_access_modifier() {
    let src = format!(
        "{PKG}class Svc {{ public void open() {{}} private void shut() {{}} protected void tick() {{}} void pkg() {{}} }}"
    );
    let facts = extract("Svc.java", &src);
    assert_eq!(
        def(&facts, "com::example::shapes::Svc::open").visibility,
        Visibility::Public
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Svc::shut").visibility,
        Visibility::Private
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Svc::tick").visibility,
        Visibility::Protected
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Svc::pkg").visibility,
        Visibility::Package
    );
}

#[test]
fn constructor_is_function_kind() {
    let facts = extract(
        "Box.java",
        &format!("{PKG}class Box {{ public Box(int n) {{}} }}"),
    );
    let ctor = def(&facts, "com::example::shapes::Box::Box");
    assert_eq!(ctor.kind, SymbolKind::Function);
    assert_eq!(ctor.visibility, Visibility::Public);
}

#[test]
fn each_variable_declarator_is_its_own_field_def() {
    // A single `field_declaration` declares two variables; both become Field defs
    // sharing the declaration's visibility.
    let facts = extract(
        "Pair.java",
        &format!("{PKG}class Pair {{ public String left, right; }}"),
    );
    let left = def(&facts, "com::example::shapes::Pair::left");
    let right = def(&facts, "com::example::shapes::Pair::right");
    assert_eq!(left.kind, SymbolKind::Field);
    assert_eq!(right.kind, SymbolKind::Field);
    assert_eq!(left.visibility, Visibility::Public);
    assert_eq!(right.visibility, Visibility::Public);
}

#[test]
fn nested_type_fqn_nests_under_outer_type() {
    let src = format!("{PKG}class Outer {{ static class Inner {{ private int seed; }} }}");
    let facts = extract("Outer.java", &src);
    let inner = def(&facts, "com::example::shapes::Outer::Inner");
    assert_eq!(inner.kind, SymbolKind::Type);
    // The nested type's member nests one level deeper still.
    let seed = def(&facts, "com::example::shapes::Outer::Inner::seed");
    assert_eq!(seed.kind, SymbolKind::Field);
    assert_eq!(seed.visibility, Visibility::Private);
}

#[test]
fn class_opens_a_child_scope() {
    let facts = extract("Foo.java", &format!("{PKG}class Foo {{ void m() {{}} }}"));
    // root + class-body scope + method scope >= 3.
    assert!(facts.scopes.len() >= 3);
    // A def is recorded in its *containing* scope, not the scope it opens.
    let foo = def(&facts, "com::example::shapes::Foo");
    assert_eq!(foo.scope.0, 0, "top-level type lives in the root scope");
}

#[test]
fn fixture_covers_every_def_kind() {
    let facts = extract_fixture("Shapes.java");
    // Package prefix comes from the `package com.example.shapes;` declaration.
    assert_eq!(
        def(&facts, "com::example::shapes::Shapes").kind,
        SymbolKind::Type
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Shapes::Shapes").kind,
        SymbolKind::Function
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Shapes::total").kind,
        SymbolKind::Method
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Shapes::count").visibility,
        Visibility::Private
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Shapes::Helper").kind,
        SymbolKind::Type
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Shapes::Helper::seed").visibility,
        Visibility::Protected
    );
    assert!(def(&facts, "com::example::shapes::Drawable").is_abstract);
    assert_eq!(
        def(&facts, "com::example::shapes::Color::isPrimary").kind,
        SymbolKind::Method
    );
    assert_eq!(
        def(&facts, "com::example::shapes::Point::sum").kind,
        SymbolKind::Method
    );
    assert!(def(&facts, "com::example::shapes::Base").is_abstract);
    assert!(def(&facts, "com::example::shapes::Base::run").is_abstract);
}
