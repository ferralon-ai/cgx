//! Errors for `cgx-diff`.

/// The error type for diff and edge-age operations.
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    /// A git operation (discover, rev-parse, merge-base) failed. Note: blame
    /// *attribution* failures degrade to a [`crate::Warning`] and `None` rather
    /// than this error (WP-11 graceful degradation); this variant is for
    /// repository-level failures that prevent diffing at all.
    #[error("git error: {0}")]
    Git(String),

    /// The store could not be read.
    #[error(transparent)]
    Store(#[from] cgx_store::StoreError),

    /// The indexer failed to produce a graph for a requested tree.
    #[error(transparent)]
    Index(#[from] cgx_index::IndexError),

    /// A requested ref/tree was indexed but no graph was found for it.
    #[error("no indexed graph for {0}")]
    MissingGraph(String),
}

/// Result alias for `cgx-diff`.
pub type Result<T> = std::result::Result<T, DiffError>;
