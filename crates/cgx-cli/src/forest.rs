//! The ASCII call-forest renderer — the default `--format human` view for
//! `callers`, `callees`, and `reaches <from>` (the neighbor-set commands).
//!
//! It turns a [`Subgraph`] (the induced `caller → callee` adjacency a bounded
//! neighbor walk produces; [`cgx_query::neighborhood`]) into a `ps -f`-style
//! tree. Two modes:
//!
//! - **full** ([`TreeMode::Full`]): DFS from each root expanding every out-edge,
//!   so a callee reached from two callers appears under each. Bounded by a depth
//!   limit (default [`DEFAULT_TREE_DEPTH`] when unset) and a work-budget backstop.
//!   Revisiting an ancestor on the current branch renders `↺ name (cycle)` and
//!   stops descending; hitting the work budget renders `… (truncated: N more)`.
//! - **spanning** ([`TreeMode::Spanning`]): each node rendered once under its
//!   shortest-path parent, annotated `(+N call sites)` when more than one induced
//!   in-edge reaches it (N = the additional in-edges).
//!
//! Shared line shape: `fqn  file:line`, then `[if]`/`[exc]` (condition ≠ always),
//! then `[probable]`/`[possible]` (confidence ≠ certain) — the
//! `cgx-forest-edge-condition-formatting` convention. Depth is implied by
//! indentation; there is no `depth=` field. Children are sorted by `(fqn,
//! file:line)` so output is byte-identical across runs.

use std::collections::{BTreeMap, HashMap, VecDeque};

use cgx_core::{Confidence, EdgeCondition, NodeId, NodeRecord};
use cgx_query::{EdgeRec, GraphView, Subgraph};

/// Which forest shape `--tree` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum TreeMode {
    /// Expand every call edge; a node reached from N callers appears N times.
    #[default]
    Full,
    /// Render each node once under its shortest-path parent, annotating the extra
    /// in-edges as `(+N call sites)`.
    Spanning,
}

/// The depth bound applied to a full-expansion forest when `--depth` is unset.
/// An explicit `--depth` overrides this; the work budget is the hard backstop.
pub const DEFAULT_TREE_DEPTH: u32 = 2;

/// The work-budget backstop on rendered tree rows. Bounds output on a dense graph
/// regardless of depth; when hit, a `… (truncated: N more)` marker is emitted.
pub const DEFAULT_TREE_BUDGET: usize = 10_000;

/// A self-contained forest payload: the induced sub-graph plus the resolved node
/// records and the render mode/bound. Built at the CLI boundary
/// ([`ForestData::resolve`]) so the formatter needs no [`GraphView`] — the same
/// resolve-at-the-edge pattern the tabular `TableData` uses.
#[derive(Debug, Clone)]
pub struct ForestData {
    pub mode: TreeMode,
    pub max_depth: Option<u32>,
    roots: Vec<NodeId>,
    nodes: HashMap<NodeId, NodeRecord>,
    edges: Vec<EdgeRec>,
}

impl ForestData {
    /// Resolve a [`Subgraph`]'s node ids against `view` into a self-contained
    /// render payload.
    pub fn resolve(
        view: &GraphView,
        subgraph: &Subgraph,
        mode: TreeMode,
        max_depth: Option<u32>,
    ) -> ForestData {
        let nodes: HashMap<NodeId, NodeRecord> = subgraph
            .nodes
            .iter()
            .filter_map(|&id| view.try_node(id).map(|rec| (id, rec.clone())))
            .collect();
        // A root that did not resolve into `nodes` — either absent from
        // `subgraph.nodes` or absent from the view — would panic the renderer at
        // `Forest::node`'s `&self.nodes[&id]`. Rendering a forest one root short
        // is a degradation; crashing the process is not. Every caller is
        // expected to hand over a `roots ⊆ nodes` subgraph, so this filter is a
        // backstop, not a policy.
        ForestData {
            mode,
            max_depth,
            roots: subgraph
                .roots
                .iter()
                .copied()
                .filter(|id| nodes.contains_key(id))
                .collect(),
            nodes,
            edges: subgraph.edges.clone(),
        }
    }

    /// `true` when the payload has no anchor/nodes (caller prints `(no results)`).
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty() || self.nodes.is_empty()
    }
}

/// Render `data` as an ASCII forest. The trailing newline is included; an empty
/// payload yields the empty string (the caller prints the shared `(no results)`
/// message).
pub fn render(data: &ForestData) -> String {
    if data.is_empty() {
        return String::new();
    }
    let f = Forest::new(data);
    match data.mode {
        TreeMode::Full => f.render_full(data.max_depth),
        TreeMode::Spanning => f.render_spanning(),
    }
}

/// Prepared adjacency + node resolution for one render.
struct Forest<'a> {
    nodes: &'a HashMap<NodeId, NodeRecord>,
    roots: Vec<NodeId>,
    /// `children[src]` = the induced out-edges of `src`, sorted by child sort key.
    children: HashMap<NodeId, Vec<EdgeRec>>,
    /// Induced in-degree per node (how many edges point *at* it). Drives the
    /// `(+N call sites)` annotation in spanning mode.
    in_degree: HashMap<NodeId, usize>,
}

impl<'a> Forest<'a> {
    fn new(data: &'a ForestData) -> Self {
        let mut children: HashMap<NodeId, Vec<EdgeRec>> = HashMap::new();
        let mut in_degree: HashMap<NodeId, usize> = HashMap::new();
        for e in &data.edges {
            children.entry(e.src).or_default().push(e.clone());
            *in_degree.entry(e.dst).or_default() += 1;
        }
        for bucket in children.values_mut() {
            bucket.sort_by(|a, b| {
                node_sort_key(&data.nodes, a.dst).cmp(&node_sort_key(&data.nodes, b.dst))
            });
            // Render-layer dedupe: two induced call edges from the same parent to
            // the same dst with the SAME condition+confidence render byte-identically
            // and carry zero additional information for a human, so collapse the run
            // to one line. Edges to the same dst with a DIFFERENT condition/confidence
            // (e.g. `[if]` vs `[loop]` vs plain) stay distinct. Same-dst edges are
            // already adjacent after the sort above, so a consecutive-run dedupe on
            // the (dst, condition, confidence) triple suffices. This touches only the
            // rendered tree; the induced edge set (`in_degree`, the JSON neighbor
            // path) is unchanged.
            bucket.dedup_by(|a, b| {
                a.dst == b.dst && a.condition == b.condition && a.confidence == b.confidence
            });
        }
        Forest {
            nodes: &data.nodes,
            roots: data.roots.clone(),
            children,
            in_degree,
        }
    }

    fn node(&self, id: NodeId) -> &NodeRecord {
        &self.nodes[&id]
    }

    // --- full expansion ------------------------------------------------------

    fn render_full(&self, max_depth: Option<u32>) -> String {
        let depth_limit = max_depth.unwrap_or(DEFAULT_TREE_DEPTH);
        let mut w = FullWalk {
            out: String::new(),
            budget: DEFAULT_TREE_BUDGET,
            truncated_more: 0,
            depth_limit,
        };

        for &root in &self.roots {
            // Root line: bare fqn  file:line, no prefix, no edge tags. Roots are
            // not charged against the work budget — the budget bounds expansion.
            w.out.push_str(&base_line(self.node(root)));
            w.out.push('\n');

            let mut ancestors = vec![root];
            self.expand_full(root, 1, &mut ancestors, &mut Vec::new(), &mut w);
        }

        if w.truncated_more > 0 {
            w.out
                .push_str(&format!("… (truncated: {} more)\n", w.truncated_more));
        }
        w.out
    }

    fn expand_full(
        &self,
        node: NodeId,
        depth: u32,
        ancestors: &mut Vec<NodeId>,
        // `parents_is_last[i]` = whether the i-th ancestor (below the root) was the
        // last child of its parent — drives the `│` vs ` ` continuation columns.
        parents_is_last: &mut Vec<bool>,
        w: &mut FullWalk,
    ) {
        if depth > w.depth_limit {
            return;
        }
        let edges = match self.children.get(&node) {
            Some(e) => e,
            None => return,
        };
        let last_idx = edges.len() - 1;
        for (i, edge) in edges.iter().enumerate() {
            // Once the budget is spent, stop emitting and just tally would-be rows.
            if w.budget == 0 {
                w.truncated_more += 1;
                continue;
            }
            w.budget -= 1;

            let is_last = i == last_idx;
            let prefix = tree_prefix(parents_is_last, is_last);
            let child = self.node(edge.dst);

            if ancestors.contains(&edge.dst) {
                w.out.push_str(&format!("{prefix}↺ {} (cycle)\n", child.fqn));
                continue;
            }

            w.out.push_str(&prefix);
            w.out
                .push_str(&edge_line(child, edge.condition, edge.confidence));
            w.out.push('\n');

            ancestors.push(edge.dst);
            parents_is_last.push(is_last);
            self.expand_full(edge.dst, depth + 1, ancestors, parents_is_last, w);
            parents_is_last.pop();
            ancestors.pop();
        }
    }

    // --- spanning ------------------------------------------------------------

    fn render_spanning(&self) -> String {
        // BFS parent assignment: each node's parent is the source of the first
        // (shortest, lowest-sort) edge that reaches it. Roots have no parent.
        let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
        let mut visited: HashMap<NodeId, bool> = HashMap::new();
        let mut span_children: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        let mut queue: VecDeque<NodeId> = VecDeque::new();

        for &root in &self.roots {
            if visited.insert(root, true).is_none() {
                queue.push_back(root);
            }
        }
        while let Some(node) = queue.pop_front() {
            if let Some(edges) = self.children.get(&node) {
                for edge in edges {
                    if visited.contains_key(&edge.dst) {
                        continue;
                    }
                    visited.insert(edge.dst, true);
                    parent.insert(edge.dst, node);
                    span_children.entry(node).or_default().push(edge.dst);
                    queue.push_back(edge.dst);
                }
            }
        }
        // Sort each node's spanning children by the shared child sort key.
        for kids in span_children.values_mut() {
            kids.sort_by_key(|&id| node_sort_key(self.nodes, id));
        }

        let mut w = FullWalk {
            out: String::new(),
            budget: DEFAULT_TREE_BUDGET,
            truncated_more: 0,
            // Depth is already bounded by the neighborhood walk; spanning has no
            // additional render-time depth limit (each node appears once).
            depth_limit: u32::MAX,
        };
        for &root in &self.roots {
            // Roots are not charged against the budget — the budget bounds expansion.
            w.out.push_str(&base_line(self.node(root)));
            w.out.push('\n');
            self.emit_spanning(root, &span_children, &mut Vec::new(), &mut w);
        }
        if w.truncated_more > 0 {
            w.out
                .push_str(&format!("… (truncated: {} more)\n", w.truncated_more));
        }
        w.out
    }

    fn emit_spanning(
        &self,
        node: NodeId,
        span_children: &BTreeMap<NodeId, Vec<NodeId>>,
        parents_is_last: &mut Vec<bool>,
        w: &mut FullWalk,
    ) {
        let kids = match span_children.get(&node) {
            Some(k) => k,
            None => return,
        };
        let last_idx = kids.len() - 1;
        for (i, &child_id) in kids.iter().enumerate() {
            // Once the budget is spent, stop emitting and just tally would-be rows.
            if w.budget == 0 {
                w.truncated_more += 1;
                continue;
            }
            w.budget -= 1;

            let is_last = i == last_idx;
            let prefix = tree_prefix(parents_is_last, is_last);
            let child = self.node(child_id);
            // The edge metadata for the spanning parent→child tree edge: pick the
            // sort-first induced edge between the two (deterministic).
            let (cond, conf) = self.tree_edge_meta(node, child_id);
            w.out.push_str(&prefix);
            w.out.push_str(&edge_line(child, cond, conf));
            // `(+N call sites)` when more than one induced edge reaches this node.
            let extra = self.in_degree.get(&child_id).copied().unwrap_or(0).saturating_sub(1);
            if extra > 0 {
                w.out.push_str(&format!("  (+{extra} call sites)"));
            }
            w.out.push('\n');

            parents_is_last.push(is_last);
            self.emit_spanning(child_id, span_children, parents_is_last, w);
            parents_is_last.pop();
        }
    }

    /// The condition/confidence to render for the spanning tree edge `src → dst`:
    /// the metadata of the sort-first induced edge between them (children are
    /// already sorted, so this is the deterministic representative).
    fn tree_edge_meta(&self, src: NodeId, dst: NodeId) -> (EdgeCondition, Confidence) {
        self.children
            .get(&src)
            .and_then(|edges| edges.iter().find(|e| e.dst == dst))
            .map(|e| (e.condition, e.confidence))
            .unwrap_or((EdgeCondition::Always, Confidence::Certain))
    }
}

/// Mutable bookkeeping threaded through the full-expansion DFS.
struct FullWalk {
    out: String,
    /// Remaining work budget (rendered child rows). Zero ⇒ tally truncation only.
    budget: usize,
    /// Count of rows trimmed after the budget was spent (the `N` in the marker).
    truncated_more: usize,
    depth_limit: u32,
}

/// The deterministic child sort key: `(fqn, file, line)`.
fn node_sort_key(nodes: &HashMap<NodeId, NodeRecord>, id: NodeId) -> (String, String, u32) {
    let n = &nodes[&id];
    (n.fqn.clone(), n.file.clone(), n.line_start)
}

/// The box-drawing prefix for a line: one continuation column per ancestor
/// (`│  ` when that ancestor had more siblings, `   ` when it was last), then this
/// node's connector (`└─ ` when it is the last child, `├─ ` otherwise).
fn tree_prefix(parents_is_last: &[bool], is_last: bool) -> String {
    let mut s = String::new();
    for &parent_last in parents_is_last {
        s.push_str(if parent_last { "   " } else { "│  " });
    }
    s.push_str(if is_last { "└─ " } else { "├─ " });
    s
}

/// The bare `fqn  file:line` line (root nodes; no edge context).
fn base_line(n: &NodeRecord) -> String {
    format!("{}  {}:{}", n.fqn, n.file, n.line_start)
}

/// A child line: `fqn  file:line` plus the non-default condition/confidence tags.
fn edge_line(n: &NodeRecord, condition: EdgeCondition, confidence: Confidence) -> String {
    let mut s = base_line(n);
    if let Some(tag) = condition_tag(condition) {
        s.push_str(&format!("  [{tag}]"));
    }
    if let Some(tag) = confidence_tag(confidence) {
        s.push_str(&format!("  [{tag}]"));
    }
    s
}

/// The edge-condition tag, or `None` for the default `always`
/// (`cgx-forest-edge-condition-formatting`): `conditional`→`if`, `exception`→`exc`,
/// `loop`/`panic` verbatim.
fn condition_tag(c: EdgeCondition) -> Option<&'static str> {
    match c {
        EdgeCondition::Always => None,
        EdgeCondition::Conditional => Some("if"),
        EdgeCondition::Exception => Some("exc"),
        EdgeCondition::Loop => Some("loop"),
        EdgeCondition::Panic => Some("panic"),
    }
}

/// The confidence tag, or `None` for the default `certain`.
fn confidence_tag(c: Confidence) -> Option<&'static str> {
    match c {
        Confidence::Certain => None,
        Confidence::Probable => Some("probable"),
        Confidence::Possible => Some("possible"),
    }
}
