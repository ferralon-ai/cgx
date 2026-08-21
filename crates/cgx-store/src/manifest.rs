//! The per-tree **manifest**: the content-addressed root object that names every
//! shard (and the candidates blob) of one linked graph, in canonical order.
//!
//! The manifest is encoded with `cgx_core::codec::encode` exactly like every other
//! unit, so it is itself content-addressed and byte-stable: two puts of the same
//! [`crate::LinkedGraph`] under the same key produce the same manifest OID. It is
//! the single object a `graph_key` ref points at, and advancing that ref is the
//! store's linearization point (git-ref-style CAS).

use crate::oid::ObjectOid;
use serde::{Deserialize, Serialize};

/// One shard entry: the owning-function key and the OID of the object holding that
/// function's nodes + edges. The list is sorted by `fn_key` so the manifest bytes
/// are deterministic (no hash-order iteration).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardEntry {
    /// The owning-function key (see `object_store::owning_key`) this shard holds.
    pub fn_key: String,
    /// OID of the shard object (`Shard` postcard bytes).
    pub oid: ObjectOid,
}

/// The manifest object for one graph. Serialized canonically (fields in a fixed
/// order; `shards` sorted by `fn_key`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The Layer-2 key: a tree OID **or** a `workdir:<digest>` synthetic key
    /// (recon §B — not always a tree OID).
    pub graph_key: String,
    /// The revision that produced this graph, mirrored from `put_graph`.
    pub created_rev: Option<String>,
    /// Shard objects, one per owning-function key, sorted by `fn_key`.
    pub shards: Vec<ShardEntry>,
    /// OID of the per-tree candidates object, or `None` when the graph has no
    /// candidates.
    pub candidates: Option<ObjectOid>,
}

impl Manifest {
    /// Every object OID this manifest transitively references (its shards and the
    /// candidates blob) — the manifest's own OID excluded. Used by GC to compute
    /// the live set.
    pub fn referenced_oids(&self) -> impl Iterator<Item = &ObjectOid> {
        self.shards
            .iter()
            .map(|s| &s.oid)
            .chain(self.candidates.iter())
    }
}
