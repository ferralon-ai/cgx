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

/// Two-commit fixture: the second commit adds `gamma`, which also calls `beta`.
/// Used by the `--at` tests to prove a prior ref yields a different graph.
fn two_commit_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, repo) = fixture_repo();
    let main_v2 = "\
mod helper;

fn main() {
    alpha();
    gamma();
}

fn alpha() {
    helper::beta();
}

fn orphan() {
    helper::beta();
}

fn gamma() {
    helper::beta();
}
";
    std::fs::write(repo.join("src/main.rs"), main_v2).unwrap();
    git(&repo, &["add", "src/main.rs"]);
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
            "add gamma",
            "--date=2020-01-02T00:00:00Z",
        ],
    );
    (tmp, repo)
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
        out.contains("fixture::main") && out.contains("fixture::alpha"),
        "main -> alpha row present: {out}"
    );
    assert!(
        out.contains("fixture::helper::beta"),
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
        logical.as_str().map(|s| s.contains("fixture")).unwrap_or(false),
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
        out.contains("fixture::main"),
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

// --- P9a: --at ref pinning (shared across subcommands) -----------------------

#[test]
fn query_at_head_equals_no_at() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let q = "MATCH (a)-[:CALLS]->(b) RETURN a.name, b.name";
    let (plain, c1) = run_cgx(&repo, &["query", q, "--format", "json"]);
    let (at_head, c2) = run_cgx(&repo, &["query", q, "--at", "HEAD", "--format", "json"]);
    assert_eq!(c1, 0);
    assert_eq!(c2, 0, "--at HEAD query exits 0: {at_head}");
    assert_eq!(plain, at_head, "--at HEAD must equal the no-`--at` result");
}

#[test]
fn query_at_prior_commit_shows_different_graph() {
    let (_tmp, repo) = two_commit_fixture();
    let q = "MATCH (a)-[:CALLS]->(b) RETURN a.name, b.name";
    let (head, ch) = run_cgx(&repo, &["query", q, "--at", "HEAD"]);
    let (prior, cp) = run_cgx(&repo, &["query", q, "--at", "HEAD~1"]);
    assert_eq!(ch, 0, "--at HEAD exits 0: {head}");
    assert_eq!(cp, 0, "--at HEAD~1 exits 0: {prior}");
    // HEAD has `gamma`; HEAD~1 does not.
    assert!(head.contains("fixture::gamma"), "HEAD has gamma: {head}");
    assert!(
        !prior.contains("fixture::gamma"),
        "HEAD~1 lacks gamma: {prior}"
    );
    assert_ne!(head, prior, "the two refs produce different graphs");
}

/// `--at` is shared via `QueryArgs`, so it retrofits onto Layer-1 subcommands too
/// (Q-17). `callees` over a prior commit must not see the newer `gamma`.
#[test]
fn at_ref_is_shared_with_layer1_subcommands() {
    let (_tmp, repo) = two_commit_fixture();
    let (prior, code) = run_cgx(&repo, &["callees", "main", "--at", "HEAD~1"]);
    assert_eq!(code, 0, "callees --at exits 0: {prior}");
    assert!(
        prior.contains("fixture::alpha"),
        "alpha is reachable at HEAD~1: {prior}"
    );
    assert!(
        !prior.contains("fixture::gamma"),
        "gamma did not exist at HEAD~1: {prior}"
    );
}

// --- P9b: dot/mermaid/d2 path-graph emitters (shared) ------------------------

/// The whole-path CQL query that yields a `main → beta` path (matches the Layer-1
/// `paths main beta` result over the same fixture).
const PATH_QUERY: &str = "MATCH p = (a)-[:CALLS*1..3]->(b) \
     WHERE a.name = \"fixture::main\" AND b.name = \"fixture::helper::beta\" RETURN p";

#[test]
fn paths_dot_emitter() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["paths", "main", "beta", "--format", "dot"]);
    assert_eq!(code, 0, "paths --format dot exits 0: {out}");
    assert!(out.starts_with("digraph cgx {"), "dot header: {out}");
    assert!(out.contains("->"), "dot has edges: {out}");
    assert!(
        out.contains("fixture::main") && out.contains("fixture::helper::beta"),
        "dot labels carry the fqns: {out}"
    );
}

#[test]
fn paths_mermaid_emitter() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["paths", "main", "beta", "--format", "mermaid"]);
    assert_eq!(code, 0, "paths --format mermaid exits 0: {out}");
    assert!(out.starts_with("graph TD"), "mermaid header: {out}");
    assert!(out.contains("-->"), "mermaid has edges: {out}");
}

/// A fixture whose two call edges carry non-`always` conditions, so the Mermaid
/// emitter takes its *labeled*-edge branch (`a -->|label| b`).
const CONDITIONAL_RS: &str = "\
pub mod sys {
    pub fn run() {}
    pub fn boom() {}
}
pub mod handler {
    pub fn process(flag: bool) {
        if flag {
            crate::sys::run();
        }
        for _ in 0..3 {
            crate::sys::boom();
        }
    }
}
";

fn conditional_edge_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), CONDITIONAL_RS).unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"cond\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    git_init_commit(&repo);
    (tmp, repo)
}

/// R06-03: before this test, `mermaid_edge_label`'s *call site* was reached by no
/// integration test at all — `cgx diff --path-added` leaves its edges unlabeled by
/// decision D3, so `cgx paths` over a non-`always` edge is the only route in the
/// product that emits `a -->|label| b`, and nothing exercised it.
///
/// What this pins is that the labeled-edge branch is live and its label survives
/// end to end. It cannot pin *which* escaper the call site uses: cgx only ever
/// emits `condition_str` words as edge labels, and both escapers are the identity
/// on those. That negative control is `render_mermaid_escapes_both_label_positions`
/// in `output.rs`, which drives the emitter directly with a hostile label.
#[test]
fn paths_mermaid_emits_a_labeled_edge() {
    let (_tmp, repo) = conditional_edge_repo();
    index(&repo);
    for (to, condition) in [("**sys::run", "conditional"), ("**sys::boom", "loop")] {
        let (out, code) = run_cgx(
            &repo,
            &["paths", "**handler::process", to, "--format", "mermaid"],
        );
        assert_eq!(code, 0, "paths --format mermaid exits 0: {out}");
        let edge = out
            .lines()
            .find(|l| l.contains("-->|"))
            .unwrap_or_else(|| panic!("a labeled mermaid edge was emitted: {out}"));
        assert_eq!(
            edge.trim(),
            format!("n0 -->|{condition}| n1"),
            "the edge carries its condition as a label: {out}"
        );
    }
}

#[test]
fn paths_d2_emitter() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["paths", "main", "beta", "--format", "d2"]);
    assert_eq!(code, 0, "paths --format d2 exits 0: {out}");
    assert!(out.contains(" -> "), "d2 has node->node statements: {out}");
    assert!(
        out.contains("fixture::main"),
        "d2 carries the fqns: {out}"
    );
}

/// The shared-emitter contract: a CQL `RETURN path` query renders byte-identical
/// graph source to the Layer-1 `paths` command over the same graph.
#[test]
fn return_path_query_dot_matches_layer1_paths() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (layer1, c1) = run_cgx(&repo, &["paths", "main", "beta", "--format", "dot"]);
    let (cql, c2) = run_cgx(&repo, &["query", PATH_QUERY, "--format", "dot"]);
    assert_eq!(c1, 0);
    assert_eq!(c2, 0, "RETURN path --format dot exits 0: {cql}");
    assert_eq!(
        layer1, cql,
        "CQL RETURN path and Layer-1 paths emit identical dot"
    );
}

#[test]
fn return_path_query_mermaid_and_d2() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (m, cm) = run_cgx(&repo, &["query", PATH_QUERY, "--format", "mermaid"]);
    assert_eq!(cm, 0, "RETURN path --format mermaid exits 0: {m}");
    assert!(m.starts_with("graph TD"), "mermaid header: {m}");
    let (d, cd) = run_cgx(&repo, &["query", PATH_QUERY, "--format", "d2"]);
    assert_eq!(cd, 0, "RETURN path --format d2 exits 0: {d}");
    assert!(d.contains(" -> "), "d2 statements: {d}");
}

#[test]
fn dot_on_tabular_query_is_usage_error_exit_2() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, stderr, code) = run_cgx_full(
        &repo,
        &[
            "query",
            "MATCH (a)-[:CALLS]->(b) RETURN a.name",
            "--format",
            "dot",
        ],
    );
    assert_eq!(code, 2, "dot on a tabular query → usage error exit 2");
    assert!(
        stderr.contains("path-returning"),
        "error explains the path-only constraint: {stderr}"
    );
}

#[test]
fn mermaid_on_callers_is_usage_error_exit_2() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    // A non-path Layer-1 subcommand also rejects the path-graph emitters.
    let (_out, _stderr, code) = run_cgx_full(&repo, &["callers", "beta", "--format", "mermaid"]);
    assert_eq!(code, 2, "mermaid on `callers` → usage error exit 2");
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

// --- N2: --sql deferral message + exit 2 -------------------------------------

/// `--sql` is a recognized flag that returns a clear deferral error (exit 2)
/// naming the not-yet-implemented interface, never clap's generic unknown-flag
/// message.
#[test]
fn sql_flag_defers_with_clear_message_exit_2() {
    let (_tmp, repo) = fixture_repo();
    // No index needed: the --sql check fires before any graph I/O.
    let (_out, stderr, code) = run_cgx_full(
        &repo,
        &["query", "MATCH (a)-[:CALLS]->(b) RETURN a.name", "--sql"],
    );
    assert_eq!(code, 2, "--sql → deferral error → exit 2; stderr={stderr}");
    assert!(
        stderr.contains("--sql") || stderr.contains("recursive-CTE"),
        "--sql deferral message names the feature: {stderr}"
    );
    assert!(
        stderr.contains("not implemented"),
        "--sql deferral message says not-implemented: {stderr}"
    );
}
