//! Shared deterministic graph algorithms (v0.3 SC4).
//!
//! Extracted from the GM-12 effect-closure pass (`effects.rs`), which had an
//! inline `NodeId`-keyed Tarjan. The IFDS interprocedural-summary pass needs the
//! same SCC machinery over the *call graph* keyed by `String` fn_fqns, so the
//! algorithm is lifted here as a generic, ordered, iterative implementation and
//! both callers share it.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;

/// Iterative Tarjan strongly-connected-components over the node set `nodes` with
/// forward adjacency `succ`. Returns the SCCs in **reverse-topological order**
/// (every component is emitted before the components that point *into* it — i.e.
/// successors are emitted first), which is exactly the order both the effect
/// closure and the IFDS worklist consume (process callees before callers).
///
/// Iterative (explicit stack) to avoid recursion-depth limits on deep call
/// chains. Deterministic: `nodes` is taken in its given order (callers pass a
/// `BTreeSet`/sorted vec), and successors are visited in ascending node order.
/// Each emitted component is sorted, so the result is a pure function of the
/// graph regardless of insertion order.
///
/// `N` is any cloneable, ordered key (`NodeId`, `String`, …). `nodes` must
/// contain every node that appears as a key *or* a successor; callers that build
/// adjacency from edges should seed `nodes` with both endpoints.
pub fn tarjan_sccs<N>(nodes: &BTreeSet<N>, succ: &BTreeMap<N, BTreeSet<N>>) -> Vec<Vec<N>>
where
    N: Clone + Ord + Eq + Hash,
{
    // Stable dense indexing of nodes in ascending order.
    let node_vec: Vec<N> = nodes.iter().cloned().collect();
    let index_of: BTreeMap<&N, usize> = node_vec.iter().enumerate().map(|(i, n)| (n, i)).collect();
    // Adjacency as index lists in ascending node order (BTreeSet iterates sorted).
    let adj: Vec<Vec<usize>> = node_vec
        .iter()
        .map(|n| {
            succ.get(n)
                .map(|ts| ts.iter().filter_map(|t| index_of.get(t).copied()).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();

    let n = node_vec.len();
    const UNVISITED: usize = usize::MAX;
    let mut idx = vec![UNVISITED; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut tarjan_stack: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    let mut sccs: Vec<Vec<N>> = Vec::new();

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
                    let mut comp: Vec<usize> = Vec::new();
                    loop {
                        let w = tarjan_stack.pop().unwrap();
                        on_stack[w] = false;
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    let mut named: Vec<N> = comp.into_iter().map(|i| node_vec[i].clone()).collect();
                    named.sort();
                    sccs.push(named);
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
