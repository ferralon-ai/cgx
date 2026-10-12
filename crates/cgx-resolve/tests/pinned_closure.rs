//! Pinned closure calls (`RefKind::CallPinnedClosure`) bind lexically to their
//! one `Lambda` definition before the indirect signature fan-out is considered.
//!
//! The frontend emits the pinned kind only when it has proved the binding is
//! immutable, unshadowed and in scope (TypeScript `const f = () => …; f()`).
//! These tests pin the resolver half of that contract:
//!
//! - a pinned call whose `Lambda` is in an enclosing scope yields exactly one
//!   `certain` `scope-ref` `calls-closure` edge, and the signature pass adds
//!   nothing for it;
//! - an unpinned `CallClosure` with the same facts keeps the indirect fan-out;
//! - a pinned call the resolver cannot bind (no `Lambda` in an enclosing scope,
//!   or two same-named lambdas in the nearest one) falls back to the indirect
//!   placeholder and its signature-compatible set, never to a guessed target;
//! - a class member with the callee's name is not a lexical binding and is
//!   passed over.

mod common;

use std::collections::BTreeSet;

use cgx_core::condition::EdgeCondition;
use cgx_core::confidence::{Confidence, Tier};
use cgx_core::edge::EdgeKind;
use cgx_core::id::NodeId;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_frontend::facts::{FileFacts, RefKind, ScopeId};
use cgx_resolve::{link, run_sig, FileInput, LinkOpts, ResolvedGraph};

use common::FileBuilder;

const PUB: Visibility = Visibility::Public;
const ROOT: ScopeId = ScopeId::ROOT;

fn link_ts(facts: &FileFacts) -> ResolvedGraph {
    let inputs = vec![FileInput::new(
        "blob".to_string(),
        "src/m.ts",
        "typescript",
        facts,
    )];
    let mut g = link(&inputs, &LinkOpts::default());
    run_sig(&mut g);
    g
}

fn node_id(g: &ResolvedGraph, fqn: &str) -> NodeId {
    g.node_records()
        .find(|n| n.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"))
        .id
}

fn closure_dsts(g: &ResolvedGraph, src: NodeId) -> BTreeSet<NodeId> {
    g.edge_records()
        .filter(|e| e.src == src && e.kind == EdgeKind::CallsClosure)
        .map(|e| e.dst)
        .collect()
}

/// `function run() { const f = (x) => …; items.forEach(() => f(1)); }` plus an
/// unrelated arity-1 free function `g`, as facts: the call to `f` sits in an
/// anonymous arrow scope nested inside `run`'s scope, where `f` is defined.
fn nested_closure_call(kind: RefKind) -> FileFacts {
    let mut b = FileBuilder::new();
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    b.def("m::g", SymbolKind::Function, ROOT, 9, PUB, false, Some(1));
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.def("m::run::f", SymbolKind::Lambda, run_scope, 2, PUB, false, Some(1));
    let arrow_scope = b.scope(run_scope, None);
    b.raw_ref(&["f"], arrow_scope, 3, kind, EdgeCondition::Always, Some(1));
    b.build()
}

#[test]
fn pinned_call_binds_to_its_lambda_in_an_enclosing_scope() {
    let g = link_ts(&nested_closure_call(RefKind::CallPinnedClosure));
    let run = node_id(&g, "m::run");
    let f = node_id(&g, "m::run::f");

    let edges: Vec<_> = g
        .edge_records()
        .filter(|e| e.src == run && e.kind == EdgeKind::CallsClosure)
        .collect();
    assert_eq!(edges.len(), 1, "exactly one edge for the pinned call");
    let e = edges[0];
    assert_eq!(e.dst, f);
    assert_eq!(e.confidence, Confidence::Certain);
    assert_eq!(e.tier, Tier::ScopeGraph);
    assert_eq!(e.rule, "scope-ref");
    assert!(e.candidate_group.is_none());
    assert!(
        !g.edge_records()
            .any(|e| e.rule == "sig-compat" || e.rule.starts_with("indirect")),
        "no indirect placeholder and no signature fan-out"
    );
}

#[test]
fn unpinned_closure_call_keeps_the_signature_fan_out() {
    // Same facts, but the frontend could not pin the binding (e.g. `let f`
    // reassigned, or a parameter named `f` in between): today's behaviour.
    let g = link_ts(&nested_closure_call(RefKind::CallClosure));
    let run = node_id(&g, "m::run");
    let expected: BTreeSet<NodeId> = ["m::run::f", "m::g"]
        .iter()
        .map(|fqn| node_id(&g, fqn))
        .collect();
    assert_eq!(closure_dsts(&g, run), expected);
    assert!(g
        .edge_records()
        .filter(|e| e.src == run && e.kind == EdgeKind::CallsClosure)
        .all(|e| e.rule == "sig-compat" && e.confidence == Confidence::Possible));
}

#[test]
fn pinned_call_without_a_lambda_in_scope_falls_back_to_the_fan_out() {
    // `f` is defined in a sibling function, not an enclosing scope.
    let mut b = FileBuilder::new();
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    b.def("m::other", SymbolKind::Function, ROOT, 5, PUB, false, Some(0));
    let run_scope = b.scope(ROOT, Some("m::run"));
    let other_scope = b.scope(ROOT, Some("m::other"));
    b.def("m::other::f", SymbolKind::Lambda, other_scope, 6, PUB, false, Some(1));
    b.raw_ref(
        &["f"],
        run_scope,
        2,
        RefKind::CallPinnedClosure,
        EdgeCondition::Always,
        Some(1),
    );
    let facts = b.build();

    let inputs = vec![FileInput::new(
        "blob".to_string(),
        "src/m.ts",
        "typescript",
        &facts,
    )];
    let g = link(&inputs, &LinkOpts::default());
    assert!(
        g.edge_records()
            .any(|e| e.rule == "indirect:1" && e.kind == EdgeKind::CallsClosure),
        "an unbindable pinned call becomes the indirect placeholder"
    );

    let g = link_ts(&facts);
    let run = node_id(&g, "m::run");
    assert_eq!(
        closure_dsts(&g, run),
        [node_id(&g, "m::other::f")].into_iter().collect(),
        "the arity-1 function values, at `possible`"
    );
    assert!(g
        .edge_records()
        .filter(|e| e.src == run && e.kind == EdgeKind::CallsClosure)
        .all(|e| e.rule == "sig-compat" && e.confidence == Confidence::Possible));
}

#[test]
fn two_same_named_lambdas_in_the_nearest_scope_are_not_guessed_between() {
    // `if (a) { const f = … } else { const f = … }` puts both lambdas in the
    // function's scope; the resolver must not pick one.
    let mut b = FileBuilder::new();
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    let run_scope = b.scope(ROOT, Some("m::run"));
    b.def("m::run::f", SymbolKind::Lambda, run_scope, 2, PUB, false, Some(1));
    b.def("m::run::f", SymbolKind::Lambda, run_scope, 4, PUB, false, Some(1));
    b.raw_ref(
        &["f"],
        run_scope,
        6,
        RefKind::CallPinnedClosure,
        EdgeCondition::Always,
        Some(1),
    );
    let g = link_ts(&b.build());
    assert!(
        !g.edge_records().any(|e| e.rule == "scope-ref"),
        "no lexical edge when the nearest scope is ambiguous"
    );
    assert!(g.edge_records().any(|e| e.rule == "sig-compat"));
}

#[test]
fn a_lambda_whose_fqn_is_shared_with_another_node_is_not_guessed_between() {
    // `run() { xs.map(() => { const f = …; f(1) }); ys.map(() => { const f = … }) }`:
    // both lambdas live in different anonymous scopes but share the owner's FQN
    // `m::run::f`, so the FQN does not identify the node the call means.
    let mut b = FileBuilder::new();
    b.def("m::run", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    let run_scope = b.scope(ROOT, Some("m::run"));
    let a1 = b.scope(run_scope, None);
    let a2 = b.scope(run_scope, None);
    b.def("m::run::f", SymbolKind::Lambda, a1, 2, PUB, false, Some(1));
    b.def("m::run::f", SymbolKind::Lambda, a2, 5, PUB, false, Some(1));
    b.raw_ref(
        &["f"],
        a1,
        3,
        RefKind::CallPinnedClosure,
        EdgeCondition::Always,
        Some(1),
    );
    let g = link_ts(&b.build());
    assert!(!g.edge_records().any(|e| e.rule == "scope-ref"));
    assert!(g.edge_records().any(|e| e.rule == "sig-compat"));
}

#[test]
fn a_class_member_named_like_the_callee_is_not_a_lexical_binding() {
    // const f = (x) => …;  class C { f() {}  m() { f(1); } }
    // The bare `f(1)` inside `C::m` refers to the module-level const, not the
    // method `C::f`.
    let mut b = FileBuilder::new();
    b.def("m::f", SymbolKind::Lambda, ROOT, 1, PUB, false, Some(1));
    b.def("m::C", SymbolKind::Type, ROOT, 2, PUB, false, None);
    let class_scope = b.scope(ROOT, Some("m::C"));
    b.def("m::C::f", SymbolKind::Method, class_scope, 3, PUB, false, Some(1));
    b.def("m::C::m", SymbolKind::Method, class_scope, 4, PUB, false, Some(0));
    let m_scope = b.scope(class_scope, Some("m::C::m"));
    b.raw_ref(
        &["f"],
        m_scope,
        5,
        RefKind::CallPinnedClosure,
        EdgeCondition::Always,
        Some(1),
    );
    let g = link_ts(&b.build());
    let m = node_id(&g, "m::C::m");
    assert_eq!(
        closure_dsts(&g, m),
        [node_id(&g, "m::f")].into_iter().collect()
    );
    let e = g
        .edge_records()
        .find(|e| e.src == m && e.kind == EdgeKind::CallsClosure)
        .unwrap();
    assert_eq!(e.confidence, Confidence::Certain);
    assert_eq!(e.rule, "scope-ref");
}
