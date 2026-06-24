//! v0.3 DATA_FLOW SC3: incremental / lazy-invalidation convergence criteria.
//!
//! Each test drives the full pipeline (extract → cache → link → store) with
//! `IndexOpts { dataflow: true }` over a fresh git repo, editing a function and
//! re-indexing against the SAME store so the per-function `(blob_oid, fn_fqn)`
//! intraproc cache is consulted.
//!
//! Covered: minimal recompute (1), zero recompute on untouched files (2),
//! summary-dep propagation + fixpoint (3), full == incremental equivalence (4),
//! and the conservative wildcard fallback (6). Determinism (5) stays covered by
//! `dataflow::dataflow_reindex_is_byte_identical`.

mod common;

use cgx_index::{default_registry, index_path, IndexOpts};
use common::*;

fn dataflow_opts() -> IndexOpts {
    IndexOpts {
        dataflow: true,
        ..Default::default()
    }
}

/// Three independent leaf functions in one file. Editing one changes only its
/// own intraproc dataflow facts; none is a summary-dep of another. Each function
/// is a single line so a within-line edit leaves siblings' spans untouched.
const THREE_LEAVES_V1: &str = "\
pub fn one(a: i32, x: i32) -> i32 { let b = a; b }
pub fn two(a: i32, x: i32) -> i32 { let b = a; b }
pub fn three(a: i32, x: i32) -> i32 { let b = a; b }
";

/// Same file with only `two`'s dataflow changed (`b` now derives from `x`, not
/// `a`) — a structural change to its `DataFlowFact` set, on the same line.
const THREE_LEAVES_V2: &str = "\
pub fn one(a: i32, x: i32) -> i32 { let b = a; b }
pub fn two(a: i32, x: i32) -> i32 { let b = x; b }
pub fn three(a: i32, x: i32) -> i32 { let b = a; b }
";

#[test]
fn editing_one_function_recomputes_only_that_function() {
    // Criterion 1: edit one fn in a multi-fn file → functions_recomputed == 1.
    let (_tmp, repo) = init_empty_repo();
    let registry = default_registry();
    let mut store = mem_store();

    write_file(&repo, "src/leaves.rs", THREE_LEAVES_V1);
    commit_all(&repo, "v1");
    let cold = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();
    let n_fns = cold.stats.dataflow.functions_recomputed;
    assert!(n_fns >= 3, "cold build recomputes every function, got {n_fns}");
    assert_eq!(cold.stats.dataflow.functions_reused, 0, "cold build reuses nothing");

    // Edit exactly one function's body and re-index against the same store.
    write_file(&repo, "src/leaves.rs", THREE_LEAVES_V2);
    commit_all(&repo, "edit two");
    let warm = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();

    assert_eq!(
        warm.stats.dataflow.functions_recomputed, 1,
        "only the edited function recomputes: {:?}",
        warm.stats.dataflow
    );
    assert_eq!(
        warm.stats.dataflow.functions_reused,
        n_fns - 1,
        "every other function is reused"
    );
}

#[test]
fn untouched_file_recomputes_zero_functions() {
    // Criterion 2: after an edit elsewhere, an unchanged file recomputes 0 fns.
    let (_tmp, repo) = init_empty_repo();
    let registry = default_registry();
    let mut store = mem_store();

    write_file(&repo, "src/leaves.rs", THREE_LEAVES_V1);
    write_file(
        &repo,
        "src/other.rs",
        "pub fn untouched(a: i32) -> i32 { let z = a; z }\n",
    );
    commit_all(&repo, "v1");
    let cold = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();
    let total = cold.stats.dataflow.functions_recomputed;

    // Edit only leaves.rs; other.rs is byte-identical.
    write_file(&repo, "src/leaves.rs", THREE_LEAVES_V2);
    commit_all(&repo, "edit leaves only");
    let warm = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();

    // Exactly one function (in leaves.rs) recomputed; everything else reused —
    // so the untouched file's function(s) are all reused.
    assert_eq!(
        warm.stats.dataflow.functions_recomputed, 1,
        "only the one edited fn recomputes: {:?}",
        warm.stats.dataflow
    );
    assert_eq!(warm.stats.dataflow.functions_reused, total - 1);
}

/// A caller chain: `caller` calls `middle`, `middle` calls `leaf`; `solo` is
/// independent. Editing `leaf` must propagate to `middle` and `caller`.
const CHAIN_V1: &str = "\
pub fn leaf(a: i32, x: i32) -> i32 { let b = a; b }
pub fn middle(a: i32) -> i32 { let r = leaf(a, a); r }
pub fn caller(a: i32) -> i32 { let r = middle(a); r }
pub fn solo(a: i32, x: i32) -> i32 { let b = a; b }
";

/// `leaf`'s dataflow changes (`b` derives from `x`, not `a`) on the same line.
/// `middle`/`caller`/`solo` source text is byte-identical, so only `leaf`'s own
/// facts_hash changes — transitive propagation must pull `middle` and `caller`.
const CHAIN_V2_EDIT_LEAF: &str = "\
pub fn leaf(a: i32, x: i32) -> i32 { let b = x; b }
pub fn middle(a: i32) -> i32 { let r = leaf(a, a); r }
pub fn caller(a: i32) -> i32 { let r = middle(a); r }
pub fn solo(a: i32, x: i32) -> i32 { let b = a; b }
";

#[test]
fn summary_dep_change_propagates_to_transitive_callers_and_stops_at_fixpoint() {
    // Criterion 3: editing `leaf` recomputes leaf + its transitive callers
    // (middle, caller) via summary_deps, but NOT the independent `solo`.
    let (_tmp, repo) = init_empty_repo();
    let registry = default_registry();
    let mut store = mem_store();

    write_file(&repo, "src/chain.rs", CHAIN_V1);
    commit_all(&repo, "v1");
    let cold = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();
    let total = cold.stats.dataflow.functions_recomputed;
    assert!(total >= 4, "cold build saw all functions, got {total}");

    write_file(&repo, "src/chain.rs", CHAIN_V2_EDIT_LEAF);
    commit_all(&repo, "edit leaf");
    let warm = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();

    // leaf (own change) + middle + caller (transitive callers) = 3; solo stays.
    assert_eq!(
        warm.stats.dataflow.functions_recomputed, 3,
        "leaf + middle + caller recompute, solo does not: {:?}",
        warm.stats.dataflow
    );
    assert_eq!(
        warm.stats.dataflow.functions_reused,
        total - 3,
        "the fixpoint stops at solo (and any non-caller fns)"
    );
}

#[test]
fn full_rebuild_equals_incremental_byte_identical() {
    // Criterion 4 (soundness gate): an incremental update over a warm store yields
    // a byte-identical graph to a cold build of the same final tree.
    let registry = default_registry();

    // Incremental: index v1, edit, re-index against the same store.
    let (_tmp_inc, repo_inc) = init_empty_repo();
    let mut store_inc = mem_store();
    write_file(&repo_inc, "src/chain.rs", CHAIN_V1);
    commit_all(&repo_inc, "v1");
    index_path(&repo_inc, &registry, &mut store_inc, &dataflow_opts()).unwrap();
    write_file(&repo_inc, "src/chain.rs", CHAIN_V2_EDIT_LEAF);
    commit_all(&repo_inc, "edit leaf");
    let inc = index_path(&repo_inc, &registry, &mut store_inc, &dataflow_opts()).unwrap();

    // Cold: a fresh repo + store at the SAME final content.
    let (_tmp_cold, repo_cold) = init_empty_repo();
    let mut store_cold = mem_store();
    write_file(&repo_cold, "src/chain.rs", CHAIN_V2_EDIT_LEAF);
    commit_all(&repo_cold, "v1");
    let cold = index_path(&repo_cold, &registry, &mut store_cold, &dataflow_opts()).unwrap();

    assert_eq!(
        inc.graph_key, cold.graph_key,
        "same final tree content keys the same Layer-2 graph"
    );
    let dump_inc = store_inc.dump_node_edge_data(inc.graph_id).unwrap();
    let dump_cold = store_cold.dump_node_edge_data(cold.graph_id).unwrap();
    assert_eq!(
        dump_inc, dump_cold,
        "incremental graph must be byte-identical to the cold rebuild"
    );
}

/// A virtual/duck-typed call site: `obj.method()` produces no resolvable callee
/// name, so `caller_v` records a `*` wildcard summary dep. Editing the unrelated
/// `mutator` must force `caller_v` to recompute (conservative over-invalidation).
const WILDCARD_V1: &str = "\
pub struct Widget;
impl Widget {
    pub fn poke(&self) -> i32 { 1 }
}
pub fn caller_v(w: &Widget) -> i32 { let r = w.poke(); r }
pub fn mutator(a: i32, x: i32) -> i32 { let b = a; b }
";

/// `mutator`'s dataflow changes (`b` derives from `x`) on the same line. Its
/// facts_hash changes; `caller_v` is byte-identical but its virtual call to
/// `w.poke()` recorded a `*` wildcard dep, so it must recompute too.
const WILDCARD_V2_EDIT_MUTATOR: &str = "\
pub struct Widget;
impl Widget {
    pub fn poke(&self) -> i32 { 1 }
}
pub fn caller_v(w: &Widget) -> i32 { let r = w.poke(); r }
pub fn mutator(a: i32, x: i32) -> i32 { let b = x; b }
";

#[test]
fn interproc_summary_edge_materialized_end_to_end() {
    // Criterion 1 (end-to-end): the real Rust extractor + IFDS pass materialize an
    // interprocedural DerivesFrom edge in `middle` (which calls `leaf(a, a)`),
    // tagged `interprocedural` + confidence `probable`.
    use cgx_core::confidence::Confidence;
    use cgx_core::edge::EdgeKind;

    let (_tmp, repo) = init_empty_repo();
    let registry = default_registry();
    let mut store = mem_store();
    write_file(&repo, "src/chain.rs", CHAIN_V1);
    commit_all(&repo, "v1");
    let out = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();

    assert!(
        out.stats.ifds.summary_edges_materialized >= 1,
        "the chain must materialize at least one interproc edge: {:?}",
        out.stats.ifds
    );
    assert_eq!(out.stats.ifds.budget_exceeded_sccs, 0, "no budget trip on the small chain");

    let g = read_graph(&store, out.graph_id);
    let interproc: Vec<_> = g
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::DerivesFrom && e.rule == "interprocedural")
        .collect();
    assert!(
        !interproc.is_empty(),
        "an interprocedural DerivesFrom edge must be stored"
    );
    assert!(
        interproc.iter().all(|e| e.confidence == Confidence::Probable),
        "every summary edge is probable"
    );
}

#[test]
fn summary_edges_consumed_by_existing_walk_no_second_path() {
    // Criterion 5: summaries materialize as DerivesFrom edges (read by the EXISTING
    // walk over the `edges` table) — there is no second execution path. The
    // `v_data_flow_edges` view surfaces them; a count query over the store sees the
    // interproc rows like any other DerivesFrom edge.
    use cgx_core::edge::EdgeKind;

    let (_tmp, repo) = init_empty_repo();
    let registry = default_registry();
    let mut store = mem_store();
    write_file(&repo, "src/chain.rs", CHAIN_V1);
    commit_all(&repo, "v1");
    let out = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();

    // The store's read path reconstructs the interproc edges from the same `edges`
    // table as every other edge — proving they live on the one execution path.
    let g = read_graph(&store, out.graph_id);
    let interproc = g
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::DerivesFrom && e.rule == "interprocedural")
        .count();
    assert!(interproc >= 1, "summary edges must be queryable via the edges table");

    // The `v_data_flow_edges` ADR-05 view (unchanged contract) counts all
    // derives-from edges including the interproc ones — the query surface needs no
    // new code path.
    let n: i64 = store
        .query_count("SELECT COUNT(*) FROM v_data_flow_edges WHERE rule = 'interprocedural'")
        .unwrap();
    assert!(n >= 1, "the interproc edges are visible through the existing v_data_flow_edges view");
}

#[test]
fn wildcard_dependent_recomputes_when_any_candidate_changes() {
    // Criterion 6: caller_v has an unresolved (virtual) callee → wildcard dep. An
    // edit to ANY function (mutator) forces caller_v to recompute too, so the
    // recompute count exceeds just the one edited function.
    let (_tmp, repo) = init_empty_repo();
    let registry = default_registry();
    let mut store = mem_store();

    write_file(&repo, "src/wild.rs", WILDCARD_V1);
    commit_all(&repo, "v1");
    index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();

    write_file(&repo, "src/wild.rs", WILDCARD_V2_EDIT_MUTATOR);
    commit_all(&repo, "edit mutator");
    let warm = index_path(&repo, &registry, &mut store, &dataflow_opts()).unwrap();

    // mutator changed (1) + caller_v forced by its wildcard dep (1) = at least 2.
    assert!(
        warm.stats.dataflow.functions_recomputed >= 2,
        "wildcard dep must drag caller_v into the recompute set: {:?}",
        warm.stats.dataflow
    );
}
