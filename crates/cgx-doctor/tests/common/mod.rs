//! Minimal graph builder shared across cgx-doctor integration tests.
//! Mirrors the builder in cgx-query/tests/common, kept separate so cgx-doctor
//! does not depend on the cgx-query crate.

#![allow(dead_code)]

use std::collections::BTreeMap;

use cgx_core::cut::{CutMarker, CutMarkers};
use cgx_core::edge::EdgeRecord;
use cgx_core::{
    Candidate, Confidence, EdgeCondition, EdgeId, EdgeKind, NodeId, NodeRecord, SymbolKind, Tier,
    Visibility,
};
use cgx_store::LinkedGraph;

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
}

struct EdgeSpec {
    src: String,
    dst: String,
    kind: EdgeKind,
    condition: EdgeCondition,
    confidence: Confidence,
    cut_markers: Vec<CutMarker>,
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

    pub fn func(mut self, fqn: &str) -> Self {
        let line = (self.nodes.len() as u32 + 1) * 10;
        self.nodes.push(NodeSpec {
            fqn: fqn.to_string(),
            file: "src/lib.rs".to_string(),
            line,
            kind: SymbolKind::Function,
        });
        self
    }

    pub fn calls(self, src: &str, dst: &str) -> Self {
        self.edge(
            src,
            dst,
            EdgeKind::Calls,
            EdgeCondition::Always,
            Confidence::Certain,
            vec![],
        )
    }

    pub fn calls_conf(self, src: &str, dst: &str, conf: Confidence) -> Self {
        self.edge(
            src,
            dst,
            EdgeKind::Calls,
            EdgeCondition::Always,
            conf,
            vec![],
        )
    }

    pub fn calls_with_marker(self, src: &str, dst: &str, marker: CutMarker) -> Self {
        self.edge(
            src,
            dst,
            EdgeKind::Calls,
            EdgeCondition::Always,
            Confidence::Possible,
            vec![marker],
        )
    }

    pub fn unresolved_call(self, src: &str, dst: &str) -> Self {
        self.calls_with_marker(src, dst, CutMarker::Unresolved)
    }

    fn edge(
        mut self,
        src: &str,
        dst: &str,
        kind: EdgeKind,
        condition: EdgeCondition,
        confidence: Confidence,
        cut_markers: Vec<CutMarker>,
    ) -> Self {
        self.edges.push(EdgeSpec {
            src: src.to_string(),
            dst: dst.to_string(),
            kind,
            condition,
            confidence,
            cut_markers,
        });
        self
    }

    pub fn build(self) -> LinkedGraph {
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
                    own_effects: cgx_core::EffectSet::new(),
                    transitive_effects: cgx_core::EffectSet::new(),
                }
            })
            .collect();

        let mut edges: Vec<EdgeRecord> = self
            .edges
            .iter()
            .map(|e| EdgeRecord {
                id: EdgeId(0),
                src: *id_of
                    .get(&e.src)
                    .unwrap_or_else(|| panic!("no node {}", e.src)),
                dst: *id_of
                    .get(&e.dst)
                    .unwrap_or_else(|| panic!("no node {}", e.dst)),
                kind: e.kind,
                condition: e.condition,
                confidence: e.confidence,
                tier: Tier::ScopeGraph,
                rule: "test".to_string(),
                site_id: None,
                stmt_index: None,
                cut_markers: CutMarkers::from_iter_canonical(e.cut_markers.iter().copied()),
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

        LinkedGraph::new(nodes, edges, self.candidates)
    }
}
