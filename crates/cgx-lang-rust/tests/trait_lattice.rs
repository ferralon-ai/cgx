//! Trait/type-lattice extraction (P3): the `impl_relations` fact stream
//! (Implements / Inherits / Overrides) and Instantiate refs. These assert the
//! frontend facts; resolution to graph edges is the resolver's job and is
//! covered in cgx-resolve's `trait_lattice` suite.

mod common;

use cgx_frontend::RelationKind::{Implements, Inherits};
use common::{extract, has_override_suffix, has_relation, instantiations, relations_of};

/// A small but representative trait lattice: a supertrait, three impls (two
/// overriding methods, one leaving a default un-overridden), and constructor
/// sites of several forms.
const LATTICE: &str = r#"
trait Super {
    fn base(&self) -> u32 { 0 }
}

trait Shape: Super {
    fn area(&self) -> u32;
    fn name(&self) -> &str { "shape" }
}

struct Circle { r: u32 }
struct Square { s: u32 }
struct Triangle { b: u32, h: u32 }

impl Shape for Circle {
    fn area(&self) -> u32 { self.r * self.r * 3 }
    fn name(&self) -> &str { "circle" }
}

impl Shape for Square {
    fn area(&self) -> u32 { self.s * self.s }
    // name() left as the trait default (no override).
}

impl Shape for Triangle {
    fn area(&self) -> u32 { self.b * self.h / 2 }
}

impl Circle {
    fn new(r: u32) -> Self { Circle { r } }
}

fn build() {
    let c = Circle::new(3);
    let s = Square { s: 4 };
    let t = Box::new(Triangle { b: 2, h: 3 });
}
"#;

#[test]
fn impl_trait_for_type_emits_an_implements_relation() {
    let facts = extract("src/lattice.rs", LATTICE);
    assert!(has_relation(&facts, Implements, "Circle", "Shape"));
    assert!(has_relation(&facts, Implements, "Square", "Shape"));
    assert!(has_relation(&facts, Implements, "Triangle", "Shape"));
}

#[test]
fn inherent_impl_emits_no_implements_relation() {
    // `impl Circle { fn new ... }` has no trait field → no Implements.
    let facts = extract("src/lattice.rs", LATTICE);
    let implements = relations_of(&facts, Implements);
    assert!(
        implements.iter().all(|(subj, _)| subj == "Circle"
            || subj == "Square"
            || subj == "Triangle"),
        "only trait impls produce Implements; got {implements:?}"
    );
    // Exactly the three trait impls, no inherent-impl phantom.
    assert_eq!(implements.len(), 3, "got {implements:?}");
}

#[test]
fn supertrait_bound_emits_an_inherits_relation() {
    let facts = extract("src/lattice.rs", LATTICE);
    assert!(has_relation(&facts, Inherits, "Shape", "Super"));
}

#[test]
fn multiple_supertraits_each_emit_an_inherits_relation() {
    let facts = extract(
        "src/sub.rs",
        "trait A {} trait B {} trait Sub: A + B {}",
    );
    let inherits = relations_of(&facts, Inherits);
    assert!(inherits.contains(&("Sub".into(), "A".into())));
    assert!(inherits.contains(&("Sub".into(), "B".into())));
    assert_eq!(inherits.len(), 2, "got {inherits:?}");
}

#[test]
fn overriding_methods_emit_overrides_relations_keyed_to_the_trait_method() {
    let facts = extract("src/lattice.rs", LATTICE);
    // Circle overrides both area and name.
    assert!(has_override_suffix(&facts, "Circle::area", "Shape::area"));
    assert!(has_override_suffix(&facts, "Circle::name", "Shape::name"));
    // Square overrides only area (name uses the default).
    assert!(has_override_suffix(&facts, "Square::area", "Shape::area"));
    assert!(!has_override_suffix(&facts, "Square::name", "Shape::name"));
}

#[test]
fn struct_literal_emits_an_instantiate_ref() {
    let facts = extract("src/lattice.rs", LATTICE);
    let inst = instantiations(&facts);
    assert!(inst.contains(&"Square".to_string()), "got {inst:?}");
}

#[test]
fn associated_new_constructor_emits_an_instantiate_ref() {
    let facts = extract("src/lattice.rs", LATTICE);
    let inst = instantiations(&facts);
    assert!(inst.contains(&"Circle".to_string()), "got {inst:?}");
}

#[test]
fn box_new_records_the_inner_type_not_the_wrapper() {
    let facts = extract("src/lattice.rs", LATTICE);
    let inst = instantiations(&facts);
    // `Box::new(Triangle { .. })` instantiates Triangle, never Box.
    assert!(inst.contains(&"Triangle".to_string()), "got {inst:?}");
    assert!(!inst.contains(&"Box".to_string()), "Box wrapper must not appear: {inst:?}");
}

#[test]
fn default_default_has_no_nameable_instantiation_target() {
    let facts = extract(
        "src/d.rs",
        "fn f() { let x: u32 = Default::default(); }",
    );
    let inst = instantiations(&facts);
    assert!(!inst.contains(&"Default".to_string()), "got {inst:?}");
}

#[test]
fn typed_default_constructor_instantiates_the_named_type() {
    let facts = extract(
        "src/d.rs",
        "struct Cfg; fn f() { let x = Cfg::default(); }",
    );
    let inst = instantiations(&facts);
    assert!(inst.contains(&"Cfg".to_string()), "got {inst:?}");
}

#[test]
fn blanket_impl_is_captured_as_an_implements_relation() {
    // `impl<T> Trait for T` — the implementing "type" is a type-param `T`; the
    // relation is captured (over-approximation is acceptable, design §4.1).
    let facts = extract(
        "src/blanket.rs",
        "trait Trait {} impl<T> Trait for T {}",
    );
    assert!(has_relation(&facts, Implements, "T", "Trait"));
}

#[test]
fn conditional_impl_is_captured_unconditionally() {
    // `impl Trait for T where T: Bound` — the where-bound is not evaluated; the
    // relation is recorded unconditionally (design §4.1).
    let facts = extract(
        "src/cond.rs",
        "trait Trait {} struct W<T>(T); impl<T> Trait for W<T> where T: Clone {}",
    );
    assert!(has_relation(&facts, Implements, "W", "Trait"));
}

#[test]
fn extraction_is_deterministic_across_runs() {
    let first = extract("src/lattice.rs", LATTICE);
    let second = extract("src/lattice.rs", LATTICE);
    assert_eq!(first, second);
    let b1 = cgx_core::codec::encode(&first).unwrap();
    let b2 = cgx_core::codec::encode(&second).unwrap();
    assert_eq!(b1, b2, "canonical fragment is byte-identical");
}
