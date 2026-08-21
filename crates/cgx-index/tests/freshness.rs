//! The git-facing half of the index-freshness envelope: the committed tree's
//! path→blob-OID map and the working-tree divergence count.
//!
//! Table-driven where the cases share a shape (the mutation matrix below); the
//! degenerate repository states each need their own setup, so they stand alone.

mod common;

use cgx_index::Repo;
use common::*;
use std::collections::BTreeMap;
use std::path::Path;

/// The indexed map for the repo's current HEAD tree.
fn head_map(repo: &Path) -> (Repo, BTreeMap<String, String>) {
    let r = Repo::discover(repo).expect("discover");
    let oid = r.head_tree_oid().expect("head tree");
    let map = r.tree_blob_oids(&oid).expect("tree blob oids");
    (r, map)
}

#[test]
fn tree_blob_oids_matches_the_content_addressed_enumeration() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let (r, map) = head_map(&repo);

    let enumerated: BTreeMap<String, String> = r
        .enumerate_tree()
        .unwrap()
        .into_iter()
        .map(|s| (s.rel_path, s.blob_oid))
        .collect();
    assert!(!map.is_empty(), "fixture tree must have blobs");
    assert_eq!(
        map, enumerated,
        "the content-free tree walk must agree with the content-reading one"
    );
}

/// One working-tree mutation, and the divergence count it must produce.
struct Case {
    name: &'static str,
    /// `(relative path, Some(new content) | None to delete)`.
    edits: &'static [(&'static str, Option<&'static str>)],
    want_dirty: usize,
}

#[test]
fn dirty_file_count_counts_modified_added_and_removed() {
    let cases = [
        Case {
            name: "clean tree",
            edits: &[],
            want_dirty: 0,
        },
        Case {
            name: "one modified file",
            edits: &[("src/main.rs", Some("pub fn changed() {}\n"))],
            want_dirty: 1,
        },
        Case {
            name: "one added file",
            edits: &[("src/brand_new.rs", Some("pub fn added() {}\n"))],
            want_dirty: 1,
        },
        Case {
            name: "one removed file",
            edits: &[("src/main.rs", None)],
            want_dirty: 1,
        },
        Case {
            name: "modified plus added plus removed",
            edits: &[
                ("src/main.rs", Some("pub fn changed() {}\n")),
                ("src/brand_new.rs", Some("pub fn added() {}\n")),
                ("src/direct.rs", None),
            ],
            want_dirty: 3,
        },
    ];

    for c in cases {
        let (_tmp, repo) = init_fixture_repo("rust-sample");
        let (r, map) = head_map(&repo);
        assert!(
            map.contains_key("src/main.rs") && map.contains_key("src/direct.rs"),
            "fixture shape assumption: {}",
            c.name
        );
        for (rel, content) in c.edits {
            match content {
                Some(text) => write_file(&repo, rel, text),
                None => std::fs::remove_file(repo.join(rel)).unwrap(),
            }
        }
        assert_eq!(
            r.dirty_file_count(&map).unwrap(),
            c.want_dirty,
            "case: {}",
            c.name
        );
    }
}

#[test]
fn rewriting_a_file_with_identical_content_is_not_dirty() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let (r, map) = head_map(&repo);
    let path = repo.join("src/main.rs");
    let original = std::fs::read(&path).unwrap();
    // Touches mtime; content-addressing must see through it.
    std::fs::write(&path, &original).unwrap();
    assert_eq!(r.dirty_file_count(&map).unwrap(), 0);
}

#[test]
fn ignored_paths_do_not_count_as_dirty() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    write_file(&repo, ".gitignore", "target/\nscratch.log\n");
    commit_all(&repo, "add gitignore");

    let (r, map) = head_map(&repo);
    assert_eq!(r.dirty_file_count(&map).unwrap(), 0);

    // A build-artifact tree and a stray ignored file are both invisible.
    write_file(&repo, "target/debug/huge.bin", "binary-ish\n");
    write_file(&repo, "target/debug/deps/more.bin", "more\n");
    write_file(&repo, "scratch.log", "noise\n");
    assert_eq!(
        r.dirty_file_count(&map).unwrap(),
        0,
        "ignored paths can never be in a committed tree, so they are never divergence"
    );

    // A non-ignored addition alongside them still counts.
    write_file(&repo, "src/counted.rs", "pub fn counted() {}\n");
    assert_eq!(r.dirty_file_count(&map).unwrap(), 1);
}

#[test]
fn the_cgx_store_dir_is_never_counted_as_divergence() {
    // Once `.cgx/` is committable (Slice 2) its objects are no longer git-ignored,
    // and a user may commit them into the tree. The store is cgx's own artifact,
    // never source, so it must count as divergence on neither side: not as a
    // working-tree addition (disk-side prune) nor as a phantom removal when it is
    // present in the indexed tree (tree-side exclusion).
    let (_tmp, repo) = init_fixture_repo("rust-sample");

    // On disk but never committed: must not count as additions.
    write_file(&repo, ".cgx/HEAD.json", "{\"graph_key\":\"deadbeef\"}\n");
    write_file(&repo, ".cgx/objects/ab/cdef0123", "postcard-ish\n");
    write_file(&repo, ".cgx/refs/deadbeef", "oid\ndeadbeef\n");
    let (r, map) = head_map(&repo);
    assert!(!map.keys().any(|k| k.starts_with(".cgx/")));
    assert_eq!(
        r.dirty_file_count(&map).unwrap(),
        0,
        "an uncommitted .cgx/ store is not a working-tree addition"
    );

    // Now commit the store into the tree: it appears in the indexed map, but must
    // still not count as removals or divergence.
    commit_all(&repo, "commit the cgx store");
    let (r, map) = head_map(&repo);
    assert!(
        map.keys().any(|k| k.starts_with(".cgx/")),
        "the committed store must be in the indexed tree"
    );
    assert_eq!(
        r.dirty_file_count(&map).unwrap(),
        0,
        "a committed .cgx/ store, unchanged on disk, is not divergence"
    );

    // A real source edit still counts — the exclusion is scoped to the store.
    write_file(&repo, "src/counted.rs", "pub fn counted() {}\n");
    assert_eq!(r.dirty_file_count(&map).unwrap(), 1);
}

#[test]
fn a_tracked_but_ignored_file_is_still_compared() {
    // Git applies ignore rules only to untracked paths. A file that is both
    // committed and matched by .gitignore must still be diffed, not reported as
    // removed just because the ignore stack would exclude it.
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    write_file(&repo, "tracked.log", "one\n");
    commit_all(&repo, "track the file first");
    write_file(&repo, ".gitignore", "tracked.log\n");
    commit_all(&repo, "then start ignoring it");

    let (r, map) = head_map(&repo);
    assert!(map.contains_key("tracked.log"));
    assert_eq!(r.dirty_file_count(&map).unwrap(), 0);

    write_file(&repo, "tracked.log", "two\n");
    assert_eq!(r.dirty_file_count(&map).unwrap(), 1);
}

#[test]
fn an_index_behind_head_reports_the_committed_divergence() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let (r, old_map) = head_map(&repo);
    let old_oid = r.head_tree_oid().unwrap();

    write_file(&repo, "src/main.rs", "pub fn moved_on() {}\n");
    commit_all(&repo, "move HEAD forward");

    let new_oid = r.head_tree_oid().unwrap();
    assert_ne!(old_oid, new_oid, "HEAD tree must have moved");
    // The working tree is clean against the NEW tree but diverges from the old one.
    assert_eq!(
        r.dirty_file_count(&r.tree_blob_oids(&new_oid).unwrap())
            .unwrap(),
        0
    );
    assert_eq!(r.dirty_file_count(&old_map).unwrap(), 1);
}

/// A submodule is a single gitlink entry in the parent's tree, never a set of
/// blobs — so descending into it counts every file inside as an addition and a
/// clean checkout reports itself permanently stale. Nothing inside a submodule is
/// indexed either, so nothing inside it can be divergence.
#[test]
fn a_clean_checkout_with_a_submodule_is_clean() {
    let (_child_tmp, child) = init_empty_repo();
    write_file(&child, "a.rs", "pub fn a() {}\n");
    write_file(&child, "b.rs", "pub fn b() {}\n");
    write_file(&child, "c.rs", "pub fn c() {}\n");
    commit_all(&child, "child fixture");

    let (_tmp, repo) = init_fixture_repo("rust-sample");
    add_submodule(&repo, &child, "vendor/child");

    let (r, map) = head_map(&repo);
    assert!(
        !map.keys().any(|p| p.starts_with("vendor/child/")),
        "a gitlink contributes no indexed blob: {map:?}"
    );
    assert_eq!(
        r.dirty_file_count(&map).unwrap(),
        0,
        "a clean parent with a clean submodule is clean"
    );

    // The parent's own divergence is still seen, right next to the submodule.
    write_file(&repo, "src/counted.rs", "pub fn counted() {}\n");
    assert_eq!(r.dirty_file_count(&map).unwrap(), 1);
}

#[test]
fn a_repo_with_no_commits_has_no_head_tree() {
    let (_tmp, repo) = init_empty_repo();
    let r = Repo::discover(&repo).expect("discover an empty repo");
    assert!(
        r.head_tree_oid().is_err(),
        "an unborn HEAD must not resolve to a tree"
    );
    // Everything on disk is then an addition against an empty indexed set.
    write_file(&repo, "a.rs", "pub fn a() {}\n");
    assert_eq!(r.dirty_file_count(&BTreeMap::new()).unwrap(), 1);
}

#[test]
fn a_detached_head_still_resolves_a_tree() {
    let (_tmp, repo) = init_fixture_repo("rust-sample");
    let r = Repo::discover(&repo).unwrap();
    let attached = r.head_tree_oid().unwrap();

    detach_head(&repo);
    let r = Repo::discover(&repo).unwrap();
    assert_eq!(
        r.head_tree_oid().unwrap(),
        attached,
        "detaching HEAD at the same commit must not change the tree OID"
    );
    assert_eq!(
        r.dirty_file_count(&r.tree_blob_oids(&attached).unwrap())
            .unwrap(),
        0
    );
}

#[test]
fn a_directory_outside_the_repo_is_not_a_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("plain");
    std::fs::create_dir_all(&outside).unwrap();
    // `gix::discover` walks upward, so this only holds when no ancestor is a repo —
    // guard on that rather than asserting an environment-dependent failure.
    if let Ok(r) = Repo::discover(&outside) {
        assert!(
            r.workdir() != Some(outside.as_path()),
            "a plain directory must not present itself as this repo's workdir"
        );
    }
}
