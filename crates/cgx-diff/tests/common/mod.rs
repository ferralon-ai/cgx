//! Self-contained test harness: build throwaway git repos with controlled Rust
//! source across two commits, index each committed tree, and read its graph.
#![allow(dead_code)]

use cgx_index::{default_registry, index_path};
use cgx_store::{FactStore, GraphId, LinkedGraph, SqliteStore};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A temp git repo we control file-by-file. Holds the tempdir alive.
pub struct TestRepo {
    pub dir: tempfile::TempDir,
    pub path: PathBuf,
    pub store: SqliteStore,
}

impl TestRepo {
    /// Init a fresh `git`-backed repo with a deterministic identity.
    pub fn init() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["config", "user.name", "cgx-test"]);
        git(&path, &["config", "user.email", "cgx@test.invalid"]);
        git(&path, &["config", "commit.gpgsign", "false"]);
        let store = SqliteStore::open_in_memory().expect("store");
        TestRepo { dir, path, store }
    }

    /// Write a file (relative path), creating parent dirs.
    pub fn write(&self, rel: &str, text: &str) {
        let p = self.path.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, text).unwrap();
    }

    /// `git add -A && git commit` at the given author date (RFC3339), returning
    /// the new commit's hex OID.
    pub fn commit(&self, msg: &str, date: &str) -> String {
        git(&self.path, &["add", "-A"]);
        git(
            &self.path,
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
        rev_parse(&self.path, "HEAD")
    }

    /// `git checkout -b <name>`.
    pub fn branch(&self, name: &str) {
        git(&self.path, &["checkout", "-q", "-b", name]);
    }

    /// `git checkout <name>`.
    pub fn checkout(&self, name: &str) {
        git(&self.path, &["checkout", "-q", name]);
    }

    /// `git checkout --orphan <name>` — start a **disjoint** history. The only way
    /// to build a rev range whose walk actually reaches a root commit: `A..B`
    /// hides `A`'s ancestors, so a root shared with `A` is never returned.
    pub fn orphan_branch(&self, name: &str) {
        git(&self.path, &["checkout", "-q", "--orphan", name]);
    }

    /// `git merge --no-ff -m <msg> <name>`, returning the merge commit's hex OID.
    ///
    /// Goes through the same `git()` helper as every other command so it inherits
    /// the pinned `GIT_COMMITTER_*` env — a merge created any other way has a
    /// non-reproducible OID and makes every test that walks past it flaky.
    pub fn merge_no_ff(&self, name: &str, msg: &str) -> String {
        git(
            &self.path,
            &[
                "-c",
                "author.name=cgx-test",
                "-c",
                "author.email=cgx@test.invalid",
                "merge",
                "-q",
                "--no-ff",
                "-m",
                msg,
                name,
            ],
        );
        rev_parse(&self.path, "HEAD")
    }

    /// `git tag -a <name> -m <msg>` at HEAD — an **annotated** tag, i.e. a real tag
    /// object that `rev-parse` resolves to instead of the commit. Returns the tag
    /// object's hex OID (deliberately *not* the commit's, so a caller can assert
    /// the two differ). Goes through the same `git()` helper, whose pinned
    /// `GIT_COMMITTER_*` env is what `git tag` uses for the tagger line, so the tag
    /// object's OID is reproducible.
    pub fn annotated_tag(&self, name: &str, msg: &str) -> String {
        git(&self.path, &["tag", "-a", name, "-m", msg]);
        rev_parse(&self.path, name)
    }

    /// `git config <key> <value>` (repo-local).
    pub fn config(&self, key: &str, value: &str) {
        git(&self.path, &["config", key, value]);
    }

    /// Index the current committed HEAD tree into the store, returning its graph.
    pub fn index_head(&mut self) -> (GraphId, LinkedGraph) {
        let registry = default_registry();
        let outcome = index_path(&self.path, &registry, &mut self.store, &Default::default()).expect("index");
        let graph = self.store.read_graph(outcome.graph_id).expect("read");
        (outcome.graph_id, graph)
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
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn rev_parse(repo: &Path, spec: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", spec])
        .output()
        .expect("rev-parse");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Helper: collect the `(src_fqn, dst_fqn)` pairs of a diff's added edges.
pub fn added_pairs(diff: &cgx_diff::GraphDiff) -> Vec<(String, String)> {
    diff.added_edges
        .iter()
        .map(|e| (e.identity.src_fqn.clone(), e.identity.dst_fqn.clone()))
        .collect()
}

/// Helper: collect the `(src_fqn, dst_fqn)` pairs of a diff's removed edges.
pub fn removed_pairs(diff: &cgx_diff::GraphDiff) -> Vec<(String, String)> {
    diff.removed_edges
        .iter()
        .map(|e| (e.identity.src_fqn.clone(), e.identity.dst_fqn.clone()))
        .collect()
}

/// Whether `pairs` contains an edge whose endpoints end with `::<src>` and
/// `::<dst>` (FQNs are module-prefixed, e.g. `rust_sample::caller`).
pub fn has_pair(pairs: &[(String, String)], src: &str, dst: &str) -> bool {
    let s = format!("::{src}");
    let d = format!("::{dst}");
    pairs
        .iter()
        .any(|(a, b)| a.ends_with(&s) && b.ends_with(&d))
}
