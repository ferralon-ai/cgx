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
