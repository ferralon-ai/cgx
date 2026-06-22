//! Resolution of the trait/type-lattice facts (P3) into structural graph edges:
//! Implements / Inherits / Overrides, plus Instantiates from Instantiate refs.
//! Asserts the edges the CHA/RTA passes (P4/P5) will consume.

mod common;

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::edge::EdgeKind;
use cgx_core::id::NodeId;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_frontend::facts::{FileFacts, RelationKind, ScopeId};
use cgx_resolve::{link, FileInput, LinkOpts};

use common::FileBuilder;

const PUB: Visibility = Visibility::Public;
const ROOT: ScopeId = ScopeId::ROOT;

/// A single-file lattice:
///   trait Super { base }            (default method)
///   trait Shape: Super { area; name(default) }
///   Circle, Square, Triangle impl Shape (Circle+Square override area; Circle
///     also overrides name; Square leaves name default; Triangle overrides area)
///   build() instantiates Circle (literal) and Square (Square::new).
fn lattice_file() -> FileFacts {
    let mut b = FileBuilder::new();
    // --- defs (FQNs as the Rust frontend would emit them) ---
    for (fqn, kind) in [
        ("m::Super", SymbolKind::Type),
        ("m::Super::base", SymbolKind::Method),
        ("m::Shape", SymbolKind::Type),
        ("m::Shape::area", SymbolKind::Method),
        ("m::Shape::name", SymbolKind::Method),
        ("m::Circle", SymbolKind::Type),
        ("m::Circle::area", SymbolKind::Method),
        ("m::Circle::name", SymbolKind::Method),
        ("m::Square", SymbolKind::Type),
        ("m::Square::area", SymbolKind::Method),
        ("m::Triangle", SymbolKind::Type),
        ("m::Triangle::area", SymbolKind::Method),
        ("m::build", SymbolKind::Function),
    ] {
        b.def(fqn, kind, ROOT, 1, PUB, false, None);
    }
    let build_scope = b.scope(ROOT, Some("m::build"));

    // --- relations (subjects/objects are short type names; Overrides subject is
    // the impl method's full FQN, object is [Trait, method]) ---
    b.relation(RelationKind::Inherits, &["Shape"], &["Super"], 1);
    b.relation(RelationKind::Implements, &["Circle"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Square"], &["Shape"], 1);
    b.relation(RelationKind::Implements, &["Triangle"], &["Shape"], 1);
    b.relation(RelationKind::Overrides, &["m", "Circle", "area"], &["Shape", "area"], 1);
    b.relation(RelationKind::Overrides, &["m", "Circle", "name"], &["Shape", "name"], 1);
    b.relation(RelationKind::Overrides, &["m", "Square", "area"], &["Shape", "area"], 1);
    b.relation(RelationKind::Overrides, &["m", "Triangle", "area"], &["Shape", "area"], 1);

    // --- instantiations in build() ---
    b.instantiate(&["Circle"], build_scope, 2);
    b.instantiate(&["Square"], build_scope, 3);
    b.build()
}

fn link_lattice(facts: &FileFacts) -> cgx_resolve::ResolvedGraph {
    let inputs = vec![FileInput::new("blob".to_string(), "src/m.rs", "rust", facts)];
    link(&inputs, &LinkOpts::default())
}

fn node_id(g: &cgx_resolve::ResolvedGraph, fqn: &str) -> NodeId {
    g.node_records()
        .find(|n| n.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"))
        .id
}

fn has_edge(g: &cgx_resolve::ResolvedGraph, kind: EdgeKind, src: NodeId, dst: NodeId) -> bool {
    g.edge_records()
        .any(|e| e.kind == kind && e.src == src && e.dst == dst)
}

#[test]
fn implements_edges_link_each_type_to_the_trait() {
    let facts = lattice_file();
    let g = link_lattice(&facts);
    let shape = node_id(&g, "m::Shape");
    for ty in ["m::Circle", "m::Square", "m::Triangle"] {
        assert!(
            has_edge(&g, EdgeKind::Implements, node_id(&g, ty), shape),
            "expected Implements({ty}, Shape)"
        );
    }
}

#[test]
fn inherits_edge_links_subtrait_to_supertrait() {
    let facts = lattice_file();
    let g = link_lattice(&facts);
    assert!(has_edge(
        &g,
        EdgeKind::Inherits,
        node_id(&g, "m::Shape"),
        node_id(&g, "m::Super"),
    ));
}

#[test]
fn overrides_edges_link_impl_method_to_trait_method() {
    let facts = lattice_file();
    let g = link_lattice(&facts);
    let area = node_id(&g, "m::Shape::area");
    let name = node_id(&g, "m::Shape::name");
    assert!(has_edge(&g, EdgeKind::Overrides, node_id(&g, "m::Circle::area"), area));
    assert!(has_edge(&g, EdgeKind::Overrides, node_id(&g, "m::Circle::name"), name));
    assert!(has_edge(&g, EdgeKind::Overrides, node_id(&g, "m::Square::area"), area));
    assert!(has_edge(&g, EdgeKind::Overrides, node_id(&g, "m::Triangle::area"), area));
}

#[test]
fn unoverridden_default_method_has_no_override_edge() {
    // Square does not override name(): no Overrides edge targets Shape::name from
    // a Square method.
    let facts = lattice_file();
    let g = link_lattice(&facts);
    let name = node_id(&g, "m::Shape::name");
    let square_overrides_name = g
        .edge_records()
        .any(|e| e.kind == EdgeKind::Overrides && e.dst == name && e.src != node_id(&g, "m::Circle::name"));
    assert!(!square_overrides_name, "only Circle overrides name()");
}

#[test]
fn instantiates_edges_point_from_the_site_caller_to_the_type() {
    let facts = lattice_file();
    let g = link_lattice(&facts);
    let build = node_id(&g, "m::build");
    assert!(has_edge(&g, EdgeKind::Instantiates, build, node_id(&g, "m::Circle")));
    assert!(has_edge(&g, EdgeKind::Instantiates, build, node_id(&g, "m::Square")));
}

#[test]
fn structural_edges_are_certain_scopegraph_without_a_site_id() {
    let facts = lattice_file();
    let g = link_lattice(&facts);
    for e in g.edge_records().filter(|e| {
        matches!(
            e.kind,
            EdgeKind::Implements | EdgeKind::Inherits | EdgeKind::Overrides
        )
    }) {
        assert_eq!(e.confidence, Confidence::Certain, "structural relation is certain");
        assert_eq!(e.tier, Tier::ScopeGraph);
        assert!(e.site_id.is_none(), "structural edges carry no site_id");
        assert!(e.candidate_group.is_none());
    }
}

#[test]
fn an_unresolvable_relation_endpoint_yields_no_edge() {
    // A relation to a trait that does not exist as a node leaves no edge (honest
    // absence rather than an invented dangling edge).
    let mut b = FileBuilder::new();
    b.def("m::Foo", SymbolKind::Type, ROOT, 1, PUB, false, None);
    b.relation(RelationKind::Implements, &["Foo"], &["Ghost"], 1);
    let facts = b.build();
    let g = link_lattice(&facts);
    assert_eq!(
        g.edge_records().filter(|e| e.kind == EdgeKind::Implements).count(),
        0,
        "no Implements edge when the trait endpoint is unresolved"
    );
}

#[test]
fn reindex_is_byte_identical() {
    let facts = lattice_file();
    let g1 = link_lattice(&facts);
    let g2 = link_lattice(&facts);
    let (n1, e1, c1) = g1.into_linked();
    let (n2, e2, c2) = g2.into_linked();
    let b1 = cgx_core::codec::encode(&(&n1, &e1, &c1)).unwrap();
    let b2 = cgx_core::codec::encode(&(&n2, &e2, &c2)).unwrap();
    assert_eq!(b1, b2, "re-link is byte-identical");
}
