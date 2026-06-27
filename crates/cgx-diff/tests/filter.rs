//! CH-11 S0: diff post-filters as pure set math over a `GraphDiff`.
//!
//! Each test builds a two-commit repo, computes the real diff, then asserts a
//! `DiffFilter` selects exactly the intended buckets/edges. Filtering never
//! re-parses and preserves the deterministic order of the surviving records.

mod common;

use cgx_core::{EdgeCondition, EdgeKind, SymbolPattern};
use cgx_diff::{diff_trees, BucketSelect, DiffFilter};
use common::*;

/// Base: `caller_a` calls `target`; `lonely` exists.
/// Head: `caller_a` drops the call (removed edge), `caller_b` gains it (added
/// edge), `lonely` is removed (removed node), `fresh` appears (added node).
const SRC_A: &str = r#"
pub fn target() -> i32 { 1 }
pub fn caller_a() -> i32 { target() }
pub fn caller_b() -> i32 { 0 }
pub fn lonely() -> i32 { 99 }
"#;

const SRC_B: &str = r#"
pub fn target() -> i32 { 1 }
pub fn caller_a() -> i32 { 0 }
pub fn caller_b() -> i32 { target() }
pub fn fresh() -> i32 { target() }
"#;

fn two_commit_diff() -> cgx_diff::GraphDiff {
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", SRC_A);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (base_id, _) = repo.index_head();
    repo.write("src/lib.rs", SRC_B);
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (head_id, _) = repo.index_head();
    diff_trees(&repo.store, base_id, head_id).expect("diff")
}

#[test]
fn added_bucket_select_keeps_only_added() {
    let diff = two_commit_diff();
    let filter = DiffFilter::with_buckets(BucketSelect::from_flags(true, false, false));
    let out = filter.apply(&diff);

    assert!(!out.added_edges.is_empty(), "added edges survive");
    assert!(out.removed_edges.is_empty(), "removed edges dropped");
    assert!(out.changed_edges.is_empty(), "changed edges dropped");
    assert!(!out.added_nodes.is_empty(), "added nodes follow the added bucket");
    assert!(out.removed_nodes.is_empty(), "removed nodes dropped");
}

#[test]
fn removed_bucket_select_keeps_only_removed() {
    let diff = two_commit_diff();
    let filter = DiffFilter::with_buckets(BucketSelect::from_flags(false, true, false));
    let out = filter.apply(&diff);

    assert!(out.added_edges.is_empty(), "added edges dropped");
    assert!(!out.removed_edges.is_empty(), "removed edges survive");
    assert!(out.added_nodes.is_empty(), "added nodes dropped");
    assert!(!out.removed_nodes.is_empty(), "removed nodes follow removed bucket");
}

#[test]
fn no_bucket_flag_defaults_to_all_buckets() {
    let diff = two_commit_diff();
    let filter = DiffFilter::with_buckets(BucketSelect::from_flags(false, false, false));
    let out = filter.apply(&diff);

    // Identical to the unfiltered diff (default = all buckets, no edge predicate).
    assert_eq!(out, diff, "no flags = the full diff, unchanged");
}

#[test]
fn kind_filter_keeps_only_matching_edge_kind() {
    let diff = two_commit_diff();
    let filter = DiffFilter {
        kinds: vec![EdgeKind::Calls],
        ..Default::default()
    };
    let out = filter.apply(&diff);

    assert!(
        out.added_edges.iter().all(|e| e.identity.edge_kind == EdgeKind::Calls),
        "only Calls edges survive the kind filter"
    );
    // An edge predicate suppresses node output (nodes aren't edges).
    assert!(out.added_nodes.is_empty() && out.removed_nodes.is_empty());
}

#[test]
fn kind_filter_for_absent_kind_yields_no_edges() {
    let diff = two_commit_diff();
    let filter = DiffFilter {
        kinds: vec![EdgeKind::Throws],
        ..Default::default()
    };
    let out = filter.apply(&diff);
    assert!(
        out.added_edges.is_empty() && out.removed_edges.is_empty() && out.changed_edges.is_empty(),
        "no throws edges in this fixture, so all edge buckets are empty"
    );
}

#[test]
fn edge_condition_filter_keeps_only_matching_condition() {
    // Base: unconditional call; Head: conditional call (a changed edge).
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

    // The changed caller->target edge is Conditional at head; filtering on
    // Conditional keeps it, filtering on Exception drops it.
    let keep = DiffFilter {
        edge_condition: Some(EdgeCondition::Conditional),
        ..Default::default()
    }
    .apply(&diff);
    assert!(
        keep.changed_edges
            .iter()
            .any(|c| c.identity.src_fqn.ends_with("::caller")),
        "conditional-condition filter keeps the caller->target change"
    );

    let drop = DiffFilter {
        edge_condition: Some(EdgeCondition::Exception),
        ..Default::default()
    }
    .apply(&diff);
    assert!(
        !drop
            .changed_edges
            .iter()
            .any(|c| c.identity.src_fqn.ends_with("::caller")),
        "exception-condition filter drops the (conditional) change"
    );
}

#[test]
fn from_glob_filters_by_source_fqn() {
    let diff = two_commit_diff();
    // Keep only edges whose source ends in caller_b.
    let filter = DiffFilter {
        from: Some(SymbolPattern::glob("**::caller_b")),
        ..Default::default()
    };
    let out = filter.apply(&diff);
    assert!(
        out.added_edges
            .iter()
            .all(|e| e.identity.src_fqn.ends_with("::caller_b")),
        "every surviving added edge has caller_b as source"
    );
    assert!(
        out.added_edges.iter().any(|e| e.identity.dst_fqn.ends_with("::target")),
        "the caller_b->target edge survives the from-glob"
    );
}

#[test]
fn to_glob_filters_by_destination_fqn() {
    let diff = two_commit_diff();
    let filter = DiffFilter {
        to: Some(SymbolPattern::glob("**::target")),
        ..Default::default()
    };
    let out = filter.apply(&diff);
    assert!(
        out.added_edges
            .iter()
            .all(|e| e.identity.dst_fqn.ends_with("::target")),
        "every surviving added edge points at target"
    );
}

#[test]
fn filter_preserves_order_and_is_idempotent() {
    let diff = two_commit_diff();
    let filter = DiffFilter::with_buckets(BucketSelect::from_flags(true, false, false));
    let once = filter.apply(&diff);
    let twice = filter.apply(&once);
    assert_eq!(once, twice, "applying the same filter twice is idempotent");
    // Surviving edges keep the input's canonical identity order.
    let mut sorted = once.added_edges.clone();
    sorted.sort_by(|a, b| a.identity.cmp(&b.identity));
    assert_eq!(once.added_edges, sorted, "order preserved through the filter");
}
