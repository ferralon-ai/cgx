//! The SQLite-backed fact store (architecture §3): open/create with schema +
//! migrations, Layer-1 fragment CRUD, Layer-2 graph put/get, and prune.

use crate::error::{Result, StoreError};
use crate::lock::WriteLock;
use crate::schema::{
    META_SCHEMA_VERSION, META_VIEW_SCHEMA_VERSION, SCHEMA_DDL, SCHEMA_VERSION, VIEW_SCHEMA_VERSION,
};
use crate::types::{BlobOid, BlobSet, CachedFragment, GraphId, LinkedGraph, PruneStats, TreeOid};
use cgx_core::codec::{decode, encode};
use cgx_core::{EdgeRecord, NodeRecord};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

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

    /// Look up the Layer-2 graph id for a tree OID, or `None` if not linked yet.
    fn graph_for(&self, tree: &TreeOid) -> Result<Option<GraphId>>;

    /// Materialize a linked graph for a tree OID, returning its graph id. If the
    /// tree is already linked, its rows are replaced atomically and the existing
    /// id is reused.
    fn put_graph(
        &mut self,
        tree: &TreeOid,
        created_rev: Option<&str>,
        g: &LinkedGraph,
    ) -> Result<GraphId>;

    /// Read back the linked graph for a graph id. The returned value is byte-for-
    /// byte identical to what was written (records reconstructed from canonical
    /// postcard `data`).
    fn read_graph(&self, graph: GraphId) -> Result<LinkedGraph>;

    /// Remove Layer-1 fragments whose blob OID is not in `live`, and Layer-2
    /// graphs whose tree is no longer referenced (IX-5). With `aggressive`, also
    /// `VACUUM`s to reclaim disk.
    fn prune(&mut self, live: &BlobSet, aggressive: bool) -> Result<PruneStats>;

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

/// A SQLite-backed [`FactStore`] (rusqlite, bundled, WAL).
#[derive(Debug)]
pub struct SqliteStore {
    conn: Connection,
    lock_path: PathBuf,
}

impl SqliteStore {
    /// Open or create the store at `db_path`, applying the schema and verifying
    /// the version gate. The advisory lockfile lives beside it as `index.lock`.
    ///
    /// WAL is enabled for MVCC reads (IX-7); foreign keys are enabled so a graph
    /// delete cascades to its node/edge/candidate rows.
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        let lock_path = lock_path_for(db_path);
        let store = SqliteStore { conn, lock_path };
        store.init_schema()?;
        Ok(store)
    }

    /// Open an in-memory store (tests, ephemeral indexing). No lockfile is
    /// meaningful; lock acquisition targets a throwaway path.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = SqliteStore {
            conn,
            lock_path: PathBuf::from(":memory:.lock"),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Create tables/views (idempotent), then read-or-write the version rows and
    /// enforce the gate.
    ///
    /// ## Forward-compat migration (v0.3 DATA_FLOW SC2)
    ///
    /// An *older* on-disk schema (`found < SCHEMA_VERSION`) is migrated by
    /// **clear-and-reindex**: the disposable Layer-2 tables (`graphs`, `nodes`,
    /// `edges`, `candidates`) are dropped and recreated at the current schema, and
    /// the version rows are rewritten. The Layer-1 `blob_facts` cache is preserved
    /// — it is content-addressed and expensive to re-extract, and is decoded
    /// through the additive (`#[serde(default)]`) record fields, so old fragments
    /// stay valid. The next index run repopulates Layer 2 from that cache. A
    /// *newer* on-disk schema (`found > SCHEMA_VERSION`) is still a hard error
    /// (forward-incompatible: this binary cannot read it).
    fn init_schema(&self) -> Result<()> {
        // Run the DDL first so every table (incl. `cgx_meta_kv` and `blob_facts`)
        // exists before we read the version. On a fresh or current store this is
        // the only step; on an *older* store the version read below triggers the
        // clear-and-reindex migration, which recreates the Layer-2 tables at the
        // current column layout.
        self.conn.execute_batch(SCHEMA_DDL)?;

        match self.read_meta(META_SCHEMA_VERSION)? {
            None => {
                self.write_meta(META_SCHEMA_VERSION, SCHEMA_VERSION)?;
                self.write_meta(META_VIEW_SCHEMA_VERSION, VIEW_SCHEMA_VERSION)?;
            }
            Some(found) if found == SCHEMA_VERSION => {
                // Keep the recorded view version current with this binary.
                self.write_meta(META_VIEW_SCHEMA_VERSION, VIEW_SCHEMA_VERSION)?;
            }
            Some(found) if found < SCHEMA_VERSION => {
                self.migrate_to_current()?;
            }
            Some(found) => {
                // Newer on-disk schema: this binary cannot read it.
                return Err(StoreError::SchemaVersion {
                    found,
                    expected: SCHEMA_VERSION,
                });
            }
        }
        Ok(())
    }

    /// Clear-and-reindex an older store to the current schema: drop the disposable
    /// Layer-2 tables and views, recreate them via the DDL, and rewrite the version
    /// rows. `blob_facts` (the Layer-1 cache) is never touched.
    fn migrate_to_current(&self) -> Result<()> {
        self.conn.execute_batch(
            "DROP VIEW IF EXISTS v_data_flow_edges;
             DROP VIEW IF EXISTS v_call_edges;
             DROP VIEW IF EXISTS v_call_sites;
             DROP VIEW IF EXISTS v_provenance;
             DROP VIEW IF EXISTS v_symbols;
             DROP VIEW IF EXISTS cgx_meta;
             DROP TABLE IF EXISTS candidates;
             DROP TABLE IF EXISTS edges;
             DROP TABLE IF EXISTS nodes;
             DROP TABLE IF EXISTS graphs;
             DROP TABLE IF EXISTS fn_intraproc_cache;
             DROP TABLE IF EXISTS summary_deps;
             DROP TABLE IF EXISTS fn_summaries;",
        )?;
        // Recreate every table/view at the current schema (blob_facts/cgx_meta_kv
        // are `IF NOT EXISTS`, so the preserved ones are left intact).
        self.conn.execute_batch(SCHEMA_DDL)?;
        self.write_meta(META_SCHEMA_VERSION, SCHEMA_VERSION)?;
        self.write_meta(META_VIEW_SCHEMA_VERSION, VIEW_SCHEMA_VERSION)?;
        Ok(())
    }

    fn read_meta(&self, key: &str) -> Result<Option<i64>> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM cgx_meta_kv WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.and_then(|s| s.parse::<i64>().ok()))
    }

    fn write_meta(&self, key: &str, value: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO cgx_meta_kv(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value.to_string()],
        )?;
        Ok(())
    }

    /// The on-disk physical schema version.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .read_meta(META_SCHEMA_VERSION)?
            .unwrap_or(SCHEMA_VERSION))
    }

    /// The on-disk view-schema version (ADR-05), also exposed via the `cgx_meta`
    /// view.
    pub fn view_schema_version(&self) -> Result<i64> {
        Ok(self
            .read_meta(META_VIEW_SCHEMA_VERSION)?
            .unwrap_or(VIEW_SCHEMA_VERSION))
    }

    /// Acquire the advisory write lock (IX-7). Callers hold this across a
    /// check → lock → re-check sequence before mutating.
    pub fn write_lock(&self) -> Result<WriteLock> {
        WriteLock::acquire(&self.lock_path)
    }

    /// Run a read-only `SELECT COUNT(*)`-shaped query against the store. Intended
    /// for `--sql` over the `v_*` views (ADR-05); the caller is responsible for
    /// confining itself to the documented views.
    pub fn query_count(&self, sql: &str) -> Result<i64> {
        Ok(self.conn.query_row(sql, [], |r| r.get(0))?)
    }

    /// Read a `cgx_meta` value by key through the public view (ADR-05).
    pub fn query_meta_value(&self, key: &str) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row(
                "SELECT value FROM cgx_meta WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(v)
    }

    /// Dump the canonical `data` bytes of every node and edge row of a graph, in
    /// row order. Used by the determinism harness to assert two writes of the same
    /// graph hold byte-identical row data.
    pub fn dump_node_edge_data(&self, graph: GraphId) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut nstmt = self
            .conn
            .prepare("SELECT data FROM nodes WHERE graph_id = ?1 ORDER BY node_id")?;
        for row in nstmt.query_map(params![graph.0], |r| r.get::<_, Vec<u8>>(0))? {
            out.push(row?);
        }
        let mut estmt = self
            .conn
            .prepare("SELECT data FROM edges WHERE graph_id = ?1 ORDER BY edge_id")?;
        for row in estmt.query_map(params![graph.0], |r| r.get::<_, Vec<u8>>(0))? {
            out.push(row?);
        }
        Ok(out)
    }
}

/// The lockfile path beside a database file.
fn lock_path_for(db_path: &Path) -> PathBuf {
    let mut p = db_path.to_path_buf();
    p.set_extension("lock");
    p
}

impl FactStore for SqliteStore {
    fn fragment(&self, blob: &BlobOid) -> Result<Option<CachedFragment>> {
        let row = self
            .conn
            .query_row(
                "SELECT lang, frontend_version, fragment FROM blob_facts WHERE blob_oid = ?1",
                params![blob.as_str()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(lang, fv, fragment)| CachedFragment {
            blob: blob.clone(),
            lang,
            frontend_version: fv as u32,
            fragment,
        }))
    }

    fn put_fragments(&mut self, batch: &[FragmentInput<'_>]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            // INSERT-or-ignore makes an unchanged blob a true no-op (IX-1): the
            // existing row and its bytes are left untouched. A blob OID is
            // content-addressed, so a present key already holds the right bytes.
            let mut stmt = tx.prepare(
                "INSERT INTO blob_facts(blob_oid, lang, frontend_version, fragment)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(blob_oid) DO NOTHING",
            )?;
            for f in batch {
                stmt.execute(params![
                    f.blob.as_str(),
                    f.lang,
                    f.frontend_version as i64,
                    f.bytes,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn graph_for(&self, tree: &TreeOid) -> Result<Option<GraphId>> {
        let id = self
            .conn
            .query_row(
                "SELECT graph_id FROM graphs WHERE tree_oid = ?1",
                params![tree.as_str()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        Ok(id.map(GraphId))
    }

    fn put_graph(
        &mut self,
        tree: &TreeOid,
        created_rev: Option<&str>,
        g: &LinkedGraph,
    ) -> Result<GraphId> {
        let tx = self.conn.transaction()?;
        let graph_id: i64 = {
            // Reuse the row for an already-linked tree (idempotent put).
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT graph_id FROM graphs WHERE tree_oid = ?1",
                    params![tree.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(id) => {
                    tx.execute("DELETE FROM nodes WHERE graph_id = ?1", params![id])?;
                    tx.execute("DELETE FROM edges WHERE graph_id = ?1", params![id])?;
                    tx.execute("DELETE FROM candidates WHERE graph_id = ?1", params![id])?;
                    tx.execute(
                        "UPDATE graphs SET created_rev = ?2, schema_version = ?3 WHERE graph_id = ?1",
                        params![id, created_rev, SCHEMA_VERSION],
                    )?;
                    id
                }
                None => {
                    tx.execute(
                        "INSERT INTO graphs(tree_oid, created_rev, schema_version)
                         VALUES(?1, ?2, ?3)",
                        params![tree.as_str(), created_rev, SCHEMA_VERSION],
                    )?;
                    tx.last_insert_rowid()
                }
            }
        };

        {
            let mut node_stmt = tx.prepare(
                "INSERT INTO nodes(graph_id, node_id, kind, fqn, file, line_start, line_end,
                                   lang, visibility, entrypoint_kind, signature, own_effects, data)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            )?;
            for n in &g.nodes {
                node_stmt.execute(params![
                    graph_id,
                    n.id.0 as i64,
                    enum_token(n.kind),
                    n.fqn,
                    n.file,
                    n.line_start as i64,
                    n.line_end as i64,
                    n.lang,
                    enum_token(n.visibility),
                    n.entrypoint_kind.map(enum_token),
                    n.signature.as_ref().map(|s| s.canonical()),
                    effect_labels(n.own_effects),
                    encode(n)?,
                ])?;
            }

            let mut edge_stmt = tx.prepare(
                "INSERT INTO edges(graph_id, edge_id, src, dst, edge_kind, edge_condition,
                                   confidence, tier, rule, transform, site_id, candidate_group, data)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            )?;
            for e in &g.edges {
                edge_stmt.execute(params![
                    graph_id,
                    e.id.0 as i64,
                    e.src.0 as i64,
                    e.dst.0 as i64,
                    enum_token(e.kind),
                    enum_token(e.condition),
                    enum_token(e.confidence),
                    e.tier.level() as i64,
                    e.rule,
                    e.transform.map(enum_token),
                    e.site_id.map(|s| s.0 as i64),
                    e.candidate_group.map(|c| c as i64),
                    encode(e)?,
                ])?;
            }

            let mut cand_stmt = tx.prepare(
                "INSERT INTO candidates(graph_id, candidate_group, dst, rank)
                 VALUES(?1,?2,?3,?4)",
            )?;
            for c in &g.candidates {
                cand_stmt.execute(params![
                    graph_id,
                    c.candidate_group as i64,
                    c.dst.0 as i64,
                    c.rank as i64,
                ])?;
            }
        }

        tx.commit()?;
        Ok(GraphId(graph_id))
    }

    fn read_graph(&self, graph: GraphId) -> Result<LinkedGraph> {
        let mut nodes = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT data FROM nodes WHERE graph_id = ?1 ORDER BY node_id")?;
            let rows = stmt.query_map(params![graph.0], |r| r.get::<_, Vec<u8>>(0))?;
            for row in rows {
                let bytes = row?;
                nodes.push(decode::<NodeRecord>(&bytes)?);
            }
        }

        let mut edges = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT data FROM edges WHERE graph_id = ?1 ORDER BY edge_id")?;
            let rows = stmt.query_map(params![graph.0], |r| r.get::<_, Vec<u8>>(0))?;
            for row in rows {
                let bytes = row?;
                edges.push(decode::<EdgeRecord>(&bytes)?);
            }
        }

        let mut candidates = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT candidate_group, dst, rank FROM candidates
                 WHERE graph_id = ?1 ORDER BY candidate_group, rank, dst",
            )?;
            let rows = stmt.query_map(params![graph.0], |r| {
                Ok(cgx_core::Candidate {
                    candidate_group: r.get::<_, i64>(0)? as u32,
                    dst: cgx_core::NodeId(r.get::<_, i64>(1)? as u32),
                    rank: r.get::<_, i64>(2)? as u32,
                })
            })?;
            for row in rows {
                candidates.push(row?);
            }
        }

        Ok(LinkedGraph {
            nodes,
            edges,
            candidates,
        })
    }

    fn prune(&mut self, live: &BlobSet, aggressive: bool) -> Result<PruneStats> {
        let mut stats = PruneStats::default();
        let tx = self.conn.transaction()?;
        {
            // Collect stored blob OIDs, delete those not in the live set.
            let stored: Vec<String> = {
                let mut stmt = tx.prepare("SELECT blob_oid FROM blob_facts")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut del = tx.prepare("DELETE FROM blob_facts WHERE blob_oid = ?1")?;
            for oid in stored {
                if !live.contains(&BlobOid(oid.clone())) {
                    del.execute(params![oid])?;
                    stats.fragments_removed += 1;
                }
            }
        }
        tx.commit()?;

        // Layer-2 graphs are a disposable materialization; nothing references a
        // graph row except by tree OID, and a tree's liveness is the caller's
        // concern (IX-5 operates on blob reachability). Phase-1 prune does not
        // garbage-collect graphs here; that is the index orchestrator's call when
        // it knows which tree OIDs are dead. `graphs_removed` stays 0.

        if aggressive {
            self.conn.execute_batch("VACUUM")?;
            stats.compacted = true;
        }
        Ok(stats)
    }

    fn load_fn_intraproc_cache(&self) -> Result<Vec<((String, String), String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT blob_oid, fn_fqn, facts_hash FROM fn_intraproc_cache")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                r.get::<_, String>(2)?,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn put_fn_intraproc_cache(&mut self, rows: &[((String, String), String)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            tx.execute("DELETE FROM fn_intraproc_cache", [])?;
            let mut stmt = tx.prepare(
                "INSERT INTO fn_intraproc_cache(blob_oid, fn_fqn, facts_hash) VALUES(?1, ?2, ?3)",
            )?;
            for ((blob_oid, fn_fqn), facts_hash) in rows {
                stmt.execute(params![blob_oid, fn_fqn, facts_hash])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn put_summary_deps(&mut self, rows: &[(String, String)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            tx.execute("DELETE FROM summary_deps", [])?;
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO summary_deps(fn_fqn, callee_fqn) VALUES(?1, ?2)",
            )?;
            for (fn_fqn, callee_fqn) in rows {
                stmt.execute(params![fn_fqn, callee_fqn])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn load_fn_summaries(&self) -> Result<Vec<FnSummaryRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT blob_oid, fn_fqn, summary_bytes FROM fn_summaries")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn put_fn_summaries(&mut self, rows: &[FnSummaryRow]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            tx.execute("DELETE FROM fn_summaries", [])?;
            let mut stmt = tx.prepare(
                "INSERT INTO fn_summaries(blob_oid, fn_fqn, summary_bytes) VALUES(?1, ?2, ?3)",
            )?;
            for ((blob_oid, fn_fqn), summary_bytes) in rows {
                stmt.execute(params![blob_oid, fn_fqn, summary_bytes])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

/// Render a `cgx-core` enum to its serde string token, for the denormalized
/// columns the `v_*` views project. Uses the same `serde` rename the model
/// declares, so the column text matches the postcard/JSON vocabulary.
fn enum_token<T: serde::Serialize>(value: T) -> String {
    // Every model enum serializes as a string (snake/kebab-case). Round-trip
    // through serde_json would add a dep; instead use postcard-free rendering:
    // these enums all `#[serde(rename_all=...)]` to a unit string, which we
    // recover via a tiny in-crate serializer.
    crate::token::to_token(&value)
}

/// Render an [`EffectSet`](cgx_core::EffectSet) to the denormalized `own_effects`
/// column: the canonical effect labels in fixed iteration order, `,`-joined
/// (e.g. `blocking,io.file`). Empty set → empty string. Deterministic.
fn effect_labels(effects: cgx_core::EffectSet) -> String {
    effects
        .iter()
        .map(|e| e.as_str())
        .collect::<Vec<_>>()
        .join(",")
}
