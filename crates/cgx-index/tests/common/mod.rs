//! Shared test helpers: build throwaway git repos from the committed fixture
//! source trees, and inspect a stored linked graph by FQN.
//!
//! Shared across several test binaries; not every helper is used by each, so
//! dead-code is allowed here.
#![allow(dead_code)]

use cgx_core::{EdgeRecord, NodeRecord};
use cgx_store::{FactStore, GraphId, LinkedGraph, SqliteStore};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Absolute path to the workspace `fixtures/` directory.
pub fn fixtures_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../crates/cgx-index
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
}

/// Copy a fixture source tree into a fresh temp dir, `git init` it, and commit so
/// it has a real HEAD tree OID. Returns the temp dir (kept alive by the caller)
/// and the repo path. Git identity/config is set locally and deterministically so
/// blob/tree OIDs are reproducible.
pub fn init_fixture_repo(fixture: &str) -> (tempfile::TempDir, PathBuf) {
    let src = fixtures_root().join(fixture);
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    copy_dir(&src, &repo);

    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.name", "cgx-test"]);
    run_git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    run_git(&repo, &["config", "commit.gpgsign", "false"]);
    run_git(&repo, &["add", "-A"]);
    run_git(
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

/// Create a fresh, empty, deterministic git repo in a temp dir (no fixture copy).
/// Caller writes files with [`write_file`] then commits with [`commit_all`].
/// Returns the temp dir (kept alive by the caller) and the repo path.
pub fn init_empty_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.name", "cgx-test"]);
    run_git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    run_git(&repo, &["config", "commit.gpgsign", "false"]);
    (tmp, repo)
}

fn run_git(repo: &Path, args: &[&str]) {
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

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Write `text` to `path` (relative to `repo`). Leaves it uncommitted (use
/// [`commit_all`] to commit a new tree).
pub fn write_file(repo: &Path, rel: &str, text: &str) {
    let path = repo.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, text).unwrap();
}

/// `git add -A && git commit` so the working changes become a new HEAD tree.
pub fn commit_all(repo: &Path, msg: &str) {
    run_git(repo, &["add", "-A"]);
    run_git(
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
            "--date=2020-01-02T00:00:00Z",
        ],
    );
}

/// Add `child` as a submodule of `repo` at `rel` and commit it, so the parent's
/// tree holds a gitlink (mode `160000`) there. `protocol.file.allow` is set
/// locally because git refuses `file://`-style submodule sources by default
/// (CVE-2022-39253); this is a throwaway temp repo.
pub fn add_submodule(repo: &Path, child: &Path, rel: &str) {
    run_git(
        repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            child.to_str().expect("utf-8 child path"),
            rel,
        ],
    );
    commit_all(repo, "add submodule");
}

/// Detach `HEAD` at its current commit (leaves the tree untouched).
pub fn detach_head(repo: &Path) {
    run_git(repo, &["checkout", "-q", "--detach"]);
}

/// An in-memory store, fine for single-process tests.
pub fn mem_store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("open store")
}

/// Read back the stored graph for assertions.
pub fn read_graph(store: &SqliteStore, id: GraphId) -> LinkedGraph {
    store.read_graph(id).expect("read graph")
}

/// Index a graph by FQN for edge-existence assertions.
pub struct GraphIndex {
    pub nodes: Vec<NodeRecord>,
    pub edges: Vec<EdgeRecord>,
}

impl GraphIndex {
    pub fn new(g: &LinkedGraph) -> Self {
        GraphIndex {
            nodes: g.nodes.clone(),
            edges: g.edges.clone(),
        }
    }

    pub fn fqn_of(&self, id: cgx_core::NodeId) -> &str {
        &self
            .nodes
            .iter()
            .find(|n| n.id == id)
            .expect("node id present")
            .fqn
    }

    /// All edges from `caller_fqn` to `callee_fqn`.
    pub fn edges_between<'a>(
        &'a self,
        caller_fqn: &'a str,
        callee_fqn: &'a str,
    ) -> impl Iterator<Item = &'a EdgeRecord> + 'a {
        self.edges
            .iter()
            .filter(move |e| self.fqn_of(e.src) == caller_fqn && self.fqn_of(e.dst) == callee_fqn)
    }

    pub fn has_node(&self, fqn: &str) -> bool {
        self.nodes.iter().any(|n| n.fqn == fqn)
    }

    pub fn has_edge(&self, caller_fqn: &str, callee_fqn: &str) -> bool {
        self.edges_between(caller_fqn, callee_fqn).next().is_some()
    }
}
