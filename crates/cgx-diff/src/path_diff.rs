//! Path-level diff (CH-11 S1): the structural PR gate.
//!
//! Where [`crate::diff`] classifies individual *edges* as added/removed, this
//! module answers the path-level question CH-11 is built on:
//!
//! > Does a **path** from any node matching `--from` to any node matching `--to`
//! > exist at HEAD that did **not** exist at BASE?
//!
//! This is **call/dataflow reachability over the resolved graph** — NOT a
//! soundness or security guarantee. A reported path means "HEAD admits a
//! call/dataflow route from the source to the sink that BASE did not"; it does not
//! prove the route is exercised at runtime, nor that data of any particular kind
//! flows along it (that is typed taint, a later slice). The honesty framing is
//! load-bearing: the gate reports *reachability*, and the CLI help/output say so.
//!
//! ## Boundedness (the hard guard)
//!
//! An unanchored path search over a dense dataflow graph blows up
//! (71s / 1.87M rows on the C-health corpus). Two defenses:
//!
//! 1. **Both anchors required.** A path diff needs *both* a `from` and a `to`
//!    pattern. The caller (CLI) rejects a missing anchor before reaching here; this
//!    module's entry point takes both patterns by value so an unanchored call is
//!    unrepresentable.
//! 2. **Step budget.** Reachability runs through the shipped [`PathWalker`], whose
//!    work is capped at [`DEFAULT_MAX_STEPS`] DFS node-visits. Each (source, sink)
//!    probe is a single bounded BFS shortest-path search.

use cgx_core::{EdgeKind, SymbolPattern};
use cgx_query::{reaches, EdgeFilter, GraphView, PathWalker, DEFAULT_MAX_STEPS};
use cgx_store::LinkedGraph;

/// One newly-introduced source→sink path (present at HEAD, absent at BASE).
///
/// The `via` witness is the shortest HEAD path's intermediate FQNs (inclusive of
/// the source and sink), so the caller can render the route and attribute the
/// introducing commit off the source symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedPath {
    /// The matched source symbol's FQN (a concrete node, not the glob).
    pub from_fqn: String,
    /// The matched sink symbol's FQN.
    pub to_fqn: String,
    /// The shortest witnessing path at HEAD as a sequence of node FQNs, from
    /// `from_fqn` to `to_fqn` inclusive.
    pub via: Vec<String>,
}

/// The result of a path-added diff: every newly-introduced source→sink path, in
/// deterministic `(from_fqn, to_fqn)` order, plus a flag noting whether any side's
/// reachability search hit the work budget (so the gate can report "incomplete").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathDiff {
    /// New paths present at HEAD but not BASE, in `(from_fqn, to_fqn)` order.
    pub added_paths: Vec<AddedPath>,
}

impl PathDiff {
    /// Whether the gate found a forbidden new path (the nonzero-exit predicate).
    pub fn has_new_path(&self) -> bool {
        !self.added_paths.is_empty()
    }
}

/// The edge filter the path gate reaches over: call-family edges **plus**
/// `DerivesFrom` dataflow edges. CH-11 is "call/dataflow reachability", so a path
/// may thread through both a call chain and a value-flow chain. Confidence floor
/// is `Possible` (admit all) — the gate's job is to surface *any* new route; the
/// caller can tighten later.
fn reachability_filter() -> EdgeFilter {
    // Call-family kinds (per `EdgeKind::is_call`) plus the dataflow edge.
    EdgeFilter::default().with_kinds(vec![
        EdgeKind::Calls,
        EdgeKind::CallsVirtual,
        EdgeKind::CallsClosure,
        EdgeKind::CallsCallback,
        EdgeKind::CallsAsync,
        EdgeKind::CallsIndirect,
        EdgeKind::Spawns,
        EdgeKind::DerivesFrom,
    ])
}

/// The set of `(from_fqn, to_fqn)` reachable pairs in `view`, plus a witness path
/// for each, restricted to sources matching `from` and sinks matching `to`.
///
/// Bounded: each probe is a single [`PathWalker::shortest_path`]-backed
/// reachability search capped at `max_steps`. Pairs are produced in canonical node
/// order (the view yields nodes in ascending id = `(file, line, fqn)` order), so
/// the output is deterministic.
fn reachable_pairs(
    view: &GraphView,
    from: &SymbolPattern,
    to: &SymbolPattern,
    max_steps: u64,
) -> Vec<AddedPath> {
    let sources: Vec<_> = view
        .nodes()
        .iter()
        .filter(|n| from.matches(n))
        .map(|n| n.id)
        .collect();
    let sinks: Vec<_> = view
        .nodes()
        .iter()
        .filter(|n| to.matches(n))
        .map(|n| n.id)
        .collect();

    let walker = PathWalker {
        filter: reachability_filter(),
        max_depth: None,
        max_paths: None,
        max_steps: Some(max_steps),
    };

    let mut out = Vec::new();
    for &src in &sources {
        for &dst in &sinks {
            if src == dst {
                continue;
            }
            let result = reaches(view, src, dst, &walker);
            if result.reachable {
                let via = result
                    .witness
                    .as_ref()
                    .map(|w| w.steps.iter().map(|s| s.node.fqn.clone()).collect())
                    .unwrap_or_default();
                out.push(AddedPath {
                    from_fqn: view.node(src).fqn.clone(),
                    to_fqn: view.node(dst).fqn.clone(),
                    via,
                });
            }
        }
    }
    out
}

/// Compute the path-added diff between two already-linked graphs.
///
/// Reports every `(from, to)` pair that is reachable at `head` but not at `base`,
/// where `from`/`to` are FQN-glob patterns. Both anchors are required by type (the
/// function cannot be called unanchored). Bounded by the default step budget.
pub fn path_diff_graphs(
    base: &LinkedGraph,
    head: &LinkedGraph,
    from: &SymbolPattern,
    to: &SymbolPattern,
) -> PathDiff {
    path_diff_graphs_bounded(base, head, from, to, DEFAULT_MAX_STEPS)
}

/// [`path_diff_graphs`] with an explicit step budget (used by tests to force or
/// relax the bound).
pub fn path_diff_graphs_bounded(
    base: &LinkedGraph,
    head: &LinkedGraph,
    from: &SymbolPattern,
    to: &SymbolPattern,
    max_steps: u64,
) -> PathDiff {
    let base_view = GraphView::new(base.nodes.clone(), base.edges.clone(), base.candidates.clone());
    let head_view = GraphView::new(head.nodes.clone(), head.edges.clone(), head.candidates.clone());

    let base_pairs: std::collections::BTreeSet<(String, String)> =
        reachable_pairs(&base_view, from, to, max_steps)
            .into_iter()
            .map(|p| (p.from_fqn, p.to_fqn))
            .collect();

    let mut added_paths: Vec<AddedPath> = reachable_pairs(&head_view, from, to, max_steps)
        .into_iter()
        .filter(|p| !base_pairs.contains(&(p.from_fqn.clone(), p.to_fqn.clone())))
        .collect();
    added_paths.sort_by(|a, b| {
        (a.from_fqn.as_str(), a.to_fqn.as_str()).cmp(&(b.from_fqn.as_str(), b.to_fqn.as_str()))
    });

    PathDiff { added_paths }
}
