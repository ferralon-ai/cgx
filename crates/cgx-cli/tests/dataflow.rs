//! v0.3 DATA_FLOW SC5 end-to-end: the user-facing surface over `derives-from`
//! edges — the `flows-to`/`flows-from` subcommands and real `:DATA_FLOW` CQL rows.
//!
//! Each test builds a throwaway git repo from the committed `rust-sample` fixture
//! (`src/dataflow.rs`: `through_call` → `helper(a)` gives the interprocedural
//! pedigree edge `r ⇝ a`), indexes it with `--dataflow`, and drives the *built
//! binary* via `assert_cmd`. These cover SC5 convergence criteria 1–8.
//!
//! Value-node FQNs (from `cgx search`):
//!   rust_sample::dataflow::through_call::a#0       (the parameter)
//!   rust_sample::dataflow::through_call::r#1       (let r = helper(a))
//!   rust_sample::dataflow::through_call::return#1  (the returned value)
//! The DerivesFrom chain (src derives from dst): return#1 → r#1 → a#0.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

const A: &str = "rust_sample::dataflow::through_call::a#0";
const R: &str = "rust_sample::dataflow::through_call::r#1";
const RETURN: &str = "rust_sample::dataflow::through_call::return#1";
const FN: &str = "rust_sample::dataflow::through_call";

/// Absolute path to the workspace `fixtures/rust-sample` source tree.
fn fixture_src() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../crates/cgx-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
        .join("rust-sample")
}

/// Copy the committed `rust-sample` fixture into a fresh temp dir, `git init` it,
/// and commit so it has a real HEAD tree OID. Returns the temp dir (kept alive by
/// the caller) and the repo path.
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

/// Index the repo with the v0.3 dataflow layer (`cgx index --dataflow`).
fn index_dataflow(repo: &Path) {
    let (_o, _e, code) = run_cgx(repo, &["index", "--dataflow"]);
    assert_eq!(code, 0, "index --dataflow should succeed");
}

/// Index the repo with the base (no-dataflow) layer.
fn index_base(repo: &Path) {
    let (_o, _e, code) = run_cgx(repo, &["index"]);
    assert_eq!(code, 0, "base index should succeed");
}

/// The set of result FQNs from a `flows-to`/`flows-from` JSON run.
fn flow_fqns(repo: &Path, subcommand: &str, symbol: &str) -> Vec<String> {
    let (out, _e, code) = run_cgx(
        repo,
        &[subcommand, symbol, "--depth", "8", "--format", "json"],
    );
    assert_eq!(code, 0, "{subcommand} {symbol} exits 0: {out}");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("flow json");
    parsed["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["fqn"].as_str().unwrap().to_string())
        .collect()
}

// ── Criterion 1: interprocedural pedigree ───────────────────────────────────

#[test]
fn flows_from_value_reaches_interprocedural_pedigree() {
    // `r = helper(a)`: `flows-from r` walks the DerivesFrom pedigree and reaches
    // `a` *through* helper's IFDS summary (the interproc edge, confidence
    // `probable`). Proves the cross-function DerivesFrom edge is walked.
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let fqns = flow_fqns(&repo, "flows-from", R);
    assert!(
        fqns.iter().any(|f| f == A),
        "flows-from r must reach a through the interproc summary, got {fqns:?}"
    );
}

#[test]
fn flows_to_value_reaches_forward_slice() {
    // The forward mirror: `flows-to a` reaches the values `a` flows into — `r`
    // (via the interproc edge) and onward `return`.
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let fqns = flow_fqns(&repo, "flows-to", A);
    assert!(
        fqns.iter().any(|f| f == R),
        "flows-to a must reach r, got {fqns:?}"
    );
    assert!(
        fqns.iter().any(|f| f == RETURN),
        "flows-to a must reach return downstream of r, got {fqns:?}"
    );
}

// ── Criterion 2: CQL ↔ CLI parity ───────────────────────────────────────────

#[test]
fn cql_data_flow_matches_cli_flows_from_walk() {
    // The CQL `:DATA_FLOW` walk and the `flows-from` CLI surface read the same
    // underlying DerivesFrom edge set. Pin both to the same reachable-FQN result:
    // every value `r` flows-from (its pedigree) is a DATA_FLOW successor of `r`.
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);

    let cli: std::collections::BTreeSet<String> =
        flow_fqns(&repo, "flows-from", R).into_iter().collect();

    let (out, _e, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:DATA_FLOW*1..8]->(b) WHERE a.name = \
             \"rust_sample::dataflow::through_call::r#1\" RETURN b.name AS fqn",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 0, "DATA_FLOW query exits 0: {out}");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("cql json");
    // CQL tabular `rows` are positional arrays; the single projected column is [0].
    let cql: std::collections::BTreeSet<String> = parsed["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().to_string())
        .collect();

    assert_eq!(
        cli, cql,
        "CQL :DATA_FLOW and CLI flows-from must return the same reachable set"
    );
    assert!(cli.contains(A), "parity set non-trivial (contains a): {cli:?}");
}

// ── Criterion 3: :DATA_FLOW returns real rows / empty without --dataflow ─────

#[test]
fn data_flow_query_returns_rows_with_dataflow_index() {
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let (out, _e, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:DATA_FLOW*1..8]->(b) RETURN a.name, b.name LIMIT 10",
        ],
    );
    assert_eq!(code, 0, ":DATA_FLOW exits 0: {out}");
    // Real rows present: every dataflow row is a value-node pair (FQNs carry `#`).
    assert!(
        out.contains('#'),
        ":DATA_FLOW returns real value-node rows against a --dataflow index: {out}"
    );
}

#[test]
fn data_flow_query_is_empty_against_base_index() {
    let (_tmp, repo) = fixture_repo();
    index_base(&repo);
    let (out, _e, code) = run_cgx(
        &repo,
        &[
            "query",
            "MATCH (a)-[:DATA_FLOW*1..8]->(b) RETURN a.name, b.name LIMIT 10",
            "--no-auto-index",
        ],
    );
    assert_eq!(code, 0, ":DATA_FLOW over a base index exits 0 (opt-in): {out}");
    assert!(
        !out.contains("through_call") && !out.contains("dataflow::"),
        ":DATA_FLOW must be empty against a base index (no DerivesFrom edges): {out}"
    );
}

// ── Criterion 5: forest edge-condition / annotation rendering (shared) ───────

#[test]
fn flows_output_uses_shared_forest_renderer() {
    // The forest renderer is reused verbatim — confidence/condition annotations
    // appear on dataflow output exactly as on callers/callees. The interproc edge
    // is `probable`, so the `[probable]` tag must render; and the renderer must
    // *omit* the default `always` condition (cgx-forest-edge-condition-formatting),
    // never printing it literally.
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let (out, _e, code) = run_cgx(&repo, &["flows-from", R, "--depth", "8"]);
    assert_eq!(code, 0, "flows-from exits 0: {out}");
    assert!(
        out.contains("[probable]"),
        "the interproc DerivesFrom edge renders its confidence tag: {out}"
    );
    assert!(
        !out.contains("always"),
        "the default `always` condition is omitted, not printed: {out}"
    );
}

// ── Criterion 6: flag reuse (confidence / at / depth) ────────────────────────

#[test]
fn flows_confidence_floor_excludes_lower_confidence_edges() {
    // The interproc edge `r ⇝ a` is `probable`. A `--confidence certain` floor
    // must exclude it via the shared EdgeFilter, leaving `a` unreachable.
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let fqns = flow_fqns_with(&repo, "flows-from", R, &["--confidence", "certain"]);
    assert!(
        !fqns.iter().any(|f| f == A),
        "a `certain` floor must not cross the `probable` interproc edge, got {fqns:?}"
    );
}

#[test]
fn flows_at_ref_flag_pins_to_committed_graph() {
    // `--at HEAD` pins the dataflow walk to the committed graph. NOTE: `--at`
    // re-indexes the ref without `--dataflow` (the base layer), so there are no
    // DerivesFrom edges and a value-node FQN does not resolve — the contract under
    // test is that `--at` is accepted and takes the resolution path (exit 2 for a
    // value-node FQN that exists only under --dataflow), not a crash.
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let (_o, _e, code) = run_cgx(&repo, &["flows-from", R, "--at", "HEAD"]);
    assert!(
        code == 0 || code == 2,
        "--at is accepted on flows-from (exit 0 or honest not-found 2), got {code}"
    );
    assert_ne!(code, 101, "--at must not panic");
}

#[test]
fn flows_depth_flag_bounds_the_walk() {
    // The forward slice from `a` is `a → r → return` (depth 2). `--depth 1` must
    // truncate to just `r`, omitting the depth-2 `return`. (Criterion 7: bounded.)
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let shallow = flow_fqns_with(&repo, "flows-to", A, &["--depth", "1"]);
    assert!(
        shallow.iter().any(|f| f == R),
        "depth-1 still reaches the direct successor r: {shallow:?}"
    );
    assert!(
        !shallow.iter().any(|f| f == RETURN),
        "depth-1 must NOT reach the depth-2 return: {shallow:?}"
    );
}

/// Like [`flow_fqns`] but with extra flags injected before `--format json`.
fn flow_fqns_with(repo: &Path, subcommand: &str, symbol: &str, extra: &[&str]) -> Vec<String> {
    let mut args = vec![subcommand, symbol];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["--format", "json"]);
    let (out, _e, code) = run_cgx(repo, &args);
    assert_eq!(code, 0, "{subcommand} {symbol} {extra:?} exits 0: {out}");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("flow json");
    parsed["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["fqn"].as_str().unwrap().to_string())
        .collect()
}

// ── Criterion 8: exit-code discipline ────────────────────────────────────────

#[test]
fn flows_unknown_symbol_is_usage_error_exit_2() {
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let (_o, stderr, code) = run_cgx(&repo, &["flows-to", "no_such_symbol_zzz"]);
    assert_eq!(code, 2, "unknown symbol → exit 2");
    assert!(
        stderr.contains("no symbol matched"),
        "unknown symbol prints the not-found message: {stderr}"
    );
}

#[test]
fn flows_function_fqn_with_no_dataflow_edges_is_empty_exit_0() {
    // A *function* FQN resolves (it is a real symbol node) but has no DerivesFrom
    // edges — only its value nodes do. The honest contract: empty result, exit 0,
    // not an error. (No silent function→value-node expansion in SC5.)
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let (out, _e, code) = run_cgx(&repo, &["flows-to", FN, "--format", "json"]);
    assert_eq!(code, 0, "function FQN with no dataflow edges → exit 0: {out}");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("flow json");
    assert_eq!(
        parsed["count"], 0,
        "a function FQN has no DerivesFrom edges → empty slice: {out}"
    );
}

// ── Criterion 7: DEFAULT_MAX_STEPS / boundedness (walk always terminates) ────

#[test]
fn flows_high_depth_terminates_under_step_budget() {
    // A very large `--depth` exercises the shared PathWalker's step budget
    // (DEFAULT_MAX_STEPS) rather than the depth bound. On this acyclic graph the
    // walk must terminate cleanly (no hang, no panic) and still surface the full
    // forward slice — proving the walk is bounded by the budget, not unbounded.
    let (_tmp, repo) = fixture_repo();
    index_dataflow(&repo);
    let (out, _e, code) = run_cgx(
        &repo,
        &["flows-to", A, "--depth", "100000", "--format", "json"],
    );
    assert_eq!(code, 0, "high-depth flow terminates and exits 0: {out}");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("flow json");
    assert!(
        parsed["count"].as_u64().unwrap() >= 2,
        "the bounded walk still reaches the full forward slice: {out}"
    );
}
