//! Class-Hierarchy-Analysis (CHA) post-pass: replace the name-scoped virtual
//! dispatch candidate sets the link pass emits (`rule = "name-method"`) with
//! **trait-scoped** sets built from the P3 trait lattice
//! (`Implements`/`Inherits`/`Overrides`), per design §4.2.
//!
//! ## Where it runs (design §2.2, §5)
//!
//! This is a pure graph analysis over a [`ResolvedGraph`] — no external input —
//! so it runs as a normal pipeline post-pass with or without `--scip`. The
//! precedence ladder (§5) puts SCIP first: SCIP collapses a candidate group to a
//! single target (clearing its `candidate_group` and rewriting `rule`), so a site
//! SCIP already settled is no longer `rule = "name-method"` and CHA leaves it
//! alone. CHA therefore processes only the `CallsVirtual` sites still carrying the
//! name-scoped resolution the link pass produced.
//!
//! ## The candidate set (design §4.2)
//!
//! For a virtual call to method `m`, the trait-scoped set is, over every trait
//! `Tr` that declares a method named `m`:
//!
//! ```text
//!   { T::m | Implements(T, Tr) }                            // direct impls
//! ∪ { T::m | Implements(T, Sub) ∧ Inherits*(Sub, Tr) }      // supertrait reach
//! ∪ { Tr::m default body  if T implements Tr but does not override m }   // R4
//! ```
//!
//! The call site carries only the method short name (`x.m()` — the frontend has
//! no static receiver type), so "the trait" is *every* trait declaring `m`; the
//! union is still strictly trait-scoped (only types that actually implement such
//! a trait), far narrower than the name-match baseline (every same-named method,
//! including unrelated inherent methods). For a single-trait site this is exactly
//! the `Implements(_, Tr)` impl set (exit criterion E3).
//!
//! The edge **kind stays `CallsVirtual`** (design §4.2); only the target set
//! behind it changes. Confidence is `Probable` for a one-target set, `Possible`
//! otherwise. Tier is stamped [`Tier::ChaRta`], rule `"cha-trait-set"`.
//!
//! ## Supernode cap (design §4.5, decision R6)
//!
//! A blanket `impl<T> Trait for T` or a trait with hundreds of impls blows the
//! candidate set up. Over [`CHA_SUPERNODE_CAP`] (= 64) the group is **kept** but
//! every edge is stamped with a [`CutMarker::Unresolved`] cut marker so the
//! over-approximation is honest and counted — candidates are never silently
//! dropped (LS-6).
//!
//! ## Determinism (design §7)
//!
//! CHA replaces candidate-group membership, so it always marks the graph dirty and
//! re-runs [`crate::canonicalize`]. New candidate dsts are canonicalized through
//! the shared [`crate::link::canonicalize_candidate_dsts`] helper, so ordering,
//! dedup, and the confidence band match the link pass exactly. New group ids are
//! allocated above the existing maximum in a deterministic site order.

use std::collections::{BTreeMap, BTreeSet};

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::CutMarker;
use cgx_core::edge::{Candidate, EdgeKind, EdgeRecord, EdgeWithProvenance};
use cgx_core::id::{EdgeId, NodeId, SiteId};
use cgx_core::node::SymbolKind;
use cgx_core::provenance::Provenance;

use crate::graph::ResolvedGraph;
use crate::link::canonicalize_candidate_dsts;

/// The deterministic CHA candidate-set fan-out cap (decision R6). A trait-scoped
/// group larger than this (e.g. from a blanket `impl<T> Trait for T`) is kept but
/// cut-marked (`Unresolved`) rather than silently truncated — the
/// over-approximation stays honest and counted.
pub const CHA_SUPERNODE_CAP: usize = 64;

/// The rule string the link pass stamps on a name-scoped virtual-dispatch edge —
/// the sites CHA replaces.
const NAME_METHOD_RULE: &str = "name-method";
/// The rule string CHA stamps on a trait-scoped candidate edge.
const CHA_RULE: &str = "cha-trait-set";

/// Per-run CHA counters, surfaced through `IndexStats` so `cgx doctor` can report
/// what the CHA pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChaStats {
    /// Name-scoped virtual-dispatch sites replaced with a trait-scoped set.
    pub sites_rescoped: usize,
    /// Trait-scoped sites whose candidate set exceeded [`CHA_SUPERNODE_CAP`] and
    /// were kept with a cut marker (honest over-approximation, never dropped).
    pub supernode_sites: usize,
}

/// Run the CHA trait-scoping post-pass on `graph` in place, returning counters.
/// Re-canonicalizes the graph if any site was rescoped (candidate-group
/// membership changed).
pub fn run_cha(graph: &mut ResolvedGraph) -> ChaStats {
    let lattice = Lattice::build(graph);
    let mut stats = ChaStats::default();

    // Group the name-scoped CallsVirtual edges by call site, in a deterministic
    // order (the edge Vec is already canonical when CHA runs). A site is keyed by
    // (src, site_id) — the link pass derives one site_id per call site.
    let sites = collect_name_method_sites(graph);
    if sites.is_empty() {
        return stats;
    }

    // Allocate new group ids above the current maximum, deterministically.
    let mut next_group: u32 = graph
        .edges
        .iter()
        .filter_map(|e| e.edge.candidate_group)
        .max()
        .map(|g| g + 1)
        .unwrap_or(0);

    // Build the replacement edges/candidates for every site before mutating the
    // graph (so we read a stable view). Sites are visited in `collect`'s
    // deterministic order; group ids are assigned in that same order.
    let mut new_candidates: Vec<Candidate> = Vec::new();
    let mut new_edges: Vec<EdgeWithProvenance> = Vec::new();
    let mut drop_edge_idx: BTreeSet<usize> = BTreeSet::new();

    for site in &sites {
        let template = &graph.edges[site.edge_indices[0]];
        let method = match short_name_of(graph, template.edge.dst) {
            Some(m) => m,
            None => continue,
        };
        let mut dsts = lattice.trait_scoped_candidates(&method);
        if dsts.is_empty() {
            // No trait declares this method (a pure inherent-method or
            // duck-typed call) — leave the name-scoped resolution untouched.
            continue;
        }

        let confidence = canonicalize_candidate_dsts(&mut dsts);
        let over_cap = dsts.len() > CHA_SUPERNODE_CAP;
        if over_cap {
            stats.supernode_sites += 1;
        }
        stats.sites_rescoped += 1;

        // The new group: only when the rescoped set has >1 candidate.
        let group = if dsts.len() > 1 {
            let g = next_group;
            next_group += 1;
            Some(g)
        } else {
            None
        };

        for idx in &site.edge_indices {
            drop_edge_idx.insert(*idx);
        }

        let proto = graph.edges[site.edge_indices[0]].clone();
        for (rank, dst) in dsts.iter().enumerate() {
            if let Some(g) = group {
                new_candidates.push(Candidate {
                    candidate_group: g,
                    dst: *dst,
                    rank: rank as u32,
                });
            }
            new_edges.push(build_cha_edge(&proto, *dst, confidence, group, over_cap));
        }
    }

    if drop_edge_idx.is_empty() {
        return stats;
    }

    // Remove the replaced name-method edges and their candidate rows, then append
    // the trait-scoped replacements.
    let replaced_groups: BTreeSet<u32> = drop_edge_idx
        .iter()
        .filter_map(|i| graph.edges[*i].edge.candidate_group)
        .collect();
    let mut kept: Vec<EdgeWithProvenance> = Vec::with_capacity(graph.edges.len());
    for (i, e) in graph.edges.drain(..).enumerate() {
        if !drop_edge_idx.contains(&i) {
            kept.push(e);
        }
    }
    kept.extend(new_edges);
    graph.edges = kept;

    graph
        .candidates
        .retain(|c| !replaced_groups.contains(&c.candidate_group));
    graph.candidates.extend(new_candidates);

    crate::canonicalize(graph);
    stats
}

/// One name-scoped virtual-dispatch call site: the indices of its `CallsVirtual`
/// edges (one per name-method candidate) in `graph.edges`.
struct Site {
    edge_indices: Vec<usize>,
}

/// Collect every name-scoped `CallsVirtual` call site (the edges the link pass
/// emitted with `rule = "name-method"`), grouped by `(src, site_id)`. Returns
/// sites in a deterministic order (sorted by `(src, site_id)`), each site's edge
/// indices in ascending index order.
fn collect_name_method_sites(graph: &ResolvedGraph) -> Vec<Site> {
    let mut by_site: BTreeMap<(NodeId, SiteId), Vec<usize>> = BTreeMap::new();
    for (i, e) in graph.edges.iter().enumerate() {
        if e.edge.kind == EdgeKind::CallsVirtual && e.edge.rule == NAME_METHOD_RULE {
            if let Some(site_id) = e.edge.site_id {
                by_site.entry((e.edge.src, site_id)).or_default().push(i);
            }
        }
    }
    by_site.into_values().map(|edge_indices| Site { edge_indices }).collect()
}

/// The short (last-segment) name of a node's FQN.
fn short_name_of(graph: &ResolvedGraph, id: NodeId) -> Option<String> {
    graph
        .nodes
        .iter()
        .find(|n| n.node.id == id)
        .map(|n| n.node.fqn.rsplit("::").next().unwrap_or(&n.node.fqn).to_owned())
}

/// Build a trait-scoped CHA edge from a name-method edge prototype, keeping the
/// site identity (src, site_id, span, condition, stmt_index) and the kind
/// (`CallsVirtual`), but redirecting the dst and stamping the CHA tier/rule. Over
/// the supernode cap, stamps a `CutMarker::Unresolved` so the over-approximation
/// is honest.
fn build_cha_edge(
    proto: &EdgeWithProvenance,
    dst: NodeId,
    confidence: Confidence,
    group: Option<u32>,
    over_cap: bool,
) -> EdgeWithProvenance {
    let mut cut_markers = proto.edge.cut_markers.clone();
    if over_cap {
        cut_markers.insert(CutMarker::Unresolved);
    }
    let record = EdgeRecord {
        id: EdgeId(0), // reassigned by canonicalize
        src: proto.edge.src,
        dst,
        kind: EdgeKind::CallsVirtual,
        condition: proto.edge.condition,
        confidence,
        tier: Tier::ChaRta,
        rule: CHA_RULE.to_owned(),
        site_id: proto.edge.site_id,
        stmt_index: proto.edge.stmt_index,
        cut_markers,
        implicit: proto.edge.implicit,
        candidate_group: group,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
    };
    let prov = Provenance::new(
        proto.provenance.span.clone(),
        CHA_RULE,
        Tier::ChaRta,
        String::new(),
    );
    EdgeWithProvenance {
        edge: record,
        provenance: prov,
    }
}

/// The trait lattice projected out of a [`ResolvedGraph`]: the relations CHA
/// needs to map a method short name to its trait-scoped candidate set.
struct Lattice {
    /// `trait_fqn → [implementing type_fqn]` (from `Implements` edges).
    implementors: BTreeMap<String, Vec<String>>,
    /// `supertrait_fqn → [subtrait_fqn]`, **transitively** closed over
    /// `Inherits`. A type implementing the subtrait reaches the supertrait's
    /// methods (design §4.2 supertrait reach).
    subtraits: BTreeMap<String, BTreeSet<String>>,
    /// `method short name → [trait_fqn]` for every trait declaring that method
    /// (a trait method node `Tr::m` exists).
    trait_methods_by_name: BTreeMap<String, BTreeSet<String>>,
    /// `fqn → NodeId` for every node (to resolve a computed `T::m` FQN to its id).
    node_by_fqn: BTreeMap<String, NodeId>,
    /// FQNs of callable nodes (method/function) — used to test whether a type
    /// overrides a method (`T::m` is a callable node).
    callable_fqns: BTreeSet<String>,
    /// Whether a trait-method node `Tr::m` carries a body (is not abstract) — a
    /// non-abstract trait method is a default body usable as an R4 fallback.
    has_default_body: BTreeSet<String>,
}

impl Lattice {
    fn build(graph: &ResolvedGraph) -> Lattice {
        let mut implementors: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut inherits: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut trait_fqns: BTreeSet<String> = BTreeSet::new();
        let mut node_by_fqn: BTreeMap<String, NodeId> = BTreeMap::new();
        let mut callable_fqns: BTreeSet<String> = BTreeSet::new();
        let mut type_fqns: BTreeSet<String> = BTreeSet::new();

        for n in &graph.nodes {
            node_by_fqn.insert(n.node.fqn.clone(), n.node.id);
            match n.node.kind {
                SymbolKind::Method | SymbolKind::Function => {
                    callable_fqns.insert(n.node.fqn.clone());
                }
                SymbolKind::Type => {
                    type_fqns.insert(n.node.fqn.clone());
                }
                _ => {}
            }
        }

        let fqn_of = |id: NodeId| -> Option<&String> {
            graph
                .nodes
                .iter()
                .find(|n| n.node.id == id)
                .map(|n| &n.node.fqn)
        };

        for e in &graph.edges {
            match e.edge.kind {
                EdgeKind::Implements => {
                    if let (Some(ty), Some(tr)) = (fqn_of(e.edge.src), fqn_of(e.edge.dst)) {
                        implementors.entry(tr.clone()).or_default().push(ty.clone());
                        trait_fqns.insert(tr.clone());
                    }
                }
                EdgeKind::Inherits => {
                    // src = subtrait, dst = supertrait.
                    if let (Some(sub), Some(sup)) = (fqn_of(e.edge.src), fqn_of(e.edge.dst)) {
                        inherits.entry(sub.clone()).or_default().insert(sup.clone());
                        trait_fqns.insert(sub.clone());
                        trait_fqns.insert(sup.clone());
                    }
                }
                _ => {}
            }
        }
        for v in implementors.values_mut() {
            v.sort();
            v.dedup();
        }

        // Transitively close Inherits into `subtraits[super] = { all subs }`.
        let subtraits = transitive_subtraits(&inherits);

        // Trait methods: a callable node whose FQN parent is a trait FQN.
        let mut trait_methods_by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut has_default_body: BTreeSet<String> = BTreeSet::new();
        for n in &graph.nodes {
            if !matches!(n.node.kind, SymbolKind::Method | SymbolKind::Function) {
                continue;
            }
            let Some((parent, method)) = split_fqn(&n.node.fqn) else {
                continue;
            };
            if trait_fqns.contains(parent) {
                trait_methods_by_name
                    .entry(method.to_owned())
                    .or_default()
                    .insert(parent.to_owned());
                if !n.node.is_abstract {
                    has_default_body.insert(n.node.fqn.clone());
                }
            }
        }

        Lattice {
            implementors,
            subtraits,
            trait_methods_by_name,
            node_by_fqn,
            callable_fqns,
            has_default_body,
        }
    }

    /// The trait-scoped candidate set (as `NodeId`s) for a virtual call to method
    /// `method`, per design §4.2. Empty when no trait declares `method` (a pure
    /// inherent/duck-typed call CHA does not touch).
    fn trait_scoped_candidates(&self, method: &str) -> Vec<NodeId> {
        let Some(traits) = self.trait_methods_by_name.get(method) else {
            return Vec::new();
        };
        let mut out: BTreeSet<NodeId> = BTreeSet::new();
        for tr in traits {
            // Direct implementors of the trait, plus implementors of any subtrait
            // that inherits it (supertrait reach).
            let mut types: BTreeSet<&String> = BTreeSet::new();
            if let Some(direct) = self.implementors.get(tr) {
                types.extend(direct.iter());
            }
            if let Some(subs) = self.subtraits.get(tr) {
                for sub in subs {
                    if let Some(impls) = self.implementors.get(sub) {
                        types.extend(impls.iter());
                    }
                }
            }
            let trait_method_fqn = format!("{tr}::{method}");
            for ty in types {
                // The type's own override `T::m`, else the trait default body
                // `Tr::m` (R4) when one exists.
                let own = format!("{ty}::{method}");
                if self.callable_fqns.contains(&own) {
                    if let Some(id) = self.node_by_fqn.get(&own) {
                        out.insert(*id);
                    }
                } else if self.has_default_body.contains(&trait_method_fqn) {
                    if let Some(id) = self.node_by_fqn.get(&trait_method_fqn) {
                        out.insert(*id);
                    }
                }
            }
        }
        out.into_iter().collect()
    }
}

/// Transitively close an `Inherits` adjacency (`sub → {direct supers}`) into
/// `super → {all subs that reach it, directly or via intermediate supertraits}`.
fn transitive_subtraits(
    inherits: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut subtraits: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for sub in inherits.keys() {
        // Walk all supertraits reachable from `sub`.
        let mut stack: Vec<&String> = inherits.get(sub).into_iter().flatten().collect();
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        while let Some(sup) = stack.pop() {
            if !seen.insert(sup) {
                continue;
            }
            subtraits.entry(sup.clone()).or_default().insert(sub.clone());
            if let Some(ups) = inherits.get(sup) {
                stack.extend(ups.iter());
            }
        }
    }
    subtraits
}

/// Split an FQN into `(parent, last-segment)`; `None` for a bare name.
fn split_fqn(fqn: &str) -> Option<(&str, &str)> {
    fqn.rfind("::").map(|i| (&fqn[..i], &fqn[i + 2..]))
}
