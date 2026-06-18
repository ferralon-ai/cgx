//! End-to-end integration tests for `cgx query` (the Layer-2 CQL command, P8/P9).
//!
//! Each test builds a throwaway git repo from a tiny, hand-auditable Rust source
//! tree (the same call graph the `cli.rs` suite uses), then drives `cgx query`
//! through the built binary via `assert_cmd`. Coverage: tabular output in
//! human/json/sarif, `--assert-empty` (exit 1), the ADR-08 vacuity guard (exit 4),
//! and the CQL parse-error caret path (exit 2). P9 adds `--at` and the
//! dot/mermaid/d2 path emitters here too.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

/// `main` (entrypoint) → `alpha` → `beta`; `orphan` → `beta`. `beta` lives in a
/// second module so a cross-file/cross-module CALLS edge exists.
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
    git_init_commit(&repo);
    (tmp, repo)
}

/// A repo whose only source is a comment-only file: the index has zero nodes, so a
/// query over it passes `--assert-empty` *vacuously* (ADR-08 clause a).
fn empty_graph_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "// only a comment, no symbols\n").unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"empty\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    git_init_commit(&repo);
    (tmp, repo)
}

fn git_init_commit(repo: &Path) {
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.name", "cgx-test"]);
    git(repo, &["config", "user.email", "cgx@test.invalid"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
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
            "fixture",
            "--date=2020-01-01T00:00:00Z",
        ],
    );
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

fn run_cgx(repo: &Path, args: &[&str]) -> (String, i32) {
    let out = Command::new(cargo_bin("cgx"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run cgx");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    (stdout, out.status.code().unwrap_or(-1))
}

fn run_cgx_full(repo: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(cargo_bin("cgx"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run cgx");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    (stdout, stderr, out.status.code().unwrap_or(-1))
}

fn index(repo: &Path) {
    let (_out, code) = run_cgx(repo, &["index"]);
    assert_eq!(code, 0, "index should succeed");
}

// --- P8: tabular output in every format -------------------------------------

#[test]
fn query_tabular_human_aligns_columns() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(
        &repo,
        &["query", "MATCH (a)-[:CALLS]->(b) RETURN a.name, b.name"],
    );
    assert_eq!(code, 0, "tabular query exits 0: {out}");
    // Header row carries the column names; a body row carries an edge.
    assert!(out.contains("a.name") && out.contains("b.name"), "header: {out}");
    assert!(
        out.contains("rust_sample::main") && out.contains("rust_sample::alpha"),
        "main -> alpha row present: {out}"
    );
    assert!(
        out.contains("rust_sample::helper::beta"),
        "beta appears as a callee: {out}"
    );
}

#[test]
fn query_tabular_json_has_columns_and_rows() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) RETURN b.name AS name, b.file AS file, b.line AS line",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 0, "json query exits 0: {out}");
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("query --format json emits valid JSON");
    assert_eq!(
        parsed["columns"],
        serde_json::json!(["name", "file", "line"]),
        "json carries ordered columns: {out}"
    );
    assert!(
        parsed["rows"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "json carries rows: {out}"
    );
    assert_eq!(parsed["vacuous"], serde_json::json!(false), "not vacuous: {out}");
}

#[test]
fn query_tabular_sarif_locates_rows_from_file_line_columns() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) RETURN b.name AS name, b.file AS file, b.line AS line",
            "--format",
            "sarif",
        ],
    );
    assert_eq!(code, 0, "sarif query exits 0: {out}");
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("query --format sarif emits valid JSON");
    let results = parsed["runs"][0]["results"]
        .as_array()
        .expect("sarif results array");
    assert!(!results.is_empty(), "at least one sarif result: {out}");
    // The physical location is taken from the row's file/line columns.
    let loc = &results[0]["locations"][0]["physicalLocation"];
    assert_eq!(
        loc["artifactLocation"]["uri"], "src/helper.rs",
        "physical location uri from the `file` column: {out}"
    );
    assert_eq!(
        loc["region"]["startLine"], 1,
        "physical location line from the `line` column: {out}"
    );
}

#[test]
fn query_sarif_logical_location_when_no_file_column() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    // A purely-scalar projection has no file/line column: the result still locates
    // logically (never a silent drop).
    let (out, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) RETURN b.name AS name",
            "--format",
            "sarif",
        ],
    );
    assert_eq!(code, 0, "sarif query exits 0: {out}");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid sarif json");
    let results = parsed["runs"][0]["results"].as_array().unwrap();
    assert!(!results.is_empty(), "results present: {out}");
    let logical = &results[0]["locations"][0]["logicalLocations"][0]["fullyQualifiedName"];
    assert!(
        logical.as_str().map(|s| s.contains("rust_sample")).unwrap_or(false),
        "logical location carries the fqn: {out}"
    );
}

#[test]
fn query_output_is_byte_identical_across_two_runs() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let q = "MATCH (a)-[:CALLS]->(b) RETURN a.name, b.name";
    let (a, _) = run_cgx(&repo, &["query", q, "--format", "json"]);
    let (b, _) = run_cgx(&repo, &["query", q, "--format", "json"]);
    assert_eq!(a, b, "two CQL query runs must be byte-identical");
}

// --- P8: @file query source --------------------------------------------------

#[test]
fn query_reads_at_file_source() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let qfile = repo.join("q.cql");
    std::fs::write(&qfile, "MATCH (a)-[:CALLS]->(b) RETURN a.name, b.name\n").unwrap();
    let (out, code) = run_cgx(&repo, &["query", &format!("@{}", qfile.display())]);
    assert_eq!(code, 0, "@file query exits 0: {out}");
    assert!(
        out.contains("rust_sample::main"),
        "@file query produces rows: {out}"
    );
}

// --- P8: assertions + vacuity ------------------------------------------------

#[test]
fn query_assert_empty_fires_when_results_exist_exit_1() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) RETURN b.name",
            "--assert-empty",
        ],
    );
    assert_eq!(code, 1, "results present under --assert-empty → exit 1");
}

#[test]
fn query_assert_empty_genuine_zero_over_populated_graph_exit_0() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    // A WHERE that matches nothing over a populated graph is a genuine (non-vacuous)
    // pass per the documented CQL vacuity rule.
    let (_out, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) WHERE b.name = \"no_such_symbol_zzz\" RETURN b.name",
            "--assert-empty",
        ],
    );
    assert_eq!(code, 0, "genuine zero over a populated graph → exit 0");
}

#[test]
fn query_assert_empty_vacuous_on_empty_graph_exit_4() {
    let (_tmp, repo) = empty_graph_repo();
    index(&repo);
    let (_out, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) RETURN b.name",
            "--assert-empty",
        ],
    );
    assert_eq!(code, 4, "empty graph under --assert-empty → vacuous exit 4");
}

#[test]
fn query_assert_empty_vacuous_suppressed_by_allow_vacuous_exit_0() {
    let (_tmp, repo) = empty_graph_repo();
    index(&repo);
    let (_out, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) RETURN b.name",
            "--assert-empty",
            "--allow-vacuous",
        ],
    );
    assert_eq!(code, 0, "--allow-vacuous downgrades the vacuous pass to 0");
}

// --- P8: error paths ---------------------------------------------------------

#[test]
fn query_parse_error_renders_caret_exit_2() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, stderr, code) = run_cgx_full(&repo, &["query", "MATCH (n -[:CALLS]-> RETURN"]);
    assert_eq!(code, 2, "CQL parse error → exit 2 (usage)");
    assert!(
        stderr.contains("error:") && stderr.contains("query:1:"),
        "caret view printed to stderr: {stderr}"
    );
    assert!(stderr.contains('^'), "caret marker present: {stderr}");
}

#[test]
fn query_plan_error_for_deferred_edge_type_exit_2() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, stderr, code) =
        run_cgx_full(&repo, &["query", "MATCH (a)-[:RESOLVES_TO]->(b) RETURN a"]);
    assert_eq!(code, 2, "deferred edge type → plan error → exit 2");
    assert!(
        stderr.contains("error:"),
        "plan error caret printed: {stderr}"
    );
}

#[test]
fn query_without_index_with_no_auto_index_is_graph_error_exit_3() {
    let (_tmp, repo) = fixture_repo();
    let (_out, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) RETURN a.name",
            "--no-auto-index",
        ],
    );
    assert_eq!(code, 3, "un-indexed repo with --no-auto-index → exit 3");
}
