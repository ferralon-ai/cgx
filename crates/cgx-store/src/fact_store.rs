//! The storage contract the indexer and query layers program against
//! ([`FactStore`]), kept apart from its backends so it is available on every
//! target — including `wasm32`, where the SQLite and object-store backends (and
//! their advisory file locking) are not compiled and the host owns persistence.

use crate::error::Result;
use crate::types::{BlobOid, BlobSet, CachedFragment, LinkedGraph, PruneStats, TreeOid};

/// One persisted IFDS summary row (v0.3 SC4): `((blob_oid, fn_fqn),
/// postcard(Vec<SummaryFact>))`. Aliased to keep the [`FactStore`] signatures
/// readable (clippy type-complexity).
pub type FnSummaryRow = ((String, String), Vec<u8>);

/// The Phase-1 persistence contract (architecture §3 interface sketch). WP-08
/// (index) and WP-09 (query) build on exactly this surface.
pub trait FactStore {
    /// Fetch a cached Layer-1 fragment by blob OID, or `None` if not stored.
    fn fragment(&self, blob: &BlobOid) -> Result<Option<CachedFragment>>;

    /// Store a batch of Layer-1 fragments. Re-putting an unchanged blob OID is a
    /// no-op (content-addressed incrementality, IX-1): identical `(blob, bytes)`
    /// leaves the row and its bytes unchanged. Each fragment's `bytes` is the
    /// canonical postcard encoding the indexer produced; the store keeps it
    /// verbatim.
    fn put_fragments(&mut self, batch: &[FragmentInput<'_>]) -> Result<()>;

    /// Whether a Layer-2 graph is linked for `tree`. An existence predicate: the
    /// tree OID is itself the read key, so callers gate a [`Self::read_graph`] on
    /// this rather than threading a storage id.
    fn graph_for(&self, tree: &TreeOid) -> Result<bool>;

    /// Materialize a linked graph for a tree OID. If the tree is already linked,
    /// its rows are replaced atomically (the store's own id, if any, is reused
    /// internally).
    fn put_graph(
        &mut self,
        tree: &TreeOid,
        created_rev: Option<&str>,
        g: &LinkedGraph,
    ) -> Result<()>;

    /// Read back the linked graph stored under `tree`. The returned value is byte-
    /// for-byte identical to what was written (records reconstructed from canonical
    /// postcard `data`). A tree with no linked graph yields an empty [`LinkedGraph`].
    fn read_graph(&self, tree: &TreeOid) -> Result<LinkedGraph>;

    /// Remove Layer-1 fragments whose blob OID is not in `live`, and Layer-2
    /// graphs whose tree is no longer referenced (IX-5). With `aggressive`, also
    /// `VACUUM`s to reclaim disk.
    fn prune(&mut self, live: &BlobSet, aggressive: bool) -> Result<PruneStats>;

    /// Garbage-collect every Layer-2 graph except the ones in `keep` (their tree
    /// OIDs), returning the number of graph rows removed. Deletes from `graphs`;
    /// the FK `ON DELETE CASCADE` reclaims the dependent `nodes`/`edges`/
    /// `candidates` rows. No `VACUUM` — byte-determinism of the surviving rows is
    /// preserved (the blast-radius fix relies on this).
    ///
    /// The normal `cgx index` path keeps exactly the current tree's graph; the
    /// `diff` path, which accumulates two graphs in one session, never calls this.
    fn prune_graphs_except(&mut self, keep: &[&TreeOid]) -> Result<usize>;

    /// Load the entire per-function intraproc cache (v0.3 SC3): a map from
    /// `(blob_oid, fn_fqn)` to the function's `DataFlowFact`-subset content hash.
    /// Consulted before a dataflow re-link to decide which functions changed.
    fn load_fn_intraproc_cache(&self) -> Result<Vec<((String, String), String)>>;

    /// Replace the per-function intraproc cache with `rows` (v0.3 SC3). Each row
    /// is `((blob_oid, fn_fqn), facts_hash)`. Idempotent for an unchanged tree.
    fn put_fn_intraproc_cache(&mut self, rows: &[((String, String), String)]) -> Result<()>;

    /// Replace the `summary_deps` relation with `rows` (v0.3 SC3): each row is
    /// `(fn_fqn, callee_fqn)`, where `callee_fqn == "*"` is the wildcard.
    fn put_summary_deps(&mut self, rows: &[(String, String)]) -> Result<()>;

    /// Load the entire per-function IFDS summary cache (v0.3 SC4): a map from
    /// `(blob_oid, fn_fqn)` to the function's postcard-encoded `Vec<SummaryFact>`
    /// bytes. Consulted before a dataflow re-link so callees whose facts are
    /// unchanged need not be re-summarized.
    fn load_fn_summaries(&self) -> Result<Vec<FnSummaryRow>>;

    /// Replace the per-function IFDS summary cache with `rows` (v0.3 SC4). Each
    /// row is `((blob_oid, fn_fqn), summary_bytes)`. Replace semantics, matching
    /// [`Self::put_fn_intraproc_cache`]; idempotent for an unchanged tree.
    fn put_fn_summaries(&mut self, rows: &[FnSummaryRow]) -> Result<()>;
}

/// One fragment to store: the content-addressed key plus the canonical bytes.
#[derive(Debug, Clone, Copy)]
pub struct FragmentInput<'a> {
    pub blob: &'a BlobOid,
    pub lang: &'a str,
    pub frontend_version: u32,
    pub bytes: &'a [u8],
}
