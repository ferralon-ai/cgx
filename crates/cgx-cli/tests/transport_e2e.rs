//! `cgx push`/`cgx pull`/`cgx configure-remote` end-to-end via the built binary
//! and a LOCAL bare remote (cgx remote is PRIVATE — no test here may touch a
//! network remote). Complements `crates/cgx-transport/tests/**`, which exercises
//! the crate API directly; this file proves the CLI wiring (verbs, flags, exit
//! codes, printed messages) documented in `findings/03-cli-wiring.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

/// Absolute path to the workspace `fixtures/go` source tree.
fn fixture_src() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../crates/cgx-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
        .join("go")
}

fn fixture_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    copy_dir(&fixture_src(), &repo);

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "cgx-test"]);
    git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &[
            "-c",
            "author.name=cgx-test",
            "-c",
            "author.email=cgx@test.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
            "--date=2020-01-01T00:00:00Z",
        ],
    );
    (tmp, repo)
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_COMMITTER_NAME", "cgx-test")
        .env("GIT_COMMITTER_EMAIL", "cgx@test.invalid")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// `git init --bare` a fresh temp dir as a LOCAL stand-in remote. cgx remote is
/// PRIVATE — every test here uses a local path, never a network URL.
fn bare_remote() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let remote = tmp.path().join("remote.git");
    let status = Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&remote)
        .status()
        .expect("git init --bare");
    assert!(status.success());
    (tmp, remote)
}

fn clone_bare(remote: &Path) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dst = tmp.path().join("clone");
    let status = Command::new("git")
        .args(["clone", "-q"])
        .arg(remote)
        .arg(&dst)
        .status()
        .expect("git clone");
    assert!(status.success());
    (tmp, dst)
}

/// Run `cgx <args>` with `cwd = repo`, returning (stdout, stderr, exit code).
fn run_cgx(repo: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(cargo_bin("cgx"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run cgx");
    (
        String::from_utf8(out.stdout).expect("utf8 stdout"),
        String::from_utf8(out.stderr).expect("utf8 stderr"),
        out.status.code().unwrap_or(-1),
    )
}

fn index(repo: &Path) {
    let (_o, e, code) = run_cgx(repo, &["index"]);
    assert_eq!(code, 0, "index should succeed: {e}");
}

#[test]
fn push_then_clone_configure_pull_reproduces_the_graph_via_dump() {
    let (_tmp_a, repo_a) = fixture_repo();
    index(&repo_a);

    let (_tmp_remote, remote) = bare_remote();
    git(&repo_a, &["remote", "add", "origin", remote.to_str().unwrap()]);

    let (out, err, code) = run_cgx(&repo_a, &["push"]);
    assert_eq!(code, 0, "push should succeed: {err}");
    assert!(
        out.contains("refs/cgx/index") && out.contains("advanced to"),
        "push output must name the ref and its new state, got: {out}"
    );

    let (dump_a, _e, code) = run_cgx(&repo_a, &["dump", "--format", "json"]);
    assert_eq!(code, 0);

    let (_tmp_b, repo_b) = clone_bare(&remote);
    let (out, err, code) = run_cgx(&repo_b, &["configure-remote"]);
    assert_eq!(code, 0, "configure-remote should succeed: {err}");
    assert!(out.contains("refs/cgx/*"), "configure-remote must name the refspec: {out}");

    let (out, err, code) = run_cgx(&repo_b, &["pull"]);
    assert_eq!(code, 0, "pull should succeed: {err}");
    assert!(
        out.contains("materialized") && out.contains("graph"),
        "pull output must print materialized counts, got: {out}"
    );

    let (dump_b, _e, code) = run_cgx(&repo_b, &["dump", "--format", "json"]);
    assert_eq!(code, 0);
    let a: serde_json::Value = serde_json::from_str(&dump_a).unwrap();
    let b: serde_json::Value = serde_json::from_str(&dump_b).unwrap();
    assert_eq!(a, b, "cgx dump must reproduce the same graph after push/pull round trip");
}

#[test]
fn second_push_of_unchanged_tree_reports_already_at_not_advanced() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_tmp_remote, remote) = bare_remote();
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);

    let (out1, err1, code1) = run_cgx(&repo, &["push"]);
    assert_eq!(code1, 0, "{err1}");
    assert!(out1.contains("advanced to"), "first push must advance the ref: {out1}");

    let (out2, err2, code2) = run_cgx(&repo, &["push"]);
    assert_eq!(code2, 0, "{err2}");
    assert!(
        out2.contains("already at"),
        "second push of an unchanged tree must be idempotent (\"already at\"), got: {out2}"
    );

    // Same commit OID both times (extract the trailing token from each message).
    let oid1 = out1.trim().rsplit(' ').next().unwrap();
    let oid2 = out2.trim().rsplit(' ').next().unwrap();
    assert_eq!(oid1, oid2, "two pushes of one unchanged tree must advance to the same OID");
}

#[test]
fn configure_remote_run_twice_does_not_duplicate_refspec_entries() {
    let (_tmp, repo) = fixture_repo();
    let (_tmp_remote, remote) = bare_remote();
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);

    let (_o, e, code) = run_cgx(&repo, &["configure-remote"]);
    assert_eq!(code, 0, "{e}");
    let (_o, e, code) = run_cgx(&repo, &["configure-remote"]);
    assert_eq!(code, 0, "{e}");

    let fetch_entries = git(&repo, &["config", "--get-all", "remote.origin.fetch"]);
    let refspec_count = fetch_entries
        .lines()
        .filter(|l| l.trim() == "+refs/cgx/*:refs/cgx/*")
        .count();
    assert_eq!(refspec_count, 1, "configure-remote must be idempotent, got entries: {fetch_entries:?}");
}

#[test]
fn pull_from_remote_with_no_index_fails_loudly_with_refspec_hint_exit_3() {
    let (_tmp, repo) = fixture_repo();
    let (_tmp_remote, remote) = bare_remote(); // never pushed to
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);

    let (out, err, code) = run_cgx(&repo, &["pull"]);
    assert_eq!(code, 3, "pull against a ref-less remote must exit 3 (graph error), stdout={out} stderr={err}");
    assert!(
        err.contains("refs/cgx/*"),
        "the failure must surface the refspec fix-hint, got stderr: {err}"
    );
    assert!(
        !out.contains("materialized"),
        "a failed pull must never print a success line, got stdout: {out}"
    );
}

#[test]
fn gc_prune_on_bare_remote_keeps_cgx_ref_and_pull_still_succeeds() {
    let (_tmp_a, repo_a) = fixture_repo();
    index(&repo_a);
    let (_tmp_remote, remote) = bare_remote();
    git(&repo_a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let (_o, e, code) = run_cgx(&repo_a, &["push"]);
    assert_eq!(code, 0, "{e}");

    git(&remote, &["gc", "--prune=now"]);
    let fsck = git(&remote, &["fsck", "--full"]);
    assert!(fsck.trim().is_empty(), "git fsck must be clean after gc: {fsck}");

    let (_tmp_b, repo_b) = clone_bare(&remote);
    let (_o, e, code) = run_cgx(&repo_b, &["configure-remote"]);
    assert_eq!(code, 0, "{e}");
    let (out, e, code) = run_cgx(&repo_b, &["pull"]);
    assert_eq!(code, 0, "pull after remote gc must still succeed: {e}");
    assert!(out.contains("materialized"), "pull must report materialized objects: {out}");
}
