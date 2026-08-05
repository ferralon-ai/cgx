//! # cgx-mcp
//!
//! The Phase-2-bridge **MCP STDIO server** (docs/07 IF-9..19, architecture §5
//! WP-13). It exposes the deterministic, read-only `cgx-query` engine to AI
//! coding/security agents over JSON-RPC 2.0 on standard input/output — no daemon,
//! no network, no LLM calls. Given the same working tree, every tool is a pure
//! function of its arguments.
//!
//! ## Transport & methods
//!
//! NDJSON (newline-delimited JSON-RPC 2.0). The Phase-1 server speaks three
//! methods:
//!
//! - **`initialize`** — announces protocol version [`PROTOCOL_VERSION`] and the
//!   `tools` capability.
//! - **`tools/list`** — the tool registry with input schemas (docs/07 IF-10..15).
//! - **`tools/call`** — runs one tool and returns a `structuredContent` body plus
//!   a `content` text mirror (IF-17).
//!
//! ## Tools (map to `cgx-query`)
//!
//! `callers`, `callees` (IF-11/12), `reaches`, `paths` (IF-13), `unused` (IF-14),
//! `explain` (IF-15), `search`, `symbols`, `flows_to`, `flows_from` — ten typed
//! query wrappers, each a thin envelope over the matching `cgx-query` function.
//! `graph_query` (IF-10) routes to the live CQL engine (`cgx_cql::run`) — the
//! same engine the `cgx query` CLI subcommand drives, no second execution path.
//! A `RETURN path` query surfaces a `paths` channel (the `paths`-tool shape); a
//! tabular query surfaces `columns`/`rows`. Parse/plan/eval rejects map to an
//! actionable `invalid_params` error rather than a wrong answer. Every result
//! carries the confidence ladder (GM-5), edge-condition context (GM-3), and the
//! A3/A4 approximation contract (`callers`/`callees`/`reaches`/`paths`/`unused`/
//! `flows_to`/`flows_from`/`graph_query`; `explain`/`search`/`symbols` do not) —
//! the same honesty the CLI emits — and supports `max_results`/`cursor`
//! pagination (IF-18).
//!
//! ## `include_dirty` overlay (ADR-06, default **true** for MCP)
//!
//! Every graph-reading tool accepts `include_dirty` (default `true`, the inverse
//! of the CLI's `--include-dirty`, which defaults to `false`). On a call with
//! `include_dirty: true` the server indexes the **working tree** for that call so
//! an agent's uncommitted edits are visible to `callers`/`paths`/… The overlay is
//! **per call** and **never persisted** (a fresh in-memory store backs each
//! acquisition). When the tree differs from `HEAD`, `graph_version` becomes
//! `"<tree-oid>+dirty.<overlay-digest>"` and the response reports `dirty: true`
//! plus `dirty_files_analyzed` — so an agent (and a cache keyed on
//! `(query, graph_version)`) can tell the post-edit base from the committed one.
//! See [`session`] for the mechanism.
//!
//! ## Testability
//!
//! [`dispatch`] is a pure function over a [`Request`]; [`serve_io`] drives the
//! loop over any `BufRead`/`Write`. Tests exercise both directly — no live stdio
//! pipe is required. [`serve`] is the thin wrapper binding them to the process's
//! stdin/stdout, and is the one line the CLI wires to `cgx mcp`.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod error;
pub mod protocol;
pub mod server;
pub mod session;
pub mod tools;

pub use error::{ServeError, ToolError};
pub use protocol::{Request, Response, RpcError};
pub use server::{dispatch, serve, serve_io, ServerConfig, PROTOCOL_VERSION};
pub use session::{acquire, GraphSession};
pub use tools::{call as call_tool, tool_list};
