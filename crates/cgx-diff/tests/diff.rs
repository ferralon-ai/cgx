//! Graph-diff classification (IX-4): added / removed / changed edges and nodes,
//! and determinism — over two committed-and-indexed trees of a temp git repo.

mod common;

use cgx_diff::{diff_graphs, diff_trees, edges_newer_than};
use common::*;

/// Source at commit A: `caller_a` calls `target`; `caller_b` calls nothing;
/// `lonely` exists and is removed at B.
const SRC_A: &str = r#"
pub fn target() -> i32 { 1 }

pub fn caller_a() -> i32 { target() }

pub fn caller_b() -> i32 { 0 }

pub fn lonely() -> i32 { 99 }
"#;

/// Source at commit B: `caller_a` no longer calls `target` (removed edge);
/// `caller_b` now calls `target` (added edge); `lonely` is gone (removed node),
/// `fresh` appears (added node).
const SRC_B: &str = r#"
pub fn target() -> i32 { 1 }

pub fn caller_a() -> i32 { 0 }

pub fn caller_b() -> i32 { target() }

pub fn fresh() -> i32 { target() }
"#;

#[test]
fn added_and_removed_edges_are_classified_by_identity() {
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", SRC_A);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (base_id, base) = repo.index_head();

    repo.write("src/lib.rs", SRC_B);
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (head_id, head) = repo.index_head();

    let diff = diff_trees(&repo.store, base_id, head_id).expect("diff");

    let added = added_pairs(&diff);
    let removed = removed_pairs(&diff);

    assert!(
        has_pair(&added, "caller_b", "target"),
        "caller_b->target is a new edge at head: added={added:?}"
    );
    assert!(
        has_pair(&removed, "caller_a", "target"),
        "caller_a->target existed only at base: removed={removed:?}"
    );
    assert!(
        !has_pair(&added, "caller_a", "target"),
        "caller_a->target must not be reported added"
    );
    // Sanity: diff_graphs over the read-back graphs agrees with diff_trees.
    assert_eq!(diff, diff_graphs(&base, &head));
}

#[test]
fn added_and_removed_nodes_are_classified_by_fqn() {
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", SRC_A);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (base_id, _) = repo.index_head();

    repo.write("src/lib.rs", SRC_B);
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (head_id, _) = repo.index_head();

    let diff = diff_trees(&repo.store, base_id, head_id).expect("diff");

    let added: Vec<&str> = diff.added_nodes.iter().map(|n| n.fqn.as_str()).collect();
    let removed: Vec<&str> = diff.removed_nodes.iter().map(|n| n.fqn.as_str()).collect();

    assert!(
        added.iter().any(|f| f.ends_with("::fresh")),
        "fresh is new: {added:?}"
    );
    assert!(
        removed.iter().any(|f| f.ends_with("::lonely")),
        "lonely was removed: {removed:?}"
    );
    assert!(
        !added.iter().any(|f| f.ends_with("::target")),
        "target persists, not added: {added:?}"
    );
}

#[test]
fn identical_trees_produce_empty_diff() {
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", SRC_A);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (id, _) = repo.index_head();

    let diff = diff_trees(&repo.store, id, id).expect("diff");
    assert!(
        diff.is_empty(),
        "diffing a tree against itself is empty: {diff:?}"
    );
}

#[test]
fn changed_edge_reports_condition_change() {
    // caller calls target unconditionally at A, conditionally at B — same
    // caller/callee identity, changed `condition`.
    let src_a = r#"
pub fn target() -> i32 { 1 }
pub fn caller(flag: bool) -> i32 { target() }
"#;
    let src_b = r#"
pub fn target() -> i32 { 1 }
pub fn caller(flag: bool) -> i32 { if flag { target() } else { 0 } }
"#;
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", src_a);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (base_id, _) = repo.index_head();

    repo.write("src/lib.rs", src_b);
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (head_id, _) = repo.index_head();

    let diff = diff_trees(&repo.store, base_id, head_id).expect("diff");

    let changed = diff.changed_edges.iter().find(|c| {
        c.identity.src_fqn.ends_with("::caller") && c.identity.dst_fqn.ends_with("::target")
    });
    let changed =
        changed.unwrap_or_else(|| panic!("caller->target should be a changed edge; diff={diff:?}"));
    assert!(
        changed.changes.condition,
        "the condition attribute changed (Always -> Conditional): {:?}",
        changed.changes
    );
    assert_ne!(
        changed.base.condition, changed.head.condition,
        "base and head conditions differ"
    );
}

#[test]
fn edges_newer_than_equals_added_edges() {
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", SRC_A);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (base_id, _) = repo.index_head();

    repo.write("src/lib.rs", SRC_B);
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (head_id, _) = repo.index_head();

    let newer = edges_newer_than(&repo.store, base_id, head_id).expect("gate");
    let diff = diff_trees(&repo.store, base_id, head_id).expect("diff");
    assert_eq!(
        newer, diff.added_edges,
        "the gate is exactly the added-edge set"
    );
    assert!(
        newer
            .iter()
            .any(|e| e.identity.src_fqn.ends_with("::caller_b")
                && e.identity.dst_fqn.ends_with("::target")),
        "the new caller_b->target edge is gated in: {newer:?}"
    );
}

#[test]
fn diff_is_deterministic_across_repeated_runs() {
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", SRC_A);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (base_id, base) = repo.index_head();
    repo.write("src/lib.rs", SRC_B);
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (head_id, head) = repo.index_head();

    let d1 = diff_trees(&repo.store, base_id, head_id).expect("diff");
    let d2 = diff_graphs(&base, &head);
    let d3 = diff_graphs(&base, &head);
    assert_eq!(d1, d2);
    assert_eq!(d2, d3, "repeated diffs are identical");

    // Ordering is total: added edges are sorted by identity.
    let mut sorted = d1.added_edges.clone();
    sorted.sort_by(|a, b| a.identity.cmp(&b.identity));
    assert_eq!(
        d1.added_edges, sorted,
        "added edges already in identity order"
    );
}
