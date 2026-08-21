//! Shared fixture helpers for `cgx-transport` crate-API integration tests.
//!
//! Fixture repos are built the same way as the `cgx-cli` e2e suite (`go_e2e.rs`,
//! `dataflow.rs`): copy a committed fixture into a fresh temp dir, `git init` +
//! commit it so it has a real HEAD tree OID, then index it via the *built `cgx`
//! binary* — that's the only way to populate a genuine `.cgx/` committable set
//! without re-deriving the indexing pipeline in test code. Transport itself
//! (`push`/`pull`/`configure_remote`) is then exercised as a direct Rust function
//! call against the crate API, per this dispatch's scope.

#![allow(dead_code)] // not every helper is used by every test binary in this dir

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

/// Absolute path to the workspace `fixtures/go` source tree — small (24K) and
/// already used as an e2e fixture elsewhere, so it doubles as a light index input.
pub fn fixture_src() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../crates/cgx-transport
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
        .join("go")
}

/// Copy `fixtures/go` into a fresh temp dir, `git init` it, and commit so it has a
/// real HEAD tree OID. Returns the temp dir (kept alive by the caller) and path.
pub fn fixture_repo() -> (tempfile::TempDir, PathBuf) {
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

pub fn git(repo: &Path, args: &[&str]) -> String {
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

/// Run `cgx <args>` in `repo` via the built binary, requiring success.
pub fn run_cgx_ok(repo: &Path, args: &[&str]) -> String {
    let out = Command::new(cargo_bin("cgx"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run cgx");
    assert!(
        out.status.success(),
        "cgx {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Index `repo` (dataflow on by default; no flag needed).
pub fn index(repo: &Path) {
    run_cgx_ok(repo, &["index"]);
}

/// `cgx dump --format json` parsed to a [`serde_json::Value`] — a cheap
/// graph-equality cross-check independent of the on-disk committable set.
pub fn dump_json(repo: &Path) -> serde_json::Value {
    let out = run_cgx_ok(repo, &["dump", "--format", "json"]);
    serde_json::from_str(&out).expect("cgx dump --format json produces valid JSON")
}

/// `git init --bare` a fresh temp dir to stand in for a remote. LOCAL only — cgx
/// remote is PRIVATE; no test may point at a network remote.
pub fn bare_remote() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let remote = tmp.path().join("remote.git");
    let status = Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&remote)
        .status()
        .expect("git init --bare");
    assert!(status.success(), "git init --bare failed");
    (tmp, remote)
}

/// Read the content-addressed committable set under `.cgx/` — `objects/**`,
/// `refs/**`, and `HEAD.json` only (D-a: NOT `fragments/`, `index.db*`,
/// `cache.db`, `objects.lock`, `.gitignore`) — as a sorted map from the path
/// relative to `.cgx/` (forward-slash-normalized) to its exact bytes. Two repos'
/// maps compare byte-identical iff their committable sets are byte-identical.
pub fn committable_set(repo: &Path) -> BTreeMap<String, Vec<u8>> {
    let cgx_dir = repo.join(".cgx");
    let mut out = BTreeMap::new();
    for sub in ["objects", "refs"] {
        let dir = cgx_dir.join(sub);
        if !dir.exists() {
            continue;
        }
        walk(&dir, &cgx_dir, &mut out);
    }
    let head = cgx_dir.join("HEAD.json");
    if head.exists() {
        out.insert("HEAD.json".to_owned(), std::fs::read(&head).unwrap());
    }
    out
}

fn walk(dir: &Path, cgx_dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, cgx_dir, out);
        } else {
            let rel = path
                .strip_prefix(cgx_dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel, std::fs::read(&path).unwrap());
        }
    }
}
