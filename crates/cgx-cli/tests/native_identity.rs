//! Language-native symbol identity and queries, end to end through the built
//! `cgx` binary.
//!
//! The stored fqn is an abstract segment sequence whose string encoding uses an
//! opaque internal delimiter; every surface a user sees must render it in the
//! owning language's native syntax, and a query may name a symbol in that native
//! syntax. These tests pin the observable contract:
//!   - a native-syntax anchor and an internal-delimiter anchor resolve the *same*
//!     node (query-side back-compat);
//!   - result fqns are rendered per-node in each node's own language, with no
//!     internal delimiter surviving and no `display_fqn` sidecar;
//!   - an anchor matched only across a dropped language gate is flagged
//!     (`agnostic_cross_language`), and a native in-language anchor is not;
//!   - an anchor that matches more than one node is a hard ambiguity error whose
//!     candidate list is itself natively rendered;
//!   - Rust output is byte-for-byte what it always was (produced *by* rendering).

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

// --- harness -----------------------------------------------------------------

fn go_fixture_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
        .join("go")
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
        .current_dir(repo)
        .args(args)
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn commit_repo(repo: &Path) {
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.name", "cgx-test"]);
    git(repo, &["config", "user.email", "cgx@test.invalid"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "init"]);
}

fn go_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    copy_dir(&go_fixture_src(), &repo);
    commit_repo(&repo);
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

fn index(repo: &Path) {
    let (_o, e, code) = run_cgx(repo, &["index"]);
    assert_eq!(code, 0, "index should succeed: {e}");
}

/// Parse a `{results: [...], ...}` envelope and return the top-level JSON value.
fn query_json(repo: &Path, args: &[&str]) -> serde_json::Value {
    let (out, e, code) = run_cgx(repo, args);
    assert_eq!(code, 0, "{args:?} exits 0: {out}{e}");
    serde_json::from_str(&out).expect("query json")
}

fn result_fqns(v: &serde_json::Value) -> Vec<String> {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["fqn"].as_str().unwrap().to_string())
        .collect()
}

// --- tests -------------------------------------------------------------------

/// A native-syntax anchor (`go_sample/worker`) and the internal-delimiter anchor
/// (`go_sample::worker`) resolve the *same* node: the two caller sets are equal.
#[test]
fn native_and_internal_anchor_resolve_same_node() {
    let (_tmp, repo) = go_repo();
    index(&repo);
    let native = result_fqns(&query_json(
        &repo,
        &["callers", "go_sample/worker", "--format", "json"],
    ));
    let internal = result_fqns(&query_json(
        &repo,
        &["callers", "go_sample::worker", "--format", "json"],
    ));
    assert_eq!(
        native, internal,
        "native and internal-delimiter anchors must resolve the same callers"
    );
    assert!(
        native.iter().any(|f| f == "go_sample/Spawn"),
        "the Spawn caller is present (and natively rendered): {native:?}"
    );
}

/// Every result fqn is rendered in the node's native syntax: for Go that means a
/// `/` separator and no surviving internal delimiter, and there is no
/// `display_fqn` sidecar field on any row.
#[test]
fn json_fqns_are_native_with_no_sidecar() {
    let (_tmp, repo) = go_repo();
    index(&repo);
    let v = query_json(&repo, &["callers", "go_sample::worker", "--format", "json"]);
    for row in v["results"].as_array().unwrap() {
        let fqn = row["fqn"].as_str().unwrap();
        assert!(!fqn.contains("::"), "no internal delimiter in output fqn: {fqn}");
        assert!(
            row.get("display_fqn").is_none(),
            "no display_fqn sidecar on a row: {row}"
        );
    }
    assert!(
        result_fqns(&v).iter().any(|f| f == "go_sample/Spawn"),
        "Spawn rendered with Go `/` separator"
    );
}

/// The cross-language honesty signal: an anchor matched only because the family
/// gate was dropped (a `::`-surface query hitting a Go node) is flagged on the
/// JSON envelope; a native in-language anchor is not.
#[test]
fn cross_language_anchor_is_flagged_native_is_not() {
    let (_tmp, repo) = go_repo();
    index(&repo);
    let crossed = query_json(&repo, &["callers", "go_sample::worker", "--format", "json"]);
    assert_eq!(
        crossed["agnostic_cross_language"],
        serde_json::json!(true),
        "a `::`-surface query against a Go node is a cross-language match: {crossed}"
    );
    let native = query_json(&repo, &["callers", "go_sample/worker", "--format", "json"]);
    assert_eq!(
        native["agnostic_cross_language"],
        serde_json::json!(false),
        "a Go-native anchor is an in-language match: {native}"
    );
}

/// An anchor that matches more than one node is a hard usage error (exit 2) that
/// lists the candidates — and each candidate line is itself natively rendered.
#[test]
fn ambiguous_anchor_errors_with_native_candidates() {
    // Two classes, each with a `validate` method. A globstar selector matches both.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("pkg")).unwrap();
    std::fs::write(repo.join("pkg/__init__.py"), "").unwrap();
    std::fs::write(
        repo.join("pkg/a.py"),
        "class A:\n    def validate(self):\n        return 1\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("pkg/b.py"),
        "class B:\n    def validate(self):\n        return 2\n",
    )
    .unwrap();
    commit_repo(&repo);
    index(&repo);

    let (out, stderr, code) = run_cgx(&repo, &["callers", "**::validate", "--format", "json"]);
    assert_eq!(code, 2, "an ambiguous anchor is a usage error: {out}{stderr}");
    assert!(
        stderr.contains("ambiguous"),
        "the error names the ambiguity: {stderr}"
    );
    // Candidates are natively rendered (Python `.`), never the internal delimiter.
    assert!(
        stderr.contains("pkg.a.A.validate") && stderr.contains("pkg.b.B.validate"),
        "both candidates are listed in native syntax: {stderr}"
    );
    assert!(
        !stderr.contains("pkg::a::A::validate"),
        "no internal delimiter leaks into the candidate list: {stderr}"
    );
}

/// A single result set spanning two languages renders each node in its *own*
/// language — Go with `/`, Python with `.` — in one `search` table.
#[test]
fn mixed_language_results_render_per_node() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/thing.go"),
        "package app\n\nfunc GoThing() int { return 1 }\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/thing.py"),
        "def py_thing():\n    return 1\n",
    )
    .unwrap();
    commit_repo(&repo);
    index(&repo);

    // `**` is a selector (globstar), so this scans every symbol across languages.
    let v = query_json(&repo, &["search", "**", "--format", "json"]);
    let fqns = result_fqns(&v);
    let go_hit = fqns.iter().find(|f| f.contains("GoThing")).expect("Go symbol present");
    let py_hit = fqns.iter().find(|f| f.contains("py_thing")).expect("Python symbol present");
    assert!(
        go_hit.contains('/') && !go_hit.contains("::"),
        "the Go node renders with `/`: {go_hit}"
    );
    assert!(
        py_hit.contains('.') && !py_hit.contains("::"),
        "the Python node renders with `.`: {py_hit}"
    );
}

/// Rust output is unchanged — the native separator happens to coincide with the
/// internal delimiter, but the string is produced *by* rendering, not passed
/// through. A Rust anchor is an in-language (not cross-language) match.
#[test]
fn rust_output_is_unchanged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/main.rs"),
        "mod helper;\n\nfn main() {\n    alpha();\n}\n\nfn alpha() {\n    helper::beta();\n}\n",
    )
    .unwrap();
    std::fs::write(repo.join("src/helper.rs"), "pub fn beta() {\n    let _ = 1 + 1;\n}\n").unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    commit_repo(&repo);
    index(&repo);

    // The Rust fqn root carries the crate name; callers render with `::`.
    let v = query_json(&repo, &["callers", "fixture::helper::beta", "--format", "json"]);
    assert_eq!(
        v["agnostic_cross_language"],
        serde_json::json!(false),
        "a Rust `::` anchor against a Rust node is in-language: {v}"
    );
    assert!(
        result_fqns(&v).iter().any(|f| f == "fixture::alpha"),
        "the Rust caller keeps its `::` rendering: {:?}",
        result_fqns(&v)
    );
}
