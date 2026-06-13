//! Tests that DoctorReport fields are correctly computed on known small graphs.

mod common;

use cgx_core::{Confidence, CutMarker};
use cgx_doctor::report::{AnomalyKind, TrustLevel};
use cgx_doctor::{render_json, render_text};
use cgx_store::LinkedGraph;

// ---------------------------------------------------------------------------
// Happy path: a small, well-formed graph
// ---------------------------------------------------------------------------

#[test]
fn report_node_and_edge_counts_correct() {
    let graph = common::GraphBuilder::new()
        .func("crate::a")
        .func("crate::b")
        .func("crate::c")
        .calls("crate::a", "crate::b")
        .calls("crate::b", "crate::c")
        .build();

    let rep = cgx_doctor::report::compute(&graph);

    assert_eq!(rep.node_count, 3, "node count");
    assert_eq!(rep.edge_count, 2, "edge count");
    assert_eq!(rep.call_edge_count, 2, "call edge count");
}

#[test]
fn confidence_distribution_all_certain() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b") // certain by default
        .build();

    let rep = cgx_doctor::report::compute(&graph);

    assert_eq!(rep.confidence.certain, 1);
    assert_eq!(rep.confidence.probable, 0);
    assert_eq!(rep.confidence.possible, 0);
    assert_eq!(rep.trust, TrustLevel::High);
    assert!(rep.anomalies.is_empty(), "no anomalies on a clean graph");
}

#[test]
fn confidence_distribution_mixed() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .calls("a", "b") // certain
        .calls_conf("a", "c", Confidence::Probable) // probable
        .build();

    let rep = cgx_doctor::report::compute(&graph);

    assert_eq!(rep.confidence.certain, 1);
    assert_eq!(rep.confidence.probable, 1);
    assert_eq!(rep.confidence.possible, 0);
    assert_eq!(rep.confidence.total(), 2);
}

#[test]
fn all_possible_confidence_anomaly() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls_conf("a", "b", Confidence::Possible)
        .build();

    let rep = cgx_doctor::report::compute(&graph);

    assert!(
        rep.anomalies.contains(&AnomalyKind::AllPossibleConfidence),
        "should flag all-possible when no certain/probable edges: {:?}",
        rep.anomalies
    );
    assert_eq!(rep.trust, TrustLevel::Moderate);
}

// ---------------------------------------------------------------------------
// Unresolved-reference rate
// ---------------------------------------------------------------------------

#[test]
fn unresolved_rate_zero_when_no_unresolved_markers() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b")
        .build();

    let rep = cgx_doctor::report::compute(&graph);

    assert_eq!(rep.unresolved_count, 0);
    assert_eq!(rep.unresolved_rate, Some(0.0));
}

#[test]
fn unresolved_rate_computed_correctly() {
    // 1 resolved call + 1 edge with Unresolved marker = 50% rate.
    // Both are call-family edges so total_refs = 2.
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .calls("a", "b") // resolved
        .unresolved_call("a", "c") // unresolved marker
        .build();

    let rep = cgx_doctor::report::compute(&graph);

    assert_eq!(rep.unresolved_count, 1, "unresolved count");
    assert_eq!(rep.total_refs, 2, "total refs = call_edge_count");
    // rate = 0.5 exactly; threshold is > 0.50 (strict), so NOT flagged.
    assert!(
        !rep.anomalies.contains(&AnomalyKind::HighUnresolvedRate),
        "50% exactly should NOT flag HighUnresolvedRate (threshold is strict >50%)"
    );
    let rate = rep.unresolved_rate.unwrap();
    assert!((rate - 0.5).abs() < 1e-9, "rate should be exactly 0.5");
}

#[test]
fn high_unresolved_rate_anomaly_above_threshold() {
    // 2 unresolved, 1 resolved → 67% unresolved.
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .func("d")
        .calls("a", "b")
        .unresolved_call("a", "c")
        .unresolved_call("a", "d")
        .build();

    let rep = cgx_doctor::report::compute(&graph);

    assert!(
        rep.anomalies.contains(&AnomalyKind::HighUnresolvedRate),
        "67% unresolved should flag HighUnresolvedRate"
    );
}

#[test]
fn unresolved_rate_none_when_no_refs() {
    let graph = LinkedGraph::default();
    let rep = cgx_doctor::report::compute(&graph);
    assert_eq!(rep.unresolved_rate, None, "no refs → rate is None");
}

// ---------------------------------------------------------------------------
// Cut-marker counts
// ---------------------------------------------------------------------------

#[test]
fn cut_marker_counts_per_kind() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .func("d")
        .calls_with_marker("a", "b", CutMarker::UnexpandedMacro)
        .calls_with_marker("a", "c", CutMarker::ViaFfi)
        .calls_with_marker("a", "d", CutMarker::Dynamic)
        .build();

    let rep = cgx_doctor::report::compute(&graph);

    assert_eq!(
        rep.cut_markers.unexpanded_macro, 1,
        "unexpanded-macro count"
    );
    assert_eq!(rep.cut_markers.via_ffi, 1, "ffi count");
    assert_eq!(rep.cut_markers.dynamic, 1, "dynamic count");
    assert_eq!(rep.cut_markers.unresolved, 0, "unresolved count");
}

// ---------------------------------------------------------------------------
// Anomaly: empty graph
// ---------------------------------------------------------------------------

#[test]
fn zero_edge_and_zero_node_anomaly() {
    let graph = LinkedGraph::default();
    let rep = cgx_doctor::report::compute(&graph);

    assert!(
        rep.anomalies.contains(&AnomalyKind::ZeroEdges),
        "empty graph should flag ZeroEdges"
    );
    assert!(
        rep.anomalies.contains(&AnomalyKind::ZeroNodes),
        "empty graph should flag ZeroNodes"
    );
    assert_eq!(rep.trust, TrustLevel::Low, "empty graph is Low trust");
}

#[test]
fn zero_edges_but_has_nodes_is_low() {
    let graph = common::GraphBuilder::new().func("a").build();

    let rep = cgx_doctor::report::compute(&graph);

    assert!(rep.anomalies.contains(&AnomalyKind::ZeroEdges));
    assert_eq!(rep.trust, TrustLevel::Low);
}

// ---------------------------------------------------------------------------
// Trust derivation
// ---------------------------------------------------------------------------

#[test]
fn high_trust_on_clean_graph() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b")
        .build();

    let rep = cgx_doctor::report::compute(&graph);
    assert_eq!(rep.trust, TrustLevel::High);
}

#[test]
fn two_anomalies_produce_low_trust() {
    // All-possible + HighUnresolvedRate → two anomalies → Low.
    // Use 3 calls, 2 unresolved → 67% rate → HighUnresolvedRate triggered.
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .func("c")
        .func("d")
        // All edges are Possible (triggers AllPossibleConfidence)
        .calls_conf("a", "b", Confidence::Possible)
        .unresolved_call("a", "c") // unresolved + possible
        .unresolved_call("a", "d") // unresolved + possible
        .build();

    let rep = cgx_doctor::report::compute(&graph);
    // Both anomalies should be set.
    assert!(
        rep.anomalies.contains(&AnomalyKind::AllPossibleConfidence),
        "AllPossibleConfidence must be set: {:?}",
        rep.anomalies
    );
    assert!(
        rep.anomalies.contains(&AnomalyKind::HighUnresolvedRate),
        "HighUnresolvedRate must be set: {:?}",
        rep.anomalies
    );
    // Two anomalies → Low trust.
    assert_eq!(rep.trust, TrustLevel::Low);
}

// ---------------------------------------------------------------------------
// patch_index_stats
// ---------------------------------------------------------------------------

#[test]
fn patch_index_stats_populates_file_fields() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b")
        .build();

    let mut rep = cgx_doctor::report::compute(&graph);
    cgx_doctor::report::patch_index_stats(&mut rep, 10, 2);

    assert_eq!(rep.total_files, Some(10));
    assert_eq!(rep.unsupported_files, 2);
    let share = rep.unsupported_share.unwrap();
    assert!((share - 0.2).abs() < 1e-9, "share should be 0.2");
}

#[test]
fn patch_index_stats_flags_high_unsupported() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b")
        .build();

    let mut rep = cgx_doctor::report::compute(&graph);
    // 8/10 unsupported = 80% → HighUnsupportedShare
    cgx_doctor::report::patch_index_stats(&mut rep, 10, 8);

    assert!(
        rep.anomalies.contains(&AnomalyKind::HighUnsupportedShare),
        "80% unsupported should flag HighUnsupportedShare"
    );
    // trust should now be at least Moderate (one anomaly).
    assert!(
        rep.trust == TrustLevel::Moderate || rep.trust == TrustLevel::Low,
        "trust should not be High with HighUnsupportedShare"
    );
}

// ---------------------------------------------------------------------------
// Renderer: determinism
// ---------------------------------------------------------------------------

#[test]
fn render_text_is_deterministic() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b")
        .build();

    let rep = cgx_doctor::report::compute(&graph);
    let s1 = render_text(&rep);
    let s2 = render_text(&rep);
    assert_eq!(s1, s2, "render_text must be byte-identical across calls");
}

#[test]
fn render_json_is_deterministic() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b")
        .build();

    let rep = cgx_doctor::report::compute(&graph);
    let j1 = render_json(&rep).unwrap();
    let j2 = render_json(&rep).unwrap();
    assert_eq!(j1, j2, "render_json must be byte-identical across calls");
}

#[test]
fn render_text_contains_expected_sections() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b")
        .build();

    let rep = cgx_doctor::report::compute(&graph);
    let text = render_text(&rep);

    assert!(text.contains("trust:"), "text must contain trust line");
    assert!(text.contains("nodes:"), "text must contain nodes line");
    assert!(text.contains("edges:"), "text must contain edges line");
    assert!(
        text.contains("confidence distribution"),
        "text must contain confidence section"
    );
    assert!(
        text.contains("unresolved references"),
        "text must contain unresolved section"
    );
    assert!(
        text.contains("cut-marker inventory"),
        "text must contain cut-marker section"
    );
    assert!(
        text.contains("anomalies:"),
        "text must contain anomalies section"
    );
}

#[test]
fn render_json_round_trips() {
    let graph = common::GraphBuilder::new()
        .func("a")
        .func("b")
        .calls("a", "b")
        .build();

    let rep = cgx_doctor::report::compute(&graph);
    let json = render_json(&rep).unwrap();
    let rep2: cgx_doctor::DoctorReport = serde_json::from_str(&json).unwrap();
    // After round-tripping, re-rendering should produce the same JSON.
    let json2 = render_json(&rep2).unwrap();
    assert_eq!(json, json2, "JSON round-trip must be byte-identical");
}
