//! Confidence labels and the resolution ladder (GM-5).
//!
//! Confidence is a deterministic label derived from the resolution method, not a
//! probabilistic score. It correlates with the resolution [`Tier`] that produced
//! the edge.

use serde::{Deserialize, Serialize};

/// How sure `cgx` is of an edge's target (GM-5.1).
///
/// Discriminants ascend with certainty so the derived `Ord` answers "is this at
/// least `probable`?" via `>=`, and so that "the weakest confidence of any
/// contributing fact" (GM-1.3 node confidence) is a plain `min`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Confidence {
    /// Over-approximated; candidate set is large or unverified (name match with
    /// multiple candidates, `dyn Trait` candidate set, duck-typed name match).
    Possible = 0,
    /// Resolved heuristically with a small, credible candidate set (unique name
    /// match in scope; CHA/RTA with few overrides; SCIP narrowed by type).
    Probable = 1,
    /// Resolved by direct static binding; no ambiguity remains (non-virtual call,
    /// monomorphized generic, SCIP def/ref with exactly one target).
    Certain = 2,
}

impl Confidence {
    /// All three values, weakest first.
    pub const ALL: [Confidence; 3] = [
        Confidence::Possible,
        Confidence::Probable,
        Confidence::Certain,
    ];

    /// The weaker of two confidences (GM-1.3: a node carries the weakest
    /// confidence of any fact contributing to it).
    #[inline]
    pub fn weakest(self, other: Confidence) -> Confidence {
        self.min(other)
    }
}

/// The resolution tier that produced an edge (GM-5.2). Recorded in provenance.
///
/// The ladder is strictly narrowing; each tier emits a confidence band. Phase 1
/// ships tiers 0–1; tiers 2–4 are reserved so the schema does not change when
/// SCIP/CHA/points-to enrichment lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Tier {
    /// Tier 0 — name/syntactic. Callee name + arity match. Band: `possible`.
    NameSyntactic = 0,
    /// Tier 1 — scope-graph resolution. Band: `probable` (resolved) / `possible`
    /// (ambiguous).
    ScopeGraph = 1,
    /// Tier 2 — SCIP-enriched. Band: `probable`..`certain`. Reserved (Phase 3).
    Scip = 2,
    /// Tier 3 — CHA/RTA over the typed graph. Band: `probable`. Reserved.
    ChaRta = 3,
    /// Tier 4 — points-to. Band: `certain` (monomorphic) / `probable` (aliased).
    /// Reserved.
    PointsTo = 4,
}

impl Tier {
    pub const ALL: [Tier; 5] = [
        Tier::NameSyntactic,
        Tier::ScopeGraph,
        Tier::Scip,
        Tier::ChaRta,
        Tier::PointsTo,
    ];

    /// Numeric tier (0–4) as recorded in the provenance `tier` field (GM-6.1).
    #[inline]
    pub fn level(self) -> u8 {
        self as u8
    }
}
