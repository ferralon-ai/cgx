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
    assert!(out.contains("fixture::alpha"), "alpha reached: {out}");
    assert!(
        out.contains("fixture::helper::beta"),
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
        out.contains("fixture::alpha"),
        "alpha is a caller: {out}"
    );
    assert!(
        out.contains("fixture::orphan"),
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
fn out_of_range_node_id_does_not_panic() {
    // Boundary hardening: a query for a symbol that does not exist must take the
    // graceful IF-4 "not found" path, never a panic/abort. In this CLI's contract
    // an unresolved *symbol* is a usage error (exit 2); an unindexed *graph* is the
    // exit-3 path. The load-bearing assertion is that neither crashes: a Rust panic
    // would surface as exit 101 (or 134 on abort), which we explicitly reject.
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(&repo, &["callers", "no_such_symbol_zzz"]);
    assert_ne!(code, 101, "must not panic on an unknown symbol");
    assert_ne!(code, 134, "must not abort on an unknown symbol");
    assert_eq!(code, 2, "unknown symbol takes the IF-4 not-found path (exit 2)");
}

#[test]
fn query_without_index_with_no_auto_index_is_graph_error_exit_3() {
    let (_tmp, repo) = fixture_repo();
    // No `cgx index` run, and auto-index explicitly disabled.
    let (_out, code) = run_cgx(&repo, &["callers", "beta", "--no-auto-index"]);
    assert_eq!(
        code, 3,
        "querying an un-indexed repo with --no-auto-index → exit 3"
    );
}

// --- auto-index (audit row #23) integration tests ---

#[test]
fn query_auto_indexes_when_no_store_exists() {
    let (_tmp, repo) = fixture_repo();
    // No `cgx index` run: the query must build the index itself, then answer.
    assert!(
        !repo.join(".cgx/HEAD.json").exists(),
        "precondition: no index yet"
    );
    let (out, code) = run_cgx(&repo, &["callees", "main"]);
    assert_eq!(code, 0, "auto-indexed query should succeed: {out}");
    assert!(
        out.contains("fixture::alpha"),
        "auto-indexed query answers correctly: {out}"
    );
    assert!(
        repo.join(".cgx/HEAD.json").exists() && repo.join(".cgx/index.db").exists(),
        "auto-index wrote .cgx/ like `cgx index` does"
    );
}

/// The idempotence/determinism contract: a query auto-indexes on the first run;
/// a subsequent explicit `cgx index` over the unchanged tree hits the Layer-1
/// cache and extracts ZERO blobs (IX-1), proving the auto-index populated the same
/// cache and is not redundantly re-parsing.
#[test]
fn second_run_hits_cache_zero_extracted() {
    let (_tmp, repo) = fixture_repo();
    // First query auto-indexes (cold: blobs are extracted).
    let (_out, code) = run_cgx(&repo, &["callees", "main"]);
    assert_eq!(code, 0, "first auto-indexed query succeeds");
    let pointer_after_auto = std::fs::read_to_string(repo.join(".cgx/HEAD.json")).unwrap();

    // An explicit index over the now-warm cache must report `0 extracted`.
    let (idx_out, idx_code) = run_cgx(&repo, &["index"]);
    assert_eq!(idx_code, 0);
    assert!(
        idx_out.contains("0 extracted"),
        "second pass over unchanged tree extracts 0 blobs (cache hit): {idx_out}"
    );

    // And the pointer the auto-index wrote is byte-identical to the explicit one
    // (deterministic Layer-2 key).
    let pointer_after_explicit = std::fs::read_to_string(repo.join(".cgx/HEAD.json")).unwrap();
    assert_eq!(
        pointer_after_auto, pointer_after_explicit,
        "auto-index and explicit index produce an identical pointer"
    );

    // A second query over the warm store is still correct and byte-stable.
    let (a, _) = run_cgx(&repo, &["callees", "main", "--format", "json"]);
    let (b, _) = run_cgx(&repo, &["callees", "main", "--format", "json"]);
    assert_eq!(a, b, "repeat query over the cached index is byte-identical");
}

// --- cgx explain (Q-6 / IF-15) integration tests ---

#[test]
fn explain_happy_path_reports_provenance() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["explain", "alpha"]);
    assert_eq!(code, 0, "explain of a known symbol exits 0: {out}");
    // alpha is called by main and calls beta.
    assert!(out.contains("fixture::alpha"), "names the symbol: {out}");
    assert!(
        out.contains("callers: 1, callees: 1"),
        "reports caller/callee counts: {out}"
    );
    assert!(
        out.contains("fixture::main") && out.contains("fixture::helper::beta"),
        "lists both incident edges: {out}"
    );
    // P7/IF-6: each incident edge now renders its resolution tier and rule.
    assert!(
        out.contains("tier=") && out.contains("rule="),
        "human format surfaces tier/rule provenance: {out}"
    );
}

#[test]
fn explain_json_format_is_valid_json() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["explain", "alpha", "--format", "json"]);
    assert_eq!(code, 0, "explain --format json exits 0: {out}");
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("explain --format json emits valid JSON");
    assert_eq!(parsed["callers_count"], 1, "json carries callers_count: {out}");
    assert_eq!(parsed["callees_count"], 1, "json carries callees_count: {out}");
    assert!(
        parsed["edges"].as_array().map(|a| a.len()) == Some(2),
        "json lists both edges: {out}"
    );
    // P7/IF-6: every edge carries the new provenance fields (tier/rule always
    // present; resolution_source/site are nullable).
    for edge in parsed["edges"].as_array().expect("edges array") {
        assert!(edge["tier"].is_string(), "edge carries a tier string: {out}");
        assert!(edge["rule"].is_string(), "edge carries a rule string: {out}");
        assert!(
            edge.as_object().unwrap().contains_key("resolution_source"),
            "edge carries a resolution_source key: {out}"
        );
        assert!(
            edge.as_object().unwrap().contains_key("site"),
            "edge carries a site key: {out}"
        );
    }
}

#[test]
fn confidence_certain_floor_returns_only_certain_edges() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    // `main` transitively reaches `alpha` via a same-file lexical call (certain)
    // and `helper::beta` via a cross-module call (below certain).
    let (all, code) = run_cgx(&repo, &["callees", "main", "--format", "json"]);
    assert_eq!(code, 0, "unfiltered callees succeed: {all}");
    let all: serde_json::Value = serde_json::from_str(&all).expect("json");
    let all_names: Vec<&str> = all["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["fqn"].as_str().unwrap())
        .collect();
    assert!(all_names.iter().any(|n| n.ends_with("alpha")));
    assert!(all_names.iter().any(|n| n.ends_with("beta")));

    // `--confidence certain` keeps only the certain edge.
    let (certain, code) = run_cgx(
        &repo,
        &["callees", "main", "--confidence", "certain", "--format", "json"],
    );
    assert_eq!(code, 0, "filtered callees succeed: {certain}");
    let certain: serde_json::Value = serde_json::from_str(&certain).expect("json");
    let results = certain["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .all(|r| r["confidence"] == serde_json::json!("certain")),
        "every surviving edge is certain: {certain}"
    );
    assert!(
        !results
            .iter()
            .any(|r| r["fqn"].as_str().unwrap().ends_with("beta")),
        "the weaker cross-module edge is filtered out: {certain}"
    );
}

#[test]
fn explain_unknown_symbol_is_usage_error_exit_2() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(&repo, &["explain", "does_not_exist_zzz"]);
    assert_ne!(code, 101, "must not panic on an unknown symbol");
    assert_eq!(code, 2, "unknown symbol → IF-4 not-found path (exit 2)");
}

#[test]
fn explain_auto_indexes_when_no_store_exists() {
    let (_tmp, repo) = fixture_repo();
    // No `cgx index` run: explain must auto-index then answer.
    let (out, code) = run_cgx(&repo, &["explain", "main"]);
    assert_eq!(code, 0, "explain auto-indexes and exits 0: {out}");
    assert!(
        repo.join(".cgx/HEAD.json").exists(),
        "explain auto-index wrote the index pointer"
    );
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
        out.contains("fixture::orphan"),
        "orphan is unused: {out}"
    );
}

// --- cgx doctor integration tests ---

#[test]
fn doctor_reports_on_indexed_repo() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["doctor"]);
    assert_eq!(code, 0, "doctor should exit 0: {out}");
    // The human report must contain some basic quality fields.
    assert!(
        out.contains("nodes:") || out.contains("Nodes:") || out.contains("node"),
        "doctor output mentions nodes: {out}"
    );
    assert!(
        out.contains("trust") || out.contains("Trust"),
        "doctor output mentions trust: {out}"
    );
}

#[test]
fn doctor_json_format_is_valid_json() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["doctor", "--format", "json"]);
    assert_eq!(code, 0, "doctor --format json should exit 0: {out}");
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("doctor --format json must emit valid JSON");
    // The JSON report should carry node_count and trust fields.
    assert!(
        parsed.get("node_count").is_some(),
        "JSON report has node_count: {out}"
    );
    assert!(
        parsed.get("trust").is_some(),
        "JSON report has trust: {out}"
    );
}

#[test]
fn doctor_without_index_is_graph_error_exit_3() {
    let (_tmp, repo) = fixture_repo();
    // No `cgx index` run.
    let (_out, code) = run_cgx(&repo, &["doctor"]);
    assert_eq!(code, 3, "doctor on un-indexed repo → exit 3");
}

// --- cgx diff integration tests ---

/// Create a two-commit fixture repo where the second commit adds a new function
/// that calls `beta`, so the diff has at least one added edge.
fn two_commit_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, repo) = fixture_repo(); // first commit: main→alpha→beta + orphan

    // Second commit: add a new caller `gamma` that also calls `beta`.
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

#[test]
fn diff_two_commits_shows_added_edges() {
    let (_tmp, repo) = two_commit_fixture();
    let (out, code) = run_cgx(&repo, &["diff", "HEAD~1", "HEAD"]);
    assert_eq!(code, 0, "diff should exit 0: {out}");
    // The diff must report at least one added edge or node for `gamma`.
    assert!(
        out.contains("+ edge") || out.contains("+ node"),
        "diff HEAD~1 HEAD should show additions: {out}"
    );
}

#[test]
fn diff_same_commit_shows_no_diff() {
    let (_tmp, repo) = fixture_repo();
    let (out, code) = run_cgx(&repo, &["diff", "HEAD", "HEAD"]);
    assert_eq!(code, 0, "diff HEAD HEAD should exit 0: {out}");
    assert!(
        out.contains("(no diff)"),
        "diff of same commit should be empty: {out}"
    );
}

#[test]
fn diff_newer_than_shows_only_added_edges() {
    let (_tmp, repo) = two_commit_fixture();
    let (out, code) = run_cgx(&repo, &["diff", "HEAD~1", "HEAD", "--newer-than"]);
    assert_eq!(code, 0, "--newer-than should exit 0: {out}");
    // Must not report removed or changed edges.
    assert!(
        !out.contains("- edge"),
        "newer-than must not show removed edges: {out}"
    );
    assert!(
        !out.contains("~ edge"),
        "newer-than must not show changed edges: {out}"
    );
}

#[test]
fn diff_json_format_is_valid_json() {
    let (_tmp, repo) = two_commit_fixture();
    let (out, code) = run_cgx(&repo, &["diff", "HEAD~1", "HEAD", "--format", "json"]);
    assert_eq!(code, 0, "diff --format json should exit 0: {out}");
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("diff --format json must emit valid JSON");
    assert!(
        parsed.get("added_edges").is_some(),
        "JSON diff has added_edges: {out}"
    );
}

// --- cgx mcp integration tests ---

#[test]
fn mcp_initialize_responds_and_exits_on_eof() {
    // Pipe a single initialize request and expect a valid JSON-RPC response.
    let initialize_msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#;
    let out = Command::new(cargo_bin("cgx"))
        .args(["mcp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.take() {
                let mut s = stdin;
                let _ = writeln!(s, "{initialize_msg}");
                // Drop stdin to signal EOF.
                drop(s);
            }
            child.wait_with_output()
        })
        .expect("run cgx mcp");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    // The server must respond with a valid JSON-RPC result.
    assert!(
        stdout.contains(r#""id":1"#) || stdout.contains(r#""id": 1"#),
        "mcp initialize response has id=1: {stdout}"
    );
    assert!(
        stdout.contains("protocolVersion"),
        "mcp initialize response has protocolVersion: {stdout}"
    );
}

#[test]
fn mcp_tools_list_returns_tools() {
    let list_msg = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let out = Command::new(cargo_bin("cgx"))
        .args(["mcp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.take() {
                let mut s = stdin;
                let _ = writeln!(s, "{list_msg}");
                drop(s);
            }
            child.wait_with_output()
        })
        .expect("run cgx mcp tools/list");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("callers") || stdout.contains("tools"),
        "mcp tools/list response mentions tools: {stdout}"
    );
}

// --- `cgx search` (symbol search, Since: v0.2) -------------------------------

/// A fixture with a known mix of symbol kinds and FQNs, so `search` assertions
/// (substring, `--kind` narrowing, `--limit` truncation, sort order) are exact.
/// `helper::Widget` is a `type`; the rest are `function`s. FQNs share the
/// `search_target` substring so a single pattern hits a controlled set.
const SEARCH_MAIN_RS: &str = "\
mod helper;

fn main() {
    search_target_alpha();
}

fn search_target_alpha() {
    helper::search_target_beta();
}

fn search_target_gamma() {
    let _ = 1;
}
";

const SEARCH_HELPER_RS: &str = "\
pub struct SearchWidget {
    pub n: i32,
}

pub fn search_target_beta() {
    let _ = 1 + 1;
}
";

fn search_fixture_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), SEARCH_MAIN_RS).unwrap();
    std::fs::write(repo.join("src/helper.rs"), SEARCH_HELPER_RS).unwrap();
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

#[test]
fn search_substring_returns_matching_fqns_exit_0() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["search", "search_target"]);
    assert_eq!(code, 0, "a search that matches still exits 0: {out}");
    assert!(out.contains("fixture::search_target_alpha"), "alpha: {out}");
    assert!(
        out.contains("fixture::helper::search_target_beta"),
        "beta: {out}"
    );
    assert!(out.contains("fixture::search_target_gamma"), "gamma: {out}");
}

#[test]
fn search_is_case_insensitive_substring_anywhere() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    // Upper-case query, mid-FQN substring: still matches the type SearchWidget.
    let (out, code) = run_cgx(&repo, &["search", "WIDGET"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("fixture::helper::SearchWidget"),
        "case-insensitive substring match anywhere in the FQN: {out}"
    );
}

#[test]
fn search_empty_result_exits_0_not_error() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["search", "zzz_no_such_symbol"]);
    assert_eq!(code, 0, "an empty search is NOT an error (unlike callers): {out}");
    assert!(out.contains("(no results)"), "empty marker: {out}");
}

#[test]
fn search_bad_regex_exits_2() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(&repo, &["search", "(unclosed", "--regex"]);
    assert_eq!(code, 2, "an invalid regex is a usage error");
}

#[test]
fn search_no_pattern_exits_2() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    // A bare `cgx search` (no pattern) is a clap usage error, not "list all".
    let (_out, code) = run_cgx(&repo, &["search"]);
    assert_eq!(code, 2, "missing required pattern → usage exit 2");
}

#[test]
fn search_kind_function_narrows() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    // `search` matches both the type SearchWidget and the search_target_* fns;
    // `--kind function` drops the type.
    let (out, code) = run_cgx(&repo, &["search", "search", "--kind", "function"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("fixture::search_target_alpha"),
        "function kept: {out}"
    );
    assert!(
        !out.contains("SearchWidget"),
        "type excluded by --kind function: {out}"
    );
}

#[test]
fn search_kind_type_excludes_functions() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["search", "Search", "--kind", "type"]);
    assert_eq!(code, 0);
    assert!(out.contains("fixture::helper::SearchWidget"), "type kept: {out}");
    assert!(
        !out.contains("search_target_alpha"),
        "functions excluded by --kind type: {out}"
    );
}

#[test]
fn search_json_shape_is_array_of_objects() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["search", "search_target", "--format", "json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid json");
    let arr = v.as_array().expect("search json is a bare array");
    assert!(!arr.is_empty(), "array has hits: {out}");
    let first = &arr[0];
    for key in ["fqn", "file", "line", "kind"] {
        assert!(first.get(key).is_some(), "object has `{key}`: {out}");
    }
    // FQNs are sorted ascending.
    let fqns: Vec<&str> = arr
        .iter()
        .map(|o| o.get("fqn").unwrap().as_str().unwrap())
        .collect();
    let mut sorted = fqns.clone();
    sorted.sort();
    assert_eq!(fqns, sorted, "results sorted by FQN: {out}");
}

#[test]
fn search_limit_truncates_with_footer() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    // Three `search_target_*` functions exist; --limit 2 shows two + a footer.
    let (out, code) = run_cgx(&repo, &["search", "search_target", "--kind", "function", "--limit", "2"]);
    assert_eq!(code, 0);
    let body_lines = out.lines().filter(|l| !l.starts_with('…')).count();
    assert_eq!(body_lines, 2, "exactly --limit rows shown: {out}");
    assert!(
        out.contains("(1 more — raise --limit)"),
        "footer reports the hidden count: {out}"
    );
}

#[test]
fn search_limit_zero_is_unlimited() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["search", "search_target", "--kind", "function", "--limit", "0"]);
    assert_eq!(code, 0);
    assert!(!out.contains("more — raise"), "no footer when unlimited: {out}");
    assert_eq!(
        out.lines().count(),
        3,
        "all three functions shown with --limit 0: {out}"
    );
}

#[test]
fn search_empty_pattern_exits_2() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    // An empty *provided* pattern would match everything — the "dump the table"
    // path the spec keeps out of scope. It is a usage error, not a full dump.
    let (_out, code) = run_cgx(&repo, &["search", ""]);
    assert_eq!(code, 2, "empty pattern → usage exit 2");
}

#[test]
fn search_unsupported_format_exits_2() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    // `search`'s surface is human|json only; sarif/dot/mermaid/d2 are usage errors,
    // not silently rendered as human.
    for fmt in ["sarif", "dot", "mermaid", "d2"] {
        let (_out, code) = run_cgx(&repo, &["search", "search_target", "--format", fmt]);
        assert_eq!(code, 2, "--format {fmt} unsupported by search → exit 2");
    }
}

#[test]
fn search_output_is_byte_identical_across_runs() {
    let (_tmp, repo) = search_fixture_repo();
    index(&repo);
    let (a, _) = run_cgx(&repo, &["search", "search_target"]);
    let (b, _) = run_cgx(&repo, &["search", "search_target"]);
    assert_eq!(a, b, "two search runs must be byte-identical");
    let (ja, _) = run_cgx(&repo, &["search", "search_target", "--format", "json"]);
    let (jb, _) = run_cgx(&repo, &["search", "search_target", "--format", "json"]);
    assert_eq!(ja, jb, "two json search runs must be byte-identical");
}
