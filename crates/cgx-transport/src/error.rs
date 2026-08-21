//! Transport error type. Every git subprocess failure carries the exact command
//! and its stderr so a caller can diagnose a plumbing fault without re-running it.

/// The refspec a clone / CI runner must configure for `refs/cgx/*` to fetch, and
/// the refspec name surfaced in the [`TransportError::RefAbsent`] honesty hint.
pub const CGX_REFSPEC: &str = "+refs/cgx/*:refs/cgx/*";

/// Failure modes of the `cgx` ref-overlay transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// A `git` subprocess exited non-zero (or could not be spawned). `command` is
    /// the invocation for diagnosability; `stderr` is git's own message.
    #[error("git command failed: {command}\n{stderr}")]
    Git { command: String, stderr: String },

    /// A local filesystem read/write on the `.cgx/` set failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A stored manifest could not be decoded (postcard).
    #[error("codec error: {0}")]
    Codec(String),

    /// A pulled object's bytes did not re-hash to the OID naming it in the tree —
    /// a corrupt/tampered object. Never served (criterion 5).
    #[error("object integrity failure: entry names oid {oid}, but its bytes hash to {got}")]
    Integrity { oid: String, got: String },

    /// `refs/cgx/index` was absent after a fetch — pull fails loudly rather than
    /// materializing nothing (criterion 4 honesty spine). `hint` names the refspec.
    #[error("refs/cgx/index absent after fetch — {hint}")]
    RefAbsent { hint: String },

    /// git emitted non-UTF-8 where an OID (hex) was expected.
    #[error("git produced non-utf8 output where hex was expected ({0})")]
    NonUtf8(String),

    /// The compare-and-swap on `refs/cgx/index` lost repeatedly to concurrent
    /// local pushers.
    #[error("could not advance refs/cgx/index: repeated CAS contention")]
    RefContention,

    /// `push` found no indexed graph in `.cgx/` — refuse rather than publish an
    /// empty index ref (honesty spine: empty ≠ proven-absent).
    #[error("nothing indexed in .cgx — run `cgx index` before pushing")]
    NothingIndexed,
}

impl TransportError {
    pub(crate) fn codec(e: impl std::fmt::Display) -> Self {
        TransportError::Codec(e.to_string())
    }

    /// A `RefAbsent` error carrying the standard configure hint.
    pub(crate) fn ref_absent() -> Self {
        TransportError::RefAbsent {
            hint: format!(
                "configure the fetch refspec (`cgx configure-remote`, or \
                 `git config --add remote.<remote>.fetch {CGX_REFSPEC}`) and pull again"
            ),
        }
    }
}
