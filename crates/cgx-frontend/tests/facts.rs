//! FileFacts type tests (WP-03): scope tree, canonicalization, codec round-trip.

use cgx_core::codec;
use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::{
    CutHint, FileFacts, ImportFact, ImportedName, RawRef, RefKind, ScopeId, ScopeTree, SymbolDef,
};
use smallvec::smallvec;

#[test]
fn empty_facts_have_a_single_root_scope() {
    let facts = FileFacts::empty();
    assert_eq!(facts.scopes.len(), 1);
    assert_eq!(facts.scopes.scopes[0].parent, None);
    assert!(facts.is_degraded_empty());
}

#[test]
fn scope_tree_push_links_to_parent() {
    let mut tree = ScopeTree::new();
    let child = tree.push(ScopeId::ROOT, Some("m::f".to_string()));
    let grand = tree.push(child, None);

    assert_eq!(tree.scopes[child.index()].parent, Some(ScopeId::ROOT));
    assert_eq!(tree.scopes[grand.index()].parent, Some(child));
    assert_eq!(
        tree.scopes[child.index()].owner_fqn.as_deref(),
        Some("m::f")
    );
}

#[test]
fn scope_ancestors_walk_root_inclusive() {
    let mut tree = ScopeTree::new();
    let a = tree.push(ScopeId::ROOT, Some("a".to_string()));
    let b = tree.push(a, Some("a::b".to_string()));

    let chain: Vec<ScopeId> = tree.ancestors(b).collect();
    assert_eq!(chain, vec![b, a, ScopeId::ROOT]);
}

fn def(fqn: &str, line: u32) -> SymbolDef {
    SymbolDef {
        fqn: fqn.to_string(),
        kind: SymbolKind::Function,
        visibility: Visibility::Internal,
        scope: ScopeId::ROOT,
        span: Span::new("f.x", line, Some(1)),
        line_end: line,
        is_abstract: false,
        signature: None,
    }
}

fn call(name: &str, line: u32) -> RawRef {
    RawRef {
        name_path: smallvec![name.to_string()],
        scope: ScopeId::ROOT,
        kind: RefKind::Call,
        edge_condition: EdgeCondition::Always,
        implicit: None,
        span: Span::new("f.x", line, Some(1)),
        stmt_index: 0,
        arity: None,
        cut_markers: smallvec![],
    }
}

#[test]
fn canonicalize_sorts_facts_deterministically() {
    let mut a = FileFacts::empty();
    a.defs.push(def("z", 3));
    a.defs.push(def("a", 1));
    a.refs.push(call("zzz", 9));
    a.refs.push(call("aaa", 2));
    a.canonicalize();

    let mut b = FileFacts::empty();
    b.defs.push(def("a", 1));
    b.defs.push(def("z", 3));
    b.refs.push(call("aaa", 2));
    b.refs.push(call("zzz", 9));
    b.canonicalize();

    // Same facts inserted in different orders canonicalize identically.
    assert_eq!(a, b);
}

#[test]
fn canonicalize_dedups_and_sorts_cut_markers_on_refs() {
    let mut facts = FileFacts::empty();
    let mut r = call("f", 1);
    r.cut_markers = smallvec![
        CutMarker::Unresolved,
        CutMarker::Dynamic,
        CutMarker::Dynamic
    ];
    facts.refs.push(r);
    facts.canonicalize();

    let markers: Vec<CutMarker> = facts.refs[0].cut_markers.iter().copied().collect();
    assert_eq!(markers, vec![CutMarker::Dynamic, CutMarker::Unresolved]);
}

#[test]
fn canonicalize_is_idempotent() {
    let mut facts = FileFacts::empty();
    facts.defs.push(def("b", 2));
    facts.defs.push(def("a", 1));
    facts.canonicalize();
    let once = facts.clone();
    facts.canonicalize();
    assert_eq!(once, facts);
}

#[test]
fn codec_round_trips_canonical_facts() {
    let mut facts = FileFacts::empty();
    facts.defs.push(def("m::f", 1));
    facts.refs.push(call("g", 2));
    facts.imports.push(ImportFact {
        specifier: "std::collections".to_string(),
        names: vec![ImportedName {
            name: "HashMap".to_string(),
            alias: None,
        }],
        glob: false,
        re_export: false,
        scope: ScopeId::ROOT,
        span: Span::new("f.x", 1, Some(1)),
    });
    facts.cut_hints.push(CutHint {
        marker: CutMarker::UnexpandedMacro,
        span: Span::new("f.x", 5, Some(3)),
        macro_origin: Some("derive(Foo)".to_string()),
    });
    facts.canonicalize();

    let bytes = codec::encode(&facts).unwrap();
    let decoded: FileFacts = codec::decode(&bytes).unwrap();
    assert_eq!(facts, decoded);
}

#[test]
fn is_degraded_empty_is_false_once_a_def_is_present() {
    let mut facts = FileFacts::empty();
    assert!(facts.is_degraded_empty());
    facts.defs.push(def("a", 1));
    assert!(!facts.is_degraded_empty());
}
