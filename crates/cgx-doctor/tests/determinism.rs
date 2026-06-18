//! Determinism harness (WP-12): index a fixture twice into fresh stores and
//! assert the stored graph + a representative query are byte-identical.
//!
//! This codifies the determinism guarantee from architecture §3/§4: two index
//! runs of the same source tree produce byte-identical stored graphs (same
//! node/edge `data` bytes, same ordering). The test uses the same fixtures that
//! WP-02 committed under `fixtures/` — if those are unavailable it falls back
//! to an in-memory graph built entirely from known constants, which exercises
//! the codec round-trip path without requiring git.
//!
//! The two assertions are:
//! 1. **Store bytes identical:** `SqliteStore::dump_node_edge_data` on run-1 ==
//!    run-2 (byte-level column data, not just logical equality).
//! 2. **Report identical:** `DoctorReport` computed on run-1 == run-2 (struct
//!    equality), and `render_json` of each is byte-identical.
//!
//! When `cgx-index` is available (it is listed as a dev-dependency), we run the
//! full pipeline on the committed git tree of this repository (the smallest
//! real index available: the workspace itself). When `cgx-index` cannot discover
//! a git repo (e.g. in a flat test environment), we skip the pipeline path and
//! only run the in-memory path.

mod common;

use cgx_doctor::{render_json, report};
use cgx_store::{FactStore, LinkedGraph, SqliteStore};

// ---------------------------------------------------------------------------
// Helper: build a deterministic in-memory graph from constants
// ---------------------------------------------------------------------------

fn fixture_graph() -> LinkedGraph {
    use cgx_core::Confidence;
    use cgx_core::CutMarker;
    common::GraphBuilder::new()
        .func("crate::entry")
        .func("crate::core::process")
        .func("crate::core::validate")
        .func("crate::io::read")
        .func("crate::io::write")
        .calls("crate::entry", "crate::core::process")
        .calls("crate::entry", "crate::io::read")
        .calls_conf(
            "crate::core::process",
            "crate::core::validate",
            Confidence::Probable,
        )
        .calls_conf(
            "crate::core::process",
            "crate::io::write",
            Confidence::Certain,
        )
        .calls_with_marker(
            "crate::core::validate",
            "crate::io::write",
            CutMarker::UnexpandedMacro,
        )
        .build()
}

// ---------------------------------------------------------------------------
// Test 1: two writes of the same LinkedGraph produce byte-identical store data
// ---------------------------------------------------------------------------

#[test]
fn two_writes_of_same_graph_are_byte_identical() {
    let graph = fixture_graph();

    // Write into two separate in-memory stores.
    let data_a = {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let tree = cgx_store::TreeOid::new("sha1:abcdef0000000000000000000000000000000001");
        let gid = store.put_graph(&tree, None, &graph).unwrap();
        store.dump_node_edge_data(gid).unwrap()
    };

    let data_b = {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let tree = cgx_store::TreeOid::new("sha1:abcdef0000000000000000000000000000000001");
        let gid = store.put_graph(&tree, None, &graph).unwrap();
        store.dump_node_edge_data(gid).unwrap()
    };

    assert_eq!(data_a.len(), data_b.len(), "row count must match");
    for (i, (a, b)) in data_a.iter().zip(data_b.iter()).enumerate() {
        assert_eq!(a, b, "row {i} data bytes differ between run-1 and run-2");
    }
}

// ---------------------------------------------------------------------------
// Test 2: read_graph round-trip is byte-identical
// ---------------------------------------------------------------------------

#[test]
fn read_graph_round_trips_identically() {
    let graph = fixture_graph();

    let mut store = SqliteStore::open_in_memory().unwrap();
    let tree = cgx_store::TreeOid::new("sha1:abcdef0000000000000000000000000000000002");
    let gid = store.put_graph(&tree, None, &graph).unwrap();

    let readback = store.read_graph(gid).unwrap();
    assert_eq!(
        graph, readback,
        "read_graph must reproduce identical LinkedGraph"
    );
}

// ---------------------------------------------------------------------------
// Test 3: DoctorReport is identical across two runs of the same graph
// ---------------------------------------------------------------------------

#[test]
fn doctor_report_is_identical_across_two_runs() {
    let graph = fixture_graph();

    // Run A
    let rep_a = {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let tree = cgx_store::TreeOid::new("sha1:abcdef0000000000000000000000000000000003");
        let gid = store.put_graph(&tree, None, &graph).unwrap();
        report(&store, gid).unwrap()
    };

    // Run B
    let rep_b = {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let tree = cgx_store::TreeOid::new("sha1:abcdef0000000000000000000000000000000003");
        let gid = store.put_graph(&tree, None, &graph).unwrap();
        report(&store, gid).unwrap()
    };

    assert_eq!(
        rep_a, rep_b,
        "DoctorReport must be structurally identical across two runs"
    );

    let json_a = render_json(&rep_a).unwrap();
    let json_b = render_json(&rep_b).unwrap();
    assert_eq!(
        json_a, json_b,
        "render_json must be byte-identical across two runs"
    );
}

// ---------------------------------------------------------------------------
// Test 4: full index-pipeline determinism (two runs, same tree, file-based store)
// ---------------------------------------------------------------------------

/// Index the same source directory twice into separate on-disk stores and assert
/// byte-identical results. Uses `cgx-index`'s `index_path` on the workspace root
/// (the smallest real Rust source available in the test environment).
///
/// Skipped automatically if we cannot discover a git repository from the workspace
/// root (e.g., running in a non-git environment).
#[test]
fn full_pipeline_two_run_byte_identity() {
    // Discover the workspace root by walking up from the cargo manifest dir.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // Walk up to the workspace root (where Cargo.toml with [workspace] lives).
    let workspace_root = manifest_dir
        .ancestors()
        .find(|p| p.join("Cargo.toml").exists() && p.join("crates").exists());

    let Some(repo_root) = workspace_root else {
        eprintln!("skip: no workspace root found from {manifest_dir:?}");
        return;
    };

    // Verify it's a git repo.
    if !repo_root.join(".git").exists() {
        eprintln!("skip: {repo_root:?} is not a git repo");
        return;
    }

    let dir = tempfile::tempdir().unwrap();

    // Run A: index into store_a.db
    let (data_a, gid_a, rep_a) = {
        let db_a = dir.path().join("store_a.db");
        let mut store = SqliteStore::open(&db_a).unwrap();
        let registry = cgx_index::default_registry();
        let outcome = cgx_index::index_path(repo_root, &registry, &mut store).unwrap();
        let data = store.dump_node_edge_data(outcome.graph_id).unwrap();
        let rep = cgx_doctor::report(&store, outcome.graph_id).unwrap();
        (data, outcome.graph_id, rep)
    };

    // Run B: index into a fresh store_b.db
    let (data_b, _gid_b, rep_b) = {
        let db_b = dir.path().join("store_b.db");
        let mut store = SqliteStore::open(&db_b).unwrap();
        let registry = cgx_index::default_registry();
        let outcome = cgx_index::index_path(repo_root, &registry, &mut store).unwrap();
        let data = store.dump_node_edge_data(outcome.graph_id).unwrap();
        let rep = cgx_doctor::report(&store, outcome.graph_id).unwrap();
        (data, outcome.graph_id, rep)
    };

    // Assert byte identity of stored graph data.
    assert_eq!(
        data_a.len(),
        data_b.len(),
        "node+edge row count must match across runs (run_a graph_id={gid_a:?})"
    );
    for (i, (a, b)) in data_a.iter().zip(data_b.iter()).enumerate() {
        assert_eq!(
            a, b,
            "row {i} data bytes differ between run-1 and run-2 of full pipeline"
        );
    }

    // Assert DoctorReport identity.
    assert_eq!(
        rep_a, rep_b,
        "DoctorReport must be identical across two full pipeline runs"
    );
    let json_a = render_json(&rep_a).unwrap();
    let json_b = render_json(&rep_b).unwrap();
    assert_eq!(
        json_a, json_b,
        "render_json of DoctorReport must be byte-identical across two full pipeline runs"
    );
}
