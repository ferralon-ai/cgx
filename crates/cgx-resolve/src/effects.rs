//! GM-12 transitive effect-closure post-pass (P8b, design §2.2 (E), §8).
//!
//! P8a stamped each function node's syntactic [`own_effects`](cgx_core::node::NodeRecord::own_effects)
//! — what the body does *directly*. This pass computes the *transitive* closure:
//!
//! ```text
//!   transitive_effects(n) = own_effects(n) ∪ ( ⋃ transitive_effects(c)
//!                                               for c a callee of n over a
//!                                               CALL-FAMILY edge that is NOT Spawns )
//! ```
//!
//! ## Where it runs (design §2.2 (E), §5 row E)
//!
//! After the confidence passes (SCIP / CHA / RTA / sig) have settled, so the
//! closure rides the *improved* edge precision (RTA-pruned candidate sets, SCIP
//! redirects). It mutates only node `transitive_effects` — never edges, candidate
//! groups, or EdgeIds — so it needs no [`crate::canonicalize`] re-run (design §7
//! row E: EdgeId-stable).
//!
//! ## Propagation edges (GM-12)
//!
//! Effects flow over the call family — [`EdgeKind::Calls`], `CallsVirtual`,
//! `CallsClosure`, `CallsCallback`, `CallsAsync`, `CallsIndirect` — but **NOT over
//! [`EdgeKind::Spawns`]**. Spawned work's effects are reachable *via* the `Spawns`
//! edge but are deliberately not unioned into the spawner's transitive set: the
//! spawner returns without awaiting the detached task (the `spawns` *own*-effect
//! label already marks that it launches work). [`EdgeKind::is_call`] includes
//! `Spawns`, so this pass uses its own [`propagates_effects`] predicate rather than
//! `is_call`.
//!
//! ## Cycles / SCCs (design §8, dispatch step 3)
//!
//! Mutual recursion (`f` ↔ `g`) forms a cycle in the call graph; a naive recursive
//! union would not terminate. The closure is a sound fixpoint: every node in a
//! strongly-connected component shares the same transitive set — the union of all
//! members' own effects plus the transitive effects of every successor outside the
//! SCC. We compute SCCs with an **iterative** Tarjan (no recursion → no stack
//! overflow on deep/wide graphs), which emits components in reverse-topological
//! order. Processing them in that order means every cross-SCC successor's closure
//! is already final when we reach a component, so a single linear pass suffices —
//! no iterate-to-stable loop. petgraph is intentionally not used (it is absent from
//! the dependency tree; zero-new-dep constraint), so SCC detection is hand-rolled
//! here over the [`ResolvedGraph`].
//!
//! ## Determinism (design §7 row E)
//!
//! Adjacency, SCC membership, and successor iteration are all built over sorted
//! [`NodeId`] keys (`BTreeMap`/`BTreeSet`); no `HashMap` iteration order reaches the
//! output. [`EffectSet`] is order-independent by construction (a `u16` bitset), so
//! the union is a pure function of the input set. Re-indexing twice yields a
//! byte-identical store.

use std::collections::{BTreeMap, BTreeSet};

use cgx_core::id::NodeId;
use cgx_core::EffectSet;

use crate::graph::ResolvedGraph;

/// Per-run effect-closure counters, surfaced through `IndexStats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EffectStats {
    /// Nodes whose `transitive_effects` ended up non-empty.
    pub nodes_with_effects: usize,
    /// Strongly-connected components with more than one member (recursion cycles)
    /// observed over the propagating call graph.
    pub recursive_sccs: usize,
}

/// Whether effects propagate across this edge kind. The full call family minus
/// [`EdgeKind::Spawns`] (GM-12): a spawner does not inherit its detached task's
/// effects.
fn propagates_effects(kind: cgx_core::edge::EdgeKind) -> bool {
    use cgx_core::edge::EdgeKind;
    matches!(
        kind,
        EdgeKind::Calls
            | EdgeKind::CallsVirtual
            | EdgeKind::CallsClosure
            | EdgeKind::CallsCallback
            | EdgeKind::CallsAsync
            | EdgeKind::CallsIndirect
    )
}

/// Compute the GM-12 transitive effect closure over `graph` in place, populating
/// every node's `transitive_effects`. Returns counters. Edges, candidate groups,
/// and EdgeIds are untouched, so no re-canonicalization is required.
pub fn run_effect_closure(graph: &mut ResolvedGraph) -> EffectStats {
    // Own effects + forward propagating adjacency, keyed by NodeId (deterministic).
    let mut own: BTreeMap<NodeId, EffectSet> = BTreeMap::new();
    for n in &graph.nodes {
        own.insert(n.node.id, n.node.own_effects);
    }
    let mut succ: BTreeMap<NodeId, BTreeSet<NodeId>> = BTreeMap::new();
    for e in &graph.edges {
        if propagates_effects(e.edge.kind) {
            // Ignore self-loops for adjacency; a direct self-recursive call adds no
            // effect beyond the node's own (handled by the SCC union anyway).
            if e.edge.src != e.edge.dst {
                succ.entry(e.edge.src).or_default().insert(e.edge.dst);
            }
        }
    }

    let sccs = tarjan_sccs(&own, &succ);

    // Map each node to its SCC index (SCCs are in reverse-topological order: a
    // component appears before any component it points into is *false* — Tarjan
    // emits in reverse topo, i.e. successors first; see `tarjan_sccs`).
    let mut scc_of: BTreeMap<NodeId, usize> = BTreeMap::new();
    for (i, comp) in sccs.iter().enumerate() {
        for &n in comp {
            scc_of.insert(n, i);
        }
    }

    let mut stats = EffectStats::default();

    // Each SCC's settled transitive effect set, indexed by SCC index.
    let mut scc_effects: Vec<EffectSet> = vec![EffectSet::new(); sccs.len()];

    // Process components in emission order. Tarjan emits successors before
    // predecessors (reverse topological), so when we reach a component every
    // cross-component successor's `scc_effects` entry is already final.
    for (i, comp) in sccs.iter().enumerate() {
        if comp.len() > 1 {
            stats.recursive_sccs += 1;
        }
        let mut acc = EffectSet::new();
        // Union all members' own effects (the SCC fixpoint: every member sees every
        // other member's effects through the cycle).
        for &n in comp {
            acc.union_with(*own.get(&n).unwrap_or(&EffectSet::new()));
        }
        // Union the settled effects of every successor outside this component.
        for &n in comp {
            if let Some(targets) = succ.get(&n) {
                for &t in targets {
                    let tj = scc_of[&t];
                    if tj != i {
                        acc.union_with(scc_effects[tj]);
                    }
                }
            }
        }
        scc_effects[i] = acc;
    }

    // Write each node's transitive set from its SCC's settled effects.
    for n in &mut graph.nodes {
        let eff = scc_of
            .get(&n.node.id)
            .map(|&i| scc_effects[i])
            .unwrap_or_default();
        n.node.transitive_effects = eff;
        if !eff.is_empty() {
            stats.nodes_with_effects += 1;
        }
    }

    stats
}

/// Iterative Tarjan strongly-connected-components over the node set `own.keys()`
/// with forward adjacency `succ`. Returns the SCCs in **reverse-topological order**
/// (every component is emitted before the components that point *into* it, i.e.
/// successors are emitted first), which is exactly the order the effect closure
/// consumes. Iterative (explicit stack) to avoid recursion-depth limits on deep
/// call chains. Deterministic: roots and successors are visited in ascending
/// `NodeId` order.
fn tarjan_sccs(
    own: &BTreeMap<NodeId, EffectSet>,
    succ: &BTreeMap<NodeId, BTreeSet<NodeId>>,
) -> Vec<Vec<NodeId>> {
    // Stable dense indexing of nodes by ascending NodeId.
    let nodes: Vec<NodeId> = own.keys().copied().collect();
    let index_of: BTreeMap<NodeId, usize> =
        nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    // Adjacency as index lists in ascending NodeId order.
    let adj: Vec<Vec<usize>> = nodes
        .iter()
        .map(|n| {
            succ.get(n)
                .map(|ts| {
                    ts.iter()
                        .filter_map(|t| index_of.get(t).copied())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();

    let n = nodes.len();
    const UNVISITED: usize = usize::MAX;
    let mut idx = vec![UNVISITED; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut tarjan_stack: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    let mut sccs: Vec<Vec<NodeId>> = Vec::new();

    // Explicit DFS frame: the node, and a cursor into its adjacency list.
    struct Frame {
        v: usize,
        child: usize,
    }

    for start in 0..n {
        if idx[start] != UNVISITED {
            continue;
        }
        let mut call_stack: Vec<Frame> = vec![Frame { v: start, child: 0 }];
        idx[start] = next_index;
        low[start] = next_index;
        next_index += 1;
        tarjan_stack.push(start);
        on_stack[start] = true;

        while let Some(frame) = call_stack.last_mut() {
            let v = frame.v;
            if frame.child < adj[v].len() {
                let w = adj[v][frame.child];
                frame.child += 1;
                if idx[w] == UNVISITED {
                    // Descend into w.
                    idx[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    tarjan_stack.push(w);
                    on_stack[w] = true;
                    call_stack.push(Frame { v: w, child: 0 });
                } else if on_stack[w] {
                    low[v] = low[v].min(idx[w]);
                }
            } else {
                // Done with v's children: if v is a root, pop its SCC.
                if low[v] == idx[v] {
                    let mut comp: Vec<NodeId> = Vec::new();
                    loop {
                        let w = tarjan_stack.pop().unwrap();
                        on_stack[w] = false;
                        comp.push(nodes[w]);
                        if w == v {
                            break;
                        }
                    }
                    comp.sort_unstable();
                    sccs.push(comp);
                }
                call_stack.pop();
                // Relax the parent's low-link with v's.
                if let Some(parent) = call_stack.last() {
                    let p = parent.v;
                    low[p] = low[p].min(low[v]);
                }
            }
        }
    }

    sccs
}
