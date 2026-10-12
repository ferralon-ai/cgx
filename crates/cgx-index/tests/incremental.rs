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

    let first = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    assert!(first.stats.blobs_extracted > 0, "first run must extract");
    assert_eq!(first.stats.blobs_cached, 0, "first run has no cache");

    // Second run against the SAME store: every blob OID is already cached.
    let second = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
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

    let first = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    let total = first.stats.blobs_indexed;

    // Edit one source file and commit a new tree.
    write_file(
        &repo,
        "src/direct.rs",
        "pub fn add(a: i32, b: i32) -> i32 { inner_add(a, b) }\npub fn inner_add(a: i32, b: i32) -> i32 { a + b }\n",
    );
    commit_all(&repo, "edit direct");

    let second = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
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
    let main = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
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

    let feature = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
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
    let back = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
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
    let out_a = index_path(&repo, &registry, &mut store_a, &Default::default()).unwrap();
    let graph_a = read_graph(&store_a, &out_a.graph_key);

    let mut store_b = mem_store();
    let out_b = index_path(&repo, &registry, &mut store_b, &Default::default()).unwrap();
    let graph_b = read_graph(&store_b, &out_b.graph_key);

    assert_eq!(out_a.graph_key, out_b.graph_key, "same tree OID key");
    assert_eq!(graph_a, graph_b, "stored graph must be deterministic");
    assert_eq!(out_a.stats, out_b.stats, "stats must be deterministic");
}

#[test]
fn determinism_holds_for_typescript_too() {
    let (_tmp, repo) = init_fixture_repo("ts-sample");
    let registry = default_registry();

    let mut store_a = mem_store();
    let id_a = index_path(&repo, &registry, &mut store_a, &Default::default()).unwrap().graph_key;
    let a = read_graph(&store_a, &id_a);
    let mut store_b = mem_store();
    let id_b = index_path(&repo, &registry, &mut store_b, &Default::default()).unwrap().graph_key;
    let b = read_graph(&store_b, &id_b);
    assert_eq!(a, b);
}

#[test]
fn stale_frontend_version_fragment_is_reextracted_and_overwritten() {
    use cgx_core::codec::encode;
    use cgx_frontend::RelPath;
    use cgx_store::{FactStore, FragmentInput};

    let (_tmp, repo) = init_empty_repo();
    write_file(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");
    commit_all(&repo, "one file");

    let blob = fragment_key(&repo, "src/lib.rs");

    let registry = default_registry();
    let current = registry
        .frontend_for(&RelPath::new("src/lib.rs"))
        .fragment_version();
    let mut store = mem_store();
    // Garbage bytes under an older version: reading them would be a decode error.
    store
        .put_fragments(&[FragmentInput {
            key: &blob,
            lang: "rust",
            frontend_version: current - 1,
            bytes: &encode(&"not a FileFacts").unwrap(),
        }])
        .unwrap();

    let first = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    assert_eq!(first.stats.blobs_extracted, 1, "{:?}", first.stats);
    assert_eq!(first.stats.blobs_stale, 1, "{:?}", first.stats);
    assert_eq!(first.stats.blobs_cached, 0, "{:?}", first.stats);
    assert_eq!(
        store.fragment(&blob).unwrap().unwrap().frontend_version,
        current,
        "the stale fragment is overwritten at the current version"
    );

    let second = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    assert_eq!(second.stats.blobs_cached, 1, "{:?}", second.stats);
    assert_eq!(second.stats.blobs_extracted, 0, "{:?}", second.stats);
    assert_eq!(second.stats.blobs_stale, 0, "{:?}", second.stats);
}

/// Node FQNs whose `file` is `path`, sorted.
fn fqns_in(g: &cgx_store::LinkedGraph, path: &str) -> Vec<String> {
    let mut v: Vec<String> = g
        .nodes
        .iter()
        .filter(|n| n.file == path)
        .map(|n| n.fqn.clone())
        .collect();
    v.sort();
    v
}

#[test]
fn identical_blobs_at_different_paths_keep_their_own_fqns_when_warm() {
    let (_tmp, repo) = init_empty_repo();
    let body = "def fmt(x):\n    return helper(x)\n\ndef helper(x):\n    return x\n";
    write_file(&repo, "pkg/nb/formats.py", body);
    write_file(&repo, "pkg/nn/formats.py", body);
    write_file(&repo, "pkg/other.py", "def g():\n    return 1\n");
    commit_all(&repo, "two identical blobs at two paths");

    let registry = default_registry();
    let mut store = mem_store();
    index_path(&repo, &registry, &mut store, &Default::default()).unwrap();

    // An unrelated edit forces a warm re-index of a new tree.
    write_file(&repo, "pkg/other.py", "def g():\n    return 2\n");
    commit_all(&repo, "unrelated edit");
    let warm = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    assert_eq!(warm.stats.blobs_extracted, 1, "{:?}", warm.stats);
    assert_eq!(warm.stats.blobs_cached, 2, "{:?}", warm.stats);

    let g = read_graph(&store, &warm.graph_key);
    for (path, own, other) in [
        ("pkg/nb/formats.py", "::nb::", "::nn::"),
        ("pkg/nn/formats.py", "::nn::", "::nb::"),
    ] {
        let fqns = fqns_in(&g, path);
        assert!(!fqns.is_empty(), "no nodes for {path}");
        for fqn in &fqns {
            assert!(
                fqn.contains(own) && !fqn.contains(other),
                "{path} carries a foreign FQN {fqn}: {fqns:?}"
            );
        }
    }
}

#[test]
fn package_rename_invalidates_unchanged_blob_fragment() {
    let (_tmp, repo) = init_empty_repo();
    let manifest = |name: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n");
    write_file(&repo, "Cargo.toml", &manifest("alpha"));
    write_file(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");
    commit_all(&repo, "alpha");

    let registry = default_registry();
    let mut store = mem_store();
    let first = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    let fqns = fqns_in(&read_graph(&store, &first.graph_key), "src/lib.rs");
    assert!(fqns.iter().any(|f| f.starts_with("alpha::")), "{fqns:?}");

    // Only the manifest changes; src/lib.rs keeps its blob OID.
    write_file(&repo, "Cargo.toml", &manifest("beta"));
    commit_all(&repo, "rename crate");
    let second = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    let fqns = fqns_in(&read_graph(&store, &second.graph_key), "src/lib.rs");
    assert!(!fqns.is_empty());
    assert!(
        fqns.iter().all(|f| !f.starts_with("alpha::")),
        "stale crate root survived the rename: {fqns:?}"
    );
    assert!(fqns.iter().any(|f| f.starts_with("beta::")), "{fqns:?}");
}

/// The fragment key the pipeline derives for `rel` in a repo with no build
/// manifest (no package, no manifest context).
fn fragment_key(repo: &std::path::Path, rel: &str) -> cgx_store::FragmentKey {
    let ctx = cgx_frontend::FileCtx::new(rel, hash_object(repo, rel));
    cgx_store::FragmentKey::derive(&ctx.cache_identity())
}

fn hash_object(repo: &std::path::Path, rel: &str) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["hash-object", rel])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn legacy_blob_oid_keyed_fragment_reads_as_miss() {
    use cgx_core::codec::encode;
    use cgx_frontend::{FileFacts, RelPath};
    use cgx_store::{FactStore, FragmentInput, FragmentKey};

    let (_tmp, repo) = init_empty_repo();
    write_file(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");
    commit_all(&repo, "one file");

    // A store written before fragments were path-keyed holds this blob's facts
    // under the bare blob OID, at the current frontend version. Here those facts
    // are empty, so a wrong hit would drop `f` from the graph.
    let registry = default_registry();
    let current = registry
        .frontend_for(&RelPath::new("src/lib.rs"))
        .fragment_version();
    let legacy = FragmentKey::new(hash_object(&repo, "src/lib.rs"));
    let mut store = mem_store();
    store
        .put_fragments(&[FragmentInput {
            key: &legacy,
            lang: "rust",
            frontend_version: current,
            bytes: &encode(&FileFacts::default()).unwrap(),
        }])
        .unwrap();

    let run = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    assert_eq!(run.stats.blobs_extracted, 1, "{:?}", run.stats);
    assert_eq!(run.stats.blobs_cached, 0, "{:?}", run.stats);
    assert_eq!(run.stats.blobs_stale, 0, "{:?}", run.stats);
    let fqns = fqns_in(&read_graph(&store, &run.graph_key), "src/lib.rs");
    assert!(!fqns.is_empty(), "the legacy fragment was read as a hit");
}
