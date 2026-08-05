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

// --- two changed symbols on one witness chain --------------------------------
//
// The trigger is one edited file holding a caller and its callee — the ordinary
// case, and the shape every earlier fixture missed by putting one changed symbol
// per file. Both seeds are marked reached before any expansion, so the seed
// lying *between* the expanding root and the test gets no `via_of` entry while
// the walk runs past it. Stamping the witness root forward from the expanding
// root then put a node in `Subgraph.roots` that never entered `Subgraph.nodes`:
// the human forest silently dropped the row whose chain was orphaned (exit 0, no
// truncation marker, no contract reason) or panicked in `Forest::node`.
//
// The invariant these tests hold is **cross-surface agreement on the row count**:
// human, JSON and SARIF must report the same tests, because that is what was
// violated and it is the only signal a reader of the default format has.

/// `add ← mid` in one file, `top ← test_top` in a second, `test_add → add` in a
/// third. Editing `core.rs` puts both `add` and `mid` in the changed set, with
/// `mid` sitting strictly between `add` and `test_top`.
fn rust_seed_on_chain_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, path) = repo();
    write(
        &path,
        "src/lib.rs",
        "pub mod core;\npub mod util;\npub mod direct;\n",
    );
    write(
        &path,
        "src/core.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn mid(x: i32) -> i32 { add(x, 1) }\n",
    );
    write(
        &path,
        "src/util.rs",
        "use crate::core::mid;\n\
         pub fn top(x: i32) -> i32 { mid(x) }\n\
         #[cfg(test)]\n\
         mod tests {\n\
         use super::*;\n\
         #[test]\n\
         fn test_top() { let got = top(1); assert_eq!(got, 2); }\n\
         }\n",
    );
    write(
        &path,
        "src/direct.rs",
        "use crate::core::add;\n\
         #[cfg(test)]\n\
         mod tests {\n\
         use super::*;\n\
         #[test]\n\
         fn test_add() { let got = add(1, 1); assert_eq!(got, 2); }\n\
         }\n",
    );
    commit(&path, "base");
    write(
        &path,
        "src/core.rs",
        "pub fn add(a: i32, b: i32) -> i32 { b + a }\npub fn mid(x: i32) -> i32 { add(x, 1) }\n",
    );
    (tmp, path)
}

/// The same shape in Go: `core.go` holds `Add` and its caller `Mid`, `top.go`
/// holds `Top`, and the test reaches the change only through both seeds.
fn go_seed_on_chain_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, path) = repo();
    write(&path, "go.mod", "module demo\n\ngo 1.21\n");
    write(
        &path,
        "core.go",
        "package demo\n\nfunc Add(a, b int) int { return a + b }\n\nfunc Mid(x int) int { return Add(x, 1) }\n",
    );
    write(
        &path,
        "top.go",
        "package demo\n\nfunc Top(x int) int { return Mid(x) }\n",
    );
    write(
        &path,
        "core_test.go",
        "package demo\n\nimport \"testing\"\n\nfunc TestTop(t *testing.T) {\n\tgot := Top(1)\n\tif got != 2 {\n\t\tt.Fatal(\"bad\")\n\t}\n}\n",
    );
    commit(&path, "base");
    write(
        &path,
        "core.go",
        "package demo\n\nfunc Add(a, b int) int { return b + a }\n\nfunc Mid(x int) int { return Add(x, 1) }\n",
    );
    (tmp, path)
}

/// The FQNs each surface reports, for one invocation of the same query. The
/// human set is the forest lines whose leading token is a reported test — the
/// forest also renders the intermediate call chain, so it is intersected with
/// the JSON row set rather than counted blind.
fn rows_on_every_surface(repo: &Path, extra: &[&str]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut human_args = vec!["impacted-tests", "--uncommitted"];
    human_args.extend_from_slice(extra);
    let (human, h_err, h_code) = run_cgx(repo, &human_args);
    assert_eq!(h_code, 0, "human render must not panic or fail: {h_err}");

    let mut json_args = human_args.clone();
    json_args.extend_from_slice(&["--format", "json"]);
    let (j_out, j_err, j_code) = run_cgx(repo, &json_args);
    assert_eq!(j_code, 0, "{j_err}");
    let doc: serde_json::Value = serde_json::from_str(&j_out).expect("json");
    let mut json_rows: Vec<String> = doc["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|r| r["fqn"].as_str().unwrap_or_default().to_string())
        .collect();
    json_rows.sort();

    let mut sarif_args = human_args.clone();
    sarif_args.extend_from_slice(&["--format", "sarif"]);
    let (s_out, s_err, s_code) = run_cgx(repo, &sarif_args);
    assert_eq!(s_code, 0, "{s_err}");
    let sarif: serde_json::Value = serde_json::from_str(&s_out).expect("sarif");
    let mut sarif_rows: Vec<String> = sarif["runs"][0]["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter(|r| r["ruleId"] == "cgx/impacted-tests")
        .map(|r| {
            r["message"]["text"]
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    sarif_rows.sort();

    let mut human_rows: Vec<String> = human
        .lines()
        .filter(|l| !l.starts_with("approximation:"))
        .filter_map(|l| {
            l.replace(['└', '├', '│', '─'], " ")
                .split_whitespace()
                .next()
                .map(|s| s.to_string())
        })
        .filter(|fqn| json_rows.contains(fqn))
        .collect();
    human_rows.sort();
    human_rows.dedup();

    (human_rows, json_rows, sarif_rows)
}

#[test]
fn a_caller_and_its_callee_in_one_edited_file_agree_on_every_surface_rust() {
    let (_tmp, repo) = rust_seed_on_chain_fixture();
    let (human, json, sarif) = rows_on_every_surface(&repo, &[]);
    assert_eq!(json.len(), 2, "both tests are impacted: {json:?}");
    assert_eq!(human, json, "the human forest dropped a reported test");
    assert_eq!(sarif, json, "SARIF and JSON disagree");
    assert!(
        json.iter().any(|f| f.ends_with("::test_top")),
        "the test reached only through the interior seed is present: {json:?}"
    );
}

#[test]
fn a_caller_and_its_callee_in_one_edited_file_agree_on_every_surface_go() {
    let (_tmp, repo) = go_seed_on_chain_fixture();
    let (human, json, sarif) = rows_on_every_surface(&repo, &[]);
    assert_eq!(json.len(), 1, "the Go test is impacted: {json:?}");
    assert_eq!(human, json, "the human forest dropped a reported test");
    assert_eq!(sarif, json, "SARIF and JSON disagree");
}

#[test]
fn the_orphaned_chain_shape_renders_in_both_tree_modes() {
    // The panic manifestation: the bogus root was on no other row's chain, so it
    // was absent from the node map entirely and `Forest::node` indexed a missing
    // key — on the default format, in both `--tree` modes, and in CI mode.
    let (_tmp, repo) = rust_seed_on_chain_fixture();
    for mode in ["full", "spanning"] {
        let (human, json, _) = rows_on_every_surface(&repo, &["--tree", mode]);
        assert_eq!(human, json, "--tree {mode} dropped a reported test");
    }
}

#[test]
fn ci_mode_over_two_refs_survives_a_seed_on_the_chain() {
    let (_tmp, repo) = rust_seed_on_chain_fixture();
    commit(&repo, "head: reorder the addends");
    let (stdout, stderr, code) = run_cgx(&repo, &["impacted-tests", "HEAD~1", "HEAD"]);
    assert_eq!(code, 0, "the PR-gate mode must not panic: {stderr}");
    assert!(
        stdout.contains("::test_top") && stdout.contains("::test_add"),
        "both tests render in CI mode: {stdout}"
    );
}

/// Every rendered witness root must be a node the forest can resolve. Asserted
/// through the observable consequence: the roots of the forest are exactly the
/// changed symbols that actually reached a test, each with its own subtree.
#[test]
fn the_forest_roots_at_the_nearest_changed_symbol_on_each_chain() {
    let (_tmp, repo) = rust_seed_on_chain_fixture();
    let (stdout, stderr, code) = run_cgx(&repo, &["impacted-tests", "--uncommitted"]);
    assert_eq!(code, 0, "{stderr}");
    let roots: Vec<&str> = stdout
        .lines()
        .filter(|l| !l.starts_with(' ') && !l.starts_with("approximation:"))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert!(
        roots.iter().any(|r| r.ends_with("::mid")),
        "`test_top`'s chain terminates at `mid`, so `mid` is a root: {stdout}"
    );
    assert!(
        roots.iter().any(|r| r.ends_with("::add")),
        "`test_add`'s chain terminates at `add`: {stdout}"
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

// --- the clean-tree regression -----------------------------------------------

/// A clean working tree must not be degenerate: "no tests are impacted because
/// nothing changed" is a real answer, not a vacuous one.
///
/// This is the end-to-end guard on the changed-path narrowing in
/// `cgx_diff::impacted::run`. The CLI writes its store to `.cgx/` inside the
/// repository, and `Repo::enumerate_workdir` walks every file under the root
/// except `.git` — no gitignore, no tracked-file set. Without the narrowing
/// cgx's own store reads as four changed files that contribute no symbols, the
/// vacuity predicate fires on an unedited tree, and the inner loop's default
/// invocation exits 4 on every clean checkout.
#[test]
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

// --- the `--assert-empty` gate ------------------------------------------------

/// A library symbol reached by **no** test. The repo's one test reaches `add`
/// through `mid`; `lonely` is called by nothing, so editing it is the shape a CI
/// gate exists for — a real, non-degenerate, empty answer.
const LONELY_BASE: &str = "pub fn lonely() -> i32 { 7 }\n";
const LONELY_HEAD: &str = "pub fn lonely() -> i32 { 8 }\n";

/// [`rust_fixture`] plus `lonely`, with the working-tree edit moved onto
/// `lonely.rs` so the changed set is non-empty and reaches no test.
fn unreached_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, path) = repo();
    write(
        &path,
        "src/lib.rs",
        "pub mod api;\npub mod check;\npub mod lonely;\n",
    );
    write(&path, "src/api.rs", API_BASE);
    write(&path, "src/check.rs", CHECK);
    write(&path, "src/lonely.rs", LONELY_BASE);
    commit(&path, "base");
    write(&path, "src/lonely.rs", LONELY_HEAD);
    (tmp, path)
}

/// A repo whose only working-tree edit is a tracked file no adapter claims.
fn no_symbol_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, path) = repo();
    write(&path, "src/lib.rs", "pub fn keep() -> i32 { 1 }\n");
    write(&path, "NOTES.md", "first\n");
    commit(&path, "base");
    write(&path, "NOTES.md", "second\n");
    (tmp, path)
}

/// **The gate must be able to pass.** A change that reaches no test at all is the
/// success case `--assert-empty` was added for, and it has to exit 0 on its own —
/// not via `--allow-vacuous`, which also suppresses genuine vacuity and would
/// silently disable the safety property the gate provides.
///
/// The regression it guards: ADR-08 clause (b) fires on `unfiltered_count > 0`,
/// meaning "candidates existed and the user's filters removed them all". Feeding
/// it the reverse walk's reached-node count made it true on every run — seeds are
/// marked reached at seeding — so an honest empty answer always read as vacuous
/// and blamed confidence filters the user never set.
#[test]
fn an_empty_impacted_set_with_no_filters_passes_assert_empty() {
    let (_tmp, repo) = unreached_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "--uncommitted",
            "--assert-empty",
            "--format",
            "json",
        ],
    );

    assert_eq!(
        code, 0,
        "no test reaches the change: the gate passes. stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stderr.contains("vacuous"),
        "nothing was filtered, so nothing may be blamed on a filter: {stderr}"
    );
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(doc["count"].as_u64(), Some(0));
    assert_eq!(
        doc["vacuous"].as_bool(),
        Some(false),
        "an honest pass is not flagged vacuous: {stdout}"
    );
}

/// The assertion still fires. Fixing the vacuity misfire must not turn the gate
/// into a no-op.
#[test]
fn a_non_empty_impacted_set_fails_assert_empty() {
    let (_tmp, repo) = rust_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--assert-empty"],
    );
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("assertion failed"),
        "the failure names itself: {stderr}"
    );
}

/// A degenerate answer keeps its exit 4 under the gate. The empty list is not a
/// clean bill of health, and `--assert-empty` must not launder it into one.
#[test]
fn a_typescript_only_diff_still_exits_4_under_assert_empty() {
    let (_tmp, repo) = typescript_only_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--assert-empty"],
    );
    assert_eq!(code, 4, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("typescript") && stderr.contains("not a clean bill of health"),
        "the degenerate reason survives the gate: {stderr}"
    );
}

/// The other degenerate shape: everything that changed is a file no adapter
/// claims, so the changed set is empty and clause (a) — the zero-symbol match —
/// is the honest verdict.
#[test]
fn a_changed_file_contributing_no_symbols_still_exits_4_under_assert_empty() {
    let (_tmp, repo) = no_symbol_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--assert-empty"],
    );
    assert_eq!(code, 4, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("matched zero symbols"),
        "clause (a) is the reason, and says so: {stderr}"
    );
}

/// Clause (b) must still fire when it is actually true. `--confidence certain`
/// excludes the `probable` hop between `test_mid` and the change, so the one
/// candidate the query would otherwise return is removed **by a filter** — the
/// exact condition ADR-08 clause (b) is defined over.
#[test]
fn a_confidence_floor_that_excludes_every_candidate_is_still_vacuous() {
    let (_tmp, repo) = rust_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "--uncommitted",
            "--assert-empty",
            "--confidence",
            "certain",
        ],
    );
    assert_eq!(
        code, 4,
        "a filter removed the only candidate: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("confidence/edge-condition filters excluded every candidate result"),
        "and clause (b) is the reason given: {stderr}"
    );

    // The control: same repo, same query, no floor — the candidate is there, so
    // the gate fails rather than passing vacuously.
    let (_stdout, _stderr, code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--assert-empty"],
    );
    assert_eq!(code, 1, "without the floor the candidate is reported");
}

// --- the `--depth` truncation guard -------------------------------------------
//
// A depth bound is not a confidence filter, so ADR-08 clause (b) does not cover
// it — but it produces the same damage on the same surface. `--depth N` added
// for speed can cut the only route from the change to a test, and the gate then
// exits 0 with an empty stderr, which is exactly the code documented as "a
// change that genuinely reaches no test". The contract does carry `depth-limit`,
// but a CI author reads the exit code.

/// `--depth 1` cannot reach `test_mid`, which sits two hops from the change. The
/// gate must not report that as a pass indistinguishable from a real one.
#[test]
fn a_depth_bound_that_truncates_the_only_route_cannot_pass_the_gate() {
    let (_tmp, repo) = rust_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "--uncommitted",
            "--depth",
            "1",
            "--assert-empty",
        ],
    );
    assert_eq!(
        code, 4,
        "a depth-truncated empty answer is not a clean bill of health: \
         stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("--depth 1") && stderr.contains("truncated"),
        "the reason names the bound that caused it, not a filter the user never set: {stderr}"
    );
    assert!(
        !stderr.contains("confidence/edge-condition filters"),
        "no confidence filter is in play, so clause (b) must not be blamed: {stderr}"
    );
}

/// The machine-readable half of the same disclosure: the contract carries
/// `depth-limit` as an `under` reason on the very answer whose gate exited 4.
#[test]
fn the_depth_truncated_answer_carries_depth_limit_on_the_contract() {
    let (_tmp, repo) = rust_fixture();
    let (stdout, _stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "--uncommitted",
            "--depth",
            "1",
            "--assert-empty",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 4);
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json body still emitted");
    assert_eq!(doc["count"].as_u64(), Some(0));
    assert!(
        reason_codes(&doc).contains(&"depth-limit".to_string()),
        "the truncation is on the contract too: {stdout}"
    );
    assert_ne!(doc["approximation"]["direction"].as_str(), Some("exact"));
}

/// The guard is a counterfactual, not a blanket ban on `--depth`: a bound that
/// removes nothing leaves the honest exit 0 exactly as it was. `lonely` is
/// called by nothing, so no depth reaches a test and the pass is real.
#[test]
fn a_depth_bound_that_changes_nothing_still_passes_the_gate() {
    let (_tmp, repo) = unreached_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "--uncommitted",
            "--depth",
            "1",
            "--assert-empty",
        ],
    );
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.trim().is_empty(),
        "an honest pass says nothing on stderr: {stderr}"
    );
}

/// And the assertion itself still fires when the bound is wide enough to see the
/// test — the guard must not turn a real failure into a vacuity report.
#[test]
fn a_depth_bound_wide_enough_to_see_the_test_still_fails_the_assertion() {
    let (_tmp, repo) = rust_fixture();
    let (_stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "--uncommitted",
            "--depth",
            "2",
            "--assert-empty",
        ],
    );
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("assertion failed"), "{stderr}");
}

/// `--allow-vacuous` is the documented "I know, proceed" escape, and it covers
/// this vacuity the same way it covers ADR-08's two clauses: exit 0, with the
/// reason still on stderr.
#[test]
fn allow_vacuous_downgrades_a_depth_truncated_gate_but_keeps_the_warning() {
    let (_tmp, repo) = rust_fixture();
    let (_stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "--uncommitted",
            "--depth",
            "1",
            "--assert-empty",
            "--allow-vacuous",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.contains("passed vacuously") && stderr.contains("--depth 1"),
        "the downgrade still names what was truncated: {stderr}"
    );
}

/// A plain query is unaffected: a depth bound narrowing the question is an
/// honest, disclosed answer, and only the *gate* needs the extra guard.
#[test]
fn a_depth_bound_without_the_gate_is_still_a_plain_exit_zero() {
    let (_tmp, repo) = rust_fixture();
    let (_stdout, stderr, code) =
        run_cgx(&repo, &["impacted-tests", "--uncommitted", "--depth", "1"]);
    assert_eq!(code, 0, "{stderr}");
}

// --- the degenerate reason survives the gate ----------------------------------

/// `emit` returns `Err` for an ADR-08 vacuous pass, which used to short-circuit
/// the degenerate check below it. Both exits are 4, so no gate misbehaved — but
/// adding `--assert-empty` replaced the message that names the cause with one
/// that does not. The degenerate reason now wins whenever both fire.
#[test]
fn adding_assert_empty_does_not_cost_the_user_the_degenerate_reason() {
    let (_tmp, repo) = no_symbol_fixture();
    let (_stdout, plain_err, plain_code) = run_cgx(&repo, &["impacted-tests", "--uncommitted"]);
    assert_eq!(plain_code, 4, "{plain_err}");
    assert!(
        plain_err.contains("no changed file contributed an indexed symbol"),
        "the plain run names the cause: {plain_err}"
    );

    let (_stdout, gate_err, gate_code) = run_cgx(
        &repo,
        &["impacted-tests", "--uncommitted", "--assert-empty"],
    );
    assert_eq!(gate_code, 4, "same exit code either way: {gate_err}");
    assert!(
        gate_err.contains("no changed file contributed an indexed symbol"),
        "and the gate must not swallow it: {gate_err}"
    );
}

/// The degenerate reason wins over the generic vacuity line, but a *usage* error
/// still wins over both — it is about the invocation, not the answer.
#[test]
fn a_format_usage_error_outranks_the_degenerate_reason() {
    let (_tmp, repo) = typescript_only_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "impacted-tests",
            "--uncommitted",
            "--assert-empty",
            "--format",
            "dot",
        ],
    );
    assert_eq!(code, 2, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("only valid for path-returning results"),
        "{stderr}"
    );
    assert!(
        stdout.is_empty(),
        "no answer body on a usage error: {stdout}"
    );
}
