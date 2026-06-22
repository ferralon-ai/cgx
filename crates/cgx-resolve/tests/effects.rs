//! GM-12 transitive effect-closure (P8b): [`run_effect_closure`] unions a
//! function's syntactic `own_effects` (stamped in P8a) with the transitive effects
//! of everything it calls over the call family — but **not** over `Spawns` edges
//! (design §2.2 (E), §8).
//!
//! These tests pin the four behaviors the P8b dispatch calls out:
//! - **Chain** `a → b → c`: `c`'s own `io.net` reaches `a` transitively, while
//!   `a.own_effects` stays clean.
//! - **Spawn boundary** `s` spawns `w`: `w`'s own `io.file` is NOT unioned into
//!   `s.transitive_effects` (the `Spawns` edge does not propagate effects).
//! - **Cycle** `f ↔ g`: a single own-effect on one member reaches both via the SCC
//!   fixpoint, and the closure terminates (no hang on mutual recursion).
//! - **Determinism**: the closure is order-independent (it only unions a `u16`
//!   bitset over sorted node/edge order); the store round-trip + byte-identical
//!   re-index lives in the cgx-index integration suite.

mod common;

use cgx_core::edge::EdgeKind;
use cgx_core::id::NodeId;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::{Effect, EffectSet};
use cgx_frontend::facts::{FileFacts, RefKind, ScopeId};
use cgx_resolve::{link, run_effect_closure, FileInput, LinkOpts, ResolvedGraph};

use cgx_core::condition::EdgeCondition;

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

/// Stamp a node's `own_effects` (simulating the P8a frontend pass) so the closure
/// has something to propagate.
fn set_own(g: &mut ResolvedGraph, fqn: &str, effects: &[Effect]) {
    let set = EffectSet::from_iter_canonical(effects.iter().copied());
    let n = g
        .nodes
        .iter_mut()
        .find(|n| n.node.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"));
    n.node.own_effects = set;
}

fn transitive(g: &ResolvedGraph, fqn: &str) -> EffectSet {
    g.node_records()
        .find(|n| n.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"))
        .transitive_effects
}

fn own(g: &ResolvedGraph, fqn: &str) -> EffectSet {
    g.node_records()
        .find(|n| n.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"))
        .own_effects
}

/// `a` calls `b` calls `c`. Only `c` has the `io.net` own-effect.
fn chain_fixture() -> FileFacts {
    let mut b = FileBuilder::new();
    let a_scope = b.scope(ROOT, Some("m::a"));
    let b_scope = b.scope(ROOT, Some("m::b"));
    b.def("m::a", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    b.def("m::b", SymbolKind::Function, ROOT, 5, PUB, false, Some(0));
    b.def("m::c", SymbolKind::Function, ROOT, 9, PUB, false, Some(0));
    b.call(&["b"], a_scope, 2);
    b.call(&["c"], b_scope, 6);
    b.build()
}

#[test]
fn chain_propagates_callee_effects_to_root_without_dirtying_own() {
    let facts = chain_fixture();
    let mut g = link_facts(&facts);
    set_own(&mut g, "m::c", &[Effect::IoNet]);

    run_effect_closure(&mut g);

    // a's transitive set contains io.net (reached through b → c) ...
    assert!(
        transitive(&g, "m::a").contains(Effect::IoNet),
        "a.transitive_effects must contain io.net (a -> b -> c)"
    );
    // ... and so does b's (the intermediate).
    assert!(
        transitive(&g, "m::b").contains(Effect::IoNet),
        "b.transitive_effects must contain io.net (b -> c)"
    );
    // But a's OWN effects are untouched — io.net is not something a does directly.
    assert!(
        !own(&g, "m::a").contains(Effect::IoNet),
        "a.own_effects must NOT contain io.net"
    );
    // c's own and transitive both carry it.
    assert!(transitive(&g, "m::c").contains(Effect::IoNet));
}

/// `s` spawns `w` (a `Spawns` edge). `w` has the `io.file` own-effect.
fn spawn_fixture() -> FileFacts {
    let mut b = FileBuilder::new();
    let s_scope = b.scope(ROOT, Some("m::s"));
    b.def("m::s", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    b.def("m::w", SymbolKind::Function, ROOT, 5, PUB, false, Some(0));
    // A detached spawn of `w` from `s`.
    b.raw_ref(&["w"], s_scope, 2, RefKind::Spawn, EdgeCondition::Always, None);
    b.build()
}

#[test]
fn spawn_boundary_does_not_union_spawned_effects() {
    let facts = spawn_fixture();
    let mut g = link_facts(&facts);

    // Confirm the fixture really produced a Spawns edge s -> w.
    let s = node_id(&g, "m::s");
    let w = node_id(&g, "m::w");
    assert!(
        g.edge_records()
            .any(|e| e.kind == EdgeKind::Spawns && e.src == s && e.dst == w),
        "fixture must contain a Spawns edge s -> w"
    );

    set_own(&mut g, "m::w", &[Effect::IoFile]);

    run_effect_closure(&mut g);

    // The spawn boundary: w's io.file is reachable VIA the Spawns edge but is NOT
    // unioned into the spawner's transitive set (design §2.2 (E), GM-12).
    assert!(
        !transitive(&g, "m::s").contains(Effect::IoFile),
        "io.file must NOT be in s.transitive_effects (Spawns does not propagate)"
    );
    // w itself still carries it transitively.
    assert!(transitive(&g, "m::w").contains(Effect::IoFile));
}

/// Mutually-recursive `f ↔ g`. `f` has the `blocking` own-effect; `g` has none.
fn cycle_fixture() -> FileFacts {
    let mut b = FileBuilder::new();
    let f_scope = b.scope(ROOT, Some("m::f"));
    let g_scope = b.scope(ROOT, Some("m::g"));
    b.def("m::f", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    b.def("m::g", SymbolKind::Function, ROOT, 5, PUB, false, Some(0));
    b.call(&["g"], f_scope, 2);
    b.call(&["f"], g_scope, 6);
    b.build()
}

#[test]
fn cycle_propagates_to_all_members_and_terminates() {
    let facts = cycle_fixture();
    let mut g = link_facts(&facts);
    set_own(&mut g, "m::f", &[Effect::Blocking]);

    // Termination is the implicit assertion: a naive recursive union would hang on
    // f ↔ g. The SCC fixpoint must finish.
    run_effect_closure(&mut g);

    // Both members of the cycle see the effect (SCC = union of all members' own).
    assert!(
        transitive(&g, "m::f").contains(Effect::Blocking),
        "f.transitive_effects must contain blocking"
    );
    assert!(
        transitive(&g, "m::g").contains(Effect::Blocking),
        "g.transitive_effects must contain blocking (reached through the f<->g cycle)"
    );
    // g's OWN effects stay empty — it performs no blocking call directly.
    assert!(
        own(&g, "m::g").is_empty(),
        "g.own_effects must stay empty"
    );
}

#[test]
fn closure_is_independent_of_node_order() {
    // The closure must be a pure function of the graph, not of vector order. Run it
    // on the chain fixture, then on a copy whose nodes/edges are reversed, and
    // assert every fqn's transitive set matches.
    let facts = chain_fixture();
    let mut g1 = link_facts(&facts);
    set_own(&mut g1, "m::c", &[Effect::IoNet]);
    let mut g2 = g1.clone();
    g2.nodes.reverse();
    g2.edges.reverse();

    run_effect_closure(&mut g1);
    run_effect_closure(&mut g2);

    for fqn in ["m::a", "m::b", "m::c"] {
        assert_eq!(
            transitive(&g1, fqn),
            transitive(&g2, fqn),
            "transitive_effects for {fqn} must be order-independent"
        );
    }
}

#[test]
fn diamond_unions_both_branches() {
    // a -> b -> d, a -> c -> d; b has io.file, c has io.net, d has io.proc.
    // a must see all three; b sees io.file+io.proc; c sees io.net+io.proc.
    let mut bldr = FileBuilder::new();
    let a_scope = bldr.scope(ROOT, Some("m::a"));
    let b_scope = bldr.scope(ROOT, Some("m::b"));
    let c_scope = bldr.scope(ROOT, Some("m::c"));
    let d_scope = bldr.scope(ROOT, Some("m::d"));
    let _ = d_scope;
    bldr.def("m::a", SymbolKind::Function, ROOT, 1, PUB, false, Some(0));
    bldr.def("m::b", SymbolKind::Function, ROOT, 5, PUB, false, Some(0));
    bldr.def("m::c", SymbolKind::Function, ROOT, 9, PUB, false, Some(0));
    bldr.def("m::d", SymbolKind::Function, ROOT, 13, PUB, false, Some(0));
    bldr.call(&["b"], a_scope, 2);
    bldr.call(&["c"], a_scope, 3);
    bldr.call(&["d"], b_scope, 6);
    bldr.call(&["d"], c_scope, 10);
    let facts = bldr.build();

    let mut g = link_facts(&facts);
    set_own(&mut g, "m::b", &[Effect::IoFile]);
    set_own(&mut g, "m::c", &[Effect::IoNet]);
    set_own(&mut g, "m::d", &[Effect::IoProc]);

    run_effect_closure(&mut g);

    let a = transitive(&g, "m::a");
    assert!(a.contains(Effect::IoFile) && a.contains(Effect::IoNet) && a.contains(Effect::IoProc));
    let b = transitive(&g, "m::b");
    assert!(b.contains(Effect::IoFile) && b.contains(Effect::IoProc) && !b.contains(Effect::IoNet));
}
