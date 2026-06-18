//! Store-level identifiers and the linked-graph value (architecture §3).
//!
//! These types name the things the store persists. They live here rather than in
//! `cgx-core` because they are storage concepts (git OIDs, dense graph ids, the
//! materialized node/edge/candidate triple) rather than graph-model primitives.
//! `cgx-frontend`'s `FileFacts` is *not* referenced: a Layer-1 fragment is stored
//! as opaque canonical postcard bytes that the indexer produced, so the store
//! never depends on the frontend crate (dependency direction `core ← store`).

use cgx_core::{Candidate, EdgeRecord, NodeRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A git blob OID (hex sha1 or sha256). The content-addressed key of a Layer-1
/// fragment: an unchanged blob re-puts to a no-op (architecture §3, IX-1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlobOid(pub String);

impl BlobOid {
    pub fn new(oid: impl Into<String>) -> Self {
        BlobOid(oid.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for BlobOid {
    fn from(s: &str) -> Self {
        BlobOid(s.to_owned())
    }
}

/// A git tree OID. Layer-2 linked graphs are keyed by tree OID; branch tips that
/// share a tree share the graph row (architecture §3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TreeOid(pub String);

impl TreeOid {
    pub fn new(oid: impl Into<String>) -> Self {
        TreeOid(oid.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TreeOid {
    fn from(s: &str) -> Self {
        TreeOid(s.to_owned())
    }
}

/// Dense row id of a Layer-2 graph (`graphs.graph_id`). Opaque outside the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GraphId(pub i64);

/// A cached Layer-1 fragment as returned by [`crate::FactStore::fragment`].
///
/// `fragment` is the canonical postcard encoding of the file's facts, stored and
/// returned verbatim; the store treats it as opaque bytes. `frontend_version`
/// lets a caller invalidate a fragment when the producing frontend's extraction
/// rules change (architecture §3 `frontend_version`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFragment {
    pub blob: BlobOid,
    pub lang: String,
    pub frontend_version: u32,
    pub fragment: Vec<u8>,
}

/// The set of blob OIDs reachable from live refs, supplied to [`crate::FactStore::prune`]
/// (the result of the IX-5 `git rev-list --objects --all` cross-reference).
pub type BlobSet = BTreeSet<BlobOid>;

/// What a [`crate::FactStore::prune`] removed (IX-5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneStats {
    /// Layer-1 fragment rows deleted (blob OIDs not in the live set).
    pub fragments_removed: usize,
    /// Layer-2 graph rows deleted (graphs whose tree is no longer referenced).
    pub graphs_removed: usize,
    /// Whether the storage file was compacted (`--aggressive`, `VACUUM`).
    pub compacted: bool,
}

/// A linked call graph for one tree (architecture §3 Layer 2): the disposable
/// materialization the resolver produces from Layer-1 fragments.
///
/// The store persists this into the `nodes`/`edges`/`candidates` tables under a
/// `graph_id`, and reconstructs an identical value on read. Determinism is the
/// caller's responsibility: nodes must be in `cgx_core::sort` node order, edges in
/// edge order, candidates by `(group, rank, dst)`. The store preserves the order
/// it is given and round-trips it byte-for-byte.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkedGraph {
    pub nodes: Vec<NodeRecord>,
    pub edges: Vec<EdgeRecord>,
    pub candidates: Vec<Candidate>,
}

impl LinkedGraph {
    pub fn new(nodes: Vec<NodeRecord>, edges: Vec<EdgeRecord>, candidates: Vec<Candidate>) -> Self {
        LinkedGraph {
            nodes,
            edges,
            candidates,
        }
    }
}
