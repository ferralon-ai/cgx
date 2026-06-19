//! Integration + determinism tests for `cgx index --scip` over a real fixture
//! crate (`fixtures/scip-relabel`).
//!
//! The `.scip` index is authored programmatically (no rust-analyzer; it is not
//! installed in CI) via `cgx-scip`'s test-only encoder, calibrating the SCIP
//! occurrence coordinates against the committed fixture source so a free-fn
//! direct call resolves to a unique SCIP definition. These exercise the full
//! pipeline through `index_path` + `IndexOpts.scip`, including finalize-if-dirty
//! and byte-identical determinism (IF-8).

mod common;

use std::path::Path;

use cgx_core::{Confidence, Tier};
use cgx_index::{index_path, IndexOpts};
use cgx_scip::testsupport::{document, index, occurrence, symbol_info, ROLE_DEFINITION};
use common::*;

// cgx's Rust frontend roots a `src/`-relative file at the default crate name
// `rust_sample` (see cgx-lang-rust::module), independent of Cargo.toml's package
// name. The synthetic SCIP must use the same package so its qnames join with
// the in-tree node FQNs.
const PKG: &str = "rust_sample";
const VER: &str = "0.1.0";

/// 0-based `(line, char)` of the first occurrence of `needle` on the line that
/// contains the *call* `needle(`. Scans the committed fixture source so the
/// synthetic `.scip` occurrence lands exactly on the call-site identifier (the
/// span the frontend records and the relabel pass joins on).
fn call_site(src: &str, needle: &str) -> (i32, i32) {
    let call = format!("{needle}(");
    for (line_no, line) in src.lines().enumerate() {
        // Skip the definition line (`pub fn add(`); we want the call.
        if line.trim_start().starts_with("pub fn") {
            continue;
        }
        if let Some(col) = line.find(&call) {
            return (line_no as i32, col as i32);
        }
    }
    panic!("call site for {needle} not found");
}

/// 0-based `(line, char)` of the definition identifier `pub fn <needle>`.
fn def_site(src: &str, needle: &str) -> (i32, i32) {
    let def = format!("pub fn {needle}");
    for (line_no, line) in src.lines().enumerate() {
        if let Some(col) = line.find(&def) {
            // Point at the identifier, after `pub fn `.
            return (line_no as i32, (col + "pub fn ".len()) as i32);
        }
    }
    panic!("def site for {needle} not found");
}

/// Author a synthetic `.scip` for the fixture: `add` and `helper` are free fns
/// defined in `src/math.rs` (unique defs) and called from `src/lib.rs`. The
/// fixture source is read so the occurrence coordinates match the indexed graph
/// exactly, and the baseline cross-module edges (probable) upgrade to certain.
fn synthetic_scip(lib_src: &str, math_src: &str) -> Vec<u8> {
    let add = format!("rust-analyzer cargo {PKG} {VER} math/add().");
    let helper = format!("rust-analyzer cargo {PKG} {VER} math/helper().");

    let (add_def_l, add_def_c) = def_site(math_src, "add");
    let (add_ref_l, add_ref_c) = call_site(lib_src, "add");
    let (helper_def_l, helper_def_c) = def_site(math_src, "helper");
    let (helper_ref_l, helper_ref_c) = call_site(lib_src, "helper");

    index(
        "/repo",
        "rust-analyzer",
        vec![
            document(
                "src/math.rs",
                vec![
                    occurrence(&[add_def_l, add_def_c, add_def_c + 3], &add, ROLE_DEFINITION),
                    occurrence(
                        &[helper_def_l, helper_def_c, helper_def_c + 6],
                        &helper,
                        ROLE_DEFINITION,
                    ),
                ],
                vec![symbol_info(&add, 0, ""), symbol_info(&helper, 0, "")],
            ),
            document(
                "src/lib.rs",
                vec![
                    occurrence(&[add_ref_l, add_ref_c, add_ref_c + 3], &add, 0),
                    occurrence(&[helper_ref_l, helper_ref_c, helper_ref_c + 6], &helper, 0),
                ],
                vec![],
            ),
        ],
        vec![],
    )
}

#[test]
fn scip_upgrades_free_fn_call_to_certain_via_index_path() {
    let (_t, repo) = init_fixture_repo("scip-relabel");
    let lib_src = std::fs::read_to_string(repo.join("src/lib.rs")).unwrap();
    let math_src = std::fs::read_to_string(repo.join("src/math.rs")).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let scip_path = dir.path().join("relabel.scip");
    std::fs::write(&scip_path, synthetic_scip(&lib_src, &math_src)).unwrap();

    let registry = cgx_index::default_registry();
    let mut store = mem_store();
    let opts = IndexOpts {
        scip: Some(scip_path.clone()),
    };
    let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();

    let g = read_graph(&store, outcome.graph_id);
    let idx = GraphIndex::new(&g);

    // E1: the cross-module free-fn call `caller -> math::add` is certain@scip
    // after ingest (its baseline is probable@import-ref, upgraded by a unique
    // SCIP definition).
    let e = idx
        .edges_between("rust_sample::caller", "rust_sample::math::add")
        .next()
        .expect("caller -> add edge present");
    assert_eq!(
        e.confidence,
        Confidence::Certain,
        "E1: unique-def free-fn direct call must upgrade to certain via --scip"
    );
    assert_eq!(e.tier, Tier::Scip);
    assert_eq!(e.rule, "scip-occurrence");

    // The SCIP counters must be surfaced for cgx doctor.
    let scip = outcome.stats.scip.expect("scip stats present when --scip given");
    assert!(scip.upgraded_certain >= 1, "at least the add call upgraded");
}

#[test]
fn no_scip_flag_is_byte_identical_to_phase1() {
    // The no-scip path must behave EXACTLY as Phase 1: indexing with
    // IndexOpts::default() yields the same stored bytes as before, and no scip
    // stats are attached.
    let (_t, repo) = init_fixture_repo("scip-relabel");
    let registry = cgx_index::default_registry();
    let mut store = mem_store();
    let outcome = index_path(&repo, &registry, &mut store, &IndexOpts::default()).unwrap();
    assert!(
        outcome.stats.scip.is_none(),
        "no --scip ⇒ no scip stats (Phase-1 behavior unchanged)"
    );
}

#[test]
fn re_indexing_with_scip_is_byte_identical() {
    // Determinism (IF-8): two index runs with the same --scip produce a
    // byte-identical stored graph, including the finalize-if-dirty re-sort.
    let (_t, repo) = init_fixture_repo("scip-relabel");
    let lib_src = std::fs::read_to_string(repo.join("src/lib.rs")).unwrap();
    let math_src = std::fs::read_to_string(repo.join("src/math.rs")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let scip_path = dir.path().join("relabel.scip");
    std::fs::write(&scip_path, synthetic_scip(&lib_src, &math_src)).unwrap();
    let opts = IndexOpts {
        scip: Some(scip_path.clone()),
    };

    let registry = cgx_index::default_registry();

    let data_a = {
        let mut store = mem_store();
        let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();
        store.dump_node_edge_data(outcome.graph_id).unwrap()
    };
    let data_b = {
        let mut store = mem_store();
        let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();
        store.dump_node_edge_data(outcome.graph_id).unwrap()
    };

    assert_eq!(
        data_a.len(),
        data_b.len(),
        "node+edge row count must match across --scip runs"
    );
    for (i, (a, b)) in data_a.iter().zip(data_b.iter()).enumerate() {
        assert_eq!(a, b, "row {i} bytes differ between two --scip index runs");
    }
}

#[test]
fn real_scip_blob_when_present() {
    // The real rust-analyzer blob path (decision R7): gated behind the committed
    // blob's presence. Skipped in this environment (rust-analyzer not installed);
    // see fixtures/scip-relabel/REGEN.md.
    let blob = common::fixtures_root().join("scip-relabel").join("relabel.scip");
    if !Path::new(&blob).exists() {
        eprintln!("skipping: real {blob:?} absent (regen with rust-analyzer scip)");
        return;
    }
    let (_t, repo) = init_fixture_repo("scip-relabel");
    let registry = cgx_index::default_registry();
    let mut store = mem_store();
    let opts = IndexOpts {
        scip: Some(blob),
    };
    let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();
    let g = read_graph(&store, outcome.graph_id);
    let idx = GraphIndex::new(&g);
    assert!(
        idx.edges_between("rust_sample::caller", "rust_sample::math::add")
            .any(|e| e.confidence == Confidence::Certain),
        "real scip blob should upgrade the free-fn call to certain"
    );
}
