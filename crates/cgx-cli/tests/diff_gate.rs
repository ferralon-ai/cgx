//! CH-11 S0+S1 end-to-end: the `cgx diff` post-filters and the `--path-added`
//! structural gate, driven through the built binary.
//!
//! The headline fixture builds a two-commit repo where HEAD introduces a
//! `*::handler::* -> *::Command::*` call/dataflow reachability path absent at BASE;
//! the gate must flag it, exit nonzero, and name the introducing commit. A second
//! fixture proves the hard anchor guard: an unanchored `--path-added` is a usage
//! error (exit 2), never an unanchored walk.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

/// BASE: `handler::process` does NOT reach the exec sink.
const SRC_BASE: &str = "\
pub mod handler {
    pub fn process() {
        let _ = 1 + 1;
    }
}
pub mod sys {
    pub struct Command;
    impl Command {
        pub fn run() {}
    }
}
";

/// HEAD: `handler::process` now calls `sys::Command::run` — a brand-new
/// handler→exec path.
const SRC_HEAD: &str = "\
pub mod handler {
    pub fn process() {
        crate::sys::Command::run();
    }
}
pub mod sys {
    pub struct Command;
    impl Command {
        pub fn run() {}
    }
}
";

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

fn write(repo: &Path, rel: &str, text: &str) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn commit(repo: &Path, msg: &str, date: &str) {
    git(repo, &["add", "-A"]);
    git(
        repo,
        &[
            "-c",
            "author.name=cgx-test",
            "-c",
            "author.email=cgx@test.invalid",
            "commit",
            "-q",
            "-m",
            msg,
            &format!("--date={date}"),
        ],
    );
}

/// A two-commit repo: BASE has no handler→exec path, HEAD introduces it.
fn gate_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    write(
        &repo,
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "cgx-test"]);
    git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);

    write(&repo, "src/lib.rs", SRC_BASE);
    commit(&repo, "base", "2020-01-01T00:00:00Z");

    write(&repo, "src/lib.rs", SRC_HEAD);
    commit(&repo, "head: add handler->exec path", "2020-02-01T00:00:00Z");

    (tmp, repo)
}

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

#[test]
fn path_added_flags_new_handler_to_exec_path_with_nonzero_exit_and_commit() {
    let (_tmp, repo) = gate_fixture();
    let (stdout, _stderr, code) = run_cgx(
        &repo,
        &[
            "diff",
            "HEAD~1",
            "HEAD",
            "--path-added",
            "--from",
            "**::handler::*",
            "--to",
            "**::Command::*",
            "--format",
            "json",
        ],
    );

    assert_eq!(code, 1, "a forbidden new path exits 1: stdout={stdout}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let paths = doc["added_paths"].as_array().expect("added_paths array");
    assert_eq!(paths.len(), 1, "exactly one new path: {stdout}");
    let p = &paths[0];
    assert!(
        p["from"].as_str().unwrap().ends_with("::process"),
        "source is the handler: {p}"
    );
    assert!(
        p["to"].as_str().unwrap().contains("Command"),
        "sink is the exec boundary: {p}"
    );
    assert!(
        p["introducing_commit"].as_str().is_some(),
        "the introducing commit is attributed: {p}"
    );
    assert_eq!(
        doc["semantics"].as_str().unwrap(),
        "call/dataflow reachability (not a soundness/security guarantee)",
        "the honesty framing is present in the output"
    );
}

#[test]
fn path_added_is_clean_when_no_new_path() {
    let (_tmp, repo) = gate_fixture();
    // A sink glob that matches nothing → no new path → clean exit 0.
    let (stdout, _stderr, code) = run_cgx(
        &repo,
        &[
            "diff",
            "HEAD~1",
            "HEAD",
            "--path-added",
            "--from",
            "**::handler::*",
            "--to",
            "**::NoSuchSink::*",
        ],
    );
    assert_eq!(code, 0, "no new path exits 0: stdout={stdout}");
    assert!(
        stdout.contains("no new call/dataflow reachability path"),
        "clean message present: {stdout}"
    );
}

#[test]
fn unanchored_path_added_is_rejected_at_cli() {
    let (_tmp, repo) = gate_fixture();

    // Missing --to.
    let (_o, stderr, code) = run_cgx(
        &repo,
        &["diff", "HEAD~1", "HEAD", "--path-added", "--from", "**::handler::*"],
    );
    assert_eq!(code, 2, "missing --to is a usage error (exit 2)");
    assert!(
        stderr.contains("requires both --from and --to"),
        "the guard names the requirement: {stderr}"
    );

    // Missing --from.
    let (_o2, stderr2, code2) = run_cgx(
        &repo,
        &["diff", "HEAD~1", "HEAD", "--path-added", "--to", "**::Command::*"],
    );
    assert_eq!(code2, 2, "missing --from is a usage error (exit 2)");
    assert!(stderr2.contains("requires both --from and --to"));

    // Both missing.
    let (_o3, _stderr3, code3) = run_cgx(&repo, &["diff", "HEAD~1", "HEAD", "--path-added"]);
    assert_eq!(code3, 2, "fully unanchored is a usage error (exit 2)");
}

#[test]
fn newer_than_is_sugar_for_added_bucket() {
    let (_tmp, repo) = gate_fixture();

    let (newer, _e1, c1) = run_cgx(
        &repo,
        &["diff", "HEAD~1", "HEAD", "--newer-than", "--format", "json"],
    );
    assert_eq!(c1, 0);
    let (added, _e2, c2) = run_cgx(
        &repo,
        &["diff", "HEAD~1", "HEAD", "--added", "--format", "json"],
    );
    assert_eq!(c2, 0);

    let nd: serde_json::Value = serde_json::from_str(&newer).expect("json");
    let ad: serde_json::Value = serde_json::from_str(&added).expect("json");
    assert_eq!(
        nd["added_edges"], ad["added_edges"],
        "--newer-than and --added select the same added-edge set"
    );
    // The added bucket carries the new handler->Command call edge.
    assert!(
        ad["added_edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["dst"].as_str().map(|s| s.contains("Command")).unwrap_or(false)),
        "the new handler->Command edge is in the added bucket: {added}"
    );
}

#[test]
fn from_glob_post_filter_narrows_added_edges() {
    let (_tmp, repo) = gate_fixture();
    let (out, _e, code) = run_cgx(
        &repo,
        &[
            "diff",
            "HEAD~1",
            "HEAD",
            "--added",
            "--from",
            "**::handler::*",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    let doc: serde_json::Value = serde_json::from_str(&out).expect("json");
    for e in doc["added_edges"].as_array().unwrap() {
        assert!(
            e["src"].as_str().unwrap().contains("::handler::"),
            "every surviving added edge has a handler source: {e}"
        );
    }
}
