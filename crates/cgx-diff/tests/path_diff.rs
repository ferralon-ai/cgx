//! CH-11 S1: path-level diff (the structural gate).
//!
//! A new *path* (not just a single edge) from a `--from` source to a `--to` sink
//! that exists at HEAD but not BASE is reported as call/dataflow reachability —
//! NOT a security guarantee. An unchanged path is never reported. Boundedness is
//! exercised by the shipped `PathWalker` step budget.

mod common;

use cgx_core::SymbolPattern;
use cgx_diff::path_diff_graphs;
use common::*;

/// A new multi-hop path is reported when HEAD introduces the connecting hop.
#[test]
fn new_multi_hop_path_is_reported() {
    // Base: handler does NOT reach sink (no call into the middle hop).
    let src_a = r#"
pub fn sink() {}
pub fn middle() {}
pub fn handler() {}
"#;
    // Head: handler -> middle -> sink, a brand-new 2-hop path.
    let src_b = r#"
pub fn sink() {}
pub fn middle() { sink() }
pub fn handler() { middle() }
"#;
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", src_a);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (_id, base) = repo.index_head();
    repo.write("src/lib.rs", src_b);
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (_id2, head) = repo.index_head();

    let from = SymbolPattern::glob("**::handler");
    let to = SymbolPattern::glob("**::sink");
    let diff = path_diff_graphs(&base, &head, &from, &to);

    assert!(diff.has_new_path(), "a new handler->sink path exists at head");
    let p = &diff.added_paths[0];
    assert!(p.from_fqn.ends_with("::handler"));
    assert!(p.to_fqn.ends_with("::sink"));
    assert!(
        p.via.first().map(|s| s.ends_with("::handler")).unwrap_or(false),
        "witness starts at the source: {:?}",
        p.via
    );
    assert!(
        p.via.last().map(|s| s.ends_with("::sink")).unwrap_or(false),
        "witness ends at the sink: {:?}",
        p.via
    );
    assert!(
        p.via.iter().any(|s| s.ends_with("::middle")),
        "witness threads through the middle hop: {:?}",
        p.via
    );
}

/// A path that exists on BOTH sides is NOT reported (only newly-introduced paths).
#[test]
fn unchanged_path_is_not_reported() {
    let src = r#"
pub fn sink() {}
pub fn handler() { sink() }
"#;
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", src);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (_id, base) = repo.index_head();
    // Head identical (re-commit so both graphs exist): the handler->sink path is
    // unchanged.
    repo.write("src/lib.rs", src);
    repo.write("src/other.rs", "pub fn noise() {}");
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (_id2, head) = repo.index_head();

    let from = SymbolPattern::glob("**::handler");
    let to = SymbolPattern::glob("**::sink");
    let diff = path_diff_graphs(&base, &head, &from, &to);

    assert!(
        !diff.has_new_path(),
        "the handler->sink path exists on both sides, so it is not 'added': {:?}",
        diff.added_paths
    );
}

/// A newly-added direct edge that creates the path is reported even when the
/// endpoints both already existed (the *path* is new, not the nodes).
#[test]
fn new_direct_path_between_existing_nodes_is_reported() {
    let src_a = r#"
pub fn sink() {}
pub fn handler() {}
"#;
    let src_b = r#"
pub fn sink() {}
pub fn handler() { sink() }
"#;
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", src_a);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (_id, base) = repo.index_head();
    repo.write("src/lib.rs", src_b);
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (_id2, head) = repo.index_head();

    let from = SymbolPattern::glob("**::handler");
    let to = SymbolPattern::glob("**::sink");
    let diff = path_diff_graphs(&base, &head, &from, &to);

    assert!(
        diff.has_new_path(),
        "handler->sink is a new path at head though both nodes existed at base"
    );
}

/// When the `--to` glob matches nothing, no path is reported (open-world: an
/// unmatched sink is simply not a sink).
#[test]
fn unmatched_sink_yields_no_path() {
    let src = r#"
pub fn sink() {}
pub fn handler() { sink() }
"#;
    let mut repo = TestRepo::init();
    repo.write("src/lib.rs", src);
    repo.commit("a", "2020-01-01T00:00:00Z");
    let (_id, base) = repo.index_head();
    repo.write("src/lib.rs", src);
    repo.write("src/x.rs", "pub fn x() {}");
    repo.commit("b", "2020-02-01T00:00:00Z");
    let (_id2, head) = repo.index_head();

    let from = SymbolPattern::glob("**::handler");
    let to = SymbolPattern::glob("**::does_not_exist");
    let diff = path_diff_graphs(&base, &head, &from, &to);
    assert!(!diff.has_new_path(), "no sink matched, so no path");
}
