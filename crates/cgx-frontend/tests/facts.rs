//! FileFacts type tests (WP-03): scope tree, canonicalization, codec round-trip.

use cgx_core::codec;
use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::{
    AnonRoot, BaseExpr, CallArg, CutHint, FileFacts, ImportFact, ImportedName, LiteralKind,
    ParamKind, RawRef, RefKind, ScopeId, ScopeTree, SymbolDef, TypeExpr, TypeFact, ValueSource,
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

fn bind(func: &str, var: &str, src: ValueSource) -> TypeFact {
    TypeFact::Bind {
        func: func.to_string(),
        var: var.to_string(),
        src,
    }
}

#[test]
fn canonicalize_sorts_and_dedups_type_facts() {
    let mut facts = FileFacts::empty();
    facts.type_facts.push(bind("m::g", "y", ValueSource::Null));
    facts
        .type_facts
        .push(bind("m::f", "x", ValueSource::Opaque));
    facts.type_facts.push(bind("m::g", "y", ValueSource::Null));
    facts.canonicalize();
    assert_eq!(
        facts.type_facts,
        vec![
            bind("m::f", "x", ValueSource::Opaque),
            bind("m::g", "y", ValueSource::Null),
        ]
    );
    assert!(!facts.is_degraded_empty());
}

#[test]
fn module_alone_does_not_make_facts_non_degraded() {
    let mut facts = FileFacts::empty();
    facts.module = Some("pkg::m".to_string());
    assert!(facts.is_degraded_empty());
}

#[test]
fn codec_round_trips_every_type_fact_variant() {
    let named = |p: &[&str]| TypeExpr::Named {
        path: p.iter().map(|s| s.to_string()).collect(),
        indirect: false,
    };
    let mut facts = FileFacts::empty();
    facts.module = Some("pkg::m".to_string());
    facts.type_facts = vec![
        TypeFact::Param {
            func: "pkg::m::f".into(),
            name: "r".into(),
            index: None,
            kind: ParamKind::Receiver,
            ty: Some(TypeExpr::Named {
                path: smallvec!["T".into()],
                indirect: true,
            }),
        },
        TypeFact::Param {
            func: "pkg::m::f".into(),
            name: "kw".into(),
            index: Some(3),
            kind: ParamKind::VarKeywords,
            ty: None,
        },
        bind(
            "pkg::m::f",
            "x",
            ValueSource::Enter(Box::new(ValueSource::Element(Box::new(ValueSource::Var(
                "xs".into(),
            ))))),
        ),
        bind("pkg::m::f", "d", ValueSource::Declared(TypeExpr::Unknown)),
        bind("pkg::m::f", "n", ValueSource::New(named(&["pkg", "Foo"]))),
        bind(
            "pkg::m::f",
            "c",
            ValueSource::Call(smallvec!["make".into()]),
        ),
        bind("pkg::m::f", "l", ValueSource::Literal(LiteralKind::Bytes)),
        TypeFact::FieldBind {
            func: "pkg::m::C::__init__".into(),
            field: "h".into(),
            src: ValueSource::Null,
        },
        TypeFact::ClassBases {
            class: "pkg::m::C".into(),
            bases: vec![
                BaseExpr::Type(TypeExpr::Generic {
                    head: smallvec!["Generic".into()],
                    args: vec![TypeExpr::Union(vec![named(&["A"]), named(&["B"])])],
                    indirect: false,
                }),
                BaseExpr::Call(smallvec!["declarative_base".into()]),
                BaseExpr::Unknown,
            ],
        },
        TypeFact::AnonReceiver {
            func: "pkg::m::f".into(),
            line: 4,
            col: 9,
            method: "join".into(),
            depth: 1,
            root: AnonRoot::Literal(LiteralKind::Str),
        },
        TypeFact::AnonReceiver {
            func: "pkg::m::f".into(),
            line: 5,
            col: 9,
            method: "m".into(),
            depth: 1,
            root: AnonRoot::Call(smallvec!["Foo".into()]),
        },
        TypeFact::CallArgs {
            func: "pkg::m::f".into(),
            line: 6,
            col: 5,
            callee: smallvec!["pkg".into(), "g".into()],
            args: vec![
                CallArg {
                    keyword: None,
                    value: ValueSource::Var("x".into()),
                },
                CallArg {
                    keyword: Some("k".into()),
                    value: ValueSource::Opaque,
                },
            ],
        },
        TypeFact::Return {
            func: "pkg::m::f".into(),
            ty: named(&["Foo"]),
        },
        TypeFact::FieldType {
            class: "pkg::m::C".into(),
            field: "h".into(),
            ty: TypeExpr::Unknown,
        },
    ];
    facts.canonicalize();

    let bytes = codec::encode(&facts).unwrap();
    let decoded: FileFacts = codec::decode(&bytes).unwrap();
    assert_eq!(facts, decoded);
}

/// Hex of the postcard encoding of `value`.
fn pinned_hex<T: serde::Serialize>(value: &T) -> String {
    codec::encode(value)
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The receiver-typing enums are a persisted ABI: one value of every variant,
/// in declaration order, must keep encoding to these exact bytes. Reordering
/// variants or changing a variant's fields changes them; appending a variant
/// only appends a new entry here.
#[test]
fn type_fact_encodings_are_pinned() {
    let n = || -> smallvec::SmallVec<[String; 2]> { smallvec!["a".into(), "B".into()] };
    let named = TypeExpr::Named {
        path: n(),
        indirect: true,
    };
    let type_exprs = vec![
        named.clone(),
        TypeExpr::Generic {
            head: n(),
            args: vec![TypeExpr::Unknown],
            indirect: true,
        },
        TypeExpr::Union(vec![TypeExpr::Unknown]),
        TypeExpr::Unknown,
    ];
    let param_kinds = vec![
        ParamKind::Receiver,
        ParamKind::Positional,
        ParamKind::KeywordOnly,
        ParamKind::VarArgs,
        ParamKind::VarKeywords,
    ];
    let literal_kinds = vec![
        LiteralKind::Str,
        LiteralKind::Bytes,
        LiteralKind::Num,
        LiteralKind::Bool,
        LiteralKind::List,
        LiteralKind::Dict,
        LiteralKind::Set,
        LiteralKind::Tuple,
    ];
    let sources = vec![
        ValueSource::Var("v".into()),
        ValueSource::New(TypeExpr::Unknown),
        ValueSource::Call(n()),
        ValueSource::Declared(TypeExpr::Unknown),
        ValueSource::Enter(Box::new(ValueSource::Null)),
        ValueSource::Element(Box::new(ValueSource::Null)),
        ValueSource::Literal(LiteralKind::Tuple),
        ValueSource::Null,
        ValueSource::Opaque,
    ];
    let roots = vec![
        AnonRoot::Literal(LiteralKind::Bytes),
        AnonRoot::Super,
        AnonRoot::Call(n()),
        AnonRoot::Subscript,
        AnonRoot::Other,
    ];
    let bases = vec![
        BaseExpr::Type(TypeExpr::Unknown),
        BaseExpr::Call(n()),
        BaseExpr::Unknown,
    ];
    let facts = vec![
        TypeFact::Param {
            func: "f".into(),
            name: "p".into(),
            index: Some(2),
            kind: ParamKind::VarArgs,
            ty: Some(TypeExpr::Unknown),
        },
        TypeFact::Bind {
            func: "f".into(),
            var: "v".into(),
            src: ValueSource::Null,
        },
        TypeFact::FieldBind {
            func: "f".into(),
            field: "x".into(),
            src: ValueSource::Opaque,
        },
        TypeFact::ClassBases {
            class: "C".into(),
            bases: vec![BaseExpr::Unknown],
        },
        TypeFact::AnonReceiver {
            func: "f".into(),
            line: 300,
            col: 4,
            method: "m".into(),
            depth: 2,
            root: AnonRoot::Super,
        },
        TypeFact::CallArgs {
            func: "f".into(),
            line: 1,
            col: 2,
            callee: n(),
            args: vec![CallArg {
                keyword: Some("k".into()),
                value: ValueSource::Null,
            }],
        },
        TypeFact::Return {
            func: "f".into(),
            ty: TypeExpr::Unknown,
        },
        TypeFact::FieldType {
            class: "C".into(),
            field: "x".into(),
            ty: named,
        },
    ];
    let got = [
        ("TypeExpr", pinned_hex(&type_exprs)),
        ("ParamKind", pinned_hex(&param_kinds)),
        ("LiteralKind", pinned_hex(&literal_kinds)),
        ("ValueSource", pinned_hex(&sources)),
        ("AnonRoot", pinned_hex(&roots)),
        ("BaseExpr", pinned_hex(&bases)),
        ("TypeFact", pinned_hex(&facts)),
    ];
    let want = [
        ("TypeExpr", "040002016101420101020161014201030102010303"),
        ("ParamKind", "050001020304"),
        ("LiteralKind", "080001020304050607"),
        ("ValueSource", "09000176010302020161014203030407050706070708"),
        ("AnonRoot", "050001010202016101420304"),
        ("BaseExpr", "03000301020161014202"),
        ("TypeFact", "08000166017001020301030101660176070201660178080301430102040166ac0204016d0201050166010202016101420101016b0706016603070143017800020161014201"),
    ];
    for ((name, got), (_, want)) in got.iter().zip(want) {
        assert_eq!(got, want, "{name} encoding changed");
    }
}
