//! Shared fixtures for cgx-store tests.

use cgx_core::{
    Candidate, Confidence, CutMarker, CutMarkers, EdgeCondition, EdgeId, EdgeKind, EdgeRecord,
    EntrypointKind, NodeId, NodeRecord, Param, Signature, SiteId, SymbolKind, Tier, Visibility,
};
use cgx_store::LinkedGraph;

/// A small, fully-populated linked graph in canonical order. Deterministic: built
/// the same way every call, so two builds are byte-identical.
pub fn sample_graph() -> LinkedGraph {
    let nodes = vec![
        NodeRecord {
            id: NodeId(0),
            kind: SymbolKind::Function,
            fqn: "app::main".into(),
            file: "src/main.rs".into(),
            line_start: 1,
            line_end: 10,
            lang: "rust".into(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: Some(EntrypointKind::Main),
            signature: Some(Signature {
                params: vec![],
                return_type_text: None,
                type_params: vec![],
                receiver: None,
            }),
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
        },
        NodeRecord {
            id: NodeId(1),
            kind: SymbolKind::Method,
            fqn: "app::db::Conn::commit".into(),
            file: "src/db.rs".into(),
            line_start: 20,
            line_end: 30,
            lang: "rust".into(),
            visibility: Visibility::Private,
            is_abstract: false,
            entrypoint_kind: None,
            signature: Some(Signature {
                params: vec![Param {
                    name: "self".into(),
                    type_text: Some("&mut Conn".into()),
                    has_default: false,
                    variadic: false,
                }],
                return_type_text: Some("Result<()>".into()),
                type_params: vec![],
                receiver: Some("Conn".into()),
            }),
            own_effects: cgx_core::EffectSet::from_iter_canonical([
                cgx_core::Effect::IoFile,
                cgx_core::Effect::Nondeterministic,
            ]),
            transitive_effects: cgx_core::EffectSet::new(),
        },
        NodeRecord {
            id: NodeId(2),
            kind: SymbolKind::Method,
            fqn: "app::db::Conn::rollback".into(),
            file: "src/db.rs".into(),
            line_start: 31,
            line_end: 40,
            lang: "rust".into(),
            visibility: Visibility::Private,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
        },
    ];

    let edges = vec![
        EdgeRecord {
            id: EdgeId(0),
            src: NodeId(0),
            dst: NodeId(1),
            kind: EdgeKind::CallsVirtual,
            condition: EdgeCondition::Conditional,
            confidence: Confidence::Possible,
            tier: Tier::ScopeGraph,
            rule: "import-ref".into(),
            site_id: Some(SiteId::derive("app::main", "src/main.rs", 5, 8)),
            stmt_index: Some(1),
            cut_markers: CutMarkers::from_iter_canonical([CutMarker::ViaDi]),
            implicit: None,
            candidate_group: Some(0),
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
        },
        EdgeRecord {
            id: EdgeId(1),
            src: NodeId(0),
            dst: NodeId(2),
            kind: EdgeKind::CallsVirtual,
            condition: EdgeCondition::Exception,
            confidence: Confidence::Possible,
            tier: Tier::ScopeGraph,
            rule: "import-ref".into(),
            site_id: Some(SiteId::derive("app::main", "src/main.rs", 7, 8)),
            stmt_index: Some(2),
            cut_markers: CutMarkers::new(),
            implicit: None,
            candidate_group: Some(0),
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
        },
    ];

    // A candidate set: the virtual call at the group resolves to both methods.
    let candidates = vec![
        Candidate {
            candidate_group: 0,
            dst: NodeId(1),
            rank: 0,
        },
        Candidate {
            candidate_group: 0,
            dst: NodeId(2),
            rank: 1,
        },
    ];

    LinkedGraph::new(nodes, edges, candidates)
}
