//! The per-answer **approximation contract** (A3) and **negative-completeness
//! scope** (A4): the honesty layer that says, per answer, *which direction it can
//! be wrong and why*.
//!
//! CGX's edges already carry the raw honesty facts — a resolution [`Confidence`]
//! band (GM-5), over-approximated candidate-set grouping (GM-2.1), and
//! [`CutMarker`]s for the blind spots no static resolution can follow (ADR-07).
//! This module folds those per-edge facts, plus the walk's own bounds, into a
//! single statement attached to the *answer*:
//!
//! - **Direction** — [`ApproxDirection`]: `over` (the answer may report edges/paths
//!   that cannot occur — it traversed an over-approximated candidate set),
//!   `under` (it may have missed edges/paths — the searched frontier touched a
//!   resolution cut, a confidence floor, or a traversal bound), `over_under`
//!   (both), or `exact` (the traversed subgraph is fully `certain` and
//!   cut-marker-free, and no bound was hit).
//! - **Reasons** — machine-readable [`ApproxReason`]s, each tagged with the
//!   direction it pushes and a stable `code`, derived from what the resolution +
//!   walk *actually did*, never from a static per-language table.
//! - **Scope** — for a *negative* answer (`no path`, no callers, a `unused`/dead
//!   list), a [`NegativeScope`] states what was searched (edge kinds, confidence
//!   floor, depth bound) so a consumer can gate on the negative *with* its scope.
//!
//! ## Cost
//!
//! The **over** signal is read straight off the result records (the weakest
//! confidence and candidate grouping the engine already surfaced) — no extra
//! graph work. The **under** signal needs to know what the searched frontier
//! looked like; for that a single bounded pass over the touched subgraph is run
//! ([`scan_frontier`]) — the same "scope the doctor to the subgraph this query
//! touches" pass A4 calls for, and only the touched region, never the whole index.
//! A *positive* reachability answer is proven by its witness and takes no scan.

use std::collections::BTreeMap;

use cgx_core::{Confidence, CutMarker, EdgeKind};
use serde::Serialize;

use crate::filter::{ConditionFilter, Direction, EdgeFilter};
use crate::result::{NeighborResult, PathSet, ReachResult, TruncationReason};
use crate::view::GraphView;
use crate::walk::PathWalker;
use cgx_core::NodeId;

/// The direction an answer can be wrong (GM-5 assertion-honesty). The fold of the
/// per-reason directions: no reasons ⇒ [`Exact`](ApproxDirection::Exact).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApproxDirection {
    /// The traversed subgraph is fully `certain` and cut-marker-free and no
    /// traversal bound was hit: the answer admits no *known* source of error.
    Exact,
    /// The answer may include edges/paths that cannot actually occur (false
    /// positives): it was derived through an over-approximated candidate set.
    Over,
    /// The answer may have missed edges/paths (false negatives): the searched
    /// frontier touched a resolution cut, a confidence floor, or a traversal bound.
    Under,
    /// Both risks apply.
    OverUnder,
}

/// Which way a single [`ApproxReason`] pushes the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonDirection {
    Over,
    Under,
}

/// One machine-readable reason contributing to the [`ApproxDirection`]. `code` is
/// a stable kebab-case token a CI consumer can match on; `detail` is a compact
/// human phrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApproxReason {
    pub direction: ReasonDirection,
    pub code: &'static str,
    pub detail: String,
}

/// The scope of a *negative* answer (A4): what the search covered, so a consumer
/// can gate on the negative claim instead of trusting a bare "no". The
/// invalidators ("what could make this negative wrong") are the `under`-direction
/// entries in the contract's `reasons`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NegativeScope {
    /// The edge kinds the search followed. The call family is spelled out so the
    /// claim is self-describing.
    pub searched_edge_kinds: Vec<EdgeKind>,
    /// The minimum edge confidence the search required (edges below this were not
    /// followed). `possible` means "every edge".
    pub confidence_floor: Confidence,
    /// The maximum hop depth searched; `null` = unbounded.
    pub max_depth: Option<u32>,
}

/// The standing modeling boundary every **call-graph** contract is scoped to (an
/// answer derived from something other than the call graph states its own — see
/// [`for_history`]). `exact` is always
/// relative to this modeled graph, never an absolute claim about the program:
/// blind spots that leave no per-answer fact (undescended closure/lambda bodies
/// deferred by all five adapters, external callees pre-SCIP whose dangling refs
/// fall outside the searched region) are carved out here so a bare unqualified
/// `exact` is impossible. Per-answer facts (cut markers, dangling-ref counts,
/// bounds) *narrow* the claim further via `reasons[]`.
pub const MODELED_GRAPH: &str = "descended function bodies in the indexed repository; \
external/unindexed callees, undescended closure bodies, and unexpanded macros are \
outside the modeled graph";

/// The per-answer approximation contract attached to every query answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApproximationContract {
    pub direction: ApproxDirection,
    pub reasons: Vec<ApproxReason>,
    /// The modeling boundary the whole contract (including `exact`) is relative
    /// to. A property of *this answer*, not of the binary: every call-graph
    /// answer carries [`MODELED_GRAPH`], while an answer derived from git
    /// history supplies its own boundary via [`for_history`]. The field name is
    /// historical — it is the answer's modeled domain, which is the call graph
    /// only for the call-graph builders.
    pub modeled_graph: &'static str,
    /// Present only for a *negative*/absence answer (A4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<NegativeScope>,
}

impl ApproximationContract {
    /// An `exact`, reason-free contract (no known source of error *within the
    /// modeled graph* — the standing [`MODELED_GRAPH`] carve-out still applies).
    pub fn exact() -> Self {
        ApproximationContract {
            direction: ApproxDirection::Exact,
            reasons: Vec::new(),
            modeled_graph: MODELED_GRAPH,
            scope: None,
        }
    }

    fn assemble(reasons: Vec<ApproxReason>, scope: Option<NegativeScope>) -> Self {
        Self::assemble_with(reasons, scope, MODELED_GRAPH)
    }

    /// The one direction fold in the codebase, with the modeling boundary as a
    /// parameter so an answer derived from something other than the call graph
    /// (see [`for_history`]) can state its own boundary without copying the fold.
    fn assemble_with(
        reasons: Vec<ApproxReason>,
        scope: Option<NegativeScope>,
        modeled_graph: &'static str,
    ) -> Self {
        let over = reasons.iter().any(|r| r.direction == ReasonDirection::Over);
        let under = reasons
            .iter()
            .any(|r| r.direction == ReasonDirection::Under);
        let direction = match (over, under) {
            (false, false) => ApproxDirection::Exact,
            (true, false) => ApproxDirection::Over,
            (false, true) => ApproxDirection::Under,
            (true, true) => ApproxDirection::OverUnder,
        };
        ApproximationContract {
            direction,
            reasons,
            modeled_graph,
            scope,
        }
    }

    /// The compact single-line human rendering (the CLI contract line). Never a
    /// wall: direction, the reason phrases joined by `; `, and (for a negative) a
    /// terse scope tail.
    pub fn human_summary(&self) -> String {
        // `exact` is always qualified: it claims nothing beyond the modeled
        // graph (the full statement rides in the JSON `modeled_graph` field).
        let dir = match self.direction {
            ApproxDirection::Exact => "exact (within modeled graph)",
            ApproxDirection::Over => "over-approximate",
            ApproxDirection::Under => "under-approximate",
            ApproxDirection::OverUnder => "over- and under-approximate",
        };
        let mut s = format!("approximation: {dir}");
        if !self.reasons.is_empty() {
            let joined = self
                .reasons
                .iter()
                .map(|r| r.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            s.push_str(" — ");
            s.push_str(&joined);
        }
        if let Some(scope) = &self.scope {
            s.push_str(&format!(
                " | scope: {} edges, confidence>={}, depth{}",
                scope.family_label(),
                confidence_token(scope.confidence_floor),
                match scope.max_depth {
                    Some(d) => format!("<={d}"),
                    None => "=unbounded".to_string(),
                }
            ));
        }
        s
    }
}

impl NegativeScope {
    /// A compact family label for the human line: the default call family renders
    /// as `call`, a pure `DerivesFrom` scope as `data-flow`, else the joined tokens.
    fn family_label(&self) -> String {
        if self.searched_edge_kinds.iter().all(|k| k.is_call()) {
            "call".to_string()
        } else if self.searched_edge_kinds == [EdgeKind::DerivesFrom] {
            "data-flow".to_string()
        } else {
            self.searched_edge_kinds
                .iter()
                .map(edge_kind_token)
                .collect::<Vec<_>>()
                .join(",")
        }
    }
}

// --- reason construction ----------------------------------------------------

fn over_candidate_reason() -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Over,
        code: "over-approx-candidate-set",
        detail: "resolved through an over-approximated candidate set (dynamic dispatch or \
                 name-collision); some reported edges may not occur"
            .to_string(),
    }
}

fn cut_reason(marker: CutMarker, count: usize) -> ApproxReason {
    let (code, phrase) = match marker {
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
        CutMarker::External => (
            "external-receiver",
            "call on an out-of-repo receiver type not modeled",
        ),
        CutMarker::UntypedReceiver => (
            "untyped-receiver",
            "call on a receiver of unknown type not modeled",
        ),
    };
    ApproxReason {
        direction: ReasonDirection::Under,
        code,
        detail: format!("{phrase} ({count} site(s) on the searched frontier)"),
    }
}

fn below_floor_reason(floor: Confidence, count: usize) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "below-confidence-floor",
        detail: format!(
            "{count} edge(s) below the confidence>={} floor were excluded from the search",
            confidence_token(floor)
        ),
    }
}

/// The `--max-candidates` fan-out cap dropped edges from the searched frontier:
/// an *under*-completeness fact (the answer may now miss a real caller/callee that
/// lived in a large over-approximated candidate set). Counts only edges that would
/// otherwise have passed the confidence floor — the below-floor exclusion is
/// reported separately, so the two counts partition the excluded edges.
fn dropped_max_candidates_reason(max: u32, count: usize) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "dropped-max-candidates",
        detail: format!(
            "{count} edge(s) in a candidate set larger than {max} were excluded from the search \
             (--max-candidates fan-out cap)"
        ),
    }
}

fn depth_limit_reason(depth: u32) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "depth-limit",
        detail: format!("search stopped at depth {depth}; deeper edges were not explored"),
    }
}

/// Resolver Step-5 dangling refs on touched nodes: calls that resolved to **no**
/// target and therefore left no edge the walk could follow (the common external/
/// unindexed-callee case pre-SCIP). Read from `NodeRecord.unresolved_calls`.
fn dangling_refs_reason(count: u64) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "unresolved-external-calls",
        detail: format!(
            "{count} call(s) in the searched region resolved to no in-repo target \
             (external/unindexed callee; no SCIP) and could not be followed"
        ),
    }
}

/// Calls on receivers proven to be of out-of-repo types: no edge, so the walk
/// could not follow them. Read from `NodeRecord.external_calls`.
fn external_receiver_calls_reason(count: u64) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "external-receiver-calls",
        detail: format!(
            "{count} call(s) on receivers of out-of-repo types (builtin, stdlib or library) \
             have no in-repo target and could not be followed"
        ),
    }
}

/// Virtual call sites in the walked region whose targets receiver typing
/// narrowed. Read from `NarrowingCounts::typed_out`.
fn receiver_narrowed_reason(count: u64) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "receiver-narrowed",
        detail: format!(
            "{count} virtual call site(s) were bound by receiver typing (self/cls/super, class \
             heads, annotations, constructors, fields); targets outside the inferred type are \
             excluded and are reachable only if a typing assumption fails (monkey-patching or \
             methods assigned from outside the class family, untruthful annotations, \
             attributes set dynamically, a metaclass that makes a constructor return another \
             class)"
        ),
    }
}

/// As [`receiver_narrowed_reason`], for typing that used interprocedural facts.
/// Read from `NarrowingCounts::interproc_out`.
fn receiver_narrowed_interproc_reason(count: u64) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "receiver-narrowed-interproc",
        detail: format!(
            "{count} site(s) were bound using argument/return/fixture typing, which assumes \
             every caller of the typed function is in the indexed repo"
        ),
    }
}

/// Untyped-receiver sites the import-visibility rule bounded (`bounded`) or left
/// with no target (`dropped`). Read from `NarrowingCounts::visible_out` and
/// `visible_dropped_out`.
fn residual_import_visible_reason(bounded: u64, dropped: u64) -> ApproxReason {
    let total = bounded + dropped;
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "residual-import-visible",
        detail: format!(
            "{total} untyped-receiver call site(s) were limited to methods of classes the \
             caller's file defines or imports, plus their subclasses ({dropped} left with no \
             target); this rule is not sound for duck typing: an object can reach a module \
             that never imports its class"
        ),
    }
}

/// Same-name call sites elsewhere that receiver typing bound to targets other
/// than the searched symbols. Read from `NarrowingCounts::typed_away_in`.
fn receiver_narrowed_away_reason(count: u64) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "receiver-narrowed-away",
        detail: format!(
            "{count} same-name virtual call site(s) were bound by receiver typing to other \
             targets; they could reach the searched symbols only if a typing assumption fails"
        ),
    }
}

/// As [`receiver_narrowed_away_reason`], for interprocedural typing. Read from
/// `NarrowingCounts::interproc_away_in`.
fn receiver_narrowed_away_interproc_reason(count: u64) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "receiver-narrowed-away-interproc",
        detail: format!(
            "{count} same-name virtual call site(s) were bound by argument/return/fixture typing \
             to other targets; they could reach the searched symbols only if a typing assumption \
             fails or a caller of the typed function is outside the indexed repo"
        ),
    }
}

/// Same-name untyped-receiver sites elsewhere whose import-visibility bound
/// excluded the searched symbols. Read from `NarrowingCounts::visible_away_in`.
fn residual_narrowed_away_reason(count: u64) -> ApproxReason {
    ApproxReason {
        direction: ReasonDirection::Under,
        code: "residual-narrowed-away",
        detail: format!(
            "{count} same-name untyped-receiver call site(s) excluded the searched symbols \
             because their classes are not import-visible to the caller; not sound for duck \
             typing"
        ),
    }
}

fn truncation_reason(reason: TruncationReason) -> ApproxReason {
    let (code, detail) = match reason {
        TruncationReason::StepBudget => (
            "truncated-step-budget",
            "path enumeration hit the work budget; more paths may exist",
        ),
        TruncationReason::PathCap => (
            "truncated-path-cap",
            "path enumeration hit the result cap; more paths may exist",
        ),
    };
    ApproxReason {
        direction: ReasonDirection::Under,
        code,
        detail: detail.to_string(),
    }
}

// --- frontier scan (the A4 "scope the doctor to this subgraph" pass) ---------

/// The incompleteness facts gathered by scanning the subgraph a walk touched.
#[derive(Debug, Default)]
struct FrontierFacts {
    /// Total resolver Step-5 dangling refs (`NodeRecord.unresolved_calls`) on
    /// touched nodes — calls that left NO edge and could not be followed. The
    /// walk-invisible blind spot; without this a negative over an external call
    /// would be claimed clean.
    dangling_refs: u64,
    /// Forward call-scope walks only: calls on out-of-repo receivers
    /// (`NodeRecord.external_calls`), which also left no edge.
    external_calls: u64,
    /// Forward call-scope walks only: receiver-narrowing counts for sites in the
    /// touched bodies (`NarrowingCounts::*_out`).
    typed_out: u64,
    interproc_out: u64,
    visible_out: u64,
    visible_dropped_out: u64,
    /// Backward call-scope walks only: same-name sites elsewhere whose narrowed
    /// target set excluded a touched node (`NarrowingCounts::*_away_in`). Those
    /// sites have no edge into the touched region, so the walk cannot see them.
    typed_away_in: u64,
    interproc_away_in: u64,
    visible_away_in: u64,
    /// Per-marker count of cut-marked edges incident (in the walk direction) to a
    /// reached node — the blind spots on the search frontier.
    cut_counts: BTreeMap<CutMarker, usize>,
    /// Edges of the searched family that sit below the confidence floor (excluded
    /// from the walk but a real, unfollowed source of edges).
    below_floor: usize,
    /// Edges of the searched family that pass the confidence floor but were dropped
    /// by the `--max-candidates` fan-out cap (their candidate group exceeds the
    /// limit) — the completeness cost of the fan-out dial.
    dropped_max_candidates: usize,
    /// A reached node at the depth horizon had an admitted neighbor beyond it.
    depth_truncated: bool,
}

impl FrontierFacts {
    fn into_reasons(self, walker: &PathWalker) -> Vec<ApproxReason> {
        let mut reasons = Vec::new();
        if self.dangling_refs > 0 {
            reasons.push(dangling_refs_reason(self.dangling_refs));
        }
        if self.external_calls > 0 {
            reasons.push(external_receiver_calls_reason(self.external_calls));
        }
        if self.typed_out > 0 {
            reasons.push(receiver_narrowed_reason(self.typed_out));
        }
        if self.interproc_out > 0 {
            reasons.push(receiver_narrowed_interproc_reason(self.interproc_out));
        }
        if self.visible_out + self.visible_dropped_out > 0 {
            reasons.push(residual_import_visible_reason(
                self.visible_out,
                self.visible_dropped_out,
            ));
        }
        if self.typed_away_in > 0 {
            reasons.push(receiver_narrowed_away_reason(self.typed_away_in));
        }
        if self.interproc_away_in > 0 {
            reasons.push(receiver_narrowed_away_interproc_reason(
                self.interproc_away_in,
            ));
        }
        if self.visible_away_in > 0 {
            reasons.push(residual_narrowed_away_reason(self.visible_away_in));
        }
        for (marker, count) in self.cut_counts {
            reasons.push(cut_reason(marker, count));
        }
        if self.below_floor > 0 {
            reasons.push(below_floor_reason(
                walker.filter.min_confidence,
                self.below_floor,
            ));
        }
        if self.dropped_max_candidates > 0 {
            if let Some(max) = walker.filter.max_candidates {
                reasons.push(dropped_max_candidates_reason(
                    max,
                    self.dropped_max_candidates,
                ));
            }
        }
        if self.depth_truncated {
            if let Some(d) = walker.max_depth {
                reasons.push(depth_limit_reason(d));
            }
        }
        reasons
    }
}

/// Scan the subgraph reached from `roots` in direction `dir` under `walker`,
/// collecting the incompleteness facts on its frontier: dangling-ref and
/// receiver-narrowing counts on the touched nodes (calls that left no edge or
/// whose targets were narrowed), cut-marked and below-floor
/// incident edges, and depth-horizon truncation. Edge/node work is O(touched);
/// the visited/depth arrays are O(total nodes), the same allocation profile as
/// every walk in this crate. Deterministic (touched list in BFS order; never
/// iterates a hash map).
fn scan_frontier(
    view: &GraphView,
    walker: &PathWalker,
    roots: &[NodeId],
    dir: Direction,
) -> FrontierFacts {
    let n = view.node_count();
    let mut reached = vec![false; n];
    let mut depth_of = vec![u32::MAX; n];
    let mut touched: Vec<NodeId> = Vec::new();
    for &r in roots {
        if let Some(i) = idx(r, n) {
            if !reached[i] {
                reached[i] = true;
                depth_of[i] = 0;
                touched.push(r);
            }
        }
    }
    for &r in roots {
        for d in walker.bfs(view, r, dir) {
            if let Some(i) = idx(d.node, n) {
                if !reached[i] {
                    reached[i] = true;
                    touched.push(d.node);
                }
                depth_of[i] = depth_of[i].min(d.depth);
            }
        }
    }

    // A kind-only filter: every edge of the searched family regardless of the
    // confidence floor, condition restriction, or fan-out cap, so cuts,
    // below-floor, and max-candidates-dropped edges the *walk* skipped are still
    // seen as honest incompleteness sources.
    let scan_filter = EdgeFilter {
        min_confidence: Confidence::Possible,
        condition: ConditionFilter::Any,
        kinds: walker.filter.kinds.clone(),
        max_candidates: None,
    };
    let floor = walker.filter.min_confidence;

    // Dangling refs are a fact about a node's *body* (its outgoing calls), so
    // they witness incompleteness only when the walk follows call-family edges
    // in the forward direction (callees/reaches/unused frontiers). A backward
    // (callers) walk or a DerivesFrom-scoped walk does not traverse the missing
    // out-edge.
    //
    // The scope test is "is this a call scope", not "did the caller leave the
    // scope at its default". Testing `kinds.is_none()` conflated the two and let
    // any explicit `kind` argument silence the fact on a walk following exactly
    // the edges the fact is about — reachable today through MCP `callees`, whose
    // `kind` array the CLI has no flag for. `NegativeScope::family_label` above
    // already asks this question the right way; ask it the same way here.
    let is_call_scope = walker
        .filter
        .kinds
        .as_ref()
        .is_none_or(|ks| ks.iter().all(|k| k.is_call()));
    let count_dangling = dir == Direction::Forward && is_call_scope;
    // The receiver-narrowing `*_away_in` counts are the backward mirror: sites
    // elsewhere that *would* have had an edge into a touched node before
    // narrowing excluded it. Only a callers-direction call-scope walk can have
    // lost them.
    let count_backward = dir == Direction::Backward && is_call_scope;

    let mut facts = FrontierFacts::default();
    for &node in &touched {
        let node_ix = node.index();
        let record = view.node(node);
        // A narrowed site on a node at the depth horizon could only lose a
        // neighbor one hop *beyond* the bound, which the answer never claimed
        // to cover, so the narrowing counters skip horizon nodes.
        let inside_horizon = walker.max_depth != Some(depth_of[node_ix]);
        if count_dangling {
            facts.dangling_refs += u64::from(record.unresolved_calls);
        }
        if count_dangling && inside_horizon {
            facts.external_calls += u64::from(record.external_calls);
            facts.typed_out += u64::from(record.narrowing.typed_out);
            facts.interproc_out += u64::from(record.narrowing.interproc_out);
            facts.visible_out += u64::from(record.narrowing.visible_out);
            facts.visible_dropped_out += u64::from(record.narrowing.visible_dropped_out);
        }
        if count_backward && inside_horizon {
            facts.typed_away_in += u64::from(record.narrowing.typed_away_in);
            facts.interproc_away_in += u64::from(record.narrowing.interproc_away_in);
            facts.visible_away_in += u64::from(record.narrowing.visible_away_in);
        }
        for er in view.neighbors(node, dir, &scan_filter) {
            for marker in er.edge.cut_markers.iter() {
                *facts.cut_counts.entry(marker).or_default() += 1;
            }
            if er.edge.confidence < floor {
                facts.below_floor += 1;
            } else if let (Some(max), Some(group)) =
                (walker.filter.max_candidates, er.edge.candidate_group)
            {
                // Passes the confidence floor but the fan-out cap dropped it: the
                // completeness cost of `--max-candidates`. The `else` keeps this
                // disjoint from `below_floor` so an excluded edge is attributed to
                // exactly one reason.
                if view.candidate_group_size(group) > max {
                    facts.dropped_max_candidates += 1;
                }
            }
        }
        if walker.max_depth == Some(depth_of[node_ix]) {
            let horizon = view
                .neighbors(node, dir, &walker.filter)
                .any(|er| idx(er.peer, n).is_some_and(|i| !reached[i]));
            if horizon {
                facts.depth_truncated = true;
            }
        }
    }
    facts
}

fn idx(node: NodeId, n: usize) -> Option<usize> {
    let i = node.index();
    (i < n).then_some(i)
}

fn scope_of(walker: &PathWalker, _dir: Direction) -> NegativeScope {
    NegativeScope {
        searched_edge_kinds: searched_kinds(&walker.filter),
        confidence_floor: walker.filter.min_confidence,
        max_depth: walker.max_depth,
    }
}

/// The concrete edge kinds a filter follows: its explicit `kinds`, or the call
/// family when it defaults to the call-graph scope.
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

// --- public builders (one per query answer shape) ---------------------------

/// Contract for a neighbor answer (`callers`/`callees`/`reaches <from>`/`flows-*`).
/// `root` is the query anchor, `dir` the walk direction. Over iff any result was
/// reached over an over-approximated candidate set; under from the frontier scan;
/// scope attached when the answer is empty (a negative "no callers/callees").
pub fn for_neighbors(
    view: &GraphView,
    walker: &PathWalker,
    root: NodeId,
    dir: Direction,
    results: &[NeighborResult],
) -> ApproximationContract {
    let mut reasons = Vec::new();
    if results.iter().any(|r| {
        r.min_confidence_on_path == Confidence::Possible || r.via.candidate_group.is_some()
    }) {
        reasons.push(over_candidate_reason());
    }
    reasons.extend(scan_frontier(view, walker, &[root], dir).into_reasons(walker));
    let scope = results.is_empty().then(|| scope_of(walker, dir));
    ApproximationContract::assemble(reasons, scope)
}

/// Contract for a `reaches <from> <to>` answer. A positive answer is proven by its
/// witness — only `over` can apply (the witness path may traverse an
/// over-approximated edge) and no frontier scan is needed. A negative answer
/// (`reachable == false`) scans the forward frontier from `from` and attaches the
/// negative scope.
pub fn for_reaches(
    view: &GraphView,
    walker: &PathWalker,
    from: NodeId,
    result: &ReachResult,
) -> ApproximationContract {
    let mut reasons = Vec::new();
    match &result.witness {
        Some(witness) => {
            let over = witness.min_confidence == Confidence::Possible
                || witness
                    .steps
                    .iter()
                    .any(|s| s.via.as_ref().is_some_and(|e| e.candidate_group.is_some()));
            if over {
                reasons.push(over_candidate_reason());
            }
            ApproximationContract::assemble(reasons, None)
        }
        None => {
            reasons.extend(
                scan_frontier(view, walker, &[from], Direction::Forward).into_reasons(walker),
            );
            ApproximationContract::assemble(reasons, Some(scope_of(walker, Direction::Forward)))
        }
    }
}

/// The reasons intrinsic to a [`PathSet`] alone, independent of any anchor or
/// frontier: `over` from an over-approximated path, `under` from an honest
/// truncation marker. Shared by [`for_paths`] and [`for_path_set`].
fn path_set_reasons(result: &PathSet) -> Vec<ApproxReason> {
    let mut reasons = Vec::new();
    if result.paths.iter().any(|p| {
        p.min_confidence == Confidence::Possible
            || p.steps
                .iter()
                .any(|s| s.via.as_ref().is_some_and(|e| e.candidate_group.is_some()))
    }) {
        reasons.push(over_candidate_reason());
    }
    if let Some(reason) = result.truncation {
        reasons.push(truncation_reason(reason));
    }
    reasons
}

/// Contract for a `paths <from> <to>` answer. Over iff any enumerated path was
/// found over an over-approximated candidate set; under from the honest
/// truncation marker and — when *no* path was found — the forward frontier scope.
pub fn for_paths(
    view: &GraphView,
    walker: &PathWalker,
    from: NodeId,
    result: &PathSet,
) -> ApproximationContract {
    let mut reasons = path_set_reasons(result);
    let scope = if result.is_empty() {
        reasons
            .extend(scan_frontier(view, walker, &[from], Direction::Forward).into_reasons(walker));
        Some(scope_of(walker, Direction::Forward))
    } else {
        None
    };
    ApproximationContract::assemble(reasons, scope)
}

/// Contract for a path-set with no single anchor to scan a frontier from (a CQL
/// `RETURN path` query): the intrinsic over/truncation reasons only. A CQL query
/// carries its scope intrinsically, so no negative-scope envelope is attached.
pub fn for_path_set(result: &PathSet) -> ApproximationContract {
    ApproximationContract::assemble(path_set_reasons(result), None)
}

/// The minimal contract for an answer whose only cheaply-derivable honesty signal
/// is whether it was computed over an over-approximated candidate set (a CQL
/// table result, whose bound edges carry confidence but expose no walkable
/// frontier). `over` ⇒ [`ApproxDirection::Over`], else [`ApproxDirection::Exact`].
pub fn over_only(over: bool) -> ApproximationContract {
    let reasons = if over {
        vec![over_candidate_reason()]
    } else {
        Vec::new()
    };
    ApproximationContract::assemble(reasons, None)
}

/// Contract for a `unused`/dead answer — an inherently *negative* claim ("these
/// symbols are not reached from the entrypoints"). Under iff the used-set frontier
/// (forward from `roots`) touches a resolution cut or below-floor edge, since such
/// a blind spot could hide a real use and make the dead list over-inclusive. Scope
/// is always attached. `roots` is the resolved entrypoint set (see
/// [`crate::engine::entrypoint_roots`]).
pub fn for_unused(
    view: &GraphView,
    walker: &PathWalker,
    roots: &[NodeId],
) -> ApproximationContract {
    let reasons = scan_frontier(view, walker, roots, Direction::Forward).into_reasons(walker);
    ApproximationContract::assemble(reasons, Some(scope_of(walker, Direction::Forward)))
}

/// The contract for an answer derived from **git history** rather than from the
/// indexed call graph. The modeling boundary is supplied by the caller because it
/// is a property of the answer, not of the binary — a commit-walk answer is scoped
/// to a rev range and models no call edges at all.
///
/// No [`NegativeScope`] is attached: its fields (searched edge kinds, confidence
/// floor, depth) are call-graph concepts and would be a lie on a history answer.
pub fn for_history(
    reasons: Vec<ApproxReason>,
    modeled_history: &'static str,
) -> ApproximationContract {
    ApproximationContract::assemble_with(reasons, None, modeled_history)
}

// --- tokens -----------------------------------------------------------------

fn confidence_token(c: Confidence) -> &'static str {
    match c {
        Confidence::Possible => "possible",
        Confidence::Probable => "probable",
        Confidence::Certain => "certain",
    }
}

fn edge_kind_token(k: &EdgeKind) -> &'static str {
    match k {
        EdgeKind::Calls => "calls",
        EdgeKind::CallsVirtual => "calls-virtual",
        EdgeKind::CallsClosure => "calls-closure",
        EdgeKind::CallsCallback => "calls-callback",
        EdgeKind::CallsAsync => "calls-async",
        EdgeKind::CallsIndirect => "calls-indirect",
        EdgeKind::Spawns => "spawns",
        EdgeKind::DerivesFrom => "derives-from",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Direction;
    use crate::walk::PathWalker;
    use crate::{callees, paths as query_paths, reaches, unused, GraphView};
    use cgx_core::edge::EdgeRecord;
    use cgx_core::{
        Candidate, CutMarker, CutMarkers, EdgeCondition, EdgeId, EntrypointKind, NodeId,
        NodeRecord, SymbolKind, Tier, Visibility,
    };

    fn node(id: u32, fqn: &str, line: u32, entry: Option<EntrypointKind>) -> NodeRecord {
        NodeRecord {
            id: NodeId(id),
            kind: SymbolKind::Function,
            fqn: fqn.to_string(),
            file: "src/lib.rs".to_string(),
            line_start: line,
            line_end: line,
            lang: "rust".to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: entry,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
            unresolved_calls: 0,
            external_calls: 0,
            narrowing: Default::default(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn edge(
        id: u32,
        src: u32,
        dst: u32,
        conf: Confidence,
        cuts: &[CutMarker],
        candidate_group: Option<u32>,
    ) -> EdgeRecord {
        EdgeRecord {
            id: EdgeId(id),
            src: NodeId(src),
            dst: NodeId(dst),
            kind: EdgeKind::Calls,
            condition: EdgeCondition::Always,
            confidence: conf,
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
        }
    }

    fn walker(max_depth: Option<u32>, floor: Confidence) -> PathWalker {
        PathWalker {
            filter: EdgeFilter::calls().with_min_confidence(floor),
            max_depth,
            max_paths: None,
            max_steps: None,
        }
    }

    // a -> b (certain), b -> c (certain): fully exact call chain.
    fn exact_view() -> GraphView {
        let nodes = vec![
            node(0, "a", 10, Some(EntrypointKind::Main)),
            node(1, "b", 20, None),
            node(2, "c", 30, None),
        ];
        let edges = vec![
            edge(0, 0, 1, Confidence::Certain, &[], None),
            edge(1, 1, 2, Confidence::Certain, &[], None),
        ];
        GraphView::new(nodes, edges, Vec::<Candidate>::new())
    }

    #[test]
    fn positive_all_certain_is_exact() {
        let view = exact_view();
        let w = walker(None, Confidence::Possible);
        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        assert_eq!(c.direction, ApproxDirection::Exact);
        assert!(c.reasons.is_empty());
        assert!(c.scope.is_none());
    }

    #[test]
    fn possible_edge_makes_positive_over() {
        // a -> b over a possible candidate-set edge.
        let nodes = vec![node(0, "a", 10, None), node(1, "b", 20, None)];
        let edges = vec![edge(0, 0, 1, Confidence::Possible, &[], Some(7))];
        let view = GraphView::new(nodes, edges, Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);
        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        assert_eq!(c.direction, ApproxDirection::Over);
        assert!(c
            .reasons
            .iter()
            .any(|r| r.code == "over-approx-candidate-set"));
    }

    #[test]
    fn cut_on_frontier_makes_under_with_reason_code() {
        // a -> b (certain), b -> c via an unresolved (dynamic) cut edge.
        let nodes = vec![
            node(0, "a", 10, None),
            node(1, "b", 20, None),
            node(2, "c", 30, None),
        ];
        let edges = vec![
            edge(0, 0, 1, Confidence::Certain, &[], None),
            edge(1, 1, 2, Confidence::Certain, &[CutMarker::Dynamic], None),
        ];
        let view = GraphView::new(nodes, edges, Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);
        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        assert_eq!(c.direction, ApproxDirection::Under);
        assert!(c.reasons.iter().any(|r| r.code == "dynamic-dispatch"));
    }

    #[test]
    fn dangling_external_call_blocks_false_exact_on_negative_reaches() {
        // `a` calls only an external symbol: the resolver's Step-5 leaves NO edge
        // and stamps `unresolved_calls` on `a` instead. A negative `reaches a b`
        // must NOT be claimed bare-exact — the search could not follow the call
        // out of the modeled graph.
        let mut a = node(0, "a", 10, None);
        a.unresolved_calls = 1;
        let nodes = vec![a, node(1, "b", 20, None)];
        let view = GraphView::new(nodes, Vec::new(), Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);
        let result = reaches(&view, NodeId(0), NodeId(1), &w);
        assert!(!result.reachable);
        let c = for_reaches(&view, &w, NodeId(0), &result);
        assert_eq!(c.direction, ApproxDirection::Under);
        assert!(
            c.reasons
                .iter()
                .any(|r| r.code == "unresolved-external-calls"),
            "dangling refs on the touched frontier must surface: {c:?}"
        );
        assert!(c.scope.is_some());
    }

    /// The dangling-ref fact must survive an explicit **call-family** kind
    /// filter. `kinds.is_none()` conflated "default scope" with "call scope", so
    /// any `kind` argument silenced the fact — on a forward walk following
    /// exactly the edges the fact is about. Only a non-call scope should silence
    /// it, and `DerivesFrom` is the one that reaches this path in production
    /// (`flows-to`/`flows-from`).
    #[test]
    fn kind_filter_silences_the_dangling_fact_only_outside_the_call_family() {
        // (kinds the walk is scoped to, must the dangling fact still surface?)
        let cases: &[(Option<&[EdgeKind]>, bool)] = &[
            (None, true),
            (Some(&[EdgeKind::Calls]), true),
            (Some(&[EdgeKind::Calls, EdgeKind::CallsVirtual]), true),
            (Some(&[EdgeKind::CallsIndirect]), true),
            (Some(&[EdgeKind::Spawns]), true),
            (Some(&[EdgeKind::DerivesFrom]), false),
            (Some(&[EdgeKind::Calls, EdgeKind::DerivesFrom]), false),
        ];

        // `a` -> `b`, and `a`'s body also holds 2 calls that resolved to nothing.
        let mut a = node(0, "a", 10, None);
        a.unresolved_calls = 2;
        let nodes = vec![a, node(1, "b", 20, None)];
        let edges = vec![edge(0, 0, 1, Confidence::Certain, &[], None)];
        let view = GraphView::new(nodes, edges, Vec::<Candidate>::new());

        for (kinds, expect_reason) in cases {
            let mut filter = EdgeFilter::default();
            if let Some(ks) = kinds {
                filter = filter.with_kinds(ks.to_vec());
            }
            let w = PathWalker {
                filter,
                max_depth: None,
                max_paths: None,
                max_steps: None,
            };
            let results = callees(&view, NodeId(0), &w);
            let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
            let has = c
                .reasons
                .iter()
                .any(|r| r.code == "unresolved-external-calls");
            assert_eq!(
                has, *expect_reason,
                "kinds {kinds:?}: expected dangling reason present={expect_reason}, got {c:?}"
            );
        }
    }

    #[test]
    fn negative_reaches_carries_scope_and_frontier_caveat() {
        // a -> b (certain) with an unresolved cut on b; c is unreachable from a.
        let nodes = vec![
            node(0, "a", 10, None),
            node(1, "b", 20, None),
            node(2, "c", 30, None),
        ];
        let edges = vec![
            edge(0, 0, 1, Confidence::Certain, &[], None),
            edge(1, 1, 1, Confidence::Certain, &[CutMarker::Unresolved], None),
        ];
        let view = GraphView::new(nodes, edges, Vec::<Candidate>::new());
        let w = walker(Some(6), Confidence::Possible);
        let result = reaches(&view, NodeId(0), NodeId(2), &w);
        assert!(!result.reachable);
        let c = for_reaches(&view, &w, NodeId(0), &result);
        assert_eq!(c.direction, ApproxDirection::Under);
        let scope = c.scope.expect("negative answer carries scope");
        assert_eq!(scope.confidence_floor, Confidence::Possible);
        assert_eq!(scope.max_depth, Some(6));
        assert!(scope.searched_edge_kinds.contains(&EdgeKind::Calls));
        assert!(c.reasons.iter().any(|r| r.code == "unresolved-call"));
    }

    #[test]
    fn positive_reaches_is_exact_no_scan() {
        let view = exact_view();
        let w = walker(None, Confidence::Possible);
        let result = reaches(&view, NodeId(0), NodeId(2), &w);
        assert!(result.reachable);
        let c = for_reaches(&view, &w, NodeId(0), &result);
        assert_eq!(c.direction, ApproxDirection::Exact);
        assert!(c.scope.is_none());
    }

    #[test]
    fn below_floor_edge_is_under() {
        // a -> b possible; with a probable floor the walk skips it, but the
        // contract still reports the excluded edge as an under-completeness caveat.
        let nodes = vec![node(0, "a", 10, None), node(1, "b", 20, None)];
        let edges = vec![edge(0, 0, 1, Confidence::Possible, &[], None)];
        let view = GraphView::new(nodes, edges, Vec::<Candidate>::new());
        let w = walker(None, Confidence::Probable);
        let results = callees(&view, NodeId(0), &w);
        assert!(results.is_empty());
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        assert_eq!(c.direction, ApproxDirection::Under);
        assert!(c.reasons.iter().any(|r| r.code == "below-confidence-floor"));
        assert!(c.scope.is_some(), "empty neighbor answer is negative");
    }

    // `a` over-approximates one call site to a 3-way candidate set {b,c,d} plus a
    // single resolved call to `e`. The candidate table (not the edge count) is the
    // source of truth for the group's fan-out.
    fn fanout_view() -> GraphView {
        let nodes = vec![
            node(0, "a", 10, None),
            node(1, "b", 20, None),
            node(2, "c", 30, None),
            node(3, "d", 40, None),
            node(4, "e", 50, None),
        ];
        let edges = vec![
            edge(0, 0, 1, Confidence::Possible, &[], Some(0)),
            edge(1, 0, 2, Confidence::Possible, &[], Some(0)),
            edge(2, 0, 3, Confidence::Possible, &[], Some(0)),
            edge(3, 0, 4, Confidence::Certain, &[], None),
        ];
        let candidates = vec![
            Candidate {
                candidate_group: 0,
                dst: NodeId(1),
                rank: 0,
            },
            Candidate {
                candidate_group: 0,
                dst: NodeId(2),
                rank: 1,
            },
            Candidate {
                candidate_group: 0,
                dst: NodeId(3),
                rank: 2,
            },
        ];
        GraphView::new(nodes, edges, candidates)
    }

    fn fanout_walker(max_candidates: Option<u32>) -> PathWalker {
        let mut filter = EdgeFilter::calls();
        if let Some(n) = max_candidates {
            filter = filter.with_max_candidates(n);
        }
        PathWalker {
            filter,
            max_depth: None,
            max_paths: None,
            max_steps: None,
        }
    }

    #[test]
    fn max_candidates_drops_large_group_and_reports_exclusion() {
        // Fan-out cap of 2 drops the 3-way candidate set (b,c,d); only the singly
        // resolved `e` survives, and the contract states what it excluded.
        let view = fanout_view();
        let w = fanout_walker(Some(2));
        let results = callees(&view, NodeId(0), &w);
        let names: Vec<&str> = results.iter().map(|r| r.node.fqn.as_str()).collect();
        assert_eq!(
            names,
            vec!["e"],
            "the 3-way set is dropped, the singleton kept"
        );

        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        let reason = c
            .reasons
            .iter()
            .find(|r| r.code == "dropped-max-candidates")
            .expect("the excluded fan-out is reported");
        assert_eq!(reason.direction, ReasonDirection::Under);
        assert!(
            reason.detail.contains('3') && reason.detail.contains('2'),
            "reports 3 excluded edges over the cap of 2: {}",
            reason.detail
        );
        // Every surviving result is certain (no candidate group), so the answer is
        // not over; the only known error is the under from the dropped set.
        assert_eq!(c.direction, ApproxDirection::Under);
    }

    #[test]
    fn max_candidates_at_or_above_group_size_keeps_it() {
        // A cap of 3 admits the size-3 set: all four callees survive and the
        // surviving candidate edges make the answer over, with no dropped-set under.
        let view = fanout_view();
        let w = fanout_walker(Some(3));
        let results = callees(&view, NodeId(0), &w);
        assert_eq!(results.len(), 4, "size-3 set is within the cap of 3");
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        assert!(
            !c.reasons.iter().any(|r| r.code == "dropped-max-candidates"),
            "nothing excluded: {c:?}"
        );
        assert_eq!(c.direction, ApproxDirection::Over);
    }

    #[test]
    fn max_candidates_below_floor_edges_are_not_double_counted() {
        // An edge that is both below the confidence floor and over the fan-out cap
        // is attributed to the confidence exclusion only (the two counts partition
        // the excluded edges).
        let view = fanout_view();
        let filter = EdgeFilter::calls()
            .with_min_confidence(Confidence::Certain)
            .with_max_candidates(2);
        let w = PathWalker {
            filter,
            max_depth: None,
            max_paths: None,
            max_steps: None,
        };
        let results = callees(&view, NodeId(0), &w);
        // Only `e` (certain) passes the floor; the possible candidate set is below
        // it and never reaches the fan-out clause.
        assert_eq!(
            results
                .iter()
                .map(|r| r.node.fqn.as_str())
                .collect::<Vec<_>>(),
            vec!["e"]
        );
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        assert!(c.reasons.iter().any(|r| r.code == "below-confidence-floor"));
        assert!(
            !c.reasons.iter().any(|r| r.code == "dropped-max-candidates"),
            "below-floor edges are not also counted as fan-out drops: {c:?}"
        );
    }

    #[test]
    fn unused_is_negative_with_scope() {
        // entry `a` reaches `b`; `c` is dead. A dynamic cut on the used frontier
        // makes the dead list under (over-inclusive).
        let nodes = vec![
            node(0, "a", 10, Some(EntrypointKind::Main)),
            node(1, "b", 20, None),
            node(2, "c", 30, None),
        ];
        let edges = vec![edge(
            0,
            0,
            1,
            Confidence::Certain,
            &[CutMarker::Reflective],
            None,
        )];
        let view = GraphView::new(nodes, edges, Vec::<Candidate>::new());
        let w = PathWalker::default();
        let roots = crate::engine::entrypoint_roots(&view, &[]);
        let results = unused(&view, &[], &[], &w);
        assert!(results.iter().any(|n| n.fqn == "c"));
        let c = for_unused(&view, &w, &roots);
        assert!(c.scope.is_some());
        assert!(c.reasons.iter().any(|r| r.code == "reflective-dispatch"));
    }

    #[test]
    fn human_summary_is_one_compact_line() {
        let view = exact_view();
        let w = walker(None, Confidence::Possible);
        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        assert_eq!(
            c.human_summary(),
            "approximation: exact (within modeled graph)"
        );
        assert!(!c.human_summary().contains('\n'));
    }

    #[test]
    fn paths_truncation_is_under() {
        let view = exact_view();
        let w = PathWalker {
            filter: EdgeFilter::calls(),
            max_depth: None,
            max_paths: Some(1),
            max_steps: None,
        };
        // Force a path set with a synthetic truncation to exercise the mapping.
        let mut set = query_paths(&view, NodeId(0), NodeId(2), &w);
        set.truncation = Some(TruncationReason::PathCap);
        let c = for_paths(&view, &w, NodeId(0), &set);
        assert!(c.reasons.iter().any(|r| r.code == "truncated-path-cap"));
        assert_eq!(c.direction, ApproxDirection::Under);
    }

    const TEST_HISTORY: &str = "commits in the given rev range; nothing outside it is modeled";

    fn history_reason(direction: ReasonDirection, code: &'static str) -> ApproxReason {
        ApproxReason {
            direction,
            code,
            detail: "test".to_string(),
        }
    }

    #[test]
    fn for_history_carries_the_supplied_boundary_not_the_call_graph_one() {
        let c = for_history(
            vec![history_reason(ReasonDirection::Under, "bounded-rev-range")],
            TEST_HISTORY,
        );
        assert_eq!(c.modeled_graph, TEST_HISTORY);
        assert_ne!(c.modeled_graph, MODELED_GRAPH);
        // A history answer models no call edges, so it can carry no negative scope.
        assert!(c.scope.is_none());
    }

    #[test]
    fn for_history_folds_direction_exactly_like_the_call_graph_builders() {
        let over = history_reason(ReasonDirection::Over, "file-level-granularity");
        let under = history_reason(ReasonDirection::Under, "bounded-rev-range");
        let cases: Vec<(&str, Vec<ApproxReason>, ApproxDirection)> = vec![
            ("no reasons", vec![], ApproxDirection::Exact),
            ("over only", vec![over.clone()], ApproxDirection::Over),
            ("under only", vec![under.clone()], ApproxDirection::Under),
            (
                "both",
                vec![over.clone(), under.clone()],
                ApproxDirection::OverUnder,
            ),
        ];
        for (name, reasons, want) in cases {
            let history = for_history(reasons.clone(), TEST_HISTORY);
            let graph = ApproximationContract::assemble(reasons, None);
            assert_eq!(history.direction, want, "{name}");
            assert_eq!(
                history.direction, graph.direction,
                "{name}: for_history diverged from the call-graph fold"
            );
        }
    }

    // --- receiver narrowing: node counters on the frontier ------------------

    const FORWARD_NARROWING_CODES: [&str; 4] = [
        "external-receiver-calls",
        "receiver-narrowed",
        "receiver-narrowed-interproc",
        "residual-import-visible",
    ];
    const BACKWARD_NARROWING_CODES: [&str; 3] = [
        "receiver-narrowed-away",
        "receiver-narrowed-away-interproc",
        "residual-narrowed-away",
    ];

    fn every_narrowing_counter(mut n: NodeRecord) -> NodeRecord {
        n.external_calls = 1;
        n.narrowing = cgx_core::NarrowingCounts {
            typed_out: 2,
            interproc_out: 3,
            visible_out: 4,
            visible_dropped_out: 5,
            typed_away_in: 6,
            interproc_away_in: 7,
            visible_away_in: 8,
        };
        n
    }

    fn reason<'c>(c: &'c ApproximationContract, code: &str) -> Option<&'c ApproxReason> {
        c.reasons.iter().find(|r| r.code == code)
    }

    fn narrowing_codes(c: &ApproximationContract) -> Vec<&'static str> {
        c.reasons
            .iter()
            .map(|r| r.code)
            .filter(|code| {
                FORWARD_NARROWING_CODES.contains(code) || BACKWARD_NARROWING_CODES.contains(code)
            })
            .collect()
    }

    /// A `callers` answer that may have lost callers to receiver narrowing must
    /// say so — including the negative case, where "no callers" would otherwise
    /// read as a clean, scoped negative.
    #[test]
    fn callers_of_a_symbol_with_narrowed_away_sites_says_so() {
        let mut x = node(1, "x", 20, None);
        x.narrowing.typed_away_in = 3;

        // Negative: nothing calls `x` by an edge.
        let view = GraphView::new(
            vec![node(0, "w", 10, None), x.clone()],
            Vec::new(),
            Vec::<Candidate>::new(),
        );
        let w = walker(None, Confidence::Possible);
        let results = crate::callers(&view, NodeId(1), &w);
        assert!(results.is_empty());
        let c = for_neighbors(&view, &w, NodeId(1), Direction::Backward, &results);
        assert_eq!(c.direction, ApproxDirection::Under);
        let r = reason(&c, "receiver-narrowed-away").expect("reason present");
        assert_eq!(r.direction, ReasonDirection::Under);
        assert!(r.detail.starts_with("3 same-name"), "{}", r.detail);
        assert!(c.scope.is_some(), "a negative keeps its scope");

        // Positive: `w -> x` is found, and the answer still says it may be short.
        let view = GraphView::new(
            vec![node(0, "w", 10, None), x],
            vec![edge(0, 0, 1, Confidence::Certain, &[], None)],
            Vec::<Candidate>::new(),
        );
        let results = crate::callers(&view, NodeId(1), &w);
        assert_eq!(results.len(), 1);
        let c = for_neighbors(&view, &w, NodeId(1), Direction::Backward, &results);
        assert_eq!(c.direction, ApproxDirection::Under);
        assert!(reason(&c, "receiver-narrowed-away").is_some());
    }

    #[test]
    fn callees_does_not_carry_the_backward_narrowing_reasons() {
        let x = every_narrowing_counter(node(0, "x", 10, None));
        let view = GraphView::new(vec![x], Vec::new(), Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);
        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        assert_eq!(narrowing_codes(&c), FORWARD_NARROWING_CODES.to_vec());

        // And the mirror: a callers walk carries only the backward ones.
        let results = crate::callers(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Backward, &results);
        assert_eq!(narrowing_codes(&c), BACKWARD_NARROWING_CODES.to_vec());
    }

    #[test]
    fn forward_narrowed_sites_qualify_callees_and_a_negative_reaches() {
        let mut a = node(0, "a", 10, None);
        a.narrowing.typed_out = 2;
        let nodes = vec![a, node(1, "b", 20, None)];
        let view = GraphView::new(nodes, Vec::new(), Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);

        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        let r = reason(&c, "receiver-narrowed").expect("callees carries it");
        assert!(
            r.detail.starts_with("2 virtual call site(s)"),
            "{}",
            r.detail
        );
        assert!(
            r.detail.contains("monkey-patching"),
            "assumptions are stated"
        );

        let result = reaches(&view, NodeId(0), NodeId(1), &w);
        assert!(!result.reachable);
        let c = for_reaches(&view, &w, NodeId(0), &result);
        assert_eq!(c.direction, ApproxDirection::Under);
        assert!(reason(&c, "receiver-narrowed").is_some());
        assert!(c.scope.is_some());
    }

    /// The import-visibility rule is not sound for duck typing, and both reasons
    /// it feeds must say so where they are used.
    #[test]
    fn import_visibility_reasons_state_they_are_not_sound_for_duck_typing() {
        let mut a = node(0, "a", 10, None);
        a.narrowing.visible_out = 3;
        a.narrowing.visible_dropped_out = 1;
        a.narrowing.visible_away_in = 2;
        let view = GraphView::new(vec![a], Vec::new(), Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);

        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        let r = reason(&c, "residual-import-visible").expect("forward reason");
        assert!(
            r.detail.starts_with("4 untyped-receiver call site(s)"),
            "{}",
            r.detail
        );
        assert!(r.detail.contains("(1 left with no target)"), "{}", r.detail);
        assert!(r.detail.contains("not sound for duck typing"));

        let results = crate::callers(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Backward, &results);
        let r = reason(&c, "residual-narrowed-away").expect("backward reason");
        assert!(r.detail.starts_with("2 same-name"), "{}", r.detail);
        assert!(r.detail.contains("not sound for duck typing"));
    }

    #[test]
    fn external_receiver_calls_are_distinct_from_unresolved_external_calls() {
        let mut a = node(0, "a", 10, None);
        a.external_calls = 1;
        let nodes = vec![a, node(1, "b", 20, None)];
        let view = GraphView::new(nodes, Vec::new(), Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);
        let result = reaches(&view, NodeId(0), NodeId(1), &w);
        let c = for_reaches(&view, &w, NodeId(0), &result);
        assert_eq!(c.direction, ApproxDirection::Under);
        let r = reason(&c, "external-receiver-calls").expect("reason present");
        assert!(r.detail.starts_with("1 call(s)"), "{}", r.detail);
        assert!(reason(&c, "unresolved-external-calls").is_none());
    }

    #[test]
    fn kind_filter_silences_the_narrowing_facts_only_outside_the_call_family() {
        let cases: &[(Option<&[EdgeKind]>, bool)] = &[
            (None, true),
            (Some(&[EdgeKind::Calls]), true),
            (Some(&[EdgeKind::CallsVirtual]), true),
            (Some(&[EdgeKind::DerivesFrom]), false),
            (Some(&[EdgeKind::Calls, EdgeKind::DerivesFrom]), false),
        ];
        let x = every_narrowing_counter(node(0, "x", 10, None));
        let view = GraphView::new(vec![x], Vec::new(), Vec::<Candidate>::new());
        for (kinds, expect) in cases {
            let mut filter = EdgeFilter::default();
            if let Some(ks) = kinds {
                filter = filter.with_kinds(ks.to_vec());
            }
            let w = PathWalker {
                filter,
                max_depth: None,
                max_paths: None,
                max_steps: None,
            };
            for (dir, codes) in [
                (Direction::Forward, FORWARD_NARROWING_CODES.to_vec()),
                (Direction::Backward, BACKWARD_NARROWING_CODES.to_vec()),
            ] {
                let c = for_neighbors(&view, &w, NodeId(0), dir, &[]);
                let want = if *expect { codes } else { Vec::new() };
                assert_eq!(narrowing_codes(&c), want, "kinds {kinds:?}, {dir:?}");
            }
        }
    }

    #[test]
    fn narrowing_counters_on_depth_horizon_nodes_are_not_reported() {
        // w -> x at depth 1. With `max_depth = 1`, `x` sits on the horizon:
        // its narrowed sites could only lose neighbors beyond the bound.
        let w_node = every_narrowing_counter(node(0, "w", 10, None));
        let x_node = every_narrowing_counter(node(1, "x", 20, None));
        let view = GraphView::new(
            vec![w_node, x_node],
            vec![edge(0, 0, 1, Confidence::Certain, &[], None)],
            Vec::<Candidate>::new(),
        );
        let w = walker(Some(1), Confidence::Possible);

        // Forward from `w`: only `w` (depth 0) contributes.
        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        let r = reason(&c, "receiver-narrowed").expect("w's own sites count");
        assert!(r.detail.starts_with("2 virtual"), "{}", r.detail);

        // Backward from `x`: only `x` (depth 0) contributes, not `w` (depth 1).
        let results = crate::callers(&view, NodeId(1), &w);
        let c = for_neighbors(&view, &w, NodeId(1), Direction::Backward, &results);
        let r = reason(&c, "receiver-narrowed-away").expect("x's own sites count");
        assert!(r.detail.starts_with("6 same-name"), "{}", r.detail);

        // Unbounded, both nodes contribute.
        let w = walker(None, Confidence::Possible);
        let results = crate::callers(&view, NodeId(1), &w);
        let c = for_neighbors(&view, &w, NodeId(1), Direction::Backward, &results);
        let r = reason(&c, "receiver-narrowed-away").expect("both count");
        assert!(r.detail.starts_with("12 same-name"), "{}", r.detail);
    }

    #[test]
    fn positive_reaches_with_a_witness_carries_no_narrowing_reason() {
        let a = every_narrowing_counter(node(0, "a", 10, None));
        let nodes = vec![a, node(1, "b", 20, None)];
        let edges = vec![edge(0, 0, 1, Confidence::Certain, &[], None)];
        let view = GraphView::new(nodes, edges, Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);
        let result = reaches(&view, NodeId(0), NodeId(1), &w);
        assert!(result.reachable);
        let c = for_reaches(&view, &w, NodeId(0), &result);
        assert_eq!(c.direction, ApproxDirection::Exact);
        assert!(narrowing_codes(&c).is_empty());
    }

    #[test]
    fn narrowing_reasons_follow_dangling_refs_and_precede_cut_markers() {
        let mut a = every_narrowing_counter(node(0, "a", 10, None));
        a.unresolved_calls = 1;
        let nodes = vec![a, node(1, "b", 20, None)];
        let edges = vec![edge(
            0,
            0,
            1,
            Confidence::Certain,
            &[CutMarker::Dynamic],
            None,
        )];
        let view = GraphView::new(nodes, edges, Vec::<Candidate>::new());
        let w = walker(None, Confidence::Possible);
        let results = callees(&view, NodeId(0), &w);
        let c = for_neighbors(&view, &w, NodeId(0), Direction::Forward, &results);
        let codes: Vec<_> = c.reasons.iter().map(|r| r.code).collect();
        let mut want = vec!["unresolved-external-calls"];
        want.extend(FORWARD_NARROWING_CODES);
        want.push("dynamic-dispatch");
        assert_eq!(codes, want);
    }

    #[test]
    fn receiver_cut_markers_have_their_own_codes_and_serde_tokens() {
        assert_eq!(
            serde_json::to_string(&CutMarker::External).unwrap(),
            "\"external\""
        );
        assert_eq!(
            serde_json::to_string(&CutMarker::UntypedReceiver).unwrap(),
            "\"untyped-receiver\""
        );
        assert_eq!(cut_reason(CutMarker::External, 1).code, "external-receiver");
        assert_eq!(
            cut_reason(CutMarker::UntypedReceiver, 1).code,
            "untyped-receiver"
        );
    }
}
