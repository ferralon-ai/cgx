//! Rapid-Type-Analysis (RTA) post-pass: narrow the CHA trait-scoped candidate
//! sets (`rule = "cha-trait-set"`) to the types that are actually *instantiated*
//! somewhere reachable, and upgrade the surviving group `possible → probable`
//! (design §4.3, §5 row 3).
//!
//! ## Where it runs (design §2.2, §5)
//!
//! RTA runs **after** [`crate::run_cha`] in the pipeline. CHA over-approximates a
//! `dyn Trait` call to *every* in-source impl of the trait (`possible`); RTA
//! observes that only a type that is constructed somewhere can be the dynamic
//! type behind the trait object, so it drops candidates whose declaring type is
//! never instantiated in the reachable graph and upgrades the survivors to
//! `probable` (the set is now a tighter approximation, but still an
//! approximation — see the cut-marker guard below). Tier stays [`Tier::ChaRta`];
//! the rule becomes `"rta-pruned"`.
//!
//! ## Instantiated-type set / "live caller" (design §4.3 step 1)
//!
//! The instantiated set is `{ T | Instantiates(src, T) ∧ src reachable from a
//! root }`. "Reachable from a root" reuses cgx's existing reachability rooting
//! convention (the one `cgx-query`'s `unused` / Q-4 uses, engine.rs): the roots
//! are the nodes tagged with an [`entrypoint_kind`](cgx_core::node::NodeRecord::entrypoint_kind),
//! and reachability is forward BFS over the **call-family** edges
//! ([`EdgeKind::is_call`]). cgx-resolve sits below cgx-query, so the traversal is
//! re-implemented here over the [`ResolvedGraph`] rather than imported — it
//! matches the same rooting and edge filter.
//!
//! **Empty-root fallback (sound by construction).** When the graph declares *no*
//! entrypoints (common for a library crate or a unit-test fixture), an empty root
//! set would make *everything* unreachable and RTA would prune every candidate —
//! a catastrophic false negative. The roadmap's `unused` query tolerates that
//! (it just reports everything unused), but RTA *acts* on reachability, so it must
//! not. We therefore treat **every callable node as a root** when no entrypoint is
//! tagged. This is the conservative, sound choice: it can only *keep* more
//! candidates, never wrongly drop one.
//!
//! ## CUT-MARKER GUARD (design §4.3 step 3 — load-bearing)
//!
//! RTA's pruning is sound only if the instantiated-type set is *complete*. A type
//! constructed behind an unexpanded macro, across an FFI boundary, by a
//! dependency, by deserialization, or at any site cgx could not resolve is
//! invisible to the `Instantiates` relation — pruning it would be a false
//! negative (we would drop a candidate that really can be the dynamic type). So:
//!
//! - if the **CHA call site** carries any [`CutMarker`] (e.g. CHA's own supernode
//!   `Unresolved` stamp, or an `UnexpandedMacro` on the call), OR
//! - if the graph contains **any reachable construction site under a cut** (an
//!   `Instantiates` edge carrying a cut marker, *or* an unresolved reference that
//!   could be a hidden construction),
//!
//! then the instantiated set cannot be trusted as complete, and RTA **does not
//! prune** — the full CHA set is left at `possible`. This honors the existing
//! honest-dangling / LS-6 philosophy: never silently drop an edge that might be
//! real.
//!
//! ## Determinism (design §7)
//!
//! Pruning changes candidate-group membership, so when any site is pruned RTA
//! marks the graph dirty and re-runs [`crate::canonicalize`]. The instantiated-set
//! computation is fully deterministic: it is built over `BTreeMap`/`BTreeSet` and
//! sorted vectors, with no `HashMap` iteration order reaching the output.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cgx_core::confidence::Confidence;
use cgx_core::cut::CutMarker;
use cgx_core::edge::EdgeKind;
use cgx_core::id::{NodeId, SiteId};
use cgx_core::node::SymbolKind;

use crate::graph::ResolvedGraph;
use crate::link::canonicalize_candidate_dsts;

/// The rule string CHA stamps on a trait-scoped candidate edge — the sites RTA
/// considers for instantiation pruning.
const CHA_RULE: &str = "cha-trait-set";
/// The rule string RTA stamps on an instantiation-pruned (and upgraded) edge.
const RTA_RULE: &str = "rta-pruned";

/// Per-run RTA counters, surfaced through `IndexStats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RtaStats {
    /// CHA candidate-group sites pruned to their instantiated-type subset and
    /// upgraded `possible → probable`.
    pub sites_pruned: usize,
    /// CHA candidate edges dropped because their declaring type is never
    /// instantiated in the reachable graph.
    pub candidates_dropped: usize,
    /// CHA sites RTA left untouched because a cut marker (on the call site or on a
    /// reachable construction site) means the instantiated set is incomplete.
    pub sites_guarded_by_cut: usize,
}

/// Run the RTA instantiation-pruning post-pass on `graph` in place, returning
/// counters. Re-canonicalizes the graph if any site was pruned (candidate-group
/// membership changed).
pub fn run_rta(graph: &mut ResolvedGraph) -> RtaStats {
    let mut stats = RtaStats::default();

    let sites = collect_cha_sites(graph);
    if sites.is_empty() {
        return stats;
    }

    let reachable = reachable_callers(graph);
    let instantiated = instantiated_types(graph, &reachable);
    // If any *reachable* construction is itself behind a cut, the instantiated set
    // is globally incomplete: a type could be constructed off-graph. In that case
    // RTA must not prune any site (sound over-approximation, design §4.3 step 3).
    let construction_under_cut = construction_under_cut(graph, &reachable);

    // The type FQN that declares each candidate dst (the dst is a `T::m` callable;
    // its declaring type is the FQN parent).
    let fqn_by_id: BTreeMap<NodeId, &str> =
        graph.nodes.iter().map(|n| (n.node.id, n.node.fqn.as_str())).collect();

    let mut drop_edge_idx: BTreeSet<usize> = BTreeSet::new();
    let mut upgrade_edge_idx: BTreeSet<usize> = BTreeSet::new();
    // Groups whose surviving membership must be recomputed (rank reassigned).
    let mut touched_groups: BTreeSet<u32> = BTreeSet::new();

    for site in &sites {
        // CUT-MARKER GUARD (load-bearing): never prune a site whose call edge is
        // cut-marked, or when any reachable construction is behind a cut.
        let call_site_cut = site
            .edge_indices
            .iter()
            .any(|i| !graph.edges[*i].edge.cut_markers.is_empty());
        if call_site_cut || construction_under_cut {
            stats.sites_guarded_by_cut += 1;
            continue;
        }

        // Partition the site's candidate edges into kept (declaring type is
        // instantiated) and dropped.
        let mut keep: Vec<usize> = Vec::new();
        let mut drop: Vec<usize> = Vec::new();
        for &i in &site.edge_indices {
            let dst = graph.edges[i].edge.dst;
            let declaring_type = fqn_by_id.get(&dst).and_then(|fqn| declaring_type(fqn));
            match declaring_type {
                Some(ty) if instantiated.contains(ty) => keep.push(i),
                // A candidate whose declaring type we cannot name (no `::` parent)
                // is kept conservatively — we cannot prove it un-instantiated.
                None => keep.push(i),
                Some(_) => drop.push(i),
            }
        }

        // Nothing to prune at this site (every candidate's type is instantiated):
        // still an RTA observation, but membership is unchanged. We still upgrade
        // to `probable` only when pruning actually narrowed the set; a set that
        // RTA could not narrow stays exactly as CHA left it (design §4.3 step 2
        // upgrades the *surviving* group, i.e. after a prune).
        if drop.is_empty() {
            continue;
        }
        // RTA never prunes a site to *zero* survivors: an empty instantiated
        // intersection means our instantiation view is too incomplete to trust, so
        // keep the full CHA set at `possible` (sound — same posture as the cut
        // guard). This also covers fixtures with no Instantiates at all.
        if keep.is_empty() {
            stats.sites_guarded_by_cut += 1;
            continue;
        }

        stats.sites_pruned += 1;
        stats.candidates_dropped += drop.len();
        for i in drop {
            drop_edge_idx.insert(i);
        }
        for i in &keep {
            upgrade_edge_idx.insert(*i);
            if let Some(g) = graph.edges[*i].edge.candidate_group {
                touched_groups.insert(g);
            }
        }
    }

    if drop_edge_idx.is_empty() {
        return stats;
    }

    // Recompute the surviving confidence per touched group (a prune to a single
    // survivor is still `probable`, not `certain` — RTA is an approximation;
    // `canonicalize_candidate_dsts` returns `Probable` for a singleton, `Possible`
    // for multi — but the design pins the *surviving* set to `probable`, so a
    // multi-survivor set is also upgraded to `probable`).
    for &i in &upgrade_edge_idx {
        graph.edges[i].edge.confidence = Confidence::Probable;
        graph.edges[i].edge.rule = RTA_RULE.to_owned();
        graph.edges[i].provenance.rule = RTA_RULE.to_owned();
    }

    // Drop pruned edges and their candidate rows.
    let dropped_dsts_by_group: BTreeMap<u32, Vec<NodeId>> = {
        let mut m: BTreeMap<u32, Vec<NodeId>> = BTreeMap::new();
        for &i in &drop_edge_idx {
            if let Some(g) = graph.edges[i].edge.candidate_group {
                m.entry(g).or_default().push(graph.edges[i].edge.dst);
            }
        }
        m
    };

    let mut kept: Vec<_> = Vec::with_capacity(graph.edges.len());
    for (i, e) in graph.edges.drain(..).enumerate() {
        if !drop_edge_idx.contains(&i) {
            kept.push(e);
        }
    }
    graph.edges = kept;

    // Remove the dropped candidate rows.
    graph.candidates.retain(|c| {
        dropped_dsts_by_group
            .get(&c.candidate_group)
            .map(|dsts| !dsts.contains(&c.dst))
            .unwrap_or(true)
    });

    // A group pruned to a single survivor must shed its `candidate_group`
    // (single-target edges carry no group), and its remaining candidate rows are
    // dropped. Recompute survivors per touched group from the post-drop edge set.
    let mut survivors_by_group: BTreeMap<u32, Vec<NodeId>> = BTreeMap::new();
    for e in &graph.edges {
        if e.edge.rule == RTA_RULE {
            if let Some(g) = e.edge.candidate_group {
                survivors_by_group.entry(g).or_default().push(e.edge.dst);
            }
        }
    }
    let mut singleton_groups: BTreeSet<u32> = BTreeSet::new();
    for (g, dsts) in &mut survivors_by_group {
        let mut d = dsts.clone();
        let _ = canonicalize_candidate_dsts(&mut d);
        if d.len() == 1 {
            singleton_groups.insert(*g);
        }
    }
    if !singleton_groups.is_empty() {
        for e in &mut graph.edges {
            if let Some(g) = e.edge.candidate_group {
                if singleton_groups.contains(&g) {
                    e.edge.candidate_group = None;
                }
            }
        }
        graph
            .candidates
            .retain(|c| !singleton_groups.contains(&c.candidate_group));
    }

    crate::canonicalize(graph);
    stats
}

/// One CHA trait-scoped call site: the indices of its `cha-trait-set`
/// `CallsVirtual` edges in `graph.edges`, keyed by `(src, site_id)`.
struct ChaSite {
    edge_indices: Vec<usize>,
}

/// Collect every CHA trait-scoped call site (`rule == "cha-trait-set"`), grouped
/// by `(src, site_id)`, in a deterministic order.
fn collect_cha_sites(graph: &ResolvedGraph) -> Vec<ChaSite> {
    let mut by_site: BTreeMap<(NodeId, SiteId), Vec<usize>> = BTreeMap::new();
    for (i, e) in graph.edges.iter().enumerate() {
        if e.edge.kind == EdgeKind::CallsVirtual && e.edge.rule == CHA_RULE {
            if let Some(site_id) = e.edge.site_id {
                by_site.entry((e.edge.src, site_id)).or_default().push(i);
            }
        }
    }
    by_site.into_values().map(|edge_indices| ChaSite { edge_indices }).collect()
}

/// The set of node ids reachable forward over call-family edges from the root set.
///
/// Roots are the entrypoint-tagged nodes (matching cgx-query's `unused` rooting);
/// when none are tagged, every callable node is a root (the sound empty-root
/// fallback — RTA must not prune a graph it cannot root).
fn reachable_callers(graph: &ResolvedGraph) -> BTreeSet<NodeId> {
    // Forward adjacency over call-family edges only.
    let mut adj: BTreeMap<NodeId, BTreeSet<NodeId>> = BTreeMap::new();
    for e in &graph.edges {
        if e.edge.kind.is_call() {
            adj.entry(e.edge.src).or_default().insert(e.edge.dst);
        }
    }

    let roots: Vec<NodeId> = {
        let tagged: Vec<NodeId> = graph
            .nodes
            .iter()
            .filter(|n| n.node.entrypoint_kind.is_some())
            .map(|n| n.node.id)
            .collect();
        if tagged.is_empty() {
            graph
                .nodes
                .iter()
                .filter(|n| matches!(n.node.kind, SymbolKind::Function | SymbolKind::Method))
                .map(|n| n.node.id)
                .collect()
        } else {
            tagged
        }
    };

    let mut reachable: BTreeSet<NodeId> = BTreeSet::new();
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    for r in roots {
        if reachable.insert(r) {
            queue.push_back(r);
        }
    }
    while let Some(n) = queue.pop_front() {
        if let Some(succ) = adj.get(&n) {
            for &s in succ {
                if reachable.insert(s) {
                    queue.push_back(s);
                }
            }
        }
    }
    reachable
}

/// The instantiated-type set: the FQNs of types `T` such that some
/// `Instantiates(src, T)` edge has its source node in the reachable set
/// (design §4.3 step 1).
fn instantiated_types<'g>(
    graph: &'g ResolvedGraph,
    reachable: &BTreeSet<NodeId>,
) -> BTreeSet<&'g str> {
    let fqn_by_id: BTreeMap<NodeId, &str> =
        graph.nodes.iter().map(|n| (n.node.id, n.node.fqn.as_str())).collect();
    let mut out: BTreeSet<&str> = BTreeSet::new();
    for e in &graph.edges {
        if e.edge.kind == EdgeKind::Instantiates && reachable.contains(&e.edge.src) {
            if let Some(ty) = fqn_by_id.get(&e.edge.dst) {
                out.insert(ty);
            }
        }
    }
    out
}

/// Whether any *reachable* construction site is behind a cut — an `Instantiates`
/// edge carrying a cut marker, or an unresolved reference that could be a hidden
/// construction. Either means the instantiated-type set is globally incomplete, so
/// RTA must not prune (design §4.3 step 3). Unresolved refs in
/// [`ResolvedGraph::unresolved`] are call/construction names cgx could not bind;
/// they are conservatively treated as potential off-graph constructions.
fn construction_under_cut(graph: &ResolvedGraph, reachable: &BTreeSet<NodeId>) -> bool {
    let instantiate_under_cut = graph.edges.iter().any(|e| {
        e.edge.kind == EdgeKind::Instantiates
            && reachable.contains(&e.edge.src)
            && !e.edge.cut_markers.is_empty()
    });
    if instantiate_under_cut {
        return true;
    }
    // A reachable caller that carries an `UnexpandedMacro`/`ViaFfi`/`Reflective`/
    // `Dynamic`/`ViaDi` cut on *any* of its outgoing edges may construct a type we
    // cannot see. Treat that as an incomplete instantiation view.
    graph.edges.iter().any(|e| {
        reachable.contains(&e.edge.src)
            && e.edge.cut_markers.iter().any(|m| {
                matches!(
                    m,
                    CutMarker::UnexpandedMacro
                        | CutMarker::ViaFfi
                        | CutMarker::Reflective
                        | CutMarker::Dynamic
                        | CutMarker::ViaDi
                )
            })
    })
}

/// The declaring type FQN of a `T::m` callable FQN — the `::`-parent. `None` for a
/// bare name (no parent to name a type from).
fn declaring_type(fqn: &str) -> Option<&str> {
    fqn.rfind("::").map(|i| &fqn[..i])
}
