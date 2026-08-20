//! The Slice-1 shadow wiring: a [`FactStore`] that runs the content-addressed
//! [`ObjectStore`] **alongside** the incumbent [`SqliteStore`].
//!
//! Graphs and Layer-1 fragments are written to **both** stores on every index run
//! (the SQLite copy is the shadow kept only so parity can be checked), while the
//! query/read path is served **entirely from objects** — [`read_graph`],
//! [`graph_for`], and [`fragment`] never touch SQLite, so a production query
//! issues no SQL (convergence criterion 3). The three dataflow caches are homed in
//! the [`ObjectStore`]'s scoped, gitignored `.cgx/cache.db` (criterion 4); they are
//! read only at index time, never at query time.
//!
//! Both writes for one index run must land or the run fails: each write method
//! fans to SQLite first, then objects, and propagates the first error, so a
//! half-written shadow surfaces as a failed index rather than silent drift.
//!
//! [`read_graph`]: FactStore::read_graph
//! [`graph_for`]: FactStore::graph_for
//! [`fragment`]: FactStore::fragment

use crate::store_loc::{cgx_dir, db_path};
use cgx_store::store::{FnSummaryRow, FragmentInput};
use cgx_store::{
    BlobOid, BlobSet, CachedFragment, FactStore, LinkedGraph, ObjectStore, PruneStats, Result,
    SqliteStore, TreeOid,
};
use std::path::Path;

/// A [`FactStore`] that writes graphs/fragments to both the incumbent SQLite store
/// and the content-addressed object store, and serves every read from objects.
#[derive(Debug)]
pub struct ShadowStore {
    /// The incumbent, written in shadow so parity can be checked. Never read on the
    /// query path.
    sqlite: SqliteStore,
    /// The content-addressed store: the read source, and the home of the scoped
    /// dataflow-cache SQLite (`.cgx/cache.db`).
    objects: ObjectStore,
}

impl ShadowStore {
    /// Open both stores rooted at `repo_root`'s `.cgx/` directory: the incumbent at
    /// `.cgx/index.db`, the object store (objects, fragments, refs, and the scoped
    /// `cache.db`) under `.cgx/`.
    pub fn open(repo_root: &Path) -> Result<Self> {
        let sqlite = SqliteStore::open(db_path(repo_root))?;
        let objects = ObjectStore::open(cgx_dir(repo_root))?;
        Ok(ShadowStore { sqlite, objects })
    }
}

impl FactStore for ShadowStore {
    // ---- reads: object store only (no SQL on the query path) ------------------

    fn fragment(&self, blob: &BlobOid) -> Result<Option<CachedFragment>> {
        self.objects.fragment(blob)
    }

    fn graph_for(&self, tree: &TreeOid) -> Result<bool> {
        self.objects.graph_for(tree)
    }

    fn read_graph(&self, tree: &TreeOid) -> Result<LinkedGraph> {
        self.objects.read_graph(tree)
    }

    // ---- writes: both stores (SQLite first, then objects) ---------------------

    fn put_fragments(&mut self, batch: &[FragmentInput<'_>]) -> Result<()> {
        self.sqlite.put_fragments(batch)?;
        self.objects.put_fragments(batch)
    }

    fn put_graph(
        &mut self,
        tree: &TreeOid,
        created_rev: Option<&str>,
        g: &LinkedGraph,
    ) -> Result<()> {
        self.sqlite.put_graph(tree, created_rev, g)?;
        self.objects.put_graph(tree, created_rev, g)
    }

    fn prune(&mut self, live: &BlobSet, aggressive: bool) -> Result<PruneStats> {
        let stats = self.sqlite.prune(live, aggressive)?;
        self.objects.prune(live, aggressive)?;
        Ok(stats)
    }

    fn prune_graphs_except(&mut self, keep: &[&TreeOid]) -> Result<usize> {
        let removed = self.sqlite.prune_graphs_except(keep)?;
        self.objects.prune_graphs_except(keep)?;
        Ok(removed)
    }

    // ---- dataflow caches: the object store's scoped cache.db (index-time only) -

    fn load_fn_intraproc_cache(&self) -> Result<Vec<((String, String), String)>> {
        self.objects.load_fn_intraproc_cache()
    }

    fn put_fn_intraproc_cache(&mut self, rows: &[((String, String), String)]) -> Result<()> {
        self.objects.put_fn_intraproc_cache(rows)
    }

    fn put_summary_deps(&mut self, rows: &[(String, String)]) -> Result<()> {
        self.objects.put_summary_deps(rows)
    }

    fn load_fn_summaries(&self) -> Result<Vec<FnSummaryRow>> {
        self.objects.load_fn_summaries()
    }

    fn put_fn_summaries(&mut self, rows: &[FnSummaryRow]) -> Result<()> {
        self.objects.put_fn_summaries(rows)
    }
}
