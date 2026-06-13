//! The IF-4 exit-code contract (docs/07-interfaces.md), including the ADR-08
//! addition of code 4 for a vacuously-satisfied assertion.
//!
//! `main` returns one of these codes via `std::process::exit`; nothing in the
//! crate calls `exit` directly, so the value is testable and centralized.

/// The process exit code for a `cgx` invocation (IF-4 + ADR-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// Query completed; results returned (or zero results when expected).
    Ok = 0,
    /// A CI assertion failed (`--assert-empty` fired, or a count/max threshold
    /// was exceeded).
    AssertionFailed = 1,
    /// Query parse error or invalid flag combination.
    Usage = 2,
    /// Graph error: the index is missing, corrupt, or could not be built.
    Graph = 3,
    /// Assertion vacuously satisfied — the gate passed but matched zero symbols
    /// or every candidate result was filtered out (ADR-08). Suppress with
    /// `--allow-vacuous`.
    Vacuous = 4,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        self as i32
    }
}
