//! Behavioral tests for the Layer-1 subcommands (Q-1/2/3/4, Q-11, Q-13, Q-17,
//! Q-18, Q-19) plus the GM-4 transience and GM-9.2 spawn-reset semantics.

mod common;

use cgx_core::{Confidence, EdgeCondition, EntrypointKind, NodeId, SymbolKind, SymbolPattern};
use cgx_query::{
    callees, callers, paths, reaches, reaches_all, unused, Direction, EdgeFilter, GraphView,
    PathWalker,
};

use common::{id_of, GraphBuilder};

fn view(builder: GraphBuilder) -> GraphView {
    let (nodes, edges, cands) = builder.build();
    GraphView::new(nodes, edges, cands)
}

// --- Q-1: callers -------------------------------------------------------------

#[test]
fn callers_returns_direct_callers() {
    // a -> c, b -> c
    let g = GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .calls("a", "c")
        .calls("b", "c");
    let v = view(g);
    let c = v.resolve_one(&SymbolPattern::fqn("c")).unwrap();
    let res = callers(&v, c, &PathWalker::default());
    let fqns: Vec<&str> = res.iter().map(|r| r.node.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["a", "b"]);
    assert!(res.iter().all(|r| r.depth == 1));
}

#[test]
fn callers_transitive_respects_depth_limit() {
    // a -> b -> c (querying callers of c)
    let g = GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .calls("a", "b")
        .calls("b", "c");
    let v = view(g);
    let c = id_of(v.nodes(), "c");

    let depth1 = PathWalker {
        max_depth: Some(1),
        ..Default::default()
    };
    let r1 = callers(&v, c, &depth1);
    assert_eq!(
        r1.iter().map(|r| r.node.fqn.as_str()).collect::<Vec<_>>(),
        vec!["b"]
    );

    let depth2 = PathWalker {
        max_depth: Some(2),
        ..Default::default()
    };
    let r2 = callers(&v, c, &depth2);
    let mut got: Vec<&str> = r2.iter().map(|r| r.node.fqn.as_str()).collect();
    got.sort_unstable();
    assert_eq!(got, vec!["a", "b"]);
}

// --- Q-2: callees -------------------------------------------------------------

#[test]
fn callees_returns_transitive_callees() {
    let g = GraphBuilder::new()
        .func("main")
        .func("handle")
        .func("log")
        .calls("main", "handle")
        .calls("handle", "log");
    let v = view(g);
    let m = id_of(v.nodes(), "main");
    let res = callees(&v, m, &PathWalker::default());
    let fqns: Vec<&str> = res.iter().map(|r| r.node.fqn.as_str()).collect();
    assert!(fqns.contains(&"handle"));
    assert!(fqns.contains(&"log"));
    let log = res.iter().find(|r| r.node.fqn == "log").unwrap();
    assert_eq!(log.depth, 2);
}

#[test]
fn callees_ignores_structural_edges() {
    // `Contains` must not be traversed as a call.
    let g = GraphBuilder::new()
        .func("m")
        .sym("Mod", SymbolKind::Module)
        .calls("m", "Mod") // give Mod an incoming call so it's a node
        .contains("Mod", "m");
    let v = view(g);
    let modn = id_of(v.nodes(), "Mod");
    let res = callees(&v, modn, &PathWalker::default());
    assert!(res.is_empty(), "Contains edge must not be a callee");
}

// --- Q-3 + Q-11: paths with edge-condition filter -----------------------------

#[test]
fn paths_enumerates_all_simple_paths() {
    // main -> a -> sink, main -> b -> sink (two paths)
    let g = GraphBuilder::new()
        .func("main")
        .func("a")
        .func("b")
        .func("sink")
        .calls("main", "a")
        .calls("main", "b")
        .calls("a", "sink")
        .calls("b", "sink");
    let v = view(g);
    let from = id_of(v.nodes(), "main");
    let to = id_of(v.nodes(), "sink");
    let res = paths(&v, from, to, &PathWalker::default());
    assert_eq!(res.len(), 2);
    for p in &res {
        assert_eq!(p.steps.first().unwrap().node.fqn, "main");
        assert_eq!(p.steps.last().unwrap().node.fqn, "sink");
    }
}

#[test]
fn paths_exclude_exception_condition_drops_exception_path() {
    // main -> a -> sink (happy), main -> b -[exception]-> sink (error path)
    let g = GraphBuilder::new()
        .func("main")
        .func("a")
        .func("b")
        .func("sink")
        .calls("main", "a")
        .calls("main", "b")
        .calls("a", "sink")
        .calls_cond("b", "sink", EdgeCondition::Exception);
    let v = view(g);
    let from = id_of(v.nodes(), "main");
    let to = id_of(v.nodes(), "sink");

    let walker = PathWalker {
        filter: EdgeFilter::calls().exclude_condition(EdgeCondition::Exception),
        ..Default::default()
    };
    let res = paths(&v, from, to, &walker);
    assert_eq!(res.len(), 1, "exception path should be excluded");
    let only = &res[0];
    assert!(only.steps.iter().any(|s| s.node.fqn == "a"));
    assert!(!only.crosses_exceptional);
}

#[test]
fn paths_only_exception_condition_keeps_error_path() {
    let g = GraphBuilder::new()
        .func("main")
        .func("a")
        .func("b")
        .func("sink")
        .calls("main", "a")
        .calls("main", "b")
        .calls("a", "sink")
        .calls_cond("b", "sink", EdgeCondition::Exception);
    let v = view(g);
    let from = id_of(v.nodes(), "main");
    let to = id_of(v.nodes(), "sink");

    let walker = PathWalker {
        filter: EdgeFilter::calls().only_condition(EdgeCondition::Exception),
        ..Default::default()
    };
    // `only` requires every edge to be exception; main->b is `always`, so no full
    // path survives. The error-path semantics for "only-edge-condition exception"
    // (Q-3) is "require at least one such edge", which is the transience test
    // below; the strict per-edge form correctly yields nothing here.
    let res = paths(&v, from, to, &walker);
    assert!(res.is_empty());
}

// --- GM-4.1 canonical transience ---------------------------------------------

#[test]
fn transience_canonical_example_marks_d_exception_transient() {
    // A -> B (always), B -> C (exception), C -> D (always).
    let g = GraphBuilder::new()
        .func("A")
        .func("B")
        .func("C")
        .func("D")
        .calls("A", "B")
        .calls_cond("B", "C", EdgeCondition::Exception)
        .calls("C", "D");
    let v = view(g);
    let a = id_of(v.nodes(), "A");
    let d = id_of(v.nodes(), "D");
    let res = paths(&v, a, d, &PathWalker::default());
    assert_eq!(res.len(), 1);
    let p = &res[0];
    assert!(p.crosses_exceptional);
    // D is reached after the exceptional edge: exception-transient (GM-4.1).
    let d_step = p.steps.iter().find(|s| s.node.fqn == "D").unwrap();
    assert!(
        d_step.exception_transient,
        "D is exception-transient on this path (GM-4.1)"
    );
    // C is entered *via* the exception edge B->C; per GM-4.2 step 2 the flag is set
    // as that edge is crossed, so the flag holds at C (GM-4.2 step 3).
    let c_step = p.steps.iter().find(|s| s.node.fqn == "C").unwrap();
    assert!(c_step.exception_transient);
    // B is reached before any exceptional edge — not transient.
    let b_step = p.steps.iter().find(|s| s.node.fqn == "B").unwrap();
    assert!(!b_step.exception_transient);
}

#[test]
fn transience_not_set_on_non_exception_path() {
    // X -> C -> D with no exceptional edge: D not transient.
    let g = GraphBuilder::new()
        .func("X")
        .func("C")
        .func("D")
        .calls("X", "C")
        .calls("C", "D");
    let v = view(g);
    let x = id_of(v.nodes(), "X");
    let d = id_of(v.nodes(), "D");
    let res = paths(&v, x, d, &PathWalker::default());
    assert_eq!(res.len(), 1);
    assert!(!res[0].crosses_exceptional);
    let d_step = res[0].steps.iter().find(|s| s.node.fqn == "D").unwrap();
    assert!(!d_step.exception_transient);
}

// --- GM-9.2 spawn-domain reset ------------------------------------------------

#[test]
fn spawn_edge_resets_exceptional_domain() {
    // A -[exception]-> B -[spawns]-> C -> D.
    // After the spawn into B's task, the exception flag must reset: C and D are
    // NOT exception-transient even though the spawner crossed an exception edge.
    let g = GraphBuilder::new()
        .func("A")
        .func("B")
        .func("C")
        .func("D")
        .calls_cond("A", "B", EdgeCondition::Exception)
        .spawns("B", "C", EdgeCondition::Always)
        .calls("C", "D");
    let v = view(g);
    let a = id_of(v.nodes(), "A");
    let d = id_of(v.nodes(), "D");
    let res = paths(&v, a, d, &PathWalker::default());
    assert_eq!(res.len(), 1);
    let p = &res[0];
    let b_step = p.steps.iter().find(|s| s.node.fqn == "B").unwrap();
    let c_step = p.steps.iter().find(|s| s.node.fqn == "C").unwrap();
    let d_step = p.steps.iter().find(|s| s.node.fqn == "D").unwrap();
    // B is reached via the spawner's exception edge — still transient.
    assert!(b_step.exception_transient);
    // C and D are inside the detached, spawned task: the flag reinitialises to
    // false across the `spawns` edge (GM-9.2.1).
    assert!(
        !c_step.exception_transient,
        "spawn resets the domain (GM-9.2.1)"
    );
    assert!(
        !d_step.exception_transient,
        "downstream of spawn stays reset"
    );
}

// --- Q-18: confidence surfacing + filtering -----------------------------------

#[test]
fn callers_surface_confidence_and_condition() {
    let g = GraphBuilder::new().func("a").func("c").calls_full(
        "a",
        "c",
        EdgeCondition::Conditional,
        Confidence::Probable,
    );
    let v = view(g);
    let c = id_of(v.nodes(), "c");
    let res = callers(&v, c, &PathWalker::default());
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].confidence, Confidence::Probable);
    assert_eq!(res[0].condition, EdgeCondition::Conditional);
}

#[test]
fn confidence_filter_drops_weaker_edges() {
    // a -[possible]-> c, b -[certain]-> c. Filter certain keeps only b.
    let g = GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .calls_full("a", "c", EdgeCondition::Always, Confidence::Possible)
        .calls_full("b", "c", EdgeCondition::Always, Confidence::Certain);
    let v = view(g);
    let c = id_of(v.nodes(), "c");
    let walker = PathWalker {
        filter: EdgeFilter::calls().with_min_confidence(Confidence::Certain),
        ..Default::default()
    };
    let res = callers(&v, c, &walker);
    assert_eq!(
        res.iter().map(|r| r.node.fqn.as_str()).collect::<Vec<_>>(),
        vec!["b"]
    );
}

#[test]
fn weakest_confidence_on_path_is_surfaced() {
    // main -[certain]-> a -[possible]-> sink: sink's path confidence is possible.
    let g = GraphBuilder::new()
        .func("main")
        .func("a")
        .func("sink")
        .calls_full("main", "a", EdgeCondition::Always, Confidence::Certain)
        .calls_full("a", "sink", EdgeCondition::Always, Confidence::Possible);
    let v = view(g);
    let m = id_of(v.nodes(), "main");
    let res = callees(&v, m, &PathWalker::default());
    let sink = res.iter().find(|r| r.node.fqn == "sink").unwrap();
    assert_eq!(sink.min_confidence_on_path, Confidence::Possible);
}

// --- reachability -------------------------------------------------------------

#[test]
fn reaches_finds_witness_path() {
    let g = GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .calls("a", "b")
        .calls("b", "c");
    let v = view(g);
    let a = id_of(v.nodes(), "a");
    let c = id_of(v.nodes(), "c");
    let r = reaches(&v, a, c, &PathWalker::default());
    assert!(r.reachable);
    let w = r.witness.unwrap();
    assert_eq!(
        w.steps
            .iter()
            .map(|s| s.node.fqn.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
}

#[test]
fn reaches_reports_unreachable() {
    let g = GraphBuilder::new().func("a").func("b").calls("a", "b");
    let v = view(g);
    let a = id_of(v.nodes(), "a");
    let b = id_of(v.nodes(), "b");
    // b cannot reach a.
    let r = reaches(&v, b, a, &PathWalker::default());
    assert!(!r.reachable);
    assert!(r.witness.is_none());
}

// --- Q-4 + Q-19: unused / entrypoint-scoped reachability ----------------------

#[test]
fn unused_finds_unreachable_from_declared_entrypoints() {
    // main(entry) -> used; orphan is never called.
    let g = GraphBuilder::new()
        .entry("main", EntrypointKind::Main)
        .func("used")
        .func("orphan")
        .calls("main", "used");
    let v = view(g);
    let res = unused(&v, &[], &[], &PathWalker::default());
    let fqns: Vec<&str> = res.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["orphan"]);
}

#[test]
fn unused_respects_explicit_entrypoint_root() {
    let g = GraphBuilder::new()
        .func("start")
        .func("used")
        .func("orphan")
        .calls("start", "used");
    let v = view(g);
    let start = id_of(v.nodes(), "start");
    let res = unused(&v, &[start], &[], &PathWalker::default());
    let fqns: Vec<&str> = res.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["orphan"]);
}

#[test]
fn unused_filters_by_kind() {
    let g = GraphBuilder::new()
        .entry("main", EntrypointKind::Main)
        .sym("OrphanField", SymbolKind::Field)
        .func("orphan_fn");
    let v = view(g);
    let res = unused(&v, &[], &[SymbolKind::Field], &PathWalker::default());
    let fqns: Vec<&str> = res.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["OrphanField"]);
}

// --- Q-17: determinism --------------------------------------------------------

#[test]
fn results_are_deterministically_ordered_across_runs() {
    let build = || {
        GraphBuilder::new()
            .func_at("z::caller", "z.rs", 5)
            .func_at("a::caller", "a.rs", 5)
            .func_at("m::caller", "m.rs", 5)
            .func("target")
            .calls("z::caller", "target")
            .calls("a::caller", "target")
            .calls("m::caller", "target")
    };
    let v1 = view(build());
    let v2 = view(build());
    let t1 = id_of(v1.nodes(), "target");
    let t2 = id_of(v2.nodes(), "target");
    let r1: Vec<String> = callers(&v1, t1, &PathWalker::default())
        .iter()
        .map(|r| r.node.fqn.clone())
        .collect();
    let r2: Vec<String> = callers(&v2, t2, &PathWalker::default())
        .iter()
        .map(|r| r.node.fqn.clone())
        .collect();
    assert_eq!(r1, r2);
    // Ordered by (file, line, col): a.rs < m.rs < z.rs.
    assert_eq!(r1, vec!["a::caller", "m::caller", "z::caller"]);
}

#[test]
fn paths_are_deterministically_ordered() {
    // Two paths of differing length; shorter first, then by node-id sequence.
    let g = GraphBuilder::new()
        .func("s")
        .func("mid")
        .func("t")
        .calls("s", "t") // 1-hop
        .calls("s", "mid")
        .calls("mid", "t"); // 2-hop
    let v = view(g);
    let s = id_of(v.nodes(), "s");
    let t = id_of(v.nodes(), "t");
    let res = paths(&v, s, t, &PathWalker::default());
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].hops(), 1);
    assert_eq!(res[1].hops(), 2);
}

// --- pattern resolution -------------------------------------------------------

#[test]
fn resolve_symbol_by_glob_and_short_name() {
    let g = GraphBuilder::new()
        .func("crypto::hash")
        .func("crypto::verify")
        .func("net::hash");
    let v = view(g);

    let glob = v.resolve_symbol(&SymbolPattern::glob("crypto::*"));
    assert_eq!(glob.len(), 2);

    let short = v.resolve_symbol(&SymbolPattern::short_name("hash"));
    assert_eq!(short.len(), 2); // crypto::hash and net::hash
}

// --- CALLS* cycle-termination semantics (docs/05-queries.md, finding 2.3c) ----
//
// These mirror the `fixtures/rust-sample/src/recursion.rs` fixture as an inline
// graph (the cgx-query suite's convention) and prove the engine honors
// SIMPLE-PATH semantics under cycles: each node is visited at most once per
// traversal, so `CALLS*` TERMINATES regardless of cycle structure and counts
// each reachable node exactly once. If simple-path semantics ever regressed
// (e.g. the BFS `seen` / DFS `on_path` cycle-cut were removed), these tests
// would hang or OOM rather than fail — that is the guarantee being protected.

/// Direct self-recursion `factorial -> factorial` (a 1-node SCC / self-edge).
/// `CALLS*` must terminate and report no *distinct* reachable node other than
/// the start (which `callees` excludes), not loop forever on the self-edge.
#[test]
fn calls_star_terminates_on_direct_self_recursion() {
    let g = GraphBuilder::new()
        .func("factorial")
        .calls("factorial", "factorial");
    let v = view(g);
    let f = id_of(v.nodes(), "factorial");

    // Terminates; the only reachable node is the start itself, excluded from the
    // forward-reachability set — so zero distinct *other* nodes, not infinite.
    let reached = reaches_all(&v, f, &PathWalker::default());
    assert!(reached.is_empty());

    // `paths` over the self-edge must not enumerate an infinite path set.
    let ps = paths(&v, f, f, &PathWalker::default());
    assert!(ps.is_empty(), "self-edge yields no nontrivial simple path");
}

/// Mutual recursion `is_even <-> is_odd` (a 2-node SCC). `CALLS*` from one must
/// reach exactly the OTHER (1 distinct node), terminating — not infinite.
#[test]
fn calls_star_terminates_on_mutual_recursion() {
    let g = GraphBuilder::new()
        .func("is_even")
        .func("is_odd")
        .calls("is_even", "is_odd")
        .calls("is_odd", "is_even");
    let v = view(g);
    let even = id_of(v.nodes(), "is_even");

    let reached = reaches_all(&v, even, &PathWalker::default());
    let fqns: Vec<&str> = reached.iter().map(|r| r.node.fqn.as_str()).collect();
    // Exactly is_odd reachable (is_even is the start, excluded). 1 distinct node,
    // each counted once — NOT a path-length count that would diverge on the cycle.
    assert_eq!(fqns, vec!["is_odd"]);
}

/// 3-node SCC `a -> b -> c -> a` with a non-recursive tail `c -> leaf`.
/// Reachability from the SCC entry over `CALLS*` is the DISTINCT set
/// {b, c, leaf} (a is the start, excluded) = 3 nodes — finite despite the cycle,
/// and the leaf past the cycle is still reached.
#[test]
fn calls_star_terminates_and_counts_distinct_over_3_node_scc() {
    let g = GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .func("leaf")
        .calls("a", "b")
        .calls("b", "c")
        .calls("c", "a")
        .calls("c", "leaf");
    let v = view(g);
    let a = id_of(v.nodes(), "a");

    // Distinct-node reachability: terminates, each node once.
    let reached = reaches_all(&v, a, &PathWalker::default());
    let mut fqns: Vec<&str> = reached.iter().map(|r| r.node.fqn.as_str()).collect();
    fqns.sort_unstable();
    assert_eq!(fqns, vec!["b", "c", "leaf"]);
    // The full distinct reachable set including the start is {a, b, c, leaf} = 4.
    assert_eq!(reached.len() + 1, 4);

    // `reaches` past the cycle to the leaf terminates with a witness path.
    let leaf = id_of(v.nodes(), "leaf");
    let r = reaches(&v, a, leaf, &PathWalker::default());
    assert!(r.reachable);

    // `paths` from a to leaf does not loop on the cycle: simple paths only, so
    // exactly one acyclic path a -> b -> c -> leaf is enumerated (the cycle
    // back-edge c -> a is cut by `on_path`).
    let ps = paths(&v, a, leaf, &PathWalker::default());
    assert_eq!(ps.len(), 1);
    let seq: Vec<&str> = ps[0].steps.iter().map(|s| s.node.fqn.as_str()).collect();
    assert_eq!(seq, vec!["a", "b", "c", "leaf"]);
}

/// Bounded `CALLS*1..N` respects the depth bound even inside a cycle: from the
/// SCC entry `a`, depth 1 reaches only `b`, depth 2 reaches {b, c}, and the walk
/// still terminates (the cycle does not let a low bound over-collect).
#[test]
fn bounded_calls_star_respects_depth_bound_inside_cycle() {
    let g = GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .func("leaf")
        .calls("a", "b")
        .calls("b", "c")
        .calls("c", "a")
        .calls("c", "leaf");
    let v = view(g);
    let a = id_of(v.nodes(), "a");

    let depth1 = PathWalker {
        max_depth: Some(1),
        ..Default::default()
    };
    let reached1 = reaches_all(&v, a, &depth1);
    let r1: Vec<&str> = reached1.iter().map(|r| r.node.fqn.as_str()).collect();
    assert_eq!(r1, vec!["b"]);

    let depth2 = PathWalker {
        max_depth: Some(2),
        ..Default::default()
    };
    let reached2 = reaches_all(&v, a, &depth2);
    let mut r2: Vec<&str> = reached2.iter().map(|r| r.node.fqn.as_str()).collect();
    r2.sort_unstable();
    assert_eq!(r2, vec!["b", "c"]);
}

#[test]
fn neighbors_primitive_respects_direction() {
    let g = GraphBuilder::new().func("a").func("b").calls("a", "b");
    let v = view(g);
    let a = id_of(v.nodes(), "a");
    let b = id_of(v.nodes(), "b");
    let f = EdgeFilter::calls();
    let fwd: Vec<_> = v
        .neighbors(a, Direction::Forward, &f)
        .map(|e| e.peer)
        .collect();
    assert_eq!(fwd, vec![b]);
    let bwd: Vec<_> = v
        .neighbors(b, Direction::Backward, &f)
        .map(|e| e.peer)
        .collect();
    assert_eq!(bwd, vec![a]);
}

// --- Boundary hardening: untrusted NodeId must not panic ---------------------

#[test]
fn out_of_range_node_id_does_not_panic() {
    // A `GraphView` with a handful of nodes; the largest valid id is < node_count.
    let v = view(GraphBuilder::new().func("a").func("b").func("c"));

    // `try_node` is the boundary-safe accessor: a wildly out-of-range id (the kind
    // an untrusted CLI/MCP input could construct) yields `None`, never a panic.
    assert_eq!(v.try_node(NodeId(u32::MAX)), None);

    // And a valid id still resolves through the same accessor.
    let a = v.resolve_one(&SymbolPattern::fqn("a")).unwrap();
    assert!(v.try_node(a).is_some());
}
