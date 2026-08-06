//! The [`DoctorReport`] type and its computation from a [`cgx_store::LinkedGraph`].
//!
//! All fields are plain scalar/enum values so the struct is serialisable via
//! `serde` for the JSON renderer and clone-able for tests.

use cgx_core::{Confidence, CutMarker};
use cgx_store::LinkedGraph;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// How much the user should trust this index, derived from the quality metrics.
///
/// Designed to be printed directly to the user as a headline trust signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// The index looks structurally sound: node/edge counts are reasonable,
    /// confidence distribution is plausible, and unresolved-reference rate is low.
    High,
    /// Some quality indicators are degraded (e.g., high unresolved rate, many cut
    /// markers) but the index is not empty and can still answer queries.
    Moderate,
    /// The index has structural problems (zero edges, all-`possible` confidence,
    /// abnormally high anomaly count) that mean query results should be treated
    /// with significant scepticism.
    Low,
}

/// Edge counts broken down by confidence tier (GM-5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ConfidenceBreakdown {
    /// Edges with [`Confidence::Certain`].
    pub certain: usize,
    /// Edges with [`Confidence::Probable`].
    pub probable: usize,
    /// Edges with [`Confidence::Possible`].
    pub possible: usize,
}

impl ConfidenceBreakdown {
    /// Total call-family edges (sum of all tiers).
    pub fn total(&self) -> usize {
        self.certain + self.probable + self.possible
    }
}

/// How many edges carry a specific [`CutMarker`] (GM-5.3, ADR-07).
///
/// Markers are known blind spots in the index. Their presence is not an error —
/// the resolver emits them honestly — but high counts indicate categories of call
/// sites whose resolution is structurally limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct CutMarkerCount {
    /// [`CutMarker::Unresolved`]: refs that resolved to no target.
    pub unresolved: usize,
    /// [`CutMarker::UnexpandedMacro`]: call sites inside unexpanded proc macros.
    pub unexpanded_macro: usize,
    /// [`CutMarker::ViaFfi`]: edges crossing a foreign-function boundary.
    pub via_ffi: usize,
    /// [`CutMarker::Dynamic`]: dynamic/plugin dispatch.
    pub dynamic: usize,
    /// [`CutMarker::Reflective`]: string-computed / reflective dispatch.
    pub reflective: usize,
    /// [`CutMarker::ViaDi`]: dependency-injection container dispatch.
    pub via_di: usize,
}

impl CutMarkerCount {
    /// Total edges carrying any cut marker (markers are per-edge, so an edge with
    /// two markers is counted once in each category but contributes 1 to this sum).
    /// Computed as `edges` carrying at least one marker, not the sum of
    /// per-marker counts.
    pub fn any_cut_total(&self) -> usize {
        self.unresolved
            + self.unexpanded_macro
            + self.via_ffi
            + self.dynamic
            + self.reflective
            + self.via_di
    }
}

/// An anomaly flag: a condition that suggests the index may be incomplete or
/// low-quality in a way worth surfacing explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    /// The graph contains zero edges. A non-empty source corpus should always
    /// produce at least some call edges; this usually means extraction failed or
    /// all files were skipped as unsupported.
    ZeroEdges,
    /// The graph contains zero nodes.
    ZeroNodes,
    /// Every call-family edge is `possible` — no `probable` or `certain` edges at
    /// all. Possible with a purely Tier-0 index over a language with very dynamic
    /// dispatch, but unusual; suggests a resolution bug or missing adapter.
    AllPossibleConfidence,
    /// The unresolved-reference rate exceeds 50 %. More than half of all call
    /// references produced no edge, typically indicating that imports are missing
    /// from the index or the codebase references many unindexed external symbols.
    HighUnresolvedRate,
    /// The unsupported-file share exceeds 50 %. Most source files are not covered
    /// by any registered adapter.
    HighUnsupportedShare,
    /// The `possible`-confidence share of call edges exceeds 85 %. Even with a
    /// nonzero sliver of `certain`/`probable` edges, an index this dominated by
    /// name-guess resolution should not read as sound (see `AllPossibleConfidence`
    /// for the 100 % case, which this generalises to majorities-of-guesses).
    HighPossibleShare,
}

/// The full index-quality report for one stored graph (WP-12).
///
/// Compute it with [`crate::report`]; render with [`crate::render_text`] or
/// [`crate::render_json`].
///
/// `PartialEq` but not `Eq`: the `f64` rate fields violate `Eq` (NaN != NaN).
/// Use exact field comparisons in tests or compare via `render_json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorReport {
    // --- Counts ---------------------------------------------------------
    /// Total symbol nodes in the graph.
    pub node_count: usize,
    /// Total edges in the graph (all kinds).
    pub edge_count: usize,
    /// Call-family edges only (kinds for which `EdgeKind::is_call()` is true).
    pub call_edge_count: usize,

    // --- Confidence distribution ----------------------------------------
    /// Call-family edges broken down by confidence tier.
    pub confidence: ConfidenceBreakdown,

    // --- Reference resolution quality -----------------------------------
    /// Total raw references that were emitted by the frontend
    /// (resolved edges + unresolved refs). This is `call_edge_count + unresolved_count`
    /// at the resolution level, giving the denominator for the unresolved rate.
    ///
    /// NOTE: after `into_linked()` the per-ref unresolved list is lost; we
    /// reconstruct this as `call_edge_count + unresolved_edge_count` where
    /// `unresolved_edge_count` is edges that carry `CutMarker::Unresolved`.
    pub total_refs: usize,
    /// Edges carrying [`CutMarker::Unresolved`] — refs that produced no resolvable
    /// target. These are real call sites in the source, just unresolvable by the
    /// syntactic stack.
    pub unresolved_count: usize,
    /// Unresolved refs as a fraction of total refs (0.0 = perfect, 1.0 = all
    /// unresolved). `None` if `total_refs == 0`.
    pub unresolved_rate: Option<f64>,

    // --- Unsupported files ----------------------------------------------
    /// Files that were skipped because no registered adapter claimed them.
    pub unsupported_files: usize,
    /// Total files seen (supported + unsupported). `None` if unavailable.
    pub total_files: Option<usize>,
    /// Unsupported share (0.0–1.0). `None` if `total_files` is `None` or zero.
    pub unsupported_share: Option<f64>,

    // --- Cut-marker inventory (known blind spots) -----------------------
    /// Per-marker counts across all edges in the graph.
    pub cut_markers: CutMarkerCount,

    // --- Anomalies ------------------------------------------------------
    /// Structural anomalies worth surfacing to the user.
    pub anomalies: Vec<AnomalyKind>,

    // --- Trust headline -------------------------------------------------
    /// Derived overall trust level for this index.
    pub trust: TrustLevel,
}

// ---------------------------------------------------------------------------
// Computation
// ---------------------------------------------------------------------------

/// Compute a [`DoctorReport`] from an already-loaded [`LinkedGraph`].
///
/// Called by [`crate::report`] after reading the graph from the store; exposed
/// directly here so tests can inject a `LinkedGraph` without going through a
/// store.
pub fn compute(graph: &LinkedGraph) -> DoctorReport {
    let node_count = graph.nodes.len();
    let edge_count = graph.edges.len();

    // --- Confidence breakdown + call count ---
    let mut confidence = ConfidenceBreakdown::default();
    let mut call_edge_count = 0usize;
    let mut cut_markers = CutMarkerCount::default();

    for edge in &graph.edges {
        if edge.kind.is_call() {
            call_edge_count += 1;
            match edge.confidence {
                Confidence::Certain => confidence.certain += 1,
                Confidence::Probable => confidence.probable += 1,
                Confidence::Possible => confidence.possible += 1,
            }
        }
        // Count cut markers on every edge (structural edges can also carry
        // cut markers, e.g. an unresolved `Imports` edge).
        for marker in edge.cut_markers.iter() {
            match marker {
                CutMarker::Unresolved => cut_markers.unresolved += 1,
                CutMarker::UnexpandedMacro => cut_markers.unexpanded_macro += 1,
                CutMarker::ViaFfi => cut_markers.via_ffi += 1,
                CutMarker::Dynamic => cut_markers.dynamic += 1,
                CutMarker::Reflective => cut_markers.reflective += 1,
                CutMarker::ViaDi => cut_markers.via_di += 1,
                // Dataflow-only cut markers (v0.3 DATA_FLOW): not part of the
                // call-resolution quality breakdown this report summarizes.
                CutMarker::OpaqueCall
                | CutMarker::TruncatedAccessPath
                | CutMarker::SummaryBudgetExceeded => {}
            }
        }
    }

    // --- Unresolved reference rate ---
    // `unresolved_count` = call-family edges carrying `CutMarker::Unresolved`.
    // These edges ARE in the graph (Phase-1 honesty: unresolvable refs produce a
    // stub edge with the marker rather than being silently dropped). They are a
    // strict subset of `call_edge_count`.
    //
    // `total_refs = call_edge_count` (all call-family edges, both resolved and
    // unresolved-marked). Rate = unresolved_count / total_refs.
    //
    // Note: `ResolvedGraph.unresolved` (raw unresolved refs that produced *no*
    // edge at all) is dropped in `into_linked()` and therefore not visible here.
    // If the resolver changes to persist those as zero-dst sentinel edges, the
    // formula below still holds.
    let unresolved_count = cut_markers.unresolved;
    let total_refs = call_edge_count;
    let unresolved_rate = if total_refs > 0 {
        Some(unresolved_count as f64 / total_refs as f64)
    } else {
        None
    };

    // --- Unsupported files (not tracked in LinkedGraph; set to 0/None) ---
    // The LinkedGraph does not carry pipeline counters (those live in IndexStats).
    // The doctor report can be computed from just the stored graph; callers who
    // have the IndexStats can patch these fields after construction.
    let unsupported_files = 0;
    let total_files: Option<usize> = None;
    let unsupported_share: Option<f64> = None;

    // --- Possible-confidence share ---
    // Fraction of call edges resolved by name-guess only. `None` if there are no
    // call edges (mirrors `unresolved_rate`'s `None`-on-empty convention).
    let possible_share = if call_edge_count > 0 {
        Some(confidence.possible as f64 / call_edge_count as f64)
    } else {
        None
    };

    // --- Anomaly detection ---
    let mut anomalies = Vec::new();

    if node_count == 0 {
        anomalies.push(AnomalyKind::ZeroNodes);
    }
    if edge_count == 0 {
        anomalies.push(AnomalyKind::ZeroEdges);
    }
    // All-possible: only flag when there are call edges at all.
    if call_edge_count > 0 && confidence.certain == 0 && confidence.probable == 0 {
        anomalies.push(AnomalyKind::AllPossibleConfidence);
    }
    if let Some(rate) = unresolved_rate {
        if rate > 0.50 {
            anomalies.push(AnomalyKind::HighUnresolvedRate);
        }
    }
    if let Some(share) = possible_share {
        if share > 0.85 {
            anomalies.push(AnomalyKind::HighPossibleShare);
        }
    }

    // --- Trust level ---
    let trust = derive_trust(
        node_count,
        edge_count,
        call_edge_count,
        &confidence,
        unresolved_rate,
        possible_share,
        &anomalies,
    );

    DoctorReport {
        node_count,
        edge_count,
        call_edge_count,
        confidence,
        total_refs,
        unresolved_count,
        unresolved_rate,
        unsupported_files,
        total_files,
        unsupported_share,
        cut_markers,
        anomalies,
        trust,
    }
}

/// Patch a [`DoctorReport`] with pipeline counters from an
/// [`cgx_index::IndexStats`]-equivalent source.
///
/// [`compute`] cannot access `IndexStats` directly (that would create a dependency
/// on `cgx-index`, which depends on `cgx-store` + many other crates). Instead,
/// callers who have both pieces call [`patch_index_stats`] after [`compute`].
///
/// ```rust,ignore
/// let mut rep = cgx_doctor::report::compute(&graph);
/// cgx_doctor::report::patch_index_stats(
///     &mut rep,
///     stats.blobs_indexed + stats.blobs_unsupported,
///     stats.blobs_unsupported,
/// );
/// ```
pub fn patch_index_stats(rep: &mut DoctorReport, total_files: usize, unsupported_files: usize) {
    rep.total_files = Some(total_files);
    rep.unsupported_files = unsupported_files;
    rep.unsupported_share = if total_files > 0 {
        Some(unsupported_files as f64 / total_files as f64)
    } else {
        None
    };
    // Re-check for HighUnsupportedShare now that we have file data.
    rep.anomalies
        .retain(|a| *a != AnomalyKind::HighUnsupportedShare);
    if let Some(share) = rep.unsupported_share {
        if share > 0.50 {
            rep.anomalies.push(AnomalyKind::HighUnsupportedShare);
        }
    }
    // Re-derive trust with updated anomaly set.
    let possible_share = if rep.call_edge_count > 0 {
        Some(rep.confidence.possible as f64 / rep.call_edge_count as f64)
    } else {
        None
    };
    rep.trust = derive_trust(
        rep.node_count,
        rep.edge_count,
        rep.call_edge_count,
        &rep.confidence,
        rep.unresolved_rate,
        possible_share,
        &rep.anomalies,
    );
}

/// Derive the trust headline from computed metrics.
fn derive_trust(
    node_count: usize,
    edge_count: usize,
    call_edge_count: usize,
    confidence: &ConfidenceBreakdown,
    unresolved_rate: Option<f64>,
    possible_share: Option<f64>,
    anomalies: &[AnomalyKind],
) -> TrustLevel {
    // Hard low signals: no graph at all, or multiple structural anomalies.
    if node_count == 0 || edge_count == 0 {
        return TrustLevel::Low;
    }
    if anomalies.len() >= 2 {
        return TrustLevel::Low;
    }
    // Any single structural anomaly or high unresolved rate → moderate.
    if !anomalies.is_empty() {
        return TrustLevel::Moderate;
    }
    if let Some(rate) = unresolved_rate {
        if rate > 0.30 {
            return TrustLevel::Moderate;
        }
    }
    // All-possible with any call edges is moderate even without the anomaly flag
    // (the anomaly fires at >50%; 30–50% is still worth noting as moderate).
    if call_edge_count > 0 && confidence.certain == 0 && confidence.probable == 0 {
        return TrustLevel::Moderate;
    }
    // Majority-possible is moderate even short of the HighPossibleShare anomaly
    // (that fires at >85%; 60–85% guess-dominated resolution is still not "sound").
    if let Some(share) = possible_share {
        if share > 0.60 {
            return TrustLevel::Moderate;
        }
    }
    TrustLevel::High
}
