//! The Layer-1 subcommands as typed query functions (architecture §4).
//!
//! These are the surface WP-10 (the CLI) calls: each takes a [`GraphView`], a
//! resolved anchor (or [`SymbolPattern`]), and a [`PathWalker`] config, and
//! returns the typed records in [`crate::result`] — already in the deterministic
//! result order (IF-8: `(file, line, col)`, with depth/structural tiebreaks). The
//! CLI handles all formatting and SARIF emission.
//!
//! Every result surfaces confidence (GM-5) and edge-condition context (GM-3);
//! path-relative transience (GM-4) is computed per walk by the [`PathWalker`].

use cgx_core::{Confidence, NodeId, SymbolKind, SymbolPattern};

use crate::filter::Direction;
use crate::result::{NeighborResult, PathResult, PathStep, ReachResult};
use crate::view::{GraphView, ResolveError};
use crate::walk::{PathWalker, WalkStep};

/// Q-1 `callers`: symbols that (transitively, to the walker's depth) call `target`.
///
/// A reverse-direction frontier walk. Each result surfaces the discovery edge's
/// condition + confidence and the weakest confidence on the shortest discovery
/// path. Ordered by `(file, line, col, fqn)`.
pub fn callers(view: &GraphView, target: NodeId, walker: &PathWalker) -> Vec<NeighborResult> {
    neighbor_walk(view, target, walker, Direction::Backward)
}

/// Q-2 `callees`: symbols that `source` (transitively, to the walker's depth)
/// calls. A forward-direction frontier walk. Ordering and surfacing as `callers`.
pub fn callees(view: &GraphView, source: NodeId, walker: &PathWalker) -> Vec<NeighborResult> {
    neighbor_walk(view, source, walker, Direction::Forward)
}

fn neighbor_walk(
    view: &GraphView,
    anchor: NodeId,
    walker: &PathWalker,
    dir: Direction,
) -> Vec<NeighborResult> {
    // Build a depth/confidence-aware view by reconstructing the shortest-path
    // weakest confidence. The BFS gives each node's shortest discovery edge; for
    // the weakest-confidence-on-path we walk the predecessor chain once more via a
    // confidence-tracking BFS.
    let discovered = walker.bfs(view, anchor, dir);
    let min_conf = weakest_confidence_to(view, anchor, walker, dir);

    let mut out: Vec<NeighborResult> = discovered
        .into_iter()
        .map(|d| {
            let node = view.node(d.node).clone();
            let confidence = d.via.confidence;
            let condition = d.via.condition;
            let min_confidence_on_path = min_conf
                .get(d.node.index())
                .copied()
                .flatten()
                .unwrap_or(confidence);
            NeighborResult {
                node,
                depth: d.depth,
                via: d.via,
                condition,
                confidence,
                min_confidence_on_path,
                exception_transient: d.exception_transient,
            }
        })
        .collect();

    out.sort_by(|a, b| result_key(&a.node).cmp(&result_key(&b.node)));
    out
}

/// For every reachable node, the weakest edge confidence along *a* shortest path
/// from the anchor (GM-1.3 weakest-link applied to a path). Indexed by node id.
fn weakest_confidence_to(
    view: &GraphView,
    anchor: NodeId,
    walker: &PathWalker,
    dir: Direction,
) -> Vec<Option<Confidence>> {
    use std::collections::VecDeque;
    let n = view.node_count();
    let mut best: Vec<Option<Confidence>> = vec![None; n];
    let mut seen = vec![false; n];
    let mut frontier: VecDeque<(NodeId, u32)> = VecDeque::new();
    if anchor.index() < n {
        seen[anchor.index()] = true;
    }
    frontier.push_back((anchor, 0));
    while let Some((node, depth)) = frontier.pop_front() {
        if walker.max_depth.is_some_and(|d| depth + 1 > d) {
            continue;
        }
        let carried = if node == anchor {
            Confidence::Certain
        } else {
            best[node.index()].unwrap_or(Confidence::Certain)
        };
        let mut nbrs: Vec<(NodeId, Confidence, u32)> = view
            .neighbors(node, dir, &walker.filter)
            .map(|er| (er.peer, er.edge.confidence, er.edge.id.0))
            .collect();
        nbrs.sort_by_key(|(peer, _, eid)| (peer.0, *eid));
        for (peer, edge_conf, _) in nbrs {
            if peer.index() >= n || seen[peer.index()] {
                continue;
            }
            seen[peer.index()] = true;
            best[peer.index()] = Some(carried.weakest(edge_conf));
            frontier.push_back((peer, depth + 1));
        }
    }
    best
}

/// Q-19 / reachability: whether `from` reaches `to` along admitted call edges,
/// with a shortest witnessing path when it does.
pub fn reaches(view: &GraphView, from: NodeId, to: NodeId, walker: &PathWalker) -> ReachResult {
    match walker.shortest_path(view, from, to, Direction::Forward) {
        Some(seq) => {
            let witness = build_path_result(view, &seq_to_walksteps(view, &seq));
            ReachResult {
                from,
                to,
                reachable: true,
                witness: Some(witness),
            }
        }
        None => ReachResult {
            from,
            to,
            reachable: false,
            witness: None,
        },
    }
}

/// The set of nodes reachable forward from `from` (the `from → *` form of
/// reachability), as [`NeighborResult`]s — identical machinery to `callees` but
/// named for the reachability question. Ordered by `(file, line, col, fqn)`.
pub fn reaches_all(view: &GraphView, from: NodeId, walker: &PathWalker) -> Vec<NeighborResult> {
    callees(view, from, walker)
}

/// Q-3 `paths` / `why`: enumerate simple paths from `from` to `to`, honoring the
/// walker's edge-condition + confidence filter and surfacing per-path confidence
/// and exceptional-class crossing (GM-4). Ordered deterministically by
/// `(hops, source→sink node ids)`.
pub fn paths(view: &GraphView, from: NodeId, to: NodeId, walker: &PathWalker) -> Vec<PathResult> {
    let raw = walker.enumerate_paths(view, from, to, Direction::Forward);
    let mut results: Vec<PathResult> = raw
        .into_iter()
        .map(|steps| build_path_result(view, &steps))
        .collect();
    results.sort_by_key(path_key);
    results
}

/// Convert a BFS predecessor sequence into the [`WalkStep`] shape, recomputing the
/// per-step transience flag along the (now linear) path. The flag *at* a node is
/// the value after crossing its entering edge (GM-4.2 step 3), matching the DFS
/// path enumeration; a `spawns` entering edge resets the domain (GM-9.2.1).
fn seq_to_walksteps(
    _view: &GraphView,
    seq: &[(NodeId, Option<cgx_core::EdgeRecord>)],
) -> Vec<WalkStep> {
    let mut exc = false;
    let mut out = Vec::with_capacity(seq.len());
    for (node, via) in seq {
        if let Some(edge) = via {
            if edge.kind == cgx_core::EdgeKind::Spawns {
                exc = false;
            } else if edge.condition.is_exceptional() {
                exc = true;
            }
        }
        out.push(WalkStep {
            node: *node,
            via: via.clone(),
            exception_transient: exc,
        });
    }
    out
}

/// Assemble a [`PathResult`] from a list of walk steps, surfacing the weakest
/// confidence and whether the path is exception-transient at any node (GM-4).
///
/// `crosses_exceptional` is defined via the per-walk transience flag, not raw
/// edge presence: a node counts only if `seen_exceptional` holds *at* it. Because
/// a `spawns` edge resets that flag downstream (GM-9.2), an exception edge that is
/// upstream of a detaching spawn does not make the path exception-transient.
fn build_path_result(view: &GraphView, steps: &[WalkStep]) -> PathResult {
    let mut min_confidence = Confidence::Certain;
    let mut crosses_exceptional = false;
    let result_steps: Vec<PathStep> = steps
        .iter()
        .map(|s| {
            if let Some(edge) = &s.via {
                min_confidence = min_confidence.weakest(edge.confidence);
            }
            if s.exception_transient {
                crosses_exceptional = true;
            }
            PathStep {
                node: view.node(s.node).clone(),
                via: s.via.clone(),
                exception_transient: s.exception_transient,
            }
        })
        .collect();
    // A single-node path (from == to) traverses no edge: weakest confidence is the
    // top of the ladder by convention.
    PathResult {
        steps: result_steps,
        min_confidence,
        crosses_exceptional,
    }
}

/// Q-4 `unused`: symbols (of the requested kinds) not reachable by any call edge
/// from the entrypoint set. The complement of forward reachability from
/// entrypoints (GM-7). Ordered by `(file, line, col, fqn)`.
///
/// `entrypoints` is the resolved root set; when empty, every node tagged with an
/// `entrypoint_kind` is used (the declared-entrypoint default, Q-19). `kinds`
/// restricts which symbol kinds are reported as unused.
pub fn unused(
    view: &GraphView,
    entrypoints: &[NodeId],
    kinds: &[SymbolKind],
    walker: &PathWalker,
) -> Vec<cgx_core::NodeRecord> {
    let n = view.node_count();
    let mut reachable = vec![false; n];

    let roots: Vec<NodeId> = if entrypoints.is_empty() {
        view.nodes()
            .iter()
            .filter(|node| node.entrypoint_kind.is_some())
            .map(|node| node.id)
            .collect()
    } else {
        entrypoints.to_vec()
    };

    for &root in &roots {
        if root.index() < n {
            reachable[root.index()] = true;
        }
        for d in walker.bfs(view, root, Direction::Forward) {
            if d.node.index() < n {
                reachable[d.node.index()] = true;
            }
        }
    }

    let mut out: Vec<cgx_core::NodeRecord> = view
        .nodes()
        .iter()
        .filter(|node| !reachable[node.id.index()])
        .filter(|node| kinds.is_empty() || kinds.contains(&node.kind))
        .cloned()
        .collect();
    out.sort_by(|a, b| result_key(a).cmp(&result_key(b)));
    out
}

/// Resolve a `--from`/`--to` endpoint pattern to a unique anchor, surfacing the
/// resolution error to the caller (CLI maps it to an exit code).
pub fn resolve_anchor(view: &GraphView, pat: &SymbolPattern) -> Result<NodeId, ResolveError> {
    view.resolve_one(pat)
}

/// The default result ordering key (IF-8): `(file, line, col≈line_start, fqn)`.
fn result_key(node: &cgx_core::NodeRecord) -> (&str, u32, &str) {
    (node.file.as_str(), node.line_start, node.fqn.as_str())
}

/// Deterministic path ordering: fewer hops first, then by the source→sink node-id
/// sequence (stable and independent of discovery order).
fn path_key(p: &PathResult) -> (usize, Vec<u32>) {
    (p.hops(), p.node_ids().into_iter().map(|i| i.0).collect())
}
