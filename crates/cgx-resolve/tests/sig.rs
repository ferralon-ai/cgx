//! Signature-keyed candidate sets for indirect closure / fn-pointer calls (P6):
//! the [`run_sig`] post-pass replaces the link pass's indirect-call placeholder
//! edges (`rule = "indirect:<arity>"`) with the **arity-compatible** candidate set
//! over `Lambda` + free `Function` nodes, at `possible`, `rule = "sig-compat"`.
//!
//! These tests pin design §4.4 / decision R3: the candidate set is the
//! signature-COMPATIBLE function values (matching arity included, mismatched
//! excluded), all `possible`; the supernode cap keeps an over-large set with a cut
//! marker; and a re-run of the passes is byte-identical (determinism §7). They do
//! NOT assert value-flow narrowing (which closure actually flows where) — that is
//! DF-18 / Phase 3, explicitly out of scope.

mod common;

use std::collections::BTreeSet;

use cgx_core::condition::EdgeCondition;
use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::CutMarker;
use cgx_core::edge::EdgeKind;
use cgx_core::id::NodeId;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_frontend::facts::{FileFacts, RefKind, ScopeId};
use cgx_resolve::{link, run_sig, FileInput, LinkOpts, ResolvedGraph, CHA_SUPERNODE_CAP};

use common::FileBuilder;

const PUB: Visibility = Visibility::Public;
const ROOT: ScopeId = ScopeId::ROOT;

fn link_facts(facts: &FileFacts) -> ResolvedGraph {
    let inputs = vec![FileInput::new("blob".to_string(), "src/m.rs", "rust", facts)];
    link(&inputs, &LinkOpts::default())
}

fn node_id(g: &ResolvedGraph, fqn: &str) -> NodeId {
    g.node_records()
        .find(|n| n.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"))
        .id
}

/// The dst node-ids of the signature-compatible (`sig-compat`) indirect-call edges
/// the pass produced.
fn sig_candidate_dsts(g: &ResolvedGraph) -> BTreeSet<NodeId> {
    g.edge_records()
        .filter(|e| e.rule == "sig-compat")
        .map(|e| e.dst)
        .collect()
}

/// A module with:
/// - `apply` taking a callback `fn(i32) -> i32` and calling it indirectly (arity 1);
/// - two closures and one free fn of arity 1 (signature-compatible);
/// - one free fn of arity 2 (NOT compatible — must be excluded).
fn callback_with_arity_one_and_two() -> FileFacts {
    let mut b = FileBuilder::new();
    // The fn that performs the indirect call.
    b.def("m::apply", SymbolKind::Function, ROOT, 1, PUB, false, Some(2));
    // Compatible (arity 1):
    b.def("m::inc", SymbolKind::Function, ROOT, 2, PUB, false, Some(1));
    b.def("m::dbl", SymbolKind::Function, ROOT, 3, PUB, false, Some(1));
    b.def("m::{closure@9:5}", SymbolKind::Lambda, ROOT, 9, PUB, false, Some(1));
    // Incompatible (arity 2):
    b.def("m::add2", SymbolKind::Function, ROOT, 4, PUB, false, Some(2));

    // The indirect call `cb(x)` inside apply(): arity 1.
    let apply_scope = b.scope(ROOT, Some("m::apply"));
    b.raw_ref(
        &["cb"],
        apply_scope,
        10,
        RefKind::CallCallback,
        EdgeCondition::Always,
        Some(1),
    );
    b.build()
}

#[test]
fn indirect_call_resolves_to_the_arity_compatible_function_values_only() {
    // The candidate set must be exactly the Lambda + free Functions whose arity
    // matches the call site (1): inc, dbl, the closure. add2 (arity 2) is excluded.
    let facts = callback_with_arity_one_and_two();
    let mut g = link_facts(&facts);

    // The link pass emitted exactly one indirect placeholder (a self-edge sentinel).
    let placeholders: Vec<_> = g
        .edge_records()
        .filter(|e| e.kind == EdgeKind::CallsCallback && e.rule.starts_with("indirect"))
        .collect();
    assert_eq!(placeholders.len(), 1, "one indirect placeholder edge");
    assert_eq!(placeholders[0].rule, "indirect:1", "arity encoded in the rule");

    let stats = run_sig(&mut g);
    assert_eq!(stats.sites_resolved, 1, "the one indirect site is resolved");
    assert_eq!(stats.sites_unmatched, 0);

    let expected: BTreeSet<NodeId> = ["m::inc", "m::dbl", "m::{closure@9:5}"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(
        sig_candidate_dsts(&g),
        expected,
        "candidate set = the arity-compatible Lambda + free Function values"
    );
    // add2 (arity 2) is NOT a candidate.
    assert!(
        !sig_candidate_dsts(&g).contains(&node_id(&g, "m::add2")),
        "an arity-mismatched function is excluded"
    );
}

#[test]
fn the_signature_compatible_set_is_all_possible_chararta_callscallback() {
    // Every candidate edge is `Possible`, `Tier::ChaRta`, kind `CallsCallback`,
    // rule `sig-compat`, and carries a candidate group (a multi-candidate set).
    let facts = callback_with_arity_one_and_two();
    let mut g = link_facts(&facts);
    run_sig(&mut g);

    let edges: Vec<_> = g.edge_records().filter(|e| e.rule == "sig-compat").collect();
    assert_eq!(edges.len(), 3, "three compatible candidates");
    for e in &edges {
        assert_eq!(e.confidence, Confidence::Possible, "sig set is `possible`");
        assert_eq!(e.tier, Tier::ChaRta);
        assert_eq!(e.kind, EdgeKind::CallsCallback);
        assert!(e.candidate_group.is_some(), "multi-candidate set has a group");
    }
    // The placeholder self-edge is gone — never stored as a resolved edge.
    assert!(
        !g.edge_records().any(|e| e.rule.starts_with("indirect")),
        "indirect placeholder is replaced, not left behind"
    );
}

#[test]
fn no_compatible_target_leaves_the_call_honestly_dangling() {
    // An indirect call of arity 3 with no arity-3 function value: the placeholder
    // is dropped (no edge invented), counted as unmatched.
    let mut b = FileBuilder::new();
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(1));
    b.def("m::inc", SymbolKind::Function, ROOT, 2, PUB, false, Some(1));
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.raw_ref(
        &["cb"],
        run_scope,
        5,
        RefKind::CallCallback,
        EdgeCondition::Always,
        Some(3),
    );
    let facts = b.build();
    let mut g = link_facts(&facts);

    let stats = run_sig(&mut g);
    assert_eq!(stats.sites_resolved, 0);
    assert_eq!(stats.sites_unmatched, 1, "no arity-3 candidate → unmatched");
    assert_eq!(sig_candidate_dsts(&g).len(), 0, "no sig-compat edge emitted");
    assert!(
        !g.edge_records().any(|e| e.rule.starts_with("indirect")),
        "the placeholder is dropped, not stored"
    );
}

#[test]
fn unknown_call_site_arity_admits_every_function_value() {
    // When the source does not pin the call-site arity (`indirect:?`), the set is
    // every Lambda + free Function — the soundest over-approximation.
    let mut b = FileBuilder::new();
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    b.def("m::a", SymbolKind::Function, ROOT, 2, PUB, false, Some(1));
    b.def("m::b", SymbolKind::Function, ROOT, 3, PUB, false, Some(2));
    b.def("m::{closure@5:5}", SymbolKind::Lambda, ROOT, 5, PUB, false, Some(0));
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.raw_ref(
        &["cb"],
        run_scope,
        6,
        RefKind::CallClosure,
        EdgeCondition::Always,
        None, // arity unknown
    );
    let facts = b.build();
    let mut g = link_facts(&facts);
    assert!(
        g.edge_records()
            .any(|e| e.rule == "indirect:?" && e.kind == EdgeKind::CallsClosure),
        "arity-less placeholder encodes `indirect:?`"
    );

    let stats = run_sig(&mut g);
    assert_eq!(stats.sites_resolved, 1);
    // run, a, b, and the closure are all function values (run is the caller but
    // also a free fn value); all admitted under unknown arity.
    let expected: BTreeSet<NodeId> = ["m::run", "m::a", "m::b", "m::{closure@5:5}"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(sig_candidate_dsts(&g), expected);
}

#[test]
fn methods_are_excluded_only_free_functions_and_closures_are_candidates() {
    // A method (a Function whose FQN parent is a Type) cannot flow as a free fn
    // value, so it is not a candidate even when its arity matches.
    let mut b = FileBuilder::new();
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(1));
    b.def("m::Widget", SymbolKind::Type, ROOT, 2, PUB, false, None);
    // A method on Widget with matching arity — must be excluded.
    b.def("m::Widget::tick", SymbolKind::Method, ROOT, 3, PUB, false, Some(1));
    // A free fn with matching arity — included.
    b.def("m::free", SymbolKind::Function, ROOT, 4, PUB, false, Some(1));
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.raw_ref(
        &["cb"],
        run_scope,
        5,
        RefKind::CallCallback,
        EdgeCondition::Always,
        Some(1),
    );
    let facts = b.build();
    let mut g = link_facts(&facts);
    run_sig(&mut g);

    let dsts = sig_candidate_dsts(&g);
    assert!(dsts.contains(&node_id(&g, "m::free")), "free fn is a candidate");
    assert!(dsts.contains(&node_id(&g, "m::run")), "the caller free fn is too");
    assert!(
        !dsts.contains(&node_id(&g, "m::Widget::tick")),
        "a method (parent is a Type) is not a function value"
    );
}

#[test]
fn supernode_cap_keeps_the_full_set_with_a_cut_marker() {
    // An arity that matches many free functions (> CHA_SUPERNODE_CAP): the group is
    // kept in full, every edge cut-marked `Unresolved`, nothing dropped (LS-6).
    let n = CHA_SUPERNODE_CAP + 5;
    let mut b = FileBuilder::new();
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    for i in 0..n {
        b.def(
            &format!("m::f{i:03}"),
            SymbolKind::Function,
            ROOT,
            (i + 2) as u32,
            PUB,
            false,
            Some(1),
        );
    }
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.raw_ref(
        &["cb"],
        run_scope,
        500,
        RefKind::CallCallback,
        EdgeCondition::Always,
        Some(1),
    );
    let facts = b.build();
    let mut g = link_facts(&facts);
    let stats = run_sig(&mut g);

    assert_eq!(stats.sites_resolved, 1);
    assert_eq!(stats.supernode_sites, 1, "the over-cap site is counted");

    let sig_edges: Vec<_> = g.edge_records().filter(|e| e.rule == "sig-compat").collect();
    assert_eq!(sig_edges.len(), n, "all {n} candidates kept — nothing dropped");
    for e in &sig_edges {
        assert!(
            e.cut_markers.contains(CutMarker::Unresolved),
            "every over-cap candidate is cut-marked"
        );
    }
}

#[test]
fn candidates_are_partitioned_by_source_language() {
    // A TS CallClosure at a call site of arity 1: candidates are restricted to
    // TS Lambda/free-Function values (the call site's language). A same-arity
    // function in a *different* language (rust) must never be a candidate — a
    // bare TS closure invocation cannot reach a non-TS function value without an
    // intermediating JS-visible binding (F1c verdict). A same-language,
    // same-arity function is a legitimate candidate and must be preserved.
    let mut ts = FileBuilder::new();
    ts.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    ts.def("m::double", SymbolKind::Function, ROOT, 2, PUB, false, Some(1));
    let run_scope = ts.scope(ROOT, Some("m::run"));
    ts.raw_ref(
        &["cb"],
        run_scope,
        3,
        RefKind::CallClosure,
        EdgeCondition::Always,
        Some(1),
    );
    let ts_facts = ts.build();

    let mut rs = FileBuilder::new();
    rs.def("x::add_one", SymbolKind::Function, ROOT, 1, PUB, false, Some(1));
    let rs_facts = rs.build();

    let inputs = vec![
        FileInput::new("blob-ts".to_string(), "src/m.ts", "typescript", &ts_facts),
        FileInput::new("blob-rs".to_string(), "src/x.rs", "rust", &rs_facts),
    ];
    let mut g = link(&inputs, &LinkOpts::default());

    let stats = run_sig(&mut g);
    assert_eq!(stats.sites_resolved, 1);

    let dsts = sig_candidate_dsts(&g);
    assert!(
        dsts.contains(&node_id(&g, "m::double")),
        "same-language same-arity candidate is preserved"
    );
    assert!(
        !dsts.contains(&node_id(&g, "x::add_one")),
        "cross-language same-arity candidate is excluded"
    );
}

#[test]
fn re_running_the_signature_pass_is_byte_identical() {
    // Determinism (§7): link + run_sig twice → identical graphs.
    let facts = callback_with_arity_one_and_two();

    let mut g1 = link_facts(&facts);
    run_sig(&mut g1);
    let mut g2 = link_facts(&facts);
    run_sig(&mut g2);

    assert_eq!(g1.edges, g2.edges, "edges are byte-identical across runs");
    assert_eq!(
        g1.candidates, g2.candidates,
        "candidate rows are byte-identical across runs"
    );
}
