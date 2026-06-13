//! End-to-end integration tests for the `cgx` binary (WP-10 convergence proof).
//!
//! Each test builds a throwaway git repo from a tiny, fully-controlled Rust
//! source tree, runs `cgx index`, then drives query subcommands through the
//! *built binary* (via `assert_cmd`). The headline test asserts that two runs of
//! the same query over the same index produce BYTE-IDENTICAL output — the
//! determinism contract that proves the cycle converged.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

/// A minimal two-file Rust crate with a known, hand-auditable call graph:
///
/// `main` (entrypoint) → `alpha` → `beta`; `orphan` is reachable from nobody.
/// `beta` lives in a second module so a cross-file/cross-module edge exists.
const MAIN_RS: &str = "\
mod helper;

fn main() {
    alpha();
}

fn alpha() {
    helper::beta();
}

fn orphan() {
    helper::beta();
}
";

const HELPER_RS: &str = "\
pub fn beta() {
    let _ = 1 + 1;
}
";

/// Build a fresh temp git repo containing the fixture crate, committed so it has a
/// real HEAD tree OID. Returns the temp dir (kept alive by the caller) and repo path.
fn fixture_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), MAIN_RS).unwrap();
    std::fs::write(repo.join("src/helper.rs"), HELPER_RS).unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

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

/// Run `cgx <args>` in `repo`, returning (stdout, exit code).
fn run_cgx(repo: &Path, args: &[&str]) -> (String, i32) {
    let out = Command::new(cargo_bin("cgx"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run cgx");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    (stdout, out.status.code().unwrap_or(-1))
}

fn index(repo: &Path) {
    let (_out, code) = run_cgx(repo, &["index"]);
    assert_eq!(code, 0, "index should succeed");
}

#[test]
fn index_reports_stats_and_persists_pointer() {
    let (_tmp, repo) = fixture_repo();
    let (out, code) = run_cgx(&repo, &["index"]);
    assert_eq!(code, 0);
    assert!(out.contains("nodes"), "stats line present: {out}");
    assert!(out.contains("edges"));
    assert!(
        repo.join(".cgx/HEAD.json").exists(),
        "index pointer written"
    );
    assert!(repo.join(".cgx/index.db").exists(), "store written");
}

#[test]
fn callees_finds_transitive_target() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["callees", "main"]);
    assert_eq!(code, 0);
    // main → alpha → beta: both appear.
    assert!(out.contains("rust_sample::alpha"), "alpha reached: {out}");
    assert!(
        out.contains("rust_sample::helper::beta"),
        "beta reached: {out}"
    );
}

#[test]
fn callers_finds_all_callers_of_beta() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["callers", "beta"]);
    assert_eq!(code, 0);
    // beta is called directly by alpha and orphan.
    assert!(
        out.contains("rust_sample::alpha"),
        "alpha is a caller: {out}"
    );
    assert!(
        out.contains("rust_sample::orphan"),
        "orphan is a caller: {out}"
    );
}

/// The convergence criterion: two runs of the same query over the same index
/// produce byte-identical output (architecture §4 determinism).
#[test]
fn query_output_is_byte_identical_across_two_runs() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);

    let (a, ca) = run_cgx(&repo, &["callers", "beta", "--format", "json"]);
    let (b, cb) = run_cgx(&repo, &["callers", "beta", "--format", "json"]);
    assert_eq!(ca, 0);
    assert_eq!(cb, 0);
    assert_eq!(a, b, "two query runs must be byte-identical");

    // And a second subcommand, to cover more of the formatter surface.
    let (c, _) = run_cgx(&repo, &["callees", "main", "--format", "sarif"]);
    let (d, _) = run_cgx(&repo, &["callees", "main", "--format", "sarif"]);
    assert_eq!(c, d, "sarif output must be byte-identical across runs");
}

/// A re-index of the same committed tree must reproduce an identical pointer, so
/// queries off two independent indexes match too (index-level determinism).
#[test]
fn reindex_is_stable() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let first = std::fs::read_to_string(repo.join(".cgx/HEAD.json")).unwrap();
    index(&repo);
    let second = std::fs::read_to_string(repo.join(".cgx/HEAD.json")).unwrap();
    assert_eq!(first, second, "re-index pointer is identical");
}

#[test]
fn unknown_symbol_is_usage_error_exit_2() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(&repo, &["callers", "does_not_exist"]);
    assert_eq!(code, 2, "unresolved symbol → exit 2");
}

#[test]
fn query_without_index_is_graph_error_exit_3() {
    let (_tmp, repo) = fixture_repo();
    // No `cgx index` run.
    let (_out, code) = run_cgx(&repo, &["callers", "beta"]);
    assert_eq!(code, 3, "querying an un-indexed repo → exit 3");
}

#[test]
fn assert_empty_fires_when_results_exist_exit_1() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    // beta has callers, so --assert-empty should fire.
    let (_out, code) = run_cgx(&repo, &["callers", "beta", "--assert-empty"]);
    assert_eq!(code, 1, "results present under --assert-empty → exit 1");
}

#[test]
fn assert_empty_vacuous_on_zero_symbol_exit_4() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    // A glob matching nothing → zero-symbol match → vacuous pass → exit 4.
    let (_out, code) = run_cgx(&repo, &["callers", "no_such_*", "--assert-empty"]);
    assert_eq!(code, 4, "zero-symbol --assert-empty → exit 4");
}

#[test]
fn assert_empty_vacuous_suppressed_by_allow_vacuous_exit_0() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(
        &repo,
        &["callers", "no_such_*", "--assert-empty", "--allow-vacuous"],
    );
    assert_eq!(code, 0, "--allow-vacuous downgrades the vacuous pass to 0");
}

#[test]
fn unused_reports_orphan() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["unused", "--format", "json"]);
    assert_eq!(code, 0);
    // `orphan` is reachable from no entrypoint (only `main` is).
    assert!(
        out.contains("rust_sample::orphan"),
        "orphan is unused: {out}"
    );
}
