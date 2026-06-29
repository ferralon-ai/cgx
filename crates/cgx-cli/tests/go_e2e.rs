//! Go language-coverage end-to-end smoke: prove the language-agnostic
//! index/query/dataflow pipeline lights up on the committed `fixtures/go` module
//! via the *built `cgx` binary*, mirroring the `rust-sample` dataflow harness.
//!
//! It is intentionally small — not a golden. Two claims:
//!   1. a structural query (`callers`) resolves a Go call edge — `Spawn` is the
//!      caller of `worker` through the `go worker()` spawn (`concurrency.go`);
//!   2. `flows-to` resolves the intraprocedural `Transform` chain a → b → c →
//!      return in `dataflow.go`.
//!
//! Value-node FQNs (from `cgx search Transform`):
//!   go_sample::Transform::a#0       (the parameter)
//!   go_sample::Transform::b#1       (b := a)
//!   go_sample::Transform::c#1       (c := b + 1)
//!   go_sample::Transform::return#1  (return c)

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

const A: &str = "go_sample::Transform::a#0";
const B: &str = "go_sample::Transform::b#1";
const C: &str = "go_sample::Transform::c#1";
const RETURN: &str = "go_sample::Transform::return#1";

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

/// Copy `fixtures/go` into a fresh temp dir, `git init` it, and commit so it has
/// a real HEAD tree OID. Returns the temp dir (kept alive by the caller) and path.
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

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_COMMITTER_NAME", "cgx-test")
        .env("GIT_COMMITTER_EMAIL", "cgx@test.invalid")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// Run `cgx <args>` in `repo`, returning (stdout, stderr, exit code).
fn run_cgx(repo: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(cargo_bin("cgx"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run cgx");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    (stdout, stderr, out.status.code().unwrap_or(-1))
}

/// Index the repo. Dataflow is on by default (SC6), so a bare `cgx index` builds
/// it — no flag needed.
fn index(repo: &Path) {
    let (_o, e, code) = run_cgx(repo, &["index"]);
    assert_eq!(code, 0, "index of fixtures/go should succeed: {e}");
}

/// The `fqn` field of each result of a query returning the `{count, results}`
/// envelope (`callers`/`flows-to`).
fn result_fqns(repo: &Path, args: &[&str]) -> Vec<String> {
    let (out, e, code) = run_cgx(repo, args);
    assert_eq!(code, 0, "{args:?} exits 0: {out}{e}");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("query json");
    parsed["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["fqn"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn structural_callers_resolves_go_spawn_edge() {
    // `go worker()` in `Spawn` makes `Spawn` a caller of `worker`. Proves the
    // structural call graph resolves Go refs end-to-end.
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let callers = result_fqns(&repo, &["callers", "go_sample::worker", "--format", "json"]);
    assert!(
        callers.iter().any(|f| f == "go_sample::Spawn"),
        "callers of worker must include Spawn, got {callers:?}"
    );
}

#[test]
fn flows_to_resolves_transform_chain() {
    // `flows-to a` walks the DerivesFrom slice a → b → c → return in `Transform`.
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let fqns = result_fqns(&repo, &["flows-to", A, "--depth", "8", "--format", "json"]);
    for expected in [B, C, RETURN] {
        assert!(
            fqns.iter().any(|f| f == expected),
            "flows-to a must reach {expected}, got {fqns:?}"
        );
    }
}
