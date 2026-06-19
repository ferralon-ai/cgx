//! Postcard round-trip and canonical-bytes property tests (WP-01 convergence
//! criterion).

use cgx_core::codec::{decode, encode};
use cgx_core::*;
use proptest::prelude::*;

fn sample_node() -> NodeRecord {
    NodeRecord {
        id: NodeId(7),
        kind: SymbolKind::Method,
        fqn: "myapp::db::Connection::commit".into(),
        file: "src/db.rs".into(),
        line_start: 42,
        line_end: 58,
        lang: "rust".into(),
        visibility: Visibility::Public,
        is_abstract: false,
        entrypoint_kind: None,
        signature: Some(Signature {
            params: vec![Param {
                name: "self".into(),
                type_text: Some("&mut Connection".into()),
                has_default: false,
                variadic: false,
            }],
            return_type_text: Some("Result<()>".into()),
            type_params: vec![],
            receiver: Some("Connection".into()),
        }),
        own_effects: cgx_core::EffectSet::from_iter_canonical([
            cgx_core::Effect::IoFile,
            cgx_core::Effect::Blocking,
        ]),
        transitive_effects: cgx_core::EffectSet::new(),
    }
}

fn sample_edge() -> EdgeRecord {
    EdgeRecord {
        id: EdgeId(3),
        src: NodeId(1),
        dst: NodeId(2),
        kind: EdgeKind::CallsVirtual,
        condition: EdgeCondition::Exception,
        confidence: Confidence::Possible,
        tier: Tier::ScopeGraph,
        rule: "import-ref".into(),
        site_id: Some(SiteId::derive("a::b", "src/lib.rs", 10, 4)),
        stmt_index: Some(2),
        cut_markers: CutMarkers::from_iter_canonical([CutMarker::Reflective, CutMarker::ViaFfi]),
        implicit: None,
        candidate_group: Some(5),
        established_by: Some(EstablishedBy::Annotation),
        cfg_condition: Some("feature = \"legacy\"".into()),
        macro_origin: None,
    }
}

#[test]
fn node_round_trips() {
    let n = sample_node();
    let bytes = encode(&n).unwrap();
    let back: NodeRecord = decode(&bytes).unwrap();
    assert_eq!(n, back);
}

#[test]
fn edge_round_trips() {
    let e = sample_edge();
    let bytes = encode(&e).unwrap();
    let back: EdgeRecord = decode(&bytes).unwrap();
    assert_eq!(e, back);
}

#[test]
fn encoding_is_byte_stable_across_calls() {
    let e = sample_edge();
    let a = encode(&e).unwrap();
    let b = encode(&e).unwrap();
    assert_eq!(a, b, "same value must encode to identical bytes");
}

/// Equal values encode to equal bytes even when built independently (no hidden
/// per-instance state leaks into the encoding).
#[test]
fn independently_built_equal_values_encode_identically() {
    let a = encode(&sample_edge()).unwrap();
    let b = encode(&sample_edge()).unwrap();
    assert_eq!(a, b);
}

// --- Property tests ---

prop_compose! {
    fn arb_condition()(i in 0u8..5) -> EdgeCondition {
        EdgeCondition::ALL[i as usize]
    }
}

prop_compose! {
    fn arb_confidence()(i in 0u8..3) -> Confidence {
        Confidence::ALL[i as usize]
    }
}

prop_compose! {
    fn arb_tier()(i in 0u8..5) -> Tier {
        Tier::ALL[i as usize]
    }
}

prop_compose! {
    fn arb_cut_markers()(set in proptest::collection::vec(0u8..6, 0..6)) -> CutMarkers {
        CutMarkers::from_iter_canonical(set.into_iter().map(|i| CutMarker::ALL[i as usize]))
    }
}

prop_compose! {
    fn arb_edge()(
        id in any::<u32>(),
        src in any::<u32>(),
        dst in any::<u32>(),
        kind_i in 0usize..1,
        condition in arb_condition(),
        confidence in arb_confidence(),
        tier in arb_tier(),
        rule in "[a-z-]{1,12}",
        has_site in any::<bool>(),
        stmt in proptest::option::of(any::<u32>()),
        markers in arb_cut_markers(),
        cand in proptest::option::of(any::<u32>()),
    ) -> EdgeRecord {
        let _ = kind_i;
        EdgeRecord {
            id: EdgeId(id),
            src: NodeId(src),
            dst: NodeId(dst),
            kind: EdgeKind::Calls,
            condition,
            confidence,
            tier,
            rule,
            site_id: if has_site { Some(SiteId::derive("c", "f", src, dst)) } else { None },
            stmt_index: stmt,
            cut_markers: markers,
            implicit: None,
            candidate_group: cand,
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
        }
    }
}

proptest! {
    /// Round-trip: decode(encode(v)) == v for arbitrary edges.
    #[test]
    fn prop_edge_round_trips(e in arb_edge()) {
        let bytes = encode(&e).unwrap();
        let back: EdgeRecord = decode(&bytes).unwrap();
        prop_assert_eq!(e, back);
    }

    /// Canonical bytes: encode is a pure function of the value.
    #[test]
    fn prop_encode_is_deterministic(e in arb_edge()) {
        let a = encode(&e).unwrap();
        let b = encode(&e.clone()).unwrap();
        prop_assert_eq!(a, b);
    }

    /// Cut-marker sets are canonicalized: any input permutation of the same
    /// markers yields an equal set and therefore identical bytes.
    #[test]
    fn prop_cut_marker_set_is_permutation_invariant(
        idxs in proptest::collection::vec(0u8..6, 0..8)
    ) {
        let forward = CutMarkers::from_iter_canonical(
            idxs.iter().map(|&i| CutMarker::ALL[i as usize])
        );
        let reversed = CutMarkers::from_iter_canonical(
            idxs.iter().rev().map(|&i| CutMarker::ALL[i as usize])
        );
        prop_assert_eq!(&forward, &reversed);
        prop_assert_eq!(encode(&forward).unwrap(), encode(&reversed).unwrap());
    }
}
