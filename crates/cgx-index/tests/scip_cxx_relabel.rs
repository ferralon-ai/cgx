//! scip-clang (C/C++) relabel proofs — the parity-defining phase (dispatch F).
//!
//! These exercise the C++ symbol mapper + the fail-closed relabel directly on a
//! hand-built `ResolvedGraph` and a synthesized scip-clang SCIP index authored
//! via `cgx-scip`'s test-only encoder. Hermetic + zero-egress: scip-clang itself
//! is never invoked (the wire format is known and `cgx-scip` decodes it).
//!
//! The scenario mirrors the Phase-E C++ adapter output: two overloads of
//! `app::f` share the signature-free qname `app::f` and form a `possible`
//! multi-candidate call. scip-clang resolves the call to one specific overload
//! (by its method disambiguator). We prove:
//!   1. COMPLETE compdb → the overload promotes to `certain`, the edge is
//!      redirected to the exact resolved overload (span-precise, not the
//!      arbitrary last node sharing the qname), and the candidate group collapses.
//!   2. PARTIAL compdb → the same resolution is capped at `probable` (FAIL-CLOSED),
//!      the candidate group is left intact, and the capped-promotion counter ticks.
//!   3. No SCIP occurrence at the call → the edge is left untouched (scip-clang is
//!      an upgrade, not a hard dependency; no fabricated resolution).
//!   4. A virtual-dispatch call resolved by scip-clang caps at `probable`
//!      (static target ≠ devirtualization).

use cgx_core::{
    Candidate, Confidence, CutMarkers, EdgeCondition, EdgeKind, EdgeRecord, EdgeWithProvenance,
    NodeId, NodeRecord, NodeWithProvenance, Provenance, Span, SymbolKind, Tier, Visibility,
};
use cgx_index::{scip_relabel, CompdbCompleteness, ScipRelabelOpts};
use cgx_resolve::ResolvedGraph;
use cgx_scip::testsupport::{document, index, occurrence, relationship, symbol_info, symbol_info_rel, ROLE_DEFINITION};
use cgx_scip::ScipResolver;

fn node(id: u32, fqn: &str, file: &str, line: u32, kind: SymbolKind) -> NodeWithProvenance {
    NodeWithProvenance {
        provenance: Provenance::new(Span::new(file.to_string(), line, None), "test", Tier::NameSyntactic, String::new()),
        node: NodeRecord {
            id: NodeId(id),
            kind,
            fqn: fqn.to_string(),
            file: file.to_string(),
            line_start: line,
            line_end: line,
            lang: "cpp".to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
            unresolved_calls: 0,
        },
    }
}

/// A call edge at a given call-site span, over-approximated into `candidate_group`.
fn call_edge(
    src: u32,
    dst: u32,
    kind: EdgeKind,
    file: &str,
    line: u32,
    col: u32,
    group: u32,
) -> EdgeWithProvenance {
    let span = Span::new(file.to_string(), line, Some(col));
    EdgeWithProvenance {
        edge: EdgeRecord {
            id: cgx_core::EdgeId(0),
            src: NodeId(src),
            dst: NodeId(dst),
            kind,
            condition: EdgeCondition::Always,
            confidence: Confidence::Possible,
            tier: Tier::NameSyntactic,
            rule: "name-syntactic".to_string(),
            site_id: None,
            stmt_index: Some(0),
            cut_markers: CutMarkers::default(),
            implicit: None,
            candidate_group: Some(group),
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
            transform: None,
        },
        provenance: Provenance::new(span, "name-syntactic", Tier::NameSyntactic, String::new()),
    }
}

/// Two overloads of `app::f` defined in `src/f.cpp`, called once from
/// `src/main.cpp`. The syntactic default binds the lexically-last overload
/// (node 2, `app::f(double)`); scip-clang resolves the call to the OTHER overload
/// (node 1, `app::f(int)`), so a correct redirect must be span-precise.
fn overload_graph() -> ResolvedGraph {
    let mut g = ResolvedGraph {
        nodes: vec![
            node(0, "app::run", "src/main.cpp", 10, SymbolKind::Function),
            node(1, "app::f", "src/f.cpp", 3, SymbolKind::Function), // f(int)
            node(2, "app::f", "src/f.cpp", 7, SymbolKind::Function), // f(double)
        ],
        // Lexical default dst = node 2; both overloads are candidates (group 7).
        edges: vec![call_edge(0, 2, EdgeKind::Calls, "src/main.cpp", 10, 5, 7)],
        candidates: vec![
            Candidate { candidate_group: 7, dst: NodeId(2), rank: 0 },
            Candidate { candidate_group: 7, dst: NodeId(1), rank: 1 },
        ],
        ..Default::default()
    };
    cgx_resolve::canonicalize(&mut g);
    g
}

const F_INT: &str = "cxx . . . app/f(ai).";
const F_DOUBLE: &str = "cxx . . . app/f(ad).";

/// A scip-clang index where the call at `src/main.cpp:10:5` resolves to the
/// `app::f(int)` overload (node 1, def at `src/f.cpp:3`). `with_ref` false drops
/// the call-site reference occurrence (the degrade-without-resolution case).
fn overload_scip(with_ref: bool) -> Vec<u8> {
    // SCIP ranges are 0-based: def ids at lines 3 and 7 → 2 and 6; the call at
    // line 10 col 5 → (9, 4).
    let mut main_occs = Vec::new();
    if with_ref {
        main_occs.push(occurrence(&[9, 4, 5], F_INT, 0));
    }
    index(
        "/repo",
        "scip-clang",
        vec![
            document(
                "src/f.cpp",
                vec![
                    occurrence(&[2, 0, 1], F_INT, ROLE_DEFINITION),
                    occurrence(&[6, 0, 1], F_DOUBLE, ROLE_DEFINITION),
                ],
                vec![symbol_info(F_INT, 0, ""), symbol_info(F_DOUBLE, 0, "")],
            ),
            document("src/main.cpp", main_occs, vec![]),
        ],
        vec![],
    )
}

fn call_edge_of(g: &ResolvedGraph) -> &EdgeRecord {
    g.edges
        .iter()
        .map(|e| &e.edge)
        .find(|e| e.kind.is_call())
        .expect("one call edge")
}

#[test]
fn overload_promotes_to_certain_under_complete_compdb() {
    let mut g = overload_graph();
    assert_eq!(call_edge_of(&g).confidence, Confidence::Possible, "baseline: possible overload set");

    let scip = ScipResolver::from_bytes(&overload_scip(true)).unwrap();
    assert_eq!(scip.scheme(), cgx_scip::SymbolScheme::ScipClang, "tool_info selects the C++ mapper");

    let opts = ScipRelabelOpts { local_packages: Vec::new(), compdb: CompdbCompleteness::Complete };
    let stats = scip_relabel(&mut g, &scip, &opts);

    let e = call_edge_of(&g);
    assert_eq!(e.confidence, Confidence::Certain, "COMPLETE compdb: resolved overload → certain");
    assert_eq!(e.tier, Tier::Scip);
    assert_eq!(e.rule, "scip-occurrence");
    // Span-precise redirect: the call now points at app::f(int) (node 1), NOT the
    // lexically-last node 2 that the signature-free qname map would have picked.
    assert_eq!(e.dst, NodeId(1), "redirected to the exact resolved overload by def-site span");
    assert_eq!(e.candidate_group, None, "candidate group collapsed on the certain resolution");
    assert_eq!(stats.upgraded_certain, 1);
    assert_eq!(stats.capped_partial_compdb, 0);
}

#[test]
fn overload_caps_at_probable_under_partial_compdb() {
    let mut g = overload_graph();
    let scip = ScipResolver::from_bytes(&overload_scip(true)).unwrap();

    let opts = ScipRelabelOpts { local_packages: Vec::new(), compdb: CompdbCompleteness::Partial };
    let stats = scip_relabel(&mut g, &scip, &opts);

    let e = call_edge_of(&g);
    assert_eq!(e.confidence, Confidence::Probable, "PARTIAL compdb: fail-closed cap, never certain");
    assert_eq!(e.tier, Tier::Scip);
    assert_eq!(e.dst, NodeId(2), "no redirect under fail-closed: possibly-false singleton");
    assert_eq!(e.candidate_group, Some(7), "candidate group left intact under fail-closed");
    assert_eq!(stats.upgraded_certain, 0, "fail-closed emits zero certain");
    assert_eq!(stats.upgraded_probable, 1);
    assert_eq!(stats.capped_partial_compdb, 1, "the capped-certain counter ticks for cgx doctor");
}

#[test]
fn no_scip_occurrence_leaves_edge_untouched() {
    // scip-clang saw no reference at the call site → the possible overload set is
    // left exactly as the syntactic adapter produced it. No fabrication.
    let mut g = overload_graph();
    let scip = ScipResolver::from_bytes(&overload_scip(false)).unwrap();

    let opts = ScipRelabelOpts { local_packages: Vec::new(), compdb: CompdbCompleteness::Complete };
    let stats = scip_relabel(&mut g, &scip, &opts);

    let e = call_edge_of(&g);
    assert_eq!(e.confidence, Confidence::Possible, "degrade-without-resolution stays honest");
    assert_eq!(e.tier, Tier::NameSyntactic);
    assert_eq!(e.dst, NodeId(2));
    assert_eq!(e.candidate_group, Some(7));
    assert_eq!(stats.upgraded_certain, 0);
    assert_eq!(stats.upgraded_probable, 0);
}

#[test]
fn virtual_dispatch_caps_at_probable_even_under_complete_compdb() {
    // A virtual call that scip-clang resolves to the statically-declared override
    // is NOT a devirtualization → capped at probable, candidate set preserved.
    let circle = "cxx . . . shape/Circle#area().";
    let shape = "cxx . . . shape/Shape#area().";

    let mut g = ResolvedGraph {
        nodes: vec![
            node(0, "app::run", "src/main.cpp", 10, SymbolKind::Function),
            node(1, "shape::Circle::area", "src/shape.cpp", 5, SymbolKind::Method),
        ],
        edges: vec![call_edge(0, 1, EdgeKind::CallsVirtual, "src/main.cpp", 10, 5, 3)],
        candidates: vec![Candidate { candidate_group: 3, dst: NodeId(1), rank: 0 }],
        ..Default::default()
    };
    cgx_resolve::canonicalize(&mut g);

    let bytes = index(
        "/repo",
        "scip-clang",
        vec![
            document(
                "src/shape.cpp",
                vec![occurrence(&[4, 0, 1], circle, ROLE_DEFINITION)],
                // Circle::area overrides Shape::area (is_implementation) → virtual.
                vec![symbol_info_rel(circle, 0, "", vec![relationship(shape, true)])],
            ),
            document("src/main.cpp", vec![occurrence(&[9, 4, 5], circle, 0)], vec![]),
        ],
        vec![],
    );
    let scip = ScipResolver::from_bytes(&bytes).unwrap();

    let opts = ScipRelabelOpts { local_packages: Vec::new(), compdb: CompdbCompleteness::Complete };
    let stats = scip_relabel(&mut g, &scip, &opts);

    let e = call_edge_of(&g);
    assert_eq!(e.confidence, Confidence::Probable, "virtual dispatch never promotes to certain");
    assert_eq!(e.tier, Tier::Scip);
    assert_eq!(e.candidate_group, Some(3), "virtual candidate set preserved (no collapse)");
    assert_eq!(stats.upgraded_certain, 0);
    assert_eq!(stats.upgraded_probable, 1);
}
