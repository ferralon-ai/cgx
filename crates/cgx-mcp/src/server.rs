//! The request dispatcher and the STDIO serve loop (docs/07 IF-9).
//!
//! [`dispatch`] is the testable core: a pure function from a [`Request`] to an
//! `Option<Response>` (notifications produce no response). It handles the three
//! MCP methods the Phase-1 server speaks — `initialize`, `tools/list`,
//! `tools/call` — and routes every other method to a JSON-RPC "method not found".
//!
//! [`serve`] is the thin I/O wrapper: read NDJSON lines from stdin, dispatch
//! each, write NDJSON responses to stdout. It holds no mutable cross-call state,
//! so determinism is a property of [`dispatch`] alone.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::error::ServeError;
use crate::protocol::{Request, Response, RpcError};
use crate::tools;

/// The MCP protocol version this server implements (docs/07 IF-9).
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Server configuration: the default repository root and the advertised name.
///
/// `root` is the fallback repository path when a tool call omits `root`; tools
/// that take an explicit `root` use theirs. The CLI (`cgx mcp --root <path>`)
/// constructs this and calls [`serve`].
#[derive(Debug, Clone, Default)]
pub struct ServerConfig {
    /// Default repository root (used when a tool omits `root`).
    pub root: Option<PathBuf>,
}

/// Dispatch one JSON-RPC request to its handler.
///
/// Returns `None` for notifications (no `id`), which by JSON-RPC must not be
/// answered. Every error path returns a well-formed JSON-RPC error response
/// rather than tearing down the session: one malformed `tools/call` never stops
/// the server.
pub fn dispatch(config: &ServerConfig, req: &Request) -> Option<Response> {
    if req.is_notification() {
        // Notifications (e.g. `notifications/initialized`) are accepted silently.
        return None;
    }
    let id = req.id.clone().unwrap_or(Value::Null);

    let outcome = match req.method.as_str() {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(tools::tool_list()),
        "tools/call" => tools_call(config, &req.params),
        other => Err(RpcError::method_not_found(other)),
    };

    Some(match outcome {
        Ok(result) => Response::ok(id, result),
        Err(error) => Response::err(id, error),
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": "cgx",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// Handle a `tools/call`: locate the tool, run it, and wrap the structured body
/// in the MCP `content` + `structuredContent` envelope (docs/07 IF-17).
fn tools_call(config: &ServerConfig, params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("tools/call requires a string `name`"))?;

    // Merge the server's default root into the arguments when the caller omits it.
    let mut args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    apply_default_root(config, &mut args);

    match tools::call(name, &args) {
        Ok(structured) => Ok(wrap_tool_result(structured)),
        Err(e) => Err(e.to_rpc()),
    }
}

/// Inject the server's default `root` into tool arguments when absent, so a
/// client configured with `cgx mcp --root <path>` need not repeat it per call.
fn apply_default_root(config: &ServerConfig, args: &mut Value) {
    if let Some(root) = &config.root {
        if let Some(obj) = args.as_object_mut() {
            let missing = obj
                .get("root")
                .and_then(Value::as_str)
                .is_none_or(|s| s.is_empty());
            if missing {
                obj.insert("root".into(), json!(root.to_string_lossy()));
            }
        }
    }
}

/// Wrap a tool's structured body in the 2025-06-18 result envelope: the canonical
/// `structuredContent` plus a `content` text mirror for older clients (IF-17).
fn wrap_tool_result(structured: Value) -> Value {
    let text = serde_json::to_string(&structured).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [ { "type": "text", "text": text } ],
        "structuredContent": structured,
        "isError": false
    })
}

/// Run the STDIO serve loop: read NDJSON requests from `input`, write NDJSON
/// responses to `output`. Each line is one JSON-RPC object. A malformed line
/// yields a JSON-RPC parse-error response (id `null`); EOF ends the loop.
///
/// This is the testable inner form of [`serve`]; tests drive it with in-memory
/// readers/writers without a real pipe.
pub fn serve_io(
    config: &ServerConfig,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<(), ServeError> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => dispatch(config, &req),
            Err(e) => Some(Response::err(
                Value::Null,
                RpcError::parse_error(e.to_string()),
            )),
        };
        if let Some(resp) = response {
            let bytes = serde_json::to_string(&resp)
                .map_err(|e| ServeError::Io(std::io::Error::other(e.to_string())))?;
            output.write_all(bytes.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// Run the MCP STDIO server on the process's stdin/stdout (docs/07 IF-9).
///
/// This is the one-line entry point the CLI wires to `cgx mcp`:
///
/// ```ignore
/// cgx_mcp::serve(cgx_mcp::ServerConfig { root: Some(root) })?;
/// ```
pub fn serve(config: ServerConfig) -> Result<(), ServeError> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve_io(&config, stdin.lock(), stdout.lock())
}
