//! Worktree / dirty-tree awareness (docs/06 IX-6, IX-3).
//!
//! - Indexing a working directory includes uncommitted files (synthetic blob OID).
//! - A committed-then-worktree index shares Layer-1 cache hits: identical content
//!   yields the identical blob OID, so the second pass extracts nothing new.
//! - A second linked worktree of the same repo reuses the shared cache.

mod common;

use cgx_index::{compute_blob_oid, default_registry, index_path, index_workdir, Repo};
use common::*;
use std::process::Command;

#[test]
fn workdir_index_includes_uncommitted_file() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();

    // Add an uncommitted source file referencing an existing symbol.
    write_file(
        &repo,
        "src/uncommitted.rs",
        "pub fn brand_new_entrypoint() -> i32 { helper() }\npub fn helper() -> i32 { 7 }\n",
    );

    let out = index_workdir(&repo, &repo, &registry, &mut store, &Default::default()).unwrap();
    let g = read_graph(&store, &out.graph_key);
    let idx = GraphIndex::new(&g);
    assert!(
        idx.has_node("rust_sample::uncommitted::brand_new_entrypoint"),
        "uncommitted file was not indexed"
    );
    assert!(idx.has_edge(
        "rust_sample::uncommitted::brand_new_entrypoint",
        "rust_sample::uncommitted::helper"
    ));
}

#[test]
fn committed_then_workdir_shares_layer1_cache() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();

    // 1) Index the committed tree (warms the Layer-1 cache).
    let committed = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    assert!(committed.stats.blobs_extracted > 0);

    // 2) Index the working directory with NO edits — every file's content matches
    //    its committed blob, so every blob OID is a cache hit (IX-6 sharing).
    let work = index_workdir(&repo, &repo, &registry, &mut store, &Default::default()).unwrap();
    assert_eq!(
        work.stats.blobs_extracted, 0,
        "clean working dir must hit the committed Layer-1 cache: {:?}",
        work.stats
    );
    assert_eq!(work.stats.blobs_cached, work.stats.blobs_indexed);
}

#[test]
fn dirty_edit_reextracts_only_dirty_blob() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();

    let committed = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    let total = committed.stats.blobs_indexed;

    // Edit one file WITHOUT committing; index the working dir.
    write_file(
        &repo,
        "src/direct.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    );
    let work = index_workdir(&repo, &repo, &registry, &mut store, &Default::default()).unwrap();
    assert_eq!(
        work.stats.blobs_extracted, 1,
        "only the dirty blob re-extracts: {:?}",
        work.stats
    );
    assert_eq!(work.stats.blobs_cached, total - 1);
}

#[test]
fn synthetic_blob_oid_matches_git() {
    // The synthetic OID used for dirty files is the exact OID git would assign.
    let content = b"fn x() {}\n";
    let ours = compute_blob_oid(content);
    let theirs = git_hash_object(content);
    assert_eq!(ours, theirs, "synthetic blob OID must equal git's");
}

#[test]
fn second_worktree_shares_layer1_cache() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();

    // Index the main checkout's committed tree.
    let main = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    assert!(main.stats.blobs_extracted > 0);

    // Create a linked worktree on a new branch (shares .git/objects).
    let wt_dir = repo.parent().unwrap().join("wt-feature");
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["worktree", "add", "-q", "-b", "feature"])
        .arg(&wt_dir)
        .status()
        .unwrap();
    assert!(status.success(), "git worktree add failed");

    // Enumeration finds the linked worktree.
    let r = Repo::discover(&repo).unwrap();
    let dirs = r.linked_worktree_dirs().unwrap();
    assert!(
        dirs.iter().any(|d| d.ends_with("wt-feature")),
        "linked worktree not enumerated: {dirs:?}"
    );

    // Indexing the worktree's working dir: same content -> all cache hits.
    let work = index_workdir(&repo, &wt_dir, &registry, &mut store, &Default::default()).unwrap();
    assert_eq!(
        work.stats.blobs_extracted, 0,
        "worktree with identical content must reuse the shared Layer-1 cache: {:?}",
        work.stats
    );
}

fn git_hash_object(content: &[u8]) -> String {
    use std::io::Write;
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(content).unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}
