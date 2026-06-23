//! Signature-keyed candidate sets for indirect closure / fn-pointer calls (P6,
//! design §4.4, decision R3).
//!
//! ## What it resolves
//!
//! An *indirect* call goes through a function **value** rather than a named
//! callable: invoking a closure bound in scope (`let f = |x| ..; f(x)`) or a
//! function passed as an argument / stored in a field (a `fn(i32) -> i32`
//! callback). The value that flows to the call site is not named at the call, so
//! name resolution cannot bind it to a single target — the link pass therefore
//! emits a single **placeholder** edge for each such site (`rule = "indirect:<arity>"`,
//! a self-edge sentinel; see [`crate::link`]). This pass replaces that placeholder
//! with the signature-compatible candidate set.
//!
//! The handled edge kinds are [`EdgeKind::CallsClosure`], [`EdgeKind::CallsCallback`],
//! and [`EdgeKind::CallsIndirect`] — the call-family kinds the frontend maps a
//! closure/callback/fn-pointer invocation to.
//!
//! ## The candidate set (design §4.4)
//!
//! ```text
//!   candidates(indirect call of arity N) =
//!       { L | L is a Lambda  ∧ sig_compatible(L, N) }
//!     ∪ { F | F is a free Function ∧ sig_compatible(F, N) }
//! ```
//!
//! A *free* function is one whose FQN parent is not a known type (i.e. not a
//! method) — only free functions and closures can be coerced to a function value
//! and flow to an indirect call site. Methods are excluded.
//!
//! ## Compatibility definition (`sig_compatible`)
//!
//! **Arity match only**, against what the frontend actually stores. The call site
//! carries a syntactic arity (the argument count, encoded in the `indirect:<arity>`
//! rule); a candidate is compatible when its [`Signature::params`] count equals
//! that arity. Per-parameter and return types are *not* compared: the frontend
//! records them best-effort and a closure literal frequently elides them
//! (`|x| ..`), so a type comparison would spuriously exclude real targets. We
//! intentionally use only what is reliably captured (arity) rather than
//! over-engineer a type match the data cannot support — when the call site's arity
//! is unknown (`indirect:?`), every Lambda/free-Function is admitted (the soundest
//! over-approximation).
//!
//! ## Confidence / tier (design §4.4)
//!
//! Always [`Confidence::Possible`] for a multi-candidate set, [`Tier::ChaRta`],
//! `rule = "sig-compat"`. **This is a signature-compatible over-approximation only.**
//! It does NOT attempt value-flow analysis — which closure value actually flows to
//! which call site — that is DF-18 and explicitly Phase 3, out of scope here.
//!
//! ## Supernode cap (design §4.5, decision R6)
//!
//! A common arity (e.g. a no-arg or one-arg closure) can match many functions.
//! Over [`crate::CHA_SUPERNODE_CAP`] (= 64) the group is **kept in full** but every
//! candidate edge is stamped with a [`CutMarker::Unresolved`] so the
//! over-approximation is honest and counted — candidates are never silently
//! dropped (LS-6).
//!
//! ## Determinism (design §7)
//!
//! The pass replaces candidate-group membership, so it always re-runs
//! [`crate::canonicalize`] when it changed anything. Candidate dsts are
//! canonicalized through the shared [`crate::link::canonicalize_candidate_dsts`]
//! helper (sort by node id, dedup); new group ids are allocated above the existing
//! maximum in a deterministic `(src, site_id)` site order. No `HashMap` iteration
//! order reaches the output.

use std::collections::{BTreeMap, BTreeSet};

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::CutMarker;
use cgx_core::edge::{Candidate, EdgeKind, EdgeRecord, EdgeWithProvenance};
use cgx_core::id::{EdgeId, NodeId, SiteId};
use cgx_core::node::SymbolKind;
use cgx_core::provenance::Provenance;

use crate::cha::CHA_SUPERNODE_CAP;
use crate::graph::ResolvedGraph;
use crate::link::canonicalize_candidate_dsts;

/// The rule prefix the link pass stamps on an indirect-call placeholder edge.
const INDIRECT_RULE_PREFIX: &str = "indirect";
/// The rule string this pass stamps on a signature-compatible candidate edge.
const SIG_RULE: &str = "sig-compat";

/// Per-run signature-resolution counters, surfaced through `IndexStats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SigStats {
    /// Indirect-call placeholder sites replaced with a signature-compatible set.
    pub sites_resolved: usize,
    /// Indirect-call sites for which no signature-compatible Lambda/Function
    /// existed — the placeholder is dropped (honest dangling; no target invented).
    pub sites_unmatched: usize,
    /// Sites whose candidate set exceeded [`CHA_SUPERNODE_CAP`] and were kept with
    /// a cut marker (honest over-approximation, never dropped).
    pub supernode_sites: usize,
}

/// Run the signature-keyed indirect-call resolution post-pass on `graph` in place,
/// returning counters. Re-canonicalizes the graph if any placeholder was replaced
/// (candidate-group membership changed).
pub fn run_sig(graph: &mut ResolvedGraph) -> SigStats {
    let mut stats = SigStats::default();

    let sites = collect_indirect_sites(graph);
    if sites.is_empty() {
        return stats;
    }

    let index = CallableIndex::build(graph);

    let mut next_group: u32 = graph
        .edges
        .iter()
        .filter_map(|e| e.edge.candidate_group)
        .max()
        .map(|g| g + 1)
        .unwrap_or(0);

    let mut new_candidates: Vec<Candidate> = Vec::new();
    let mut new_edges: Vec<EdgeWithProvenance> = Vec::new();
    let mut drop_edge_idx: BTreeSet<usize> = BTreeSet::new();

    for site in &sites {
        // Always drop the placeholder edge — it is a sentinel, never stored as-is.
        drop_edge_idx.insert(site.edge_index);

        let mut dsts = index.compatible(site.arity);
        if dsts.is_empty() {
            // No signature-compatible function value exists. Leave the call
            // honestly dangling (no edge) rather than inventing a target.
            stats.sites_unmatched += 1;
            continue;
        }

        let confidence = canonicalize_candidate_dsts(&mut dsts);
        // P6 ships the `possible` signature set only; a single compatible target is
        // NOT promoted to a resolved `certain`/`probable` call — value-flow (DF-18,
        // Phase 3) is what would justify that. Pin the band to `Possible`.
        let confidence = match confidence {
            Confidence::Probable | Confidence::Certain => Confidence::Possible,
            c => c,
        };
        let over_cap = dsts.len() > CHA_SUPERNODE_CAP;
        if over_cap {
            stats.supernode_sites += 1;
        }
        stats.sites_resolved += 1;

        // A group is allocated only for a multi-candidate set; a single-candidate
        // `possible` set carries no group (matches the link/CHA convention).
        let group = if dsts.len() > 1 {
            let g = next_group;
            next_group += 1;
            Some(g)
        } else {
            None
        };

        let proto = graph.edges[site.edge_index].clone();
        for (rank, dst) in dsts.iter().enumerate() {
            if let Some(g) = group {
                new_candidates.push(Candidate {
                    candidate_group: g,
                    dst: *dst,
                    rank: rank as u32,
                });
            }
            new_edges.push(build_sig_edge(&proto, *dst, confidence, group, over_cap));
        }
    }

    if new_edges.is_empty() && drop_edge_idx.is_empty() {
        return stats;
    }

    let mut kept: Vec<EdgeWithProvenance> = Vec::with_capacity(graph.edges.len());
    for (i, e) in graph.edges.drain(..).enumerate() {
        if !drop_edge_idx.contains(&i) {
            kept.push(e);
        }
    }
    kept.extend(new_edges);
    graph.edges = kept;
    graph.candidates.extend(new_candidates);

    crate::canonicalize(graph);
    stats
}

/// One indirect-call placeholder site: its single edge index and the syntactic
/// call-site arity (`None` when the source did not pin it — `indirect:?`).
struct IndirectSite {
    edge_index: usize,
    arity: Option<usize>,
}

/// Collect every indirect-call placeholder edge (`rule` starting `"indirect"`,
/// one of the indirect call-family kinds), in deterministic `(src, site_id)`
/// order. Each placeholder is one edge (the link pass emits exactly one).
fn collect_indirect_sites(graph: &ResolvedGraph) -> Vec<IndirectSite> {
    let mut by_site: BTreeMap<(NodeId, SiteId), IndirectSite> = BTreeMap::new();
    for (i, e) in graph.edges.iter().enumerate() {
        let is_indirect = matches!(
            e.edge.kind,
            EdgeKind::CallsClosure | EdgeKind::CallsCallback | EdgeKind::CallsIndirect
        );
        if !is_indirect || !e.edge.rule.starts_with(INDIRECT_RULE_PREFIX) {
            continue;
        }
        let Some(site_id) = e.edge.site_id else {
            continue;
        };
        by_site.entry((e.edge.src, site_id)).or_insert(IndirectSite {
            edge_index: i,
            arity: parse_arity(&e.edge.rule),
        });
    }
    by_site.into_values().collect()
}

/// Parse the arity out of an `indirect:<arity>` rule string. `indirect:?` (and any
/// unparseable tail) yields `None` (arity unknown → admit every candidate).
fn parse_arity(rule: &str) -> Option<usize> {
    rule.strip_prefix("indirect:")?.parse::<usize>().ok()
}

/// Build the signature-compatible candidate edge from the placeholder prototype,
/// keeping the call-site identity (src, site_id, span, condition, stmt_index) and
/// the kind, redirecting the dst and stamping the P6 tier/rule. Over the supernode
/// cap, stamps a `CutMarker::Unresolved`.
fn build_sig_edge(
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
        kind: proto.edge.kind,
        condition: proto.edge.condition,
        confidence,
        tier: Tier::ChaRta,
        rule: SIG_RULE.to_owned(),
        site_id: proto.edge.site_id,
        stmt_index: proto.edge.stmt_index,
        cut_markers,
        implicit: proto.edge.implicit,
        candidate_group: group,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
        transform: None,
    };
    let prov = Provenance::new(
        proto.provenance.span.clone(),
        SIG_RULE,
        Tier::ChaRta,
        String::new(),
    );
    EdgeWithProvenance {
        edge: record,
        provenance: prov,
    }
}

/// The function-value candidate index: every `Lambda` and *free* `Function` node,
/// keyed by signature arity, plus an arity-less bucket of all of them.
struct CallableIndex {
    /// `arity → [node ids]` for Lambda + free Function nodes with that param count.
    by_arity: BTreeMap<usize, Vec<NodeId>>,
    /// All Lambda + free Function node ids (used when the call-site arity is
    /// unknown — admit every function value, the soundest over-approximation).
    all: Vec<NodeId>,
}

impl CallableIndex {
    fn build(graph: &ResolvedGraph) -> CallableIndex {
        // A free function's FQN parent is not a type. Collect type FQNs first so we
        // can exclude methods (a callable whose parent is a known type).
        let type_fqns: BTreeSet<&str> = graph
            .node_records()
            .filter(|n| n.kind == SymbolKind::Type)
            .map(|n| n.fqn.as_str())
            .collect();

        let mut by_arity: BTreeMap<usize, Vec<NodeId>> = BTreeMap::new();
        let mut all: Vec<NodeId> = Vec::new();
        for n in graph.node_records() {
            let is_value = match n.kind {
                SymbolKind::Lambda => true,
                // Free function: a Function whose FQN parent is not a known type
                // (i.e. it is not a method/associated fn) can flow as a fn value.
                SymbolKind::Function => parent_fqn(&n.fqn)
                    .map(|p| !type_fqns.contains(p))
                    .unwrap_or(true),
                _ => false,
            };
            if !is_value {
                continue;
            }
            let arity = n.signature.as_ref().map(|s| s.params.len());
            if let Some(a) = arity {
                by_arity.entry(a).or_default().push(n.id);
            }
            all.push(n.id);
        }
        for v in by_arity.values_mut() {
            v.sort_by_key(|id| id.0);
            v.dedup();
        }
        all.sort_by_key(|id| id.0);
        all.dedup();
        CallableIndex { by_arity, all }
    }

    /// The signature-compatible candidate dsts for a call site of the given arity.
    /// Compatibility = arity match. An unknown call-site arity (`None`) admits
    /// every function value (sound over-approximation). A candidate whose signature
    /// has no recorded params count is only admitted under the unknown-arity case.
    fn compatible(&self, arity: Option<usize>) -> Vec<NodeId> {
        match arity {
            Some(a) => self.by_arity.get(&a).cloned().unwrap_or_default(),
            None => self.all.clone(),
        }
    }
}

/// The `::`-parent of an FQN; `None` for a bare name.
fn parent_fqn(fqn: &str) -> Option<&str> {
    fqn.rfind("::").map(|i| &fqn[..i])
}
