//! TypeScript language-coverage end-to-end smoke: prove the language-agnostic
//! index/query/dataflow pipeline lights up on the committed `fixtures/ts` tree
//! via the *built `cgx` binary*, mirroring the `go_e2e` / `python_e2e` harness.
//!
//! Claim: `flows-to`/`flows-from` resolve the intraprocedural chain
//! a -> b -> c -> return in `dataflow.ts`, proving the `DerivesFrom` edge
//! DIRECTION (derived -> source) survives the real resolver end-to-end from the
//! TS frontend's facts. (Per-transform-class coverage lives in the
//! `cgx-lang-ts` `dataflow` suite.)
//!
//! Value-node FQNs (TS module prefix = `ts_sample::<file-stem>`; `cgx search`):
//!   ts_sample::dataflow::transform::a#0       (the parameter, SSA root)
//!   ts_sample::dataflow::transform::b#1       (const b = a)
//!   ts_sample::dataflow::transform::c#1       (const c = b + 1)
//!   ts_sample::dataflow::transform::return#1  (return c)

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

const A: &str = "ts_sample::dataflow::transform::a#0";
const B: &str = "ts_sample::dataflow::transform::b#1";
const C: &str = "ts_sample::dataflow::transform::c#1";
const RETURN: &str = "ts_sample::dataflow::transform::return#1";

/// Absolute path to the workspace `fixtures/ts` source tree.
fn fixture_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
        .join("ts")
}

/// Copy `fixtures/ts` into a fresh temp dir, `git init` it, and commit so it has
/// a real HEAD tree OID. Returns the temp dir (kept alive by the caller).
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
    assert_eq!(code, 0, "index of fixtures/ts should succeed: {e}");
}

/// The `fqn` field of each result of a `{count, results}` envelope query.
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
fn flows_to_resolves_transform_chain() {
    // `flows-to a` walks the DerivesFrom forward slice a -> b -> c -> return.
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

#[test]
fn flows_from_return_walks_pedigree_to_param() {
    // The mirror: `flows-from return` walks the pedigree return -> c -> b -> a.
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let fqns = result_fqns(
        &repo,
        &["flows-from", RETURN, "--depth", "8", "--format", "json"],
    );
    assert!(
        fqns.iter().any(|f| f == A),
        "flows-from return must reach a through the pedigree, got {fqns:?}"
    );
}
