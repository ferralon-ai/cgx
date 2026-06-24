//! v0.3 DATA_FLOW SC2: forward-compat v2 → v3 migration.
//!
//! Opening an *older*-schema store with the current binary must **clear and
//! reindex** the disposable Layer-2 tables (graphs/nodes/edges/candidates) rather
//! than hard-error, while preserving the Layer-1 `blob_facts` cache.

mod common;

use cgx_store::{BlobOid, FactStore, FragmentInput, SqliteStore, TreeOid, SCHEMA_VERSION};
use common::sample_graph;
use rusqlite::Connection;

/// Force the on-disk schema version down to `v`, simulating an older store. The
/// table layout stays current (SQLite cannot drop a column in-place), which is
/// exactly what the migration's drop-and-recreate handles regardless.
fn downgrade_schema_version(db_path: &std::path::Path, v: i64) {
    let conn = Connection::open(db_path).expect("open raw conn");
    conn.execute(
        "INSERT INTO cgx_meta_kv(key, value) VALUES('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![v.to_string()],
    )
    .expect("downgrade version");
}

fn count(db_path: &std::path::Path, sql: &str) -> i64 {
    let conn = Connection::open(db_path).expect("open raw conn");
    conn.query_row(sql, [], |r| r.get(0)).expect("count")
}

#[test]
fn v2_store_opened_by_v3_clears_and_reindexes_not_hard_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");

    // Build a populated store (a blob_facts cache row + a Layer-2 graph).
    {
        let mut store = SqliteStore::open(&db).expect("open fresh store");
        let blob = BlobOid::new("blob-keepme");
        let bytes = b"cached-fragment-bytes";
        store
            .put_fragments(&[FragmentInput {
                blob: &blob,
                lang: "rust",
                frontend_version: 1,
                bytes,
            }])
            .unwrap();
        store
            .put_graph(&TreeOid::new("tree-old"), Some("rev"), &sample_graph())
            .unwrap();
    }

    // Sanity: the graph rows and the cache row exist before migration.
    assert!(count(&db, "SELECT COUNT(*) FROM nodes") > 0);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM blob_facts"), 1);

    // Simulate an older store, then reopen with the current binary.
    downgrade_schema_version(&db, SCHEMA_VERSION - 1);

    let store = SqliteStore::open(&db).expect("opening an older store must NOT hard-error");

    // The version is rewritten to current.
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    // Layer-2 graph tables are cleared by the clear-and-reindex migration.
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM nodes"),
        0,
        "migration must clear node rows"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM edges"),
        0,
        "migration must clear edge rows"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM graphs"),
        0,
        "migration must clear graph rows"
    );
    // The expensive Layer-1 cache is preserved (not re-extracted).
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM blob_facts"),
        1,
        "migration must preserve the blob_facts cache"
    );
}

#[test]
fn migrated_store_accepts_a_fresh_put_graph() {
    // After migration the store is fully usable: a new put_graph round-trips.
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");
    {
        let mut store = SqliteStore::open(&db).unwrap();
        store
            .put_graph(&TreeOid::new("tree-x"), None, &sample_graph())
            .unwrap();
    }
    downgrade_schema_version(&db, SCHEMA_VERSION - 1);

    let mut store = SqliteStore::open(&db).unwrap();
    let id = store
        .put_graph(&TreeOid::new("tree-y"), None, &sample_graph())
        .unwrap();
    let back = store.read_graph(id).unwrap();
    assert_eq!(back, sample_graph());
}

#[test]
fn sc3_tables_exist_after_fresh_open_and_after_migration() {
    // v0.3 SC3 missing-migration trap: the new fn_intraproc_cache / summary_deps
    // tables must exist both on a fresh store AND after a clear-and-reindex
    // migration from an older schema.
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");
    {
        let _ = SqliteStore::open(&db).unwrap();
    }
    // Fresh store: both tables present.
    let table_exists = |name: &str| -> i64 {
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{name}'"
            ),
        )
    };
    assert_eq!(table_exists("fn_intraproc_cache"), 1, "fresh store has the cache table");
    assert_eq!(table_exists("summary_deps"), 1, "fresh store has summary_deps");

    // Downgrade and reopen → the migration path must (re)create them.
    downgrade_schema_version(&db, SCHEMA_VERSION - 1);
    let _ = SqliteStore::open(&db).expect("migration must not hard-error");
    assert_eq!(
        table_exists("fn_intraproc_cache"),
        1,
        "migration must leave the cache table present"
    );
    assert_eq!(
        table_exists("summary_deps"),
        1,
        "migration must leave summary_deps present"
    );
}

#[test]
fn fn_intraproc_cache_and_summary_deps_round_trip() {
    // The SC3 FactStore API persists and reloads the per-function cache and the
    // summary_deps relation; a re-put replaces the prior rows.
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");
    let mut store = SqliteStore::open(&db).unwrap();

    let rows = vec![
        (("blobA".to_string(), "m::f".to_string()), "hashf".to_string()),
        (("blobA".to_string(), "m::g".to_string()), "hashg".to_string()),
    ];
    store.put_fn_intraproc_cache(&rows).unwrap();
    let mut back = store.load_fn_intraproc_cache().unwrap();
    back.sort();
    let mut want = rows.clone();
    want.sort();
    assert_eq!(back, want, "cache rows round-trip");

    // A re-put replaces (does not append) the prior set.
    let replacement = vec![(("blobB".to_string(), "m::f".to_string()), "hash2".to_string())];
    store.put_fn_intraproc_cache(&replacement).unwrap();
    assert_eq!(
        store.load_fn_intraproc_cache().unwrap(),
        replacement,
        "re-put replaces the prior cache"
    );

    // summary_deps persist (dedup on the PK).
    let deps = vec![
        ("m::g".to_string(), "m::f".to_string()),
        ("m::g".to_string(), "*".to_string()),
    ];
    store.put_summary_deps(&deps).unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM summary_deps"),
        2,
        "both summary_deps rows persist"
    );
}

#[test]
fn newer_schema_still_hard_errors() {
    // A *newer* on-disk schema is still rejected (this binary cannot read it).
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");
    {
        let _ = SqliteStore::open(&db).unwrap();
    }
    downgrade_schema_version(&db, SCHEMA_VERSION + 1);
    assert!(
        SqliteStore::open(&db).is_err(),
        "a newer schema must hard-error"
    );
}
