//! White-box tests for the SCIP upgrade-only re-label pass (design §3.5/§3.6).
//!
//! These build a [`ResolvedGraph`] by hand (so the call-site spans are known
//! exactly) and a synthetic-but-valid `.scip` byte stream via `cgx-scip`'s
//! test-only encoder — no `rust-analyzer` (it is not installed in CI). They pin
//! the roadmap exit criteria E1/E2 plus the load-bearing upgrade-only invariant
//! and the GM-14 dependency-edge encoding.

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::CutMarkers;
use cgx_core::edge::{EdgeKind, EdgeRecord, EdgeWithProvenance};
use cgx_core::id::{EdgeId, NodeId};
use cgx_core::EdgeCondition;
use cgx_core::node::{NodeRecord, NodeWithProvenance, SymbolKind, Visibility};
use cgx_core::provenance::{Provenance, Span};
use cgx_index::{scip_relabel, ScipRelabelOpts};
use cgx_resolve::{canonicalize, ResolvedGraph};
use cgx_scip::testsupport::{document, index, occurrence, symbol_info, KIND_TRAIT, ROLE_DEFINITION};
use cgx_scip::ScipResolver;

const PKG: &str = "fixture_crate";
const VER: &str = "0.1.0";

fn node(id: u32, fqn: &str, file: &str, line: u32) -> NodeWithProvenance {
    NodeWithProvenance {
        node: NodeRecord {
            id: NodeId(id),
            kind: SymbolKind::Function,
            fqn: fqn.to_string(),
            file: file.to_string(),
            line_start: line,
            line_end: line,
            lang: "rust".to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
        },
        provenance: Provenance::new(Span::new(file, line, None), "def", Tier::ScopeGraph, String::new()),
    }
}

/// A call edge whose provenance span is the call site (1-based line/col, as the
/// frontend emits). The relabel pass joins on `(file, line-1, col-1)`.
#[allow(clippy::too_many_arguments)]
fn call_edge(
    src: u32,
    dst: u32,
    file: &str,
    line: u32,
    col: u32,
    confidence: Confidence,
    tier: Tier,
    rule: &str,
) -> EdgeWithProvenance {
    let span = Span::new(file, line, Some(col));
    EdgeWithProvenance {
        edge: EdgeRecord {
            id: EdgeId(0),
            src: NodeId(src),
            dst: NodeId(dst),
            kind: EdgeKind::Calls,
            condition: EdgeCondition::Always,
            confidence,
            tier,
            rule: rule.to_string(),
            site_id: None,
            stmt_index: Some(0),
            cut_markers: CutMarkers::new(),
            implicit: None,
            candidate_group: None,
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
            transform: None,
        },
        provenance: Provenance::new(span, rule, tier, String::new()),
    }
}

fn resolver(bytes: &[u8]) -> ScipResolver {
    ScipResolver::from_bytes(bytes).expect("synthetic scip parses")
}

/// A SCIP symbol string for a free function `fixture_crate::<name>`.
fn free_fn_symbol(name: &str) -> String {
    format!("rust-analyzer cargo {PKG} {VER} {name}().")
}

/// A SCIP symbol string for a trait method `fixture_crate::<trait>::<method>`.
fn trait_method_symbol(tr: &str, method: &str) -> String {
    format!("rust-analyzer cargo {PKG} {VER} {tr}#{method}().")
}

#[test]
fn e1_free_fn_direct_call_upgrades_to_certain() {
    // Graph: caller(0) -> add(1). Baseline edge is `probable@scope-graph` (a
    // single import binding). The call site is at line 5, col 9 (1-based).
    let mut graph = ResolvedGraph {
        nodes: vec![node(0, "fixture_crate::caller", "src/lib.rs", 4), node(1, "fixture_crate::add", "src/lib.rs", 1)],
        edges: vec![call_edge(0, 1, "src/lib.rs", 5, 9, Confidence::Probable, Tier::ScopeGraph, "import-ref")],
        candidates: vec![],
        unresolved: vec![],
        ..Default::default()
    };
    canonicalize(&mut graph);

    // SCIP: a definition of `add` (def_count == 1) + a reference occurrence at the
    // call site (0-based: line 4, char 8). Unique def + free fn → certain.
    let add = free_fn_symbol("add");
    let bytes = index(
        "/repo",
        "rust-analyzer",
        vec![document(
            "src/lib.rs",
            vec![
                occurrence(&[0, 0, 3], &add, ROLE_DEFINITION), // def of add at line 0
                occurrence(&[4, 8, 11], &add, 0),              // ref at the call site
            ],
            vec![symbol_info(&add, 0, "")],
        )],
        vec![],
    );

    let stats = scip_relabel(&mut graph, &resolver(&bytes), &ScipRelabelOpts::default());

    let edge = &graph.edges[0].edge;
    assert_eq!(
        edge.confidence,
        Confidence::Certain,
        "E1: a unique-def free-fn direct call must upgrade to certain"
    );
    assert_eq!(edge.tier, Tier::Scip);
    assert_eq!(edge.rule, "scip-occurrence");
    assert_eq!(stats.upgraded_certain, 1);
}

#[test]
fn e2_trait_member_call_caps_at_probable_not_certain() {
    // Graph: caller(0) -> Iterator::next(1), a virtual/trait-member call. Baseline
    // is `possible`. SCIP resolves it to the trait-method declaration symbol.
    let mut graph = ResolvedGraph {
        nodes: vec![
            node(0, "fixture_crate::caller", "src/lib.rs", 4),
            node(1, "fixture_crate::Shape::area", "src/lib.rs", 1),
        ],
        edges: vec![call_edge(0, 1, "src/lib.rs", 6, 11, Confidence::Possible, Tier::ScopeGraph, "name-method")],
        candidates: vec![],
        unresolved: vec![],
        ..Default::default()
    };
    canonicalize(&mut graph);

    let area = trait_method_symbol("Shape", "area");
    let bytes = index(
        "/repo",
        "rust-analyzer",
        vec![document(
            "src/lib.rs",
            vec![
                occurrence(&[0, 0, 4], &area, ROLE_DEFINITION),
                occurrence(&[5, 10, 14], &area, 0),
            ],
            // KIND_TRAIT on the *enclosing* type marks this a trait member: encode
            // the method's enclosing symbol as the trait, and the trait as a Trait.
            vec![
                symbol_info(&area, KIND_TRAIT, &format!("rust-analyzer cargo {PKG} {VER} Shape#")),
                symbol_info(&format!("rust-analyzer cargo {PKG} {VER} Shape#"), KIND_TRAIT, ""),
            ],
        )],
        vec![],
    );

    let stats = scip_relabel(&mut graph, &resolver(&bytes), &ScipRelabelOpts::default());

    let edge = &graph.edges[0].edge;
    assert_ne!(
        edge.confidence,
        Confidence::Certain,
        "E2 negative: a trait-member (dyn) call must NEVER be certain"
    );
    assert_eq!(
        edge.confidence,
        Confidence::Probable,
        "a trait-member call caps at probable"
    );
    assert_eq!(stats.upgraded_certain, 0);
    assert_eq!(stats.upgraded_probable, 1);
}

#[test]
fn existing_certain_scope_graph_edge_is_never_downgraded() {
    // A same-file lexical call already carries certain@scope-graph. SCIP resolves
    // the same site to a trait member (probable ceiling). Upgrade-only ⇒ the
    // existing certain must survive untouched.
    let mut graph = ResolvedGraph {
        nodes: vec![
            node(0, "fixture_crate::caller", "src/lib.rs", 4),
            node(1, "fixture_crate::Shape::area", "src/lib.rs", 1),
        ],
        edges: vec![call_edge(0, 1, "src/lib.rs", 6, 11, Confidence::Certain, Tier::ScopeGraph, "scope-ref")],
        candidates: vec![],
        unresolved: vec![],
        ..Default::default()
    };
    canonicalize(&mut graph);

    let area = trait_method_symbol("Shape", "area");
    let bytes = index(
        "/repo",
        "rust-analyzer",
        vec![document(
            "src/lib.rs",
            vec![occurrence(&[5, 10, 14], &area, 0)],
            vec![symbol_info(&area, KIND_TRAIT, &format!("rust-analyzer cargo {PKG} {VER} Shape#"))],
        )],
        vec![],
    );

    scip_relabel(&mut graph, &resolver(&bytes), &ScipRelabelOpts::default());

    let edge = &graph.edges[0].edge;
    assert_eq!(edge.confidence, Confidence::Certain, "must not downgrade");
    assert_eq!(edge.tier, Tier::ScopeGraph, "tier must stay ScopeGraph");
    assert_eq!(edge.rule, "scope-ref", "rule must be untouched");
}

#[test]
fn double_def_site_collision_18772_caps_at_probable() {
    // `helper` has TWO definition sites (the #18772 inherent-impl collision).
    // def_count > 1 ⇒ cannot claim uniqueness ⇒ probable, never certain.
    let mut graph = ResolvedGraph {
        nodes: vec![
            node(0, "fixture_crate::caller", "src/lib.rs", 9),
            node(1, "fixture_crate::helper", "src/lib.rs", 1),
        ],
        edges: vec![call_edge(0, 1, "src/lib.rs", 10, 5, Confidence::Possible, Tier::NameSyntactic, "name-arity")],
        candidates: vec![],
        unresolved: vec![],
        ..Default::default()
    };
    canonicalize(&mut graph);

    let helper = free_fn_symbol("helper");
    let bytes = index(
        "/repo",
        "rust-analyzer",
        vec![
            document(
                "src/a.rs",
                vec![occurrence(&[0, 0, 6], &helper, ROLE_DEFINITION)],
                vec![],
            ),
            document(
                "src/lib.rs",
                vec![
                    occurrence(&[0, 0, 6], &helper, ROLE_DEFINITION), // second def site
                    occurrence(&[9, 4, 10], &helper, 0),              // the call site
                ],
                vec![],
            ),
        ],
        vec![],
    );

    let stats = scip_relabel(&mut graph, &resolver(&bytes), &ScipRelabelOpts::default());

    let edge = &graph.edges[0].edge;
    assert_eq!(
        edge.confidence,
        Confidence::Probable,
        "#18772 double-def must cap at probable, never certain"
    );
    assert!(stats.collisions >= 1, "the collision must be counted for cgx doctor");
    assert_eq!(stats.upgraded_certain, 0);
}

#[test]
fn cross_crate_ref_emits_scip_dep_edge() {
    // The call resolves to a symbol in a DIFFERENT package (`serde`). A GM-14
    // dependency edge must be emitted, encoding `(pkg, version)` in `rule`.
    let mut graph = ResolvedGraph {
        nodes: vec![node(0, "fixture_crate::caller", "src/lib.rs", 4), node(1, "fixture_crate::add", "src/lib.rs", 1)],
        // One in-tree def (add) so the pass infers `fixture_crate` as local.
        edges: vec![call_edge(0, 1, "src/lib.rs", 6, 5, Confidence::Possible, Tier::NameSyntactic, "name-arity")],
        candidates: vec![],
        unresolved: vec![],
        ..Default::default()
    };
    canonicalize(&mut graph);

    let local_add = free_fn_symbol("add");
    let serde_fn = "rust-analyzer cargo serde 1.0.200 from_str().".to_string();
    let bytes = index(
        "/repo",
        "rust-analyzer",
        vec![document(
            "src/lib.rs",
            vec![
                occurrence(&[0, 0, 3], &local_add, ROLE_DEFINITION), // local def → local pkg
                occurrence(&[5, 4, 12], &serde_fn, 0),               // cross-crate ref at the site
            ],
            vec![],
        )],
        vec![symbol_info(&serde_fn, 0, "")],
    );

    let stats = scip_relabel(&mut graph, &resolver(&bytes), &ScipRelabelOpts::default());

    assert_eq!(stats.dep_edges, 1, "a cross-crate ref must emit one dep edge");
    let dep = graph
        .edges
        .iter()
        .find(|e| e.edge.rule.starts_with("scip-dep:"))
        .expect("a scip-dep: edge must exist");
    assert_eq!(
        dep.edge.rule, "scip-dep:serde@1.0.200",
        "the dep edge encodes (pkg, version) in rule (R5)"
    );
    assert_eq!(dep.edge.tier, Tier::Scip);
}

#[test]
fn dst_redirect_and_group_collapse_recanonicalizes() {
    // SCIP uniquely resolves a call that the syntactic pass over-approximated to
    // the WRONG in-tree target (a candidate group). The pass redirects dst to the
    // unique SCIP target and collapses the group, which re-canonicalizes EdgeIds.
    let mut graph = ResolvedGraph {
        nodes: vec![
            node(0, "fixture_crate::caller", "src/lib.rs", 9),
            node(1, "fixture_crate::wrong_target", "src/lib.rs", 1),
            node(2, "fixture_crate::right_target", "src/lib.rs", 5),
        ],
        edges: vec![call_edge(0, 1, "src/lib.rs", 10, 5, Confidence::Possible, Tier::NameSyntactic, "name-arity")],
        candidates: vec![],
        unresolved: vec![],
        ..Default::default()
    };
    graph.edges[0].edge.candidate_group = Some(0);
    canonicalize(&mut graph);

    let right = free_fn_symbol("right_target");
    let bytes = index(
        "/repo",
        "rust-analyzer",
        vec![document(
            "src/lib.rs",
            vec![
                occurrence(&[4, 0, 12], &right, ROLE_DEFINITION),
                occurrence(&[9, 4, 16], &right, 0),
            ],
            vec![],
        )],
        vec![],
    );

    scip_relabel(&mut graph, &resolver(&bytes), &ScipRelabelOpts::default());

    let edge = &graph.edges[0].edge;
    assert_eq!(edge.dst, NodeId(2), "dst redirected to the unique SCIP target");
    assert_eq!(edge.candidate_group, None, "candidate group collapsed");
    assert_eq!(edge.confidence, Confidence::Certain, "unique def → certain");
    // EdgeIds must be densely reassigned (canonicalize ran).
    assert_eq!(graph.edges[0].edge.id, EdgeId(0));
}

#[test]
fn no_scip_occurrence_leaves_edge_untouched() {
    // SCIP saw nothing at the call site → the pass must be honest and not touch
    // the edge at all.
    let mut graph = ResolvedGraph {
        nodes: vec![node(0, "fixture_crate::caller", "src/lib.rs", 4), node(1, "fixture_crate::add", "src/lib.rs", 1)],
        edges: vec![call_edge(0, 1, "src/lib.rs", 5, 9, Confidence::Probable, Tier::ScopeGraph, "import-ref")],
        candidates: vec![],
        unresolved: vec![],
        ..Default::default()
    };
    canonicalize(&mut graph);

    // An empty SCIP document — no occurrences at the site.
    let bytes = index("/repo", "rust-analyzer", vec![document("src/lib.rs", vec![], vec![])], vec![]);
    let stats = scip_relabel(&mut graph, &resolver(&bytes), &ScipRelabelOpts::default());

    let edge = &graph.edges[0].edge;
    assert_eq!(edge.confidence, Confidence::Probable);
    assert_eq!(edge.tier, Tier::ScopeGraph);
    assert_eq!(edge.rule, "import-ref");
    assert_eq!(stats.upgraded_certain + stats.upgraded_probable + stats.dep_edges, 0);
}
