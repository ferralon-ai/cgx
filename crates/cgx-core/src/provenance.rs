//! Provenance attached to every fact (GM-6.1): the evidence that justifies it.

use crate::confidence::Tier;
use serde::{Deserialize, Serialize};

/// A source span: repo-relative file plus 1-based line and optional column.
/// Used both inside [`Provenance`] and on raw references emitted by frontends.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Span {
    /// Repo-relative path.
    pub file: String,
    /// 1-based line of the call site or definition.
    pub line: u32,
    /// Optional column (available when the parser/SCIP provides it).
    pub col: Option<u32>,
}

impl Span {
    pub fn new(file: impl Into<String>, line: u32, col: Option<u32>) -> Self {
        Span {
            file: file.into(),
            line,
            col,
        }
    }
}

/// The provenance record on a node or edge (GM-6.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Provenance {
    /// Where the fact was observed.
    pub span: Span,
    /// Name of the producing rule, e.g. `scope-ref`, `import-ref`, `name-arity`,
    /// `scip-occurrence`, `cha-override`, `heuristic-sentinel`, `macro-textual`.
    pub rule: String,
    /// Resolution tier (0–4) used to produce the fact (GM-6.1, GM-5.2).
    pub tier: Tier,
    /// Content-addressed blob OID of the source file at index time. Links the
    /// fact to the exact file version that produced it (GM-6.1 `index_id`),
    /// enabling staleness detection.
    pub index_id: String,
}

impl Provenance {
    pub fn new(
        span: Span,
        rule: impl Into<String>,
        tier: Tier,
        index_id: impl Into<String>,
    ) -> Self {
        Provenance {
            span,
            rule: rule.into(),
            tier,
            index_id: index_id.into(),
        }
    }
}
