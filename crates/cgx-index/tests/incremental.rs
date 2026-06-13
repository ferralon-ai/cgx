//! Incremental & determinism proofs (docs/06 IX-1/IX-2, architecture §4).
//!
//! - Re-indexing an unchanged tree does **zero** extraction (blob-OID cache hit).
//! - Changing one file re-extracts only that file's blob.
//! - Two independent index runs of the same tree produce a byte-identical graph.

mod common;

use cgx_index::{default_registry, index_path};
use common::*;

#[test]
fn reindex_unchanged_tree_does_no_extraction() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();

    let first = index_path(&repo, &registry, &mut store).unwrap();
    assert!(first.stats.blobs_extracted > 0, "first run must extract");
    assert_eq!(first.stats.blobs_cached, 0, "first run has no cache");

    // Second run against the SAME store: every blob OID is already cached.
    let second = index_path(&repo, &registry, &mut store).unwrap();
    assert_eq!(
        second.stats.blobs_extracted, 0,
        "incremental no-op must re-extract nothing: {:?}",
        second.stats
    );
    assert_eq!(
        second.stats.blobs_cached, second.stats.blobs_indexed,
        "every indexed blob must be a cache hit on re-index"
    );
    assert_eq!(
        first.stats.blobs_indexed, second.stats.blobs_indexed,
        "same files indexed both runs"
    );
}

#[test]
fn changing_one_file_reextracts_only_that_blob() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();

    let first = index_path(&repo, &registry, &mut store).unwrap();
    let total = first.stats.blobs_indexed;

    // Edit one source file and commit a new tree.
    write_file(
        &repo,
        "src/direct.rs",
        "pub fn add(a: i32, b: i32) -> i32 { inner_add(a, b) }\npub fn inner_add(a: i32, b: i32) -> i32 { a + b }\n",
    );
    commit_all(&repo, "edit direct");

    let second = index_path(&repo, &registry, &mut store).unwrap();
    assert_eq!(
        second.stats.blobs_extracted, 1,
        "exactly the one changed blob should re-extract: {:?}",
        second.stats
    );
    assert_eq!(
        second.stats.blobs_cached,
        total - 1,
        "all other blobs are cache hits across the tree change"
    );
}

#[test]
fn branch_switch_reextracts_only_changed_blobs() {
    use std::process::Command;
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();

    // Index main.
    let main = index_path(&repo, &registry, &mut store).unwrap();
    let total = main.stats.blobs_indexed;

    // Branch, change one file, commit, index the branch tip.
    let st = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["switch", "-q", "-c", "feature"])
        .status()
        .unwrap();
    assert!(st.success());
    write_file(
        &repo,
        "src/direct.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    );
    commit_all(&repo, "branch edit");

    let feature = index_path(&repo, &registry, &mut store).unwrap();
    assert_eq!(
        feature.stats.blobs_extracted, 1,
        "branch switch should only re-extract the diverged blob: {:?}",
        feature.stats
    );
    assert_eq!(feature.stats.blobs_cached, total - 1);
    assert_ne!(
        main.graph_key, feature.graph_key,
        "distinct branch tips key distinct Layer-2 graphs"
    );

    // Switching back to main is a pure cache hit (its blobs are all stored).
    let st = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["switch", "-q", "main"])
        .status()
        .unwrap();
    assert!(st.success());
    let back = index_path(&repo, &registry, &mut store).unwrap();
    assert_eq!(
        back.stats.blobs_extracted, 0,
        "returning to main extracts nothing: {:?}",
        back.stats
    );
    assert_eq!(back.graph_key, main.graph_key);
}

#[test]
fn two_index_runs_produce_byte_identical_graph() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();

    // Two fully independent stores; same committed tree.
    let mut store_a = mem_store();
    let out_a = index_path(&repo, &registry, &mut store_a).unwrap();
    let graph_a = read_graph(&store_a, out_a.graph_id);

    let mut store_b = mem_store();
    let out_b = index_path(&repo, &registry, &mut store_b).unwrap();
    let graph_b = read_graph(&store_b, out_b.graph_id);

    assert_eq!(out_a.graph_key, out_b.graph_key, "same tree OID key");
    assert_eq!(graph_a, graph_b, "stored graph must be deterministic");
    assert_eq!(out_a.stats, out_b.stats, "stats must be deterministic");
}

#[test]
fn determinism_holds_for_typescript_too() {
    let (_tmp, repo) = init_fixture_repo("ts-sample");
    let registry = default_registry();

    let mut store_a = mem_store();
    let id_a = index_path(&repo, &registry, &mut store_a).unwrap().graph_id;
    let a = read_graph(&store_a, id_a);
    let mut store_b = mem_store();
    let id_b = index_path(&repo, &registry, &mut store_b).unwrap().graph_id;
    let b = read_graph(&store_b, id_b);
    assert_eq!(a, b);
}
