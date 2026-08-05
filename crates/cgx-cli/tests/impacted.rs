//! `cgx impacted-tests` end-to-end, driven through the built binary.
//!
//! The headline case is the **degenerate answer**: a diff that touches only a
//! language cgx cannot identify tests in must never render as a clean empty
//! list. Three independent signals are asserted here — exit 4, a `cgx: …` line
//! on stderr, and an `impacted-language-unsupported` reason with a non-`exact`
//! direction on the contract — on every format a machine consumer might read.
//!
//! The rest covers the surface contract: the witness forest roots at the change
//! and descends to the test, the per-row weakest-link tier reaches CLI JSON, and
//! the path-graph formats are a usage error rather than silently-wrong output
//! (the `print_diff_full` defect this command must not reproduce).

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

/// A library whose test reaches the changed symbol through one intermediate hop,
/// so the answer exercises a real multi-hop witness chain rather than a direct
/// call. The test's call to `mid` is bound to a local **before** the assertion:
/// a call written inside `assert_eq!(..)` sits in an unexpanded macro and
/// produces no graph edge at all.
const API_BASE: &str = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
const API_HEAD: &str = "pub fn add(a: i32, b: i32) -> i32 { b + a }\n";
const CHECK: &str = "\
use crate::api::add;
pub fn mid(x: i32) -> i32 { add(x, 1) }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_mid() {
        let got = mid(1);
        assert_eq!(got, 2);
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

fn commit(repo: &Path, msg: &str) {
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
            "--date=2020-01-01T00:00:00Z",
        ],
    );
}

/// A fresh single-commit git repo with a deterministic identity.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("repo");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "user.name", "cgx-test"]);
    git(&path, &["config", "user.email", "cgx@test.invalid"]);
    git(&path, &["config", "commit.gpgsign", "false"]);
    (tmp, path)
}

/// The rust fixture: `add` <- `mid` <- `test_mid`, committed, with `add`'s body
/// then edited in the working tree.
fn rust_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, path) = repo();
    write(&path, "src/lib.rs", "pub mod api;\npub mod check;\n");
    write(&path, "src/api.rs", API_BASE);
    write(&path, "src/check.rs", CHECK);
    commit(&path, "base");
    write(&path, "src/api.rs", API_HEAD);
    (tmp, path)
}

/// A repo whose only working-tree edit is a TypeScript file — the language cgx
/// cannot identify tests in at all.
fn typescript_only_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, path) = repo();
    write(&path, "src/lib.rs", "pub fn keep() -> i32 { 1 }\n");
    write(
        &path,
        "web/app.ts",
        "export function greet(): string { return \"hi\"; }\n",
    );
    commit(&path, "base");
    write(
        &path,
        "web/app.ts",
        "export function greet(): string { return \"hello\"; }\n",
    );
    (tmp, path)
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

fn reason_codes(doc: &serde_json::Value) -> Vec<String> {
    doc["approximation"]["reasons"]
        .as_array()
        .expect("reasons array")
        .iter()
        .map(|r| r["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

// --- the degenerate answer ---------------------------------------------------

/// **The highest-damage failure this feature can ship.** TypeScript/JavaScript is
/// unsupported, so a TS-only diff yields no tests — and an empty list that reads
/// as "nothing to run" would be a silent false negative in the one direction that
/// matters for a CI gate.
///
/// All three signals must be present at once: a non-zero exit, a stderr line, and
/// a machine-readable reason naming the language on a contract that is explicitly
/// not `exact`.
#[test]
fn a_typescript_only_change_is_visibly_degenerate_not_a_clean_empty_list() {
    let (_tmp, repo) = typescript_only_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--format", "json"],
    );

    assert_eq!(
        code, 4,
        "a vacuous answer exits 4: stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stderr.trim().is_empty(),
        "a degenerate answer says so on stderr"
    );
    assert!(
        stderr.contains("typescript"),
        "stderr names the unanalysed language: {stderr}"
    );
    assert!(
        stderr.contains("not a clean bill of health"),
        "stderr says the empty result is vacuous: {stderr}"
    );

    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json body still emitted");
    assert_eq!(doc["count"].as_u64(), Some(0));
    assert!(
        reason_codes(&doc).contains(&"impacted-language-unsupported".to_string()),
        "the JSON contract names the gap: {stdout}"
    );
    assert_ne!(
        doc["approximation"]["direction"].as_str(),
        Some("exact"),
        "a degenerate answer can never claim to be exact: {stdout}"
    );
    assert!(
        stdout.contains("typescript"),
        "the JSON reason detail names the language, not just the code: {stdout}"
    );
}

/// The same gap must be legible on the human and SARIF surfaces, not only JSON —
/// a user reading the terminal, and a CI system reading SARIF, both have to be
/// able to tell "not analysed" from "nothing affected".
#[test]
fn the_unsupported_language_gap_is_legible_on_human_and_sarif_too() {
    let (_tmp, repo) = typescript_only_fixture();

    let (human, stderr, code) = run_cgx(&repo, &["impacted-tests", "--uncommitted"]);
    assert_eq!(code, 4, "human format degenerates the same way: {stderr}");
    assert!(
        human.contains("cgx cannot identify tests"),
        "the human contract line names the gap: {human}"
    );
    assert!(
        human.contains("under-approximate"),
        "the human contract line states the direction: {human}"
    );

    let (sarif, _stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--format", "sarif"],
    );
    assert_eq!(code, 4);
    let doc: serde_json::Value = serde_json::from_str(&sarif).expect("parseable SARIF");
    let results = doc["runs"][0]["results"].as_array().expect("sarif results");
    let contract = results
        .iter()
        .find(|r| r["ruleId"] == "cgx/approximation-contract")
        .expect("the contract rides on the SARIF document");
    let codes = contract["properties"]["reasons"]
        .as_array()
        .expect("reasons in SARIF properties")
        .iter()
        .map(|r| r["code"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"impacted-language-unsupported"),
        "SARIF carries the reason code: {sarif}"
    );
}

// --- the answer --------------------------------------------------------------

#[test]
fn a_rust_change_reports_the_test_that_reaches_it_with_its_weakest_tier() {
    let (_tmp, repo) = rust_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--format", "json"],
    );

    assert_eq!(code, 0, "a real answer exits 0: {stderr}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let rows = doc["results"].as_array().expect("results array");
    assert_eq!(rows.len(), 1, "exactly the one impacted test: {stdout}");
    let row = &rows[0];
    assert!(
        row["fqn"].as_str().unwrap().ends_with("::test_mid"),
        "the reaching test is reported: {row}"
    );

    // Q3: the row carries the weakest hop between the test and the change, which
    // is strictly weaker here than the discovery edge's own confidence. Getting
    // the two confused is the whole reason `min_confidence_on_path` exists.
    assert_eq!(
        row["min_confidence_on_path"].as_str(),
        Some("probable"),
        "the weakest-link tier is the path's, not the discovery edge's: {row}"
    );
    assert_eq!(row["confidence"].as_str(), Some("certain"), "row={row}");

    // The file-granular half of the changed set is disclosed as an over-approx.
    assert!(
        reason_codes(&doc).contains(&"impacted-changed-set-file-granular".to_string()),
        "the weaker input-side claim is stated: {stdout}"
    );
}

/// A shipped neighbor command must be byte-for-byte unaffected by the added
/// `Finding` field: `min_confidence_on_path` appears on `impacted-tests` rows
/// and nowhere else.
#[test]
fn the_added_row_field_does_not_leak_into_shipped_subcommands() {
    let (_tmp, repo) = rust_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["callers", "rust_sample::api::add", "--format", "json"],
    );
    assert_eq!(code, 0, "{stderr}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    for row in doc["results"].as_array().expect("results") {
        assert!(
            row.get("min_confidence_on_path").is_none(),
            "callers keeps its shipped row shape: {row}"
        );
    }
}

/// The human view is a forest rooted at the **changed symbol** and descending to
/// the tests, so a reader sees *why* each test is implicated, not just that it is.
#[test]
fn the_witness_forest_roots_at_the_change_and_descends_to_the_test() {
    let (_tmp, repo) = rust_fixture();
    let (stdout, stderr, code) = run_cgx(&repo, &["impacted-tests", "--uncommitted"]);
    assert_eq!(code, 0, "{stderr}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines[0].starts_with("rust_sample::api::add"),
        "the forest roots at the changed symbol: {stdout}"
    );
    let test_line = lines
        .iter()
        .find(|l| l.contains("::test_mid"))
        .unwrap_or_else(|| panic!("the test appears in the forest: {stdout}"));
    assert!(
        test_line.starts_with(' ')
            || test_line.starts_with('└')
            || test_line.starts_with('├')
            || test_line.contains("─"),
        "the test hangs below the change rather than being its own root: {stdout}"
    );
    assert!(
        !stdout.contains("(cycle)"),
        "no self-referential witness row: {stdout}"
    );
}

// --- CI mode -----------------------------------------------------------------

#[test]
fn two_committed_refs_report_the_test_impacted_by_the_branch() {
    let (_tmp, repo) = rust_fixture();
    commit(&repo, "head: reorder the addends");

    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "HEAD~1", "HEAD", "--format", "json"],
    );
    assert_eq!(code, 0, "{stderr}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let rows = doc["results"].as_array().expect("results");
    assert!(
        rows.iter().any(|r| r["fqn"]
            .as_str()
            .unwrap_or_default()
            .ends_with("::test_mid")),
        "CI mode reaches the same test: {stdout}"
    );
    assert!(
        !reason_codes(&doc).contains(&"impacted-tip-to-tip-base".to_string()),
        "the default is merge-base, so the tip-to-tip over-reason stays silent: {stdout}"
    );
}

#[test]
fn opting_out_of_merge_base_is_disclosed_on_the_contract() {
    let (_tmp, repo) = rust_fixture();
    commit(&repo, "head: reorder the addends");

    let (stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "HEAD~1",
            "HEAD",
            "--no-merge-base",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert!(
        reason_codes(&doc).contains(&"impacted-tip-to-tip-base".to_string()),
        "a tip base is an over-approximation and says so: {stdout}"
    );
}

// --- format guard ------------------------------------------------------------

/// The anti-`print_diff_full` test. `cgx diff` renders `--format sarif|dot|...`
/// as human text at exit 0 because it matches `Format` with a `_` arm. Routing
/// through `emit` inherits the exhaustive guard, so a path-graph format against
/// this neighbor-shaped result is a usage error — never silently-wrong output.
#[test]
fn path_graph_formats_are_a_usage_error_not_silent_human_text() {
    let (_tmp, repo) = rust_fixture();
    for format in ["dot", "mermaid", "d2"] {
        let (stdout, stderr, code) = run_cgx(
            &repo,
            &["impacted-tests", "--uncommitted", "--format", format],
        );
        assert_eq!(code, 2, "{format} is a usage error: stdout={stdout}");
        assert!(
            stderr.contains("only valid for path-returning results"),
            "{format} says why: {stderr}"
        );
        assert!(
            !stdout.contains("test_mid"),
            "{format} emits no answer body at all: {stdout}"
        );
    }
}

#[test]
fn sarif_is_parseable_and_carries_the_approximation_contract() {
    let (_tmp, repo) = rust_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--format", "sarif"],
    );
    assert_eq!(code, 0, "{stderr}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("parseable SARIF");
    assert_eq!(doc["version"].as_str(), Some("2.1.0"));
    let results = doc["runs"][0]["results"].as_array().expect("results");
    assert!(
        results
            .iter()
            .any(|r| r["ruleId"] == "cgx/approximation-contract"),
        "the contract rides on SARIF: {stdout}"
    );
    let finding = results
        .iter()
        .find(|r| r["ruleId"] == "cgx/impacted-tests")
        .expect("the impacted test is a SARIF finding");
    assert_eq!(
        finding["properties"]["minConfidenceOnPath"].as_str(),
        Some("probable"),
        "SARIF carries the weakest-link tier: {finding}"
    );
}

// --- argument validation -----------------------------------------------------

#[test]
fn conflicting_or_missing_sides_are_usage_errors() {
    let (_tmp, repo) = rust_fixture();

    let cases: Vec<(&str, Vec<&str>)> = vec![
        (
            "--uncommitted with positional refs",
            vec!["impacted-tests", "HEAD~1", "--uncommitted"],
        ),
        (
            "neither a base ref nor --uncommitted",
            vec!["impacted-tests"],
        ),
        (
            "--at, which pins a single ref's graph",
            vec!["impacted-tests", "--uncommitted", "--at", "HEAD"],
        ),
    ];

    for (what, args) in cases {
        let (stdout, stderr, code) = run_cgx(&repo, &args);
        assert_eq!(code, 2, "{what} exits 2: stdout={stdout} stderr={stderr}");
        assert!(
            !stderr.trim().is_empty(),
            "{what} explains itself: {stderr}"
        );
    }
}

// --- known defect ------------------------------------------------------------

/// **Escalation, dispatch 10.** A clean working tree must not be degenerate: the
/// answer "no tests are impacted because nothing changed" is a real answer, not a
/// vacuous one.
///
/// It currently exits 4. `cgx_diff::impacted::run` derives its changed-path set
/// from `Repo::enumerate_workdir`, which walks every file under the repository
/// root except `.git` — it consults no gitignore and no tracked-file set. cgx's
/// own `.cgx/` store therefore lands in the working-tree manifest, is absent from
/// the committed tree, and reads as four changed files that contribute no
/// symbols; the degenerate predicate (`changed paths non-empty` + `no supported
/// language in the changed set`) then fires on an unedited tree. Any untracked
/// build output (`target/`, `node_modules/`) does the same, and on a real repo it
/// is also a traversal-cost cliff.
///
/// The fix is in `cgx-diff/src/impacted.rs`, which this dispatch scopes out
/// (`cgx-query` and `cgx-diff` are not to be modified). Ignored rather than
/// deleted so the fix has a target that fails today and passes when it lands.
#[test]
#[ignore = "escalated: cgx-diff's changed-path set counts untracked/ignored files (see doc comment)"]
fn an_unchanged_working_tree_is_not_degenerate() {
    let (_tmp, repo) = repo();
    write(&repo, "src/lib.rs", "pub fn keep() -> i32 { 1 }\n");
    commit(&repo, "base");

    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--format", "json"],
    );
    assert_eq!(
        code, 0,
        "nothing changed is a real answer, not a vacuous one: stdout={stdout} stderr={stderr}"
    );
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(doc["count"].as_u64(), Some(0));
    assert!(
        !reason_codes(&doc).contains(&"impacted-changed-file-unindexed".to_string()),
        "cgx's own store is not a changed source file: {stdout}"
    );
}
