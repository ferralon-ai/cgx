//! A read-through, in-memory write layer over another [`FactStore`].
//!
//! [`OverlayStore`] answers reads from its own memory first and from the base
//! store second, and keeps every write in memory. The base is borrowed shared, so
//! nothing an overlay run produces can reach it: this is what lets a working-tree
//! index reuse the persistent Layer-1 fragment cache (unchanged files are cache
//! hits) while its own fragments and graph never touch disk (ADR-06).
//!
//! The dataflow caches are memory-only and start empty rather than reading
//! through: they only classify functions as recomputed or reused for telemetry,
//! never change the linked graph, and loading the base's would cost a full table
//! read per call.

use crate::error::Result;
use crate::object_store::check_canonical;
use crate::store::{FactStore, FnSummaryRow, FragmentInput};
use crate::types::{CachedFragment, FragmentKey, FragmentKeySet, LinkedGraph, PruneStats, TreeOid};
use std::collections::BTreeMap;

/// A [`FactStore`] whose writes live in memory and whose reads fall through to
/// `base`. Dropping it discards everything written to it.
#[derive(Debug)]
pub struct OverlayStore<'a, B: FactStore> {
    base: &'a B,
    fragments: BTreeMap<FragmentKey, CachedFragment>,
    graphs: BTreeMap<TreeOid, LinkedGraph>,
    fn_intraproc_cache: Vec<((String, String), String)>,
    fn_summaries: Vec<FnSummaryRow>,
}

impl<'a, B: FactStore> OverlayStore<'a, B> {
    pub fn new(base: &'a B) -> Self {
        OverlayStore {
            base,
            fragments: BTreeMap::new(),
            graphs: BTreeMap::new(),
            fn_intraproc_cache: Vec::new(),
            fn_summaries: Vec::new(),
        }
    }

    /// Move a graph written to this overlay out of it, without the copy
    /// [`FactStore::read_graph`] makes. `None` if the overlay holds no graph for
    /// `tree` (the base is not consulted).
    pub fn take_graph(&mut self, tree: &TreeOid) -> Option<LinkedGraph> {
        self.graphs.remove(tree)
    }
}

impl<B: FactStore> FactStore for OverlayStore<'_, B> {
    fn fragment(&self, key: &FragmentKey) -> Result<Option<CachedFragment>> {
        match self.fragments.get(key) {
            Some(f) => Ok(Some(f.clone())),
            None => self.base.fragment(key),
        }
    }

    fn put_fragments(&mut self, batch: &[FragmentInput<'_>]) -> Result<()> {
        for f in batch {
            self.fragments.insert(
                f.key.clone(),
                CachedFragment {
                    key: f.key.clone(),
                    lang: f.lang.to_owned(),
                    frontend_version: f.frontend_version,
                    fragment: f.bytes.to_vec(),
                },
            );
        }
        Ok(())
    }

    fn graph_for(&self, tree: &TreeOid) -> Result<bool> {
        Ok(self.graphs.contains_key(tree) || self.base.graph_for(tree)?)
    }

    fn put_graph(
        &mut self,
        tree: &TreeOid,
        _created_rev: Option<&str>,
        g: &LinkedGraph,
    ) -> Result<()> {
        // The same contract the on-disk store enforces, so a graph is accepted
        // here exactly when it would be accepted there.
        check_canonical(g)?;
        self.graphs.insert(tree.clone(), g.clone());
        Ok(())
    }

    fn read_graph(&self, tree: &TreeOid) -> Result<LinkedGraph> {
        match self.graphs.get(tree) {
            Some(g) => Ok(g.clone()),
            None => self.base.read_graph(tree),
        }
    }

    fn prune(&mut self, live: &FragmentKeySet, _aggressive: bool) -> Result<PruneStats> {
        let before = self.fragments.len();
        self.fragments.retain(|k, _| live.contains(k));
        Ok(PruneStats {
            fragments_removed: before - self.fragments.len(),
            ..PruneStats::default()
        })
    }

    fn prune_graphs_except(&mut self, keep: &[&TreeOid]) -> Result<usize> {
        let before = self.graphs.len();
        self.graphs.retain(|k, _| keep.contains(&k));
        Ok(before - self.graphs.len())
    }

    fn load_fn_intraproc_cache(&self) -> Result<Vec<((String, String), String)>> {
        Ok(self.fn_intraproc_cache.clone())
    }

    fn put_fn_intraproc_cache(&mut self, rows: &[((String, String), String)]) -> Result<()> {
        self.fn_intraproc_cache = rows.to_vec();
        Ok(())
    }

    fn put_summary_deps(&mut self, _rows: &[(String, String)]) -> Result<()> {
        Ok(())
    }

    fn load_fn_summaries(&self) -> Result<Vec<FnSummaryRow>> {
        Ok(self.fn_summaries.clone())
    }

    fn put_fn_summaries(&mut self, rows: &[FnSummaryRow]) -> Result<()> {
        self.fn_summaries = rows.to_vec();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ObjectStore;

    fn input<'a>(key: &'a FragmentKey, bytes: &'a [u8]) -> FragmentInput<'a> {
        FragmentInput {
            key,
            lang: "rust",
            frontend_version: 1,
            bytes,
        }
    }

    fn file_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                if e.file_type().unwrap().is_dir() {
                    file_count(&e.path())
                } else {
                    1
                }
            })
            .sum()
    }

    #[test]
    fn reads_fall_through_and_writes_never_reach_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let mut base = ObjectStore::open(dir.path()).unwrap();
        let committed = FragmentKey::derive(b"committed");
        base.put_fragments(&[input(&committed, b"base")]).unwrap();
        let files_before = file_count(dir.path());

        let mut overlay = OverlayStore::new(&base);
        assert_eq!(
            overlay.fragment(&committed).unwrap().unwrap().fragment,
            b"base"
        );

        let dirty = FragmentKey::derive(b"dirty");
        overlay.put_fragments(&[input(&dirty, b"overlay")]).unwrap();
        let tree = TreeOid::new("workdir:abc");
        overlay
            .put_graph(&tree, None, &LinkedGraph::default())
            .unwrap();
        assert!(overlay.fragment(&dirty).unwrap().is_some());
        assert!(overlay.graph_for(&tree).unwrap());
        assert_eq!(overlay.take_graph(&tree), Some(LinkedGraph::default()));
        assert_eq!(overlay.take_graph(&tree), None);
        drop(overlay);

        assert!(base.fragment(&dirty).unwrap().is_none());
        assert!(!base.graph_for(&tree).unwrap());
        assert_eq!(
            file_count(dir.path()),
            files_before,
            "the base directory is untouched"
        );
    }
}
