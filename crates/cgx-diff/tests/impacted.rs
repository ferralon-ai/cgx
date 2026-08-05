//! `cgx-diff::impacted` end to end: two-graph acquisition, the changed-symbol
//! set, and the reverse walk over the *real* committed fixtures.
//!
//! The suite exists to prove three things a synthetic graph cannot: that two
//! graphs really do coexist in one store, that a **body-only** edit still yields
//! impacted tests, and that each supported adapter's test entrypoints are
//! actually reachable from a change.

mod common;

use std::path::{Path, PathBuf};

use cgx_diff::impacted::{run, Request, Sides};
use cgx_diff::{Answer, GraphDiff};
use cgx_index::{default_registry, index_path, index_workdir, IndexOpts, Repo};
use cgx_query::PathWalker;
use cgx_store::{FactStore, SqliteStore};

// --- the proof gate ----------------------------------------------------------

/// `index_path` (the committed HEAD tree) and `index_workdir` (the dirty working
/// directory) into **one** store must yield two independently readable graphs.
/// The key spaces differ — a real tree OID vs. `workdir:<digest>`
/// (`cgx-index/src/lib.rs:151-162`) — but until this test nobody had run it, and
/// the whole inner-loop mode rests on it.
#[test]
fn two_graphs_one_store_head_and_workdir() {
    let repo = common::TestRepo::init();
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn caller() -> i32 { add(1, 2) }\n",
    );
    repo.commit("base", "2020-01-01T00:00:00Z");

    let mut store = SqliteStore::open_in_memory().expect("store");
    let registry = default_registry();
    let opts = IndexOpts::default();

    let base = index_path(&repo.path, &registry, &mut store, &opts).expect("index head tree");

    // Dirty the working directory *without* committing.
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\
         pub fn caller() -> i32 { add(1, 2) }\n\
         pub fn extra() -> i32 { caller() }\n",
    );

    let git_repo = Repo::discover(&repo.path).expect("discover");
    let workdir = git_repo.workdir().expect("workdir").to_path_buf();
    let head =
        index_workdir(&repo.path, &workdir, &registry, &mut store, &opts).expect("index workdir");

    assert_ne!(base.graph_key, head.graph_key, "keys must not collide");
    assert!(head.graph_key.starts_with("workdir:"));
    assert_ne!(base.graph_id, head.graph_id, "two distinct graph ids");

    let base_graph = store.read_graph(base.graph_id).expect("read base");
    let head_graph = store.read_graph(head.graph_id).expect("read head");

    assert!(
        base_graph.nodes.iter().all(|n| !n.fqn.ends_with("::extra")),
        "base graph must not see the uncommitted symbol"
    );
    assert!(
        head_graph.nodes.iter().any(|n| n.fqn.ends_with("::extra")),
        "head graph must see the uncommitted symbol"
    );
}

// --- harness -----------------------------------------------------------------

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .canonicalize()
        .expect("fixtures/ must exist")
}

/// Copy a committed fixture tree into a throwaway git repo and commit it.
fn fixture_repo(fixture: &str) -> common::TestRepo {
    let repo = common::TestRepo::init();
    copy_into(&fixtures_root().join(fixture), &repo.path);
    repo.commit("fixture", "2020-01-01T00:00:00Z");
    repo
}

fn copy_into(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_into(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Replace `needle` with `replacement` in a file, asserting it was present — so
/// a fixture edit can never silently become a no-op.
fn edit(repo: &common::TestRepo, rel: &str, needle: &str, replacement: &str) {
    let path = repo.path.join(rel);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(needle), "{rel} does not contain {needle:?}");
    std::fs::write(&path, text.replace(needle, replacement)).unwrap();
}

/// Run the inner loop (committed HEAD tree vs. the dirty working directory).
fn run_workdir(repo: &common::TestRepo) -> (SqliteStore, Answer) {
    let mut store = SqliteStore::open_in_memory().expect("store");
    let answer = run(
        &mut store,
        Request {
            repo_root: &repo.path,
            sides: Sides::Workdir,
            walker: PathWalker::default(),
        },
    )
    .expect("impacted-tests");
    (store, answer)
}

fn reported(a: &Answer) -> Vec<&str> {
    a.tests.tests.iter().map(|r| r.node.fqn.as_str()).collect()
}

fn codes(a: &Answer) -> Vec<&str> {
    a.contract.reasons.iter().map(|r| r.code).collect()
}

// --- the body-only-edit regression ------------------------------------------

/// The single most important test in the feature. A body-only edit changes no
/// FQN and no call edge, so the graph diff is **empty** — and a changed set
/// derived from the diff alone would report no impacted tests, silently. The
/// file-granular half of the set is what makes the answer non-empty, and the
/// contract discloses the weaker claim on the input side.
#[test]
fn a_body_only_edit_yields_an_empty_graph_diff_but_a_non_empty_answer() {
    let repo = fixture_repo("go");
    edit(&repo, "dataflow.go", "c := b + 1", "c := b + 2");

    let (store, a) = run_workdir(&repo);

    let diff: GraphDiff =
        cgx_diff::diff_trees(&store, a.base_graph_id, a.head_graph_id).expect("diff");
    assert!(
        diff.is_empty(),
        "the hazard must be real for this test to mean anything: {diff:?}"
    );

    assert!(
        reported(&a).contains(&"go_sample::TestPlain"),
        "got {:?}",
        reported(&a)
    );
    assert!(
        codes(&a).contains(&"impacted-changed-set-file-granular"),
        "the weaker claim must be disclosed: {:?}",
        codes(&a)
    );
    assert!(a.tests.file_granular_only > 0);
    assert!(!a.degenerate);
}

// --- per-adapter fixtures (criterion 7) --------------------------------------

#[test]
fn go_fixture_reports_both_the_direct_test_and_the_lifted_subtest() {
    let repo = fixture_repo("go");
    edit(&repo, "dataflow.go", "c := b + 1", "c := b + 2");
    let (_store, a) = run_workdir(&repo);

    let names = reported(&a);
    assert!(names.contains(&"go_sample::TestPlain"), "got {names:?}");
    assert!(names.contains(&"go_sample::TestSub"), "got {names:?}");

    // TestSub has no call edge at all: extraction puts the `t.Run` body's call
    // on a Lambda node (`go_sample::TestSub::{func@…}`) and leaves TestSub
    // edgeless. Only the containment lift recovers it — and it says so.
    let i = names
        .iter()
        .position(|n| *n == "go_sample::TestSub")
        .unwrap();
    assert!(a.tests.witnesses[i].via_containment_lift);
    assert_eq!(
        a.tests.tests[i].min_confidence_on_path,
        cgx_core::Confidence::Possible
    );
    let j = names
        .iter()
        .position(|n| *n == "go_sample::TestPlain")
        .unwrap();
    assert!(!a.tests.witnesses[j].via_containment_lift);

    assert!(codes(&a).contains(&"impacted-closure-containment-lift"));
    assert!(codes(&a).contains(&"impacted-test-recognition-incomplete-go"));
}

#[test]
fn rust_fixture_reports_the_test_that_calls_the_change() {
    let repo = fixture_repo("rust-sample");
    edit(&repo, "src/impacted.rs", "seed * 2", "seed * 3");
    let (_store, a) = run_workdir(&repo);

    assert!(
        reported(&a).contains(&"rust_sample::impacted::tests::test_compute"),
        "got {:?}",
        reported(&a)
    );
    assert!(codes(&a).contains(&"impacted-test-recognition-incomplete-rust"));
}

#[test]
fn java_fixture_reports_the_annotated_test() {
    let repo = fixture_repo("java");
    edit(&repo, "Shapes.java", "return count;", "return count + 0;");
    let (_store, a) = run_workdir(&repo);

    assert!(
        reported(&a).contains(&"com::example::shapes::Tests::totalReportsTheSeededCount"),
        "got {:?}",
        reported(&a)
    );
    assert!(codes(&a).contains(&"impacted-test-recognition-incomplete-java"));
}

#[test]
fn python_fixture_reports_the_cross_module_test() {
    let repo = fixture_repo("python-app");
    edit(&repo, "app.py", "scaled = base * 2", "scaled = base * 3");
    let (_store, a) = run_workdir(&repo);

    assert!(
        reported(&a).contains(&"test_app::test_build_record"),
        "got {:?}",
        reported(&a)
    );
    assert!(codes(&a).contains(&"impacted-test-recognition-incomplete-python"));
}

// --- the degenerate answer ---------------------------------------------------

/// A changed set with no supported language must be *visibly* degenerate. The
/// CLI turns this into exit 4, MCP into a top-level flag. Neither is allowed to
/// be a bare empty set that reads as "nothing to run".
#[test]
fn a_typescript_only_change_is_degenerate_and_named() {
    let repo = common::TestRepo::init();
    repo.write("src/lib.rs", "pub fn keep() -> i32 { 1 }\n");
    repo.write(
        "web/app.ts",
        "export function greet(): string { return \"hi\"; }\n",
    );
    repo.commit("base", "2020-01-01T00:00:00Z");
    edit(&repo, "web/app.ts", "\"hi\"", "\"hello\"");

    let (_store, a) = run_workdir(&repo);

    assert!(a.tests.tests.is_empty());
    assert!(a.degenerate, "reason={:?}", a.degenerate_reason);
    let reason = a.degenerate_reason.as_deref().unwrap_or_default();
    assert!(reason.contains("typescript"), "reason={reason:?}");
    assert!(codes(&a).contains(&"impacted-language-unsupported"));
    assert_ne!(
        a.contract.direction,
        cgx_query::ApproxDirection::Exact,
        "a degenerate answer can never be exact"
    );
    assert!(
        a.contract.scope.is_some(),
        "an empty answer carries its negative scope"
    );
}

#[test]
fn an_unchanged_tree_is_empty_but_not_degenerate() {
    let repo = fixture_repo("go");
    let (_store, a) = run_workdir(&repo);
    assert_eq!(a.dirty_files, 0);
    assert!(a.tests.tests.is_empty());
    assert!(
        !a.degenerate,
        "nothing changed is a real answer, not a vacuous one"
    );
}

// --- CI mode -----------------------------------------------------------------

#[test]
fn two_committed_refs_walk_the_change_between_them() {
    let repo = common::TestRepo::init();
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\
         #[cfg(test)] mod tests {\n\
         use super::*;\n\
         #[test] fn test_add() { assert_eq!(add(1, 2), 3); }\n\
         }\n",
    );
    let base = repo.commit("base", "2020-01-01T00:00:00Z");
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { b + a }\n\
         #[cfg(test)] mod tests {\n\
         use super::*;\n\
         #[test] fn test_add() { assert_eq!(add(1, 2), 3); }\n\
         }\n",
    );
    let head = repo.commit("head", "2020-01-02T00:00:00Z");

    let mut store = SqliteStore::open_in_memory().expect("store");
    let a = run(
        &mut store,
        Request {
            repo_root: &repo.path,
            sides: Sides::Refs {
                base: base.clone(),
                head: head.clone(),
                merge_base: true,
            },
            walker: PathWalker::default(),
        },
    )
    .expect("impacted-tests");

    assert!(
        reported(&a).iter().any(|n| n.ends_with("::test_add")),
        "got {:?}",
        reported(&a)
    );
    assert_eq!(a.head_ref, head);
    assert_eq!(
        a.base_ref, base,
        "merge-base of an ancestor and its descendant is the ancestor"
    );
    assert!(
        !codes(&a).contains(&"impacted-tip-to-tip-base"),
        "merge-base was used, so the tip-to-tip caveat must NOT fire"
    );
}

#[test]
fn opting_out_of_merge_base_is_disclosed_as_an_over() {
    let repo = common::TestRepo::init();
    repo.write("src/lib.rs", "pub fn add() -> i32 { 1 }\n");
    let base = repo.commit("base", "2020-01-01T00:00:00Z");
    repo.write("src/lib.rs", "pub fn add() -> i32 { 2 }\n");
    let head = repo.commit("head", "2020-01-02T00:00:00Z");

    let mut store = SqliteStore::open_in_memory().expect("store");
    let a = run(
        &mut store,
        Request {
            repo_root: &repo.path,
            sides: Sides::Refs {
                base,
                head,
                merge_base: false,
            },
            walker: PathWalker::default(),
        },
    )
    .expect("impacted-tests");
    assert!(codes(&a).contains(&"impacted-tip-to-tip-base"));
}

#[test]
fn a_deleted_symbol_is_carried_as_an_under_reason() {
    let repo = common::TestRepo::init();
    repo.write(
        "src/lib.rs",
        "pub fn gone() -> i32 { 1 }\npub fn kept() -> i32 { 2 }\n",
    );
    repo.commit("base", "2020-01-01T00:00:00Z");
    repo.write("src/lib.rs", "pub fn kept() -> i32 { 2 }\n");

    let (_store, a) = run_workdir(&repo);
    assert!(
        codes(&a).contains(&"impacted-removed-symbols-not-walked"),
        "a test that exercised the deleted symbol is NOT in the answer, and the \
         contract has to say so: {:?}",
        codes(&a)
    );
}

#[test]
fn a_changed_file_that_yields_no_symbols_is_carried_as_an_under_reason() {
    let repo = fixture_repo("go");
    repo.write("NOTES.md", "hello\n");
    repo.commit("notes", "2020-01-02T00:00:00Z");
    edit(&repo, "NOTES.md", "hello", "goodbye");

    let (_store, a) = run_workdir(&repo);
    assert!(
        codes(&a).contains(&"impacted-changed-file-unindexed"),
        "got {:?}",
        codes(&a)
    );
}

// --- the changed-path narrowing ----------------------------------------------

/// `Repo::enumerate_workdir` walks every file under the repository root except
/// `.git`, consulting no gitignore and no tracked-file set. cgx's own `.cgx/`
/// store and any build output therefore land in the working-tree manifest and
/// are absent from the committed tree. Left alone they read as changed files
/// that contribute no symbols, which inflates `dirty_files`, fires
/// `impacted-changed-file-unindexed` on every run, and — because the vacuity
/// predicate is keyed on the changed *path* set — reports an unedited tree as
/// degenerate. All three of those consequences are what this test holds.
///
/// It holds the **absence** of the contract reason too, which is the whole
/// point: this is the state of every developer's checkout between builds, and a
/// paragraph of `under` disclosure on it trains users to skip the reason list.
/// The dropped `target/debug/build.log` and a dropped, genuinely new
/// `migration.sql` are no longer the same state — `git check-ignore` separates
/// them — so the disclosure fires for the migration
/// (`a_dropped_path_git_does_not_ignore_is_still_disclosed`) and not for this.
#[test]
fn untracked_files_that_yield_no_symbols_are_not_changes() {
    let repo = fixture_repo("go");
    repo.write(".gitignore", "target/\n");
    repo.commit("ignore build output", "2020-01-02T00:00:00Z");
    repo.write(".cgx/index.db", "not a source file\n");
    repo.write("target/debug/build.log", "not a source file\n");

    let (_store, a) = run_workdir(&repo);
    assert_eq!(a.dirty_files, 0, "nothing tracked changed");
    assert!(a.tests.tests.is_empty());
    assert!(
        !a.degenerate,
        "a clean tree is a real answer, not a vacuous one"
    );
    assert!(
        !codes(&a).contains(&"impacted-changed-file-unindexed"),
        "gitignored build output is not a dropped user change: {:?}",
        codes(&a)
    );
}

/// The other half of the same classification, and the reason it is safe: an
/// untracked path git's exclude rules do **not** cover is a genuinely new
/// unmodeled file, and dropping it silently is the failure the disclosure
/// exists for. Same fixture as above, one path renamed out of `target/`.
#[test]
fn a_dropped_path_git_does_not_ignore_is_still_disclosed() {
    let repo = fixture_repo("go");
    repo.write(".gitignore", "target/\n");
    repo.commit("ignore build output", "2020-01-02T00:00:00Z");
    repo.write(".cgx/index.db", "not a source file\n");
    repo.write("target/debug/build.log", "not a source file\n");
    repo.write("migration.sql", "CREATE TABLE t (id int);\n");

    let (_store, a) = run_workdir(&repo);
    assert_eq!(a.dirty_files, 0, "the migration yields no symbols either");
    assert!(
        codes(&a).contains(&"impacted-changed-file-unindexed"),
        "an unignored new file must not be discarded silently: {:?}",
        codes(&a)
    );
    assert_eq!(a.contract.direction, cgx_query::ApproxDirection::Under);
    let disclosure = a
        .contract
        .reasons
        .iter()
        .find(|r| r.code == "impacted-changed-file-unindexed")
        .expect("the disclosure");
    assert!(
        disclosure.detail.contains("1 working-tree path(s)"),
        "the two ignored paths must not inflate the count: {}",
        disclosure.detail
    );
}

/// The one carve-out that is absolute: cgx's own store is an artefact of this
/// very invocation, not user content, so on its own it never reads as a dropped
/// change. This is what keeps an otherwise-clean checkout's answer `exact`.
#[test]
fn cgxs_own_store_alone_is_not_a_dropped_change() {
    let repo = fixture_repo("go");
    repo.write(".cgx/index.db", "not a source file\n");

    let (_store, a) = run_workdir(&repo);
    assert_eq!(a.dirty_files, 0, "nothing tracked changed");
    assert!(
        !codes(&a).contains(&"impacted-changed-file-unindexed"),
        "cgx's own store is not a changed source file: {:?}",
        codes(&a)
    );
}

/// The narrowing must not buy the clean-tree answer by dropping real work: a
/// genuine edit still reports its test with the same noise present.
#[test]
fn the_narrowing_leaves_a_real_edit_alone() {
    let repo = fixture_repo("go");
    repo.write(".cgx/index.db", "not a source file\n");
    repo.write("target/debug/build.log", "not a source file\n");
    edit(&repo, "dataflow.go", "c := b + 1", "c := b + 2");

    let (_store, a) = run_workdir(&repo);
    assert_eq!(a.dirty_files, 1, "exactly the edited file");
    assert!(
        reported(&a).contains(&"go_sample::TestPlain"),
        "got {:?}",
        reported(&a)
    );
    assert!(!a.degenerate);
}

/// A brand-new source file is untracked too, so it survives only on the second
/// half of the predicate: an adapter claims it, so it contributes head symbols.
#[test]
fn a_new_untracked_source_file_is_still_a_change() {
    let repo = fixture_repo("go");
    repo.write(".cgx/index.db", "not a source file\n");
    repo.write(
        "fresh.go",
        "package go_sample\n\nfunc Fresh(a int) int {\n\treturn a + 1\n}\n",
    );

    let (_store, a) = run_workdir(&repo);
    assert_eq!(a.dirty_files, 1, "the new source file, and only it");
    assert!(
        a.changed
            .iter()
            .filter_map(|n| a.view.try_node(*n))
            .any(|nd| nd.file == "fresh.go"),
        "a new file's symbols must seed the walk"
    );
    assert!(!a.degenerate);
}

/// The narrowing is scoped to a working-directory head. Both sides of a
/// ref-to-ref comparison are committed trees, so a file added in the head
/// commit that yields no symbols is a real change and must still be disclosed.
#[test]
fn a_file_added_between_two_refs_still_carries_the_under_reason() {
    let repo = common::TestRepo::init();
    repo.write("src/lib.rs", "pub fn add() -> i32 { 1 }\n");
    let base = repo.commit("base", "2020-01-01T00:00:00Z");
    repo.write("NOTES.md", "hello\n");
    let head = repo.commit("head", "2020-01-02T00:00:00Z");

    let mut store = SqliteStore::open_in_memory().expect("store");
    let a = run(
        &mut store,
        Request {
            repo_root: &repo.path,
            sides: Sides::Refs {
                base,
                head,
                merge_base: true,
            },
            walker: PathWalker::default(),
        },
    )
    .expect("impacted-tests");

    assert_eq!(a.dirty_files, 1);
    assert!(
        codes(&a).contains(&"impacted-changed-file-unindexed"),
        "got {:?}",
        codes(&a)
    );
}

// --- determinism at the orchestration level ----------------------------------

#[test]
fn the_same_request_answers_identically_across_runs() {
    let repo = fixture_repo("go");
    edit(&repo, "dataflow.go", "c := b + 1", "c := b + 2");

    let render = |a: &Answer| {
        let rows: Vec<String> = a
            .tests
            .tests
            .iter()
            .zip(&a.tests.witnesses)
            .map(|(r, w)| {
                format!(
                    "{}|{}|{}|{}|{:?}|{}|{:?}",
                    r.node.fqn,
                    r.node.file,
                    r.node.line_start,
                    r.depth,
                    r.min_confidence_on_path,
                    w.via_containment_lift,
                    w.chain.iter().map(|n| n.0).collect::<Vec<_>>()
                )
            })
            .collect();
        format!(
            "{}\n{:?}\n{:?}\n{}",
            rows.join("\n"),
            codes(a),
            a.contract.direction,
            a.head_graph_key
        )
    };

    let (_s1, first) = run_workdir(&repo);
    let (_s2, second) = run_workdir(&repo);
    let (_s3, third) = run_workdir(&repo);
    assert_eq!(render(&first), render(&second));
    assert_eq!(render(&second), render(&third));
}
