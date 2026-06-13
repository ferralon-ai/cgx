//! JSON-RPC 2.0 envelopes (the MCP STDIO wire format, docs/07 IF-9).
//!
//! The transport is NDJSON: one JSON-RPC object per line. These types model the
//! request/response/notification shapes the [`crate::server::dispatch`] function
//! consumes and produces. They are intentionally minimal — only the fields MCP's
//! `initialize` / `tools/list` / `tools/call` methods use — and deserialize
//! leniently so a missing `params` or an extra field never aborts a session.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC 2.0 request (or notification, when `id` is absent).
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    /// Must be `"2.0"`; retained for echoing and validation.
    #[serde(default)]
    pub jsonrpc: String,
    /// Request id. `None` marks a notification (no response is emitted).
    #[serde(default)]
    pub id: Option<Value>,
    /// The method name (`initialize`, `tools/list`, `tools/call`, …).
    pub method: String,
    /// Method parameters; method-specific shape, parsed by the handler.
    #[serde(default)]
    pub params: Value,
}

impl Request {
    /// Whether this is a notification (no `id`, so no response is expected).
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// A JSON-RPC 2.0 response: exactly one of `result` / `error` is set.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// A success response carrying `result`.
    pub fn ok(id: Value, result: Value) -> Self {
        Response {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// An error response carrying a JSON-RPC error object.
    pub fn err(id: Value, error: RpcError) -> Self {
        Response {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
        }
    }

    /// `-32700` Parse error: invalid JSON was received.
    pub fn parse_error(message: impl Into<String>) -> Self {
        RpcError::new(-32700, message)
    }

    /// `-32601` Method not found.
    pub fn method_not_found(method: &str) -> Self {
        RpcError::new(-32601, format!("method not found: {method}"))
    }

    /// `-32602` Invalid params.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        RpcError::new(-32602, message)
    }

    /// `-32603` Internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        RpcError::new(-32603, message)
    }
}
