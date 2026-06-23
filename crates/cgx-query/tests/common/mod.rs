//! Test graph builder shared across the integration suites.
//!
//! Builds a `(nodes, edges, candidates)` triple in the canonical layout the
//! resolver/store produce: symbol nodes sorted by `(file, line_start, fqn)` with
//! dense `NodeId`s equal to their position, edges with dense `EdgeId`s. Tests use
//! FQNs to refer to nodes and let the builder assign ids, so a test never hand-
//! computes a dense index.

#![allow(dead_code)]

use std::collections::BTreeMap;

use cgx_core::edge::EdgeRecord;
use cgx_core::{
    Candidate, Confidence, EdgeCondition, EdgeId, EdgeKind, EntrypointKind, NodeId, NodeRecord,
    SymbolKind, Tier, Visibility,
};

/// Fluent builder for a small linked graph.
pub struct GraphBuilder {
    nodes: Vec<NodeSpec>,
    edges: Vec<EdgeSpec>,
    candidates: Vec<Candidate>,
}

struct NodeSpec {
    fqn: String,
    file: String,
    line: u32,
    kind: SymbolKind,
    entrypoint: Option<EntrypointKind>,
}

struct EdgeSpec {
    src: String,
    dst: String,
    kind: EdgeKind,
    condition: EdgeCondition,
    confidence: Confidence,
    tier: Tier,
    rule: String,
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphBuilder {
    pub fn new() -> Self {
        GraphBuilder {
            nodes: Vec::new(),
            edges: Vec::new(),
            candidates: Vec::new(),
        }
    }

    /// Add a function symbol. `file`/`line` default to a synthetic per-fqn value
    /// derived from insertion order so node ordering is well-defined.
    pub fn func(mut self, fqn: &str) -> Self {
        let line = (self.nodes.len() as u32 + 1) * 10;
        self.nodes.push(NodeSpec {
            fqn: fqn.to_string(),
            file: "src/lib.rs".to_string(),
            line,
            kind: SymbolKind::Function,
            entrypoint: None,
        });
        self
    }

    /// Add a function at an explicit file/line (for ordering tests).
    pub fn func_at(mut self, fqn: &str, file: &str, line: u32) -> Self {
        self.nodes.push(NodeSpec {
            fqn: fqn.to_string(),
            file: file.to_string(),
            line,
            kind: SymbolKind::Function,
            entrypoint: None,
        });
        self
    }

    /// Add a symbol of an explicit kind.
    pub fn sym(mut self, fqn: &str, kind: SymbolKind) -> Self {
        let line = (self.nodes.len() as u32 + 1) * 10;
        self.nodes.push(NodeSpec {
            fqn: fqn.to_string(),
            file: "src/lib.rs".to_string(),
            line,
            kind,
            entrypoint: None,
        });
        self
    }

    /// Mark an entrypoint function.
    pub fn entry(mut self, fqn: &str, kind: EntrypointKind) -> Self {
        let line = (self.nodes.len() as u32 + 1) * 10;
        self.nodes.push(NodeSpec {
            fqn: fqn.to_string(),
            file: "src/lib.rs".to_string(),
            line,
            kind: SymbolKind::Function,
            entrypoint: Some(kind),
        });
        self
    }

    /// A direct, certain, unconditional call edge.
    pub fn calls(self, src: &str, dst: &str) -> Self {
        self.edge(
            src,
            dst,
            EdgeKind::Calls,
            EdgeCondition::Always,
            Confidence::Certain,
        )
    }

    /// A call edge with an explicit condition (confidence = certain).
    pub fn calls_cond(self, src: &str, dst: &str, cond: EdgeCondition) -> Self {
        self.edge(src, dst, EdgeKind::Calls, cond, Confidence::Certain)
    }

    /// A call edge with explicit condition and confidence.
    pub fn calls_full(self, src: &str, dst: &str, cond: EdgeCondition, conf: Confidence) -> Self {
        self.edge(src, dst, EdgeKind::Calls, cond, conf)
    }

    /// A `spawns` edge (detached error domain).
    pub fn spawns(self, src: &str, dst: &str, cond: EdgeCondition) -> Self {
        self.edge(src, dst, EdgeKind::Spawns, cond, Confidence::Certain)
    }

    /// A structural (non-call) edge — should be ignored by call walks.
    pub fn contains(self, src: &str, dst: &str) -> Self {
        self.edge(
            src,
            dst,
            EdgeKind::Contains,
            EdgeCondition::Always,
            Confidence::Certain,
        )
    }

    /// A call edge stamped with an explicit resolution `tier`/`rule` (and
    /// confidence) — for asserting `explain` provenance (P7/IF-6).
    pub fn calls_prov(
        mut self,
        src: &str,
        dst: &str,
        confidence: Confidence,
        tier: Tier,
        rule: &str,
    ) -> Self {
        self.edges.push(EdgeSpec {
            src: src.to_string(),
            dst: dst.to_string(),
            kind: EdgeKind::Calls,
            condition: EdgeCondition::Always,
            confidence,
            tier,
            rule: rule.to_string(),
        });
        self
    }

    fn edge(
        mut self,
        src: &str,
        dst: &str,
        kind: EdgeKind,
        condition: EdgeCondition,
        confidence: Confidence,
    ) -> Self {
        self.edges.push(EdgeSpec {
            src: src.to_string(),
            dst: dst.to_string(),
            kind,
            condition,
            confidence,
            tier: Tier::ScopeGraph,
            rule: "test".to_string(),
        });
        self
    }

    /// Materialize into `(nodes, edges, candidates)` in canonical order.
    pub fn build(self) -> (Vec<NodeRecord>, Vec<EdgeRecord>, Vec<Candidate>) {
        // Sort node specs by (file, line, fqn), assign dense ids = position.
        let mut specs = self.nodes;
        specs.sort_by(|a, b| {
            (a.file.as_str(), a.line, a.fqn.as_str()).cmp(&(
                b.file.as_str(),
                b.line,
                b.fqn.as_str(),
            ))
        });
        let mut id_of: BTreeMap<String, NodeId> = BTreeMap::new();
        let nodes: Vec<NodeRecord> = specs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let id = NodeId(i as u32);
                id_of.insert(s.fqn.clone(), id);
                NodeRecord {
                    id,
                    kind: s.kind,
                    fqn: s.fqn.clone(),
                    file: s.file.clone(),
                    line_start: s.line,
                    line_end: s.line,
                    lang: "rust".to_string(),
                    visibility: Visibility::Public,
                    is_abstract: false,
                    entrypoint_kind: s.entrypoint,
                    signature: None,
                    own_effects: cgx_core::EffectSet::new(),
                    transitive_effects: cgx_core::EffectSet::new(),
                }
            })
            .collect();

        // Build edges, sort canonically, then assign dense edge ids.
        let mut edges: Vec<EdgeRecord> = self
            .edges
            .iter()
            .map(|e| EdgeRecord {
                id: EdgeId(0),
                src: id_of[&e.src],
                dst: id_of[&e.dst],
                kind: e.kind,
                condition: e.condition,
                confidence: e.confidence,
                tier: e.tier,
                rule: e.rule.clone(),
                site_id: None,
                stmt_index: None,
                cut_markers: Default::default(),
                implicit: None,
                candidate_group: None,
                established_by: None,
                cfg_condition: None,
                macro_origin: None,
                transform: None,
            })
            .collect();
        cgx_core::sort::sort_edges(&mut edges);
        for (i, e) in edges.iter_mut().enumerate() {
            e.id = EdgeId(i as u32);
        }

        (nodes, edges, self.candidates)
    }
}

/// Find the node id for an fqn in a node slice.
pub fn id_of(nodes: &[NodeRecord], fqn: &str) -> NodeId {
    nodes
        .iter()
        .find(|n| n.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"))
        .id
}
