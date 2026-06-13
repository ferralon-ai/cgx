//! Typed result records returned by the Layer-1 query functions.
//!
//! These are the CLI-facing contract (WP-10 formats them into text/json/SARIF;
//! this crate never formats). Every record surfaces the confidence ladder
//! (GM-5) and the edge-condition context (GM-3) that justified its inclusion,
//! plus the path-relative transience flag (GM-4) when it is meaningful. Records
//! carry the resolved [`NodeRecord`]/[`EdgeRecord`] by value so the caller has
//! file:line provenance without re-querying the view.

use cgx_core::{Confidence, EdgeCondition, EdgeRecord, NodeId, NodeRecord};

/// One symbol reached by a `callers`/`callees` walk.
///
/// `depth` is the hop count from the query anchor (1 = direct caller/callee).
/// `via` is the call edge through which this node was first reached at this depth
/// — it carries the edge condition (GM-3) and confidence (GM-5) that the result
/// surfaces. `min_confidence_on_path` is the weakest confidence on the discovery
/// path from the anchor (GM-1.3 weakest-link semantics applied to a path): a
/// transitive result is only as trustworthy as its least-certain hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighborResult {
    /// The reached symbol.
    pub node: NodeRecord,
    /// Hops from the query anchor (≥ 1).
    pub depth: u32,
    /// The edge first reaching this node at `depth`.
    pub via: EdgeRecord,
    /// Edge condition of `via` (denormalized for the formatter; GM-3).
    pub condition: EdgeCondition,
    /// Confidence of `via` (GM-5).
    pub confidence: Confidence,
    /// Weakest confidence along the discovery path from the anchor (GM-1.3).
    pub min_confidence_on_path: Confidence,
    /// Whether this node was reached on a path that already crossed an
    /// exceptional-class edge (GM-4 path-relative transience). For a breadth-first
    /// neighbor walk this reflects the first (shortest) discovery path.
    pub exception_transient: bool,
}

/// One step of an enumerated path (a node together with the edge taken to enter
/// it). The first step of a [`PathResult`] is the source node with `via = None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathStep {
    pub node: NodeRecord,
    /// The edge traversed to arrive at `node`; `None` for the path's source.
    pub via: Option<EdgeRecord>,
    /// Whether the walk had already crossed an exceptional-class edge *before*
    /// entering `node` — i.e. `node` is exception-transient on this path (GM-4).
    pub exception_transient: bool,
}

/// One complete path between two symbols (`paths`/`why`).
///
/// Steps are in source→sink order. The path surfaces the weakest confidence of
/// any edge it traverses (`min_confidence`) and whether it crosses the
/// exceptional class at any point (`crosses_exceptional`), so a caller can sort
/// "happy-path-first" or filter to exceptional paths without re-inspecting steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathResult {
    pub steps: Vec<PathStep>,
    /// Weakest confidence of any edge on the path (GM-5 surfaced per path).
    pub min_confidence: Confidence,
    /// Whether any edge on the path is in the exceptional class (GM-4).
    pub crosses_exceptional: bool,
}

impl PathResult {
    /// Hop count (number of edges) of the path.
    pub fn hops(&self) -> usize {
        self.steps.len().saturating_sub(1)
    }

    /// The ids of the nodes on the path, source→sink. Used by the ordering keys
    /// and by callers that want the bare node sequence.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.steps.iter().map(|s| s.node.id).collect()
    }
}

/// Whether one symbol can reach another (`reaches`).
///
/// `witness` is the shortest discovery path when reachable (the proof), `None`
/// otherwise. `min_confidence` mirrors the witness path's weakest edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachResult {
    pub from: NodeId,
    pub to: NodeId,
    pub reachable: bool,
    /// A shortest witnessing path when `reachable`; `None` otherwise.
    pub witness: Option<PathResult>,
}
