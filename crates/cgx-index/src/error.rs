//! Error type for the indexing pipeline.

use cgx_core::codec::CodecError;
use cgx_frontend::FrontendError;
use cgx_store::StoreError;

/// Result alias for indexing operations.
pub type Result<T> = std::result::Result<T, IndexError>;

/// A failure raised while indexing a repository or working tree.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// A git operation failed (open, tree walk, blob read, worktree enumeration).
    #[error("git error: {0}")]
    Git(String),

    /// Walking the filesystem of a working directory failed.
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// A frontend failed to extract facts for a file.
    #[error(transparent)]
    Frontend(#[from] FrontendError),

    /// Encoding or decoding a canonical fragment failed.
    #[error("codec error: {0}")]
    Codec(String),

    /// The store rejected a read or write.
    #[error(transparent)]
    Store(#[from] StoreError),

    /// Reading or parsing a `.scip` index supplied via `--scip` failed.
    #[error("scip error: {0}")]
    Scip(String),
}

impl From<CodecError> for IndexError {
    fn from(e: CodecError) -> Self {
        IndexError::Codec(e.to_string())
    }
}
