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

    let id = store.put_graph(&tree, Some("rev-1"), &g).unwrap();
    let back = store.read_graph(id).unwrap();

    assert_eq!(g, back, "graph read back must equal the graph written");
}

#[test]
fn empty_graph_round_trips() {
    let mut store = open();
    let g = LinkedGraph::default();
    let id = store
        .put_graph(&TreeOid::new("tree-empty"), None, &g)
        .unwrap();
    assert_eq!(store.read_graph(id).unwrap(), g);
}

#[test]
fn graph_for_returns_id_after_put() {
    let mut store = open();
    let tree = TreeOid::new("tree-lookup");
    assert!(store.graph_for(&tree).unwrap().is_none());
    let id = store.put_graph(&tree, None, &sample_graph()).unwrap();
    assert_eq!(store.graph_for(&tree).unwrap(), Some(id));
}

#[test]
fn re_putting_same_tree_reuses_graph_id_and_replaces_rows() {
    let mut store = open();
    let tree = TreeOid::new("tree-reput");
    let id1 = store.put_graph(&tree, None, &sample_graph()).unwrap();
    let id2 = store.put_graph(&tree, None, &sample_graph()).unwrap();
    assert_eq!(id1, id2, "same tree OID must reuse its graph row");
    assert_eq!(store.read_graph(id2).unwrap(), sample_graph());
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

    let ida = a.put_graph(&TreeOid::new("t"), None, &g).unwrap();
    let idb = b.put_graph(&TreeOid::new("t"), None, &g).unwrap();

    let dump_a = a.dump_node_edge_data(ida).unwrap();
    let dump_b = b.dump_node_edge_data(idb).unwrap();
    assert_eq!(
        dump_a, dump_b,
        "two stores writing the same graph must hold byte-identical row data"
    );
}

#[test]
fn round_trip_then_re_encode_is_byte_identical() {
    let mut store = open();
    let g = sample_graph();
    let id = store.put_graph(&TreeOid::new("t"), None, &g).unwrap();
    let back = store.read_graph(id).unwrap();

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
        let id = store.put_graph(&TreeOid::new("t"), None, &g).unwrap();
        let back = store.read_graph(id).unwrap();
        prop_assert_eq!(g, back);
    }

    /// Two independent stores writing the same graph hold byte-identical row data
    /// (determinism).
    #[test]
    fn prop_two_writes_are_byte_identical(g in arb_graph()) {
        let mut a = SqliteStore::open_in_memory().unwrap();
        let mut b = SqliteStore::open_in_memory().unwrap();
        let ida = a.put_graph(&TreeOid::new("t"), None, &g).unwrap();
        let idb = b.put_graph(&TreeOid::new("t"), None, &g).unwrap();
        prop_assert_eq!(
            a.dump_node_edge_data(ida).unwrap(),
            b.dump_node_edge_data(idb).unwrap()
        );
    }
}
