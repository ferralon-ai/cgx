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
/// `admits` is the per-edge predicate walks consult; everything in it is a plain
/// comparison so it adds no allocation and no nondeterminism. The default admits
/// every call-family edge at any confidence.
///
/// One clause — [`max_candidates`](Self::max_candidates) — is a property of the
/// candidate *group*, not of any single edge, so it cannot be decided from an
/// `EdgeRecord` alone: [`admits`](Self::admits) deliberately ignores it, and it is
/// enforced by [`GraphView::neighbors`](crate::GraphView::neighbors), the single
/// adjacency primitive every walk consults, which owns the candidate table.
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
    /// When `Some(n)`, drop any edge that belongs to an over-approximated candidate
    /// set of size `> n` (F2 `--max-candidates`): the fan-out dial orthogonal to
    /// the confidence floor. An edge with no candidate group is a single resolved
    /// target (fan-out 1) and is never dropped. `None` = no fan-out cap.
    ///
    /// This is a group-global property; [`admits`](Self::admits) cannot see it, so
    /// it is enforced in [`GraphView::neighbors`](crate::GraphView::neighbors),
    /// which holds the candidate table. Filtering large groups changes an answer's
    /// *completeness* — the approximation contract reports the excluded edges
    /// (`dropped-max-candidates`).
    pub max_candidates: Option<u32>,
}

impl Default for EdgeFilter {
    fn default() -> Self {
        EdgeFilter {
            min_confidence: Confidence::Possible,
            condition: ConditionFilter::Any,
            kinds: None,
            max_candidates: None,
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

    /// Cap the over-approximated candidate-set fan-out (`--max-candidates`). Edges
    /// in a candidate group larger than `n` are dropped by
    /// [`GraphView::neighbors`](crate::GraphView::neighbors); see
    /// [`max_candidates`](Self::max_candidates).
    pub fn with_max_candidates(mut self, n: u32) -> Self {
        self.max_candidates = Some(n);
        self
    }

    /// Whether `edge` passes every *per-edge* clause of this filter (kind,
    /// confidence, condition). The [`max_candidates`](Self::max_candidates) clause
    /// is a candidate-group property this predicate cannot decide from an edge
    /// alone; it is applied by
    /// [`GraphView::neighbors`](crate::GraphView::neighbors).
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
