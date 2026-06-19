//! RTA instantiation-pruning (P5): the [`run_rta`] post-pass narrows the CHA
//! trait-scoped candidate sets (`rule = "cha-trait-set"`) to the types actually
//! *instantiated* in the reachable graph, upgrading the surviving group
//! `possible → probable` (design §4.3, §5 row 3).
//!
//! These tests pin the Phase-2 P5 exit criteria:
//! - a never-instantiated impl is pruned and the survivors are `probable`;
//! - the **cut-marker guard** (design §4.3 step 3, load-bearing) keeps the full
//!   CHA set at `possible` when a construction site is behind a cut — this is the
//!   guard's regression and MUST be present;
//! - a prune to a single survivor is still `probable` (RTA is an approximation);
//! - re-indexing is byte-identical (determinism, design §7).

mod common;

use std::collections::BTreeSet;

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::CutMarker;
use cgx_core::edge::EdgeKind;
use cgx_core::id::NodeId;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_frontend::facts::{FileFacts, RelationKind, ScopeId};
use cgx_resolve::{link, run_cha, run_rta, FileInput, LinkOpts, ResolvedGraph};

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

/// The dst node-ids of the surviving virtual-dispatch candidate edges after the
/// CHA→RTA pipeline, regardless of rule (so both `cha-trait-set` and `rta-pruned`
/// edges count).
fn virtual_candidate_dsts(g: &ResolvedGraph) -> BTreeSet<NodeId> {
    g.edge_records()
        .filter(|e| {
            e.kind == EdgeKind::CallsVirtual
                && (e.rule == "cha-trait-set" || e.rule == "rta-pruned")
        })
        .map(|e| e.dst)
        .collect()
}

/// A `Shape` trait with three impls (Circle/Square/Triangle), a `dyn`-style
/// virtual call `s.area()`, and constructors for Circle (`Circle { .. }`) and
/// Square (`Square::new`) — but **never** Triangle.
///
/// `with_triangle_ctor` adds a Triangle construction too (for the single-survivor
/// / no-prune variants), and `triangle_ctor_under_macro` puts the Triangle
/// construction behind an unexpanded-macro cut (the guard fixture).
fn shape_fixture(triangle_ctor: TriangleCtor) -> FileFacts {
    let mut b = FileBuilder::new();
    for (fqn, kind) in [
        ("m::Shape", SymbolKind::Type),
        ("m::Shape::area", SymbolKind::Method),
        ("m::Circle", SymbolKind::Type),
        ("m::Circle::area", SymbolKind::Method),
        ("m::Circle::new", SymbolKind::Method),
        ("m::Square", SymbolKind::Type),
        ("m::Square::area", SymbolKind::Method),
        ("m::Square::new", SymbolKind::Method),
        ("m::Triangle", SymbolKind::Type),
        ("m::Triangle::area", SymbolKind::Method),
        ("m::use_shape", SymbolKind::Function),
        ("m::build", SymbolKind::Function),
    ] {
        b.def(fqn, kind, ROOT, 1, PUB, false, None);
    }
    b.relation(RelationKind::Implements, &["Circle"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Square"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Triangle"], &["Shape"], 1);
    b.relation(RelationKind::Overrides, &["m", "Circle", "area"], &["Shape", "area"], 1);
    b.relation(RelationKind::Overrides, &["m", "Square", "area"], &["Shape", "area"], 1);
    b.relation(RelationKind::Overrides, &["m", "Triangle", "area"], &["Shape", "area"], 1);

    // The virtual call `s.area()` in use_shape().
    let use_scope = b.scope(ROOT, Some("m::use_shape"));
    b.vcall(&["s", "area"], use_scope, 10);

    // Constructions in build(): Circle and Square are instantiated; Triangle is
    // controlled by the variant.
    let build_scope = b.scope(ROOT, Some("m::build"));
    b.instantiate(&["Circle"], build_scope, 20);
    b.instantiate(&["Square"], build_scope, 21);
    match triangle_ctor {
        TriangleCtor::None => {}
        TriangleCtor::Plain => {
            b.instantiate(&["Triangle"], build_scope, 22);
        }
        TriangleCtor::UnderMacroCut => {
            b.instantiate(&["Triangle"], build_scope, 22);
            b.last_ref_cut(CutMarker::UnexpandedMacro);
        }
    }
    b.build()
}

enum TriangleCtor {
    /// Triangle is never instantiated.
    None,
    /// Triangle is instantiated by a plain, fully-resolved construction.
    Plain,
    /// Triangle is instantiated behind an unexpanded-macro cut marker.
    UnderMacroCut,
}

fn run_pipeline(facts: &FileFacts) -> ResolvedGraph {
    let mut g = link_facts(facts);
    run_cha(&mut g);
    run_rta(&mut g);
    g
}

#[test]
fn rta_prunes_the_never_instantiated_impl_and_upgrades_survivors_to_probable() {
    // Circle + Square are instantiated; Triangle is never instantiated. RTA must
    // drop Triangle and upgrade the surviving {Circle, Square} group to probable.
    let facts = shape_fixture(TriangleCtor::None);
    let mut g = link_facts(&facts);
    run_cha(&mut g);
    let stats = run_rta(&mut g);

    assert_eq!(stats.sites_pruned, 1, "the one vcall site is RTA-pruned");
    assert_eq!(stats.candidates_dropped, 1, "Triangle::area is the one dropped candidate");

    let expected: BTreeSet<NodeId> = ["m::Circle::area", "m::Square::area"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(
        virtual_candidate_dsts(&g),
        expected,
        "RTA keeps only the instantiated types' methods (Circle, Square)"
    );

    // Triangle::area must be gone from the candidate set.
    let triangle = node_id(&g, "m::Triangle::area");
    assert!(
        !virtual_candidate_dsts(&g).contains(&triangle),
        "the never-instantiated Triangle::area is pruned"
    );

    // The surviving group is `probable` (upgraded from CHA's `possible`), tier
    // stays ChaRta, rule is `rta-pruned`, kind unchanged.
    let survivors: Vec<_> = g.edge_records().filter(|e| e.rule == "rta-pruned").collect();
    assert_eq!(survivors.len(), 2, "two survivors kept");
    for e in &survivors {
        assert_eq!(e.confidence, Confidence::Probable, "RTA upgrades survivors to probable");
        assert_eq!(e.tier, Tier::ChaRta, "tier stays ChaRta");
        assert_eq!(e.kind, EdgeKind::CallsVirtual, "edge kind stays CallsVirtual");
        assert!(e.candidate_group.is_some(), "two survivors ⇒ still a candidate group");
    }
}

#[test]
fn cut_marker_guard_keeps_the_full_cha_set_when_a_construction_is_under_a_cut() {
    // GUARD REGRESSION (load-bearing, design §4.3 step 3): the same fixture, but
    // the Triangle construction is behind an unexpanded-macro cut. RTA must NOT
    // prune — a type constructed off-graph would be wrongly dropped — so the full
    // 3-candidate CHA set stays `possible`.
    let facts = shape_fixture(TriangleCtor::UnderMacroCut);
    let mut g = link_facts(&facts);
    run_cha(&mut g);
    let stats = run_rta(&mut g);

    assert_eq!(stats.sites_pruned, 0, "the cut guard prevents any prune");
    assert_eq!(stats.candidates_dropped, 0, "no candidate dropped under a cut");
    assert_eq!(stats.sites_guarded_by_cut, 1, "the vcall site is cut-guarded");

    // The full CHA set survives, still tagged cha-trait-set / possible.
    let expected: BTreeSet<NodeId> = ["m::Circle::area", "m::Square::area", "m::Triangle::area"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(
        virtual_candidate_dsts(&g),
        expected,
        "under a construction-site cut the full 3-candidate CHA set is kept"
    );
    for e in g.edge_records().filter(|e| e.kind == EdgeKind::CallsVirtual) {
        assert_eq!(e.rule, "cha-trait-set", "edges remain CHA (not rta-pruned) under the cut");
        assert_eq!(e.confidence, Confidence::Possible, "full set stays possible under the cut");
    }
}

#[test]
fn rta_pruned_to_a_single_survivor_is_still_probable_not_certain() {
    // Only Circle is instantiated (Square and Triangle are never constructed). RTA
    // prunes to the single Circle::area survivor — still `probable`, NOT `certain`
    // (RTA is an approximation). A singleton survivor sheds its candidate group.
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
        ("m::build", SymbolKind::Function),
    ] {
        b.def(fqn, kind, ROOT, 1, PUB, false, None);
    }
    b.relation(RelationKind::Implements, &["Circle"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Square"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Triangle"], &["Shape"], 1);
    b.relation(RelationKind::Overrides, &["m", "Circle", "area"], &["Shape", "area"], 1);
    b.relation(RelationKind::Overrides, &["m", "Square", "area"], &["Shape", "area"], 1);
    b.relation(RelationKind::Overrides, &["m", "Triangle", "area"], &["Shape", "area"], 1);
    let use_scope = b.scope(ROOT, Some("m::use_shape"));
    b.vcall(&["s", "area"], use_scope, 10);
    let build_scope = b.scope(ROOT, Some("m::build"));
    b.instantiate(&["Circle"], build_scope, 20); // only Circle constructed
    let facts = b.build();

    let mut g = link_facts(&facts);
    run_cha(&mut g);
    let stats = run_rta(&mut g);

    assert_eq!(stats.sites_pruned, 1);
    assert_eq!(stats.candidates_dropped, 2, "Square + Triangle dropped");

    let survivors: Vec<_> = g.edge_records().filter(|e| e.rule == "rta-pruned").collect();
    assert_eq!(survivors.len(), 1, "single survivor");
    assert_eq!(survivors[0].dst, node_id(&g, "m::Circle::area"));
    assert_eq!(
        survivors[0].confidence,
        Confidence::Probable,
        "a single RTA survivor is PROBABLE, never certain — RTA is an approximation"
    );
    assert!(
        survivors[0].candidate_group.is_none(),
        "a single survivor sheds its candidate group"
    );
}

#[test]
fn rta_leaves_a_fully_instantiated_set_as_cha_possible() {
    // All three impls are instantiated, so RTA cannot narrow the set: it stays
    // exactly as CHA left it (3 candidates, `possible`, rule `cha-trait-set`) —
    // an un-narrowed set is not "upgraded" because nothing was pruned.
    let facts = shape_fixture(TriangleCtor::Plain);
    let mut g = link_facts(&facts);
    run_cha(&mut g);
    let stats = run_rta(&mut g);

    assert_eq!(stats.sites_pruned, 0, "every candidate type is instantiated ⇒ no prune");

    let expected: BTreeSet<NodeId> = ["m::Circle::area", "m::Square::area", "m::Triangle::area"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(virtual_candidate_dsts(&g), expected, "all three impls survive");
    for e in g.edge_records().filter(|e| e.kind == EdgeKind::CallsVirtual) {
        assert_eq!(e.rule, "cha-trait-set", "an un-narrowed set stays CHA, not rta-pruned");
        assert_eq!(e.confidence, Confidence::Possible, "an un-narrowed set stays possible");
    }
}

#[test]
fn rta_does_not_prune_when_no_type_is_instantiated() {
    // No constructions at all: RTA cannot trust an empty instantiated set (the
    // construction view is too incomplete), so it keeps the full CHA set possible
    // rather than pruning every candidate to zero.
    let facts = {
        let mut b = FileBuilder::new();
        for (fqn, kind) in [
            ("m::Shape", SymbolKind::Type),
            ("m::Shape::area", SymbolKind::Method),
            ("m::Circle", SymbolKind::Type),
            ("m::Circle::area", SymbolKind::Method),
            ("m::Square", SymbolKind::Type),
            ("m::Square::area", SymbolKind::Method),
            ("m::use_shape", SymbolKind::Function),
        ] {
            b.def(fqn, kind, ROOT, 1, PUB, false, None);
        }
        b.relation(RelationKind::Implements, &["Circle"], &["Shape"], 1);
        b.relation(RelationKind::Implements, &["Square"], &["Shape"], 1);
        b.relation(RelationKind::Overrides, &["m", "Circle", "area"], &["Shape", "area"], 1);
        b.relation(RelationKind::Overrides, &["m", "Square", "area"], &["Shape", "area"], 1);
        let use_scope = b.scope(ROOT, Some("m::use_shape"));
        b.vcall(&["s", "area"], use_scope, 10);
        b.build()
    };

    let mut g = link_facts(&facts);
    run_cha(&mut g);
    let stats = run_rta(&mut g);

    assert_eq!(stats.sites_pruned, 0, "no instantiation evidence ⇒ no prune");
    assert!(
        g.edge_records().all(|e| e.rule != "rta-pruned"),
        "no edge is RTA-pruned when nothing is instantiated"
    );
    for e in g.edge_records().filter(|e| e.kind == EdgeKind::CallsVirtual) {
        assert_eq!(e.confidence, Confidence::Possible, "the full CHA set stays possible");
    }
}

#[test]
fn rta_reindex_is_byte_identical() {
    // Determinism (design §7): link + CHA + RTA twice yields a byte-identical
    // store triple, including the prune that changes candidate-group membership.
    let facts = shape_fixture(TriangleCtor::None);
    let g1 = run_pipeline(&facts);
    let g2 = run_pipeline(&facts);
    let (n1, e1, c1) = g1.into_linked();
    let (n2, e2, c2) = g2.into_linked();
    let b1 = cgx_core::codec::encode(&(&n1, &e1, &c1)).unwrap();
    let b2 = cgx_core::codec::encode(&(&n2, &e2, &c2)).unwrap();
    assert_eq!(b1, b2, "link + CHA + RTA is byte-identical across runs");
}
