//! Traversal direction and the [`EdgeFilter`] every walk consults per edge.
//!
//! An `EdgeFilter` is a pure, stateless predicate over an [`EdgeRecord`]: it is
//! the static, per-edge half of query selectivity. The dynamic, per-*path* half
//! (path-relative transience, GM-4) lives in the walker's per-walk state, not
//! here — by construction, because a CTE/edge predicate cannot express it.

use cgx_core::{Confidence, EdgeCondition, EdgeKind, EdgeRecord};

/// Which way a walk moves along call edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Source → destination: follow callees (`callees`, forward reachability).
    Forward,
    /// Destination → source: follow callers (`callers`).
    Backward,
}

impl Direction {
    /// The opposite direction.
    pub fn reverse(self) -> Direction {
        match self {
            Direction::Forward => Direction::Backward,
            Direction::Backward => Direction::Forward,
        }
    }
}

/// How an [`EdgeFilter`] restricts edge-condition labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionFilter {
    /// No restriction on edge condition.
    Any,
    /// Admit only edges whose condition is exactly this label
    /// (`--edge-condition LABEL`, Q-11).
    Only(EdgeCondition),
    /// Reject any edge whose condition is this label (`--exclude-edge-condition`,
    /// Q-11). Used for "non-exception" analyses.
    Exclude(EdgeCondition),
}

/// The static per-edge admission predicate (Q-11 edge condition, Q-18 confidence,
/// plus edge-kind scoping).
///
/// `admits` is the only method walks call; everything is a plain comparison so the
/// filter adds no allocation and no nondeterminism. The default admits every
/// call-family edge at any confidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeFilter {
    /// Minimum confidence an edge must carry (Q-18). `Possible` admits all.
    pub min_confidence: Confidence,
    /// Edge-condition restriction (Q-11).
    pub condition: ConditionFilter,
    /// When `Some`, admit only edges whose kind is in this set; when `None`, admit
    /// any **call-family** edge (the default traversal scope). Structural edges
    /// (`Contains`, `Imports`, …) are never traversed by the call-graph walks
    /// unless explicitly requested here.
    pub kinds: Option<Vec<EdgeKind>>,
}

impl Default for EdgeFilter {
    fn default() -> Self {
        EdgeFilter {
            min_confidence: Confidence::Possible,
            condition: ConditionFilter::Any,
            kinds: None,
        }
    }
}

impl EdgeFilter {
    /// A filter that admits every call-family edge at any confidence (the
    /// subcommand default before flags are applied).
    pub fn calls() -> Self {
        EdgeFilter::default()
    }

    /// Set the confidence floor (`--confidence`).
    pub fn with_min_confidence(mut self, c: Confidence) -> Self {
        self.min_confidence = c;
        self
    }

    /// Restrict to exactly one edge condition (`--edge-condition`).
    pub fn only_condition(mut self, c: EdgeCondition) -> Self {
        self.condition = ConditionFilter::Only(c);
        self
    }

    /// Exclude one edge condition (`--exclude-edge-condition`).
    pub fn exclude_condition(mut self, c: EdgeCondition) -> Self {
        self.condition = ConditionFilter::Exclude(c);
        self
    }

    /// Restrict to a specific set of edge kinds. An empty set is treated as "the
    /// default call-family scope" (same as `None`).
    pub fn with_kinds(mut self, kinds: Vec<EdgeKind>) -> Self {
        self.kinds = if kinds.is_empty() { None } else { Some(kinds) };
        self
    }

    /// Whether `edge` passes every clause of this filter.
    pub fn admits(&self, edge: &EdgeRecord) -> bool {
        match &self.kinds {
            Some(kinds) => {
                if !kinds.contains(&edge.kind) {
                    return false;
                }
            }
            None => {
                if !edge.kind.is_call() {
                    return false;
                }
            }
        }

        if edge.confidence < self.min_confidence {
            return false;
        }

        match self.condition {
            ConditionFilter::Any => true,
            ConditionFilter::Only(c) => edge.condition == c,
            ConditionFilter::Exclude(c) => edge.condition != c,
        }
    }
}
