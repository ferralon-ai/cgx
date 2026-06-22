//! [`PathWalker`] — the per-walk state machine that the traversal engines share.
//!
//! The walker owns the two pieces of state a CTE cannot express, which is exactly
//! why Layer 1 is direct Rust graph algorithms rather than recursive SQL
//! (architecture §4):
//!
//! 1. **Path-relative transience (GM-4.2).** A per-walk `seen_exceptional` flag,
//!    set irrevocably when an exceptional-class edge (`exception`/`panic`) is
//!    crossed, and *reset at every fork* to the value it held at the fork point.
//!    A DFS naturally provides fork-point reset: the flag is part of the recursion
//!    state, so backtracking restores it.
//! 2. **Spawn-domain reset (GM-9.2).** Crossing a `Spawns` edge enters a detached
//!    error domain: `seen_exceptional` is reinitialised to `false` downstream of
//!    the spawn, so exception edges in the spawner do not make callees inside the
//!    spawned task exception-transient.
//!
//! The walker is configured with an [`EdgeFilter`] (static per-edge admission)
//! and a depth limit; it exposes the two traversal shapes the subcommands need:
//! a bounded breadth-first frontier walk (`callers`/`callees`) and a bounded
//! depth-first path enumeration (`paths`/`reaches`).

use std::collections::VecDeque;

use cgx_core::{Confidence, EdgeCondition, EdgeId, EdgeRecord, NodeId};

use crate::filter::{Direction, EdgeFilter};
use crate::view::GraphView;

/// Default cap on enumerated paths, applied when the caller sets none. Keeps a
/// dense graph's path explosion bounded without the caller having to remember to.
pub const DEFAULT_MAX_PATHS: usize = 1024;

/// Default cap on total DFS traversal work for `paths` enumeration, applied when
/// the caller sets none. This is the depth-independent backstop: the path cap
/// ([`DEFAULT_MAX_PATHS`]) bounds *results found*, but on a dense graph the search
/// tree is exponential and the cap is never reached, so the DFS would run forever.
/// `DEFAULT_MAX_STEPS` bounds *work done* — counted as DFS node-visits (one per
/// recursive descent) — and stops the walk when exhausted, marking the result
/// truncated. Sized with headroom for legitimate large-but-finite searches while
/// keeping a pathological dense graph well under a second.
pub const DEFAULT_MAX_STEPS: u64 = 2_000_000;

/// Why a `paths` enumeration stopped before exhausting the full simple-path set.
/// `None` (absent) means the enumeration was complete; a value names which bound
/// it hit. Mirrors the [`cgx_core::CutMarker`] honesty idiom: a budget cutoff is a
/// surfaced fact, never a silent drop and never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationReason {
    /// The work budget ([`PathWalker::max_steps`]) was exhausted before the search
    /// tree was fully explored — there may be paths the walk never reached.
    StepBudget,
    /// The result cap ([`PathWalker::max_paths`]) filled — there may be more paths
    /// at or beyond the last one returned.
    PathCap,
}

impl TruncationReason {
    /// A stable kebab-case token for JSON / `structuredContent` surfaces.
    pub fn token(self) -> &'static str {
        match self {
            TruncationReason::StepBudget => "step-budget",
            TruncationReason::PathCap => "path-cap",
        }
    }
}

/// The outcome of a [`PathWalker::enumerate_paths`] call: the enumerated paths plus
/// whether the enumeration was cut short by a bound (and which one).
#[derive(Debug, Clone, Default)]
pub struct PathEnumeration {
    pub paths: Vec<Vec<WalkStep>>,
    /// `Some` when a bound stopped the walk before the full simple-path set was
    /// explored; `None` when the enumeration is complete.
    pub truncation: Option<TruncationReason>,
}

/// Configuration shared by every traversal a walk performs.
#[derive(Debug, Clone)]
pub struct PathWalker {
    /// Static per-edge admission predicate.
    pub filter: EdgeFilter,
    /// Maximum hop count from the anchor. `None` = unbounded (whole reachable set).
    pub max_depth: Option<u32>,
    /// Maximum number of paths to enumerate (`paths` only). `None` =
    /// [`DEFAULT_MAX_PATHS`].
    pub max_paths: Option<usize>,
    /// Maximum DFS node-visits for `paths` enumeration (the depth-independent
    /// work backstop). `None` = [`DEFAULT_MAX_STEPS`].
    pub max_steps: Option<u64>,
}

impl Default for PathWalker {
    fn default() -> Self {
        PathWalker {
            filter: EdgeFilter::calls(),
            max_depth: None,
            max_paths: None,
            max_steps: None,
        }
    }
}

/// Whether crossing `edge` should set the per-walk exceptional flag, and whether
/// it resets the exceptional domain (GM-9.2). Returns the `seen_exceptional`
/// value that holds *after* crossing `edge`, given its value `before`.
fn advance_exceptional(before: bool, edge: &EdgeRecord) -> bool {
    if edge.kind == cgx_core::EdgeKind::Spawns {
        // Detached error domain: downstream of a spawn the flag reinitialises to
        // false regardless of the spawn edge's own condition (GM-9.2.1).
        return false;
    }
    before || edge.condition.is_exceptional()
}

/// One induced edge in a [`Subgraph`], in **traversal orientation**: `src` is the
/// node the walk expands *from* and `dst` the peer it reaches (so for `callees`
/// this is caller → callee, and for `callers` it is callee → caller). The forest
/// roots at the anchor and a node's out-edges here are its children. `condition`
/// (GM-3) / `confidence` (GM-5) are the underlying call edge's, carried verbatim
/// for the rendered tag. Both endpoints are nodes in the same `Subgraph`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeRec {
    pub src: NodeId,
    pub dst: NodeId,
    /// The producing edge's id (a stable tiebreak for deterministic ordering).
    pub edge_id: EdgeId,
    pub condition: EdgeCondition,
    pub confidence: Confidence,
}

/// The induced sub-graph of a bounded neighbor walk: the reached nodes plus every
/// admitted edge among them in **traversal orientation** (see [`EdgeRec`]). This
/// is the shape both forest renderers (full + spanning) build their adjacency from
/// — see [`PathWalker::neighborhood`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subgraph {
    /// The walk roots (the anchor). The forest renders one tree per root.
    pub roots: Vec<NodeId>,
    /// Every reached node, including the anchor, in ascending id order.
    pub nodes: Vec<NodeId>,
    /// The induced edges in traversal orientation, in `(src, dst, edge id)` order.
    pub edges: Vec<EdgeRec>,
}

/// A node discovered by [`PathWalker::bfs`], with its discovery metadata.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub node: NodeId,
    /// Hops from the anchor (≥ 1).
    pub depth: u32,
    /// Edge through which the node was first reached at `depth`.
    pub via: EdgeRecord,
    /// `seen_exceptional` value on the (shortest) discovery path *at* this node.
    pub exception_transient: bool,
}

impl PathWalker {
    /// The effective path cap.
    fn path_cap(&self) -> usize {
        self.max_paths.unwrap_or(DEFAULT_MAX_PATHS)
    }

    /// The effective work budget (DFS node-visits).
    fn step_budget(&self) -> u64 {
        self.max_steps.unwrap_or(DEFAULT_MAX_STEPS)
    }

    /// Whether `depth` is within the configured limit.
    fn within_depth(&self, depth: u32) -> bool {
        self.max_depth.is_none_or(|d| depth <= d)
    }

    /// Breadth-first frontier walk from `start` in `dir`, returning each reachable
    /// node once (at its shortest depth) with its discovery edge and the
    /// path-relative transience flag on that shortest path.
    ///
    /// `start` itself is not included. Results are returned in a deterministic
    /// order: ascending `(depth, node_id)`. Because the frontier is processed in
    /// ascending node-id order and edges come back in canonical order, the first
    /// time a node is seen is stable across runs.
    pub fn bfs(&self, view: &GraphView, start: NodeId, dir: Direction) -> Vec<Discovered> {
        let n = view.node_count();
        let mut seen = vec![false; n];
        let mut out: Vec<Discovered> = Vec::new();
        // (node, depth, seen_exceptional-on-shortest-path)
        let mut frontier: VecDeque<(NodeId, u32, bool)> = VecDeque::new();

        if let Some(si) = checked_index(start, n) {
            seen[si] = true;
        }
        frontier.push_back((start, 0, false));

        while let Some((node, depth, exc)) = frontier.pop_front() {
            if !self.within_depth(depth + 1) {
                continue;
            }
            // Snapshot + sort this node's admitted neighbors by peer id so the
            // discovery order is deterministic regardless of edge layout.
            let mut nbrs: Vec<(NodeId, EdgeRecord, bool)> = view
                .neighbors(node, dir, &self.filter)
                .map(|er| {
                    let next_exc = advance_exceptional(exc, er.edge);
                    (er.peer, er.edge.clone(), next_exc)
                })
                .collect();
            nbrs.sort_by_key(|(peer, edge, _)| (peer.0, edge.id.0));

            for (peer, edge, next_exc) in nbrs {
                let pi = match checked_index(peer, n) {
                    Some(pi) => pi,
                    None => continue,
                };
                if seen[pi] {
                    continue;
                }
                seen[pi] = true;
                out.push(Discovered {
                    node: peer,
                    depth: depth + 1,
                    via: edge,
                    exception_transient: next_exc,
                });
                frontier.push_back((peer, depth + 1, next_exc));
            }
        }

        out.sort_by_key(|d| (d.depth, d.node.0));
        out
    }

    /// The bounded directional neighborhood of `anchor`: every node reached by a
    /// depth-bounded walk in `dir` (the anchor plus its [`bfs`](Self::bfs)
    /// frontier), together with the **induced** admitted call-edge set among those
    /// nodes — every admitted edge whose both endpoints are in the reached set,
    /// not just the shortest-path discovery edges.
    ///
    /// This is the single source of truth both forest renderers consume: unlike
    /// [`bfs`](Self::bfs), which keeps only each node's first (shortest) discovery
    /// edge, the induced edge set retains the full who-calls-whom adjacency so a
    /// full-expansion tree can show a callee under every caller that reaches it.
    ///
    /// `roots` is the anchor (in canonical direction the anchor is always the sole
    /// root). `nodes` and `edges` are returned in deterministic order (nodes by id;
    /// edges by `(src id, dst id, edge id)`).
    pub fn neighborhood(
        &self,
        view: &GraphView,
        anchor: NodeId,
        dir: Direction,
    ) -> Subgraph {
        let n = view.node_count();
        let mut in_set = vec![false; n];
        if let Some(ai) = checked_index(anchor, n) {
            in_set[ai] = true;
        }
        let mut nodes: Vec<NodeId> = vec![anchor];
        for d in self.bfs(view, anchor, dir) {
            if let Some(i) = checked_index(d.node, n) {
                if !in_set[i] {
                    in_set[i] = true;
                    nodes.push(d.node);
                }
            }
        }

        // Induced edge set in **traversal orientation**: for every reached node,
        // every admitted neighbor (in walk direction `dir`) that is also reached,
        // recorded as `src = the node we expand from`, `dst = the peer reached`.
        // For `callees` this is caller → callee; for `callers` it is callee →
        // caller. Either way the forest roots at the anchor and expands outward,
        // and a node's out-edges here are exactly its children in the tree. The
        // edge's own condition/confidence are carried verbatim for the tag.
        let mut edges: Vec<EdgeRec> = Vec::new();
        for &node in &nodes {
            for er in view.neighbors(node, dir, &self.filter) {
                let pi = match checked_index(er.peer, n) {
                    Some(pi) => pi,
                    None => continue,
                };
                if !in_set[pi] {
                    continue;
                }
                edges.push(EdgeRec {
                    src: node,
                    dst: er.peer,
                    edge_id: er.edge.id,
                    condition: er.edge.condition,
                    confidence: er.edge.confidence,
                });
            }
        }
        nodes.sort_by_key(|id| id.0);
        edges.sort_by_key(|e| (e.src.0, e.dst.0, e.edge_id.0));

        Subgraph {
            roots: vec![anchor],
            nodes,
            edges,
        }
    }

    /// Whether `goal` is reachable from `start` in `dir` within the depth limit.
    /// A pure BFS over admitted edges; cheaper than enumerating a witness.
    pub fn reachable(&self, view: &GraphView, start: NodeId, goal: NodeId, dir: Direction) -> bool {
        if start == goal {
            return true;
        }
        self.bfs(view, start, dir).iter().any(|d| d.node == goal)
    }

    /// A shortest witnessing path from `start` to `goal` in `dir`, as the node
    /// sequence plus the edge taken into each non-source node. `None` if
    /// unreachable within the depth limit. Deterministic: BFS predecessor recorded
    /// only on first (shortest, lowest-id) discovery.
    pub fn shortest_path(
        &self,
        view: &GraphView,
        start: NodeId,
        goal: NodeId,
        dir: Direction,
    ) -> Option<Vec<(NodeId, Option<EdgeRecord>)>> {
        if start == goal {
            return Some(vec![(start, None)]);
        }
        let n = view.node_count();
        let mut pred: Vec<Option<(NodeId, EdgeRecord)>> = vec![None; n];
        let mut seen = vec![false; n];
        let mut frontier: VecDeque<(NodeId, u32)> = VecDeque::new();
        if let Some(si) = checked_index(start, n) {
            seen[si] = true;
        }
        frontier.push_back((start, 0));

        while let Some((node, depth)) = frontier.pop_front() {
            if !self.within_depth(depth + 1) {
                continue;
            }
            let mut nbrs: Vec<(NodeId, EdgeRecord)> = view
                .neighbors(node, dir, &self.filter)
                .map(|er| (er.peer, er.edge.clone()))
                .collect();
            nbrs.sort_by_key(|(peer, edge)| (peer.0, edge.id.0));
            for (peer, edge) in nbrs {
                let pi = match checked_index(peer, n) {
                    Some(pi) => pi,
                    None => continue,
                };
                if seen[pi] {
                    continue;
                }
                seen[pi] = true;
                pred[pi] = Some((node, edge));
                if peer == goal {
                    return Some(reconstruct(start, goal, &pred));
                }
                frontier.push_back((peer, depth + 1));
            }
        }
        None
    }

    /// Enumerate simple (acyclic) paths from `start` to `goal` in `dir`, within
    /// the depth limit, capped at the path limit, and bounded by the work budget.
    /// Each path is the node sequence plus the entering edge per node (source's
    /// entering edge is `None`) plus the per-step `seen_exceptional` flag *before*
    /// entering that node.
    ///
    /// DFS gives fork-point reset for free: `seen_exceptional` lives on the
    /// recursion stack, so backtracking to a fork restores the flag (GM-4.2).
    /// Paths are returned in a deterministic discovery order (neighbors visited in
    /// ascending peer/edge id), then the caller applies the result ordering.
    ///
    /// Returns a [`PathEnumeration`] whose `truncation` is `Some` when a bound
    /// (work budget or path cap) stopped the walk before the search tree was fully
    /// explored. Truncation is deterministic: the same graph and bounds visit the
    /// same nodes in the same sorted-neighbor order and stop at the same point.
    pub fn enumerate_paths(
        &self,
        view: &GraphView,
        start: NodeId,
        goal: NodeId,
        dir: Direction,
    ) -> PathEnumeration {
        let mut walk = DfsState {
            paths: Vec::new(),
            cap: self.path_cap(),
            steps: 0,
            budget: self.step_budget(),
            truncation: None,
        };
        let mut stack: Vec<WalkStep> = vec![WalkStep {
            node: start,
            via: None,
            exception_transient: false,
        }];
        let mut on_path = vec![false; view.node_count()];
        if let Some(si) = checked_index(start, view.node_count()) {
            on_path[si] = true;
        }
        self.dfs(view, start, goal, dir, &mut stack, &mut on_path, &mut walk);
        PathEnumeration {
            paths: walk.paths,
            truncation: walk.truncation,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn dfs(
        &self,
        view: &GraphView,
        node: NodeId,
        goal: NodeId,
        dir: Direction,
        stack: &mut Vec<WalkStep>,
        on_path: &mut [bool],
        walk: &mut DfsState,
    ) {
        // Charge one unit of work per node-visit. Exhausting the budget stops the
        // search regardless of depth — the dense-graph backstop.
        walk.steps += 1;
        if walk.steps > walk.budget {
            walk.truncation = Some(TruncationReason::StepBudget);
            return;
        }
        if walk.paths.len() >= walk.cap {
            return;
        }
        if node == goal && stack.len() > 1 {
            walk.paths.push(stack.clone());
            if walk.paths.len() >= walk.cap {
                walk.truncation = Some(TruncationReason::PathCap);
            }
            return;
        }
        let depth = (stack.len() - 1) as u32;
        if !self.within_depth(depth + 1) {
            return;
        }
        let exc_here = stack.last().map(|s| s.exception_transient).unwrap();

        let mut nbrs: Vec<(NodeId, EdgeRecord, bool)> = view
            .neighbors(node, dir, &self.filter)
            .map(|er| {
                let next_exc = advance_exceptional(exc_here, er.edge);
                (er.peer, er.edge.clone(), next_exc)
            })
            .collect();
        nbrs.sort_by_key(|(peer, edge, _)| (peer.0, edge.id.0));

        let n = view.node_count();
        for (peer, edge, next_exc) in nbrs {
            let pi = match checked_index(peer, n) {
                Some(pi) => pi,
                None => continue,
            };
            if on_path[pi] {
                continue; // simple paths only — no cycles
            }
            on_path[pi] = true;
            stack.push(WalkStep {
                node: peer,
                via: Some(edge),
                exception_transient: next_exc,
            });
            self.dfs(view, peer, goal, dir, stack, on_path, walk);
            stack.pop();
            on_path[pi] = false;
            if walk.truncation == Some(TruncationReason::StepBudget)
                || walk.paths.len() >= walk.cap
            {
                return;
            }
        }
    }
}

/// Mutable per-walk DFS bookkeeping, threaded through the recursion so the
/// signature stays small as bounds accumulate.
struct DfsState {
    paths: Vec<Vec<WalkStep>>,
    cap: usize,
    steps: u64,
    budget: u64,
    truncation: Option<TruncationReason>,
}

/// One node on an enumerated walk, with the edge taken to enter it and the
/// path-relative transience flag *as the node is entered* (GM-4).
#[derive(Debug, Clone)]
pub struct WalkStep {
    pub node: NodeId,
    pub via: Option<EdgeRecord>,
    /// `seen_exceptional` value at the moment this node is entered — i.e. whether
    /// an exceptional-class edge was crossed on the way here.
    pub exception_transient: bool,
}

/// Index a node id into a `0..n` array, returning `None` if out of range.
fn checked_index(node: NodeId, n: usize) -> Option<usize> {
    let i = node.index();
    if i < n {
        Some(i)
    } else {
        None
    }
}

/// Walk the predecessor array back from `goal` to `start`, producing the path in
/// source→sink order with each node's entering edge.
fn reconstruct(
    start: NodeId,
    goal: NodeId,
    pred: &[Option<(NodeId, EdgeRecord)>],
) -> Vec<(NodeId, Option<EdgeRecord>)> {
    let mut rev: Vec<(NodeId, Option<EdgeRecord>)> = Vec::new();
    let mut cur = goal;
    loop {
        match &pred[cur.index()] {
            Some((p, edge)) => {
                rev.push((cur, Some(edge.clone())));
                cur = *p;
            }
            None => {
                rev.push((cur, None));
                break;
            }
        }
    }
    debug_assert_eq!(rev.last().map(|(n, _)| *n), Some(start));
    rev.reverse();
    rev
}
