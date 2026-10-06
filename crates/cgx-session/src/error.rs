//! The error envelope every session op returns on failure, on both transports.

use cgx_mcp::ToolError;
use serde::{Deserialize, Serialize};

/// What kind of failure a [`SessionError`] is. The first four are
/// [`ToolError`]'s variants verbatim; `stale` means no graph is resident (open a
/// fresh index or index first); `internal` is a failure that is not the caller's
/// to fix (a store or codec fault).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidParams,
    Resolve,
    Index,
    Unimplemented,
    Stale,
    Internal,
}

/// A failed session op: `{"kind": "...", "message": "..."}` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionError {
    pub kind: ErrorKind,
    pub message: String,
}

impl SessionError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        SessionError {
            kind,
            message: message.into(),
        }
    }
    pub fn invalid_params(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidParams, m)
    }
    pub fn index(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Index, m)
    }
    pub fn stale(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Stale, m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, m)
    }
}

impl From<ToolError> for SessionError {
    fn from(e: ToolError) -> Self {
        match e {
            ToolError::InvalidParams(m) => Self::new(ErrorKind::InvalidParams, m),
            ToolError::Resolve(m) => Self::new(ErrorKind::Resolve, m),
            ToolError::Index(m) => Self::new(ErrorKind::Index, m),
            ToolError::Unimplemented(m) => Self::new(ErrorKind::Unimplemented, m),
        }
    }
}

impl From<cgx_index::IndexError> for SessionError {
    fn from(e: cgx_index::IndexError) -> Self {
        Self::index(e.to_string())
    }
}

impl From<cgx_store::StoreError> for SessionError {
    fn from(e: cgx_store::StoreError) -> Self {
        Self::index(e.to_string())
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = serde_json::to_value(self.kind).expect("kind serializes");
        write!(f, "{}: {}", kind.as_str().unwrap_or("error"), self.message)
    }
}

impl std::error::Error for SessionError {}

pub type Result<T> = std::result::Result<T, SessionError>;
