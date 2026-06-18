//! Physical schema, versioned read views, and migrations (architecture §3, ADR-05).
//!
//! ## Two stability surfaces
//!
//! - **Physical tables** (`blob_facts`, `graphs`, `nodes`, `edges`, `candidates`,
//!   `cgx_meta_kv`) are *not* a public API. Their `schema_version` gates whether
//!   this binary can open the file at all.
//! - **`v_*` views + `cgx_meta`** are the only SQL-visible stability contract
//!   (ADR-05). `--sql` runs exclusively against these. `view_schema_version`
//!   (exposed through the `cgx_meta` view) bumps on any view-breaking change.
//!
//! ## Determinism
//!
//! Each `nodes`/`edges`/`candidates` row stores the *full* `cgx-core` record as
//! canonical postcard bytes in a `data` column, alongside denormalized columns
//! the views and indexes read. Reads reconstruct records from `data`, so a
//! round-trip is byte-identical regardless of column projection. Layer-1
//! fragments are stored verbatim as the bytes the indexer supplied.

/// Physical schema version. Bumped when the table layout changes incompatibly;
/// opening a file with a higher version is a hard error (forward-incompatible).
pub const SCHEMA_VERSION: i64 = 1;

/// View-schema version (ADR-05), surfaced via the `cgx_meta` view. Bumped only on
/// view-breaking changes (renamed/removed columns or views), independently of the
/// physical `SCHEMA_VERSION`.
pub const VIEW_SCHEMA_VERSION: i64 = 1;

/// `cgx_meta_kv` key under which the physical schema version is stored.
pub const META_SCHEMA_VERSION: &str = "schema_version";
/// `cgx_meta_kv` key under which the view schema version is stored.
pub const META_VIEW_SCHEMA_VERSION: &str = "view_schema_version";

/// The Phase-1 view set named by ADR-05 (and docs/05 Q-10). Physical tables are
/// not API; these views are.
pub const VIEW_SET: &[&str] = &[
    "v_symbols",
    "v_call_edges",
    "v_call_sites",
    "v_provenance",
    "cgx_meta",
];

/// Full DDL for a fresh database: physical tables, indexes, then the `v_*` views.
/// Idempotent (`IF NOT EXISTS`) so re-running on an existing file is harmless.
pub const SCHEMA_DDL: &str = r#"
-- ---- Layer 1: per-blob fragments (shared across branches & worktrees) --------
CREATE TABLE IF NOT EXISTS blob_facts (
    blob_oid         TEXT PRIMARY KEY,   -- git blob OID (content-addressed key)
    lang             TEXT NOT NULL,
    frontend_version INTEGER NOT NULL,
    fragment         BLOB NOT NULL,      -- canonical postcard bytes (opaque here)
    blame            BLOB                -- IX-9 attribution, filled by WP-11
) WITHOUT ROWID;

-- ---- Layer 2: linked graphs, one per tree OID --------------------------------
CREATE TABLE IF NOT EXISTS graphs (
    graph_id       INTEGER PRIMARY KEY,
    tree_oid       TEXT NOT NULL UNIQUE,
    created_rev    TEXT,
    schema_version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS nodes (
    graph_id    INTEGER NOT NULL REFERENCES graphs(graph_id) ON DELETE CASCADE,
    node_id     INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    fqn         TEXT NOT NULL,
    file        TEXT NOT NULL,
    line_start  INTEGER NOT NULL,
    line_end    INTEGER NOT NULL,
    lang        TEXT NOT NULL,
    visibility  TEXT NOT NULL,
    entrypoint_kind TEXT,
    signature   TEXT,                    -- canonical signature string, denormalized
    data        BLOB NOT NULL,           -- canonical postcard of the NodeRecord
    PRIMARY KEY (graph_id, node_id)
);

CREATE TABLE IF NOT EXISTS edges (
    graph_id        INTEGER NOT NULL REFERENCES graphs(graph_id) ON DELETE CASCADE,
    edge_id         INTEGER NOT NULL,
    src             INTEGER NOT NULL,
    dst             INTEGER NOT NULL,
    edge_kind       TEXT NOT NULL,
    edge_condition  TEXT NOT NULL,
    confidence      TEXT NOT NULL,
    tier            INTEGER NOT NULL,
    rule            TEXT NOT NULL,
    site_id         INTEGER,
    candidate_group INTEGER,
    data            BLOB NOT NULL,       -- canonical postcard of the EdgeRecord
    PRIMARY KEY (graph_id, edge_id)
);

CREATE TABLE IF NOT EXISTS candidates (
    graph_id        INTEGER NOT NULL REFERENCES graphs(graph_id) ON DELETE CASCADE,
    candidate_group INTEGER NOT NULL,
    dst             INTEGER NOT NULL,
    rank            INTEGER NOT NULL,
    PRIMARY KEY (graph_id, candidate_group, dst)
);

CREATE TABLE IF NOT EXISTS cgx_meta_kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_nodes_fqn ON nodes(graph_id, fqn);
CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(graph_id, dst);
CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(graph_id, src);

-- ---- ADR-05 versioned read views (the only SQL-visible contract) -------------
CREATE VIEW IF NOT EXISTS v_symbols AS
    SELECT graph_id, node_id, kind, fqn, file, line_start, line_end,
           lang, visibility, entrypoint_kind, signature
    FROM nodes;

CREATE VIEW IF NOT EXISTS v_call_edges AS
    SELECT graph_id, edge_id, src AS caller, dst AS callee, edge_kind,
           edge_condition, confidence, tier, rule, site_id, candidate_group
    FROM edges;

CREATE VIEW IF NOT EXISTS v_call_sites AS
    SELECT e.graph_id, e.site_id, e.src AS caller, e.edge_id,
           n.file, n.fqn AS caller_fqn
    FROM edges e
    JOIN nodes n ON n.graph_id = e.graph_id AND n.node_id = e.src
    WHERE e.site_id IS NOT NULL;

CREATE VIEW IF NOT EXISTS v_provenance AS
    SELECT graph_id, edge_id, rule, tier, edge_condition, confidence
    FROM edges;

CREATE VIEW IF NOT EXISTS cgx_meta AS
    SELECT key, value FROM cgx_meta_kv;
"#;
