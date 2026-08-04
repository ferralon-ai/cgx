//! CH-11 S0+S1 end-to-end: the `cgx diff` post-filters and the `--path-added`
//! structural gate, driven through the built binary.
//!
//! The headline fixture builds a two-commit repo where HEAD introduces a
//! `*::handler::* -> *::Command::*` call/dataflow reachability path absent at BASE;
//! the gate must flag it, exit nonzero, and name the introducing commit. A second
//! fixture proves the hard anchor guard: an unanchored `--path-added` is a usage
//! error (exit 2), never an unanchored walk.
//!
//! The `--format` guard rides on the same fixture at the bottom of this file: no
//! `cgx diff` invocation may answer a non-human format request with human prose
//! and exit 0.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;

/// BASE: `handler::process` does NOT reach the exec sink.
const SRC_BASE: &str = "\
pub mod handler {
    pub fn process() {
        let _ = 1 + 1;
    }
}
pub mod sys {
    pub struct Command;
    impl Command {
        pub fn run() {}
    }
}
";

/// HEAD: `handler::process` now calls `sys::Command::run` — a brand-new
/// handler→exec path.
const SRC_HEAD: &str = "\
pub mod handler {
    pub fn process() {
        crate::sys::Command::run();
    }
}
pub mod sys {
    pub struct Command;
    impl Command {
        pub fn run() {}
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

fn commit(repo: &Path, msg: &str, date: &str) {
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
            &format!("--date={date}"),
        ],
    );
}

/// A two-commit repo: BASE has no handler→exec path, HEAD introduces it.
fn gate_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    write(
        &repo,
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "cgx-test"]);
    git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);

    write(&repo, "src/lib.rs", SRC_BASE);
    commit(&repo, "base", "2020-01-01T00:00:00Z");

    write(&repo, "src/lib.rs", SRC_HEAD);
    commit(&repo, "head: add handler->exec path", "2020-02-01T00:00:00Z");

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

#[test]
fn path_added_flags_new_handler_to_exec_path_with_nonzero_exit_and_commit() {
    let (_tmp, repo) = gate_fixture();
    let (stdout, _stderr, code) = run_cgx(
        &repo,
        &[
            "diff",
            "HEAD~1",
            "HEAD",
            "--path-added",
            "--from",
            "**::handler::*",
            "--to",
            "**::Command::*",
            "--format",
            "json",
        ],
    );

    assert_eq!(code, 1, "a forbidden new path exits 1: stdout={stdout}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let paths = doc["added_paths"].as_array().expect("added_paths array");
    assert_eq!(paths.len(), 1, "exactly one new path: {stdout}");
    let p = &paths[0];
    assert!(
        p["from"].as_str().unwrap().ends_with("::process"),
        "source is the handler: {p}"
    );
    assert!(
        p["to"].as_str().unwrap().contains("Command"),
        "sink is the exec boundary: {p}"
    );
    assert!(
        p["introducing_commit"].as_str().is_some(),
        "the introducing commit is attributed: {p}"
    );
    assert_eq!(
        doc["semantics"].as_str().unwrap(),
        "call/dataflow reachability (not a soundness/security guarantee)",
        "the honesty framing is present in the output"
    );
}

#[test]
fn path_added_is_clean_when_no_new_path() {
    let (_tmp, repo) = gate_fixture();
    // Both anchors match real nodes, but the reverse direction (Command -> handler)
    // is unreachable on both sides → genuinely no new path → clean exit 0 with no
    // zero-match warning.
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "diff",
            "HEAD~1",
            "HEAD",
            "--path-added",
            "--from",
            "**::Command::*",
            "--to",
            "**::handler::*",
        ],
    );
    assert_eq!(code, 0, "no new path exits 0: stdout={stdout}");
    assert!(
        stdout.contains("no new call/dataflow reachability path"),
        "clean message present: {stdout}"
    );
    assert!(
        !stderr.contains("matched 0 nodes"),
        "no zero-match warning when both anchors match: {stderr}"
    );
}

/// A zero-match anchor must NOT silently pass as "clean": by default the gate keeps
/// exit 0 (open-world) but emits a stderr warning that the symbol is not indexed.
/// Regression for the RFC §5.3 cardinal-rule honesty bug.
#[test]
fn zero_match_anchor_warns_but_stays_exit_0_by_default() {
    let (_tmp, repo) = gate_fixture();
    let (stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "diff",
            "HEAD~1",
            "HEAD",
            "--path-added",
            "--from",
            "**::handler::*",
            "--to",
            "**::NoSuchSink::*",
        ],
    );
    assert_eq!(code, 0, "open-world default keeps exit 0: stdout={stdout}");
    assert!(
        stderr.contains("--to") && stderr.contains("matched 0 nodes"),
        "the zero-match warning names the anchor and the cause: {stderr}"
    );
    assert!(
        stderr.contains("NOT that no path exists"),
        "the warning states a clean result does not prove no path: {stderr}"
    );
}

/// `--require-anchor-match` turns a zero-match anchor into a hard error (exit 2) so
/// CI can fail closed when a configured sink isn't in the graph.
#[test]
fn require_anchor_match_turns_zero_match_into_exit_2() {
    let (_tmp, repo) = gate_fixture();
    let (_stdout, stderr, code) = run_cgx(
        &repo,
        &[
            "diff",
            "HEAD~1",
            "HEAD",
            "--path-added",
            "--require-anchor-match",
            "--from",
            "**::handler::*",
            "--to",
            "**::NoSuchSink::*",
        ],
    );
    assert_eq!(code, 2, "a zero-match anchor under --require-anchor-match is a usage error");
    assert!(
        stderr.contains("matched 0 nodes"),
        "the error names the zero-match cause: {stderr}"
    );
}

#[test]
fn unanchored_path_added_is_rejected_at_cli() {
    let (_tmp, repo) = gate_fixture();

    // Missing --to.
    let (_o, stderr, code) = run_cgx(
        &repo,
        &["diff", "HEAD~1", "HEAD", "--path-added", "--from", "**::handler::*"],
    );
    assert_eq!(code, 2, "missing --to is a usage error (exit 2)");
    assert!(
        stderr.contains("requires both --from and --to"),
        "the guard names the requirement: {stderr}"
    );

    // Missing --from.
    let (_o2, stderr2, code2) = run_cgx(
        &repo,
        &["diff", "HEAD~1", "HEAD", "--path-added", "--to", "**::Command::*"],
    );
    assert_eq!(code2, 2, "missing --from is a usage error (exit 2)");
    assert!(stderr2.contains("requires both --from and --to"));

    // Both missing.
    let (_o3, _stderr3, code3) = run_cgx(&repo, &["diff", "HEAD~1", "HEAD", "--path-added"]);
    assert_eq!(code3, 2, "fully unanchored is a usage error (exit 2)");
}

#[test]
fn newer_than_is_sugar_for_added_bucket() {
    let (_tmp, repo) = gate_fixture();

    let (newer, _e1, c1) = run_cgx(
        &repo,
        &["diff", "HEAD~1", "HEAD", "--newer-than", "--format", "json"],
    );
    assert_eq!(c1, 0);
    let (added, _e2, c2) = run_cgx(
        &repo,
        &["diff", "HEAD~1", "HEAD", "--added", "--format", "json"],
    );
    assert_eq!(c2, 0);

    let nd: serde_json::Value = serde_json::from_str(&newer).expect("json");
    let ad: serde_json::Value = serde_json::from_str(&added).expect("json");
    assert_eq!(
        nd["added_edges"], ad["added_edges"],
        "--newer-than and --added select the same added-edge set"
    );
    // The added bucket carries the new handler->Command call edge.
    assert!(
        ad["added_edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["dst"].as_str().map(|s| s.contains("Command")).unwrap_or(false)),
        "the new handler->Command edge is in the added bucket: {added}"
    );
}

#[test]
fn from_glob_post_filter_narrows_added_edges() {
    let (_tmp, repo) = gate_fixture();
    let (out, _e, code) = run_cgx(
        &repo,
        &[
            "diff",
            "HEAD~1",
            "HEAD",
            "--added",
            "--from",
            "**::handler::*",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    let doc: serde_json::Value = serde_json::from_str(&out).expect("json");
    for e in doc["added_edges"].as_array().unwrap() {
        assert!(
            e["src"].as_str().unwrap().contains("::handler::"),
            "every surviving added edge has a handler source: {e}"
        );
    }
}

// --- `--format` guard -------------------------------------------------------
//
// Regression for the silent-fallthrough defect: `print_diff_full` and
// `print_path_added` were both `match format { Json => …, _ => <human> }`, so
// `sarif`/`dot`/`mermaid`/`d2` produced human prose on stdout with exit 0.

/// Substrings that appear only in the diff path's *human* renderings. Presence of
/// any of these in stdout means the human printer ran.
const HUMAN_MARKERS: &[&str] = &[
    "+ edge  ",
    "- edge  ",
    "~ edge  ",
    "+ node  ",
    "- node  ",
    "(no diff)",
    "new call/dataflow reachability path",
];

fn contains_human_prose(stdout: &str) -> bool {
    HUMAN_MARKERS.iter().any(|m| stdout.contains(m))
}

/// Argv for one diff mode, minus `--format`. The `--path-added` anchors point the
/// other way (`Command` -> `handler`), which is unreachable on both sides, so the
/// gate is clean and a *successful* run exits 0 — making "exit 0 plus human prose"
/// the thing a broken format arm would produce.
fn diff_argv(path_added: bool) -> Vec<&'static str> {
    let mut v = vec!["diff", "HEAD~1", "HEAD"];
    if path_added {
        v.extend(["--path-added", "--from", "**::Command::*", "--to", "**::handler::*"]);
    }
    v
}

/// The C2 check: every `Format` variant against every diff mode, asserting no run
/// both exits 0 and emits human prose for a non-human format.
///
/// The table is driven by `Format::value_variants()` — the enum itself — and the
/// per-format `match` below is exhaustive, so a seventh `Format` variant fails to
/// compile here until someone decides what the diff path does with it. That is
/// the property the old `_` catch-all lacked: it absorbed new variants silently.
#[test]
fn every_format_x_every_diff_mode_never_emits_human_prose_with_exit_0() {
    use cgx_cli::output::Format;
    use clap::ValueEnum;

    let (_tmp, repo) = gate_fixture();

    for &format in Format::value_variants() {
        let name = format
            .to_possible_value()
            .expect("clap value name")
            .get_name()
            .to_string();

        for path_added in [false, true] {
            let mut argv: Vec<&str> = diff_argv(path_added);
            argv.extend(["--format", name.as_str()]);
            let (stdout, stderr, code) = run_cgx(&repo, &argv);
            let ctx = format!("--format {name} (path_added={path_added})");

            match format {
                Format::Human => {
                    assert_eq!(code, 0, "{ctx}: human is supported: {stdout}{stderr}");
                    assert!(contains_human_prose(&stdout), "{ctx}: human prose: {stdout}");
                }
                Format::Json => {
                    assert_eq!(code, 0, "{ctx}: json is supported: {stdout}{stderr}");
                    serde_json::from_str::<serde_json::Value>(&stdout)
                        .unwrap_or_else(|e| panic!("{ctx}: stdout is JSON ({e}): {stdout}"));
                    assert!(!contains_human_prose(&stdout), "{ctx}: no human prose: {stdout}");
                }
                Format::Sarif | Format::Dot | Format::Mermaid | Format::D2 => {
                    assert_eq!(code, 2, "{ctx}: unimplemented format is a usage error: {stdout}");
                    assert!(
                        stderr.contains(&name),
                        "{ctx}: the error names the requested format: {stderr}"
                    );
                    assert!(
                        stderr.contains("use --format human|json"),
                        "{ctx}: the error says what is supported: {stderr}"
                    );
                    assert!(
                        !contains_human_prose(&stdout),
                        "{ctx}: no human prose on stdout: {stdout}"
                    );
                }
            }
        }
    }
}

/// `sarif` gets its own regression because it is the damaging half: a CI pipeline
/// consuming `cgx diff --format sarif` used to receive unparseable prose and exit
/// 0. Diff-mode SARIF is *rejected*, not produced — `AddedPath` carries no file or
/// line, so a SARIF run would have no `physicalLocation` and would ingest cleanly
/// while reporting nothing actionable.
#[test]
fn sarif_is_rejected_loudly_by_both_diff_modes() {
    let (_tmp, repo) = gate_fixture();

    for (path_added, mode) in [(false, "cgx diff"), (true, "cgx diff --path-added")] {
        let mut argv: Vec<&str> = diff_argv(path_added);
        argv.extend(["--format", "sarif"]);
        let (stdout, stderr, code) = run_cgx(&repo, &argv);

        let expected =
            format!("sarif format is not supported by `{mode}` — use --format human|json");
        assert_eq!(code, 2, "{mode}: sarif is a usage error, not exit 0: {stdout}");
        assert!(stdout.is_empty(), "{mode}: nothing on stdout: {stdout}");
        assert!(
            stderr.contains(&expected),
            "{mode}: expected {expected:?} on stderr, got: {stderr}"
        );
    }
}

/// The rejection lands before any indexing work: a bad `--format` must not cost
/// two graph builds first. `.cgx/` is created by `run_diff` immediately before
/// indexing, so its absence proves the guard ran first.
#[test]
fn format_rejection_happens_before_indexing() {
    let (_tmp, repo) = gate_fixture();
    let mut argv: Vec<&str> = diff_argv(false);
    argv.extend(["--format", "dot"]);
    let (_stdout, _stderr, code) = run_cgx(&repo, &argv);

    assert_eq!(code, 2);
    assert!(
        !repo.join(".cgx").exists(),
        "the guard rejects before `.cgx/` is created and the refs are indexed"
    );
}
