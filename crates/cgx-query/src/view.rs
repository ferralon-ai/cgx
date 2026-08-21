//! [`GraphView`] — the in-memory adjacency over a loaded linked graph that every
//! Layer-1 subcommand walks (architecture §4).
//!
//! A `GraphView` owns a `(nodes, edges, candidates)` triple (as produced by the
//! resolver and round-tripped through `cgx_store::FactStore::read_graph`) and
//! precomputes the forward/reverse adjacency lists the traversal engines need.
//! Node identity is the dense [`NodeId`] = position in the canonical node order
//! (architecture §3): `nodes[id.index()]` is the record for `id`, so node lookup
//! is an array index, not a map probe — which also keeps iteration order
//! deterministic without any hashing.
//!
//! ## Layer-2 forward-compatibility
//!
//! `resolve_symbol` + [`neighbors`](GraphView::neighbors) + [`node`](GraphView::node)
//! are exactly the primitives a future Cypher planner (`cgx-cql`) would compile
//! `MATCH` patterns down to. Layer 1 is implemented as hand-written walks over
//! these primitives today; Layer 2, when scheduled, becomes a frontend that lowers
//! to the same surface rather than a second execution path.

use std::collections::BTreeMap;

use cgx_core::{
    Candidate, EdgeId, EdgeRecord, NodeId, NodeRecord, PatternKind, SymbolKind, SymbolPattern,
};

use cgx_select::{MatchOptions, Selector};

use crate::filter::{Direction, EdgeFilter};

/// A linked graph prepared for traversal.
///
/// Holds the node/edge/candidate records plus the precomputed adjacency. All
/// public accessors are pure and deterministic; nothing here allocates per call
/// except the borrowed iterators returned by [`GraphView::neighbors`].
#[derive(Debug, Clone)]
pub struct GraphView {
    nodes: Vec<NodeRecord>,
    edges: Vec<EdgeRecord>,
    candidates: Vec<Candidate>,
    /// `out_edges[n]` = edge indices whose `src == n`, in canonical edge order.
    out_edges: Vec<Vec<u32>>,
    /// `in_edges[n]` = edge indices whose `dst == n`, in canonical edge order.
    in_edges: Vec<Vec<u32>>,
    /// Size of each over-approximated candidate group (`candidate_group id → member
    /// count`), precomputed once from the candidate table. A group appears only
    /// when it has ≥ 2 members (a singleton sheds its group at resolve time), so an
    /// edge with no group id is fan-out 1. Read by [`candidate_group_size`] to
    /// enforce the `--max-candidates` fan-out cap without re-resolving.
    group_sizes: BTreeMap<u32, u32>,
}

/// A borrowed reference to one edge plus the convenience of its already-resolved
/// endpoint id for the walk direction in play.
///
/// `peer` is the node on the *other* end of the edge relative to the direction
/// the walk is moving: for a forward (callees) walk it is `edge.dst`; for a
/// reverse (callers) walk it is `edge.src`. This lets the traversal engines treat
/// both directions uniformly.
#[derive(Debug, Clone, Copy)]
pub struct EdgeRef<'a> {
    pub edge: &'a EdgeRecord,
    /// The node reached by traversing this edge in the current direction.
    pub peer: NodeId,
}

impl GraphView {
    /// Build a view from a loaded linked graph triple.
    ///
    /// The triple is expected in canonical order (the order
    /// `cgx_store::FactStore::read_graph` returns and the resolver produces);
    /// `node_id.index()` must equal the node's position. The constructor does not
    /// re-sort — it preserves determinism by trusting the canonical input — but it
    /// is robust to node ids that are not a dense `0..n` prefix: any edge endpoint
    /// outside the node array is simply dropped from the adjacency.
    pub fn new(nodes: Vec<NodeRecord>, edges: Vec<EdgeRecord>, candidates: Vec<Candidate>) -> Self {
        let n = nodes.len();
        let mut out_edges: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut in_edges: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (idx, e) in edges.iter().enumerate() {
            let idx = idx as u32;
            let s = e.src.index();
            let d = e.dst.index();
            if s < n {
                out_edges[s].push(idx);
            }
            if d < n {
                in_edges[d].push(idx);
            }
        }
        let mut group_sizes: BTreeMap<u32, u32> = BTreeMap::new();
        for c in &candidates {
            *group_sizes.entry(c.candidate_group).or_default() += 1;
        }
        GraphView {
            nodes,
            edges,
            candidates,
            out_edges,
            in_edges,
            group_sizes,
        }
    }

    /// Number of symbol nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// All node records, in canonical order.
    pub fn nodes(&self) -> &[NodeRecord] {
        &self.nodes
    }

    /// All edge records, in canonical order.
    pub fn edges(&self) -> &[EdgeRecord] {
        &self.edges
    }

    /// All candidate-set members.
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// The fan-out of an over-approximated candidate group — the number of parallel
    /// target edges that share `group`. Read from the persisted candidate table (no
    /// re-resolve). A group that is not present has no recorded members; callers
    /// treat a groupless edge as fan-out 1.
    pub fn candidate_group_size(&self, group: u32) -> u32 {
        self.group_sizes.get(&group).copied().unwrap_or(0)
    }

    /// The record for a node id.
    ///
    /// # Invariant
    ///
    /// The caller guarantees `n` is in range. This holds for ids produced by this
    /// view's own traversal (`neighbors`/`PathWalker`/`bfs`) or by `resolve_symbol`,
    /// which only ever yield positions into `self.nodes`. Use [`try_node`](Self::try_node)
    /// at any boundary where the id originates from untrusted input (a CLI argument
    /// or MCP tool parameter); this panics on an out-of-range id.
    pub fn node(&self, n: NodeId) -> &NodeRecord {
        &self.nodes[n.index()]
    }

    /// The record for a node id, or `None` if out of range. The non-panicking form
    /// of [`node`](Self::node) for untrusted-input boundaries.
    pub fn try_node(&self, n: NodeId) -> Option<&NodeRecord> {
        self.nodes.get(n.index())
    }

    /// The edge record for an [`EdgeId`].
    pub fn edge(&self, e: EdgeId) -> Option<&EdgeRecord> {
        self.edges.get(e.index())
    }

    /// Resolve a [`SymbolPattern`] to the set of matching node ids, in ascending
    /// id order (i.e. canonical `(file, line_start, fqn)` order). Call-site nodes
    /// never appear in pattern results (ADR-01); this view holds only symbol nodes
    /// today, so every node is eligible.
    pub fn resolve_symbol(&self, pat: &SymbolPattern) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|node| pat.matches(node))
            .map(|node| node.id)
            .collect()
    }

    /// Resolve a compiled [`cgx_select::Selector`] against the node table via the
    /// same regime-(b) linear scan as [`resolve_symbol`](Self::resolve_symbol),
    /// preserving ascending id order — but threading the engine's per-node honesty
    /// signals back out rather than discarding them.
    ///
    /// The matcher core is identical to `resolve_symbol`'s (a per-node predicate
    /// over the loaded view); only the feeder differs. Parse-time diagnostics (e.g.
    /// the `!*`→`*!` rewrite) stay on the [`Selector`](cgx_select::Selector) the
    /// caller already holds; the two *per-node* disclosures — truncation
    /// (empty ≠ absent) and agnostic cross-language provenance — are aggregated here
    /// into [`SelectResolution`] so the caller can surface them.
    pub fn resolve_select(&self, selector: &Selector, opts: &MatchOptions) -> SelectResolution {
        let mut res = SelectResolution::default();
        for node in &self.nodes {
            let m = selector.evaluate(node, opts);
            // Truncation is a property of the scan, surfaced even for non-matches
            // (the cap tripped, so *this* node's answer may be incomplete).
            if m.truncated {
                res.truncated = true;
            }
            if m.matched {
                res.ids.push(node.id);
                if m.agnostic_cross_language {
                    res.agnostic_cross_language.push(node.id);
                }
            }
        }
        res
    }

    /// Resolve a single symbol, returning an error string when the pattern matches
    /// zero or (for non-glob patterns) more than one node. Glob/short-name patterns
    /// that match many are allowed by [`resolve_symbol`](Self::resolve_symbol);
    /// this helper is the strict form the `paths --from/--to` endpoints use when a
    /// unique anchor is required.
    pub fn resolve_one(&self, pat: &SymbolPattern) -> Result<NodeId, ResolveError> {
        let mut matches = self.resolve_symbol(pat);
        match matches.len() {
            0 => Err(ResolveError::NotFound {
                pattern: pat.text.clone(),
            }),
            1 => Ok(matches.remove(0)),
            _ => {
                // FQN patterns must be unique; glob/short-name may legitimately
                // fan out, so the caller picks the first in canonical order.
                if pat.kind == PatternKind::Fqn {
                    Err(ResolveError::Ambiguous {
                        pattern: pat.text.clone(),
                        count: matches.len(),
                    })
                } else {
                    Ok(matches[0])
                }
            }
        }
    }

    /// Node ids whose symbol is one of the given kinds, in ascending id order.
    pub fn nodes_of_kind<'a>(
        &'a self,
        kinds: &'a [SymbolKind],
    ) -> impl Iterator<Item = NodeId> + 'a {
        self.nodes
            .iter()
            .filter(move |node| kinds.contains(&node.kind))
            .map(|node| node.id)
    }

    /// The neighbors of `n` in `dir` whose edge passes `filter`, as [`EdgeRef`]s in
    /// canonical edge order. This is the single adjacency primitive every Layer-1
    /// walk (and any future Layer-2 plan) is expressed in terms of.
    pub fn neighbors<'a>(
        &'a self,
        n: NodeId,
        dir: Direction,
        filter: &'a EdgeFilter,
    ) -> impl Iterator<Item = EdgeRef<'a>> + 'a {
        let idx = n.index();
        let bucket: &'a [u32] = match dir {
            Direction::Forward => self.out_edges.get(idx).map_or(&[], Vec::as_slice),
            Direction::Backward => self.in_edges.get(idx).map_or(&[], Vec::as_slice),
        };
        bucket.iter().filter_map(move |&ei| {
            let edge = &self.edges[ei as usize];
            if !filter.admits(edge) {
                return None;
            }
            // The `--max-candidates` fan-out cap: a candidate-group property that
            // `EdgeFilter::admits` cannot decide from the edge alone, applied here
            // because this view owns the candidate table. An edge with no group is
            // a single resolved target (fan-out 1) and is never dropped.
            if let (Some(max), Some(group)) = (filter.max_candidates, edge.candidate_group) {
                if self.candidate_group_size(group) > max {
                    return None;
                }
            }
            let peer = match dir {
                Direction::Forward => edge.dst,
                Direction::Backward => edge.src,
            };
            Some(EdgeRef { edge, peer })
        })
    }
}

/// The outcome of a [`GraphView::resolve_select`] scan: the matched node ids in
/// ascending (canonical) id order, plus the two per-node honesty signals the
/// selector engine raises, aggregated across the scan.
///
/// A `SelectResolution` never silently swallows an incomplete answer: `truncated`
/// records that the active-state cap tripped somewhere in the scan (so a node's
/// answer may be incomplete — empty ≠ absent), and `agnostic_cross_language` lists
/// exactly the nodes that matched *only* because the agnostic flag dropped the
/// family gate (a cross-language provenance surprise the caller must disclose).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectResolution {
    /// Matched node ids, ascending id order (canonical `(file, line_start, fqn)`).
    pub ids: Vec<NodeId>,
    /// The active-state cap tripped on at least one node during the scan.
    pub truncated: bool,
    /// Nodes selected only via the dropped family gate under the agnostic flag.
    pub agnostic_cross_language: Vec<NodeId>,
}

/// Failure modes of [`GraphView::resolve_one`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No symbol matched the pattern.
    NotFound { pattern: String },
    /// An exact-FQN pattern matched more than one symbol.
    Ambiguous { pattern: String, count: usize },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound { pattern } => {
                write!(f, "no symbol matched pattern `{pattern}`")
            }
            ResolveError::Ambiguous { pattern, count } => {
                write!(
                    f,
                    "pattern `{pattern}` matched {count} symbols; expected one"
                )
            }
        }
    }
}

impl std::error::Error for ResolveError {}

#[cfg(test)]
mod select_tests {
    use super::*;
    use cgx_core::{NodeId, NodeRecord, SymbolKind, Visibility};

    fn node(id: u32, fqn: &str, lang: &str) -> NodeRecord {
        NodeRecord {
            id: NodeId(id),
            kind: SymbolKind::Type,
            fqn: fqn.to_string(),
            file: format!("f{id}.rs"),
            line_start: id,
            line_end: id,
            lang: lang.to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: Default::default(),
            transitive_effects: Default::default(),
            unresolved_calls: 0,
        }
    }

    fn view() -> GraphView {
        // Ids are dense positions; kept in ascending id order deliberately so the
        // scan-order assertion is meaningful.
        let nodes = vec![
            node(0, "com::foo::UserService", "rust"),
            node(1, "com::foo::MockService", "rust"),
            node(2, "com::foo::bar::OrderService", "rust"),
            node(3, "com::foo::Helper", "rust"),
            node(4, "com::foo::PaymentService", "go"),
        ];
        GraphView::new(nodes, vec![], vec![])
    }

    #[test]
    fn resolve_select_scans_in_ascending_id_order() {
        let v = view();
        // `**` globstar then any segment ending in `Service`.
        let sel = cgx_select::compile("com::foo::**::*Service").expect("compiles");
        let res = v.resolve_select(&sel, &MatchOptions::native());
        // Native (rust) matches only: UserService(0), MockService(1),
        // OrderService(2). PaymentService(4) is Go, gated out under native.
        assert_eq!(res.ids, vec![NodeId(0), NodeId(1), NodeId(2)]);
        assert!(!res.truncated);
        assert!(res.agnostic_cross_language.is_empty());
    }

    #[test]
    fn resolve_select_agnostic_flags_cross_language() {
        let v = view();
        let sel = cgx_select::compile("com::foo::**::*Service").expect("compiles");
        let res = v.resolve_select(&sel, &MatchOptions::agnostic());
        // Agnostic drops the family gate: the Go PaymentService(4) now matches too.
        assert_eq!(res.ids, vec![NodeId(0), NodeId(1), NodeId(2), NodeId(4)]);
        // Only the Go node matched *purely* via the dropped gate; the rust nodes
        // would have matched natively, so they are not cross-language surprises.
        assert_eq!(res.agnostic_cross_language, vec![NodeId(4)]);
    }

    #[test]
    fn resolve_select_negation_excludes() {
        let v = view();
        // Segments ending in `Service` but not starting with `Mock`.
        let sel = cgx_select::compile("com::foo::!Mock*Service").expect("compiles");
        let res = v.resolve_select(&sel, &MatchOptions::native());
        // UserService(0) matches; MockService(1) excluded by `!Mock`; OrderService
        // is under a deeper `bar` segment so the fixed-depth pattern misses it.
        assert_eq!(res.ids, vec![NodeId(0)]);
    }
}
