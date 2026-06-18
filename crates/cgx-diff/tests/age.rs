//! Edge-age / introducing-commit attribution (IX-9): the commit that introduced
//! an edge (via blame on the caller's lines), authorship, and graceful
//! degradation when blame cannot run.

mod common;

use cgx_diff::BlameRepo;
use common::*;

/// A call edge introduced at commit B (the caller body did not exist at A) is
/// attributed to B, with the right author.
#[test]
fn edge_introduced_at_a_commit_is_attributed_to_it() {
    let mut repo = TestRepo::init();
    // Commit A: only `target` exists.
    repo.write("src/lib.rs", "pub fn target() -> i32 { 1 }\n");
    repo.commit("a", "2020-01-01T00:00:00Z");
    let _ = repo.index_head();

    // Commit B: add `caller` which calls `target`.
    repo.write(
        "src/lib.rs",
        "pub fn target() -> i32 { 1 }\npub fn caller() -> i32 { target() }\n",
    );
    let commit_b = repo.commit("b", "2020-02-01T00:00:00Z");
    let (_head_id, head) = repo.index_head();

    let edge = head
        .edges
        .iter()
        .find(|e| {
            head.nodes[e.src.index()].fqn.ends_with("::caller")
                && head.nodes[e.dst.index()].fqn.ends_with("::target")
        })
        .expect("caller->target edge present");

    let blame = BlameRepo::discover(&repo.path).expect("open repo");
    let head_commit = blame.resolve_commit("HEAD").expect("resolve HEAD");
    assert_eq!(head_commit, commit_b, "HEAD is commit B");

    let mut warnings = Vec::new();
    let age = blame.edge_age(&head, edge, &head_commit, &mut warnings);

    assert!(
        age.is_attributed(),
        "edge must be attributed; warnings={warnings:?}"
    );
    assert_eq!(
        age.introducing_commit.as_deref(),
        Some(commit_b.as_str()),
        "the caller body was introduced by commit B"
    );
    assert_eq!(age.introducing_author.as_deref(), Some("cgx-test"));
    assert!(age.author_time.is_some(), "author timestamp populated");
    assert!(
        warnings.is_empty(),
        "clean attribution emits no warnings: {warnings:?}"
    );
}

/// Two callers introduced in different commits get different attributions —
/// proving blame discriminates by line, not by file.
#[test]
fn edges_from_different_commits_get_different_introducing_commits() {
    let mut repo = TestRepo::init();
    repo.write(
        "src/lib.rs",
        "pub fn target() -> i32 { 1 }\npub fn old_caller() -> i32 { target() }\n",
    );
    let commit_a = repo.commit("a", "2020-01-01T00:00:00Z");

    repo.write(
        "src/lib.rs",
        "pub fn target() -> i32 { 1 }\npub fn old_caller() -> i32 { target() }\npub fn new_caller() -> i32 { target() }\n",
    );
    let commit_b = repo.commit("b", "2020-02-01T00:00:00Z");
    let (_id, head) = repo.index_head();

    let blame = BlameRepo::discover(&repo.path).expect("open");
    let head_commit = blame.resolve_commit("HEAD").expect("HEAD");

    let age_of = |suffix: &str| {
        let edge = head
            .edges
            .iter()
            .find(|e| {
                head.nodes[e.src.index()].fqn.ends_with(suffix)
                    && head.nodes[e.dst.index()].fqn.ends_with("::target")
            })
            .unwrap_or_else(|| panic!("edge from {suffix} present"));
        let mut w = Vec::new();
        blame.edge_age(&head, edge, &head_commit, &mut w)
    };

    let old = age_of("::old_caller");
    let new = age_of("::new_caller");
    assert_eq!(old.introducing_commit.as_deref(), Some(commit_a.as_str()));
    assert_eq!(new.introducing_commit.as_deref(), Some(commit_b.as_str()));
    assert_ne!(old.introducing_commit, new.introducing_commit);
}

/// A synthetic/working-dir suspect (not a commit OID) degrades to an
/// unattributed result plus a warning — never an error (WP-11 graceful
/// degradation, mirroring the shallow-clone case).
#[test]
fn non_commit_suspect_degrades_with_warning() {
    let mut repo = TestRepo::init();
    repo.write(
        "src/lib.rs",
        "pub fn target() -> i32 { 1 }\npub fn caller() -> i32 { target() }\n",
    );
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (_id, head) = repo.index_head();

    let edge = head
        .edges
        .iter()
        .find(|e| head.nodes[e.src.index()].fqn.ends_with("::caller"))
        .expect("edge present");

    let blame = BlameRepo::discover(&repo.path).expect("open");
    let mut warnings = Vec::new();
    // A synthetic working-dir key, exactly what `index_workdir` produces.
    let age = blame.edge_age(&head, edge, "workdir:deadbeef", &mut warnings);

    assert!(
        !age.is_attributed(),
        "uncommitted suspect yields no attribution"
    );
    assert!(age.introducing_commit.is_none());
    assert_eq!(
        warnings.len(),
        1,
        "exactly one degradation warning: {warnings:?}"
    );
    assert!(
        warnings[0].0.contains("uncommitted") || warnings[0].0.contains("not a commit"),
        "warning explains the degradation: {warnings:?}"
    );
}

/// `resolve_commit` resolves refs (tags, HEAD) and `merge_base` finds the
/// branch point — the selectors `cgx diff --base/--head` rely on.
#[test]
fn resolve_commit_and_merge_base_work() {
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    let base = repo.commit("base", "2020-01-01T00:00:00Z");
    let _ = repo.index_head();

    let blame = BlameRepo::discover(&repo.path).expect("open");
    assert_eq!(blame.resolve_commit("HEAD").unwrap(), base);

    // Two children of `base`: a second commit on main, and the same base again
    // (merge-base of HEAD and base is base itself).
    repo.write("src/lib.rs", "pub fn a() {}\npub fn b() {}\n");
    let head = repo.commit("head", "2020-02-01T00:00:00Z");

    let mb = blame.merge_base(&head, &base).expect("merge-base");
    assert_eq!(
        mb, base,
        "merge-base of a commit and its ancestor is the ancestor"
    );
}
