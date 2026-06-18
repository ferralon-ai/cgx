//! Deterministic in-crate fixture builder for cgx-cql integration tests.
//!
//! Builds a `(nodes, edges, candidates)` triple in the canonical layout the
//! resolver/store produce: symbol nodes sorted by `(file, line_start, fqn)` with
//! dense `NodeId`s = position; edges canonically sorted with dense `EdgeId`s.
//! Tests refer to nodes by FQN and let the builder assign ids. Richer than the
//! cgx-query test builder: explicit symbol kinds and arbitrary edge kinds so we
//! can model a `MyStruct` type with methods, exception vs non-exception edges,
//! and DATA_FLOW pedigree.

#![allow(dead_code)]

use std::collections::BTreeMap;

use cgx_core::edge::EdgeRecord;
use cgx_core::{
    Candidate, Confidence, EdgeCondition, EdgeId, EdgeKind, NodeId, NodeRecord, SymbolKind, Tier,
    Visibility,
};
use cgx_query::GraphView;

struct NodeSpec {
    fqn: String,
    file: String,
    line: u32,
    kind: SymbolKind,
}

struct EdgeSpec {
    src: String,
    dst: String,
    kind: EdgeKind,
    condition: EdgeCondition,
    confidence: Confidence,
}

#[derive(Default)]
pub struct GraphBuilder {
    nodes: Vec<NodeSpec>,
    edges: Vec<EdgeSpec>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        GraphBuilder::default()
    }

    /// Add a symbol of an explicit kind at an explicit file/line.
    pub fn sym(mut self, fqn: &str, kind: SymbolKind, file: &str, line: u32) -> Self {
        self.nodes.push(NodeSpec {
            fqn: fqn.to_string(),
            file: file.to_string(),
            line,
            kind,
        });
        self
    }

    /// A `Contains` edge (type→member). Used to model MEMBER_OF (arrow-inverted).
    pub fn contains(self, src: &str, dst: &str) -> Self {
        self.edge(
            src,
            dst,
            EdgeKind::Contains,
            EdgeCondition::Always,
            Confidence::Certain,
        )
    }

    /// A direct certain unconditional call edge.
    pub fn calls(self, src: &str, dst: &str) -> Self {
        self.edge(
            src,
            dst,
            EdgeKind::Calls,
            EdgeCondition::Always,
            Confidence::Certain,
        )
    }

    /// A call edge with an explicit condition.
    pub fn calls_cond(self, src: &str, dst: &str, cond: EdgeCondition) -> Self {
        self.edge(src, dst, EdgeKind::Calls, cond, Confidence::Certain)
    }

    /// A DATA_FLOW (`DerivesFrom`) edge at `certain` confidence.
    pub fn data_flow(self, src: &str, dst: &str) -> Self {
        self.data_flow_conf(src, dst, Confidence::Certain)
    }

    /// A DATA_FLOW (`DerivesFrom`) edge at an explicit confidence.
    pub fn data_flow_conf(self, src: &str, dst: &str, confidence: Confidence) -> Self {
        self.edge(
            src,
            dst,
            EdgeKind::DerivesFrom,
            EdgeCondition::Always,
            confidence,
        )
    }

    pub fn edge(
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
        });
        self
    }

    /// Materialize into a [`GraphView`].
    pub fn view(self) -> GraphView {
        let (n, e, c) = self.build();
        GraphView::new(n, e, c)
    }

    fn build(self) -> (Vec<NodeRecord>, Vec<EdgeRecord>, Vec<Candidate>) {
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
                    entrypoint_kind: None,
                    signature: None,
                }
            })
            .collect();

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
                tier: Tier::ScopeGraph,
                rule: "test".to_string(),
                site_id: None,
                stmt_index: None,
                cut_markers: Default::default(),
                implicit: None,
                candidate_group: None,
                established_by: None,
                cfg_condition: None,
                macro_origin: None,
            })
            .collect();
        cgx_core::sort::sort_edges(&mut edges);
        for (i, e) in edges.iter_mut().enumerate() {
            e.id = EdgeId(i as u32);
        }

        (nodes, edges, Vec::new())
    }
}
