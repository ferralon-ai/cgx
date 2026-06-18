//! Edge-condition labels (GM-3) and their precedence rule (ADR-03).
//!
//! Every call edge carries exactly one of the five values in [`EdgeCondition`].
//! Compound labels are banned (GM-3). When a call site is nested inside several
//! guarding constructs, the frontend collects the set of applicable labels and
//! emits the single maximum by precedence:
//!
//! ```text
//! panic > exception > loop > conditional > always
//! ```
//!
//! This is ADR-03: most-semantically-significant wins, so that exceptional-class
//! membership (GM-4) survives whenever *any* enclosing construct is exceptional.

use serde::{Deserialize, Serialize};

/// The runtime condition under which a call edge is taken (GM-3).
///
/// The five-value set is frozen. The discriminant values are assigned in
/// ascending precedence order so that [`EdgeCondition::max`] is a plain integer
/// comparison and the derived `Ord` matches the ADR-03 precedence total order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum EdgeCondition {
    /// Taken on every execution of the call site; no runtime condition guards it.
    /// Also used for calls inside `finally`/`defer`/cleanup blocks (the call is
    /// always taken once that block is entered — its exception-relativity is
    /// path-relative transience, GM-4, not a stored label).
    Always = 0,
    /// Taken only when an explicit boolean branch is true (`if`, `match` arm, ternary).
    Conditional = 1,
    /// Taken zero or more times inside a loop body (`for`, `while`, `loop`).
    Loop = 2,
    /// Taken only during exceptional control flow — a `catch`/`except` handler,
    /// a Rust `?`/`Err` branch, a Go `err != nil` block, or a C sentinel check.
    Exception = 3,
    /// Taken only on an unwinding/aborting path: Rust `panic!`/`unwrap`/`expect`,
    /// Go `panic`, C `abort`.
    Panic = 4,
}

impl EdgeCondition {
    /// All five labels in ascending precedence order.
    pub const ALL: [EdgeCondition; 5] = [
        EdgeCondition::Always,
        EdgeCondition::Conditional,
        EdgeCondition::Loop,
        EdgeCondition::Exception,
        EdgeCondition::Panic,
    ];

    /// ADR-03 precedence rank. Higher wins. `panic`=4 down to `always`=0.
    #[inline]
    pub fn precedence(self) -> u8 {
        self as u8
    }

    /// The ADR-03 maximum of two labels: the more semantically significant one.
    #[inline]
    pub fn max(self, other: EdgeCondition) -> EdgeCondition {
        if self.precedence() >= other.precedence() {
            self
        } else {
            other
        }
    }

    /// Reduce a set of applicable labels (from the chain of enclosing constructs
    /// between the call expression and the caller's body root) to the single
    /// emitted label, per the ADR-03 determination rule: the maximum by
    /// precedence. An empty input is treated as the unguarded case (`always`).
    ///
    /// The `finally`/`defer` carve-out (GM-3.1) is applied *before* this call by
    /// the frontend: a `finally` block contributes [`EdgeCondition::Always`] to
    /// the applicable set, not [`EdgeCondition::Exception`].
    pub fn resolve<I>(applicable: I) -> EdgeCondition
    where
        I: IntoIterator<Item = EdgeCondition>,
    {
        applicable
            .into_iter()
            .fold(EdgeCondition::Always, EdgeCondition::max)
    }

    /// Whether this label is in the **exceptional class** (`exception` or
    /// `panic`) — the class GM-4 path-relative transience filters against.
    #[inline]
    pub fn is_exceptional(self) -> bool {
        matches!(self, EdgeCondition::Exception | EdgeCondition::Panic)
    }
}
