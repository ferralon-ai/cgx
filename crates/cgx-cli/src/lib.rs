//! `cgx` command-line internals.
//!
//! The binary ([`main`](../main.rs)) is a thin clap shell over this library; the
//! library is what the integration tests link against for the unit-level checks
//! (SARIF validity, exit-code mapping, pattern parsing). Every command path
//! returns a [`CliError`] carrying the IF-4 / ADR-08 exit code so the binary's
//! `main` does a single `process::exit`.

#![forbid(unsafe_code)]

pub mod assertions;
pub mod exit;
pub mod forest;
pub mod output;
pub mod pattern;
pub mod store_loc;

use exit::ExitCode;

/// A CLI error with a fixed exit code (IF-4 / ADR-08) and a human message
/// (written to stderr). Not all non-zero exits are "errors" in the Rust sense —
/// a fired assertion (code 1) and a vacuous pass (code 4) also flow through here
/// so the exit contract has one home.
#[derive(Debug, Clone)]
pub struct CliError {
    pub code: ExitCode,
    pub message: String,
}

impl CliError {
    pub fn new(code: ExitCode, message: impl Into<String>) -> Self {
        CliError {
            code,
            message: message.into(),
        }
    }

    /// A graph error (exit 3): missing/corrupt index or a failed build.
    pub fn graph(message: impl Into<String>) -> Self {
        CliError::new(ExitCode::Graph, message)
    }

    /// A usage error (exit 2): bad symbol pattern or invalid flag combination.
    pub fn usage(message: impl Into<String>) -> Self {
        CliError::new(ExitCode::Usage, message)
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}
