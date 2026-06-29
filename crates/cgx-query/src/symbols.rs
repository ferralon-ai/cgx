//! `cgx symbols` (B-2): rank symbols by reference count, with a per-symbol edge
//! breakdown. The hub / importance lens — *not* a name filter (that is `search`).
//!
//! Like [`search`](crate::search) this is a cheap aggregation over the loaded
//! graph: it walks each node's incident edges once (the precomputed in/out
//! adjacency on [`GraphView`]) and tallies them by family (CALLS vs DERIVES_FROM),
//! by condition (GM-3) and by confidence (GM-5). No graph walk, no new persistence.
//!
//! ## Ranking
//!
//! Default rank is **inbound degree** — how depended-upon a symbol is: its
//! call-family callers plus its DerivesFrom consumers. `total` ranks by `in + out`.
//! Ties break by FQN then `(file, line)` so output is byte-identical across runs.
//!
//! ## Edge orientation (knowledge `cgx-dataflow-edge-orientation.md`)
//!
//! - A **CALLS** edge is stored causally (`caller → callee`), so a node's *in*-edges
//!   are its callers and its *out*-edges are its callees.
//! - A **DerivesFrom** edge is stored anti-causally (`derived → source`). To keep
//!   the user-facing in/out breakdown meaning "consumers vs sources" consistent
//!   with the `flows-to`/`flows-from` semantics, a node's DerivesFrom *consumers*
//!   (values derived FROM it — `flows-to`) are reached by its **in**-edges, and its
//!   DerivesFrom *sources* (what it derives FROM — `flows-from`) by its **out**-edges.
//!
//! So both families agree: `inbound` = "things that depend on this symbol".

use std::collections::BTreeMap;

use cgx_core::{Confidence, EdgeCondition, EdgeKind, SymbolKind};

use crate::view::GraphView;

/// Which edges feed the reference-count ranking and breakdown. Today this is the
/// two families a symbol participates in: the call graph and the data-flow graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeFamily {
    /// Any call-family edge (GM-2.1) — see [`EdgeKind::is_call`].
    Calls,
    /// A `DerivesFrom` data-flow edge (GM-2.2 / docs/04).
    DerivesFrom,
}

impl EdgeFamily {
    /// Classify an [`EdgeKind`] into a ranking family, or `None` for structural
    /// edges that do not participate in the reference-count lens (contains,
    /// imports, overrides, …).
    fn classify(kind: EdgeKind) -> Option<EdgeFamily> {
        if kind.is_call() {
            Some(EdgeFamily::Calls)
        } else if kind == EdgeKind::DerivesFrom {
            Some(EdgeFamily::DerivesFrom)
        } else {
            None
        }
    }

    /// The lowercase token used in human/JSON output (`calls`, `derives_from`).
    pub fn token(self) -> &'static str {
        match self {
            EdgeFamily::Calls => "calls",
            EdgeFamily::DerivesFrom => "derives_from",
        }
    }
}

/// A tally of incident edges, decomposed by family, condition and confidence. The
/// maps are `BTreeMap`s so iteration order is deterministic (the cheap, no-`HashMap`
/// determinism the rest of the engine relies on).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeBreakdown {
    /// Total edges in this direction (the degree).
    pub total: usize,
    /// Count per edge family (`calls` / `derives_from`).
    pub by_family: BTreeMap<&'static str, usize>,
    /// Count per edge condition (`always` / `conditional` / `loop` / `exception` /
    /// `panic`). Only call-family edges carry a meaningful condition; structural
    /// `DerivesFrom` edges are `always`.
    pub by_condition: BTreeMap<&'static str, usize>,
    /// Count per confidence (`possible` / `probable` / `certain`).
    pub by_confidence: BTreeMap<&'static str, usize>,
}

impl EdgeBreakdown {
    fn tally(&mut self, family: EdgeFamily, condition: EdgeCondition, confidence: Confidence) {
        self.total += 1;
        *self.by_family.entry(family.token()).or_insert(0) += 1;
        *self.by_condition.entry(condition_token(condition)).or_insert(0) += 1;
        *self.by_confidence.entry(confidence_token(confidence)).or_insert(0) += 1;
    }
}

/// One ranked symbol with its inbound/outbound edge breakdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRank {
    pub fqn: String,
    pub file: String,
    pub line: u32,
    pub kind: SymbolKind,
    /// Inbound degree across the ranking families (callers + DerivesFrom
    /// consumers) — "how depended-upon".
    pub in_degree: usize,
    /// Outbound degree (callees + DerivesFrom sources).
    pub out_degree: usize,
    /// The decomposition of the inbound edges.
    pub inbound: EdgeBreakdown,
    /// The decomposition of the outbound edges.
    pub outbound: EdgeBreakdown,
}

impl SymbolRank {
    /// The total degree (`in + out`), the `--total` ranking key.
    pub fn total_degree(&self) -> usize {
        self.in_degree + self.out_degree
    }
}

/// How [`rank_symbols`] orders results before the `--limit`/`--top` cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankBy {
    /// Default: inbound degree descending (how depended-upon a symbol is).
    Inbound,
    /// Total degree (`in + out`) descending.
    Total,
}

/// Rank every symbol (optionally narrowed to one `kind`) by reference count, each
/// with its inbound/outbound edge breakdown. A pure aggregation over the loaded
/// graph — every node's incident edges are visited exactly once.
///
/// Ranked descending by the chosen [`RankBy`] key; ties break by `(fqn, file,
/// line)` so the order — and thus the rendered output — is byte-identical across
/// runs (IF-8 determinism).
pub fn rank_symbols(
    view: &GraphView,
    rank_by: RankBy,
    kind_filter: Option<SymbolKind>,
) -> Vec<SymbolRank> {
    let nodes = view.nodes();
    let n = nodes.len();

    // One inbound + one outbound breakdown per node, indexed by NodeId position.
    let mut inbound = vec![EdgeBreakdown::default(); n];
    let mut outbound = vec![EdgeBreakdown::default(); n];

    // Single pass over every edge. Each edge contributes to its destination's
    // inbound tally and its source's outbound tally — with the DerivesFrom
    // orientation flip so "inbound" consistently means "depends on this symbol".
    for edge in view.edges() {
        let Some(family) = EdgeFamily::classify(edge.kind) else {
            continue;
        };
        // For a causal CALLS edge `caller(src) → callee(dst)`, dst gains an inbound
        // (a caller) and src gains an outbound (a callee). For an anti-causal
        // DerivesFrom edge `derived(src) → source(dst)`, the *source* (dst) is the
        // depended-upon value, so it also gains the inbound and the derived value
        // (src) the outbound — the two families share one orientation: the edge's
        // `dst` is always the depended-upon (inbound) end here.
        let (in_node, out_node) = (edge.dst.index(), edge.src.index());
        if in_node < n {
            inbound[in_node].tally(family, edge.condition, edge.confidence);
        }
        if out_node < n {
            outbound[out_node].tally(family, edge.condition, edge.confidence);
        }
    }

    let mut ranks: Vec<SymbolRank> = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| kind_filter.is_none_or(|k| node.kind == k))
        .map(|(i, node)| SymbolRank {
            fqn: node.fqn.clone(),
            file: node.file.clone(),
            line: node.line_start,
            kind: node.kind,
            in_degree: inbound[i].total,
            out_degree: outbound[i].total,
            inbound: std::mem::take(&mut inbound[i]),
            outbound: std::mem::take(&mut outbound[i]),
        })
        .collect();

    ranks.sort_by(|a, b| {
        let key = |r: &SymbolRank| match rank_by {
            RankBy::Inbound => r.in_degree,
            RankBy::Total => r.total_degree(),
        };
        // Descending on the rank key; ascending on (fqn, file, line) for a stable,
        // reproducible tiebreak.
        key(b)
            .cmp(&key(a))
            .then_with(|| a.fqn.cmp(&b.fqn))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    ranks
}

/// The canonical lowercase token for an [`EdgeCondition`], matching the rest of the
/// CLI's condition spelling.
fn condition_token(c: EdgeCondition) -> &'static str {
    match c {
        EdgeCondition::Always => "always",
        EdgeCondition::Conditional => "conditional",
        EdgeCondition::Loop => "loop",
        EdgeCondition::Exception => "exception",
        EdgeCondition::Panic => "panic",
    }
}

/// The canonical lowercase token for a [`Confidence`].
fn confidence_token(c: Confidence) -> &'static str {
    match c {
        Confidence::Possible => "possible",
        Confidence::Probable => "probable",
        Confidence::Certain => "certain",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgx_core::{CutMarkers, EdgeId, NodeId, NodeRecord, Tier, Visibility};

    fn node(id: u32, fqn: &str, kind: SymbolKind) -> NodeRecord {
        NodeRecord {
            id: NodeId(id),
            kind,
            fqn: fqn.to_string(),
            file: "src/lib.rs".to_string(),
            line_start: id,
            line_end: id,
            lang: "rust".to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: Default::default(),
            transitive_effects: Default::default(),
        }
    }

    fn call_edge(
        id: u32,
        src: u32,
        dst: u32,
        condition: EdgeCondition,
        confidence: Confidence,
    ) -> cgx_core::EdgeRecord {
        cgx_core::EdgeRecord {
            id: EdgeId(id),
            src: NodeId(src),
            dst: NodeId(dst),
            kind: EdgeKind::Calls,
            condition,
            confidence,
            tier: Tier::NameSyntactic,
            rule: "test".to_string(),
            site_id: None,
            stmt_index: None,
            cut_markers: CutMarkers::default(),
            implicit: None,
            candidate_group: None,
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
            transform: None,
        }
    }

    fn derives_edge(id: u32, src: u32, dst: u32) -> cgx_core::EdgeRecord {
        cgx_core::EdgeRecord {
            kind: EdgeKind::DerivesFrom,
            ..call_edge(id, src, dst, EdgeCondition::Always, Confidence::Certain)
        }
    }

    /// `hub` (id 1) is called by `a` and `b`; `a` calls `hub` and `leaf`.
    /// Degrees: hub in=2 out=0; a in=0 out=2; b in=0 out=1; leaf in=1 out=0.
    fn call_view() -> GraphView {
        let nodes = vec![
            node(0, "app::a", SymbolKind::Function),
            node(1, "app::hub", SymbolKind::Function),
            node(2, "app::b", SymbolKind::Function),
            node(3, "app::leaf", SymbolKind::Function),
        ];
        let edges = vec![
            call_edge(0, 0, 1, EdgeCondition::Always, Confidence::Certain), // a -> hub
            call_edge(1, 2, 1, EdgeCondition::Conditional, Confidence::Probable), // b -> hub
            call_edge(2, 0, 3, EdgeCondition::Always, Confidence::Certain), // a -> leaf
        ];
        GraphView::new(nodes, edges, vec![])
    }

    #[test]
    fn ranks_by_inbound_degree_descending() {
        let view = call_view();
        let ranks = rank_symbols(&view, RankBy::Inbound, None);
        let order: Vec<(&str, usize)> = ranks
            .iter()
            .map(|r| (r.fqn.as_str(), r.in_degree))
            .collect();
        // hub (2 callers) first; then the two in=1/in=0 ties broken by FQN.
        assert_eq!(order[0], ("app::hub", 2));
    }

    #[test]
    fn inbound_ties_break_by_fqn_deterministically() {
        let view = call_view();
        let a = rank_symbols(&view, RankBy::Inbound, None);
        let b = rank_symbols(&view, RankBy::Inbound, None);
        let fa: Vec<&str> = a.iter().map(|r| r.fqn.as_str()).collect();
        let fb: Vec<&str> = b.iter().map(|r| r.fqn.as_str()).collect();
        assert_eq!(fa, fb, "ranking is byte-identical across runs");
        // Among the in=0 nodes (a, b) the tiebreak is FQN ascending: a before b.
        let pos = |f: &str| fa.iter().position(|x| *x == f).unwrap();
        assert!(pos("app::a") < pos("app::b"));
    }

    #[test]
    fn total_degree_ranking_counts_in_plus_out() {
        let view = call_view();
        let ranks = rank_symbols(&view, RankBy::Total, None);
        // `a` has out=2 in=0 (total 2); `hub` has in=2 out=0 (total 2). Tie → FQN,
        // so `app::a` precedes `app::hub`.
        let order: Vec<&str> = ranks.iter().map(|r| r.fqn.as_str()).collect();
        let pos = |f: &str| order.iter().position(|x| *x == f).unwrap();
        assert!(pos("app::a") < pos("app::hub"));
        assert_eq!(ranks.iter().find(|r| r.fqn == "app::a").unwrap().total_degree(), 2);
    }

    #[test]
    fn breakdown_decomposes_by_condition_and_confidence() {
        let view = call_view();
        let ranks = rank_symbols(&view, RankBy::Inbound, None);
        let hub = ranks.iter().find(|r| r.fqn == "app::hub").unwrap();
        // hub's two inbound CALLS edges: one always/certain, one conditional/probable.
        assert_eq!(hub.inbound.total, 2);
        assert_eq!(hub.inbound.by_condition.get("always"), Some(&1));
        assert_eq!(hub.inbound.by_condition.get("conditional"), Some(&1));
        assert_eq!(hub.inbound.by_confidence.get("certain"), Some(&1));
        assert_eq!(hub.inbound.by_confidence.get("probable"), Some(&1));
        assert_eq!(hub.inbound.by_family.get("calls"), Some(&2));
    }

    #[test]
    fn kind_filter_narrows_the_ranked_set() {
        let mut nodes = vec![
            node(0, "app::a", SymbolKind::Function),
            node(1, "app::T", SymbolKind::Type),
        ];
        nodes[1].line_start = 1;
        let view = GraphView::new(nodes, vec![], vec![]);
        let ranks = rank_symbols(&view, RankBy::Inbound, Some(SymbolKind::Type));
        let fqns: Vec<&str> = ranks.iter().map(|r| r.fqn.as_str()).collect();
        assert_eq!(fqns, vec!["app::T"], "only the type survives the filter");
    }

    #[test]
    fn derives_from_source_is_inbound_not_outbound() {
        // `r#1 -> a#0` is an anti-causal DerivesFrom edge (r derives from a). The
        // SOURCE `a#0` is the depended-upon value, so it must gain an INBOUND edge,
        // and the derived `r#1` an OUTBOUND one (flows-to / flows-from semantics).
        let nodes = vec![
            node(0, "f::a#0", SymbolKind::Variable),
            node(1, "f::r#1", SymbolKind::Variable),
        ];
        let edges = vec![derives_edge(0, 1, 0)]; // r#1 -> a#0
        let view = GraphView::new(nodes, edges, vec![]);
        let ranks = rank_symbols(&view, RankBy::Inbound, None);
        let a = ranks.iter().find(|r| r.fqn == "f::a#0").unwrap();
        let r = ranks.iter().find(|r| r.fqn == "f::r#1").unwrap();
        assert_eq!(a.in_degree, 1, "the source value is depended-upon (inbound)");
        assert_eq!(a.out_degree, 0);
        assert_eq!(r.out_degree, 1, "the derived value points OUT to its source");
        assert_eq!(r.in_degree, 0);
        assert_eq!(a.inbound.by_family.get("derives_from"), Some(&1));
    }

    #[test]
    fn structural_edges_are_ignored_by_the_ranking() {
        let nodes = vec![
            node(0, "app::mod", SymbolKind::Module),
            node(1, "app::mod::f", SymbolKind::Function),
        ];
        // A Contains edge (structural) must not count toward reference rank.
        let mut e = call_edge(0, 0, 1, EdgeCondition::Always, Confidence::Certain);
        e.kind = EdgeKind::Contains;
        let view = GraphView::new(nodes, vec![e], vec![]);
        let ranks = rank_symbols(&view, RankBy::Inbound, None);
        for r in &ranks {
            assert_eq!(r.in_degree, 0, "{} should have no ranking edges", r.fqn);
            assert_eq!(r.out_degree, 0);
        }
    }
}
