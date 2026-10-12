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
//! by blob OID (IX-1: a re-put of an unchanged blob is a no-op). Because their name
//! is not the hash of their bytes, each carries a SHA-1 trailer; a fragment whose
//! trailer does not match reads as a cache miss.
//!
//! The three dataflow caches (`fn_intraproc_cache`, `summary_deps`, `fn_summaries`)
//! are **not** content-addressed — they want keyed upsert/delete — so they are
//! delegated to an internally-owned, scoped, gitignored [`SqliteStore`] at
//! `.cgx/cache.db`. That DB is index-time scratch and lives under the `.cgx`
//! `.gitignore`'s `*`; it is never part of the committed/transported object set.
//!
//! ## Write discipline (atomic-replace + batched durability + verify-on-read)
//!
//! Every object is written temp → writeout flush → atomic `rename` to its final
//! OID path inside a [`Batch`], and is write-once: an existing file at the OID path
//! is kept only if its bytes hash to that OID. Shards land
//! first, then the manifest object; the batch is then made durable with one full
//! flush ([`Batch::commit`]), and only then does the ref pointer advance, itself
//! written temp → full flush → `rename`. A reader therefore never observes a ref
//! naming an object that is not durably in place.
//!
//! Reads re-hash every object against its OID. An object that is missing or whose
//! bytes do not match (a write torn by power loss on a platform without the batch
//! flush, e.g. wasm) surfaces as [`StoreError::Corrupt`] rather than being served,
//! and the next index run rewrites it.

use crate::durable::{self, Batch};
use crate::error::{Result, StoreError};
use crate::lock::WriteLock;
use crate::manifest::{Manifest, ShardEntry, CURRENT_STORE_FORMAT};
use crate::oid::ObjectOid;
use crate::store::{FactStore, FnSummaryRow, FragmentInput, SqliteStore};
use crate::types::{BlobOid, BlobSet, CachedFragment, LinkedGraph, PruneStats, TreeOid};
use cgx_core::codec::{decode, encode};
use cgx_core::sort::node_sort_key;
use cgx_core::{
    Candidate, Confidence, CutMarkers, EdgeCondition, EdgeId, EdgeKind, EdgeRecord, EffectSet,
    EntrypointKind, EstablishedBy, ImplicitKind, NodeId, NodeRecord, Signature, SiteId, SymbolKind,
    Tier, Transform, Visibility,
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One per-function shard: the nodes and edges attributed to a single owning
/// function, plus the candidate rows of that function's candidate groups.
///
/// A shard carries no dense ids, so it does not change when an unrelated edit
/// renumbers the graph: a node's id is its position, an edge's `src` indexes this
/// shard's `nodes`, an edge's `dst` and a candidate's `dst` are the target's
/// [stable key](sha1_stable_key), and candidate groups are numbered by first
/// appearance among this shard's edges. Nodes and edges keep their canonical
/// relative order and candidates are sorted `(group, rank, dst)`, so the OID is
/// byte-stable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Shard {
    nodes: Vec<StoredNode>,
    edges: Vec<StoredEdge>,
    candidates: Vec<StoredCandidate>,
}

/// [`NodeRecord`] without its id. Field order must match [`StoredNodeRef`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredNode {
    kind: SymbolKind,
    fqn: String,
    file: String,
    line_start: u32,
    line_end: u32,
    lang: String,
    visibility: Visibility,
    is_abstract: bool,
    entrypoint_kind: Option<EntrypointKind>,
    signature: Option<Signature>,
    own_effects: EffectSet,
    transitive_effects: EffectSet,
    unresolved_calls: u32,
}

/// [`EdgeRecord`] without its id, with a shard-local `src`, a stable-key `dst` and
/// a shard-local `candidate_group`. Field order must match [`StoredEdgeRef`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredEdge {
    src: u32,
    dst: u64,
    kind: EdgeKind,
    condition: EdgeCondition,
    confidence: Confidence,
    tier: Tier,
    rule: String,
    site_id: Option<SiteId>,
    stmt_index: Option<u32>,
    cut_markers: CutMarkers,
    implicit: Option<ImplicitKind>,
    candidate_group: Option<u32>,
    established_by: Option<EstablishedBy>,
    cfg_condition: Option<String>,
    macro_origin: Option<String>,
    transform: Option<Transform>,
}

/// A candidate row: shard-local group, stable-key `dst`, rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct StoredCandidate {
    group: u32,
    dst: u64,
    rank: u32,
}

/// Borrowing twin of [`Shard`] for the write path, so sharding a graph does not
/// copy every record. Postcard encodes `&T` exactly as `T` (and `&str` as
/// `String`), so a `ShardRef` encodes to the same bytes (and OID) as the
/// equivalent `Shard`.
#[derive(Default, Serialize)]
struct ShardRef<'a> {
    nodes: Vec<StoredNodeRef<'a>>,
    edges: Vec<StoredEdgeRef<'a>>,
    candidates: Vec<StoredCandidate>,
}

#[derive(Serialize)]
struct StoredNodeRef<'a> {
    kind: &'a SymbolKind,
    fqn: &'a str,
    file: &'a str,
    line_start: u32,
    line_end: u32,
    lang: &'a str,
    visibility: &'a Visibility,
    is_abstract: bool,
    entrypoint_kind: &'a Option<EntrypointKind>,
    signature: &'a Option<Signature>,
    own_effects: &'a EffectSet,
    transitive_effects: &'a EffectSet,
    unresolved_calls: u32,
}

impl<'a> From<&'a NodeRecord> for StoredNodeRef<'a> {
    fn from(n: &'a NodeRecord) -> Self {
        StoredNodeRef {
            kind: &n.kind,
            fqn: &n.fqn,
            file: &n.file,
            line_start: n.line_start,
            line_end: n.line_end,
            lang: &n.lang,
            visibility: &n.visibility,
            is_abstract: n.is_abstract,
            entrypoint_kind: &n.entrypoint_kind,
            signature: &n.signature,
            own_effects: &n.own_effects,
            transitive_effects: &n.transitive_effects,
            unresolved_calls: n.unresolved_calls,
        }
    }
}

impl StoredNode {
    /// [`node_sort_key`] of the record this node decodes to.
    fn sort_key(&self) -> (&str, u32, &str) {
        (&self.file, self.line_start, &self.fqn)
    }

    fn into_record(self, id: NodeId) -> NodeRecord {
        NodeRecord {
            id,
            kind: self.kind,
            fqn: self.fqn,
            file: self.file,
            line_start: self.line_start,
            line_end: self.line_end,
            lang: self.lang,
            visibility: self.visibility,
            is_abstract: self.is_abstract,
            entrypoint_kind: self.entrypoint_kind,
            signature: self.signature,
            own_effects: self.own_effects,
            transitive_effects: self.transitive_effects,
            unresolved_calls: self.unresolved_calls,
        }
    }
}

#[derive(Serialize)]
struct StoredEdgeRef<'a> {
    src: u32,
    dst: u64,
    kind: EdgeKind,
    condition: &'a EdgeCondition,
    confidence: &'a Confidence,
    tier: &'a Tier,
    rule: &'a str,
    site_id: &'a Option<SiteId>,
    stmt_index: Option<u32>,
    cut_markers: &'a CutMarkers,
    implicit: &'a Option<ImplicitKind>,
    candidate_group: Option<u32>,
    established_by: &'a Option<EstablishedBy>,
    cfg_condition: &'a Option<String>,
    macro_origin: &'a Option<String>,
    transform: &'a Option<Transform>,
}

impl<'a> StoredEdgeRef<'a> {
    fn new(e: &'a EdgeRecord, src: u32, dst: u64, candidate_group: Option<u32>) -> Self {
        StoredEdgeRef {
            src,
            dst,
            kind: e.kind,
            condition: &e.condition,
            confidence: &e.confidence,
            tier: &e.tier,
            rule: &e.rule,
            site_id: &e.site_id,
            stmt_index: e.stmt_index,
            cut_markers: &e.cut_markers,
            implicit: &e.implicit,
            candidate_group,
            established_by: &e.established_by,
            cfg_condition: &e.cfg_condition,
            macro_origin: &e.macro_origin,
            transform: &e.transform,
        }
    }
}

impl StoredEdge {
    fn into_record(self, src: NodeId, dst: NodeId) -> EdgeRecord {
        EdgeRecord {
            id: EdgeId(0),
            src,
            dst,
            kind: self.kind,
            condition: self.condition,
            confidence: self.confidence,
            tier: self.tier,
            rule: self.rule,
            site_id: self.site_id,
            stmt_index: self.stmt_index,
            cut_markers: self.cut_markers,
            implicit: self.implicit,
            candidate_group: None,
            established_by: self.established_by,
            cfg_condition: self.cfg_condition,
            macro_origin: self.macro_origin,
            transform: self.transform,
        }
    }
}

/// The stable key (SK) of a node: the first 8 bytes of SHA-1 over its FQN and
/// its dup-ordinal (its rank among same-FQN nodes in canonical order). Unlike a
/// dense id it does not move when unrelated nodes are added or removed, which is
/// what lets an edge into another shard name its target without making that
/// shard depend on the whole graph's numbering.
pub(crate) fn sha1_stable_key(fqn: &str, dup_ordinal: u32) -> u64 {
    let mut h = Sha1::new();
    h.update((fqn.len() as u64).to_le_bytes());
    h.update(fqn.as_bytes());
    h.update(dup_ordinal.to_le_bytes());
    let digest = h.finalize();
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(head)
}

/// The canonical edge order (`cgx_resolve::canonicalize`): `(src, dst, kind,
/// stmt_index, group)`, with an absent stmt/group sorting last.
fn edge_order_key<G: Copy>(src: NodeId, dst: NodeId, kind: EdgeKind, stmt: Option<u32>, group: Option<G>, none: G) -> (u32, u32, u8, u32, G) {
    (src.0, dst.0, kind as u8, stmt.unwrap_or(u32::MAX), group.unwrap_or(none))
}

/// Stable keys for every node of `nodes` (in canonical order), indexed by node
/// position. Fails closed on a collision, naming both FQNs.
fn stable_keys<'a>(
    nodes: impl Iterator<Item = &'a str>,
    hasher: fn(&str, u32) -> u64,
) -> Result<(Vec<u64>, HashMap<u64, u32>)> {
    let mut ordinals: HashMap<&str, u32> = HashMap::new();
    let mut keys = Vec::new();
    let mut by_key: HashMap<u64, u32> = HashMap::new();
    let mut fqns: Vec<&str> = Vec::new();
    for (i, fqn) in nodes.enumerate() {
        let ord = ordinals.entry(fqn).or_insert(0);
        let sk = hasher(fqn, *ord);
        *ord += 1;
        if let Some(&prev) = by_key.get(&sk) {
            return Err(StoreError::StableKeyCollision {
                first: fqns[prev as usize].to_owned(),
                second: fqn.to_owned(),
            });
        }
        by_key.insert(sk, i as u32);
        keys.push(sk);
        fqns.push(fqn);
    }
    Ok((keys, by_key))
}

/// Check that `g` is in the canonical form `cgx_resolve::canonicalize` produces,
/// which is what lets a reader rebuild every dense id from shard positions.
fn check_canonical(g: &LinkedGraph) -> Result<()> {
    let bad = |reason: String| Err(StoreError::NonCanonical { reason });
    for (i, n) in g.nodes.iter().enumerate() {
        if n.id.index() != i {
            return bad(format!("node `{}` has id {} at position {i}", n.fqn, n.id.0));
        }
    }
    if let Some(w) = g
        .nodes
        .windows(2)
        .find(|w| node_sort_key(&w[0]) > node_sort_key(&w[1]))
    {
        return bad(format!("node `{}` sorts after `{}`", w[0].fqn, w[1].fqn));
    }
    let n = g.nodes.len();
    let mut group_src: Vec<NodeId> = Vec::new();
    let mut prev = None;
    for (i, e) in g.edges.iter().enumerate() {
        if e.id.index() != i {
            return bad(format!("edge has id {} at position {i}", e.id.0));
        }
        if e.src.index() >= n || e.dst.index() >= n {
            return bad(format!("edge {i} names a node outside the graph"));
        }
        let key = edge_order_key(e.src, e.dst, e.kind, e.stmt_index, e.candidate_group, u32::MAX);
        if prev.is_some_and(|p| p > key) {
            return bad(format!("edge {i} is out of canonical order"));
        }
        prev = Some(key);
        if let Some(gr) = e.candidate_group {
            match (gr as usize).cmp(&group_src.len()) {
                std::cmp::Ordering::Less if group_src[gr as usize] == e.src => {}
                std::cmp::Ordering::Less => {
                    return bad(format!("candidate group {gr} spans more than one src"))
                }
                std::cmp::Ordering::Equal => group_src.push(e.src),
                std::cmp::Ordering::Greater => {
                    return bad(format!("candidate group {gr} is not numbered by first appearance"))
                }
            }
        }
    }
    let mut prev = None;
    for c in &g.candidates {
        if c.candidate_group as usize >= group_src.len() {
            return bad(format!("candidate group {} has no edge", c.candidate_group));
        }
        if c.dst.index() >= n {
            return bad("candidate row names a node outside the graph".into());
        }
        let key = (c.candidate_group, c.rank, c.dst.0);
        if prev.is_some_and(|p| p > key) {
            return bad("candidate rows are out of (group, rank, dst) order".into());
        }
        prev = Some(key);
    }
    Ok(())
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
    /// The stable-key function; [`sha1_stable_key`] outside tests.
    stable_key: fn(&str, u32) -> u64,
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
            stable_key: sha1_stable_key,
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
        let manifest = decode::<Manifest>(&self.read_object(&ObjectOid(manifest_oid))?)?;
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

    /// Read the object `oid` and verify its bytes hash to it. A missing object or
    /// a mismatch is [`StoreError::Corrupt`]. The bad file is left in place: readers
    /// do not hold the write lock, and the next [`write_object`](Self::write_object)
    /// of that OID verifies and replaces it.
    fn read_object(&self, oid: &ObjectOid) -> Result<Vec<u8>> {
        let corrupt = || StoreError::Corrupt { oid: oid.0.clone() };
        let bytes = read_bytes_if_exists(&oid.object_path(&self.objects_dir))?.ok_or_else(corrupt)?;
        if ObjectOid::of_bytes(&bytes) != *oid {
            return Err(corrupt());
        }
        Ok(bytes)
    }

    /// Write `bytes` as a content-addressed object, returning its OID. Write-once:
    /// a file already at the OID path is kept only if its bytes hash to the OID, so
    /// a torn object left at its final name by a crash is replaced rather than
    /// trusted forever. New writes go through `batch`; the caller commits it before
    /// advancing any ref.
    fn write_object(&self, batch: &mut Batch, bytes: &[u8]) -> Result<ObjectOid> {
        let oid = ObjectOid::of_bytes(bytes);
        let path = oid.object_path(&self.objects_dir);
        if let Some(existing) = read_bytes_if_exists(&path)? {
            if existing.len() == bytes.len() && ObjectOid::of_bytes(&existing) == oid {
                return Ok(oid);
            }
        }
        batch.write(&path, bytes)?;
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
fn owning_key<'a>(fqn: &'a str, callables: &BTreeSet<&str>) -> &'a str {
    let mut s = fqn;
    loop {
        if callables.contains(s) {
            return s;
        }
        match s.rfind("::") {
            Some(i) => s = &s[..i],
            None => return fqn,
        }
    }
}

impl FactStore for ObjectStore {
    fn fragment(&self, blob: &BlobOid) -> Result<Option<CachedFragment>> {
        let path = ObjectOid(blob.0.clone()).object_path(&self.fragments_dir);
        let Some(bytes) = read_bytes_if_exists(&path)? else {
            return Ok(None);
        };
        let Some(sf) = open_fragment(&bytes) else {
            return Ok(None);
        };
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
            // Write-once per (blob OID, frontend version): an unchanged blob
            // re-puts to a true no-op (IX-1). An object written by a different
            // frontend version is replaced, so only the header is compared.
            // An existing file that fails its checksum is rewritten.
            if let Some(existing) = read_bytes_if_exists(&path)? {
                if open_fragment(&existing)
                    .is_some_and(|sf| sf.frontend_version == f.frontend_version)
                {
                    continue;
                }
            }
            let stored = StoredFragment {
                lang: f.lang.to_owned(),
                frontend_version: f.frontend_version,
                bytes: f.bytes.to_vec(),
            };
            // No flush: a fragment is a checksummed cache entry, so one torn or lost
            // by a crash reads as a miss and is re-extracted.
            durable::write_atomic(&path, &seal_fragment(&stored)?)?;
        }
        Ok(())
    }

    fn graph_for(&self, tree: &TreeOid) -> Result<bool> {
        Ok(self.ref_path(tree.as_str()).exists())
    }

    fn put_graph(&mut self, tree: &TreeOid, created_rev: Option<&str>, g: &LinkedGraph) -> Result<()> {
        check_canonical(g)?;
        let (sk, _) = stable_keys(g.nodes.iter().map(|n| n.fqn.as_str()), self.stable_key)?;
        let _lock = WriteLock::acquire(&self.lock_path)?;

        // Attribution: the callable FQNs, then each node's owning-function key.
        // Same-FQN nodes share a key, so dup-ordinals are shard-local too.
        let callables: BTreeSet<&str> = g
            .nodes
            .iter()
            .filter(|n| n.kind.is_callable())
            .map(|n| n.fqn.as_str())
            .collect();

        // Per node: its shard key and its index within that shard.
        let mut placement: Vec<(&str, u32)> = Vec::with_capacity(g.nodes.len());
        let mut shards: BTreeMap<&str, ShardRef<'_>> = BTreeMap::new();
        for n in &g.nodes {
            let key = owning_key(&n.fqn, &callables);
            let shard = shards.entry(key).or_default();
            placement.push((key, shard.nodes.len() as u32));
            shard.nodes.push(StoredNodeRef::from(n));
        }
        // Edges follow their source node's shard, in canonical order; candidate
        // groups are renumbered by first appearance within the shard.
        let mut group_place: Vec<(&str, u32)> = Vec::new();
        let mut shard_groups: HashMap<&str, u32> = HashMap::new();
        for e in &g.edges {
            let (key, src) = placement[e.src.index()];
            let group = e.candidate_group.map(|gr| {
                if gr as usize == group_place.len() {
                    let next = shard_groups.entry(key).or_insert(0);
                    group_place.push((key, *next));
                    *next += 1;
                }
                group_place[gr as usize].1
            });
            let shard = shards.get_mut(key).expect("src node placed");
            shard.edges.push(StoredEdgeRef::new(e, src, sk[e.dst.index()], group));
        }
        for c in &g.candidates {
            let (key, group) = group_place[c.candidate_group as usize];
            shards.get_mut(key).expect("group placed").candidates.push(StoredCandidate {
                group,
                dst: sk[c.dst.index()],
                rank: c.rank,
            });
        }

        // `shards` is a BTreeMap, so iteration is in `fn_key` order — the
        // manifest's shard list is thereby canonical and its OID deterministic.
        let mut batch = Batch::new();
        let mut shard_entries = Vec::with_capacity(shards.len());
        for (fn_key, mut shard) in shards {
            shard
                .candidates
                .sort_unstable_by_key(|c| (c.group, c.rank, c.dst));
            let oid = self.write_object(&mut batch, &encode(&shard)?)?;
            shard_entries.push(ShardEntry {
                fn_key: fn_key.to_owned(),
                oid,
            });
        }

        let manifest = Manifest {
            store_format: CURRENT_STORE_FORMAT,
            graph_key: tree.as_str().to_owned(),
            created_rev: created_rev.map(|s| s.to_owned()),
            shards: shard_entries,
        };
        let manifest_oid = self.write_object(&mut batch, &encode(&manifest)?)?;
        batch.commit(&self.objects_dir)?;

        // Linearization point: advance the ref last, atomically. The batch commit
        // above made every object it names durable first.
        durable::write_durable(
            &self.ref_path(tree.as_str()),
            format!("{}\n{}\n", manifest_oid.as_str(), tree.as_str()).as_bytes(),
        )?;
        Ok(())
    }

    /// Re-densify: rebuild the exact graph `put_graph` was given. Nodes are the
    /// shards' nodes in canonical order (a stable sort of the concatenation
    /// suffices: nodes with equal sort keys share an FQN, hence a shard, whose
    /// order they keep); edges likewise, with each shard-local group standing in
    /// for its global number (both order a src's groups by first appearance), and
    /// groups then renumbered by first appearance. Any shard that does not decode
    /// or names a node that is not there is [`StoreError::Corrupt`].
    fn read_graph(&self, tree: &TreeOid) -> Result<LinkedGraph> {
        let Some(manifest) = self.resolve_manifest(tree.as_str())? else {
            return Ok(LinkedGraph::default());
        };

        let mut shards: Vec<Shard> = Vec::with_capacity(manifest.shards.len());
        for entry in &manifest.shards {
            let bytes = self.read_object(&entry.oid)?;
            let corrupt = || StoreError::Corrupt {
                oid: entry.oid.0.clone(),
            };
            let shard = decode::<Shard>(&bytes)?;
            // The re-densify below relies on each shard's nodes being canonical.
            if !shard.nodes.is_sorted_by(|a, b| a.sort_key() <= b.sort_key()) {
                return Err(corrupt());
            }
            shards.push(shard);
        }
        let corrupt = |si: usize| StoreError::Corrupt {
            oid: manifest.shards[si].oid.0.clone(),
        };

        // Nodes: (shard, local) in manifest order, stably sorted canonically.
        let mut order: Vec<(u32, u32)> = shards
            .iter()
            .enumerate()
            .flat_map(|(si, sh)| (0..sh.nodes.len() as u32).map(move |li| (si as u32, li)))
            .collect();
        let node_at = |&(si, li): &(u32, u32)| &shards[si as usize].nodes[li as usize];
        order.sort_by(|a, b| node_at(a).sort_key().cmp(&node_at(b).sort_key()));
        let mut dense: Vec<Vec<NodeId>> = shards
            .iter()
            .map(|sh| vec![NodeId(0); sh.nodes.len()])
            .collect();
        for (i, &(si, li)) in order.iter().enumerate() {
            dense[si as usize][li as usize] = NodeId(i as u32);
        }
        let (_, by_key) = stable_keys(order.iter().map(|p| node_at(p).fqn.as_str()), self.stable_key)?;
        let resolve = |si: usize, sk: u64| by_key.get(&sk).map(|&i| NodeId(i)).ok_or_else(|| corrupt(si));

        // Edges and candidate rows, re-addressed to dense node ids.
        let mut edges: Vec<(Option<(u32, u32)>, EdgeRecord)> = Vec::new();
        let mut cands: Vec<((u32, u32), NodeId, u32)> = Vec::new();
        for (si, sh) in shards.iter_mut().enumerate() {
            for e in std::mem::take(&mut sh.edges) {
                let src = *dense[si].get(e.src as usize).ok_or_else(|| corrupt(si))?;
                let dst = resolve(si, e.dst)?;
                let group = e.candidate_group.map(|gr| (si as u32, gr));
                edges.push((group, e.into_record(src, dst)));
            }
            for c in &sh.candidates {
                cands.push(((si as u32, c.group), resolve(si, c.dst)?, c.rank));
            }
        }
        edges.sort_by_cached_key(|(g, e)| {
            edge_order_key(e.src, e.dst, e.kind, e.stmt_index, *g, (u32::MAX, u32::MAX))
        });
        let mut groups: HashMap<(u32, u32), u32> = HashMap::new();
        let edges: Vec<EdgeRecord> = edges
            .into_iter()
            .enumerate()
            .map(|(i, (g, mut e))| {
                e.id = EdgeId(i as u32);
                e.candidate_group = g.map(|g| {
                    let next = groups.len() as u32;
                    *groups.entry(g).or_insert(next)
                });
                e
            })
            .collect();
        let mut candidates = cands
            .into_iter()
            .map(|(g, dst, rank)| {
                let candidate_group = *groups.get(&g).ok_or_else(|| corrupt(g.0 as usize))?;
                Ok(Candidate {
                    candidate_group,
                    dst,
                    rank,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        candidates.sort_unstable_by_key(|c| (c.candidate_group, c.rank, c.dst.0));

        let mut nodes = Vec::with_capacity(order.len());
        let mut taken: Vec<std::vec::IntoIter<StoredNode>> = shards
            .into_iter()
            .map(|sh| sh.nodes.into_iter())
            .collect();
        // `order` is grouped per shard in increasing local index, so pulling the
        // next node from each shard's iterator yields the record at (si, li).
        for (i, &(si, _)) in order.iter().enumerate() {
            let n = taken[si as usize].next().expect("one node per placement");
            nodes.push(n.into_record(NodeId(i as u32)));
        }

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

        // Under the write lock no write is in flight, so every temp file is debris
        // from an interrupted run.
        sweep_tmp(&self.refs_dir)?;
        for dir in [&self.objects_dir, &self.fragments_dir] {
            if let Some(fanout) = read_dir_if_exists(dir)? {
                for d in fanout {
                    if d.file_type()?.is_dir() {
                        sweep_tmp(&d.path())?;
                    }
                }
            }
        }

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
                let manifest =
                    decode::<Manifest>(&self.read_object(&ObjectOid(manifest_oid.clone()))?)?;
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

const SHA1_LEN: usize = 20;

/// Encode a fragment with a trailing SHA-1 of its encoding.
fn seal_fragment(sf: &StoredFragment) -> Result<Vec<u8>> {
    let mut bytes = encode(sf)?;
    let digest = Sha1::digest(&bytes);
    bytes.extend_from_slice(&digest);
    Ok(bytes)
}

/// Decode a sealed fragment, or `None` if it is truncated, fails its checksum, or
/// does not decode (including fragments written before the checksum existed).
fn open_fragment(bytes: &[u8]) -> Option<StoredFragment> {
    let body_len = bytes.len().checked_sub(SHA1_LEN)?;
    let (body, trailer) = bytes.split_at(body_len);
    if Sha1::digest(body).as_slice() != trailer {
        return None;
    }
    decode::<StoredFragment>(body).ok()
}

/// Remove every `*.tmp` file directly under `dir`.
fn sweep_tmp(dir: &Path) -> Result<()> {
    let Some(entries) = read_dir_if_exists(dir)? else {
        return Ok(());
    };
    for e in entries {
        if e.file_name().to_str().is_some_and(|n| n.ends_with(".tmp")) {
            match fs::remove_file(e.path()) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
                _ => {}
            }
        }
    }
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

