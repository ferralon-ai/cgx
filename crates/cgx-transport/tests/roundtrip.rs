//! Criterion 2 — round-trip parity: `pull` after `push` materializes `.cgx/`
//! byte-identical to a fresh local index of the same tree, over the
//! content-addressed committable set (`objects/`+`refs/`+`HEAD.json` only; D-a
//! excludes `fragments/`, `index.db*`, `cache.db`). Exercises `cgx_transport::
//! push`/`pull`/`configure_remote` directly as library calls.

mod common;

use std::process::Command;

use common::{bare_remote, committable_set, dump_json, fixture_repo, git, index};

#[test]
fn pull_materializes_committable_set_byte_identical_to_local_index() {
    let (_tmp_a, repo_a) = fixture_repo();
    index(&repo_a);
    let expected = committable_set(&repo_a);
    assert!(
        !expected.is_empty(),
        "sanity: a freshly-indexed repo must have a non-empty committable set"
    );

    let (_tmp_remote, remote) = bare_remote();
    git(&repo_a, &["remote", "add", "origin", remote.to_str().unwrap()]);

    let push_outcome =
        cgx_transport::push(&repo_a, "origin").expect("push to local bare remote");
    assert!(push_outcome.objects_promoted > 0, "push must promote at least one object");

    // Clone fresh (no `.cgx/` of its own — pull must materialize it from nothing).
    let (_tmp_b, repo_b) = clone_bare(&remote);
    cgx_transport::configure_remote(&repo_b, "origin").expect("configure-remote on clone");
    let pull_outcome = cgx_transport::pull(&repo_b, "origin").expect("pull into clone");
    assert!(
        pull_outcome.objects_materialized > 0,
        "pull must materialize at least one object"
    );

    let actual = committable_set(&repo_b);
    assert_eq!(
        expected, actual,
        "pulled .cgx/{{objects,refs,HEAD.json}} must be byte-identical to the source's committable set"
    );

    // Cheap cross-check: the pulled graph reads back identically to the source
    // graph via `cgx dump --format json`, independent of the on-disk byte compare
    // above.
    assert_eq!(
        dump_json(&repo_a),
        dump_json(&repo_b),
        "cgx dump --format json must agree between source and pulled clone"
    );
}

#[test]
fn pull_object_count_matches_push_object_count() {
    let (_tmp_a, repo_a) = fixture_repo();
    index(&repo_a);

    let (_tmp_remote, remote) = bare_remote();
    git(&repo_a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let push_outcome = cgx_transport::push(&repo_a, "origin").expect("push");

    let (_tmp_b, repo_b) = clone_bare(&remote);
    cgx_transport::configure_remote(&repo_b, "origin").expect("configure-remote");
    let pull_outcome = cgx_transport::pull(&repo_b, "origin").expect("pull");

    assert_eq!(
        push_outcome.objects_promoted, pull_outcome.objects_materialized,
        "every promoted object must be materialized on the far side"
    );
}

/// `git clone --bare <src> <dst>` into a fresh temp dir, returning a NON-bare
/// working copy so `.cgx/` can be materialized into it by `pull`.
fn clone_bare(remote: &std::path::Path) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dst = tmp.path().join("clone");
    let status = Command::new("git")
        .args(["clone", "-q"])
        .arg(remote)
        .arg(&dst)
        .status()
        .expect("git clone");
    assert!(status.success(), "git clone failed");
    (tmp, dst)
}
