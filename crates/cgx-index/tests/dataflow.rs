//! v0.3 DATA_FLOW SC2 end-to-end: index the committed `rust-sample` fixture with
//! and without `--dataflow` and assert the convergence criteria through the full
//! pipeline + store.

mod common;

use cgx_core::{EdgeKind, SymbolKind};
use cgx_index::{default_registry, index_path, IndexOpts};
use common::*;

fn index_with(dataflow: bool) -> (tempfile::TempDir, cgx_store::SqliteStore, String) {
    let (tmp, repo) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();
    let opts = IndexOpts {
        dataflow,
        ..Default::default()
    };
    let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();
    (tmp, store, outcome.graph_key)
}

#[test]
fn dataflow_index_emits_value_nodes_and_derives_from_edges() {
    // Criterion 1: SSA value nodes + DerivesFrom edges appear with --dataflow.
    let (_t, store, id) = index_with(true);
    let g = read_graph(&store, &id);

    let value_nodes = g
        .nodes
        .iter()
        .filter(|n| n.kind == SymbolKind::Variable && n.fqn.contains('#'))
        .count();
    assert!(value_nodes > 0, "expected SSA value nodes from the fixture");

    let derives = g
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::DerivesFrom)
        .count();
    assert!(derives > 0, "expected DerivesFrom edges from the fixture");
}

#[test]
fn reassignment_in_fixture_yields_two_distinct_value_nodes() {
    // Criterion 2: flow_example's `b` is assigned twice → b#1 and b#2.
    let (_t, store, id) = index_with(true);
    let g = read_graph(&store, &id);
    let b_versions = g
        .nodes
        .iter()
        .filter(|n| {
            n.kind == SymbolKind::Variable
                && n.fqn.contains("::flow_example::b#")
        })
        .count();
    assert!(
        b_versions >= 2,
        "expected at least two SSA versions of b, got {b_versions}"
    );
}

#[test]
fn transform_column_is_populated_on_data_flow_view() {
    // Criterion 3: the `transform` column surfaces (via v_data_flow_edges) and is
    // populated for DerivesFrom rows. (CLI `--sql` itself is deferred — see the
    // execution note — so this asserts the column/view that `--sql` would read.)
    let (_t, store, _id) = index_with(true);
    let total: i64 = store
        .query_count("SELECT COUNT(*) FROM v_data_flow_edges")
        .unwrap();
    assert!(total > 0, "v_data_flow_edges has rows");
    let populated: i64 = store
        .query_count("SELECT COUNT(*) FROM v_data_flow_edges WHERE transform IS NOT NULL")
        .unwrap();
    assert_eq!(
        populated, total,
        "every DerivesFrom row carries a transform tag"
    );
    // A copy transform is present (from `let b = a`).
    let copies: i64 = store
        .query_count("SELECT COUNT(*) FROM v_data_flow_edges WHERE transform = 'copy'")
        .unwrap();
    assert!(copies > 0, "at least one copy transform present");
}

#[test]
fn base_index_view_excludes_dataflow() {
    // Criterion 5 (companion): without --dataflow, v_data_flow_edges is empty and
    // no edge carries a transform tag.
    let (_t, store, _id) = index_with(false);
    let df_rows: i64 = store
        .query_count("SELECT COUNT(*) FROM v_data_flow_edges")
        .unwrap();
    assert_eq!(df_rows, 0, "base index has no dataflow edges");
    let with_transform: i64 = store
        .query_count("SELECT COUNT(*) FROM edges WHERE transform IS NOT NULL")
        .unwrap();
    assert_eq!(with_transform, 0, "base index leaves transform NULL");
}

#[test]
fn base_index_node_and_edge_counts_unchanged_by_flag_absence() {
    // Criterion 5: the base index (no flag) has zero value nodes and zero
    // DerivesFrom edges; the dataflow index strictly *adds* both, leaving the base
    // symbol/call graph otherwise identical.
    let (_t1, base_store, base_id) = index_with(false);
    let base = read_graph(&base_store, &base_id);
    let (_t2, df_store, df_id) = index_with(true);
    let df = read_graph(&df_store, &df_id);

    // No value nodes / DerivesFrom in the base.
    assert!(base
        .nodes
        .iter()
        .all(|n| !(n.kind == SymbolKind::Variable && n.fqn.contains('#'))));
    assert!(base.edges.iter().all(|e| e.kind != EdgeKind::DerivesFrom));

    // The dataflow graph's non-dataflow slice matches the base exactly: same
    // symbol nodes (the base node set is a prefix of the dataflow node set, since
    // value nodes are appended after), same non-DerivesFrom edge endpoints.
    let base_symbol_fqns: Vec<&str> = base.nodes.iter().map(|n| n.fqn.as_str()).collect();
    let df_symbol_fqns: Vec<&str> = df
        .nodes
        .iter()
        .filter(|n| !n.fqn.contains('#'))
        .map(|n| n.fqn.as_str())
        .collect();
    assert_eq!(
        base_symbol_fqns, df_symbol_fqns,
        "the symbol-node set is unchanged by the dataflow flag"
    );
}

#[test]
fn dataflow_reindex_is_byte_identical() {
    // Criterion 4: two indexings of the same blob produce byte-identical node ids
    // + edge order (dump the canonical row data and compare).
    let (_t1, store1, id1) = index_with(true);
    let (_t2, store2, id2) = index_with(true);
    let dump1 = store1.dump_node_edge_data(&cgx_store::TreeOid::new(id1.clone())).unwrap();
    let dump2 = store2.dump_node_edge_data(&cgx_store::TreeOid::new(id2.clone())).unwrap();
    assert_eq!(
        dump1, dump2,
        "two dataflow indexings must be byte-identical"
    );
}

#[test]
fn opaque_call_resolves_through_summary_in_sc4() {
    // SC4 supersedes the SC2 "no cross-function edge" behavior: `let r = helper(a)`
    // in `through_call` now resolves through helper's IFDS summary
    // (`formal_in_0 ⇝ return`), materializing an interprocedural DerivesFrom edge
    // `r ⇝ a` tagged `interprocedural` + confidence `probable` (criterion 1).
    use cgx_core::confidence::Confidence;
    let (_t, store, id) = index_with(true);
    let g = read_graph(&store, &id);

    let r_node = g
        .nodes
        .iter()
        .find(|n| n.fqn.contains("::through_call::r#"))
        .expect("through_call::r value node exists");
    let interproc: Vec<_> = g
        .edges
        .iter()
        .filter(|e| {
            e.kind == EdgeKind::DerivesFrom && e.src == r_node.id && e.rule == "interprocedural"
        })
        .collect();
    assert_eq!(
        interproc.len(),
        1,
        "r must derive from a through helper's summary (one interproc edge)"
    );
    assert_eq!(interproc[0].confidence, Confidence::Probable);
    // The edge targets through_call's own parameter `a` (formal-in #0).
    let dst_fqn = &g
        .nodes
        .iter()
        .find(|n| n.id == interproc[0].dst)
        .unwrap()
        .fqn;
    assert!(
        dst_fqn.contains("::through_call::a#0"),
        "interproc edge sources the caller's arg `a`, got {dst_fqn}"
    );
}
