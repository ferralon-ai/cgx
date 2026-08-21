//! A content-addressed loose-object [`FactStore`] (Slice-1 Candidate A, shadow).
//!
//! Layer-2 linked graphs are sharded **per owning function** into content-addressed
//! objects (`.cgx/objects/<oid[0:2]>/<oid[2:]>`), so a one-function edit rewrites
//! only that function's object and every unchanged function keeps its OID (free
//! dedup across trees/branches). A per-tree **manifest** object names the shards in
//! canonical order; a git-ref-style pointer per `graph_key` names the current
//! manifest, and **advancing that pointer is the single linearization point**
//! (guarded by [`WriteLock`], analogous to the SQLite `tx.commit()`).
//!
//! Layer-1 fragments are stored as their own objects under `.cgx/fragments/`, keyed
//! by blob OID (IX-1: a re-put of an unchanged blob is a no-op).
//!
//! The three dataflow caches (`fn_intraproc_cache`, `summary_deps`, `fn_summaries`)
//! are **not** content-addressed — they want keyed upsert/delete — so they are
//! delegated to an internally-owned, scoped, gitignored [`SqliteStore`] at
//! `.cgx/cache.db`. That DB is index-time scratch and lives under the `.cgx`
//! `.gitignore`'s `*`; it is never part of the committed/transported object set.
//!
//! ## Write discipline (atomic-replace + CAS)
//!
//! Every object is written temp → `fsync` → atomic `rename` to its final OID path,
//! and is write-once (if the OID path already exists the bytes are identical by
//! construction, so the write is skipped). A half-written object never occupies its
//! final name, so torn *reads* are impossible. Shards and the candidates blob land
//! first; then the manifest object; then the ref pointer advances last. A reader
//! therefore never observes a manifest that references an object not yet fully
//! written.

use crate::error::{Result, StoreError};
use crate::lock::WriteLock;
use crate::manifest::{Manifest, ShardEntry, CURRENT_STORE_FORMAT};
use crate::oid::ObjectOid;
use crate::store::{FactStore, FnSummaryRow, FragmentInput, SqliteStore};
use crate::types::{BlobOid, BlobSet, CachedFragment, LinkedGraph, PruneStats, TreeOid};
use cgx_core::codec::{decode, encode};
use cgx_core::{Candidate, EdgeRecord, NodeRecord};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// One per-function shard: the nodes and edges attributed to a single owning
/// function. Encoded canonically (nodes sorted by `node_id`, edges by `edge_id`)
/// so its OID is byte-stable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Shard {
    nodes: Vec<NodeRecord>,
    edges: Vec<EdgeRecord>,
}

/// A stored Layer-1 fragment object: the frontend metadata plus the canonical
/// postcard fragment bytes, keyed on disk by blob OID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredFragment {
    lang: String,
    frontend_version: u32,
    bytes: Vec<u8>,
}

/// A content-addressed loose-object [`FactStore`].
#[derive(Debug)]
pub struct ObjectStore {
    objects_dir: PathBuf,
    fragments_dir: PathBuf,
    refs_dir: PathBuf,
    lock_path: PathBuf,
    /// Scoped, gitignored SQLite backing **only** the three dataflow caches.
    cache: SqliteStore,
}

impl ObjectStore {
    /// Open (creating the layout if absent) a content-addressed store rooted at
    /// `root` — normally the repo's `.cgx` directory. Objects live under
    /// `root/objects`, fragments under `root/fragments`, manifest pointers under
    /// `root/refs`, and the scoped cache DB at `root/cache.db`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let objects_dir = root.join("objects");
        let fragments_dir = root.join("fragments");
        let refs_dir = root.join("refs");
        fs::create_dir_all(&objects_dir)?;
        fs::create_dir_all(&fragments_dir)?;
        fs::create_dir_all(&refs_dir)?;
        let cache = SqliteStore::open(root.join("cache.db"))?;
        Ok(ObjectStore {
            objects_dir,
            fragments_dir,
            refs_dir,
            lock_path: root.join("objects.lock"),
            cache,
        })
    }

    /// The ref-pointer file naming the current manifest for `graph_key`. The file
    /// name is a stable hex digest of the key (keys may contain `:` and other
    /// filesystem-hostile characters, e.g. `workdir:<digest>`).
    fn ref_path(&self, graph_key: &str) -> PathBuf {
        self.refs_dir
            .join(ObjectOid::of_bytes(graph_key.as_bytes()).0)
    }

    /// Resolve `graph_key` to its live [`Manifest`], or `None` if no graph is
    /// linked. The ref file holds `<manifest_oid>\n<graph_key>\n`.
    fn resolve_manifest(&self, graph_key: &str) -> Result<Option<Manifest>> {
        let ref_path = self.ref_path(graph_key);
        let Some(contents) = read_if_exists(&ref_path)? else {
            return Ok(None);
        };
        let manifest_oid = contents
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let path = ObjectOid(manifest_oid).object_path(&self.objects_dir);
        let bytes = fs::read(&path)?;
        let manifest = decode::<Manifest>(&bytes)?;
        // Compat gate: a committed manifest from a newer cgx may carry a schema this
        // binary cannot decode faithfully (postcard is not self-describing). Reject
        // loudly — naming both versions — rather than silently mis-decode (D-3).
        if manifest.store_format > CURRENT_STORE_FORMAT {
            return Err(StoreError::StoreFormat {
                found: manifest.store_format,
                expected: CURRENT_STORE_FORMAT,
            });
        }
        Ok(Some(manifest))
    }

    /// Write `bytes` as a content-addressed object under `dir`, returning its OID.
    /// Write-once: if the OID path already exists the bytes are identical by
    /// construction and the write is skipped. Otherwise temp → `fsync` → atomic
    /// `rename` so a torn object never occupies its final name.
    fn write_object(&self, dir: &Path, bytes: &[u8]) -> Result<ObjectOid> {
        let oid = ObjectOid::of_bytes(bytes);
        let path = oid.object_path(dir);
        if path.exists() {
            return Ok(oid);
        }
        atomic_write(&path, bytes)?;
        Ok(oid)
    }

    fn all_object_oids(dir: &Path) -> Result<Vec<(ObjectOid, PathBuf)>> {
        let mut out = Vec::new();
        let Some(entries) = read_dir_if_exists(dir)? else {
            return Ok(out);
        };
        for shard_dir in entries {
            let prefix = shard_dir.file_name();
            let Some(prefix) = prefix.to_str() else {
                continue;
            };
            if prefix.len() != 2 {
                continue;
            }
            let Some(files) = read_dir_if_exists(&shard_dir.path())? else {
                continue;
            };
            for f in files {
                if let Some(rest) = f.file_name().to_str() {
                    if rest.ends_with(".tmp") {
                        continue;
                    }
                    out.push((ObjectOid(format!("{prefix}{rest}")), f.path()));
                }
            }
        }
        Ok(out)
    }
}

/// The owning-function key of `fqn`: the longest `::`-prefix (including `fqn`
/// itself) that is the FQN of a callable node. Value nodes (`<fn>::<local>#<ver>`)
/// and other members thus fold into their enclosing function; a symbol with no
/// callable ancestor (a top-level type/module/constant) keys to its own FQN. Pure
/// and deterministic — the whole attribution is derived from the sorted callable
/// set, never from hash iteration.
fn owning_key(fqn: &str, callables: &BTreeSet<&str>) -> String {
    let mut s = fqn;
    loop {
        if callables.contains(s) {
            return s.to_owned();
        }
        match s.rfind("::") {
            Some(i) => s = &s[..i],
            None => return fqn.to_owned(),
        }
    }
}

impl FactStore for ObjectStore {
    fn fragment(&self, blob: &BlobOid) -> Result<Option<CachedFragment>> {
        let path = ObjectOid(blob.0.clone()).object_path(&self.fragments_dir);
        let Some(bytes) = read_bytes_if_exists(&path)? else {
            return Ok(None);
        };
        let sf = decode::<StoredFragment>(&bytes)?;
        Ok(Some(CachedFragment {
            blob: blob.clone(),
            lang: sf.lang,
            frontend_version: sf.frontend_version,
            fragment: sf.bytes,
        }))
    }

    fn put_fragments(&mut self, batch: &[FragmentInput<'_>]) -> Result<()> {
        let _lock = WriteLock::acquire(&self.lock_path)?;
        for f in batch {
            let path = ObjectOid(f.blob.0.clone()).object_path(&self.fragments_dir);
            // Write-once by blob OID: an unchanged blob re-puts to a true no-op
            // (IX-1). A present key already holds the right bytes (blob OID is a
            // content hash), so no decode/compare is needed.
            if path.exists() {
                continue;
            }
            let stored = StoredFragment {
                lang: f.lang.to_owned(),
                frontend_version: f.frontend_version,
                bytes: f.bytes.to_vec(),
            };
            atomic_write(&path, &encode(&stored)?)?;
        }
        Ok(())
    }

    fn graph_for(&self, tree: &TreeOid) -> Result<bool> {
        Ok(self.ref_path(tree.as_str()).exists())
    }

    fn put_graph(&mut self, tree: &TreeOid, created_rev: Option<&str>, g: &LinkedGraph) -> Result<()> {
        let _lock = WriteLock::acquire(&self.lock_path)?;

        // Attribution: the callable FQNs, then each node's owning-function key.
        let callables: BTreeSet<&str> = g
            .nodes
            .iter()
            .filter(|n| n.kind.is_callable())
            .map(|n| n.fqn.as_str())
            .collect();

        let mut node_key: BTreeMap<u32, String> = BTreeMap::new();
        let mut shards: BTreeMap<String, Shard> = BTreeMap::new();
        for n in &g.nodes {
            let key = owning_key(&n.fqn, &callables);
            node_key.insert(n.id.0, key.clone());
            shards.entry(key).or_default().nodes.push(n.clone());
        }
        // Edges follow their source node's owning function. A src with no node
        // record (should not happen for a well-formed graph) falls back to a
        // synthetic key so the edge is never silently dropped (honesty spine).
        for e in &g.edges {
            let key = node_key
                .get(&e.src.0)
                .cloned()
                .unwrap_or_else(|| format!("\u{0}orphan\u{0}{}", e.src.0));
            shards.entry(key).or_default().edges.push(e.clone());
        }

        // Canonicalize each shard's contents and write it. `shards` is a BTreeMap,
        // so iteration is in `fn_key` order — the manifest's shard list is thereby
        // canonical and its OID deterministic.
        let mut shard_entries = Vec::with_capacity(shards.len());
        for (fn_key, mut shard) in shards {
            shard.nodes.sort_by_key(|n| n.id.0);
            shard.edges.sort_by_key(|e| e.id.0);
            let oid = self.write_object(&self.objects_dir, &encode(&shard)?)?;
            shard_entries.push(ShardEntry { fn_key, oid });
        }

        // Candidates: a single per-tree object, sorted (group, rank, dst) to match
        // SQLite's defensive read-order (D2).
        let candidates = if g.candidates.is_empty() {
            None
        } else {
            let mut cands = g.candidates.clone();
            cands.sort_by_key(|c| (c.candidate_group, c.rank, c.dst.0));
            Some(self.write_object(&self.objects_dir, &encode(&cands)?)?)
        };

        let manifest = Manifest {
            store_format: CURRENT_STORE_FORMAT,
            graph_key: tree.as_str().to_owned(),
            created_rev: created_rev.map(|s| s.to_owned()),
            shards: shard_entries,
            candidates,
        };
        let manifest_oid = self.write_object(&self.objects_dir, &encode(&manifest)?)?;

        // Linearization point: advance the ref last, atomically. Until this rename
        // lands, every object it names is already durably renamed into place.
        atomic_write(
            &self.ref_path(tree.as_str()),
            format!("{}\n{}\n", manifest_oid.as_str(), tree.as_str()).as_bytes(),
        )?;
        Ok(())
    }

    fn read_graph(&self, tree: &TreeOid) -> Result<LinkedGraph> {
        let Some(manifest) = self.resolve_manifest(tree.as_str())? else {
            return Ok(LinkedGraph::default());
        };

        let mut nodes: Vec<NodeRecord> = Vec::new();
        let mut edges: Vec<EdgeRecord> = Vec::new();
        for entry in &manifest.shards {
            let path = entry.oid.object_path(&self.objects_dir);
            let shard = decode::<Shard>(&fs::read(&path)?)?;
            nodes.extend(shard.nodes);
            edges.extend(shard.edges);
        }
        // Reconstruct the exact flat order SQLite returns: nodes by node_id, edges
        // by edge_id (dense + unique, so a sort is a total reconstruction).
        nodes.sort_by_key(|n| n.id.0);
        edges.sort_by_key(|e| e.id.0);

        let candidates = match &manifest.candidates {
            None => Vec::new(),
            Some(oid) => {
                let mut cands =
                    decode::<Vec<Candidate>>(&fs::read(oid.object_path(&self.objects_dir))?)?;
                // Defensive re-sort on read (D2 / recon §F.12.5): match SQLite's
                // `ORDER BY candidate_group, rank, dst` byte-for-byte.
                cands.sort_by_key(|c| (c.candidate_group, c.rank, c.dst.0));
                cands
            }
        };

        Ok(LinkedGraph {
            nodes,
            edges,
            candidates,
        })
    }

    fn prune(&mut self, live: &BlobSet, aggressive: bool) -> Result<PruneStats> {
        let _lock = WriteLock::acquire(&self.lock_path)?;
        let mut stats = PruneStats::default();
        for (oid, path) in Self::all_object_oids(&self.fragments_dir)? {
            // A fragment object's OID *is* its blob OID (fragments are keyed by
            // blob OID, not by their postcard-bytes hash).
            if !live.contains(&BlobOid(oid.0.clone())) {
                fs::remove_file(&path)?;
                stats.fragments_removed += 1;
            }
        }
        // Layer-2 graphs are GC'd only by `prune_graphs_except` (recon §A.1); this
        // method leaves `graphs_removed` at 0, matching SqliteStore.
        stats.compacted = aggressive;
        Ok(stats)
    }

    fn prune_graphs_except(&mut self, keep: &[&TreeOid]) -> Result<usize> {
        let _lock = WriteLock::acquire(&self.lock_path)?;
        let keep_keys: BTreeSet<&str> = keep.iter().map(|t| t.as_str()).collect();

        // Drop refs whose graph_key is not kept, counting removals.
        let mut removed = 0usize;
        let Some(refs) = read_dir_if_exists(&self.refs_dir)? else {
            return Ok(0);
        };
        for r in refs {
            let path = r.path();
            let Some(contents) = read_if_exists(&path)? else {
                continue;
            };
            let graph_key = contents.lines().nth(1).unwrap_or_default().trim();
            if !keep_keys.contains(graph_key) {
                fs::remove_file(&path)?;
                removed += 1;
            }
        }

        // Sweep: every object not referenced by a surviving manifest is dead.
        let mut live: HashSet<String> = HashSet::new();
        if let Some(refs) = read_dir_if_exists(&self.refs_dir)? {
            for r in refs {
                let Some(contents) = read_if_exists(&r.path())? else {
                    continue;
                };
                let manifest_oid = contents.lines().next().unwrap_or_default().trim().to_owned();
                let manifest = decode::<Manifest>(&fs::read(
                    ObjectOid(manifest_oid.clone()).object_path(&self.objects_dir),
                )?)?;
                live.insert(manifest_oid);
                for oid in manifest.referenced_oids() {
                    live.insert(oid.0.clone());
                }
            }
        }
        for (oid, path) in Self::all_object_oids(&self.objects_dir)? {
            if !live.contains(&oid.0) {
                fs::remove_file(&path)?;
            }
        }
        Ok(removed)
    }

    fn load_fn_intraproc_cache(&self) -> Result<Vec<((String, String), String)>> {
        self.cache.load_fn_intraproc_cache()
    }

    fn put_fn_intraproc_cache(&mut self, rows: &[((String, String), String)]) -> Result<()> {
        self.cache.put_fn_intraproc_cache(rows)
    }

    fn put_summary_deps(&mut self, rows: &[(String, String)]) -> Result<()> {
        self.cache.put_summary_deps(rows)
    }

    fn load_fn_summaries(&self) -> Result<Vec<FnSummaryRow>> {
        self.cache.load_fn_summaries()
    }

    fn put_fn_summaries(&mut self, rows: &[FnSummaryRow]) -> Result<()> {
        self.cache.put_fn_summaries(rows)
    }
}

/// Write `bytes` to `path` via temp → `fsync` → atomic `rename`. The temp file is
/// created in the destination's own directory so the rename stays on one
/// filesystem. Overwrites an existing `path` (used for the ref pointer's CAS
/// advance); object writes gate on non-existence before calling this.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().expect("object path has a parent");
    fs::create_dir_all(parent)?;
    let file_name = path.file_name().expect("object path has a file name");
    let tmp = parent.join(format!(
        "{}.tmp",
        file_name.to_str().expect("object file name is utf-8")
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn read_if_exists(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn read_bytes_if_exists(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn read_dir_if_exists(dir: &Path) -> Result<Option<Vec<fs::DirEntry>>> {
    match fs::read_dir(dir) {
        Ok(rd) => {
            let mut v = Vec::new();
            for e in rd {
                v.push(e?);
            }
            Ok(Some(v))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
