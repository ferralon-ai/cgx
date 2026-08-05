//! `impacted-tests`: the answer types, the closure-containment lift, and the
//! caller-side approximation contract for the multi-root `Backward` walk in
//! [`crate::engine::impacted_tests`].
//!
//! The walk itself lives in [`crate::engine`] because the emitted ordering key
//! (`result_key`) is module-private there and is the crate's determinism
//! contract; duplicating it into a sibling module is exactly the drift AR-10
//! exists to prevent.
//!
//! ## Why the contract is built here and not in [`crate::contract`]
//!
//! `contract::for_neighbors` takes a **single** root and `contract::for_unused`
//! hardcodes `Direction::Forward`. There is no public builder for a *multi-root
//! `Backward`* answer, and `contract.rs` is owned by another cycle. Calling
//! `for_neighbors` once per root would cost `k` extra BFS walks **and** compute
//! the depth horizon wrongly for a multi-root union (a node at root A's horizon
//! may be interior to root B's walk). So the frontier scan is replicated here
//! over the `reached` array the walk already produced, and the final contract is
//! assembled by public struct literal.

use std::collections::{BTreeMap, BTreeSet};

use cgx_core::{Confidence, CutMarker, EdgeCondition, EdgeId, EdgeKind, EdgeRecord, NodeId, Tier};

use crate::contract::{
    ApproxDirection, ApproxReason, ApproximationContract, NegativeScope, ReasonDirection,
    MODELED_GRAPH,
};
use crate::filter::{ConditionFilter, Direction, EdgeFilter};
use crate::result::NeighborResult;
use crate::view::GraphView;
use crate::walk::PathWalker;

/// The languages for which cgx can identify test entrypoints at all, in byte
/// order. A changed symbol outside this set fires `impacted-language-unsupported`
/// and makes the answer visibly degenerate rather than silently empty.
pub const SUPPORTED_LANGS: [&str; 4] = ["go", "java", "python", "rust"];

/// The `rule` stamped on the synthetic `via` of a test that is **itself** in the
/// changed set. See [`changed_symbol_marker`].
pub const CHANGED_SYMBOL_RULE: &str = "impacted-changed-symbol";

/// How one reported test reaches the change.
///
/// `chain` runs `test → … → root` inclusive, reconstructed in `O(depth)` from
/// the single BFS's discovery records — never a second `shortest_path` walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactedWitness {
    /// The changed symbol this test reaches.
    pub root: NodeId,
    /// `test → … → root`, inclusive. A single-element chain means the test is
    /// itself in the changed set.
    pub chain: Vec<NodeId>,
    /// Whether any step of `chain` is a closure-containment lift rather than a
    /// call edge. Containment is not invocation, so such a row is an `over`.
    pub via_containment_lift: bool,
}

/// The answer of [`crate::engine::impacted_tests`].
#[derive(Debug, Clone, Default)]
pub struct ImpactedTests {
    /// The impacted test entrypoints, in IF-8 `(file, line_start, fqn)` order.
    pub tests: Vec<NeighborResult>,
    /// Index-parallel to [`tests`](Self::tests).
    pub witnesses: Vec<ImpactedWitness>,
    /// Size of the changed-symbol root set that was walked.
    pub changed_symbols: usize,
    /// `|S_file \ S_struct|` — symbols that entered the changed set only because
    /// their defining file changed. Set by the caller that built the set.
    pub file_granular_only: usize,
    /// How many *reported tests* were reached across at least one containment
    /// lift (not how many lift operations the walk performed).
    pub lifted: usize,
    /// The `lang` tags of the changed symbols.
    pub langs_in_play: BTreeSet<String>,
    /// Reverse-reachability marker indexed by `NodeId::index()`, spanning every
    /// node of the view. Retained because the approximation contract's frontier
    /// scan is a pure `O(V)` pass over it — recomputing it would mean re-walking.
    pub reached: Vec<bool>,
}

impl ImpactedTests {
    /// How many nodes the backward walk reached, seeds included.
    ///
    /// **Not** the ADR-08 `unfiltered_count`. That field means "results this
    /// query returns with the confidence/edge-condition filters removed", and is
    /// used to attribute an empty answer to a filter; this is a walk-size
    /// statistic that is non-zero whenever anything changed, because seeds are
    /// marked reached at seeding. Passing it as `unfiltered_count` makes every
    /// honestly-empty `--assert-empty` gate read as vacuous.
    pub fn reached_count(&self) -> usize {
        self.reached.iter().filter(|r| **r).count()
    }
}

/// Diff-side facts the walk cannot see, folded into the contract by
/// [`contract_for`].
#[derive(Debug, Clone, Default)]
pub struct DiffFacts {
    /// `GraphDiff::removed_nodes.len()` — deleted symbols have no head-side node
    /// id, so no reverse walk can reach the tests that exercised them.
    pub removed_symbols: usize,
    /// Changed paths that contributed zero head nodes (no adapter claims the
    /// extension, or the file defines nothing).
    pub unindexed_changed_files: usize,
    /// Working-tree paths the caller's changed-path narrowing **dropped** as
    /// untracked-and-unclaimed, excluding cgx's own store. They never reach
    /// [`unindexed_changed_files`], because on an ordinary run they are
    /// `target/`-shaped noise. When the changed-symbol set comes out empty they
    /// are the only thing that changed, and an empty answer over them is not
    /// exact — see [`contract_for`].
    pub dropped_unclaimed_paths: usize,
    /// `true` when the base is the ref tip rather than the merge-base.
    pub tip_to_tip_base: bool,
}

/// The enclosing named symbol's FQN for an adapter-synthesised lambda FQN.
///
/// ```text
/// Go:   pkg::TestOuter::{func@8:17}      -> pkg::TestOuter
/// Rust: krate::m::test_x::{closure@9:5}  -> krate::m::test_x
/// pkg::helper                            -> None
/// ```
///
/// Exactly one hop, always: Go's `walk_func_literal`
/// (`cgx-lang-go/src/extract.rs:633-641`) and Rust's `walk_closure`
/// (`cgx-lang-rust/src/extract.rs:1003-1012`) both build the segment from
/// `ctx.fqn_prefix`, which only `Ctx::enter_body` changes and which closure
/// descent (`with_scope`) preserves — so a closure nested inside a closure still
/// carries the enclosing *named* callable's FQN, never another lambda's.
///
/// Guard with `node.kind == SymbolKind::Lambda` before calling: Java and Python
/// emit no `Lambda` nodes at all, so the lift is a sound no-op there.
pub fn lambda_owner_fqn(fqn: &str) -> Option<&str> {
    let (head, last) = fqn.rsplit_once("::")?;
    let is_lambda_seg =
        last.ends_with('}') && (last.starts_with("{func@") || last.starts_with("{closure@"));
    is_lambda_seg.then_some(head)
}

/// The `via` stand-in for a test that is **itself** in the changed set.
///
/// [`NeighborResult::via`] is not optional and no real edge justifies such a row
/// — the test is in the answer because the change is *inside it*, not because it
/// calls the change. The marker is self-identifying (`src == dst`,
/// `id == EdgeId(u32::MAX)`, `rule == `[`CHANGED_SYMBOL_RULE`]) so no consumer
/// mistakes it for a resolved call edge, and the row's witness chain is the bare
/// `[test]` with no edge for a forest renderer to draw.
pub fn changed_symbol_marker(node: NodeId) -> EdgeRecord {
    EdgeRecord {
        id: EdgeId(u32::MAX),
        src: node,
        dst: node,
        kind: EdgeKind::Calls,
        condition: EdgeCondition::Always,
        confidence: Confidence::Certain,
        tier: Tier::NameSyntactic,
        rule: CHANGED_SYMBOL_RULE.to_string(),
        site_id: None,
        stmt_index: None,
        cut_markers: Default::default(),
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
        transform: None,
    }
}

/// Whether `edge` is the [`changed_symbol_marker`] rather than a graph fact.
pub fn is_changed_symbol_marker(edge: &EdgeRecord) -> bool {
    edge.id == EdgeId(u32::MAX) && edge.rule == CHANGED_SYMBOL_RULE
}

// --- the approximation contract ---------------------------------------------

/// The incompleteness facts on the frontier of the reverse walk.
#[derive(Debug, Default)]
struct FrontierFacts {
    dangling_refs: u64,
    cut_counts: BTreeMap<CutMarker, usize>,
    below_floor: usize,
    depth_truncated: bool,
}

// DUPLICATES cgx-query::contract::scan_frontier (contract.rs:361-433). Delete when
// freshness-envelope lands a multi-root Backward contract builder —
// see escalations/impacted-tests-01.md. Calling contract::for_neighbors per root
// instead would cost k extra BFS walks AND compute the depth horizon wrongly.
//
// Two deliberate differences from the original:
//
//  1. The dangling-ref count is ENABLED here, where the original disables it on
//     any backward walk (`contract.rs:407`), and it is scoped to the **unreached**
//     nodes. The failure mode is: test `T` calls unindexed `H` (no edge, so
//     `T.unresolved_calls += 1`), `H` calls the changed symbol `C`, and the
//     backward walk from `C` never touches `T`. The severing node is by
//     definition not reached; a node that *was* reached has its unresolved
//     out-calls pointing downstream of itself, which cannot bear on whether it
//     reaches the change. Sharper than a repo-wide sum and still sound.
//  2. The depth horizon is a property of the multi-root union, not of any one
//     root: an admitted backward neighbour of a reached node that is itself
//     unreached can only have been stopped by the depth bound, because a BFS
//     from any root expands every admitted neighbour of every node it reaches.
fn scan_frontier(view: &GraphView, walker: &PathWalker, reached: &[bool]) -> FrontierFacts {
    // A kind-only filter: every edge of the searched family regardless of the
    // confidence floor or condition restriction, so cuts and below-floor edges
    // the *walk* skipped are still seen as honest incompleteness sources.
    let scan_filter = EdgeFilter {
        min_confidence: Confidence::Possible,
        condition: ConditionFilter::Any,
        kinds: walker.filter.kinds.clone(),
    };
    let floor = walker.filter.min_confidence;
    let bounded = walker.max_depth.is_some();

    let mut facts = FrontierFacts::default();
    for node in view.nodes() {
        let i = node.id.index();
        if i >= reached.len() {
            continue;
        }
        if !reached[i] {
            facts.dangling_refs += u64::from(node.unresolved_calls);
            continue;
        }
        for er in view.neighbors(node.id, Direction::Backward, &scan_filter) {
            for marker in er.edge.cut_markers.iter() {
                *facts.cut_counts.entry(marker).or_default() += 1;
            }
            if er.edge.confidence < floor {
                facts.below_floor += 1;
            }
        }
        if bounded && !facts.depth_truncated {
            facts.depth_truncated = view
                .neighbors(node.id, Direction::Backward, &walker.filter)
                .any(|er| reached.get(er.peer.index()).is_some_and(|r| !*r));
        }
    }
    facts
}

/// The per-answer approximation contract for an `impacted-tests` answer.
///
/// Reason emission order is fixed (AR-10): the frontier-derived reasons first
/// (dangling → cut markers → below-floor → depth-limit), then the remaining
/// `under` reasons in vocabulary order with the four per-language reasons sorted
/// by lang token, then the `over` reasons in vocabulary order. `cut_counts` comes
/// out of a `BTreeMap` and `langs_in_play` out of a `BTreeSet`, so no hash
/// iteration order reaches the output.
pub fn contract_for(
    view: &GraphView,
    walker: &PathWalker,
    changed: &[NodeId],
    tests: &ImpactedTests,
    facts: &DiffFacts,
) -> ApproximationContract {
    let mut reasons: Vec<ApproxReason> = Vec::new();

    // --- under: the frontier scan -------------------------------------------
    let frontier = scan_frontier(view, walker, &tests.reached);
    if frontier.dangling_refs > 0 {
        // `unresolved_calls` counts every call site the resolver could not bind
        // to an in-repo node, and an external/unindexed callee is only one way
        // to get there. A call written inside an unexpanded macro argument
        // lands here too, with the callee sitting in the same crate one hop
        // away — naming only the external case tells a user with no external
        // dependency on that path that the reason does not apply to them, and
        // they would be wrong. The per-language reasons name the concrete
        // shapes; this one must not narrow the cause it cannot distinguish.
        reasons.push(under(
            "impacted-unresolved-external-calls",
            format!(
                "{} call(s) in symbols outside the walked region resolved to no in-repo target — \
                 an external or unindexed callee (no SCIP), or a call site the frontend does not \
                 model as an edge, such as one written inside an unexpanded macro. A test \
                 reaching your change only through such a call is not in this answer.",
                frontier.dangling_refs
            ),
        ));
    }
    for (marker, count) in &frontier.cut_counts {
        let (code, phrase) = cut_phrase(*marker);
        reasons.push(under(
            code,
            format!("{phrase} ({count} site(s) on the searched frontier)"),
        ));
    }
    if frontier.below_floor > 0 {
        reasons.push(under(
            "below-confidence-floor",
            format!(
                "{} edge(s) below the confidence>={} floor were excluded from the search",
                frontier.below_floor,
                confidence_token(walker.filter.min_confidence)
            ),
        ));
    }
    if frontier.depth_truncated {
        if let Some(d) = walker.max_depth {
            reasons.push(under(
                "depth-limit",
                format!("search stopped at depth {d}; deeper edges were not explored"),
            ));
        }
    }

    // --- under: per-language test-recognition gaps (decision-01) ------------
    for lang in &tests.langs_in_play {
        if let Some((code, detail)) = recognition_gap(lang) {
            reasons.push(under(code, detail.to_string()));
        }
    }

    // --- under: the unsupported-language remainder --------------------------
    let unsupported: BTreeSet<&str> = changed
        .iter()
        .filter_map(|n| view.try_node(*n))
        .map(|nd| nd.lang.as_str())
        .filter(|l| !SUPPORTED_LANGS.contains(l))
        .collect();
    if !unsupported.is_empty() {
        let count = changed
            .iter()
            .filter_map(|n| view.try_node(*n))
            .filter(|nd| !SUPPORTED_LANGS.contains(&nd.lang.as_str()))
            .count();
        let langs = unsupported.iter().copied().collect::<Vec<_>>().join(", ");
        reasons.push(under(
            "impacted-language-unsupported",
            format!(
                "{count} changed symbol(s) are in {langs}, for which cgx cannot identify tests at \
                 all (EntrypointHint FQNs are built from the test's string label and never match a \
                 symbol FQN; calls inside inline test callbacks produce no graph edge). No {langs} \
                 test appears in this answer."
            ),
        ));
    }

    // --- under: diff-side facts ---------------------------------------------
    if facts.unindexed_changed_files > 0 {
        reasons.push(under(
            "impacted-changed-file-unindexed",
            format!(
                "{} changed file(s) contributed no symbols (no adapter claims the extension, or \
                 the file defines none); edits there are not represented in the changed set.",
                facts.unindexed_changed_files
            ),
        ));
    } else if changed.is_empty() && facts.dropped_unclaimed_paths > 0 {
        // The changed-path narrowing dropped every path it saw, so the walk was
        // seeded with nothing and the answer is empty for a reason the user
        // cannot see. Emitting nothing here is how an `impacted-tests` answer
        // reads `exact` while having silently discarded the only thing that
        // changed — the one failure this command exists to prevent.
        //
        // Gated on an *empty changed set* rather than on the drop itself: while
        // anything else changed, the dropped paths are `target/`- and
        // `node_modules/`-shaped noise, and firing on them would put that noise
        // back on every ordinary answer. Counting them honestly needs git's
        // exclude rules, which `Repo::enumerate_workdir` does not consult — but
        // the `exact` claim does not require a count, only the knowledge that
        // something was discarded.
        reasons.push(under(
            "impacted-changed-file-unindexed",
            format!(
                "the changed-symbol set is empty and {} working-tree path(s) were dropped from it \
                 as untracked and unclaimed (no adapter claims the extension, or the file defines \
                 nothing). cgx reads no gitignore here, so it cannot tell ignored build output \
                 from a genuinely new unmodeled file such as a migration or a config: this empty \
                 answer is not a clean bill of health.",
                facts.dropped_unclaimed_paths
            ),
        ));
    }
    if facts.removed_symbols > 0 {
        reasons.push(under(
            "impacted-removed-symbols-not-walked",
            format!(
                "{} symbol(s) were deleted at head; tests that exercised them are not in this \
                 answer.",
                facts.removed_symbols
            ),
        ));
    }

    // --- over ---------------------------------------------------------------
    if tests.file_granular_only > 0 {
        reasons.push(over(
            "impacted-changed-set-file-granular",
            format!(
                "{} symbol(s) entered the changed set because their defining file changed, not \
                 because their call graph changed; some may be unaffected by the edit.",
                tests.file_granular_only
            ),
        ));
    }
    if tests.lifted > 0 {
        reasons.push(over(
            "impacted-closure-containment-lift",
            format!(
                "{} test(s) were reached by walking from a closure body to its lexically enclosing \
                 function. Containment is not invocation: the closure may never be invoked by that \
                 test.",
                tests.lifted
            ),
        ));
    }
    if facts.tip_to_tip_base {
        reasons.push(over(
            "impacted-tip-to-tip-base",
            "base is the ref tip, not the merge-base of base and head; symbols that landed on the \
             base ref since the branch point are counted as changed."
                .to_string(),
        ));
    }
    if tests.tests.iter().any(|r| {
        r.min_confidence_on_path == Confidence::Possible || r.via.candidate_group.is_some()
    }) {
        reasons.push(over(
            "over-approx-candidate-set",
            "resolved through an over-approximated candidate set (dynamic dispatch or \
             name-collision); some reported edges may not occur"
                .to_string(),
        ));
    }

    let has_over = reasons.iter().any(|r| r.direction == ReasonDirection::Over);
    let has_under = reasons
        .iter()
        .any(|r| r.direction == ReasonDirection::Under);
    let direction = match (has_over, has_under) {
        (false, false) => ApproxDirection::Exact,
        (true, false) => ApproxDirection::Over,
        (false, true) => ApproxDirection::Under,
        (true, true) => ApproxDirection::OverUnder,
    };

    ApproximationContract {
        direction,
        reasons,
        modeled_graph: MODELED_GRAPH,
        scope: tests.tests.is_empty().then(|| NegativeScope {
            searched_edge_kinds: searched_kinds(&walker.filter),
            confidence_floor: walker.filter.min_confidence,
            max_depth: walker.max_depth,
        }),
    }
}

fn under(code: &'static str, detail: String) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code,
        detail,
    }
}

fn over(code: &'static str, detail: String) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Over,
        code,
        detail,
    }
}

/// The per-language test-recognition gaps carried by the contract instead of
/// closed in the adapters (`execution/decision-01-adapter-gaps.md`). A user must
/// never receive a test set that silently omits their `#[tokio::test]` cases.
fn recognition_gap(lang: &str) -> Option<(&'static str, &'static str)> {
    match lang {
        "go" => Some((
            "impacted-test-recognition-incomplete-go",
            "go: FuzzXxx is not recognised as a test; t.Run subtest closures are recovered by \
             containment only (see over-reasons), not by a call edge.",
        )),
        "java" => Some((
            "impacted-test-recognition-incomplete-java",
            "java: JUnit5 @ParameterizedTest/@RepeatedTest/@TestFactory/@TestTemplate and TestNG \
             class-level @Test are not recognised as tests.",
        )),
        "python" => Some((
            "impacted-test-recognition-incomplete-python",
            "python: unittest camelCase testFoo methods, non-default pytest \
             python_files/python_functions config, and TestCase chains through an unindexed \
             third-party base are not recognised as tests.",
        )),
        // The assertion-macro clause leads because it is by far the most common
        // shape: `assert_eq!(add(1, 1), 2)` is the idiomatic Rust unit test, and
        // the call inside it is an unexpanded macro argument that produces no
        // graph edge at all, so a reverse walk never reaches the test. Verified
        // Rust-only — the equivalent Python `assert`, Java `assertEquals` and Go
        // `if` forms all resolve and all report their test.
        "rust" => Some((
            "impacted-test-recognition-incomplete-rust",
            "rust: a call written inside an assertion macro (assert!, assert_eq!, assert_ne!, \
             matches! and friends) is an unexpanded macro argument and yields no call edge, so a \
             #[test] whose only use of the changed symbol is inside one is not in this answer — \
             bind the call to a local first to make it visible; #[tokio::test], #[async_std::test], \
             #[rstest], #[wasm_bindgen_test] and #[test_log::test] are not recognised as tests; \
             doc-tests have no node identity at all.",
        )),
        _ => None,
    }
}

/// The `CutMarker` reason vocabulary, verbatim from `contract.rs:222-260` so a
/// consumer matches uniformly across every cgx answer.
fn cut_phrase(marker: CutMarker) -> (&'static str, &'static str) {
    match marker {
        CutMarker::Unresolved => (
            "unresolved-call",
            "external/unindexed callees not modeled (no SCIP)",
        ),
        CutMarker::Dynamic => ("dynamic-dispatch", "dynamic/plugin dispatch not resolved"),
        CutMarker::Reflective => (
            "reflective-dispatch",
            "reflective/string-computed dispatch not resolved",
        ),
        CutMarker::ViaFfi => (
            "foreign-function",
            "calls crossing an FFI boundary not modeled",
        ),
        CutMarker::ViaDi => (
            "dependency-injection",
            "dependency-injection dispatch not resolved",
        ),
        CutMarker::UnexpandedMacro => (
            "unexpanded-macro",
            "calls inside unexpanded macros not modeled",
        ),
        CutMarker::OpaqueCall => (
            "opaque-dataflow",
            "dataflow through an un-summarized callee not modeled",
        ),
        CutMarker::TruncatedAccessPath => (
            "truncated-access-path",
            "deep field-access dataflow truncated to its base value",
        ),
        CutMarker::SummaryBudgetExceeded => (
            "summary-budget",
            "interprocedural dataflow summary budget exhausted",
        ),
    }
}

/// Mirrors `contract.rs:450-462`.
fn searched_kinds(filter: &EdgeFilter) -> Vec<EdgeKind> {
    match &filter.kinds {
        Some(kinds) => kinds.clone(),
        None => vec![
            EdgeKind::Calls,
            EdgeKind::CallsVirtual,
            EdgeKind::CallsClosure,
            EdgeKind::CallsCallback,
            EdgeKind::CallsAsync,
            EdgeKind::CallsIndirect,
        ],
    }
}

fn confidence_token(c: Confidence) -> &'static str {
    match c {
        Confidence::Possible => "possible",
        Confidence::Probable => "probable",
        Confidence::Certain => "certain",
    }
}
