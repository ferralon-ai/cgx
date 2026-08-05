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
        let over = reasons
            .iter()
            .any(|r| r.direction == ReasonDirection::Over);
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
        CutMarker::ViaFfi => ("foreign-function", "calls crossing an FFI boundary not modeled"),
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
    /// Per-marker count of cut-marked edges incident (in the walk direction) to a
    /// reached node — the blind spots on the search frontier.
    cut_counts: BTreeMap<CutMarker, usize>,
    /// Edges of the searched family that sit below the confidence floor (excluded
    /// from the walk but a real, unfollowed source of edges).
    below_floor: usize,
    /// A reached node at the depth horizon had an admitted neighbor beyond it.
    depth_truncated: bool,
}

impl FrontierFacts {
    fn into_reasons(self, walker: &PathWalker) -> Vec<ApproxReason> {
        let mut reasons = Vec::new();
        if self.dangling_refs > 0 {
            reasons.push(dangling_refs_reason(self.dangling_refs));
        }
        for (marker, count) in self.cut_counts {
            reasons.push(cut_reason(marker, count));
        }
        if self.below_floor > 0 {
            reasons.push(below_floor_reason(walker.filter.min_confidence, self.below_floor));
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
/// collecting the incompleteness facts on its frontier: dangling-ref counts on
/// the touched nodes (calls that left no edge), cut-marked and below-floor
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
    // confidence floor or condition restriction, so cuts and below-floor edges
    // the *walk* skipped are still seen as honest incompleteness sources.
    let scan_filter = EdgeFilter {
        min_confidence: Confidence::Possible,
        condition: ConditionFilter::Any,
        kinds: walker.filter.kinds.clone(),
    };
    let floor = walker.filter.min_confidence;

    // Dangling refs are a fact about a node's *body* (its outgoing calls), so
    // they witness incompleteness only when the walk follows call-family edges
    // in the forward direction (callees/reaches/unused frontiers). A backward
    // (callers) walk or a DerivesFrom-scoped walk does not traverse the missing
    // out-edge.
    let count_dangling = dir == Direction::Forward && walker.filter.kinds.is_none();

    let mut facts = FrontierFacts::default();
    for &node in &touched {
        let node_ix = node.index();
        if count_dangling {
            facts.dangling_refs += u64::from(view.node(node).unresolved_calls);
        }
        for er in view.neighbors(node, dir, &scan_filter) {
            for marker in er.edge.cut_markers.iter() {
                *facts.cut_counts.entry(marker).or_default() += 1;
            }
            if er.edge.confidence < floor {
                facts.below_floor += 1;
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
    if results
        .iter()
        .any(|r| r.min_confidence_on_path == Confidence::Possible || r.via.candidate_group.is_some())
    {
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
            reasons.extend(scan_frontier(view, walker, &[from], Direction::Forward).into_reasons(walker));
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
        reasons.extend(scan_frontier(view, walker, &[from], Direction::Forward).into_reasons(walker));
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
        Candidate, CutMarker, CutMarkers, EdgeCondition, EdgeId, EntrypointKind, NodeId, NodeRecord,
        SymbolKind, Tier, Visibility,
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
        let nodes = vec![node(0, "a", 10, None), node(1, "b", 20, None), node(2, "c", 30, None)];
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
            c.reasons.iter().any(|r| r.code == "unresolved-external-calls"),
            "dangling refs on the touched frontier must surface: {c:?}"
        );
        assert!(c.scope.is_some());
    }

    #[test]
    fn negative_reaches_carries_scope_and_frontier_caveat() {
        // a -> b (certain) with an unresolved cut on b; c is unreachable from a.
        let nodes = vec![node(0, "a", 10, None), node(1, "b", 20, None), node(2, "c", 30, None)];
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

    #[test]
    fn unused_is_negative_with_scope() {
        // entry `a` reaches `b`; `c` is dead. A dynamic cut on the used frontier
        // makes the dead list under (over-inclusive).
        let nodes = vec![
            node(0, "a", 10, Some(EntrypointKind::Main)),
            node(1, "b", 20, None),
            node(2, "c", 30, None),
        ];
        let edges = vec![edge(0, 0, 1, Confidence::Certain, &[CutMarker::Reflective], None)];
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
}
