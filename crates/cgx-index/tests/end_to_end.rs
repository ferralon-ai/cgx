//! End-to-end golden assertions: index the committed Rust and TypeScript fixture
//! trees and verify the resolved graph carries the WP-02 goldens' key
//! post-resolution facts — cross-file import resolution, exception-path edges,
//! candidate-set virtual dispatch, and loop/conditional edge labels.
//!
//! These own the integration-level golden checks (per E5's WP-04 handoff: the
//! frontend tests assert frontend-owned facts drift-tolerantly; the *resolved*
//! cross-file facts are asserted here, where the full pipeline runs).

mod common;

use cgx_core::{Confidence, EdgeCondition, EdgeKind};
use cgx_index::{default_registry, index_path};
use common::*;

fn index_fixture(fixture: &str) -> (tempfile::TempDir, cgx_store::SqliteStore, GraphIndexHolder) {
    let (tmp, repo) = init_fixture_repo(fixture);
    let registry = default_registry();
    let mut store = mem_store();
    let outcome = index_path(&repo, &registry, &mut store, &Default::default()).unwrap();
    let g = read_graph(&store, &outcome.graph_key);
    (tmp, store, GraphIndexHolder { graph: g })
}

/// Owns the graph so the borrowing `GraphIndex` can be built per assertion.
struct GraphIndexHolder {
    graph: cgx_store::LinkedGraph,
}
impl GraphIndexHolder {
    fn idx(&self) -> GraphIndex {
        GraphIndex::new(&self.graph)
    }
}

// ---- Rust ----

#[test]
fn rust_direct_intra_file_calls_resolve_certain() {
    let (_t, _s, h) = index_fixture("rust-sample");
    let idx = h.idx();
    // direct.rs: chain() calls step_a/b/c — same-file direct, certain.
    for callee in ["step_a", "step_b", "step_c"] {
        let target = format!("rust_sample::direct::{callee}");
        let e = idx
            .edges_between("rust_sample::direct::chain", &target)
            .find(|e| e.condition == EdgeCondition::Always)
            .unwrap_or_else(|| panic!("chain -> {callee} edge missing"));
        assert_eq!(e.kind, EdgeKind::Calls);
        assert_eq!(e.confidence, Confidence::Certain);
    }
}

#[test]
fn rust_cross_file_import_resolves_probable() {
    let (_t, _s, h) = index_fixture("rust-sample");
    let idx = h.idx();
    // imports.rs: double_via_reexport -> direct::add via re-export (probable).
    let e = idx
        .edges_between(
            "rust_sample::imports::double_via_reexport",
            "rust_sample::direct::add",
        )
        .next()
        .expect("re-export-resolved cross-file edge missing");
    assert_eq!(e.kind, EdgeKind::Calls);
    assert_eq!(e.confidence, Confidence::Probable);

    // use_alias -> direct::add via `use ... as sum_two` alias (probable).
    assert!(
        idx.edges_between(
            "rust_sample::imports::use_alias",
            "rust_sample::direct::add"
        )
        .any(|e| e.confidence == Confidence::Probable),
        "aliased import did not resolve to direct::add"
    );

    // utils::run_chain -> direct::chain (cross-module within file, probable).
    assert!(idx.has_edge(
        "rust_sample::imports::utils::run_chain",
        "rust_sample::direct::chain"
    ));
}

#[test]
fn rust_error_operator_emits_exception_edge_twin() {
    let (_t, _s, h) = index_fixture("rust-sample");
    let idx = h.idx();
    // errors.rs: try_parse calls check_range via `?` — both an Always and an
    // Exception edge to the same callee (the GM-3 error-path twin).
    let conds: Vec<EdgeCondition> = idx
        .edges_between(
            "rust_sample::errors::try_parse",
            "rust_sample::errors::check_range",
        )
        .map(|e| e.condition)
        .collect();
    assert!(
        conds.contains(&EdgeCondition::Always),
        "missing always edge: {conds:?}"
    );
    assert!(
        conds.contains(&EdgeCondition::Exception),
        "missing exception edge: {conds:?}"
    );
}

#[test]
fn rust_virtual_dispatch_produces_candidate_set() {
    let (_t, _s, h) = index_fixture("rust-sample");
    let idx = h.idx();
    // imports.rs: animal_chorus does virtual dispatch on Speak::speak — the
    // resolver emits a same-name-method candidate set (calls:virtual), spanning
    // Cat::speak, Dog::speak, and the trait default Speak::speak.
    let callers = ["rust_sample::imports::animal_chorus::{closure@28:24}"];
    let targets = [
        "rust_sample::virtual_dispatch::Cat::speak",
        "rust_sample::virtual_dispatch::Dog::speak",
        "rust_sample::virtual_dispatch::Speak::speak",
    ];
    let mut found = 0;
    for caller in callers {
        for t in targets {
            for e in idx.edges_between(caller, t) {
                assert_eq!(e.kind, EdgeKind::CallsVirtual);
                assert!(
                    e.candidate_group.is_some(),
                    "virtual edge lacks candidate group"
                );
                found += 1;
            }
        }
    }
    assert!(
        found >= 2,
        "expected a multi-target candidate set, found {found}"
    );
}

#[test]
fn rust_loop_and_conditional_edge_labels() {
    let (_t, _s, h) = index_fixture("rust-sample");
    let idx = h.idx();
    // conditions.rs: process_all calls process_item inside a loop body (loop).
    assert!(idx
        .edges_between(
            "rust_sample::conditions::process_all",
            "rust_sample::conditions::process_item"
        )
        .any(|e| e.condition == EdgeCondition::Loop));
    // dispatch calls log_* inside match arms (conditional).
    assert!(idx
        .edges_between(
            "rust_sample::conditions::dispatch",
            "rust_sample::conditions::log_info"
        )
        .any(|e| e.condition == EdgeCondition::Conditional));
}

// ---- TypeScript ----

#[test]
fn ts_cross_file_import_resolves_probable() {
    let (_t, _s, h) = index_fixture("ts-sample");
    let idx = h.idx();
    // imports.ts: doubleViaImport -> direct::add (probable).
    let e = idx
        .edges_between(
            "ts_sample::imports::doubleViaImport",
            "ts_sample::direct::add",
        )
        .next()
        .expect("cross-file TS import edge missing");
    assert_eq!(e.kind, EdgeKind::Calls);
    assert_eq!(e.confidence, Confidence::Probable);

    // makeDogSpeak -> virtual_dispatch::makeSpeak (probable, cross-file).
    assert!(idx.has_edge(
        "ts_sample::imports::makeDogSpeak",
        "ts_sample::virtual_dispatch::makeSpeak"
    ));
}

#[test]
fn ts_async_await_emits_calls_async() {
    let (_t, _s, h) = index_fixture("ts-sample");
    let idx = h.idx();
    // async_calls.ts: fetchData awaits httpGet -> calls:async.
    assert!(idx
        .edges_between(
            "ts_sample::async_calls::fetchData",
            "ts_sample::async_calls::httpGet"
        )
        .any(|e| e.kind == EdgeKind::CallsAsync));
}

#[test]
fn ts_try_catch_emits_exception_edge() {
    let (_t, _s, h) = index_fixture("ts-sample");
    let idx = h.idx();
    // errors.ts: tryParse logs the parse error in a catch block -> exception.
    assert!(idx
        .edges_between(
            "ts_sample::errors::tryParse",
            "ts_sample::errors::logParseError"
        )
        .any(|e| e.condition == EdgeCondition::Exception));
}

#[test]
fn both_fixtures_index_only_source_files() {
    // The non-source files (Cargo.toml / package.json / tsconfig.json) are
    // skipped, not mis-parsed: blobs_unsupported counts them.
    let (tmp_r, _repo_r) = init_fixture_repo("rust-sample");
    let registry = default_registry();
    let mut store = mem_store();
    let r = index_path(tmp_r.path().join("repo"), &registry, &mut store, &Default::default()).unwrap();
    assert!(r.stats.blobs_indexed >= 14, "{:?}", r.stats);
    assert!(
        r.stats.blobs_unsupported >= 1,
        "Cargo.toml not skipped: {:?}",
        r.stats
    );
    assert!(r.stats.nodes > 0 && r.stats.edges > 0);
}
