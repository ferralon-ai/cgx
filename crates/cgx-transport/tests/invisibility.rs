//! Criterion 3 — history invisibility: `cgx push` writes no commit on any branch
//! and never touches `refs/heads/*`, so plain `git log`/`git blame`/branch-walk
//! output is unchanged by a push. Only the branch-history walk is asserted — a
//! `refs/cgx/index` commit IS visible under `git log --all` by design (D-c); that
//! is not part of this criterion.

mod common;

use common::{bare_remote, fixture_repo, git, index};

#[test]
fn push_does_not_move_head_or_change_branch_log() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);

    let head_before = git(&repo, &["rev-parse", "HEAD"]);
    let log_before = git(&repo, &["log", "--oneline", "main"]);
    let blame_before = git(&repo, &["blame", "--porcelain", "go.mod"]);

    let (_tmp_remote, remote) = bare_remote();
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    cgx_transport::push(&repo, "origin").expect("push");

    let head_after = git(&repo, &["rev-parse", "HEAD"]);
    let log_after = git(&repo, &["log", "--oneline", "main"]);
    let blame_after = git(&repo, &["blame", "--porcelain", "go.mod"]);

    assert_eq!(head_before, head_after, "HEAD must not move on push");
    assert_eq!(log_before, log_after, "branch-history walk (`git log main`) must be unchanged by push");
    assert_eq!(blame_before, blame_after, "`git blame` on a tracked file must be unchanged by push");
}

#[test]
fn push_writes_no_ref_under_refs_heads() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);

    let heads_before = git(&repo, &["for-each-ref", "refs/heads/"]);

    let (_tmp_remote, remote) = bare_remote();
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    cgx_transport::push(&repo, "origin").expect("push");

    let heads_after = git(&repo, &["for-each-ref", "refs/heads/"]);
    assert_eq!(heads_before, heads_after, "push must not create or move any refs/heads/* ref");
}

#[test]
fn cgx_ref_is_absent_from_plain_log_but_present_under_log_all() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);

    let (_tmp_remote, remote) = bare_remote();
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let outcome = cgx_transport::push(&repo, "origin").expect("push");

    // Use full-hash `%H` output (not `--oneline`'s abbreviated hash) so the
    // containment check compares complete OIDs.
    let log_main = git(&repo, &["log", "--format=%H", "main"]);
    assert!(
        !log_main.contains(&outcome.commit_oid),
        "the cgx index commit must not appear in the plain branch-history walk"
    );

    // `refs/cgx/index` is a local ref too (advance_ref writes it before pushing),
    // so `git log --all` — which is NOT part of criterion 3 — does surface it.
    // Documented here per D-c rather than asserted as a failure.
    let log_all = git(&repo, &["log", "--format=%H", "--all"]);
    assert!(
        log_all.contains(&outcome.commit_oid),
        "sanity: `git log --all` DOES show the cgx ref's commit — confirms invisibility is a \
         property of the branch walk, not of the object being unreachable"
    );
}
