//! WP-07 convergence tests: round-trip identity, blob-OID dedup/no-op re-put,
//! view-schema-version present, determinism, and prune correctness.

mod common;

use cgx_core::codec::encode;
use cgx_store::{
    BlobOid, BlobSet, FactStore, FragmentInput, LinkedGraph, SqliteStore, TreeOid, SCHEMA_VERSION,
    VIEW_SCHEMA_VERSION, VIEW_SET,
};
use common::sample_graph;

fn open() -> SqliteStore {
    SqliteStore::open_in_memory().expect("open in-memory store")
}

fn frag<'a>(blob: &'a BlobOid, bytes: &'a [u8]) -> FragmentInput<'a> {
    FragmentInput {
        blob,
        lang: "rust",
        frontend_version: 1,
        bytes,
    }
}

// ---- Round-trip identity ----------------------------------------------------

#[test]
fn linked_graph_round_trips_identically() {
    let mut store = open();
    let g = sample_graph();
    let tree = TreeOid::new("tree-aaa");

    store.put_graph(&tree, Some("rev-1"), &g).unwrap();
    let back = store.read_graph(&tree).unwrap();

    assert_eq!(g, back, "graph read back must equal the graph written");
}

#[test]
fn own_effects_survive_the_round_trip() {
    use cgx_core::Effect;
    let mut store = open();
    let g = sample_graph();
    let tree = TreeOid::new("tree-fx");
    store.put_graph(&tree, None, &g).unwrap();
    let back = store.read_graph(&tree).unwrap();

    // node1 in the sample graph carries io.file + nondeterministic.
    let n1 = back.nodes.iter().find(|n| n.id.0 == 1).expect("node 1");
    assert!(n1.own_effects.contains(Effect::IoFile));
    assert!(n1.own_effects.contains(Effect::Nondeterministic));
    assert_eq!(n1.own_effects.len(), 2);
    // transitive_effects is P8b's job: unpopulated in Phase 1.
    assert!(n1.transitive_effects.is_empty());
}

#[test]
fn own_effects_column_is_denormalized_for_sql() {
    let mut store = open();
    store
        .put_graph(&TreeOid::new("tree-fxcol"), None, &sample_graph())
        .unwrap();
    // The canonical-string projection is queryable via the v_symbols view.
    let n = store
        .query_count(
            "SELECT COUNT(*) FROM v_symbols WHERE own_effects = 'io.file,nondeterministic'",
        )
        .unwrap();
    assert_eq!(
        n, 1,
        "denormalized own_effects column reflects the label list"
    );
}

#[test]
fn empty_graph_round_trips() {
    let mut store = open();
    let g = LinkedGraph::default();
    let tree = TreeOid::new("tree-empty");
    store.put_graph(&tree, None, &g).unwrap();
    assert_eq!(store.read_graph(&tree).unwrap(), g);
}

#[test]
fn graph_for_reports_existence_after_put() {
    let mut store = open();
    let tree = TreeOid::new("tree-lookup");
    assert!(!store.graph_for(&tree).unwrap());
    store.put_graph(&tree, None, &sample_graph()).unwrap();
    assert!(store.graph_for(&tree).unwrap());
}

#[test]
fn re_putting_same_tree_replaces_rows_without_appending() {
    let mut store = open();
    let tree = TreeOid::new("tree-reput");
    store.put_graph(&tree, None, &sample_graph()).unwrap();
    // A second put of the same tree replaces (not appends): the symbol row count
    // stays at one graph's worth, and the read-back is the single sample graph.
    let symbols_after_first = store
        .query_count("SELECT COUNT(*) FROM nodes")
        .unwrap();
    store.put_graph(&tree, None, &sample_graph()).unwrap();
    let symbols_after_second = store
        .query_count("SELECT COUNT(*) FROM nodes")
        .unwrap();
    assert_eq!(
        symbols_after_first, symbols_after_second,
        "re-putting the same tree OID must replace its rows, not append a second graph"
    );
    assert_eq!(store.read_graph(&tree).unwrap(), sample_graph());
}

// ---- Blob-OID fragment CRUD + no-op re-put ----------------------------------

#[test]
fn fragment_put_and_get_round_trips_bytes() {
    let mut store = open();
    let blob = BlobOid::new("blob-1");
    let bytes = encode(&sample_graph().nodes[0]).unwrap();

    store.put_fragments(&[frag(&blob, &bytes)]).unwrap();
    let got = store.fragment(&blob).unwrap().expect("fragment present");

    assert_eq!(got.fragment, bytes, "stored bytes returned verbatim");
    assert_eq!(got.lang, "rust");
    assert_eq!(got.frontend_version, 1);
}

#[test]
fn fragment_absent_returns_none() {
    let store = open();
    assert!(store.fragment(&BlobOid::new("nope")).unwrap().is_none());
}

#[test]
fn re_putting_unchanged_blob_is_a_no_op() {
    let mut store = open();
    let blob = BlobOid::new("blob-stable");
    let bytes = b"canonical-fragment-bytes".to_vec();

    store.put_fragments(&[frag(&blob, &bytes)]).unwrap();
    // Re-put the same blob OID: content-addressed, so the row must be untouched.
    store.put_fragments(&[frag(&blob, &bytes)]).unwrap();

    let got = store.fragment(&blob).unwrap().unwrap();
    assert_eq!(got.fragment, bytes);
}

#[test]
fn re_put_does_not_overwrite_existing_bytes_for_same_oid() {
    // A blob OID is content-addressed: the first stored bytes are authoritative.
    // A no-op ON CONFLICT means a second put with different bytes is ignored.
    let mut store = open();
    let blob = BlobOid::new("blob-cad");
    store.put_fragments(&[frag(&blob, b"original")]).unwrap();
    store.put_fragments(&[frag(&blob, b"different")]).unwrap();
    assert_eq!(
        store.fragment(&blob).unwrap().unwrap().fragment,
        b"original"
    );
}

// ---- View-schema-version present + views queryable --------------------------

#[test]
fn view_schema_version_is_present_in_cgx_meta_view() {
    let store = open();
    assert_eq!(store.view_schema_version().unwrap(), VIEW_SCHEMA_VERSION);
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn all_documented_views_exist_and_are_queryable() {
    let mut store = open();
    store
        .put_graph(&TreeOid::new("tree-views"), None, &sample_graph())
        .unwrap();

    // Each ADR-05 view must be selectable. Use the connection via a raw query
    // path: open the same store's views through the public read surface.
    for view in VIEW_SET {
        let sql = format!("SELECT COUNT(*) FROM {view}");
        let n = store.query_count(&sql).unwrap();
        // cgx_meta has >=2 rows (schema + view version); content views have >=0.
        if *view == "cgx_meta" {
            assert!(n >= 2, "cgx_meta should expose version rows");
        }
    }
}

#[test]
fn cgx_meta_view_exposes_view_schema_version_key() {
    let store = open();
    let v = store
        .query_meta_value("view_schema_version")
        .unwrap()
        .expect("view_schema_version row present");
    assert_eq!(v, VIEW_SCHEMA_VERSION.to_string());
}

#[test]
fn v_call_edges_projects_caller_callee() {
    let mut store = open();
    store
        .put_graph(&TreeOid::new("tree-edges"), None, &sample_graph())
        .unwrap();
    // Two edges in the sample graph.
    let n = store
        .query_count("SELECT COUNT(*) FROM v_call_edges")
        .unwrap();
    assert_eq!(n, 2);
}

// ---- Determinism: two writes -> identical rows / bytes -----------------------

#[test]
fn two_writes_of_same_graph_produce_identical_row_bytes() {
    let mut a = open();
    let mut b = open();
    let g = sample_graph();

    let tree = TreeOid::new("t");
    a.put_graph(&tree, None, &g).unwrap();
    b.put_graph(&tree, None, &g).unwrap();

    let dump_a = a.dump_node_edge_data(&tree).unwrap();
    let dump_b = b.dump_node_edge_data(&tree).unwrap();
    assert_eq!(
        dump_a, dump_b,
        "two stores writing the same graph must hold byte-identical row data"
    );
}

#[test]
fn round_trip_then_re_encode_is_byte_identical() {
    let mut store = open();
    let g = sample_graph();
    let tree = TreeOid::new("t");
    store.put_graph(&tree, None, &g).unwrap();
    let back = store.read_graph(&tree).unwrap();

    // Re-encoding original and round-tripped records yields identical bytes.
    for (orig, rt) in g.nodes.iter().zip(back.nodes.iter()) {
        assert_eq!(encode(orig).unwrap(), encode(rt).unwrap());
    }
    for (orig, rt) in g.edges.iter().zip(back.edges.iter()) {
        assert_eq!(encode(orig).unwrap(), encode(rt).unwrap());
    }
}

// ---- Prune correctness (IX-5) -----------------------------------------------

#[test]
fn prune_removes_orphaned_blobs_and_keeps_live_ones() {
    let mut store = open();
    let live_blob = BlobOid::new("live");
    let dead_blob = BlobOid::new("dead");
    store
        .put_fragments(&[frag(&live_blob, b"a"), frag(&dead_blob, b"b")])
        .unwrap();

    let mut live: BlobSet = BlobSet::new();
    live.insert(live_blob.clone());

    let stats = store.prune(&live, false).unwrap();
    assert_eq!(stats.fragments_removed, 1);
    assert!(!stats.compacted);
    assert!(store.fragment(&live_blob).unwrap().is_some());
    assert!(store.fragment(&dead_blob).unwrap().is_none());
}

#[test]
fn prune_aggressive_compacts() {
    // VACUUM cannot run inside a transaction or on an in-memory db reliably for
    // page reclaim, but the flag and the operation must succeed on a file db.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = SqliteStore::open(tmp.path().join("index.db")).unwrap();
    let blob = BlobOid::new("x");
    store.put_fragments(&[frag(&blob, b"payload")]).unwrap();

    let mut live = BlobSet::new();
    live.insert(blob);
    let stats = store.prune(&live, true).unwrap();
    assert!(stats.compacted);
}

#[test]
fn shared_blob_survives_when_still_live() {
    let mut store = open();
    let shared = BlobOid::new("shared");
    store.put_fragments(&[frag(&shared, b"s")]).unwrap();

    let mut live = BlobSet::new();
    live.insert(shared.clone());
    store.prune(&live, false).unwrap();
    assert!(store.fragment(&shared).unwrap().is_some());
}

// ---- Layer-2 graph GC (blast-radius P1) -------------------------------------

#[test]
fn prune_graphs_except_keeps_live_graph_and_removes_the_rest() {
    let mut store = open();
    let t1 = TreeOid::new("tree-gone-1");
    let t2 = TreeOid::new("tree-gone-2");
    let t3 = TreeOid::new("tree-keep");
    store.put_graph(&t1, None, &sample_graph()).unwrap();
    store.put_graph(&t2, None, &sample_graph()).unwrap();
    store.put_graph(&t3, None, &sample_graph()).unwrap();

    let removed = store.prune_graphs_except(&[&t3]).unwrap();
    assert_eq!(removed, 2, "the two superseded graphs are GC'd");

    // The kept graph still loads correctly (read-path regression guard).
    assert_eq!(store.read_graph(&t3).unwrap(), sample_graph());
    assert!(!store.graph_for(&t1).unwrap());
    assert!(!store.graph_for(&t2).unwrap());
    assert!(store.graph_for(&t3).unwrap());

    // Cascade reclaimed the dead graphs' node rows (no orphans): only the kept
    // graph's nodes survive, so the total node count is exactly one graph's worth.
    let total_nodes = store.query_count("SELECT COUNT(*) FROM nodes").unwrap();
    assert_eq!(
        total_nodes,
        sample_graph().nodes.len() as i64,
        "FK cascade removed dead graphs' nodes"
    );
}

#[test]
fn prune_graphs_except_can_retain_multiple_graphs() {
    // The diff session model: two tree-OIDs must survive when both are kept.
    let mut store = open();
    let a = TreeOid::new("tree-a");
    let b = TreeOid::new("tree-b");
    let c = TreeOid::new("tree-c");
    store.put_graph(&a, None, &sample_graph()).unwrap();
    store.put_graph(&b, None, &sample_graph()).unwrap();
    store.put_graph(&c, None, &sample_graph()).unwrap();

    let removed = store.prune_graphs_except(&[&a, &b]).unwrap();
    assert_eq!(removed, 1);
    assert_eq!(store.read_graph(&a).unwrap(), sample_graph());
    assert_eq!(store.read_graph(&b).unwrap(), sample_graph());
    assert!(!store.graph_for(&c).unwrap());
}

#[test]
fn prune_graphs_except_is_a_noop_when_nothing_is_stale() {
    let mut store = open();
    let t = TreeOid::new("tree-only");
    store.put_graph(&t, None, &sample_graph()).unwrap();
    assert_eq!(store.prune_graphs_except(&[&t]).unwrap(), 0);
}

// ---- Keyed-delta cache writes (blast-radius P2) -----------------------------

#[test]
fn fn_intraproc_cache_keyed_delta_preserves_unchanged_and_drops_stale() {
    let mut store = open();
    let r = |b: &str, f: &str, h: &str| ((b.to_string(), f.to_string()), h.to_string());
    store
        .put_fn_intraproc_cache(&[r("blob1", "f::a", "h1"), r("blob1", "f::b", "h2")])
        .unwrap();

    // Re-index: f::a unchanged, f::b changed, f::c new, (the old set's nothing dropped yet).
    store
        .put_fn_intraproc_cache(&[
            r("blob1", "f::a", "h1"),
            r("blob1", "f::b", "h2-new"),
            r("blob1", "f::c", "h3"),
        ])
        .unwrap();
    let mut got = store.load_fn_intraproc_cache().unwrap();
    got.sort();
    assert_eq!(
        got,
        vec![
            (("blob1".into(), "f::a".into()), "h1".into()),
            (("blob1".into(), "f::b".into()), "h2-new".into()),
            (("blob1".into(), "f::c".into()), "h3".into()),
        ]
    );

    // Drop f::b entirely on the next index: only the stale key is removed.
    store
        .put_fn_intraproc_cache(&[r("blob1", "f::a", "h1"), r("blob1", "f::c", "h3")])
        .unwrap();
    let mut got = store.load_fn_intraproc_cache().unwrap();
    got.sort();
    assert_eq!(
        got,
        vec![
            (("blob1".into(), "f::a".into()), "h1".into()),
            (("blob1".into(), "f::c".into()), "h3".into()),
        ]
    );
}

#[test]
fn summary_deps_keyed_delta_round_trips() {
    let mut store = open();
    let d = |f: &str, c: &str| (f.to_string(), c.to_string());
    store.put_summary_deps(&[d("f", "g"), d("f", "*")]).unwrap();
    // Re-put with one removed, one added.
    store.put_summary_deps(&[d("f", "g"), d("f", "h")]).unwrap();
    assert_eq!(
        store
            .query_count("SELECT COUNT(*) FROM summary_deps")
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .query_count("SELECT COUNT(*) FROM summary_deps WHERE callee_fqn = '*'")
            .unwrap(),
        0,
        "stale wildcard dep removed"
    );
}

#[test]
fn fn_summaries_keyed_delta_preserves_unchanged_bytes() {
    let mut store = open();
    let r = |b: &str, f: &str, s: &[u8]| ((b.to_string(), f.to_string()), s.to_vec());
    store
        .put_fn_summaries(&[r("b", "f::a", b"sumA"), r("b", "f::b", b"sumB")])
        .unwrap();
    store
        .put_fn_summaries(&[r("b", "f::a", b"sumA"), r("b", "f::b", b"sumB-2")])
        .unwrap();
    let mut got = store.load_fn_summaries().unwrap();
    got.sort();
    assert_eq!(
        got,
        vec![
            (("b".into(), "f::a".into()), b"sumA".to_vec()),
            (("b".into(), "f::b".into()), b"sumB-2".to_vec()),
        ]
    );
}

/// Physical byte-stability: re-indexing an unchanged cache set on a file-backed
/// store leaves the db bytes identical (the keyed-delta path performs no writes
/// when nothing changed). This is the blast-radius win, measured.
#[test]
fn unchanged_cache_reindex_leaves_db_bytes_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");
    let r = |b: &str, f: &str, h: &str| ((b.to_string(), f.to_string()), h.to_string());
    let rows = [
        r("b", "f::a", "h1"),
        r("b", "f::b", "h2"),
        r("b", "f::c", "h3"),
    ];

    {
        let mut store = SqliteStore::open(&db).unwrap();
        store.put_fn_intraproc_cache(&rows).unwrap();
    } // drop checkpoints WAL into the main db file

    let before = std::fs::read(&db).unwrap();
    {
        let mut store = SqliteStore::open(&db).unwrap();
        store.put_fn_intraproc_cache(&rows).unwrap();
    }
    let after = std::fs::read(&db).unwrap();
    assert_eq!(
        before, after,
        "re-indexing an unchanged cache set must not rewrite the db"
    );
}

// ---- Property: round-trip identity over arbitrary graphs --------------------

use cgx_core::{
    Confidence, CutMarkers, EdgeCondition, EdgeId, EdgeKind, EdgeRecord, NodeId, NodeRecord,
    SymbolKind, Tier, Visibility,
};
use proptest::prelude::*;

prop_compose! {
    fn arb_node(idx: u32)(
        line_start in 1u32..10_000,
        len in 0u32..200,
        fqn in "[a-z]{1,8}(::[a-z]{1,8}){0,3}",
    ) -> NodeRecord {
        NodeRecord {
            id: NodeId(idx),
            kind: SymbolKind::Function,
            fqn,
            file: format!("src/f{}.rs", idx % 5),
            line_start,
            line_end: line_start + len,
            lang: "rust".into(),
            visibility: Visibility::Private,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
            unresolved_calls: 0,
        }
    }
}

prop_compose! {
    fn arb_graph()(n in 1usize..12)(
        nodes in (0..n).map(|i| arb_node(i as u32)).collect::<Vec<_>>(),
        edge_specs in proptest::collection::vec((0u32..n as u32, 0u32..n as u32), 0..20),
    ) -> LinkedGraph {
        let edges = edge_specs
            .into_iter()
            .enumerate()
            .map(|(i, (src, dst))| EdgeRecord {
                id: EdgeId(i as u32),
                src: NodeId(src),
                dst: NodeId(dst),
                kind: EdgeKind::Calls,
                condition: EdgeCondition::Always,
                confidence: Confidence::Probable,
                tier: Tier::NameSyntactic,
                rule: "name-arity".into(),
                site_id: None,
                stmt_index: None,
                cut_markers: CutMarkers::new(),
                implicit: None,
                candidate_group: None,
                established_by: None,
                cfg_condition: None,
                macro_origin: None,
                transform: None,
            })
            .collect();
        LinkedGraph::new(nodes, edges, vec![])
    }
}

proptest! {
    /// Any graph written then read back is identical (round-trip identity).
    #[test]
    fn prop_graph_round_trips(g in arb_graph()) {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let tree = TreeOid::new("t");
        store.put_graph(&tree, None, &g).unwrap();
        let back = store.read_graph(&tree).unwrap();
        prop_assert_eq!(g, back);
    }

    /// Two independent stores writing the same graph hold byte-identical row data
    /// (determinism).
    #[test]
    fn prop_two_writes_are_byte_identical(g in arb_graph()) {
        let mut a = SqliteStore::open_in_memory().unwrap();
        let mut b = SqliteStore::open_in_memory().unwrap();
        let tree = TreeOid::new("t");
        a.put_graph(&tree, None, &g).unwrap();
        b.put_graph(&tree, None, &g).unwrap();
        prop_assert_eq!(
            a.dump_node_edge_data(&tree).unwrap(),
            b.dump_node_edge_data(&tree).unwrap()
        );
    }
}
