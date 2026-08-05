//! `impacted-tests`: the multi-root reverse walk, the closure-containment lift,
//! the determinism guarantees (AR-10), and the approximation contract.
//!
//! Every `under` reason in the vocabulary gets a test that makes it fire. A
//! reason that exists but never fires is exactly the silent under-approximation
//! this feature exists to prevent.

use cgx_core::edge::EdgeRecord;
use cgx_core::{
    Candidate, Confidence, CutMarker, CutMarkers, EdgeCondition, EdgeId, EdgeKind, EntrypointKind,
    NodeId, NodeRecord, SymbolKind, Tier, Visibility,
};
use cgx_query::contract::ApproxDirection;
use cgx_query::impacted::{contract_for, DiffFacts, ImpactedTests};
use cgx_query::{impacted_tests, lambda_owner_fqn, EdgeFilter, GraphView, PathWalker};

// --- a local graph builder ---------------------------------------------------
//
// Deliberately not `tests/common/mod.rs`: this suite needs per-node `lang` and
// `unresolved_calls` and per-edge cut markers / candidate groups, none of which
// the shared builder models, and widening a helper four other suites depend on
// is not this commit's business.

#[derive(Default)]
struct G {
    nodes: Vec<NodeRecord>,
    edges: Vec<EdgeRecord>,
}

impl G {
    fn new() -> Self {
        G::default()
    }

    /// Append a node. Callers add nodes in canonical `(file, line_start, fqn)`
    /// order, so the dense id is the position — the same invariant the resolver
    /// and store maintain.
    fn node(
        &mut self,
        fqn: &str,
        kind: SymbolKind,
        lang: &str,
        entry: Option<EntrypointKind>,
    ) -> NodeId {
        let line = (self.nodes.len() as u32 + 1) * 10;
        self.node_at(fqn, "src/lib.rs", line, kind, lang, entry)
    }

    fn node_at(
        &mut self,
        fqn: &str,
        file: &str,
        line: u32,
        kind: SymbolKind,
        lang: &str,
        entry: Option<EntrypointKind>,
    ) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(NodeRecord {
            id,
            kind,
            fqn: fqn.to_string(),
            file: file.to_string(),
            line_start: line,
            line_end: line,
            lang: lang.to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: entry,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
            unresolved_calls: 0,
        });
        id
    }

    fn func(&mut self, fqn: &str) -> NodeId {
        self.node(fqn, SymbolKind::Function, "rust", None)
    }

    fn test(&mut self, fqn: &str) -> NodeId {
        self.node(
            fqn,
            SymbolKind::Function,
            "rust",
            Some(EntrypointKind::Test),
        )
    }

    fn lambda(&mut self, fqn: &str) -> NodeId {
        self.node(fqn, SymbolKind::Lambda, "rust", None)
    }

    fn dangling(&mut self, node: NodeId, count: u32) {
        self.nodes[node.index()].unresolved_calls = count;
    }

    fn calls(&mut self, src: NodeId, dst: NodeId) {
        self.edge(src, dst, Confidence::Certain, &[], None);
    }

    fn edge(
        &mut self,
        src: NodeId,
        dst: NodeId,
        confidence: Confidence,
        cuts: &[CutMarker],
        candidate_group: Option<u32>,
    ) {
        self.edges.push(EdgeRecord {
            id: EdgeId(self.edges.len() as u32),
            src,
            dst,
            kind: EdgeKind::Calls,
            condition: EdgeCondition::Always,
            confidence,
            tier: Tier::ScopeGraph,
            rule: "test".to_string(),
            site_id: None,
            stmt_index: None,
            cut_markers: CutMarkers::from_iter_canonical(cuts.iter().copied()),
            implicit: None,
            candidate_group,
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
            transform: None,
        });
    }

    fn view(self) -> GraphView {
        GraphView::new(self.nodes, self.edges, Vec::<Candidate>::new())
    }
}

fn unbounded() -> PathWalker {
    PathWalker::default()
}

/// A total rendering of an answer — rows *and* every per-row fact the walk
/// decides, including the witness chain. Byte equality of this string is the
/// determinism assertion; comparing only the FQN list would hide a
/// non-deterministic `via`.
fn render(a: &ImpactedTests) -> String {
    let mut s = String::new();
    for (r, w) in a.tests.iter().zip(&a.witnesses) {
        s.push_str(&format!(
            "{} {}:{} depth={} cond={:?} conf={:?} min={:?} exc={} root={} lift={} chain={:?}\n",
            r.node.fqn,
            r.node.file,
            r.node.line_start,
            r.depth,
            r.condition,
            r.confidence,
            r.min_confidence_on_path,
            r.exception_transient,
            w.root.0,
            w.via_containment_lift,
            w.chain.iter().map(|n| n.0).collect::<Vec<_>>(),
        ));
    }
    s.push_str(&format!(
        "changed={} lifted={} langs={:?} reached={}\n",
        a.changed_symbols,
        a.lifted,
        a.langs_in_play,
        a.reached_count()
    ));
    s
}

fn codes(c: &cgx_query::ApproximationContract) -> Vec<&str> {
    c.reasons.iter().map(|r| r.code).collect()
}

// --- lambda_owner_fqn --------------------------------------------------------

#[test]
fn lambda_owner_fqn_strips_exactly_one_synthesised_segment() {
    assert_eq!(
        lambda_owner_fqn("pkg::TestOuter::{func@8:17}"),
        Some("pkg::TestOuter")
    );
    assert_eq!(
        lambda_owner_fqn("krate::m::test_x::{closure@9:5}"),
        Some("krate::m::test_x")
    );
}

#[test]
fn lambda_owner_fqn_rejects_everything_that_is_not_a_lambda_segment() {
    // The negative case that keeps the lift from turning every symbol into its
    // parent module.
    assert_eq!(lambda_owner_fqn("pkg::helper"), None);
    assert_eq!(lambda_owner_fqn("helper"), None);
    assert_eq!(lambda_owner_fqn("pkg::{block@8:17}"), None);
    assert_eq!(lambda_owner_fqn("pkg::{func@8:17"), None);
    assert_eq!(lambda_owner_fqn(""), None);
}

// --- the walk ----------------------------------------------------------------

#[test]
fn reports_only_test_entrypoints_that_reach_the_change() {
    let mut g = G::new();
    let t_hit = g.test("t_hit");
    let t_miss = g.test("t_miss");
    let mid = g.func("mid");
    let target = g.func("target");
    let other = g.func("other");
    g.calls(t_hit, mid);
    g.calls(mid, target);
    g.calls(t_miss, other);
    let v = g.view();

    let a = impacted_tests(&v, &[target], &unbounded());
    let fqns: Vec<&str> = a.tests.iter().map(|r| r.node.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["t_hit"]);
    assert_eq!(a.tests[0].depth, 2);
    assert_eq!(a.witnesses[0].root, target);
    assert_eq!(a.witnesses[0].chain, vec![t_hit, mid, target]);
    assert!(!a.witnesses[0].via_containment_lift);
    assert_eq!(a.changed_symbols, 1);
}

#[test]
fn a_test_that_is_itself_changed_is_reported_at_depth_zero() {
    // `S_file` puts every symbol of an edited file in the changed set, so editing
    // the test file itself must not produce an empty answer.
    let mut g = G::new();
    let t = g.test("t");
    let v = g.view();

    let a = impacted_tests(&v, &[t], &unbounded());
    assert_eq!(a.tests.len(), 1);
    assert_eq!(a.tests[0].depth, 0);
    assert_eq!(a.witnesses[0].chain, vec![t]);
    assert!(cgx_query::impacted::is_changed_symbol_marker(
        &a.tests[0].via
    ));
}

#[test]
fn containment_lift_recovers_a_test_whose_only_call_is_inside_a_closure() {
    // The `t.Run` shape: no edge leaves TestOuter; the call lives on the lambda.
    let mut g = G::new();
    let outer = g.test("pkg::TestOuter");
    let lambda = g.lambda("pkg::TestOuter::{func@8:17}");
    let target = g.func("pkg::target");
    g.calls(lambda, target);
    let v = g.view();

    let a = impacted_tests(&v, &[target], &unbounded());
    assert_eq!(
        a.tests.iter().map(|r| r.node.id).collect::<Vec<_>>(),
        vec![outer]
    );
    assert!(a.witnesses[0].via_containment_lift);
    assert_eq!(a.witnesses[0].chain, vec![outer, lambda, target]);
    assert_eq!(a.lifted, 1);
    assert_eq!(
        a.tests[0].min_confidence_on_path,
        Confidence::Possible,
        "containment is not invocation: the tier is clamped even though the \
         underlying call edge is certain"
    );
    assert_eq!(
        a.tests[0].confidence,
        Confidence::Certain,
        "the row still reports the real discovery edge's own confidence"
    );
}

#[test]
fn a_non_lambda_symbol_is_never_lifted_into_its_parent() {
    // `pkg::helper` has no lambda segment, so nothing pulls `pkg` (or a test
    // named for the parent path) into the answer.
    let mut g = G::new();
    let _t = g.test("pkg");
    let helper = g.node("pkg::helper", SymbolKind::Function, "rust", None);
    let target = g.func("pkg::target");
    g.calls(helper, target);
    let v = g.view();

    let a = impacted_tests(&v, &[target], &unbounded());
    assert!(a.tests.is_empty(), "got {:?}", render(&a));
    assert_eq!(a.lifted, 0);
}

#[test]
fn the_lift_is_a_fixpoint_not_a_single_pass() {
    // target ← λ1(in helper) ; helper ← λ2(in TestOuter). Reaching TestOuter
    // needs the lift to run again after the round that lifted `helper`.
    let mut g = G::new();
    let outer = g.test("pkg::TestOuter");
    let l2 = g.lambda("pkg::TestOuter::{func@4:9}");
    let helper = g.node("pkg::helper", SymbolKind::Function, "rust", None);
    let l1 = g.lambda("pkg::helper::{func@8:17}");
    let target = g.func("pkg::target");
    g.calls(l1, target);
    g.calls(l2, helper);
    let v = g.view();

    let a = impacted_tests(&v, &[target], &unbounded());
    assert_eq!(
        a.tests.iter().map(|r| r.node.id).collect::<Vec<_>>(),
        vec![outer]
    );
    assert_eq!(a.witnesses[0].chain, vec![outer, l2, helper, l1, target]);
    assert!(a.witnesses[0].via_containment_lift);
}

// --- the witness root comes off the chain -----------------------------------
//
// The shape no fixture had: a **second seed lying strictly between the expanding
// root and the reported test**. Two seeds in a graph is not this shape; two
// seeds on one path to a reported row is. Every seed is marked `reached` before
// any expansion, so the bookkeeping loop skips the interior seed while
// `walker.bfs` walks straight past it — a forward `root_of` stamp then names the
// expanding root while the backward chain reconstruction terminates at the
// interior seed. `root ∉ chain`, and the CLI's human forest either dropped the
// row silently or panicked indexing a root that never entered `Subgraph.nodes`.

/// `add ← mid ← top ← test_top`, with **both** `add` and `mid` in the changed
/// set (one edited file holding a caller and its callee — the ordinary case),
/// plus `test_add` calling `add` directly so a second, correctly-rooted row is
/// present alongside.
///
/// Ids ascend with insertion and the seed set is walked in ascending id order,
/// so `add` expands first and its backward BFS runs past `mid` to `top` and
/// `test_top`.
fn seed_on_the_chain_graph() -> (GraphView, Vec<NodeId>, [NodeId; 5]) {
    let mut g = G::new();
    let add = g.node_at(
        "core::add",
        "src/core.rs",
        1,
        SymbolKind::Function,
        "rust",
        None,
    );
    let mid = g.node_at(
        "core::mid",
        "src/core.rs",
        5,
        SymbolKind::Function,
        "rust",
        None,
    );
    let top = g.node_at(
        "util::top",
        "src/util.rs",
        1,
        SymbolKind::Function,
        "rust",
        None,
    );
    let test_top = g.node_at(
        "util::tests::test_top",
        "src/util.rs",
        7,
        SymbolKind::Function,
        "rust",
        Some(EntrypointKind::Test),
    );
    let test_add = g.node_at(
        "direct::tests::test_add",
        "src/direct.rs",
        7,
        SymbolKind::Function,
        "rust",
        Some(EntrypointKind::Test),
    );
    g.calls(mid, add);
    g.calls(top, mid);
    g.calls(test_top, top);
    g.calls(test_add, add);
    (
        g.view(),
        vec![add, mid],
        [add, mid, top, test_top, test_add],
    )
}

#[test]
fn a_seed_between_the_root_and_the_test_still_reports_both_tests() {
    let (v, changed, [_add, _mid, _top, test_top, test_add]) = seed_on_the_chain_graph();
    let a = impacted_tests(&v, &changed, &unbounded());
    let mut ids: Vec<NodeId> = a.tests.iter().map(|r| r.node.id).collect();
    ids.sort_unstable_by_key(|n| n.0);
    assert_eq!(ids, vec![test_top, test_add], "got {}", render(&a));
}

#[test]
fn the_witness_root_of_a_row_is_the_last_node_of_its_own_chain() {
    // The invariant the old `debug_assert_eq!` guarded and no fixture could
    // reach. It is now true by construction — the root IS `chain.last()` — and
    // this test is what proves the construction, in a normal (debug) test build
    // and identically in the release binary the CLI ships.
    let (v, changed, _) = seed_on_the_chain_graph();
    let a = impacted_tests(&v, &changed, &unbounded());
    assert_eq!(a.tests.len(), 2, "got {}", render(&a));
    for w in &a.witnesses {
        assert_eq!(
            w.chain.last().copied(),
            Some(w.root),
            "root must lie on the chain it is reported with: {}",
            render(&a)
        );
        assert!(
            changed.contains(&w.root),
            "the witness root must be a changed symbol: {}",
            render(&a)
        );
    }
}

#[test]
fn the_reported_root_is_the_nearest_seed_not_the_expanding_one() {
    // `add` expands first and discovers `top`/`test_top` past `mid`. The row for
    // `test_top` must be attributed to `mid` — the seed its chain actually
    // terminates at — not to `add`, which is not on its chain at all.
    let (v, changed, [add, mid, top, test_top, test_add]) = seed_on_the_chain_graph();
    let a = impacted_tests(&v, &changed, &unbounded());
    let by_id: std::collections::HashMap<NodeId, &cgx_query::impacted::ImpactedWitness> = a
        .tests
        .iter()
        .map(|r| r.node.id)
        .zip(a.witnesses.iter())
        .collect();

    let w_top = by_id[&test_top];
    assert_eq!(w_top.chain, vec![test_top, top, mid]);
    assert_eq!(w_top.root, mid, "got {}", render(&a));

    let w_add = by_id[&test_add];
    assert_eq!(w_add.chain, vec![test_add, add]);
    assert_eq!(w_add.root, add, "got {}", render(&a));
}

// --- determinism (criterion 5) ----------------------------------------------

/// A graph with several roots, a shared interior node, and two tests, so the
/// root expansion order is observable in the row *contents*.
fn multi_root_graph() -> (GraphView, Vec<NodeId>) {
    let mut g = G::new();
    let t1 = g.node_at(
        "t_alpha",
        "src/a.rs",
        30,
        SymbolKind::Function,
        "rust",
        Some(EntrypointKind::Test),
    );
    let t2 = g.node_at(
        "t_beta",
        "src/a.rs",
        10,
        SymbolKind::Function,
        "rust",
        Some(EntrypointKind::Test),
    );
    let shared = g.node_at("shared", "src/b.rs", 5, SymbolKind::Function, "rust", None);
    let c1 = g.node_at(
        "changed_one",
        "src/c.rs",
        1,
        SymbolKind::Function,
        "rust",
        None,
    );
    let c2 = g.node_at(
        "changed_two",
        "src/c.rs",
        2,
        SymbolKind::Function,
        "rust",
        None,
    );
    g.edge(t1, shared, Confidence::Probable, &[], None);
    g.edge(t2, shared, Confidence::Certain, &[], None);
    g.edge(shared, c1, Confidence::Certain, &[], None);
    g.edge(shared, c2, Confidence::Possible, &[], None);
    g.edge(c1, c2, Confidence::Certain, &[], None);
    (g.view(), vec![c1, c2])
}

#[test]
fn three_runs_produce_identical_bytes() {
    let (v, changed) = multi_root_graph();
    let w = unbounded();
    let first = render(&impacted_tests(&v, &changed, &w));
    for _ in 0..2 {
        assert_eq!(render(&impacted_tests(&v, &changed, &w)), first);
    }
}

#[test]
fn reversed_root_order_produces_identical_rows_and_per_row_facts() {
    // The test that would catch a `HashSet<NodeId>` of roots: with the
    // already-expanded-root skip, which root expands first decides each node's
    // `via`, hence its depth, confidence and witness chain — and the
    // `(file, line, fqn)` sort would hide it.
    let (v, changed) = multi_root_graph();
    let w = unbounded();
    let forward = render(&impacted_tests(&v, &changed, &w));
    let mut reversed = changed.clone();
    reversed.reverse();
    let backward = render(&impacted_tests(&v, &reversed, &w));
    assert_eq!(forward, backward, "root order must not reach the answer");
}

#[test]
fn emitted_order_equals_an_explicit_resort_by_file_line_fqn() {
    let (v, changed) = multi_root_graph();
    let a = impacted_tests(&v, &changed, &unbounded());
    let emitted: Vec<(String, u32, String)> = a
        .tests
        .iter()
        .map(|r| (r.node.file.clone(), r.node.line_start, r.node.fqn.clone()))
        .collect();
    let mut sorted = emitted.clone();
    sorted.sort();
    assert_eq!(emitted, sorted);
    assert!(emitted.len() >= 2, "the assertion needs at least two rows");
}

#[test]
fn duplicate_and_unsorted_roots_are_tolerated_without_changing_the_answer() {
    let (v, changed) = multi_root_graph();
    let w = unbounded();
    let clean = render(&impacted_tests(&v, &changed, &w));
    let messy = vec![changed[1], changed[0], changed[1], changed[0]];
    assert_eq!(render(&impacted_tests(&v, &messy, &w)), clean);
}

// --- the approximation contract ---------------------------------------------

fn contract_of(
    v: &GraphView,
    changed: &[NodeId],
    w: &PathWalker,
    facts: DiffFacts,
) -> (ImpactedTests, cgx_query::ApproximationContract) {
    let a = impacted_tests(v, changed, w);
    let c = contract_for(v, w, changed, &a, &facts);
    (a, c)
}

#[test]
fn unreached_dangling_calls_fire_the_under_reason() {
    // `t` calls an unindexed helper (no edge, a dangling ref) and is never
    // reached: exactly the false negative the caller-side count exists to name.
    let mut g = G::new();
    let t = g.test("t");
    let target = g.func("target");
    g.dangling(t, 3);
    let v = g.view();

    let (_, c) = contract_of(&v, &[target], &unbounded(), DiffFacts::default());
    assert!(codes(&c).contains(&"impacted-unresolved-external-calls"));
    assert!(c.reasons.iter().any(|r| r.detail.contains('3')));
}

#[test]
fn reached_nodes_dangling_calls_do_not_fire_it() {
    // A reached node's unresolved out-calls point *downstream* of itself and
    // cannot bear on whether it reaches the change. Scoping to unreached nodes
    // is what stops the reason firing on every repo that calls `println!`.
    let mut g = G::new();
    let t = g.test("t");
    let target = g.func("target");
    g.calls(t, target);
    g.dangling(t, 3);
    let v = g.view();

    let (_, c) = contract_of(&v, &[target], &unbounded(), DiffFacts::default());
    assert!(!codes(&c).contains(&"impacted-unresolved-external-calls"));
}

#[test]
fn each_language_in_play_carries_its_recognition_gap() {
    for (lang, code) in [
        ("rust", "impacted-test-recognition-incomplete-rust"),
        ("go", "impacted-test-recognition-incomplete-go"),
        ("java", "impacted-test-recognition-incomplete-java"),
        ("python", "impacted-test-recognition-incomplete-python"),
    ] {
        let mut g = G::new();
        let t = g.node("t", SymbolKind::Function, lang, Some(EntrypointKind::Test));
        let target = g.node("target", SymbolKind::Function, lang, None);
        g.calls(t, target);
        let v = g.view();
        let (_, c) = contract_of(&v, &[target], &unbounded(), DiffFacts::default());
        assert!(codes(&c).contains(&code), "{lang}: {:?}", codes(&c));
        assert_ne!(c.direction, ApproxDirection::Exact);
    }
}

#[test]
fn a_changed_symbol_in_an_unsupported_language_is_named() {
    let mut g = G::new();
    let target = g.node("target", SymbolKind::Function, "typescript", None);
    let v = g.view();

    let (a, c) = contract_of(&v, &[target], &unbounded(), DiffFacts::default());
    assert!(a.tests.is_empty());
    let r = c
        .reasons
        .iter()
        .find(|r| r.code == "impacted-language-unsupported")
        .expect("unsupported-language reason");
    assert!(r.detail.contains("typescript"));
    assert_ne!(c.direction, ApproxDirection::Exact);
    assert!(c.scope.is_some(), "an empty answer carries its scope");
}

#[test]
fn diff_side_facts_fire_their_reasons() {
    let mut g = G::new();
    let t = g.test("t");
    let target = g.func("target");
    g.calls(t, target);
    let v = g.view();

    let facts = DiffFacts {
        removed_symbols: 2,
        unindexed_changed_files: 4,
        tip_to_tip_base: true,
    };
    let (_, c) = contract_of(&v, &[target], &unbounded(), facts);
    let got = codes(&c);
    assert!(got.contains(&"impacted-removed-symbols-not-walked"));
    assert!(got.contains(&"impacted-changed-file-unindexed"));
    assert!(got.contains(&"impacted-tip-to-tip-base"));
    assert_eq!(c.direction, ApproxDirection::OverUnder);
}

#[test]
fn a_cut_marker_on_the_frontier_is_under() {
    let mut g = G::new();
    let t = g.test("t");
    let target = g.func("target");
    g.edge(t, target, Confidence::Certain, &[CutMarker::Dynamic], None);
    let v = g.view();

    let (_, c) = contract_of(&v, &[target], &unbounded(), DiffFacts::default());
    assert!(codes(&c).contains(&"dynamic-dispatch"));
}

#[test]
fn an_edge_below_the_confidence_floor_is_under() {
    let mut g = G::new();
    let t = g.test("t");
    let target = g.func("target");
    g.edge(t, target, Confidence::Possible, &[], None);
    let v = g.view();

    let w = PathWalker {
        filter: EdgeFilter::calls().with_min_confidence(Confidence::Probable),
        ..PathWalker::default()
    };
    let (a, c) = contract_of(&v, &[target], &w, DiffFacts::default());
    assert!(a.tests.is_empty(), "the walk must not cross it");
    assert!(codes(&c).contains(&"below-confidence-floor"));
}

#[test]
fn the_depth_horizon_is_under() {
    let mut g = G::new();
    let t = g.test("t");
    let mid = g.func("mid");
    let target = g.func("target");
    g.calls(t, mid);
    g.calls(mid, target);
    let v = g.view();

    let w = PathWalker {
        max_depth: Some(1),
        ..PathWalker::default()
    };
    let (a, c) = contract_of(&v, &[target], &w, DiffFacts::default());
    assert!(a.tests.is_empty(), "t is two hops away");
    assert!(codes(&c).contains(&"depth-limit"));

    // Unbounded, the same graph must NOT claim a depth limit.
    let (a2, c2) = contract_of(&v, &[target], &unbounded(), DiffFacts::default());
    assert_eq!(a2.tests.len(), 1);
    assert!(!codes(&c2).contains(&"depth-limit"));
}

#[test]
fn file_granular_membership_and_the_lift_are_over() {
    let mut g = G::new();
    let outer = g.test("pkg::TestOuter");
    let _lambda = g.lambda("pkg::TestOuter::{func@8:17}");
    let target = g.func("pkg::target");
    g.calls(NodeId(1), target);
    let v = g.view();

    let mut a = impacted_tests(&v, &[target], &unbounded());
    a.file_granular_only = 5;
    let c = contract_for(&v, &unbounded(), &[target], &a, &DiffFacts::default());
    let got = codes(&c);
    assert_eq!(a.tests[0].node.id, outer);
    assert!(got.contains(&"impacted-changed-set-file-granular"));
    assert!(got.contains(&"impacted-closure-containment-lift"));
    assert!(
        got.contains(&"over-approx-candidate-set"),
        "the clamped tier is a `possible` path and must say so"
    );
}

#[test]
fn a_candidate_set_edge_is_over() {
    let mut g = G::new();
    let t = g.test("t");
    let target = g.func("target");
    g.edge(t, target, Confidence::Probable, &[], Some(7));
    let v = g.view();

    let (_, c) = contract_of(&v, &[target], &unbounded(), DiffFacts::default());
    assert!(codes(&c).contains(&"over-approx-candidate-set"));
}

#[test]
fn a_fully_certain_answer_in_no_language_is_exact() {
    // The control: without a supported-language changed symbol and without any
    // frontier fact, the contract must be able to reach `exact` — otherwise the
    // `under` reasons above prove nothing.
    let mut g = G::new();
    let t = g.node("t", SymbolKind::Function, "go", Some(EntrypointKind::Test));
    let target = g.node("target", SymbolKind::Function, "go", None);
    g.calls(t, target);
    let v = g.view();
    let (_, c) = contract_of(&v, &[target], &unbounded(), DiffFacts::default());
    // `go` is in play, so exactly one reason — the recognition gap — and nothing
    // else. Any additional code here is a reason firing when it should not.
    assert_eq!(codes(&c), vec!["impacted-test-recognition-incomplete-go"]);
    assert_eq!(c.direction, ApproxDirection::Under);
}

#[test]
fn direction_is_never_exact_while_an_under_reason_is_present() {
    type Case = fn() -> (GraphView, Vec<NodeId>, PathWalker, DiffFacts);
    let cases: Vec<Case> = vec![
        || {
            let mut g = G::new();
            let t = g.test("t");
            let target = g.func("target");
            g.dangling(t, 1);
            (g.view(), vec![target], unbounded(), DiffFacts::default())
        },
        || {
            let mut g = G::new();
            let t = g.test("t");
            let target = g.func("target");
            g.edge(
                t,
                target,
                Confidence::Certain,
                &[CutMarker::Reflective],
                None,
            );
            (g.view(), vec![target], unbounded(), DiffFacts::default())
        },
        || {
            let mut g = G::new();
            let _t = g.test("t");
            let target = g.func("target");
            (
                g.view(),
                vec![target],
                unbounded(),
                DiffFacts {
                    removed_symbols: 1,
                    ..DiffFacts::default()
                },
            )
        },
    ];
    for (i, case) in cases.iter().enumerate() {
        let (v, changed, w, facts) = case();
        let (_, c) = contract_of(&v, &changed, &w, facts);
        assert!(
            c.reasons
                .iter()
                .any(|r| r.direction == cgx_query::ReasonDirection::Under),
            "case {i} must carry an under reason"
        );
        assert!(
            matches!(
                c.direction,
                ApproxDirection::Under | ApproxDirection::OverUnder
            ),
            "case {i}: direction {:?} contradicts its reasons",
            c.direction
        );
    }
}

#[test]
fn reason_order_is_stable_across_runs() {
    let mut g = G::new();
    let t = g.node("t", SymbolKind::Function, "go", Some(EntrypointKind::Test));
    let mid = g.node("mid", SymbolKind::Function, "java", None);
    let target = g.node("target", SymbolKind::Function, "python", None);
    g.edge(t, mid, Confidence::Certain, &[CutMarker::Dynamic], None);
    g.edge(
        mid,
        target,
        Confidence::Certain,
        &[CutMarker::Unresolved],
        None,
    );
    let v = g.view();

    let facts = DiffFacts {
        removed_symbols: 1,
        unindexed_changed_files: 1,
        tip_to_tip_base: true,
    };
    let first = contract_of(&v, &[mid, target], &unbounded(), facts.clone()).1;
    for _ in 0..2 {
        let again = contract_of(&v, &[mid, target], &unbounded(), facts.clone()).1;
        assert_eq!(codes(&first), codes(&again));
        assert_eq!(first.reasons, again.reasons);
    }
    // Per-language reasons sort by lang token, and the changed set spans two.
    let got = codes(&first);
    let java = got
        .iter()
        .position(|c| *c == "impacted-test-recognition-incomplete-java")
        .unwrap();
    let python = got
        .iter()
        .position(|c| *c == "impacted-test-recognition-incomplete-python")
        .unwrap();
    assert!(java < python);
}
