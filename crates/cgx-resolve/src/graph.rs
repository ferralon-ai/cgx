//! The resolver's output: a linked graph of resolved nodes and edges.

use cgx_core::cut::CutMarker;
use cgx_core::provenance::Span;
use cgx_core::{Candidate, EdgeRecord, EdgeWithProvenance, NodeRecord, NodeWithProvenance};

/// A reference the resolver could not bind to any definition (LS-6 honesty).
///
/// No edge is emitted for it — the call is left dangling — but it is recorded
/// here with a [`CutMarker::Unresolved`] marker plus its source location and the
/// name it was trying to resolve, so the index-quality report (`cgx doctor`,
/// WP-12) can surface unresolved-call hotspots. This is how the resolver "leaves
/// dangling honestly" rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnresolvedRef {
    /// FQN of the calling symbol (the edge source that would have been).
    pub caller_fqn: String,
    /// The name path the source wrote (`a.b.c` → `["a","b","c"]`).
    pub name_path: Vec<String>,
    /// Where the unresolved reference is.
    pub span: Span,
    /// Always [`CutMarker::Unresolved`]; carried for symmetry with edge markers.
    pub marker: CutMarker,
}

/// The linked graph the resolver produces (architecture §3 Layer 2).
///
/// Built from `cgx-core` records only — this crate never references `cgx-store`.
/// `nodes` are in canonical node-sort order with dense [`cgx_core::NodeId`]s;
/// `edges` are in canonical edge-sort order with dense [`cgx_core::EdgeId`]s;
/// `candidates` are sorted by `(group, rank, dst)`. Provenance travels alongside
/// each node and edge (GM-6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedGraph {
    /// Symbol nodes with provenance, in canonical sort order.
    pub nodes: Vec<NodeWithProvenance>,
    /// Call/structural edges with provenance, in canonical sort order.
    pub edges: Vec<EdgeWithProvenance>,
    /// Candidate-set members for over-approximated calls (GM-2.1).
    pub candidates: Vec<Candidate>,
    /// References that resolved to no target, recorded honestly (LS-6).
    pub unresolved: Vec<UnresolvedRef>,
    /// v0.3 SC3 incremental-dataflow products. Empty/zeroed unless the link ran
    /// with `dataflow` enabled and a prior cache was supplied.
    pub dataflow: DataflowOutput,
}

/// The v0.3 SC3 incremental-dataflow products of a link: per-function cache rows
/// to persist, the `summary_deps` relation, the set of functions whose intraproc
/// facts changed this run (own-dirty), and the telemetry counters.
///
/// The resolver computes the per-function content hash and the own-dirty set (a
/// function present in `summary_deps`/cache whose hash differs from the prior
/// cache). Transitive-caller propagation over `summary_deps` is the indexer's
/// job ([`crate::propagate_dirty`]); the final `stats` reflect that fold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataflowOutput {
    /// `((blob_oid, fn_fqn), facts_hash)` — the full cache to persist this run.
    pub fn_intraproc_cache: Vec<((String, String), String)>,
    /// `(fn_fqn, callee_fqn)` — the summary-dep relation; `"*"` is the wildcard.
    pub summary_deps: Vec<(String, String)>,
    /// Functions whose own intraproc facts changed vs. the prior cache (cache
    /// miss). Seeds the dirty set the indexer propagates over `summary_deps`.
    pub changed_fns: Vec<String>,
    /// Recompute/reuse telemetry. `functions_recomputed` is the own-dirty count at
    /// link time; the indexer raises it to include transitive callers.
    pub stats: DataflowStats,
    /// v0.3 SC4 IFDS per-function summaries to persist in `fn_summaries`:
    /// `((blob_oid, fn_fqn), postcard(Vec<SummaryFact>))`. Empty when no callee
    /// produced a summary. The blob_oid is the function's owning file blob.
    pub fn_summaries: Vec<((String, String), Vec<u8>)>,
    /// v0.3 SC4 IFDS telemetry: summaries computed, interproc edges materialized,
    /// SCCs that hit the work-budget cap.
    pub ifds_stats: IfdsDataflowStats,
}

/// v0.3 SC4 IFDS counters, surfaced through `DataflowOutput` + `IndexStats`
/// (decision E.3). A `Copy`/`Default` mirror of the resolver-internal `IfdsStats`
/// so the public surface needs no re-export of internal types.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IfdsDataflowStats {
    /// Functions for which at least one summary fact was computed.
    pub summaries_computed: usize,
    /// Interprocedural `DerivesFrom` edges materialized this run.
    pub summary_edges_materialized: usize,
    /// SCCs that hit the work-budget cap before reaching fixpoint.
    pub budget_exceeded_sccs: usize,
}

/// Recompute/reuse counters for one dataflow link (v0.3 SC3 telemetry).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataflowStats {
    /// Functions whose SSA + intraproc dataflow was (re)built this run.
    pub functions_recomputed: usize,
    /// Functions whose cached intraproc facts were reused (cache hit).
    pub functions_reused: usize,
}

/// Expand a seed dirty set to its transitive closure over `summary_deps`
/// (v0.3 SC3): if function `B` consumes callee `A`'s summary — a row
/// `(B, A)` — and `A` is dirty, then `B` becomes dirty too. The `"*"`
/// wildcard callee makes its dependent dirty whenever *any* function changed
/// (the conservative fallback for virtual/unresolved callees).
///
/// `summary_deps` is `(dependent_fn, callee_fqn)`. The traversal is plain graph
/// reachability over the reverse edges (callee → dependents) carrying a
/// **visited set**, so mutual recursion / cycles terminate. It is NOT a
/// `PathWalker` walk and does not consume `DEFAULT_MAX_STEPS`.
///
/// Returns the closed dirty set (seed ∪ transitive dependents), order-stable.
pub fn propagate_dirty(seed: &[String], summary_deps: &[(String, String)]) -> Vec<String> {
    use std::collections::{BTreeSet, HashMap, VecDeque};

    // Reverse adjacency: callee_fqn → dependents that consume its summary.
    let mut dependents_of: HashMap<&str, Vec<&str>> = HashMap::new();
    // Functions whose dep set includes the wildcard: dirty if anything changed.
    let mut wildcard_dependents: Vec<&str> = Vec::new();
    for (dependent, callee) in summary_deps {
        if callee == "*" {
            wildcard_dependents.push(dependent.as_str());
        } else {
            dependents_of
                .entry(callee.as_str())
                .or_default()
                .push(dependent.as_str());
        }
    }

    let mut dirty: BTreeSet<String> = seed.iter().cloned().collect();
    let mut queue: VecDeque<String> = seed.iter().cloned().collect();

    // Any real change activates every wildcard dependent up front; they then
    // propagate like any other dirty node.
    if !seed.is_empty() {
        for w in &wildcard_dependents {
            if dirty.insert(w.to_string()) {
                queue.push_back(w.to_string());
            }
        }
    }

    while let Some(node) = queue.pop_front() {
        if let Some(deps) = dependents_of.get(node.as_str()) {
            for dep in deps {
                if dirty.insert(dep.to_string()) {
                    queue.push_back(dep.to_string());
                }
            }
        }
    }

    dirty.into_iter().collect()
}

impl ResolvedGraph {
    /// Lower into the bare `(nodes, edges, candidates)` triple the indexer (WP-08)
    /// wraps in `cgx_store::LinkedGraph`. Provenance is dropped here because the
    /// store schema denormalizes `rule`/`tier` onto the edge record itself
    /// (architecture §3 `edges.rule`); the indexer persists fragments separately.
    pub fn into_linked(self) -> (Vec<NodeRecord>, Vec<EdgeRecord>, Vec<Candidate>) {
        (
            self.nodes.into_iter().map(|n| n.node).collect(),
            self.edges.into_iter().map(|e| e.edge).collect(),
            self.candidates,
        )
    }

    /// Borrowing view of the resolved symbol nodes.
    pub fn node_records(&self) -> impl Iterator<Item = &NodeRecord> {
        self.nodes.iter().map(|n| &n.node)
    }

    /// Borrowing view of the resolved edges.
    pub fn edge_records(&self) -> impl Iterator<Item = &EdgeRecord> {
        self.edges.iter().map(|e| &e.edge)
    }
}
