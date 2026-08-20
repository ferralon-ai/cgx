//! GM-12 transitive effect-closure (P8b), end-to-end through the full pipeline and
//! the SQLite store. The closure algorithm itself is unit-tested in
//! `cgx-resolve/tests/effects.rs`; these tests pin the *integration* facts:
//!
//! - the closure runs as a pipeline post-pass and populates
//!   `NodeRecord.transitive_effects`;
//! - `transitive_effects` round-trips losslessly through `put_graph` / `read_graph`
//!   (it rides the postcard `data` blob — no store column needed, P8a's pattern);
//! - the **spawn boundary** holds end-to-end: a callee reached only via a `Spawns`
//!   edge does not contribute its effects to the spawner;
//! - re-indexing the same tree is byte-identical at the stored-row level (the
//!   determinism contract, design §7), with `transitive_effects` inside the bytes.

mod common;

use cgx_core::effect::Effect;
use cgx_core::edge::EdgeKind;
use cgx_index::{default_registry, index_path};
use common::*;

/// A tiny effectful program committed into a throwaway repo:
///
/// `entry` directly calls `read_config` (which does `std::fs::read` → io.file) and
/// spawns `worker` via `tokio::spawn(worker())` — the recognized spawn-path plus
/// call-expression argument form the Rust frontend records as a `Spawns` edge to
/// `worker` (matching `fixtures/rust-sample/src/spawn.rs`). `worker` does
/// `std::process::Command::new` → io.proc.
///
/// So `entry`'s transitive effects must include io.file (the direct call) and the
/// `spawns` own-effect (it launches a task), but NOT io.proc (that lives behind the
/// `Spawns` edge and is not unioned in).
const PROGRAM: &str = r#"
pub fn entry() {
    read_config();
    tokio::spawn(worker());
}

fn read_config() {
    let _ = std::fs::read("config.toml");
}

fn worker() {
    let _ = std::process::Command::new("ls");
}
"#;

/// Replace a fresh rust-sample repo's source tree with the single controlled
/// program file and commit, returning the temp dir + repo path.
fn program_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let (tmp, repo) = init_fixture_repo("rust-sample");
    let src_dir = repo.join("src");
    for entry in std::fs::read_dir(&src_dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            std::fs::remove_file(&p).unwrap();
        }
    }
    write_file(&repo, "src/main.rs", PROGRAM);
    commit_all(&repo, "effectful program");
    (tmp, repo)
}

/// Index a one-file effectful program in a fresh committed repo, returning the
/// store, the stored graph, and its Layer-2 key.
fn index_program() -> (
    tempfile::TempDir,
    cgx_store::SqliteStore,
    cgx_store::LinkedGraph,
    String,
) {
    let (tmp, repo) = program_repo();
    let registry = default_registry();
    let mut store = mem_store();
    let outcome = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    let g = read_graph(&store, &outcome.graph_key);
    (tmp, store, g, outcome.graph_key)
}

fn node<'g>(g: &'g cgx_store::LinkedGraph, name: &str) -> &'g cgx_core::NodeRecord {
    g.nodes
        .iter()
        .find(|n| n.fqn.ends_with(name))
        .unwrap_or_else(|| panic!("no node ending in {name}"))
}

#[test]
fn closure_populates_transitive_effects_through_the_pipeline() {
    let (_t, _s, g, _id) = index_program();

    let entry = node(&g, "::entry");
    // entry directly calls read_config (io.file), so its transitive set carries it.
    assert!(
        entry.transitive_effects.contains(Effect::IoFile),
        "entry.transitive_effects must contain io.file via read_config(); got {:?}",
        entry.transitive_effects.iter().collect::<Vec<_>>()
    );
    // read_config's own io.file is NOT something entry does directly.
    assert!(
        !entry.own_effects.contains(Effect::IoFile),
        "entry.own_effects must not contain io.file"
    );
    // read_config itself owns and transitively has io.file.
    let rc = node(&g, "::read_config");
    assert!(rc.own_effects.contains(Effect::IoFile));
    assert!(rc.transitive_effects.contains(Effect::IoFile));
}

#[test]
fn spawn_boundary_holds_end_to_end() {
    let (_t, _s, g, _id) = index_program();

    let entry = node(&g, "::entry");
    let worker = node(&g, "::worker");

    // Sanity: the fixture really produced a Spawns edge entry -> worker.
    assert!(
        g.edges.iter().any(|e| e.kind == EdgeKind::Spawns
            && e.src == entry.id
            && e.dst == worker.id),
        "expected a Spawns edge entry -> worker"
    );

    // worker does io.proc; that effect rides the Spawns edge and must NOT be
    // unioned into the spawner's transitive set (GM-12).
    assert!(
        worker.own_effects.contains(Effect::IoProc),
        "worker.own_effects must contain io.proc"
    );
    assert!(
        !entry.transitive_effects.contains(Effect::IoProc),
        "io.proc must NOT cross the spawn boundary into entry.transitive_effects; got {:?}",
        entry.transitive_effects.iter().collect::<Vec<_>>()
    );
    // entry does carry the `spawns` own-effect (it launches a task).
    assert!(
        entry.own_effects.contains(Effect::Spawns),
        "entry.own_effects must contain the spawns label"
    );
}

#[test]
fn transitive_effects_survive_store_round_trip() {
    // Index, read back, and confirm the populated transitive set came back from the
    // store (it rides the postcard `data` blob, not a dedicated column).
    let (_t, store, g, id1) = index_program();
    let entry = node(&g, "::entry");
    assert!(
        !entry.transitive_effects.is_empty(),
        "transitive_effects must be non-empty after a store round-trip"
    );

    // Re-index a second independent store over the same program; the stored
    // node/edge bytes (which include transitive_effects inside the blob) must be
    // byte-identical — the determinism contract (design §7), now covering P8b.
    let (_t2, repo2) = program_repo();
    let registry = default_registry();
    let mut store2 = mem_store();
    let out2 = index_path(&repo2, &registry, &mut store2, &Default::default()).unwrap();

    // Compare the canonical row bytes of both stores.
    let dump1 = store.dump_node_edge_data(&cgx_store::TreeOid::new(id1.clone())).unwrap();
    let dump2 = store2.dump_node_edge_data(&cgx_store::TreeOid::new(out2.graph_key.clone())).unwrap();
    assert_eq!(dump1, dump2, "stored node/edge bytes must be byte-identical");
}
