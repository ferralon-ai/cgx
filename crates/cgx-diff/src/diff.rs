//! Graph diff (IX-4): classify the edges and nodes of two indexed trees as
//! added / removed / changed, keyed on `cgx-core`'s stable [`EdgeIdentity`].
//!
//! The diff operates on two [`LinkedGraph`]s — the disposable Layer-2
//! materializations the store reads back per tree OID. It never re-parses: a
//! diff is a pure set operation over two already-stored graph snapshots
//! (docs/06). Because the identity anchors a call edge to its caller FQN +
//! lexical ordinal rather than a raw line number, pure-formatting moves do not
//! churn the diff; the line is reported separately on each side.
//!
//! Determinism (architecture §4): every result list is sorted by a total key
//! ([`EdgeIdentity`] is `Ord`, node names are strings), so two diffs of the same
//! pair of graphs are byte-identical.

use cgx_core::{EdgeIdentity, EdgeRecord, NodeRecord};
use cgx_store::LinkedGraph;
use std::collections::BTreeMap;

/// The stable identity of an edge together with the per-side facts a diff
/// reports. `line` is the caller's defining line on that side (reported, not part
/// of the identity), so a reader can locate the edge without the identity
/// churning on formatting moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEdge {
    pub identity: EdgeIdentity,
    pub record: EdgeRecord,
    /// The caller symbol's start line on the side this edge came from.
    pub line: u32,
}

/// A changed edge: same [`EdgeIdentity`] on both sides, but one or more reported
/// attributes differ. The pair of records lets a caller render exactly what
/// changed (e.g. `condition` or `confidence`, the architecture's named cases).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedEdge {
    pub identity: EdgeIdentity,
    pub base: EdgeRecord,
    pub head: EdgeRecord,
    /// The attributes that differ between `base` and `head`.
    pub changes: EdgeChanges,
}

/// Which reported attributes of an edge changed between base and head. A change
/// is recorded only for attributes that are part of an edge's *meaning*, not its
/// identity (identity equality is the precondition for a `ChangedEdge`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeChanges {
    pub condition: bool,
    pub confidence: bool,
    pub tier: bool,
    pub candidate_group: bool,
    pub cfg_condition: bool,
}

impl EdgeChanges {
    /// Whether any attribute changed.
    pub fn any(self) -> bool {
        self.condition || self.confidence || self.tier || self.candidate_group || self.cfg_condition
    }
}

fn classify_change(base: &EdgeRecord, head: &EdgeRecord) -> EdgeChanges {
    EdgeChanges {
        condition: base.condition != head.condition,
        confidence: base.confidence != head.confidence,
        tier: base.tier != head.tier,
        candidate_group: base.candidate_group != head.candidate_group,
        cfg_condition: base.cfg_condition != head.cfg_condition,
    }
}

/// A symbol node that exists on exactly one side of a diff, identified by FQN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffNode {
    pub fqn: String,
    pub record: NodeRecord,
}

/// The full result of diffing two graphs (`base` → `head`).
///
/// Every list is in deterministic order: edge lists by [`EdgeIdentity`], node
/// lists by FQN.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphDiff {
    /// Edges present at head but not base (identity not in base).
    pub added_edges: Vec<DiffEdge>,
    /// Edges present at base but not head (identity not in head).
    pub removed_edges: Vec<DiffEdge>,
    /// Edges whose identity is on both sides but whose reported attributes differ.
    pub changed_edges: Vec<ChangedEdge>,
    /// Symbols (by FQN) present at head but not base.
    pub added_nodes: Vec<DiffNode>,
    /// Symbols (by FQN) present at base but not head.
    pub removed_nodes: Vec<DiffNode>,
}

impl GraphDiff {
    /// Whether the two graphs are identical at the diff's resolution (no added,
    /// removed, or changed edges or nodes). The natural CI gate predicate.
    pub fn is_empty(&self) -> bool {
        self.added_edges.is_empty()
            && self.removed_edges.is_empty()
            && self.changed_edges.is_empty()
            && self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
    }
}

/// Build the stable identity of an edge within `graph`. Returns `None` only if an
/// endpoint id is out of range (a corrupt graph); a well-formed stored graph
/// always resolves. The anchor ordinal is the edge's `stmt_index` (its lexical
/// position within the caller), and the file is the caller symbol's file.
fn identity_of(graph: &LinkedGraph, edge: &EdgeRecord) -> Option<(EdgeIdentity, u32)> {
    let src = graph.nodes.get(edge.src.index())?;
    let dst = graph.nodes.get(edge.dst.index())?;
    let id = EdgeIdentity::new(
        src.fqn.clone(),
        dst.fqn.clone(),
        edge,
        src.file.clone(),
        edge.stmt_index,
    );
    Some((id, src.line_start))
}

/// Index a graph's edges by stable identity. When two edges share an identity
/// (a candidate set's parallel edges differ only by `candidate_group`, which IS
/// part of the changed-attribute set but not the identity), the last in
/// canonical edge order wins — deterministic because the graph's edges arrive in
/// canonical sort order from the store.
fn index_edges(graph: &LinkedGraph) -> BTreeMap<EdgeIdentity, (&EdgeRecord, u32)> {
    let mut map = BTreeMap::new();
    for edge in &graph.edges {
        if let Some((id, line)) = identity_of(graph, edge) {
            map.insert(id, (edge, line));
        }
    }
    map
}

fn index_nodes(graph: &LinkedGraph) -> BTreeMap<&str, &NodeRecord> {
    let mut map = BTreeMap::new();
    for node in &graph.nodes {
        map.insert(node.fqn.as_str(), node);
    }
    map
}

/// Diff two already-linked graphs (`base` → `head`).
///
/// Classifies every edge by its [`EdgeIdentity`]: present only at head = added,
/// only at base = removed, on both with differing reported attributes = changed.
/// Symbol nodes are diffed by FQN. The result is fully deterministic.
pub fn diff_graphs(base: &LinkedGraph, head: &LinkedGraph) -> GraphDiff {
    let base_edges = index_edges(base);
    let head_edges = index_edges(head);

    let mut added_edges = Vec::new();
    let mut removed_edges = Vec::new();
    let mut changed_edges = Vec::new();

    for (id, (h_edge, h_line)) in &head_edges {
        match base_edges.get(id) {
            None => added_edges.push(DiffEdge {
                identity: id.clone(),
                record: (*h_edge).clone(),
                line: *h_line,
            }),
            Some((b_edge, _)) => {
                let changes = classify_change(b_edge, h_edge);
                if changes.any() {
                    changed_edges.push(ChangedEdge {
                        identity: id.clone(),
                        base: (*b_edge).clone(),
                        head: (*h_edge).clone(),
                        changes,
                    });
                }
            }
        }
    }
    for (id, (b_edge, b_line)) in &base_edges {
        if !head_edges.contains_key(id) {
            removed_edges.push(DiffEdge {
                identity: id.clone(),
                record: (*b_edge).clone(),
                line: *b_line,
            });
        }
    }

    let base_nodes = index_nodes(base);
    let head_nodes = index_nodes(head);
    let mut added_nodes = Vec::new();
    let mut removed_nodes = Vec::new();
    for (fqn, node) in &head_nodes {
        if !base_nodes.contains_key(fqn) {
            added_nodes.push(DiffNode {
                fqn: (*fqn).to_owned(),
                record: (*node).clone(),
            });
        }
    }
    for (fqn, node) in &base_nodes {
        if !head_nodes.contains_key(fqn) {
            removed_nodes.push(DiffNode {
                fqn: (*fqn).to_owned(),
                record: (*node).clone(),
            });
        }
    }

    // BTreeMap iteration already yields identity/FQN order; the vecs inherit it.
    GraphDiff {
        added_edges,
        removed_edges,
        changed_edges,
        added_nodes,
        removed_nodes,
    }
}
