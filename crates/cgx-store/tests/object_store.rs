//! Behavioral tests for the content-addressed [`ObjectStore`] (Slice-1 Candidate
//! A). Exercises round-trip byte-identity, object-level determinism (repeat-run,
//! not golden), canonical manifest/candidate ordering, per-function dedup, GC of
//! unreferenced objects, and the atomic-replace / linearization-point invariant.

mod common;

use cgx_core::codec::{decode, encode};
use cgx_core::{
    Candidate, Confidence, CutMarkers, EdgeCondition, EdgeId, EdgeKind, EdgeRecord, NodeId,
    NodeRecord, SymbolKind, Tier, Visibility,
};
use cgx_store::manifest::{Manifest, CURRENT_STORE_FORMAT};
use cgx_store::{FactStore, LinkedGraph, ObjectOid, ObjectStore, SqliteStore, StoreError, TreeOid};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

// --- helpers -------------------------------------------------------------------

fn store(root: &TempDir) -> ObjectStore {
    ObjectStore::open(root.path().join(".cgx")).expect("open object store")
}

fn objects_dir(root: &TempDir) -> PathBuf {
    root.path().join(".cgx").join("objects")
}

/// Every object OID present under `.cgx/objects` (the file-name fan-out), ignoring
/// transient `.tmp` sidecars.
fn object_oids(root: &TempDir) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let dir = objects_dir(root);
    let Ok(rd) = fs::read_dir(&dir) else {
        return out;
    };
    for shard in rd.flatten() {
        let prefix = shard.file_name().to_string_lossy().to_string();
        if prefix.len() != 2 {
            continue;
        }
        for f in fs::read_dir(shard.path()).unwrap().flatten() {
            let rest = f.file_name().to_string_lossy().to_string();
            if !rest.ends_with(".tmp") {
                out.insert(format!("{prefix}{rest}"));
            }
        }
    }
    out
}

/// Decode the live manifest for `key` by walking the public ref/object layout.
fn read_manifest(root: &TempDir, key: &str) -> Manifest {
    let ref_path = root
        .path()
        .join(".cgx")
        .join("refs")
        .join(ObjectOid::of_bytes(key.as_bytes()).0);
    let contents = fs::read_to_string(&ref_path).expect("ref file exists");
    let moid = contents.lines().next().unwrap().trim().to_owned();
    let path = ObjectOid(moid).object_path(&objects_dir(root));
    decode::<Manifest>(&fs::read(path).unwrap()).unwrap()
}

/// The shard OID for a given owning-function key in a tree's live manifest.
fn shard_oid(root: &TempDir, key: &str, fn_key: &str) -> String {
    read_manifest(root, key)
        .shards
        .iter()
        .find(|s| s.fn_key == fn_key)
        .unwrap_or_else(|| panic!("no shard for {fn_key}"))
        .oid
        .0
        .clone()
}

fn func_node(id: u32, fqn: &str, line_end: u32) -> NodeRecord {
    NodeRecord {
        id: NodeId(id),
        kind: SymbolKind::Function,
        fqn: fqn.into(),
        file: "src/lib.rs".into(),
        line_start: id + 1,
        line_end,
        lang: "rust".into(),
        visibility: Visibility::Public,
        is_abstract: false,
        entrypoint_kind: None,
        signature: None,
        own_effects: cgx_core::EffectSet::new(),
        transitive_effects: cgx_core::EffectSet::new(),
        unresolved_calls: 0,
    }
}

fn call_edge(id: u32, src: u32, dst: u32) -> EdgeRecord {
    EdgeRecord {
        id: EdgeId(id),
        src: NodeId(src),
        dst: NodeId(dst),
        kind: EdgeKind::Calls,
        condition: EdgeCondition::Always,
        confidence: Confidence::Probable,
        tier: Tier::ScopeGraph,
        rule: "call".into(),
        site_id: None,
        stmt_index: None,
        cut_markers: CutMarkers::new(),
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
        transform: None,
    }
}

// --- 1. round-trip byte-identity ----------------------------------------------

#[test]
fn read_graph_round_trips_byte_identically() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    let tree = TreeOid::new("t1");
    let g = common::sample_graph();

    s.put_graph(&tree, Some("rev1"), &g).unwrap();
    let back = s.read_graph(&tree).unwrap();

    assert_eq!(back, g, "read_graph must reconstruct the exact LinkedGraph");
}

#[test]
fn absent_tree_reads_as_empty_graph() {
    let root = TempDir::new().unwrap();
    let s = store(&root);
    assert!(!s.graph_for(&TreeOid::new("missing")).unwrap());
    assert_eq!(
        s.read_graph(&TreeOid::new("missing")).unwrap(),
        LinkedGraph::default()
    );
}

// --- shadow parity: object-store read == SQLite read (criterion 8) ------------

/// The shadow read path must lose nothing versus the incumbent: the same tree,
/// put into a fresh [`SqliteStore`] and a fresh [`ObjectStore`], reads back as a
/// byte-identical [`LinkedGraph`] — nodes/edges by dense id, candidates in
/// canonical `(group, rank, dst)` order. `sample_graph()` is the rich corpus
/// fixture (virtual calls, a candidate set, effects, cut markers) the existing
/// store tests exercise.
#[test]
fn object_store_read_matches_sqlite_read_for_the_same_tree() {
    let root = TempDir::new().unwrap();
    let mut objects = store(&root);
    let mut sqlite = SqliteStore::open_in_memory().unwrap();
    let tree = TreeOid::new("t-parity");
    let g = common::sample_graph();

    sqlite.put_graph(&tree, Some("rev1"), &g).unwrap();
    objects.put_graph(&tree, Some("rev1"), &g).unwrap();

    assert_eq!(
        objects.read_graph(&tree).unwrap(),
        sqlite.read_graph(&tree).unwrap(),
        "object-store read must equal SQLite read for the same tree"
    );
}

// --- 2. object-level determinism (repeat-run, not golden) ---------------------

#[test]
fn same_graph_into_two_fresh_stores_yields_identical_objects() {
    let root_a = TempDir::new().unwrap();
    let root_b = TempDir::new().unwrap();
    let g = common::sample_graph();
    let tree = TreeOid::new("deadbeef");

    store(&root_a).put_graph(&tree, Some("rev"), &g).unwrap();
    store(&root_b).put_graph(&tree, Some("rev"), &g).unwrap();

    let oids_a = object_oids(&root_a);
    let oids_b = object_oids(&root_b);
    assert_eq!(oids_a, oids_b, "object OID sets must be identical across runs");

    // Every object's bytes are byte-identical between the two independent runs.
    for oid in &oids_a {
        let p = ObjectOid(oid.clone());
        let ba = fs::read(p.object_path(&objects_dir(&root_a))).unwrap();
        let bb = fs::read(p.object_path(&objects_dir(&root_b))).unwrap();
        assert_eq!(ba, bb, "object {oid} bytes differ across runs");
    }
}

#[test]
fn re_put_of_same_graph_is_a_noop_at_the_object_level() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    let tree = TreeOid::new("t1");
    let g = common::sample_graph();

    s.put_graph(&tree, Some("rev"), &g).unwrap();
    let first = object_oids(&root);
    let manifest_first = read_manifest(&root, "t1");

    s.put_graph(&tree, Some("rev"), &g).unwrap();
    let second = object_oids(&root);
    let manifest_second = read_manifest(&root, "t1");

    assert_eq!(first, second);
    assert_eq!(manifest_first, manifest_second);
}

// --- store_format compat gate (D-3) -------------------------------------------

#[test]
fn manifest_is_stamped_with_current_store_format() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    s.put_graph(&TreeOid::new("t1"), Some("rev"), &common::sample_graph())
        .unwrap();

    assert_eq!(read_manifest(&root, "t1").store_format, CURRENT_STORE_FORMAT);
}

#[test]
fn read_rejects_manifest_with_newer_store_format() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    let tree = TreeOid::new("t1");
    s.put_graph(&tree, Some("rev"), &common::sample_graph())
        .unwrap();

    // Forge a manifest claiming a future format, write it as a new object, and
    // repoint the ref at it — simulating a graph committed by a newer cgx.
    let mut forged = read_manifest(&root, "t1");
    forged.store_format = CURRENT_STORE_FORMAT + 1;
    let bytes = encode(&forged).unwrap();
    let moid = ObjectOid::of_bytes(&bytes);
    let opath = moid.object_path(&objects_dir(&root));
    fs::create_dir_all(opath.parent().unwrap()).unwrap();
    fs::write(&opath, &bytes).unwrap();
    let ref_path = root
        .path()
        .join(".cgx")
        .join("refs")
        .join(ObjectOid::of_bytes("t1".as_bytes()).0);
    fs::write(&ref_path, format!("{}\nt1\n", moid.0)).unwrap();

    match s.read_graph(&tree) {
        Err(StoreError::StoreFormat { found, expected }) => {
            assert_eq!(found, CURRENT_STORE_FORMAT + 1);
            assert_eq!(expected, CURRENT_STORE_FORMAT);
        }
        other => panic!("expected StoreFormat rejection, got {other:?}"),
    }
}

// --- 3. manifest + candidate canonical ordering -------------------------------

#[test]
fn manifest_shard_list_is_sorted_and_stable() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    let g = common::sample_graph();
    s.put_graph(&TreeOid::new("t1"), None, &g).unwrap();

    let m = read_manifest(&root, "t1");
    let keys: Vec<&str> = m.shards.iter().map(|e| e.fn_key.as_str()).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted, "shard entries must be in canonical fn_key order");
}

#[test]
fn candidates_are_reconstructed_in_group_rank_dst_order() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);

    // Two callables so there is a candidate set; candidates supplied scrambled.
    let nodes = vec![
        func_node(0, "app::caller", 10),
        func_node(1, "app::target_a", 20),
        func_node(2, "app::target_b", 30),
    ];
    let edges = vec![call_edge(0, 0, 1), call_edge(1, 0, 2)];
    let scrambled = vec![
        Candidate { candidate_group: 0, dst: NodeId(2), rank: 1 },
        Candidate { candidate_group: 0, dst: NodeId(1), rank: 0 },
    ];
    let g = LinkedGraph::new(nodes, edges, scrambled);
    s.put_graph(&TreeOid::new("t1"), None, &g).unwrap();

    let back = s.read_graph(&TreeOid::new("t1")).unwrap();
    assert_eq!(
        back.candidates,
        vec![
            Candidate { candidate_group: 0, dst: NodeId(1), rank: 0 },
            Candidate { candidate_group: 0, dst: NodeId(2), rank: 1 },
        ],
        "candidates must come back sorted by (group, rank, dst)"
    );
}

// --- 4. per-function dedup ----------------------------------------------------

#[test]
fn one_function_edit_shares_every_unchanged_functions_object() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);

    // tree1: f_a calls f_b. tree2: same f_b, but f_a's body changed (line_end).
    let base = LinkedGraph::new(
        vec![func_node(0, "app::f_a", 10), func_node(1, "app::f_b", 20)],
        vec![call_edge(0, 0, 1)],
        vec![],
    );
    let changed_f_a = LinkedGraph::new(
        vec![func_node(0, "app::f_a", 99), func_node(1, "app::f_b", 20)],
        vec![call_edge(0, 0, 1)],
        vec![],
    );

    s.put_graph(&TreeOid::new("t1"), None, &base).unwrap();
    s.put_graph(&TreeOid::new("t2"), None, &changed_f_a).unwrap();

    assert_eq!(
        shard_oid(&root, "t1", "app::f_b"),
        shard_oid(&root, "t2", "app::f_b"),
        "the unchanged function's object must be shared (same OID = free dedup)"
    );
    assert_ne!(
        shard_oid(&root, "t1", "app::f_a"),
        shard_oid(&root, "t2", "app::f_a"),
        "the edited function's object must differ"
    );

    // Both trees still round-trip.
    assert_eq!(s.read_graph(&TreeOid::new("t1")).unwrap(), base);
    assert_eq!(s.read_graph(&TreeOid::new("t2")).unwrap(), changed_f_a);
}

// --- 5. GC reaps only unreferenced objects ------------------------------------

#[test]
fn prune_graphs_except_reaps_only_unreferenced_objects() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);

    // tree1: {f_a, f_b}. tree2: {f_a, f_b, f_c} — f_a/f_b shards shared with tree1.
    let g1 = LinkedGraph::new(
        vec![func_node(0, "app::f_a", 10), func_node(1, "app::f_b", 20)],
        vec![call_edge(0, 0, 1)],
        vec![],
    );
    let g2 = LinkedGraph::new(
        vec![
            func_node(0, "app::f_a", 10),
            func_node(1, "app::f_b", 20),
            func_node(2, "app::f_c", 30),
        ],
        vec![call_edge(0, 0, 1)],
        vec![],
    );
    s.put_graph(&TreeOid::new("t1"), None, &g1).unwrap();
    s.put_graph(&TreeOid::new("t2"), None, &g2).unwrap();

    let shared_fb = shard_oid(&root, "t1", "app::f_b");
    let only_fc = shard_oid(&root, "t2", "app::f_c");
    assert!(object_oids(&root).contains(&shared_fb));
    assert!(object_oids(&root).contains(&only_fc));

    let t1 = TreeOid::new("t1");
    let removed = s.prune_graphs_except(&[&t1]).unwrap();
    assert_eq!(removed, 1, "exactly one graph (tree2) dropped");

    let survivors = object_oids(&root);
    assert!(
        survivors.contains(&shared_fb),
        "shared object referenced by the live manifest survives"
    );
    assert!(
        !survivors.contains(&only_fc),
        "object only referenced by the dropped manifest is gone"
    );

    // tree1 still reads; tree2 is gone.
    assert_eq!(s.read_graph(&t1).unwrap(), g1);
    assert!(!s.graph_for(&TreeOid::new("t2")).unwrap());
    assert_eq!(s.read_graph(&TreeOid::new("t2")).unwrap(), LinkedGraph::default());
}

// --- 6. atomic-replace / torn-write -------------------------------------------

#[test]
fn ref_is_the_linearization_point_orphan_objects_are_invisible() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    let tree = TreeOid::new("t1");
    let g = common::sample_graph();
    s.put_graph(&tree, Some("rev"), &g).unwrap();

    // Simulate a crash in the post-object / pre-ref-advance window: objects land
    // on disk but the ref never advances. A reader must still see the previous
    // consistent state, never a torn manifest.
    plant_orphan_object(&root, b"a-manifest-or-shard-that-was-never-committed");
    // And a half-written object never occupies a final OID name: a `.tmp` sidecar
    // is ignored entirely.
    plant_tmp_object(&root, b"partial-bytes");

    let back = s.read_graph(&tree).unwrap();
    assert_eq!(back, g, "read follows the ref, ignoring uncommitted objects");
}

#[test]
fn missing_ref_reads_as_empty_never_torn() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    let tree = TreeOid::new("t1");
    s.put_graph(&tree, None, &common::sample_graph()).unwrap();

    // Remove the ref (simulate an advance that never landed). The objects remain,
    // but with no ref the graph reads as absent — a consistent previous state.
    let ref_path = root
        .path()
        .join(".cgx")
        .join("refs")
        .join(ObjectOid::of_bytes(tree.as_str().as_bytes()).0);
    fs::remove_file(&ref_path).unwrap();

    assert!(!s.graph_for(&tree).unwrap());
    assert_eq!(s.read_graph(&tree).unwrap(), LinkedGraph::default());
}

#[test]
fn a_normal_put_leaves_no_tmp_sidecars() {
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    s.put_graph(&TreeOid::new("t1"), None, &common::sample_graph())
        .unwrap();

    let mut tmp_count = 0;
    for shard in fs::read_dir(objects_dir(&root)).unwrap().flatten() {
        for f in fs::read_dir(shard.path()).unwrap().flatten() {
            if f.file_name().to_string_lossy().ends_with(".tmp") {
                tmp_count += 1;
            }
        }
    }
    assert_eq!(tmp_count, 0, "no partial object left under a final name");
}

// --- Layer-1 fragments --------------------------------------------------------

#[test]
fn fragments_round_trip_and_re_put_is_a_noop() {
    use cgx_store::FragmentInput;
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    let blob = cgx_store::BlobOid::new("blob-aaaa");
    let input = FragmentInput {
        blob: &blob,
        lang: "rust",
        frontend_version: 3,
        bytes: b"canonical-fragment-bytes",
    };

    s.put_fragments(&[input]).unwrap();
    let got = s.fragment(&blob).unwrap().expect("fragment stored");
    assert_eq!(got.lang, "rust");
    assert_eq!(got.frontend_version, 3);
    assert_eq!(got.fragment, b"canonical-fragment-bytes");

    // Re-put of the unchanged blob is a true no-op (IX-1).
    s.put_fragments(&[input]).unwrap();
    assert_eq!(s.fragment(&blob).unwrap().unwrap(), got);
    assert!(s.fragment(&cgx_store::BlobOid::new("absent")).unwrap().is_none());
}

#[test]
fn prune_drops_only_fragments_absent_from_the_live_set() {
    use cgx_store::{BlobOid, FragmentInput};
    let root = TempDir::new().unwrap();
    let mut s = store(&root);
    let live_blob = BlobOid::new("blob-live");
    let dead_blob = BlobOid::new("blob-dead");
    s.put_fragments(&[
        FragmentInput { blob: &live_blob, lang: "rust", frontend_version: 1, bytes: b"live" },
        FragmentInput { blob: &dead_blob, lang: "rust", frontend_version: 1, bytes: b"dead" },
    ])
    .unwrap();

    let mut live = cgx_store::BlobSet::new();
    live.insert(live_blob.clone());
    let stats = s.prune(&live, false).unwrap();

    assert_eq!(stats.fragments_removed, 1);
    assert_eq!(stats.graphs_removed, 0, "prune never GCs graphs (recon §A.1)");
    assert!(s.fragment(&live_blob).unwrap().is_some());
    assert!(s.fragment(&dead_blob).unwrap().is_none());
}

fn plant_orphan_object(root: &TempDir, bytes: &[u8]) {
    let oid = ObjectOid::of_bytes(bytes);
    let path = oid.object_path(&objects_dir(root));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn plant_tmp_object(root: &TempDir, bytes: &[u8]) {
    let dir = objects_dir(root).join("zz");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("deadbeef.tmp"), bytes).unwrap();
}
