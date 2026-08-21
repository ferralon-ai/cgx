//! Result and diagnostics types. Every honesty signal the engine can raise is a
//! structured value here — never a log string (dispatch §7).

use crate::family::Family;
use serde::{Deserialize, Serialize};

/// A parse-time diagnostic attached to a compiled [`Selector`](crate::Selector).
/// Per-node signals (truncation, agnostic cross-language) live on
/// [`NodeMatch`](crate::NodeMatch) instead, since they depend on the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Diagnostic {
    /// A degenerate `!*` was rewritten to `*!` past the wildcard (dispatch §2).
    /// The negation cannot bind a wildcard, so it moves to the next atom.
    NegationRewrite {
        family: Family,
        /// The offending segment as written.
        original_segment: String,
        /// The segment after the `!*` → `*!` rewrite.
        rewritten_segment: String,
    },
}

/// Why a single tokenizer rejected a selector (contributes no interpretation).
/// Aggregated into a [`SelectorError`] only when *every* tokenizer rejects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// `?` is not part of this grammar (the coexisting `pattern.rs` glob keeps it).
    QuestionMark { pos: usize },
    /// A character illegal for this family's identifier grammar.
    IllegalChar { ch: char, pos: usize },
    /// An empty segment (e.g. `a::::b`) — distinct from a leading root anchor.
    EmptySegment,
    /// Structurally malformed (unbalanced `()`, misplaced `!`, empty alternative).
    Malformed { detail: String },
}

impl RejectReason {
    fn describe(&self) -> String {
        match self {
            RejectReason::QuestionMark { pos } => {
                format!("`?` at position {pos} is unsupported")
            }
            RejectReason::IllegalChar { ch, pos } => {
                format!("illegal character {ch:?} at position {pos}")
            }
            RejectReason::EmptySegment => "empty segment".to_string(),
            RejectReason::Malformed { detail } => detail.clone(),
        }
    }
}

/// A hard, labeled failure to compile a selector. Never a silent empty match:
/// zero clean parses is always an error (dispatch §4 honesty invariant).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectorError {
    /// The selector uses `?`, which this engine drops. Points at the character.
    #[error(
        "selector {selector:?}: `?` at position {pos} is not supported by the \
         selector engine (the coexisting glob matcher still supports it)"
    )]
    UnsupportedQuestionMark { selector: String, pos: usize },

    /// No registered tokenizer produced a clean parse. Carries the per-family
    /// rejection reasons so the user can see why.
    #[error("selector {selector:?}: no valid parse ({reason})")]
    NoValidParse { selector: String, reason: String },

    /// The selector string was empty.
    #[error("selector is empty")]
    Empty,
}

impl SelectorError {
    /// Choose the most informative error from a set of per-tokenizer rejections.
    /// A `?` anywhere is surfaced as the specific labeled error (dispatch §2).
    pub(crate) fn from_rejects(selector: &str, rejects: &[RejectReason]) -> SelectorError {
        if let Some(RejectReason::QuestionMark { pos }) = rejects
            .iter()
            .find(|r| matches!(r, RejectReason::QuestionMark { .. }))
        {
            return SelectorError::UnsupportedQuestionMark {
                selector: selector.to_string(),
                pos: *pos,
            };
        }
        // Deduplicate reason strings for a stable, readable message.
        let mut seen: Vec<String> = Vec::new();
        for r in rejects {
            let d = r.describe();
            if !seen.contains(&d) {
                seen.push(d);
            }
        }
        seen.sort();
        SelectorError::NoValidParse {
            selector: selector.to_string(),
            reason: seen.join("; "),
        }
    }
}

/// The outcome of matching one [`NodeRecord`](cgx_core::node::NodeRecord) against
/// a compiled selector. Carries provenance and the two per-node honesty signals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeMatch {
    /// Whether the node is selected under the active [`MatchOptions`](crate::MatchOptions).
    pub matched: bool,
    /// Which families produced the match (provenance). Empty when `!matched`.
    /// Sorted and deduplicated for determinism.
    pub families: Vec<Family>,
    /// The active-state cap tripped while evaluating this node: the answer may be
    /// incomplete (empty ≠ absent). Surfaced even when `!matched` (dispatch §6).
    pub truncated: bool,
    /// The node matched *only* because the agnostic flag dropped the family gate —
    /// a cross-language surprise the caller must be told about (dispatch §7).
    pub agnostic_cross_language: bool,
}
