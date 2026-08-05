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

/// A TypeScript two-commit repo whose method FQN is deliberately hostile to all
/// three graph grammars.
///
/// TypeScript is the cheap route to such an FQN and the only one among the shipped
/// adapters: `handle_method_def` copies the *raw source text* of a property name
/// into the FQN (`cgx-lang-ts/src/extract.rs`), so a string-literal method name
/// carries its own quotes, backslashes and everything between them through to the
/// graph. Rust cannot do this — it drops generic arguments, so `Wrap<T>` indexes as
/// `Wrap` — and no other adapter offers a shorter path.
///
/// The name below carries `"`, `\`, `<`, `>`, `&`, `|`, `#`, `;`, a backtick and
/// non-ASCII. A literal newline is *not* reachable through this route: `\n` in a TS
/// string literal is an escape sequence, so the raw text carries a backslash and an
/// `n`, never a line break.
const TS_HOSTILE_BASE: &str = r##"export function sink(): void {}
export class Handler {
  "q\"<>&|#;`\\π日"(): void {
  }
}
"##;

/// HEAD: the hostile-named method now reaches `sink`.
const TS_HOSTILE_HEAD: &str = r##"export function sink(): void {}
export class Handler {
  "q\"<>&|#;`\\π日"(): void {
    sink();
  }
}
"##;

fn hostile_fqn_fixture() -> (tempfile::TempDir, PathBuf) {
    two_commit_repo(&[("a.ts", TS_HOSTILE_BASE)], &[("a.ts", TS_HOSTILE_HEAD)])
}

/// `--path-added` argv for [`hostile_fqn_fixture`].
fn hostile_argv(format: &'static str) -> Vec<&'static str> {
    vec![
        "diff",
        "HEAD~1",
        "HEAD",
        "--path-added",
        "--from",
        "**Handler::*",
        "--to",
        "**::sink",
        "--format",
        format,
    ]
}

/// BASE for [`multi_path_fixture`]: three handlers, three exec methods, no edges.
const SRC_MULTI_BASE: &str = "\
pub mod handler {
    pub fn alpha() {}
    pub fn beta() {}
    pub fn gamma() {}
}
pub mod sys {
    pub struct Command;
    impl Command {
        pub fn run() {}
        pub fn spawn() {}
        pub fn exec() {}
    }
}
";

/// HEAD for [`multi_path_fixture`]: seven distinct handler→exec pairs appear at
/// once, so the `(from_fqn, to_fqn)` ordering step is exercised with seven
/// elements rather than one.
const SRC_MULTI_HEAD: &str = "\
pub mod handler {
    pub fn alpha() {
        crate::sys::Command::run();
        crate::sys::Command::spawn();
        crate::sys::Command::exec();
    }
    pub fn beta() {
        crate::sys::Command::run();
        crate::sys::Command::exec();
    }
    pub fn gamma() {
        crate::sys::Command::spawn();
        crate::sys::Command::run();
    }
}
pub mod sys {
    pub struct Command;
    impl Command {
        pub fn run() {}
        pub fn spawn() {}
        pub fn exec() {}
    }
}
";

/// A two-commit repo whose HEAD introduces *seven* added paths.
///
/// `gate_fixture` yields exactly one, which leaves `added_paths.sort_by` — the
/// single ordering step the whole AR-10 determinism argument rests on — never
/// exercised with more than one element, and makes a nondeterministic emitter fail
/// only about half the time over a two-node witness.
fn multi_path_fixture() -> (tempfile::TempDir, PathBuf) {
    two_commit_repo(
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("src/lib.rs", SRC_MULTI_BASE),
        ],
        &[("src/lib.rs", SRC_MULTI_HEAD)],
    )
}

/// Two commits from two file sets: `base` is committed first, then `head` is
/// written over it and committed.
fn two_commit_repo(base: &[(&str, &str)], head: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "cgx-test"]);
    git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);

    for (rel, text) in base {
        write(&repo, rel, text);
    }
    commit(&repo, "base", "2020-01-01T00:00:00Z");

    for (rel, text) in head {
        write(&repo, rel, text);
    }
    commit(&repo, "head", "2020-02-01T00:00:00Z");

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

/// The `--format` values each diff mode supports, as the rejection message spells
/// them. `--path-added` is path-shaped, so it also implements the three path-graph
/// emitters; full `cgx diff` is an edge/node bucket set and does not.
fn supported_formats(path_added: bool) -> &'static str {
    if path_added {
        "human|json|dot|mermaid|d2"
    } else {
        "human|json"
    }
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
                // `--path-added` implements the path-graph emitters. `diff_argv`
                // picks the unreachable direction, so the gate is clean and the
                // graph is empty — but it must still be *valid, empty* graph source,
                // never prose and never a stray marker.
                Format::Dot | Format::Mermaid | Format::D2 if path_added => {
                    assert_eq!(code, 0, "{ctx}: path-graph format is supported: {stdout}{stderr}");
                    assert!(!contains_human_prose(&stdout), "{ctx}: no human prose: {stdout}");
                    let g = parse_graph(format, &stdout);
                    assert!(
                        g.nodes.is_empty() && g.edges.is_empty(),
                        "{ctx}: a clean gate emits an empty graph: {g:?}"
                    );
                    // `nodes.is_empty() && edges.is_empty()` is satisfied by zero
                    // bytes, so on its own it cannot tell an empty *document* from
                    // no document at all. Each format's empty rendering is
                    // therefore pinned exactly. D2's genuinely is zero bytes — the
                    // real `d2` compiler accepts an empty file — so that arm
                    // asserts emptiness *specifically* rather than by falling
                    // through a shared weak check, and D2's emitter is pinned for
                    // non-empty input by the exact-node/edge test below.
                    let empty_source = match format {
                        Format::Dot => "digraph cgx {\n  rankdir=LR;\n}\n",
                        Format::Mermaid => "graph TD\n",
                        Format::D2 => "",
                        other => unreachable!("{other:?} is not a path-graph format"),
                    };
                    assert_eq!(
                        stdout, empty_source,
                        "{ctx}: the empty rendering for this format is exact"
                    );
                }
                Format::Sarif | Format::Dot | Format::Mermaid | Format::D2 => {
                    assert_eq!(code, 2, "{ctx}: unimplemented format is a usage error: {stdout}");
                    assert!(
                        stderr.contains(&name),
                        "{ctx}: the error names the requested format: {stderr}"
                    );
                    assert!(
                        stderr.contains(&format!("use --format {}", supported_formats(path_added))),
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

        let expected = format!(
            "sarif format is not supported by `{mode}` — use --format {}",
            supported_formats(path_added)
        );
        assert_eq!(code, 2, "{mode}: sarif is a usage error, not exit 0: {stdout}");
        assert!(stdout.is_empty(), "{mode}: nothing on stdout: {stdout}");
        assert!(
            stderr.contains(&expected),
            "{mode}: expected {expected:?} on stderr, got: {stderr}"
        );
    }
}

/// The plain `cgx diff` mode is not path-shaped, so it keeps rejecting all three
/// path-graph formats even though `--path-added` now implements them. The
/// narrowing is per-mode, not global.
#[test]
fn path_graph_formats_stay_rejected_by_plain_diff() {
    let (_tmp, repo) = gate_fixture();

    for fmt in ["dot", "mermaid", "d2"] {
        let mut argv: Vec<&str> = diff_argv(false);
        argv.extend(["--format", fmt]);
        let (stdout, stderr, code) = run_cgx(&repo, &argv);

        let expected =
            format!("{fmt} format is not supported by `cgx diff` — use --format human|json");
        assert_eq!(code, 2, "{fmt}: plain diff rejects it: {stdout}");
        assert!(stdout.is_empty(), "{fmt}: nothing on stdout: {stdout}");
        assert!(
            stderr.contains(&expected),
            "{fmt}: expected {expected:?} on stderr, got: {stderr}"
        );
    }
}

// --- `--path-added` graph output (dot/mermaid/d2) ---------------------------
//
// The added-path witnesses render through the same emitters `cgx paths` uses.
// These tests parse the emitted source back into (nodes, edges) and assert
// structural validity plus correspondence to the fixture's actual added path —
// no eyeballing, no golden string, no external toolchain.

/// A graph parsed back out of emitted source: `(node_id, label)` declarations in
/// emission order and `(src_id, dst_id)` edges in emission order.
#[derive(Debug, PartialEq, Eq)]
struct ParsedGraph {
    nodes: Vec<(String, String)>,
    edges: Vec<(String, String)>,
}

/// Parse graph source for `format`, asserting the format's structural rules on the
/// way, then the rules common to all three (ids are `n0..nk` in declaration order;
/// every edge endpoint is a declared id).
fn parse_graph(format: cgx_cli::output::Format, src: &str) -> ParsedGraph {
    use cgx_cli::output::Format;
    let g = match format {
        Format::Dot => parse_dot(src),
        Format::Mermaid => parse_mermaid(src),
        Format::D2 => parse_d2(src),
        other => panic!("{other:?} is not a path-graph format"),
    };

    for (i, (id, _)) in g.nodes.iter().enumerate() {
        assert_eq!(
            id,
            &format!("n{i}"),
            "{format:?}: node ids are n0..nk in order: {src}"
        );
    }
    let ids: Vec<&str> = g.nodes.iter().map(|(id, _)| id.as_str()).collect();
    for (s, d) in &g.edges {
        assert!(
            ids.contains(&s.as_str()),
            "{format:?}: edge source {s} is declared: {src}"
        );
        assert!(
            ids.contains(&d.as_str()),
            "{format:?}: edge target {d} is declared: {src}"
        );
    }
    g
}

/// Decode the backslash escapes a **D2** double-quoted string uses, and assert on
/// the way that the body is a well-formed one: a bare `"` inside it would have closed
/// the string early, and a trailing `\` would escape the closing quote.
///
/// D2-only. DOT used to share this decoder and must not: Graphviz *also* decodes
/// character references in ordinary labels, so a DOT label checked with this function
/// passes while rendering as different text — see [`dot_unescape`]. d2 0.7.1 does not
/// decode them (measured: a label written `A&quot;B` displays `A&quot;B`), so for D2
/// backslashes really are the whole decoder.
fn backslash_unescape(body: &str, ctx: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(next) => out.push(next),
                None => panic!("{ctx}: label body ends in a dangling backslash: {body:?}"),
            },
            '"' => panic!("{ctx}: label body carries an unescaped quote: {body:?}"),
            _ => out.push(c),
        }
    }
    out
}

/// The entity names Graphviz decodes that this oracle needs to know about. Partial
/// on purpose: [`assert_dot_label_inert`] rejects any emitted `&` that does not open
/// `&amp;`, so a name Graphviz knows and this list does not can never reach here.
const DOT_ENTITIES: &[(&str, char)] = &[
    ("&amp;", '&'),
    ("&quot;", '"'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&nbsp;", '\u{a0}'),
];

/// The renderability half of the DOT contract, derived from the Graphviz grammar
/// rather than from the emitter's table.
///
/// Graphviz decodes character references in ordinary labels — measured against
/// Graphviz 15.1.0, `label="A&quot;B"` renders `A"B` — so an emitted `&` that opens
/// anything but the emitter's own `&amp;` is a silent-corruption bug even though the
/// source parses cleanly. Backslash escapes get the same treatment: only `\\` and
/// `\"` are legal here, because a lone `\` reaches Graphviz's `\n`/`\l`/`\N`
/// processing.
fn assert_dot_label_inert(body: &str, ctx: &str) {
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'&' => {
                assert!(
                    body[i..].starts_with("&amp;"),
                    "{ctx}: emitted a `&` that does not open `&amp;`; Graphviz decodes \
                     character references in ordinary labels: {body:?}"
                );
                i += 5;
            }
            b'\\' => {
                let next = b.get(i + 1);
                assert!(
                    next == Some(&b'\\') || next == Some(&b'"'),
                    "{ctx}: emitted a `\\` that opens neither `\\\\` nor `\\\"`; \
                     Graphviz would read it as an escape sequence: {body:?}"
                );
                i += 2;
            }
            b'"' => panic!("{ctx}: label body carries an unescaped quote: {body:?}"),
            _ => i += 1,
        }
    }
}

/// Decode a DOT label body the way Graphviz does: character references first, then
/// backslash escapes.
///
/// That order is Graphviz's and it matters — a reference decoding to a backslash is
/// handed to the escape stage as a live escape (`label="&#92;n"` renders as a line
/// break). Asserting inertness first is what makes the partial [`DOT_ENTITIES`] table
/// sound.
fn dot_unescape(body: &str, ctx: &str) -> String {
    assert_dot_label_inert(body, ctx);
    let mut entities = String::with_capacity(body.len());
    let mut rest = body;
    'outer: while let Some(amp) = rest.find('&') {
        entities.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        for (name, c) in DOT_ENTITIES {
            if let Some(stripped) = tail.strip_prefix(name) {
                entities.push(*c);
                rest = stripped;
                continue 'outer;
            }
        }
        panic!("{ctx}: emitted a `&` opening no entity this oracle knows: {body:?}");
    }
    entities.push_str(rest);
    backslash_unescape(&entities, ctx)
}

/// Graphviz: a `digraph` header, one matching brace pair, one `label=` per node,
/// and `->` edges.
fn parse_dot(src: &str) -> ParsedGraph {
    let lines: Vec<&str> = src.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some("digraph cgx {"),
        "dot header: {src}"
    );
    assert_eq!(
        lines.last().copied(),
        Some("}"),
        "dot closes its block: {src}"
    );
    assert_eq!(
        src.matches('{').count(),
        1,
        "exactly one opening brace: {src}"
    );
    assert_eq!(
        src.matches('}').count(),
        1,
        "exactly one closing brace: {src}"
    );

    let mut g = ParsedGraph {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    for line in &lines[1..lines.len() - 1] {
        let t = line.trim();
        if t == "rankdir=LR;" {
            continue;
        }
        // Edges first: a labeled edge would otherwise parse as a node declaration.
        if let Some((s, rest)) = t.split_once(" -> ") {
            let d = rest
                .strip_suffix(';')
                .unwrap_or_else(|| panic!("dot edge ends in ;: {t}"));
            assert!(
                !d.contains("[label="),
                "path-added edges carry no label (D3): {t}"
            );
            g.edges.push((s.to_string(), d.to_string()));
        } else if let Some((id, rest)) = t.split_once(" [label=\"") {
            let body = rest
                .strip_suffix("\"];")
                .expect("dot node line ends [label=\"…\"];");
            g.nodes
                .push((id.to_string(), dot_unescape(body, "dot node label")));
        } else {
            panic!("dot line is neither a node declaration nor an edge: {t}");
        }
    }
    assert_eq!(
        src.matches("label=").count(),
        g.nodes.len(),
        "exactly one label= per node, and none anywhere else: {src}"
    );
    g
}

/// The emitter's entity table, restated here as the oracle's own inverse rather
/// than imported: the two are meant to be checked against each other.
const MERMAID_ENTITIES: &[(&str, char)] = &[
    ("&amp;", '&'),
    ("&quot;", '"'),
    ("&lt;", '<'),
    ("&grave;", '`'),
    ("&bsol;", '\\'),
    ("&semi;", ';'),
    ("&num;", '#'),
    ("&verbar;", '|'),
    ("&lsqb;", '['),
    ("&rsqb;", ']'),
    ("&lpar;", '('),
    ("&rpar;", ')'),
    ("&lcub;", '{'),
    ("&rcub;", '}'),
];

fn mermaid_entity_at(tail: &str) -> Option<(&'static str, char)> {
    let end = tail.find(';')?;
    MERMAID_ENTITIES
        .iter()
        .find(|(e, _)| *e == &tail[..=end])
        .copied()
}

/// Characters the real Mermaid pipeline *acts on* rather than prints when they
/// appear raw in a label — established against mermaid-cli 11.16.0, not by
/// reasoning. A raw `"` is a parse error and no diagram is produced at all; a raw
/// `<` is swallowed as an HTML tag (`List<String>` renders as `List`); a raw
/// backtick opens a markdown code span. A raw `\` renders fine, but Mermaid has no
/// backslash escape, so its presence means someone reached for a Graphviz/D2
/// escaper. A raw `&` is the introducer of the encoding itself — `A&quot;B` renders
/// `A"B`, and `A&ampB` renders `A&B` even with no `;`, so a raw `&` lets an FQN's
/// own text be read as an entity. A raw `;` is what lets a raw `#` form a `#\w+;`
/// code.
const MERMAID_LABEL_HOSTILE: &[char] = &['&', '"', '<', '`', '\\', ';'];

/// Assert a Mermaid label body carries nothing the pipeline would act on.
///
/// Derived from the grammar and from the two source rewrites in `encodeEntities`
/// (`mermaid/dist/chunks/mermaid.esm/chunk-MMGVDTGO.mjs`), so it fails on emitter
/// output no real parser would render faithfully — not merely on output that fails
/// to parse. The three clauses are independent: hostile characters outside an
/// entity, the `#\w+;` rewrite, and the `style` / `classDef` rewrite.
fn assert_mermaid_label_inert(body: &str, ctx: &str) {
    // Everything that is not part of an entity the emitter produced. A `&` that
    // opens no known entity survives into the residue and trips the loop below.
    let mut residue = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(amp) = rest.find('&') {
        residue.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        match mermaid_entity_at(tail) {
            Some((entity, _)) => rest = &tail[entity.len()..],
            None => {
                residue.push('&');
                rest = &tail[1..];
            }
        }
    }
    residue.push_str(rest);

    for &c in MERMAID_LABEL_HOSTILE {
        assert!(
            !residue.contains(c),
            "{ctx}: mermaid label carries a raw {c:?}, which the Mermaid pipeline acts on \
             rather than prints — it must be a named character reference: {body:?}"
        );
    }

    let b = body.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c != b'#' {
            continue;
        }
        let mut j = i + 1;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
            j += 1;
        }
        assert!(
            !(j > i + 1 && j < b.len() && b[j] == b';'),
            "{ctx}: mermaid label carries `#\\w+;`, which Mermaid's source rewrite decodes \
             (`#quot;` → `\"`, `#1;` → U+0001): {body:?}"
        );
    }

    // `/(?:style|classDef).*:\S*#.*;/` — the rewrite that replaces its match with
    // the match minus its final character, silently deleting a `;` an entity needed.
    for kw in ["style", "classDef"] {
        let (Some(at), Some(last_semi)) = (body.find(kw), body.rfind(';')) else {
            continue;
        };
        for colon in (at + kw.len())..b.len() {
            if b[colon] != b':' {
                continue;
            }
            for (hash, &c) in b.iter().enumerate().skip(colon + 1) {
                if c.is_ascii_whitespace() {
                    break;
                }
                if c == b'#' {
                    assert!(
                        hash >= last_semi,
                        "{ctx}: mermaid's {kw} source rewrite would strip the final `;` \
                         off this label: {body:?}"
                    );
                    break;
                }
            }
        }
    }
}

/// Decode the named character references the emitter uses, so a parsed label can be
/// compared against the FQN it was built from rather than against the escaping.
///
/// The exact inverse of the emitter, and unambiguous for the same structural reason
/// the emitter is injective: `&` is the only introducer and the emitter escapes it,
/// so every `&` here opens an entity the table below knows, and every entity
/// terminates at its own `;`. Anything else is an emitter bug and panics rather
/// than decoding to something plausible.
fn mermaid_unescape(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let (entity, c) = mermaid_entity_at(tail).unwrap_or_else(|| {
            panic!("mermaid label carries a `&` that opens no known entity: {body:?}")
        });
        out.push(c);
        rest = &tail[entity.len()..];
    }
    out.push_str(rest);
    out
}

/// Mermaid: a `graph TD` header, `id["label"]` nodes and `-->` edges.
fn parse_mermaid(src: &str) -> ParsedGraph {
    let lines: Vec<&str> = src.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some("graph TD"),
        "mermaid header: {src}"
    );

    let mut g = ParsedGraph {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    for line in &lines[1..] {
        let t = line.trim();
        // A labeled mermaid edge is `a -->|label| b`, which fails this split — so
        // the panic below is the D3 check as well as the syntax check.
        if t.contains("-->") {
            let (s, d) = t
                .split_once(" --> ")
                .unwrap_or_else(|| panic!("mermaid edge is unlabeled `a --> b` (D3): {t}"));
            g.edges.push((s.to_string(), d.to_string()));
        } else if let Some((id, rest)) = t.split_once("[\"") {
            let body = rest
                .strip_suffix("\"]")
                .unwrap_or_else(|| panic!("mermaid node line ends \"]: {t}"));
            assert_mermaid_label_inert(body, "node label");
            g.nodes.push((id.to_string(), mermaid_unescape(body)));
        } else {
            panic!("mermaid line is neither a node declaration nor an edge: {t}");
        }
    }
    g
}

/// D2: `id: "label"` declarations and `->` edges. Unlabeled edges carry no `:`.
fn parse_d2(src: &str) -> ParsedGraph {
    let mut g = ParsedGraph {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some((s, d)) = t.split_once(" -> ") {
            assert!(
                !d.contains(':'),
                "path-added edges carry no label (D3): {t}"
            );
            g.edges.push((s.to_string(), d.to_string()));
        } else {
            let (id, rest) = t
                .split_once(": \"")
                .unwrap_or_else(|| panic!("d2 declaration: {t}"));
            let body = rest
                .strip_suffix('"')
                .expect("d2 declaration ends in a quote");
            g.nodes
                .push((id.to_string(), backslash_unescape(body, "d2 node label")));
        }
    }
    g
}

/// The added path this fixture introduces, read out of the JSON mode of the *same*
/// command, so the graph assertions compare against what the gate actually found
/// rather than a hand-copied golden.
fn added_path_via(repo: &Path, json_argv: &[&str]) -> Vec<String> {
    let (stdout, _e, code) = run_cgx(repo, json_argv);
    assert_eq!(code, 1, "the fixture introduces one new path: {stdout}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let paths = doc["added_paths"].as_array().expect("added_paths array");
    assert_eq!(paths.len(), 1, "exactly one added path: {stdout}");
    paths[0]["via"]
        .as_array()
        .expect("via array")
        .iter()
        .map(|v| v.as_str().expect("via entry is a string").to_string())
        .collect()
}

/// `--path-added` argv in the direction that *does* find a new path, so the graph
/// under test is non-empty.
fn path_graph_argv(format: &'static str) -> Vec<&'static str> {
    vec![
        "diff",
        "HEAD~1",
        "HEAD",
        "--path-added",
        "--from",
        "**::handler::*",
        "--to",
        "**::Command::*",
        "--format",
        format,
    ]
}

/// C3: each path-graph format emits structurally valid source whose node and edge
/// set is exactly the fixture's added path — node labels are the FQNs (D2: no
/// `file:line` anchors) and edges are unlabeled (D3).
#[test]
fn path_added_emits_valid_graph_source_for_every_path_graph_format() {
    use cgx_cli::output::Format;

    let (_tmp, repo) = gate_fixture();
    let via = added_path_via(&repo, &path_graph_argv("json"));
    assert!(
        via.len() >= 2,
        "the witness spans at least source and sink: {via:?}"
    );

    let expected_nodes: Vec<(String, String)> = via
        .iter()
        .enumerate()
        .map(|(i, fqn)| (format!("n{i}"), fqn.clone()))
        .collect();
    let expected_edges: Vec<(String, String)> = (0..via.len() - 1)
        .map(|i| (format!("n{i}"), format!("n{}", i + 1)))
        .collect();

    for (name, format) in [
        ("dot", Format::Dot),
        ("mermaid", Format::Mermaid),
        ("d2", Format::D2),
    ] {
        let (stdout, _stderr, code) = run_cgx(&repo, &path_graph_argv(name));
        assert_eq!(
            code, 1,
            "{name}: the gate still fires on a new path: {stdout}"
        );
        assert!(
            !contains_human_prose(&stdout),
            "{name}: graph source only, no prose: {stdout}"
        );

        let g = parse_graph(format, &stdout);
        assert_eq!(
            g.nodes, expected_nodes,
            "{name}: nodes are the witness FQNs: {stdout}"
        );
        assert_eq!(
            g.edges, expected_edges,
            "{name}: edges follow the witness: {stdout}"
        );
    }
}

/// C3 again, with the *emitter's own assumptions removed from the oracle*.
///
/// Every other fixture in this file uses plain Rust identifiers, so nothing here
/// ever exercised an FQN carrying a character that any of the three grammars treats
/// as syntax — and the Mermaid oracle, which mirrored the emitter rather than the
/// grammar, accepted the exact byte string mermaid-cli rejects. This drives all
/// three formats over an FQN carrying `"`, `\`, `<`, `>`, `&`, `|`, `#`, `;`, a
/// backtick and non-ASCII, and asserts the labels decode back to the FQN the same
/// command reports in JSON.
///
/// The escapings this pins were verified against the real toolchains rather than
/// derived: mermaid-cli 11.16.0 renders the Mermaid to an SVG whose node label is
/// the FQN character for character, and d2 0.7.1 compiles the D2 to an SVG that
/// does the same. Before the fix, mermaid-cli returned
/// `Parse error on line 2 … got 'STR'` and produced no SVG at all.
#[test]
fn path_added_graph_source_survives_hostile_fqn_characters() {
    use cgx_cli::output::Format;

    let (_tmp, repo) = hostile_fqn_fixture();
    let via = added_path_via(&repo, &hostile_argv("json"));

    let hostile = via
        .iter()
        .find(|f| f.contains("Handler"))
        .unwrap_or_else(|| panic!("the fixture's hostile method is on the path: {via:?}"));
    for c in ['"', '\\', '<', '>', '&', '|', '#', ';', '`'] {
        assert!(
            hostile.contains(c),
            "the fixture FQN still carries {c:?} — it is the whole point of it: {hostile:?}"
        );
    }
    assert!(
        !hostile.is_ascii(),
        "the fixture FQN still carries non-ASCII: {hostile:?}"
    );

    let expected_nodes: Vec<(String, String)> = via
        .iter()
        .enumerate()
        .map(|(i, fqn)| (format!("n{i}"), fqn.clone()))
        .collect();

    for (name, format) in [
        ("dot", Format::Dot),
        ("mermaid", Format::Mermaid),
        ("d2", Format::D2),
    ] {
        let (stdout, _stderr, code) = run_cgx(&repo, &hostile_argv(name));
        assert_eq!(code, 1, "{name}: the gate fires on the new path: {stdout}");

        let g = parse_graph(format, &stdout);
        assert_eq!(
            g.nodes, expected_nodes,
            "{name}: labels decode back to the witness FQNs: {stdout}"
        );
    }
}

/// C4: the same command run twice against the same index produces byte-identical
/// stdout. This is the AR-10 determinism claim, and it is deliberately a
/// repeat-run comparison rather than a golden-string check — a golden would pass
/// on a renderer that happened to be stable for one input and not for the order it
/// derives from.
///
/// It runs over [`multi_path_fixture`] rather than `gate_fixture` because the
/// latter yields exactly one added path with a two-node witness: `added_paths
/// .sort_by` on `(from_fqn, to_fqn)` — the one ordering step the whole determinism
/// argument rests on — would never see more than one element, and an emitter that
/// walked a `HashSet` would fail only about half the time over two nodes. Each
/// comparison is a genuinely fresh process, which is what puts hash-seed variation
/// in scope at all; a same-process comparison could not catch it.
#[test]
fn path_added_graph_output_is_byte_identical_across_runs() {
    let (_tmp, repo) = multi_path_fixture();

    let witnesses = run_cgx(&repo, &multi_path_argv("json"));
    let doc: serde_json::Value = serde_json::from_str(&witnesses.0).expect("json");
    let count = doc["added_paths"].as_array().expect("added_paths").len();
    assert!(
        count >= 2,
        "the fixture must exercise the ordering step with more than one element, got {count}"
    );

    for name in ["dot", "mermaid", "d2"] {
        let argv = multi_path_argv(name);
        let (first, _e1, c1) = run_cgx(&repo, &argv);
        assert_eq!(c1, 1, "{name}: first run fires the gate: {first}");
        assert!(!first.is_empty(), "{name}: the run emitted graph source");
        // Three fresh processes, not two: a one-in-N ordering flake over seven
        // paths should not need luck to show up.
        for round in 1..=2 {
            let (again, _e, c) = run_cgx(&repo, &argv);
            assert_eq!(c, c1, "{name}: run {round} agrees on the exit code");
            assert_eq!(
                first.as_bytes(),
                again.as_bytes(),
                "{name}: run {round} over the same index is byte-identical"
            );
        }
    }
}

/// `--path-added` argv for [`multi_path_fixture`], in the direction that finds all
/// seven new paths.
fn multi_path_argv(format: &'static str) -> Vec<&'static str> {
    vec![
        "diff",
        "HEAD~1",
        "HEAD",
        "--path-added",
        "--from",
        "**::handler::*",
        "--to",
        "**::Command::*",
        "--format",
        format,
    ]
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
