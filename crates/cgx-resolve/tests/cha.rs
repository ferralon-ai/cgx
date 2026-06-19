//! CHA trait-scoped candidate sets (P4): the [`run_cha`] post-pass replaces the
//! link pass's name-scoped virtual-dispatch sets (`rule = "name-method"`) with
//! trait-scoped sets built from the `Implements`/`Inherits`/`Overrides` lattice.
//!
//! These tests pin the Phase-2 exit criterion **E3**: the `possible` candidate
//! set for a `dyn Trait` call equals the set of `Implements(_, Trait)` types'
//! methods (which equals rust-analyzer's in-source impl set). Plus the supertrait,
//! default-method, supernode-cap, and determinism cases the design (§4.2, §4.5)
//! and the dispatch require.

mod common;

use std::collections::BTreeSet;

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::CutMarker;
use cgx_core::edge::EdgeKind;
use cgx_core::id::NodeId;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_frontend::facts::{FileFacts, RelationKind, ScopeId};
use cgx_resolve::{link, run_cha, FileInput, LinkOpts, ResolvedGraph, CHA_SUPERNODE_CAP};

use common::FileBuilder;

const PUB: Visibility = Visibility::Public;
const ROOT: ScopeId = ScopeId::ROOT;

fn link_facts(facts: &FileFacts) -> ResolvedGraph {
    let inputs = vec![FileInput::new("blob".to_string(), "src/m.rs", "rust", facts)];
    link(&inputs, &LinkOpts::default())
}

fn node_id(g: &ResolvedGraph, fqn: &str) -> NodeId {
    g.node_records()
        .find(|n| n.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"))
        .id
}

/// The set of dst node-ids of the CHA `cha-trait-set` CallsVirtual edges (the
/// trait-scoped candidate set the pass produced).
fn cha_candidate_dsts(g: &ResolvedGraph) -> BTreeSet<NodeId> {
    g.edge_records()
        .filter(|e| e.kind == EdgeKind::CallsVirtual && e.rule == "cha-trait-set")
        .map(|e| e.dst)
        .collect()
}

/// A trait with three impls (Circle/Square/Triangle), all overriding `area`, plus
/// a `use()` fn that makes a `dyn`-style virtual call `s.area()`.
fn three_impls_with_vcall() -> FileFacts {
    let mut b = FileBuilder::new();
    for (fqn, kind) in [
        ("m::Shape", SymbolKind::Type),
        ("m::Shape::area", SymbolKind::Method),
        ("m::Circle", SymbolKind::Type),
        ("m::Circle::area", SymbolKind::Method),
        ("m::Square", SymbolKind::Type),
        ("m::Square::area", SymbolKind::Method),
        ("m::Triangle", SymbolKind::Type),
        ("m::Triangle::area", SymbolKind::Method),
        ("m::use_shape", SymbolKind::Function),
    ] {
        b.def(fqn, kind, ROOT, 1, PUB, false, None);
    }
    let use_scope = b.scope(ROOT, Some("m::use_shape"));
    b.relation(RelationKind::Implements, &["Circle"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Square"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Triangle"], &["Shape"], 1);
    b.relation(RelationKind::Overrides, &["m", "Circle", "area"], &["Shape", "area"], 1);
    b.relation(RelationKind::Overrides, &["m", "Square", "area"], &["Shape", "area"], 1);
    b.relation(RelationKind::Overrides, &["m", "Triangle", "area"], &["Shape", "area"], 1);
    // The virtual call `s.area()` in use_shape().
    b.vcall(&["s", "area"], use_scope, 10);
    b.build()
}

#[test]
fn e3_cha_possible_set_equals_the_implements_trait_impl_set() {
    // E3: for a `dyn Shape` call to `area()`, the CHA `possible` candidate set
    // must equal exactly the set of `Implements(_, Shape)` types' `area` methods.
    let facts = three_impls_with_vcall();
    let mut g = link_facts(&facts);
    let stats = run_cha(&mut g);

    assert_eq!(stats.sites_rescoped, 1, "the one vcall site is trait-scoped");

    let expected: BTreeSet<NodeId> = ["m::Circle::area", "m::Square::area", "m::Triangle::area"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(
        cha_candidate_dsts(&g),
        expected,
        "CHA candidate set must equal the Implements(_, Shape)::area impl set"
    );

    // The set is multi-candidate ⇒ possible, tier ChaRta, kind unchanged.
    for e in g
        .edge_records()
        .filter(|e| e.rule == "cha-trait-set")
    {
        assert_eq!(e.confidence, Confidence::Possible, "3-candidate set is possible");
        assert_eq!(e.tier, Tier::ChaRta);
        assert_eq!(e.kind, EdgeKind::CallsVirtual, "edge kind stays CallsVirtual");
        assert!(e.candidate_group.is_some(), "multi-candidate ⇒ a group");
    }
}

#[test]
fn no_name_method_edge_survives_after_cha_rescopes_the_site() {
    // The name-scoped resolution is replaced, not added alongside.
    let facts = three_impls_with_vcall();
    let mut g = link_facts(&facts);
    run_cha(&mut g);
    assert_eq!(
        g.edge_records()
            .filter(|e| e.rule == "name-method")
            .count(),
        0,
        "no leftover name-method virtual edge"
    );
}

#[test]
fn single_impl_trait_yields_a_probable_candidate() {
    // One impl ⇒ a single-target trait-scoped set ⇒ probable, no candidate group.
    let mut b = FileBuilder::new();
    for (fqn, kind) in [
        ("m::Greeter", SymbolKind::Type),
        ("m::Greeter::hello", SymbolKind::Method),
        ("m::English", SymbolKind::Type),
        ("m::English::hello", SymbolKind::Method),
        ("m::run", SymbolKind::Function),
    ] {
        b.def(fqn, kind, ROOT, 1, PUB, false, None);
    }
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.relation(RelationKind::Implements, &["English"], &["Greeter"], 1);
    b.relation(RelationKind::Overrides, &["m", "English", "hello"], &["Greeter", "hello"], 1);
    b.vcall(&["g", "hello"], run_scope, 10);
    let facts = b.build();

    let mut g = link_facts(&facts);
    let stats = run_cha(&mut g);
    assert_eq!(stats.sites_rescoped, 1);

    let cha: Vec<_> = g
        .edge_records()
        .filter(|e| e.rule == "cha-trait-set")
        .collect();
    assert_eq!(cha.len(), 1, "exactly one candidate for a single-impl trait");
    assert_eq!(cha[0].dst, node_id(&g, "m::English::hello"));
    assert_eq!(cha[0].confidence, Confidence::Probable, "single impl ⇒ probable");
    assert!(cha[0].candidate_group.is_none(), "single candidate ⇒ no group");
}

#[test]
fn supertrait_call_reaches_super_method_impls_via_inherits() {
    // `dyn Sub` call to a `Super` method `base()`: the candidate set is the impls
    // of `Super::base` belonging to types that implement `Sub` (Inherits reach).
    let mut b = FileBuilder::new();
    for (fqn, kind) in [
        ("m::Super", SymbolKind::Type),
        ("m::Super::base", SymbolKind::Method),
        ("m::Sub", SymbolKind::Type),
        ("m::Widget", SymbolKind::Type),
        ("m::Widget::base", SymbolKind::Method),
        ("m::Gadget", SymbolKind::Type),
        ("m::Gadget::base", SymbolKind::Method),
        ("m::run", SymbolKind::Function),
    ] {
        b.def(fqn, kind, ROOT, 1, PUB, false, None);
    }
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.relation(RelationKind::Inherits, &["Sub"], &["Super"], 1);
    // Widget and Gadget implement Sub (the subtrait) and provide Super::base.
    b.relation(RelationKind::Implements, &["Widget"], &["Sub"], 1);
    b.relation(RelationKind::Implements, &["Gadget"], &["Sub"], 1);
    b.relation(RelationKind::Overrides, &["m", "Widget", "base"], &["Super", "base"], 1);
    b.relation(RelationKind::Overrides, &["m", "Gadget", "base"], &["Super", "base"], 1);
    b.vcall(&["x", "base"], run_scope, 10);
    let facts = b.build();

    let mut g = link_facts(&facts);
    run_cha(&mut g);

    let expected: BTreeSet<NodeId> = ["m::Widget::base", "m::Gadget::base"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(
        cha_candidate_dsts(&g),
        expected,
        "dyn Sub::base reaches Super::base impls of Sub-implementing types"
    );
}

#[test]
fn unoverridden_default_method_points_at_the_trait_default_body() {
    // R4: a type that implements the trait but does NOT override the default
    // method `name()` contributes the *trait's* default-body node, not a missing
    // per-type method.
    let mut b = FileBuilder::new();
    for (fqn, kind, is_abstract) in [
        ("m::Shape", SymbolKind::Type, false),
        ("m::Shape::name", SymbolKind::Method, false), // default body present
        ("m::Circle", SymbolKind::Type, false),
        ("m::Circle::name", SymbolKind::Method, false), // overrides
        ("m::Square", SymbolKind::Type, false),         // does NOT override name
        ("m::run", SymbolKind::Function, false),
    ] {
        b.def(fqn, kind, ROOT, 1, PUB, is_abstract, None);
    }
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.relation(RelationKind::Implements, &["Circle"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Square"], &["Shape"], 1);
    b.relation(RelationKind::Overrides, &["m", "Circle", "name"], &["Shape", "name"], 1);
    // Square has no name() override → falls back to Shape::name default body.
    b.vcall(&["s", "name"], run_scope, 10);
    let facts = b.build();

    let mut g = link_facts(&facts);
    run_cha(&mut g);

    let expected: BTreeSet<NodeId> = ["m::Circle::name", "m::Shape::name"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(
        cha_candidate_dsts(&g),
        expected,
        "Square (no override) contributes the Shape::name default body"
    );
}

#[test]
fn blanket_impl_over_the_cap_keeps_the_group_and_cut_marks_it() {
    // A blanket-style trait with > CHA_SUPERNODE_CAP implementing types: the group
    // is KEPT (every candidate present, nothing silently dropped) but every edge
    // carries an Unresolved cut marker so the over-approximation is honest.
    let n = CHA_SUPERNODE_CAP + 5;
    let mut b = FileBuilder::new();
    b.def("m::Wide", SymbolKind::Type, ROOT, 1, PUB, false, None);
    b.def("m::Wide::go", SymbolKind::Method, ROOT, 1, PUB, false, None);
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, None);
    let run_scope = b.scope(ROOT, Some("m::run"));
    for i in 0..n {
        let ty = format!("m::T{i}");
        let go = format!("m::T{i}::go");
        b.def(&ty, SymbolKind::Type, ROOT, 1, PUB, false, None);
        b.def(&go, SymbolKind::Method, ROOT, 1, PUB, false, None);
        b.relation(RelationKind::Implements, &[&format!("T{i}")], &["Wide"], 1);
        b.relation(RelationKind::Overrides, &["m", &format!("T{i}"), "go"], &["Wide", "go"], 1);
    }
    b.vcall(&["w", "go"], run_scope, 10);
    let facts = b.build();

    let mut g = link_facts(&facts);
    let stats = run_cha(&mut g);

    assert_eq!(stats.supernode_sites, 1, "the over-cap site is counted as a supernode");

    let cha: Vec<_> = g
        .edge_records()
        .filter(|e| e.rule == "cha-trait-set")
        .collect();
    assert_eq!(
        cha.len(),
        n,
        "all {n} candidates kept — nothing silently dropped"
    );
    assert!(
        cha.iter().all(|e| e.cut_markers.contains(CutMarker::Unresolved)),
        "every over-cap candidate edge carries an Unresolved cut marker"
    );
}

#[test]
fn pure_inherent_method_call_is_left_name_scoped() {
    // A method that no trait declares (a plain inherent method) is NOT a
    // trait-dispatch site; CHA leaves the link pass's resolution untouched.
    let mut b = FileBuilder::new();
    b.def("m::Helper", SymbolKind::Type, ROOT, 1, PUB, false, None);
    b.def("m::Helper::compute", SymbolKind::Method, ROOT, 1, PUB, false, None);
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, None);
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.vcall(&["h", "compute"], run_scope, 10);
    let facts = b.build();

    let mut g = link_facts(&facts);
    let stats = run_cha(&mut g);
    assert_eq!(stats.sites_rescoped, 0, "no trait declares compute() → untouched");
    assert_eq!(
        g.edge_records().filter(|e| e.rule == "cha-trait-set").count(),
        0
    );
    assert!(
        g.edge_records().any(|e| e.rule == "name-method"),
        "the original name-method resolution is preserved"
    );
}

#[test]
fn cha_reindex_is_byte_identical() {
    // Determinism: running link + CHA twice yields a byte-identical store triple.
    let facts = three_impls_with_vcall();
    let mut g1 = link_facts(&facts);
    let mut g2 = link_facts(&facts);
    run_cha(&mut g1);
    run_cha(&mut g2);
    let (n1, e1, c1) = g1.into_linked();
    let (n2, e2, c2) = g2.into_linked();
    let b1 = cgx_core::codec::encode(&(&n1, &e1, &c1)).unwrap();
    let b2 = cgx_core::codec::encode(&(&n2, &e2, &c2)).unwrap();
    assert_eq!(b1, b2, "link + CHA is byte-identical across runs");
}
