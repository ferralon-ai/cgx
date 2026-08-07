//! Deterministic renderers for [`DoctorReport`].
//!
//! Both renderers are pure functions: same input → same bytes, no I/O,
//! no randomness. This is asserted by the determinism tests.

use crate::report::{AnomalyKind, DoctorReport, TrustLevel};

/// Render a [`DoctorReport`] as a human-readable, deterministic text report.
///
/// The output is stable: identical reports produce byte-identical strings.
pub fn render_text(rep: &DoctorReport) -> String {
    let mut out = String::with_capacity(512);

    // --- Trust headline ---
    let trust_label = match rep.trust {
        TrustLevel::High => "HIGH   — index looks sound",
        TrustLevel::Moderate => "MODERATE — some quality indicators degraded",
        TrustLevel::Low => "LOW    — structural problems detected, results unreliable",
    };
    out.push_str("cgx doctor\n");
    out.push_str(&format!("trust:  {trust_label}\n\n"));

    // --- Counts ---
    out.push_str(&format!("nodes:       {:>8}\n", rep.node_count));
    out.push_str(&format!("edges:       {:>8}  (total)\n", rep.edge_count));
    out.push_str(&format!(
        "call edges:  {:>8}  (call-family)\n",
        rep.call_edge_count
    ));
    out.push('\n');

    // --- Confidence distribution ---
    out.push_str("confidence distribution (call edges):\n");
    if rep.call_edge_count > 0 {
        let total = rep.call_edge_count as f64;
        out.push_str(&format!(
            "  certain:   {:>6}  ({:.1}%)\n",
            rep.confidence.certain,
            100.0 * rep.confidence.certain as f64 / total
        ));
        out.push_str(&format!(
            "  probable:  {:>6}  ({:.1}%)\n",
            rep.confidence.probable,
            100.0 * rep.confidence.probable as f64 / total
        ));
        out.push_str(&format!(
            "  possible:  {:>6}  ({:.1}%)\n",
            rep.confidence.possible,
            100.0 * rep.confidence.possible as f64 / total
        ));
    } else {
        out.push_str("  (no call edges)\n");
    }
    out.push('\n');

    // --- Unresolved reference rate ---
    out.push_str("unresolved references:\n");
    if let Some(rate) = rep.unresolved_rate {
        out.push_str(&format!(
            "  {}/{} refs unresolved  ({:.1}%)\n",
            rep.unresolved_count,
            rep.total_refs,
            100.0 * rate
        ));
    } else {
        out.push_str("  n/a (no references in graph)\n");
    }
    out.push('\n');

    // --- Unsupported files ---
    out.push_str("file coverage:\n");
    if let Some(total) = rep.total_files {
        let share = rep
            .unsupported_share
            .map(|s| format!("{:.1}%", 100.0 * s))
            .unwrap_or_else(|| "n/a".into());
        out.push_str(&format!(
            "  unsupported: {}/{}  ({})\n",
            rep.unsupported_files, total, share
        ));
    } else {
        out.push_str("  (pipeline stats unavailable — run via `cgx index` to populate)\n");
    }
    out.push('\n');

    // --- Cut-marker inventory ---
    out.push_str("cut-marker inventory (known blind spots):\n");
    let cm = &rep.cut_markers;
    // Print all markers, even zeros, for a complete inventory snapshot.
    out.push_str(&format!("  unresolved:       {:>6}\n", cm.unresolved));
    out.push_str(&format!("  unexpanded-macro: {:>6}\n", cm.unexpanded_macro));
    out.push_str(&format!("  via-ffi:          {:>6}\n", cm.via_ffi));
    out.push_str(&format!("  dynamic:          {:>6}\n", cm.dynamic));
    out.push_str(&format!("  reflective:       {:>6}\n", cm.reflective));
    out.push_str(&format!("  via-di:           {:>6}\n", cm.via_di));
    out.push('\n');

    // --- Anomalies ---
    if rep.anomalies.is_empty() {
        out.push_str("anomalies:  none\n");
    } else {
        out.push_str("anomalies:\n");
        // Sort for determinism (anomalies Vec is already in insertion order, but
        // sort by Debug repr to guarantee stable output regardless of collection order).
        let mut sorted = rep.anomalies.clone();
        sorted.sort_by_key(|a| format!("{a:?}"));
        for anomaly in &sorted {
            out.push_str(&format!("  ! {}\n", anomaly_message(anomaly)));
        }
    }

    out
}

/// Render a [`DoctorReport`] as a deterministic, compact JSON string.
///
/// Serialised with `serde_json` (sorted fields via struct field order; no
/// pretty-printing by default so the output is a single line).
///
/// # Errors
///
/// Returns a [`serde_json::Error`] if serialisation fails (in practice
/// unreachable given the types involved).
pub fn render_json(rep: &DoctorReport) -> Result<String, serde_json::Error> {
    serde_json::to_string(rep)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn anomaly_message(kind: &AnomalyKind) -> &'static str {
    match kind {
        AnomalyKind::ZeroEdges => {
            "zero edges — extraction may have failed or all files were skipped"
        }
        AnomalyKind::ZeroNodes => "zero nodes — graph is empty; index did not produce any symbols",
        AnomalyKind::AllPossibleConfidence => {
            "all call edges are `possible` — no probable/certain resolution; check adapter coverage"
        }
        AnomalyKind::HighUnresolvedRate => {
            "unresolved-reference rate >50% — many call sites have no resolvable target; \
             check that dependency sources are indexed"
        }
        AnomalyKind::HighUnsupportedShare => {
            "unsupported-file share >50% — most source files were skipped; \
             check adapter registration"
        }
        AnomalyKind::HighPossibleShare => {
            "possible-confidence share >85% of call edges — resolution is dominated \
             by name guesses even with some certain/probable edges present"
        }
    }
}
