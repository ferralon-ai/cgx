//! Deterministic ordering rules (architecture §3, §4).
//!
//! Determinism is a hard constraint: no `HashMap` iteration order ever leaks
//! into output. These functions define the canonical orders the rest of the
//! system sorts by before encoding or formatting.

use crate::edge::EdgeRecord;
use crate::id::NodeSortKey;
use crate::node::NodeRecord;
use serde::{Deserialize, Serialize};

/// The canonical node order: `(file, line_start, fqn)` (architecture §3).
///
/// Sorting symbol nodes by this key and assigning dense positions is the
/// [`crate::id::NodeId`] scheme. The key is borrowed to avoid allocating during
/// the sort.
pub fn node_sort_key(node: &NodeRecord) -> (&str, u32, &str) {
    (node.file.as_str(), node.line_start, node.fqn.as_str())
}

/// Build an owned [`NodeSortKey`] for a node (for callers that need to retain
/// the key beyond the node's borrow).
pub fn node_sort_key_owned(node: &NodeRecord) -> NodeSortKey {
    NodeSortKey::new(node.file.clone(), node.line_start, node.fqn.clone())
}

/// Sort symbol nodes into canonical order in place. Stable so equal-key nodes
/// keep input order (input is itself canonicalized upstream).
pub fn sort_nodes(nodes: &mut [NodeRecord]) {
    nodes.sort_by(|a, b| node_sort_key(a).cmp(&node_sort_key(b)));
}

/// The canonical edge order within a linked graph: by endpoints, then kind, then
/// site/provenance position. Independent of insertion order, so two index runs
/// of the same tree produce identical [`crate::id::EdgeId`] assignments.
pub fn edge_sort_key(edge: &EdgeRecord) -> (u32, u32, u8, u32, u32, u32) {
    (
        edge.src.0,
        edge.dst.0,
        edge.kind as u8,
        edge.stmt_index.unwrap_or(u32::MAX),
        edge.candidate_group.unwrap_or(u32::MAX),
        edge.id.0,
    )
}

/// Sort edges into canonical order in place.
pub fn sort_edges(edges: &mut [EdgeRecord]) {
    edges.sort_by_key(edge_sort_key);
}

/// A stable, cross-graph edge identity for `cgx diff` (architecture §4).
///
/// Keyed on `(src_fqn, dst_fqn, edge_kind, file, call-site anchor)` where the
/// anchor is the surrounding-symbol FQN plus the lexical ordinal rather than a
/// raw line number, so pure-formatting moves do not churn the diff. Line is
/// reported separately, not part of the identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeIdentity {
    pub src_fqn: String,
    pub dst_fqn: String,
    pub edge_kind: crate::edge::EdgeKind,
    pub file: String,
    /// Lexical ordinal of the call site within the source symbol (the anchor).
    /// `None` for structural edges with no call site.
    pub anchor_ordinal: Option<u32>,
}

impl EdgeIdentity {
    /// Build the stable identity for an edge, given the resolved FQNs of its
    /// endpoints and the originating file/anchor. The caller supplies the FQNs
    /// because the edge record stores dense ids, not names.
    pub fn new(
        src_fqn: impl Into<String>,
        dst_fqn: impl Into<String>,
        edge: &EdgeRecord,
        file: impl Into<String>,
        anchor_ordinal: Option<u32>,
    ) -> Self {
        EdgeIdentity {
            src_fqn: src_fqn.into(),
            dst_fqn: dst_fqn.into(),
            edge_kind: edge.kind,
            file: file.into(),
            anchor_ordinal,
        }
    }
}

/// The default query-result order (IF-8): `(file, line, col)`.
///
/// Returns a comparison key from a `(file, line, col)` triple, with missing
/// columns sorting before present ones deterministically.
pub fn result_order_key(file: &str, line: u32, col: Option<u32>) -> (&str, u32, u32) {
    (file, line, col.unwrap_or(0))
}
