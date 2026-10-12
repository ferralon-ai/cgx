//! End-to-end compdb-completeness proofs (Phase F2) — the production
//! `cgx index --scip` pipeline earning C++ `certain`, not just the F fixture.
//!
//! A real multi-TU C++ source tree is committed to a throwaway git repo and
//! indexed through `index_path` (the exact pipeline the CLI drives: extract →
//! link → SCIP relabel → CHA/RTA/sig → store). The scip-clang `.scip` index is
//! authored with `cgx-scip`'s test-only encoder (the `scip-clang` binary is NOT
//! required; the wire format is producer-agnostic).
//!
//! Scenario: two overloads `pick(int)` / `pick(double)` (free functions in
//! `src/over.cpp`) share the signature-free qname `pick`; `run` in
//! `src/main.cpp` calls `pick(3)` — a `possible` 2-candidate set. scip-clang
//! resolves the call to `pick(int)`.
//!
//! Proven here, through the full pipeline:
//!   1. COMPLETE compdb (covers both discovered `.cpp` TUs) → the call upgrades to
//!      `certain`, span-redirected to the `pick(int)` node, group collapsed.
//!   2. PARTIAL compdb (one TU omitted) → the same call caps at `probable`
//!      (FAIL-CLOSED), group intact, `capped_partial_compdb` ticks, and the stats
//!      carry the missing-TU diagnostic.
//!   3. `--compdb-complete` override → `certain` without a compdb file.
//!   4. Absent compdb → `probable` (fail-closed).
//!   5. A header (`.hpp`) absent from the compdb does NOT make it `Partial`
//!      (headers are not translation units).

mod common;

use cgx_core::{Confidence, Tier};
use cgx_index::{index_path, CompdbCompleteness, IndexOpts};
use cgx_scip::testsupport::{document, index, occurrence, symbol_info, ROLE_DEFINITION};
use common::*;

const OVER_SRC: &str = "int pick(int x) { return x; }\nint pick(double x) { return 0; }\n";
const MAIN_SRC: &str = "int run() { return pick(3); }\n";
const HEADER_SRC: &str = "#pragma once\nvoid helper_decl();\n";

const PICK_INT: &str = "cxx . . . pick(ai).";
const PICK_DOUBLE: &str = "cxx . . . pick(ad).";

/// 0-based `(line, col)` of the start of the `pick(` call expression in `MAIN_SRC`
/// — the span the frontend records for the call edge and the relabel joins on.
fn call_coord() -> (i32, i32) {
    for (l, line) in MAIN_SRC.lines().enumerate() {
        if let Some(c) = line.find("pick(") {
            return (l as i32, c as i32);
        }
    }
    panic!("call site not found");
}

/// A scip-clang `.scip` resolving `run`'s `pick(3)` call to the `pick(int)`
/// overload (def at `src/over.cpp` line 0, the `pick(double)` def at line 1).
fn scip_bytes() -> Vec<u8> {
    let (cl, cc) = call_coord();
    index(
        "/repo",
        "scip-clang",
        vec![
            document(
                "src/over.cpp",
                vec![
                    occurrence(&[0, 4, 8], PICK_INT, ROLE_DEFINITION),
                    occurrence(&[1, 4, 8], PICK_DOUBLE, ROLE_DEFINITION),
                ],
                vec![symbol_info(PICK_INT, 0, ""), symbol_info(PICK_DOUBLE, 0, "")],
            ),
            document(
                "src/main.cpp",
                vec![occurrence(&[cl, cc, cc + 4], PICK_INT, 0)],
                vec![],
            ),
        ],
        vec![],
    )
}

/// Build the committed C++ repo + the scip-clang `.scip` on disk. Returns the temp
/// dir (kept alive), the repo path, and the `.scip` path. A separate temp dir (also
/// returned) homes the `.scip` so it is never itself indexed.
fn fixture() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (tmp, repo) = init_empty_repo();
    write_file(&repo, "src/over.cpp", OVER_SRC);
    write_file(&repo, "src/main.cpp", MAIN_SRC);
    write_file(&repo, "src/pick.hpp", HEADER_SRC);
    commit_all(&repo, "cpp overload fixture");

    let aux = tempfile::tempdir().unwrap();
    let scip_path = aux.path().join("pick.scip");
    std::fs::write(&scip_path, scip_bytes()).unwrap();
    (tmp, aux, repo, scip_path)
}

/// The repo working directory exactly as the pipeline resolves it (so a compdb
/// built here matches the discovered-TU anchor lexically, symlinks and all).
fn repo_workdir(repo: &std::path::Path) -> std::path::PathBuf {
    cgx_index::Repo::discover(repo)
        .unwrap()
        .workdir()
        .unwrap()
        .to_path_buf()
}

/// Write a `compile_commands.json` at `path` covering `tus` (repo-relative), with
/// absolute `file` entries anchored at the repo workdir.
fn write_compdb(path: &std::path::Path, repo: &std::path::Path, tus: &[&str]) {
    let wd = repo_workdir(repo);
    let entries: Vec<serde_json::Value> = tus
        .iter()
        .map(|rel| {
            let abs = wd.join(rel);
            serde_json::json!({
                "directory": wd.to_string_lossy(),
                "file": abs.to_string_lossy(),
                "command": format!("c++ -c {rel}"),
            })
        })
        .collect();
    std::fs::write(path, serde_json::to_vec_pretty(&entries).unwrap()).unwrap();
}

/// Index `repo` with the scip-clang index and the given compdb opts, returning
/// every `run -> pick` call edge (the link pass emits one per candidate overload,
/// so an overloaded call is a 2-edge candidate set), the outcome stats, and the
/// NodeId of the `pick(int)` overload.
fn index_pick_edges(
    repo: &std::path::Path,
    scip_path: &std::path::Path,
    compdb: Option<std::path::PathBuf>,
    compdb_complete: bool,
) -> (Vec<cgx_core::EdgeRecord>, cgx_index::IndexStats, cgx_core::NodeId) {
    let registry = cgx_index::default_registry();
    let mut store = mem_store();
    let opts = IndexOpts {
        scip: Some(scip_path.to_path_buf()),
        dataflow: false,
        compdb,
        compdb_complete,
    };
    let outcome = index_path(repo, &registry, &mut store, &opts).unwrap();
    let g = read_graph(&store, &outcome.graph_key);
    let idx = GraphIndex::new(&g);
    let edges: Vec<cgx_core::EdgeRecord> = idx.edges_between("run", "pick").cloned().collect();
    assert!(!edges.is_empty(), "run -> pick call edges present");
    // NodeId of the pick(int) overload (def at over.cpp line_start 1).
    let pick_int = g
        .nodes
        .iter()
        .find(|n| n.fqn == "pick" && n.file == "src/over.cpp" && n.line_start == 1)
        .expect("pick(int) node at over.cpp:1")
        .id;
    (edges, outcome.stats, pick_int)
}

#[test]
fn complete_compdb_promotes_overload_to_certain() {
    let (_tmp, _aux, repo, scip_path) = fixture();
    let compdb = repo.join("compile_commands.json");
    write_compdb(&compdb, &repo, &["src/over.cpp", "src/main.cpp"]);

    let (edges, stats, pick_int) = index_pick_edges(&repo, &scip_path, Some(compdb), false);

    for e in &edges {
        assert_eq!(
            e.confidence,
            Confidence::Certain,
            "COMPLETE compdb: resolved overload promotes to certain end-to-end"
        );
        assert_eq!(e.tier, Tier::Scip);
        assert_eq!(e.rule, "scip-occurrence");
        assert_eq!(e.dst, pick_int, "span-precise redirect to the resolved overload");
        assert_eq!(e.candidate_group, None, "candidate group collapsed on certain");
    }

    let scip = stats.scip.expect("scip stats");
    assert_eq!(scip.upgraded_certain, edges.len(), "every candidate edge upgraded to certain");
    assert_eq!(scip.capped_partial_compdb, 0);

    let c = stats.compdb.expect("compdb report present for a scip-clang index");
    assert_eq!(c.completeness, CompdbCompleteness::Complete);
    assert_eq!(c.discovered_tus, 2, "two .cpp TUs discovered (the .hpp is not a TU)");
    assert_eq!(c.covered_tus, 2);
    assert_eq!(c.missing_tus, 0);
}

#[test]
fn partial_compdb_caps_overload_at_probable_with_diagnostic() {
    let (_tmp, _aux, repo, scip_path) = fixture();
    let compdb = repo.join("compile_commands.json");
    // over.cpp (where pick is defined) omitted → partial.
    write_compdb(&compdb, &repo, &["src/main.cpp"]);

    let (edges, stats, _pick_int) = index_pick_edges(&repo, &scip_path, Some(compdb), false);

    for e in &edges {
        assert_eq!(
            e.confidence,
            Confidence::Probable,
            "PARTIAL compdb: fail-closed cap, never certain"
        );
        assert_eq!(e.tier, Tier::Scip);
        assert!(
            e.candidate_group.is_some(),
            "candidate group preserved under fail-closed (no collapse on a possibly-false singleton)"
        );
    }

    let scip = stats.scip.expect("scip stats");
    assert_eq!(scip.upgraded_certain, 0, "fail-closed emits zero certain");
    assert_eq!(
        scip.capped_partial_compdb,
        edges.len(),
        "the capped-certain counter ticks for cgx doctor"
    );

    let c = stats.compdb.expect("compdb report present");
    assert_eq!(c.completeness, CompdbCompleteness::Partial);
    assert_eq!(c.discovered_tus, 2);
    assert_eq!(c.covered_tus, 1);
    assert_eq!(c.missing_tus, 1);
    assert_eq!(c.missing_sample, vec!["src/over.cpp".to_string()], "the missing TU is diagnosed");
}

#[test]
fn compdb_complete_flag_overrides_to_certain_without_a_file() {
    let (_tmp, _aux, repo, scip_path) = fixture();

    let (edges, stats, _pick_int) = index_pick_edges(&repo, &scip_path, None, true);

    for e in &edges {
        assert_eq!(
            e.confidence,
            Confidence::Certain,
            "--compdb-complete asserts completeness → certain"
        );
    }
    let c = stats.compdb.expect("compdb report present");
    assert_eq!(c.completeness, CompdbCompleteness::Complete);
    assert_eq!(c.discovered_tus, 2);
    assert_eq!(c.missing_tus, 0);
}

#[test]
fn absent_compdb_is_fail_closed_probable() {
    let (_tmp, _aux, repo, scip_path) = fixture();

    let (edges, stats, _pick_int) = index_pick_edges(&repo, &scip_path, None, false);

    for e in &edges {
        assert_eq!(
            e.confidence,
            Confidence::Probable,
            "no compdb, no assertion → fail-closed probable"
        );
    }
    let scip = stats.scip.expect("scip stats");
    assert_eq!(scip.upgraded_certain, 0);
    assert_eq!(scip.capped_partial_compdb, edges.len());

    let c = stats.compdb.expect("compdb report present");
    assert_eq!(c.completeness, CompdbCompleteness::Partial);
    assert_eq!(c.covered_tus, 0, "no compdb verified ⇒ nothing covered");
}

#[test]
fn missing_header_does_not_make_compdb_partial() {
    let (_tmp, _aux, repo, scip_path) = fixture();
    let compdb = repo.join("compile_commands.json");
    // Both .cpp TUs listed; the discovered src/pick.hpp header is intentionally
    // absent from the compdb. A header is not a translation unit, so completeness
    // must stay COMPLETE.
    write_compdb(&compdb, &repo, &["src/over.cpp", "src/main.cpp"]);

    let (edges, stats, _pick_int) = index_pick_edges(&repo, &scip_path, Some(compdb), false);

    for e in &edges {
        assert_eq!(
            e.confidence,
            Confidence::Certain,
            "a header absent from the compdb must not cap C++ promotions"
        );
    }
    let c = stats.compdb.expect("compdb report present");
    assert_eq!(c.completeness, CompdbCompleteness::Complete);
    assert_eq!(c.discovered_tus, 2, "the .hpp header is excluded from the TU set");
    assert_eq!(c.missing_tus, 0);
}
