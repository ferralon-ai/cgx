//! Store error type. No `thiserror` dependency (kept lean); a hand-written enum.

use cgx_core::codec::CodecError;

/// Anything that can go wrong opening or operating the store.
#[derive(Debug)]
pub enum StoreError {
    /// An underlying SQLite error.
    Sqlite(rusqlite::Error),
    /// A fragment or graph blob failed to encode/decode (corruption or a codec
    /// mismatch).
    Codec(CodecError),
    /// The on-disk `schema_version` is newer than this binary understands, or the
    /// `view_schema_version` does not match. Carries `(found, expected)`.
    SchemaVersion { found: i64, expected: i64 },
    /// The advisory write lock could not be acquired within the timeout (IX-7).
    LockTimeout,
    /// An I/O error opening the index directory or lockfile.
    Io(std::io::Error),
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "sqlite error: {e}"),
            StoreError::Codec(e) => write!(f, "codec error: {e}"),
            StoreError::SchemaVersion { found, expected } => write!(
                f,
                "index schema version {found} is incompatible with this binary (expected {expected})"
            ),
            StoreError::LockTimeout => write!(f, "could not acquire index write lock within timeout"),
            StoreError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<CodecError> for StoreError {
    fn from(e: CodecError) -> Self {
        StoreError::Codec(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// Convenience result alias for the store.
pub type Result<T> = std::result::Result<T, StoreError>;
