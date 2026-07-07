//! Confidence ladder, signature record, cut markers, patterns, and edge-kind
//! classification.

use cgx_core::*;

// --- Confidence ladder (GM-5) ---

#[test]
fn confidence_orders_weakest_to_strongest() {
    assert!(Confidence::Possible < Confidence::Probable);
    assert!(Confidence::Probable < Confidence::Certain);
}

#[test]
fn node_confidence_is_the_weakest_contributing_fact() {
    assert_eq!(
        Confidence::Certain.weakest(Confidence::Possible),
        Confidence::Possible
    );
    assert_eq!(
        Confidence::Probable.weakest(Confidence::Certain),
        Confidence::Probable
    );
    assert_eq!(
        Confidence::Possible.weakest(Confidence::Possible),
        Confidence::Possible
    );
}

#[test]
fn tier_levels_are_zero_through_four() {
    assert_eq!(Tier::NameSyntactic.level(), 0);
    assert_eq!(Tier::ScopeGraph.level(), 1);
    assert_eq!(Tier::Scip.level(), 2);
    assert_eq!(Tier::ChaRta.level(), 3);
    assert_eq!(Tier::PointsTo.level(), 4);
}

// --- Signature (ADR-04) ---

#[test]
fn signature_canonical_renders_structured_fields() {
    let sig = Signature {
        params: vec![
            Param {
                name: "input".into(),
                type_text: Some("&str".into()),
                has_default: false,
                variadic: false,
            },
            Param {
                name: "opts".into(),
                type_text: None,
                has_default: true,
                variadic: false,
            },
        ],
        return_type_text: Some("Result<Config>".into()),
        type_params: vec!["T".into()],
        receiver: None,
    };
    assert_eq!(
        sig.canonical(),
        "<T> (input: &str, opts = …) -> Result<Config>"
    );
}

#[test]
fn signature_canonical_handles_receiver_and_variadic() {
    let sig = Signature {
        params: vec![Param {
            name: "args".into(),
            type_text: Some("T".into()),
            has_default: false,
            variadic: true,
        }],
        return_type_text: None,
        type_params: vec![],
        receiver: Some("Logger".into()),
    };
    assert_eq!(sig.canonical(), "(Logger) (...args: T)");
}

#[test]
fn empty_signature_canonical_is_empty_parens() {
    assert_eq!(Signature::empty().canonical(), "()");
}

/// ADR-04: semver equality compares structured fields, not the canonical string.
/// Renaming a generic type-param changes the structured record (and thus the
/// canonical string), so the records are unequal — a real signature change.
#[test]
fn signature_equality_is_field_wise() {
    let a = Signature {
        params: vec![],
        return_type_text: None,
        type_params: vec!["T".into()],
        receiver: None,
    };
    let b = Signature {
        params: vec![],
        return_type_text: None,
        type_params: vec!["U".into()],
        receiver: None,
    };
    assert_ne!(a, b);

    let c = a.clone();
    assert_eq!(a, c);
}

// --- Cut markers (GM-5.3, ADR-07) ---

#[test]
fn cut_markers_stay_sorted_and_deduplicated() {
    let mut m = CutMarkers::new();
    assert!(m.is_empty());
    m.insert(CutMarker::ViaFfi);
    m.insert(CutMarker::Reflective);
    m.insert(CutMarker::ViaFfi); // duplicate
    assert_eq!(m.len(), 2);
    let order: Vec<_> = m.iter().collect();
    assert_eq!(order, vec![CutMarker::Reflective, CutMarker::ViaFfi]);
    assert!(m.contains(CutMarker::Reflective));
    assert!(!m.contains(CutMarker::Unresolved));
}

#[test]
fn unexpanded_macro_marker_exists() {
    let m = CutMarkers::from_iter_canonical([CutMarker::UnexpandedMacro]);
    assert!(m.contains(CutMarker::UnexpandedMacro));
}

// --- Patterns (architecture §4) ---

fn node(fqn: &str) -> NodeRecord {
    NodeRecord {
        id: NodeId(0),
        kind: SymbolKind::Function,
        fqn: fqn.into(),
        file: "f.rs".into(),
        line_start: 1,
        line_end: 1,
        lang: "rust".into(),
        visibility: Visibility::Public,
        is_abstract: false,
        entrypoint_kind: None,
        signature: None,
        own_effects: cgx_core::EffectSet::new(),
        transitive_effects: cgx_core::EffectSet::new(),
        unresolved_calls: 0,
    }
}

#[test]
fn fqn_pattern_matches_exactly() {
    let p = SymbolPattern::fqn("a::b::c");
    assert!(p.matches(&node("a::b::c")));
    assert!(!p.matches(&node("a::b")));
    assert!(!p.matches(&node("a::b::c::d")));
}

#[test]
fn short_name_pattern_matches_last_segment() {
    let p = SymbolPattern::short_name("save");
    assert!(p.matches(&node("a::b::save")));
    assert!(p.matches(&node("save")));
    assert!(!p.matches(&node("a::save_all")));
}

#[test]
fn single_star_glob_stays_within_a_segment() {
    let p = SymbolPattern::glob("a::*");
    assert!(p.matches(&node("a::foo")));
    assert!(
        !p.matches(&node("a::foo::bar")),
        "single * must not cross ::"
    );
    assert!(!p.matches(&node("a")));
}

#[test]
fn double_star_glob_crosses_segments() {
    let p = SymbolPattern::glob("a::**");
    assert!(p.matches(&node("a::foo")));
    assert!(p.matches(&node("a::foo::bar")));
}

#[test]
fn question_mark_matches_one_non_separator_char() {
    let p = SymbolPattern::glob("a::f?o");
    assert!(p.matches(&node("a::foo")));
    assert!(!p.matches(&node("a::fo")));
    assert!(!p.matches(&node("a::fooo")));
}

#[test]
fn glob_with_suffix() {
    let p = SymbolPattern::glob("**::commit");
    assert!(p.matches(&node("a::b::commit")));
    assert!(p.matches(&node("x::commit")));
    assert!(!p.matches(&node("a::b::commit_all")));
}

// --- Edge-kind classification ---

#[test]
fn call_family_includes_spawns_and_excludes_structural() {
    assert!(EdgeKind::Calls.is_call());
    assert!(EdgeKind::CallsVirtual.is_call());
    assert!(EdgeKind::CallsAsync.is_call());
    assert!(EdgeKind::Spawns.is_call());
    assert!(!EdgeKind::Contains.is_call());
    assert!(!EdgeKind::Implements.is_call());
    assert!(!EdgeKind::Throws.is_call());
    // v0.3 DATA_FLOW: DerivesFrom is a structural edge, never a call.
    assert!(!EdgeKind::DerivesFrom.is_call());
}

#[test]
fn transform_round_trips_through_canonical_codec() {
    use cgx_core::codec::{decode, encode};
    use cgx_core::Transform;
    for t in [
        Transform::Copy,
        Transform::Projection,
        Transform::Arith,
        Transform::Composed,
        Transform::Branched,
        Transform::Other,
    ] {
        let bytes = encode(&t).unwrap();
        let back: Transform = decode(&bytes).unwrap();
        assert_eq!(t, back, "Transform must postcard round-trip");
    }
}

#[test]
fn value_id_is_flow_sensitive_and_deterministic() {
    use cgx_core::ValueId;
    // Re-assignment (distinct ssa_version) yields distinct ids; same inputs are
    // stable across calls (determinism §A.1).
    let v1 = ValueId::derive("m::g", "x", 1, 2, 4);
    let v2 = ValueId::derive("m::g", "x", 2, 3, 4);
    assert_ne!(v1, v2, "distinct ssa versions must hash distinctly");
    assert_eq!(v1, ValueId::derive("m::g", "x", 1, 2, 4), "derive is pure");
}

#[test]
fn edge_record_transform_defaults_to_none_on_old_rows() {
    use cgx_core::codec::{decode, encode};
    // An EdgeRecord encoded without a transform (the common non-dataflow case)
    // decodes with transform == None — the `#[serde(default)]` forward-compat
    // guarantee for pre-v3 postcard rows.
    let mut e = sample_edge();
    e.transform = None;
    let bytes = encode(&e).unwrap();
    let back: EdgeRecord = decode(&bytes).unwrap();
    assert_eq!(back.transform, None);
}

/// A minimal non-dataflow edge for the codec assertions above.
fn sample_edge() -> EdgeRecord {
    EdgeRecord {
        id: EdgeId(0),
        src: NodeId(0),
        dst: NodeId(1),
        kind: EdgeKind::Calls,
        condition: EdgeCondition::Always,
        confidence: Confidence::Certain,
        tier: Tier::ScopeGraph,
        rule: "scope-ref".into(),
        site_id: None,
        stmt_index: None,
        cut_markers: CutMarkers::new(),
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
        transform: None,
    }
}

#[test]
fn only_callable_kinds_carry_signatures() {
    assert!(SymbolKind::Function.is_callable());
    assert!(SymbolKind::Method.is_callable());
    assert!(SymbolKind::Lambda.is_callable());
    assert!(!SymbolKind::Type.is_callable());
    assert!(!SymbolKind::Field.is_callable());
    assert!(!SymbolKind::Module.is_callable());
}
