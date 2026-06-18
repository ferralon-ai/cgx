//! Errors raised while handling a tool call, and the top-level serve error.
//!
//! A [`ToolError`] is the failure of a single `tools/call`; it is mapped to a
//! JSON-RPC error object by the dispatcher so one bad call never tears down the
//! session. [`ServeError`] is the fatal, loop-level error of the STDIO server.

use crate::protocol::RpcError;

/// A recoverable failure while executing one MCP tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The request arguments were malformed or a required field was missing.
    InvalidParams(String),
    /// The named symbol could not be resolved (zero or ambiguous matches).
    Resolve(String),
    /// Indexing or graph loading failed.
    Index(String),
    /// The requested tool is registered but not implemented in this phase.
    Unimplemented(String),
}

impl ToolError {
    pub fn invalid_params(m: impl Into<String>) -> Self {
        ToolError::InvalidParams(m.into())
    }
    pub fn resolve(m: impl Into<String>) -> Self {
        ToolError::Resolve(m.into())
    }
    pub fn index(m: impl Into<String>) -> Self {
        ToolError::Index(m.into())
    }
    pub fn unimplemented(m: impl Into<String>) -> Self {
        ToolError::Unimplemented(m.into())
    }

    /// Map to a JSON-RPC error object.
    pub fn to_rpc(&self) -> RpcError {
        match self {
            ToolError::InvalidParams(m) => RpcError::invalid_params(m.clone()),
            ToolError::Resolve(m) => RpcError::invalid_params(m.clone()),
            ToolError::Index(m) => RpcError::internal(m.clone()),
            ToolError::Unimplemented(m) => RpcError::method_not_found(m),
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::InvalidParams(m) => write!(f, "invalid params: {m}"),
            ToolError::Resolve(m) => write!(f, "{m}"),
            ToolError::Index(m) => write!(f, "index error: {m}"),
            ToolError::Unimplemented(m) => write!(f, "unimplemented: {m}"),
        }
    }
}

impl std::error::Error for ToolError {}

/// A fatal error of the STDIO serve loop (I/O on stdin/stdout).
#[derive(Debug)]
pub enum ServeError {
    Io(std::io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Io(e) => write!(f, "stdio error: {e}"),
        }
    }
}

impl std::error::Error for ServeError {}

impl From<std::io::Error> for ServeError {
    fn from(e: std::io::Error) -> Self {
        ServeError::Io(e)
    }
}
