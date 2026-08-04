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

use std::collections::{BTreeMap, BTreeSet};

use cgx_core::{Confidence, EntrypointKind, NodeId, SymbolKind, SymbolPattern};

use crate::filter::{Direction, EdgeFilter};
use crate::impacted::{ImpactedTests, ImpactedWitness};
use crate::result::{
    ExplainEdge, Explanation, NeighborResult, PathResult, PathSet, PathStep, ReachResult,
};
use crate::view::{GraphView, ResolveError};
use crate::walk::{Discovered, PathWalker, WalkStep};

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

/// The bounded directional neighborhood of `anchor` as an induced sub-graph (the
/// reached nodes plus every admitted call edge among them, in `caller → callee`
/// orientation). The single source of truth the CLI forest renderer consumes for
/// `callers`/`callees`/`reaches <from>`. `dir` selects callee (`Forward`) or
/// caller (`Backward`) expansion; the depth bound and confidence floor come from
/// `walker`, exactly as `callees`/`callers` use it.
pub fn neighborhood(
    view: &GraphView,
    anchor: NodeId,
    walker: &PathWalker,
    dir: Direction,
) -> crate::walk::Subgraph {
    walker.neighborhood(view, anchor, dir)
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
pub fn paths(view: &GraphView, from: NodeId, to: NodeId, walker: &PathWalker) -> PathSet {
    let raw = walker.enumerate_paths(view, from, to, Direction::Forward);
    let mut paths: Vec<PathResult> = raw
        .paths
        .into_iter()
        .map(|steps| build_path_result(view, &steps))
        .collect();
    paths.sort_by_key(path_key);
    PathSet {
        paths,
        truncation: raw.truncation,
    }
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

    let roots = entrypoint_roots(view, entrypoints);

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

/// The resolved entrypoint root set for `unused` (GM-7): the caller's explicit
/// `entrypoints` when non-empty, otherwise every node tagged with an
/// `entrypoint_kind` (the declared-entrypoint default). Shared with the
/// approximation-contract layer so the negative `unused` claim scans the same
/// used-set frontier the query itself walked (no logic fork).
pub fn entrypoint_roots(view: &GraphView, entrypoints: &[NodeId]) -> Vec<NodeId> {
    if entrypoints.is_empty() {
        view.nodes()
            .iter()
            .filter(|node| node.entrypoint_kind.is_some())
            .map(|node| node.id)
            .collect()
    } else {
        entrypoints.to_vec()
    }
}

/// Q-6 `explain`: full provenance for one symbol — its definition record plus
/// every direct (depth-1) incident edge with the condition (GM-3) and confidence
/// (GM-5) that justified it. The single source of truth for both the CLI `explain`
/// subcommand and the MCP `explain` tool (no logic fork).
///
/// `anchor` must be a node id in `view`. Returns `None` only if the id is out of
/// range (the caller resolves the symbol first, so this is the boundary guard).
pub fn explain(view: &GraphView, anchor: NodeId) -> Option<Explanation> {
    let node = view.try_node(anchor)?.clone();

    let depth1 = PathWalker {
        filter: EdgeFilter::calls(),
        max_depth: Some(1),
        max_paths: None,
        max_steps: None,
    };
    let incoming = callers(view, anchor, &depth1);
    let outgoing = callees(view, anchor, &depth1);

    let mut edges: Vec<ExplainEdge> = Vec::with_capacity(incoming.len() + outgoing.len());
    for r in &incoming {
        // For an incoming edge the peer calls the explained symbol, so the call
        // site lives in the peer's body.
        edges.push(explain_edge(true, r, &r.node));
    }
    for r in &outgoing {
        // For an outgoing edge the explained symbol calls the peer, so the call
        // site lives in the explained symbol's body.
        edges.push(explain_edge(false, r, &node));
    }

    Some(Explanation {
        node,
        callers_count: incoming.len(),
        callees_count: outgoing.len(),
        edges,
    })
}

/// Build an [`ExplainEdge`], copying the resolution provenance (`tier`/`rule`)
/// off the discovery edge (`NeighborResult.via`) and deriving the render-only
/// `resolution_source` + call-site span. `call_site_node` is the symbol whose
/// body contains the call (the peer for incoming edges, the explained symbol for
/// outgoing ones); the `EdgeRecord` carries no span, so file:line is taken from
/// that node (IF-6 "source file:line").
fn explain_edge(
    incoming: bool,
    r: &NeighborResult,
    call_site_node: &cgx_core::NodeRecord,
) -> ExplainEdge {
    let tier = r.via.tier;
    let resolution_source = if tier == cgx_core::Tier::Scip {
        Some("scip".to_string())
    } else {
        None
    };
    let site = if r.via.kind.is_call() {
        Some(cgx_core::Span::new(
            call_site_node.file.clone(),
            call_site_node.line_start,
            None,
        ))
    } else {
        None
    };
    ExplainEdge {
        incoming,
        peer: r.node.clone(),
        condition: r.condition,
        confidence: r.confidence,
        tier,
        rule: r.via.rule.clone(),
        resolution_source,
        site,
    }
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

/// `impacted-tests`: the test entrypoints whose call graph reaches any changed
/// symbol. One multi-root reverse (`Backward`) frontier walk with a
/// closure-containment lift, filtered to
/// `entrypoint_kind == Some(EntrypointKind::Test)`. Ordered by
/// `(file, line_start, fqn)` — the same IF-8 key every other answer uses.
///
/// It lives here rather than in [`crate::impacted`] because [`result_key`] is
/// module-private and is the crate's determinism contract.
///
/// # Root order cannot reach the answer
///
/// The root order is load-bearing beyond row order: with the
/// already-walked-root skip below, *which root expands first* decides each
/// node's discovery edge, hence its reported `depth`, `confidence` and witness
/// chain — and the `(file, line_start, fqn)` sort would hide the difference. So
/// `changed` is sorted and deduped **here** rather than trusted to arrive that
/// way: a caller that built its set from a `HashSet` would otherwise be
/// non-deterministic in the row *contents*, silently.
///
/// # Cost
///
/// `k' × O(V+E)` traversal, where `k'` ≤ `|changed|` is the number of roots
/// actually expanded, plus `O(V log V)` for the FQN index and `O(Σ depth)` for
/// the witness chains. No `shortest_path`, no path enumeration, no
/// `DerivesFrom`-widened filter — the three ways `path_diff`'s cost cliff comes
/// back.
pub fn impacted_tests(view: &GraphView, changed: &[NodeId], walker: &PathWalker) -> ImpactedTests {
    let n = view.node_count();
    let mut reached = vec![false; n];
    // Reached by another root's backward BFS — the only condition under which a
    // root's own expansion is redundant. A node pulled in by the containment
    // lift does NOT qualify: nothing has enumerated its backward closure.
    let mut walk_reached = vec![false; n];
    let mut expanded = vec![false; n];
    let mut via_of: Vec<Option<Discovered>> = vec![None; n];
    let mut lift_from: Vec<Option<NodeId>> = vec![None; n];
    let mut root_of: Vec<Option<NodeId>> = vec![None; n];

    // FQN → node id, lowest id winning a duplicate FQN. `BTreeMap` fed in
    // canonical node order, so the lift target never depends on hash order.
    let mut fqn_index: BTreeMap<&str, NodeId> = BTreeMap::new();
    for node in view.nodes() {
        fqn_index.entry(node.fqn.as_str()).or_insert(node.id);
    }

    let mut langs_in_play: BTreeSet<String> = BTreeSet::new();
    let mut seeds: Vec<NodeId> = changed.iter().copied().filter(|r| r.index() < n).collect();
    seeds.sort_unstable_by_key(|r| r.0);
    seeds.dedup();
    for &r in &seeds {
        reached[r.index()] = true;
        root_of[r.index()] = Some(r);
        langs_in_play.insert(view.node(r).lang.clone());
    }

    // Skipping a root another root's walk already reached is sound only when the
    // walk is unbounded: under `--depth N`, being reached at depth `d` leaves
    // the root's own remaining `N - d` hops unexplored.
    let skip_walked_roots = walker.max_depth.is_none();

    let mut worklist = seeds.clone();
    let mut round_new = seeds.clone();
    loop {
        for &r in &worklist {
            let ri = r.index();
            if ri >= n || expanded[ri] || (skip_walked_roots && walk_reached[ri]) {
                continue;
            }
            expanded[ri] = true;
            for d in walker.bfs(view, r, Direction::Backward) {
                let node = d.node;
                let di = node.index();
                if di >= n || reached[di] {
                    continue;
                }
                reached[di] = true;
                walk_reached[di] = true;
                root_of[di] = root_of[ri];
                via_of[di] = Some(d);
                round_new.push(node);
            }
        }

        // The containment lift: every reached `Lambda` pulls in its lexically
        // enclosing named callable. A fixpoint, not one pass — a lambda's owner
        // may itself only be called from inside another lambda.
        round_new.sort_unstable_by_key(|id| id.0);
        let mut next: BTreeSet<NodeId> = BTreeSet::new();
        for &x in &round_new {
            let node = view.node(x);
            if node.kind != SymbolKind::Lambda {
                continue;
            }
            let Some(owner) = crate::impacted::lambda_owner_fqn(&node.fqn)
                .and_then(|fqn| fqn_index.get(fqn).copied())
            else {
                continue;
            };
            let oi = owner.index();
            if oi >= n || reached[oi] {
                continue;
            }
            reached[oi] = true;
            root_of[oi] = root_of[x.index()];
            lift_from[oi] = Some(x);
            next.insert(owner);
        }
        if next.is_empty() {
            break;
        }
        worklist = next.into_iter().collect();
        round_new = worklist.clone();
    }

    let mut rows: Vec<(NeighborResult, ImpactedWitness)> = Vec::new();
    for node in view.nodes() {
        let i = node.id.index();
        if i >= n || !reached[i] || node.entrypoint_kind != Some(EntrypointKind::Test) {
            continue;
        }

        // Witness chain, reconstructed from the one BFS. On a `Backward` walk
        // `neighbors` sets `peer = edge.src`, so the discovered node IS the edge
        // source and its predecessor toward the root is `via.dst`.
        let mut chain = vec![node.id];
        let mut min_confidence_on_path = Confidence::Certain;
        let mut via_containment_lift = false;
        let mut cur = node.id;
        while chain.len() <= n {
            let ci = cur.index();
            if let Some(lambda) = lift_from[ci] {
                via_containment_lift = true;
                chain.push(lambda);
                cur = lambda;
                continue;
            }
            let Some(d) = &via_of[ci] else { break };
            min_confidence_on_path = min_confidence_on_path.weakest(d.via.confidence);
            chain.push(d.via.dst);
            cur = d.via.dst;
        }
        if via_containment_lift {
            // Containment is not invocation (§2): clamp, never claim better.
            min_confidence_on_path = min_confidence_on_path.weakest(Confidence::Possible);
        }

        // A lifted row has no discovery edge of its own; it inherits the one
        // that reached the lambda. A row for a test that is *itself* in the
        // changed set has none at all — see `changed_symbol_marker`.
        let (via, exception_transient) = {
            let mut c = node.id;
            loop {
                let ci = c.index();
                if let Some(d) = &via_of[ci] {
                    break (d.via.clone(), d.exception_transient);
                }
                match lift_from[ci] {
                    Some(lambda) => c = lambda,
                    None => break (crate::impacted::changed_symbol_marker(node.id), false),
                }
            }
        };

        let root = root_of[i].unwrap_or(node.id);
        debug_assert_eq!(chain.last().copied(), Some(root));
        rows.push((
            NeighborResult {
                node: node.clone(),
                depth: (chain.len() - 1) as u32,
                condition: via.condition,
                confidence: via.confidence,
                via,
                min_confidence_on_path,
                exception_transient,
            },
            ImpactedWitness {
                root,
                chain,
                via_containment_lift,
            },
        ));
    }

    rows.sort_by(|a, b| result_key(&a.0.node).cmp(&result_key(&b.0.node)));
    let lifted = rows.iter().filter(|(_, w)| w.via_containment_lift).count();
    let (tests, witnesses): (Vec<_>, Vec<_>) = rows.into_iter().unzip();

    ImpactedTests {
        tests,
        witnesses,
        changed_symbols: seeds.len(),
        file_granular_only: 0,
        lifted,
        langs_in_play,
        reached,
    }
}
