//! The MCP tool surface (docs/07 IF-10..15): schemas for `tools/list` and the
//! deterministic, read-only handlers for `tools/call`.
//!
//! Each handler maps a tool's arguments onto the typed query functions in
//! `cgx-query` over a [`GraphSession`] acquired per call (with the ADR-06
//! `include_dirty` overlay). Results surface the confidence ladder (GM-5) and the
//! edge-condition context (GM-3) — the same honesty the CLI emits — plus the
//! `graph_version`/`dirty`/`dirty_files_analyzed` metadata ADR-06 requires. No
//! tool performs any network or LLM call; given the same tree, every tool is a
//! pure function of its arguments.

use cgx_core::{Confidence, EdgeCondition, NodeId, SymbolKind, SymbolPattern};
use cgx_query::{
    callees, callers, explain, paths as query_paths, resolve_anchor, unused, ConditionFilter,
    Direction, EdgeFilter, GraphView, NeighborResult, PathResult, PathWalker,
};
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::error::ToolError;
use crate::session::{self, GraphSession};

/// Default page size for list-shaped tool results (docs/07 IF-18).
const DEFAULT_MAX_RESULTS: usize = 20;
/// Hard cap on page size (docs/07 IF-18: "max 200").
const MAX_MAX_RESULTS: usize = 200;
/// `callers`/`callees` default traversal depth (docs/07 IF-11).
const DEFAULT_DEPTH: u32 = 1;
/// `paths` default depth bound (docs/07 IF-13).
const DEFAULT_PATHS_MAX_DEPTH: u32 = 10;
/// `paths` default result page size (docs/07 IF-13).
const DEFAULT_PATHS_MAX_RESULTS: usize = 10;

/// The tool registrations returned by `tools/list`. Order is fixed for
/// determinism. Each declares its `inputSchema` per docs/07; the agent-facing
/// `include_dirty` default is `true` on every graph-reading tool (ADR-06).
pub fn tool_list() -> Value {
    json!({
        "tools": [
            neighbor_tool("callers", "Symbols that (transitively, to `depth`) call the named symbol."),
            neighbor_tool("callees", "Symbols the named symbol (transitively, to `depth`) calls."),
            paths_tool(),
            unused_tool(),
            explain_tool(),
            graph_query_tool(),
        ]
    })
}

fn include_dirty_prop() -> Value {
    json!({
        "type": "boolean",
        "default": true,
        "description": "Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted"
    })
}

fn neighbor_tool(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "symbol":         { "type": "string", "description": "Qualified symbol name" },
                "root":           { "type": "string", "description": "Repository root path" },
                "depth":          { "type": "integer", "default": DEFAULT_DEPTH },
                "max_results":    { "type": "integer", "default": DEFAULT_MAX_RESULTS },
                "cursor":         { "type": "string" },
                "edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
                "confidence":     { "type": "string", "enum": ["certain","probable","possible"] },
                "include_dirty":  include_dirty_prop()
            },
            "required": ["symbol", "root"]
        }
    })
}

fn paths_tool() -> Value {
    json!({
        "name": "paths",
        "description": "Enumerate call paths from one symbol to another, surfacing per-path confidence and exceptional-class crossing.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "from":                   { "type": "string" },
                "to":                     { "type": "string" },
                "root":                   { "type": "string" },
                "max_depth":              { "type": "integer", "default": DEFAULT_PATHS_MAX_DEPTH },
                "max_results":            { "type": "integer", "default": DEFAULT_PATHS_MAX_RESULTS },
                "exclude_edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
                "only_edge_condition":    { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
                "cursor":                 { "type": "string" },
                "include_dirty":          include_dirty_prop()
            },
            "required": ["from", "to", "root"]
        }
    })
}

fn unused_tool() -> Value {
    json!({
        "name": "unused",
        "description": "Symbols not reachable from any entrypoint (the complement of entrypoint reachability).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "root":          { "type": "string" },
                "kind":          { "type": "string", "enum": ["function","method","field","all"], "default": "all" },
                "entrypoint":    { "type": "string" },
                "max_results":   { "type": "integer", "default": DEFAULT_MAX_RESULTS },
                "cursor":        { "type": "string" },
                "include_dirty": include_dirty_prop()
            },
            "required": ["root"]
        }
    })
}

fn explain_tool() -> Value {
    json!({
        "name": "explain",
        "description": "Full provenance for one symbol: definition location, caller/callee counts, and every incident edge with its condition and confidence.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "symbol":        { "type": "string" },
                "root":          { "type": "string" },
                "include_dirty": include_dirty_prop()
            },
            "required": ["symbol", "root"]
        }
    })
}

fn graph_query_tool() -> Value {
    json!({
        "name": "graph_query",
        "description": "Execute a Cypher-subset query expression. Reserved: the query language is a post-Phase-1 deliverable; use callers/callees/paths/unused for now.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query":         { "type": "string", "description": "Cypher-subset query expression" },
                "root":          { "type": "string" },
                "max_results":   { "type": "integer", "default": DEFAULT_MAX_RESULTS },
                "cursor":        { "type": "string" },
                "include_dirty": include_dirty_prop()
            },
            "required": ["query", "root"]
        }
    })
}

/// Dispatch a `tools/call` by tool name. Returns the `structuredContent` body
/// (without the surrounding `content`/`isError` wrapper, which the server adds).
pub fn call(name: &str, args: &Value) -> Result<Value, ToolError> {
    match name {
        "callers" => neighbor_call(args, Direction::Backward),
        "callees" => neighbor_call(args, Direction::Forward),
        "paths" => paths_call(args),
        "unused" => unused_call(args),
        "explain" => explain_call(args),
        "graph_query" => Err(ToolError::unimplemented(
            "graph_query: the Cypher-subset query language is not implemented in Phase 1; use callers/callees/paths/unused",
        )),
        other => Err(ToolError::unimplemented(format!("unknown tool `{other}`"))),
    }
}

// --- argument helpers -------------------------------------------------------

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::invalid_params(format!("missing required string `{key}`")))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn opt_u32(args: &Value, key: &str) -> Result<Option<u32>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(|n| Some(n as u32)).ok_or_else(|| {
            ToolError::invalid_params(format!("`{key}` must be a non-negative integer"))
        }),
    }
}

fn opt_usize(args: &Value, key: &str) -> Result<Option<usize>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(|n| Some(n as usize)).ok_or_else(|| {
            ToolError::invalid_params(format!("`{key}` must be a non-negative integer"))
        }),
    }
}

/// `include_dirty` defaults to `true` for MCP (ADR-06). Any non-bool is rejected.
fn include_dirty(args: &Value) -> Result<bool, ToolError> {
    match args.get("include_dirty") {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ToolError::invalid_params(
            "`include_dirty` must be a boolean",
        )),
    }
}

fn page_size(args: &Value, default: usize) -> Result<usize, ToolError> {
    let raw = opt_usize(args, "max_results")?.unwrap_or(default);
    Ok(raw.clamp(1, MAX_MAX_RESULTS))
}

/// Parse the opaque cursor (a 0-based offset rendered as a decimal string).
fn cursor_offset(args: &Value) -> Result<usize, ToolError> {
    match opt_str(args, "cursor") {
        None => Ok(0),
        Some(s) => s
            .parse::<usize>()
            .map_err(|_| ToolError::invalid_params("`cursor` is not a valid pagination cursor")),
    }
}

fn parse_condition(s: &str) -> Result<EdgeCondition, ToolError> {
    match s {
        "always" => Ok(EdgeCondition::Always),
        "conditional" => Ok(EdgeCondition::Conditional),
        "loop" => Ok(EdgeCondition::Loop),
        "exception" => Ok(EdgeCondition::Exception),
        "panic" => Ok(EdgeCondition::Panic),
        other => Err(ToolError::invalid_params(format!(
            "unknown edge condition `{other}`"
        ))),
    }
}

fn parse_confidence(s: &str) -> Result<Confidence, ToolError> {
    match s {
        "possible" => Ok(Confidence::Possible),
        "probable" => Ok(Confidence::Probable),
        "certain" => Ok(Confidence::Certain),
        other => Err(ToolError::invalid_params(format!(
            "unknown confidence `{other}`"
        ))),
    }
}

fn condition_token(c: EdgeCondition) -> &'static str {
    match c {
        EdgeCondition::Always => "always",
        EdgeCondition::Conditional => "conditional",
        EdgeCondition::Loop => "loop",
        EdgeCondition::Exception => "exception",
        EdgeCondition::Panic => "panic",
    }
}

fn confidence_token(c: Confidence) -> &'static str {
    match c {
        Confidence::Possible => "possible",
        Confidence::Probable => "probable",
        Confidence::Certain => "certain",
    }
}

/// Build the edge filter from the optional `edge_condition` / `confidence` args.
fn neighbor_filter(args: &Value) -> Result<EdgeFilter, ToolError> {
    let mut filter = EdgeFilter::calls();
    if let Some(c) = opt_str(args, "edge_condition") {
        filter = filter.only_condition(parse_condition(c)?);
    }
    if let Some(c) = opt_str(args, "confidence") {
        filter = filter.with_min_confidence(parse_confidence(c)?);
    }
    Ok(filter)
}

/// Resolve `root` to a session (acquires the graph with the dirty overlay).
fn session_for(args: &Value) -> Result<GraphSession, ToolError> {
    let root = PathBuf::from(req_str(args, "root")?);
    session::acquire(&root, include_dirty(args)?)
}

/// Resolve a `--symbol`-style argument to a unique node, surfacing a clean error.
fn resolve(view: &GraphView, name: &str) -> Result<NodeId, ToolError> {
    resolve_anchor(view, &SymbolPattern::fqn(name))
        .or_else(|_| resolve_anchor(view, &SymbolPattern::short_name(name)))
        .map_err(|e| ToolError::resolve(e.to_string()))
}

// --- result rendering -------------------------------------------------------

fn neighbor_json(r: &NeighborResult) -> Value {
    json!({
        "name": r.node.fqn,
        "file": r.node.file,
        "line": r.node.line_start,
        "depth": r.depth,
        "edge_condition": condition_token(r.condition),
        "confidence": confidence_token(r.confidence),
        "min_confidence_on_path": confidence_token(r.min_confidence_on_path),
        "exception_transient": r.exception_transient
    })
}

fn path_json(p: &PathResult) -> Value {
    let steps: Vec<Value> = p
        .steps
        .iter()
        .map(|s| {
            json!({
                "name": s.node.fqn,
                "file": s.node.file,
                "line": s.node.line_start,
                "edge_condition": s.via.as_ref().map(|e| condition_token(e.condition)),
                "confidence": s.via.as_ref().map(|e| confidence_token(e.confidence)),
                "exception_transient": s.exception_transient
            })
        })
        .collect();
    json!({
        "hops": p.hops(),
        "min_confidence": confidence_token(p.min_confidence),
        "crosses_exceptional": p.crosses_exceptional,
        "steps": steps
    })
}

/// Apply offset/limit pagination to a result slice, returning the page plus the
/// `has_more` flag and next cursor (docs/07 IF-18).
fn paginate(total: usize, offset: usize, limit: usize) -> (usize, usize, bool, Option<String>) {
    let start = offset.min(total);
    let end = (start + limit).min(total);
    let has_more = end < total;
    let cursor = if has_more {
        Some(end.to_string())
    } else {
        None
    };
    (start, end, has_more, cursor)
}

/// Attach the ADR-06 honesty metadata to a result envelope.
fn with_session_meta(mut body: Value, session: &GraphSession) -> Value {
    let obj = body.as_object_mut().expect("result body is an object");
    obj.insert("graph_version".into(), json!(session.graph_version));
    obj.insert("dirty".into(), json!(session.dirty));
    obj.insert(
        "dirty_files_analyzed".into(),
        json!(session.dirty_files_analyzed),
    );
    body
}

// --- handlers ---------------------------------------------------------------

fn neighbor_call(args: &Value, dir: Direction) -> Result<Value, ToolError> {
    let symbol = req_str(args, "symbol")?;
    let session = session_for(args)?;
    let anchor = resolve(&session.view, symbol)?;

    let walker = PathWalker {
        filter: neighbor_filter(args)?,
        max_depth: Some(opt_u32(args, "depth")?.unwrap_or(DEFAULT_DEPTH)),
        max_paths: None,
        max_steps: None,
    };
    let results = match dir {
        Direction::Backward => callers(&session.view, anchor, &walker),
        Direction::Forward => callees(&session.view, anchor, &walker),
    };

    let limit = page_size(args, DEFAULT_MAX_RESULTS)?;
    let offset = cursor_offset(args)?;
    let (start, end, has_more, cursor) = paginate(results.len(), offset, limit);
    let page: Vec<Value> = results[start..end].iter().map(neighbor_json).collect();

    let body = json!({
        "symbol": symbol,
        "results": page,
        "total_matched": results.len(),
        "has_more": has_more,
        "cursor": cursor
    });
    Ok(with_session_meta(body, &session))
}

fn paths_call(args: &Value) -> Result<Value, ToolError> {
    let from = req_str(args, "from")?;
    let to = req_str(args, "to")?;
    let session = session_for(args)?;
    let from_id = resolve(&session.view, from)?;
    let to_id = resolve(&session.view, to)?;

    let mut filter = EdgeFilter::calls();
    if let Some(c) = opt_str(args, "only_edge_condition") {
        filter.condition = ConditionFilter::Only(parse_condition(c)?);
    } else if let Some(c) = opt_str(args, "exclude_edge_condition") {
        filter.condition = ConditionFilter::Exclude(parse_condition(c)?);
    }
    let walker = PathWalker {
        filter,
        max_depth: Some(opt_u32(args, "max_depth")?.unwrap_or(DEFAULT_PATHS_MAX_DEPTH)),
        max_paths: None,
        max_steps: None,
    };
    let result = query_paths(&session.view, from_id, to_id, &walker);
    let results = &result.paths;

    let limit = page_size(args, DEFAULT_PATHS_MAX_RESULTS)?;
    let offset = cursor_offset(args)?;
    let (start, end, has_more, cursor) = paginate(results.len(), offset, limit);
    let page: Vec<Value> = results[start..end].iter().map(path_json).collect();

    let body = json!({
        "from": from,
        "to": to,
        "paths": page,
        "total_matched": results.len(),
        "has_more": has_more,
        "cursor": cursor,
        // ADR-06 honesty: a budget/cap cutoff is surfaced, never silent. `null`
        // when the enumeration was complete.
        "truncated": result.truncated(),
        "truncation_reason": result.truncation.map(|r| r.token())
    });
    Ok(with_session_meta(body, &session))
}

fn unused_call(args: &Value) -> Result<Value, ToolError> {
    let session = session_for(args)?;

    let kinds: Vec<SymbolKind> = match opt_str(args, "kind").unwrap_or("all") {
        "all" => vec![],
        "function" => vec![SymbolKind::Function],
        "method" => vec![SymbolKind::Method],
        "field" => vec![SymbolKind::Field],
        other => return Err(ToolError::invalid_params(format!("unknown kind `{other}`"))),
    };

    let entrypoints: Vec<NodeId> = match opt_str(args, "entrypoint") {
        Some(name) => vec![resolve(&session.view, name)?],
        None => vec![],
    };

    let walker = PathWalker::default();
    let results = unused(&session.view, &entrypoints, &kinds, &walker);

    let limit = page_size(args, DEFAULT_MAX_RESULTS)?;
    let offset = cursor_offset(args)?;
    let (start, end, has_more, cursor) = paginate(results.len(), offset, limit);
    let page: Vec<Value> = results[start..end]
        .iter()
        .map(|n| {
            json!({
                "name": n.fqn,
                "file": n.file,
                "line": n.line_start,
                "kind": format!("{:?}", n.kind).to_lowercase()
            })
        })
        .collect();

    let body = json!({
        "results": page,
        "total_matched": results.len(),
        "has_more": has_more,
        "cursor": cursor
    });
    Ok(with_session_meta(body, &session))
}

fn explain_call(args: &Value) -> Result<Value, ToolError> {
    let symbol = req_str(args, "symbol")?;
    let session = session_for(args)?;
    let view = &session.view;
    let anchor = resolve(view, symbol)?;
    let explanation = explain(view, anchor)
        .ok_or_else(|| ToolError::resolve(format!("no symbol matched pattern `{symbol}`")))?;

    let edges: Vec<Value> = explanation
        .edges
        .iter()
        .map(|e| {
            json!({
                "direction": if e.incoming { "incoming" } else { "outgoing" },
                "peer": e.peer.fqn,
                "condition": condition_token(e.condition),
                "confidence": confidence_token(e.confidence),
                "peer_file": e.peer.file,
                "peer_line": e.peer.line_start
            })
        })
        .collect();

    let node = &explanation.node;
    let body = json!({
        "symbol": node.fqn,
        "file": node.file,
        "line": node.line_start,
        "kind": format!("{:?}", node.kind).to_lowercase(),
        "callers_count": explanation.callers_count,
        "callees_count": explanation.callees_count,
        "edges": edges
    });
    Ok(with_session_meta(body, &session))
}
