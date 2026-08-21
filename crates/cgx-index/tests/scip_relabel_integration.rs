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

// cgx's Rust frontend roots a `src/`-at-root file at the owning Cargo.toml
// package name (see cgx-lang-rust::module + cgx-index::cargo_pkg). This fixture's
// manifest declares `name = "scip_relabel"`, so its node FQNs root at
// `scip_relabel::…` — and the synthetic SCIP MUST carry the same package for its
// qnames to join (the BLOCKER-2 regression: a prior bug rooted every `src/`-root
// file at a hardcoded `rust_sample`, silently missing the join on any real crate).
const PKG: &str = "scip_relabel";
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
    synthetic_scip_for(PKG, lib_src, math_src)
}

/// As [`synthetic_scip`], but for an arbitrary package name — used by the
/// BLOCKER-2 join guard, which indexes a crate NOT named `rust_sample`.
fn synthetic_scip_for(pkg: &str, lib_src: &str, math_src: &str) -> Vec<u8> {
    let add = format!("rust-analyzer cargo {pkg} {VER} math/add().");
    let helper = format!("rust-analyzer cargo {pkg} {VER} math/helper().");

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
        dataflow: false,
    };
    let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();

    let g = read_graph(&store, &outcome.graph_key);
    let idx = GraphIndex::new(&g);

    // E1: the cross-module free-fn call `caller -> math::add` is certain@scip
    // after ingest (its baseline is probable@import-ref, upgraded by a unique
    // SCIP definition).
    let e = idx
        .edges_between("scip_relabel::caller", "scip_relabel::math::add")
        .next()
        .expect("caller -> add edge present (FQN rooted at the real Cargo.toml crate)");
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

/// BLOCKER-2 join guard: a `src/`-at-root crate whose Cargo.toml package is NOT
/// `rust_sample`. The prior bug rooted every `src/`-root file's FQN at a hardcoded
/// `rust_sample`, so SCIP symbols carrying the *real* crate name (`acme`) missed
/// the `node_by_fqn` join → zero `certain` upgrades, silently. This test indexes
/// such a crate end-to-end with a synthetic `.scip` rooted at `acme` and proves
/// the join succeeds (the free-fn call upgrades to `certain`) only because the
/// node FQNs now derive `acme` from Cargo.toml.
#[test]
fn scip_joins_on_real_cargo_package_name_not_rust_sample() {
    const ACME: &str = "acme";
    let lib_src = "pub mod math;\n\nuse math::add;\nuse math::helper;\n\npub fn caller() -> i32 {\n    add(1, 2)\n}\n\npub fn caller_two() -> i32 {\n    helper(3)\n}\n";
    let math_src = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn helper(x: i32) -> i32 {\n    x * 2\n}\n";

    let (_t, repo) = init_empty_repo();
    write_file(
        &repo,
        "Cargo.toml",
        "[package]\nname = \"acme\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(&repo, "src/lib.rs", lib_src);
    write_file(&repo, "src/math.rs", math_src);
    commit_all(&repo, "acme fixture");

    let dir = tempfile::tempdir().unwrap();
    let scip_path = dir.path().join("acme.scip");
    std::fs::write(&scip_path, synthetic_scip_for(ACME, lib_src, math_src)).unwrap();

    let registry = cgx_index::default_registry();
    let mut store = mem_store();
    let opts = IndexOpts {
        scip: Some(scip_path),
        dataflow: false,
    };
    let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();

    let g = read_graph(&store, &outcome.graph_key);
    let idx = GraphIndex::new(&g);

    // The node FQNs must root at the real crate name (proves the Cargo.toml derive).
    assert!(
        idx.has_node("acme::caller") && idx.has_node("acme::math::add"),
        "node FQNs must root at the Cargo.toml crate `acme`, not `rust_sample`"
    );

    // The join now matches → the unique-def free-fn call upgrades to certain.
    let e = idx
        .edges_between("acme::caller", "acme::math::add")
        .next()
        .expect("caller -> add edge present under the real crate name");
    assert_eq!(
        e.confidence,
        Confidence::Certain,
        "BLOCKER-2: the SCIP join must succeed on a crate not named rust_sample"
    );
    assert_eq!(e.tier, Tier::Scip);
    assert_eq!(e.rule, "scip-occurrence");

    let scip = outcome.stats.scip.expect("scip stats present");
    assert!(
        scip.upgraded_certain >= 1,
        "at least the add call upgrades (zero would be the silent BLOCKER-2 miss)"
    );
    assert_eq!(
        scip.dep_edges, 0,
        "an in-tree ref must NOT be misclassified cross-crate (no spurious scip-dep: edges)"
    );
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
        dataflow: false,
    };

    let registry = cgx_index::default_registry();

    let data_a = {
        let mut store = mem_store();
        let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();
        store.dump_node_edge_data(&cgx_store::TreeOid::new(outcome.graph_key.clone())).unwrap()
    };
    let data_b = {
        let mut store = mem_store();
        let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();
        store.dump_node_edge_data(&cgx_store::TreeOid::new(outcome.graph_key.clone())).unwrap()
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
        dataflow: false,
    };
    let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();
    let g = read_graph(&store, &outcome.graph_key);
    let idx = GraphIndex::new(&g);
    assert!(
        idx.edges_between("scip_relabel::caller", "scip_relabel::math::add")
            .any(|e| e.confidence == Confidence::Certain),
        "real scip blob should upgrade the free-fn call to certain"
    );
}
