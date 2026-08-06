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

use cgx_core::{Confidence, EdgeCondition, EdgeKind, NodeId, SymbolKind, SymbolPattern, Tier};
use cgx_query::{
    callees, callers, contract, entrypoint_roots, explain, paths as query_paths, rank_symbols,
    reaches, reaches_all, resolve_anchor, search_symbols, unused, ApproximationContract,
    ConditionFilter, Direction, EdgeBreakdown, EdgeFilter, GraphView, NeighborResult, PathResult,
    PathWalker, RankBy, ReachResult, SearchMatch, SymbolHit, SymbolRank,
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
/// Forest-shaped default depth for `reaches <from>` and `flows-*` — mirrors the
/// CLI's `DEFAULT_TREE_DEPTH` (2), the bounded neighborhood the human forest uses.
const DEFAULT_FOREST_DEPTH: u32 = 2;

/// The call-family edge kinds the `kind` filter on `callers`/`callees` accepts.
/// Restricting to the call family keeps those tools call-graph tools; the tokens
/// mirror `cgx_core::EdgeKind::is_call`.
const CALL_EDGE_KINDS: &[(&str, EdgeKind)] = &[
    ("calls", EdgeKind::Calls),
    ("calls_virtual", EdgeKind::CallsVirtual),
    ("calls_closure", EdgeKind::CallsClosure),
    ("calls_callback", EdgeKind::CallsCallback),
    ("calls_async", EdgeKind::CallsAsync),
    ("calls_indirect", EdgeKind::CallsIndirect),
    ("spawns", EdgeKind::Spawns),
];

/// The tool registrations returned by `tools/list`. Order is fixed for
/// determinism. Each declares its `inputSchema` per docs/07; the agent-facing
/// `include_dirty` default is `true` on every graph-reading tool (ADR-06).
pub fn tool_list() -> Value {
    json!({
        "tools": [
            neighbor_tool("callers", "Symbols that (transitively, to `depth`) call the named symbol."),
            neighbor_tool("callees", "Symbols the named symbol (transitively, to `depth`) calls."),
            reaches_tool(),
            paths_tool(),
            unused_tool(),
            explain_tool(),
            search_tool(),
            symbols_tool(),
            flow_tool("flows_to", "Forward data-flow slice: symbols a value flows into (its DerivesFrom consumers)."),
            flow_tool("flows_from", "Data-flow pedigree: the symbols a value derives from (its DerivesFrom sources)."),
            graph_query_tool(),
            coupling_tool(),
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
                "kind":           edge_kind_prop(),
                "include_dirty":  include_dirty_prop()
            },
            "required": ["symbol", "root"]
        }
    })
}

/// Schema for the `kind` edge-kind filter: an array of call-family edge-kind
/// tokens. When present, only edges of these kinds are traversed (default: every
/// call-family edge).
fn edge_kind_prop() -> Value {
    let tokens: Vec<&str> = CALL_EDGE_KINDS.iter().map(|(t, _)| *t).collect();
    json!({
        "type": "array",
        "items": { "type": "string", "enum": tokens },
        "description": "Restrict traversal to these call-family edge kinds (default: all call edges)"
    })
}

fn reaches_tool() -> Value {
    json!({
        "name": "reaches",
        "description": "Reachability over call edges: with `to`, whether `from` reaches `to` (with a witness path); without `to`, every symbol `from` reaches.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "from":          { "type": "string", "description": "Source symbol (qualified name)" },
                "to":            { "type": "string", "description": "Optional target symbol; omit for the from→* reachable set" },
                "root":          { "type": "string", "description": "Repository root path" },
                "depth":         { "type": "integer", "description": "Max traversal depth (from→* form)", "default": DEFAULT_FOREST_DEPTH },
                "confidence":    { "type": "string", "enum": ["certain","probable","possible"] },
                "max_results":   { "type": "integer", "default": DEFAULT_MAX_RESULTS },
                "cursor":        { "type": "string" },
                "include_dirty": include_dirty_prop()
            },
            "required": ["from", "root"]
        }
    })
}

fn search_tool() -> Value {
    json!({
        "name": "search",
        "description": "Resolve a partial/half-remembered name to exact symbol definitions: a pure node-table scan (no graph walk).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "pattern":       { "type": "string", "description": "Name predicate over the whole FQN (case-insensitive substring, or regex when `regex` is true)" },
                "all":           { "type": "boolean", "default": false, "description": "List every symbol (mutually exclusive with `pattern`)" },
                "regex":         { "type": "boolean", "default": false, "description": "Treat `pattern` as an unanchored regex" },
                "root":          { "type": "string", "description": "Repository root path" },
                "kind":          { "type": "string", "enum": ["function","method","field","type","module","all"], "default": "all" },
                "max_results":   { "type": "integer", "default": DEFAULT_MAX_RESULTS },
                "cursor":        { "type": "string" },
                "include_dirty": include_dirty_prop()
            },
            "required": ["root"]
        }
    })
}

fn symbols_tool() -> Value {
    json!({
        "name": "symbols",
        "description": "Rank symbols by reference count (the hub/importance lens), each with its inbound/outbound edge breakdown.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "rank":          { "type": "string", "enum": ["total","inbound","outbound"], "default": "total" },
                "kind":          { "type": "string", "enum": ["function","method","field","type","module","all"], "default": "all" },
                "root":          { "type": "string", "description": "Repository root path" },
                "max_results":   { "type": "integer", "default": DEFAULT_MAX_RESULTS },
                "cursor":        { "type": "string" },
                "include_dirty": include_dirty_prop()
            },
            "required": ["root"]
        }
    })
}

fn flow_tool(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "symbol":        { "type": "string", "description": "Qualified symbol name" },
                "root":          { "type": "string", "description": "Repository root path" },
                "depth":         { "type": "integer", "default": DEFAULT_FOREST_DEPTH },
                "confidence":    { "type": "string", "enum": ["certain","probable","possible"] },
                "max_results":   { "type": "integer", "default": DEFAULT_MAX_RESULTS },
                "cursor":        { "type": "string" },
                "include_dirty": include_dirty_prop()
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
                "confidence":             { "type": "string", "enum": ["certain","probable","possible"] },
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
        "description": "Execute a CQL (Cypher-subset) query against the graph and return a result table (columns + rows). A `RETURN path` query yields a paths channel. Read-only; plan-time rejects of unsupported constructs return an actionable error.",
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

/// The one tool on this surface that is **not** a graph tool: it reads committed
/// git history and never opens the call graph, so it declares no `include_dirty`
/// property. `include_dirty` describes the ADR-06 working-tree overlay *on the
/// graph*; advertising it here would promise something this answer never consults.
fn coupling_tool() -> Value {
    json!({
        "name": "coupling",
        "description": "Which files historically change together: for every pair of files changed in the same commit within an explicit commit range, the co-change count and each file's own change count. Reads committed git history only — no index, no working tree.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "root":                 { "type": "string",  "description": "Repository root path" },
                "base":                 { "type": "string",  "description": "Range start, exclusive (rev/ref/SHA)" },
                "head":                 { "type": "string",  "description": "Range end, inclusive (rev/ref/SHA)" },
                "min_cochanges":        { "type": "integer", "default": 2 },
                "max_files_per_commit": { "type": "integer", "default": 50 },
                "limit":                { "type": "integer", "default": 50 }
            },
            "required": ["root", "base", "head"]
        }
    })
}

/// Dispatch a `tools/call` by tool name. Returns the `structuredContent` body
/// (without the surrounding `content`/`isError` wrapper, which the server adds).
pub fn call(name: &str, args: &Value) -> Result<Value, ToolError> {
    match name {
        "callers" => neighbor_call(args, Direction::Backward),
        "callees" => neighbor_call(args, Direction::Forward),
        "reaches" => reaches_call(args),
        "paths" => paths_call(args),
        "unused" => unused_call(args),
        "explain" => explain_call(args),
        "search" => search_call(args),
        "symbols" => symbols_call(args),
        "flows_to" => flow_call(args, FlowDir::To),
        "flows_from" => flow_call(args, FlowDir::From),
        "graph_query" => graph_query_call(args),
        "coupling" => coupling_call(args),
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

fn tier_token(t: Tier) -> &'static str {
    match t {
        Tier::NameSyntactic => "name_syntactic",
        Tier::ScopeGraph => "scope_graph",
        Tier::Scip => "scip",
        Tier::ChaRta => "cha_rta",
        Tier::PointsTo => "points_to",
    }
}

/// Build the edge filter from the optional `edge_condition` / `confidence` /
/// `kind` args. `kind` narrows the traversed call-family edges (Q-11/Q-18).
fn neighbor_filter(args: &Value) -> Result<EdgeFilter, ToolError> {
    let mut filter = EdgeFilter::calls();
    if let Some(kinds) = parse_edge_kinds(args)? {
        filter = filter.with_kinds(kinds);
    }
    if let Some(c) = opt_str(args, "edge_condition") {
        filter = filter.only_condition(parse_condition(c)?);
    }
    if let Some(c) = opt_str(args, "confidence") {
        filter = filter.with_min_confidence(parse_confidence(c)?);
    }
    Ok(filter)
}

/// Parse the optional `kind` array into a set of call-family [`EdgeKind`]s. `None`
/// (absent) leaves the default call-family scope; an empty array is also `None`.
fn parse_edge_kinds(args: &Value) -> Result<Option<Vec<EdgeKind>>, ToolError> {
    let arr = match args.get("kind") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(a)) => a,
        Some(_) => {
            return Err(ToolError::invalid_params(
                "`kind` must be an array of edge-kind tokens",
            ))
        }
    };
    let mut kinds = Vec::with_capacity(arr.len());
    for v in arr {
        let token = v.as_str().ok_or_else(|| {
            ToolError::invalid_params("`kind` entries must be edge-kind strings")
        })?;
        let kind = CALL_EDGE_KINDS
            .iter()
            .find(|(t, _)| *t == token)
            .map(|(_, k)| *k)
            .ok_or_else(|| {
                ToolError::invalid_params(format!("unknown edge kind `{token}`"))
            })?;
        kinds.push(kind);
    }
    Ok(if kinds.is_empty() { None } else { Some(kinds) })
}

/// Parse the optional `kind` string into a symbol-kind filter (`search`/`symbols`).
/// `all`/absent means no narrowing.
fn parse_symbol_kind(args: &Value) -> Result<Option<SymbolKind>, ToolError> {
    match opt_str(args, "kind").unwrap_or("all") {
        "all" => Ok(None),
        "function" => Ok(Some(SymbolKind::Function)),
        "method" => Ok(Some(SymbolKind::Method)),
        "field" => Ok(Some(SymbolKind::Field)),
        "type" => Ok(Some(SymbolKind::Type)),
        "module" => Ok(Some(SymbolKind::Module)),
        other => Err(ToolError::invalid_params(format!("unknown kind `{other}`"))),
    }
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

/// Attach the A3/A4 answer-honesty contract to a result envelope. The contract is
/// serialized straight off the shared `cgx_query` type, so the MCP tool emits the
/// same `approximation` schema the CLI `--format json` surface does (the MCP tools
/// reuse the query layer, so they inherit the contract verbatim).
fn with_contract(mut body: Value, contract: &ApproximationContract) -> Value {
    let obj = body.as_object_mut().expect("result body is an object");
    obj.insert(
        "approximation".into(),
        serde_json::to_value(contract).expect("approximation contract serializes"),
    );
    body
}

/// Attach the ADR-06 honesty metadata plus the index-freshness envelope to a
/// result envelope.
///
/// This is the one function every **graph-backed** tool's response passes through —
/// deliberately not the `with_contract` sibling above, which `explain`/`search`/
/// `symbols` skip. Anything that must ride on every graph-derived answer belongs
/// here; `crates/cgx-mcp/tests/dispatch.rs` enumerates `tool_list()` and fails if
/// such a tool's response ever misses the `freshness` key.
///
/// `coupling` does **not** pass through here, and that is the contract, not an
/// oversight: it answers from committed git history with no index open, so an
/// index-freshness envelope on its answer would describe a store it never read.
/// The test's `INDEX_FREE_TOOLS` is the closed list of such tools.
fn with_session_meta(mut body: Value, session: &GraphSession) -> Value {
    let obj = body.as_object_mut().expect("result body is an object");
    obj.insert("graph_version".into(), json!(session.graph_version));
    obj.insert("dirty".into(), json!(session.dirty));
    obj.insert(
        "dirty_files_analyzed".into(),
        json!(session.dirty_files_analyzed),
    );
    obj.insert(
        "freshness".into(),
        serde_json::to_value(&session.freshness).expect("freshness envelope serializes"),
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
    let approximation = contract::for_neighbors(&session.view, &walker, anchor, dir, &results);

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
    Ok(with_session_meta(with_contract(body, &approximation), &session))
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
    if let Some(c) = opt_str(args, "confidence") {
        filter = filter.with_min_confidence(parse_confidence(c)?);
    }
    let walker = PathWalker {
        filter,
        max_depth: Some(opt_u32(args, "max_depth")?.unwrap_or(DEFAULT_PATHS_MAX_DEPTH)),
        max_paths: None,
        max_steps: None,
    };
    let result = query_paths(&session.view, from_id, to_id, &walker);
    let approximation = contract::for_paths(&session.view, &walker, from_id, &result);
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
    Ok(with_session_meta(with_contract(body, &approximation), &session))
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
    let approximation = contract::for_unused(
        &session.view,
        &walker,
        &entrypoint_roots(&session.view, &entrypoints),
    );

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
    Ok(with_session_meta(with_contract(body, &approximation), &session))
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
                "peer_line": e.peer.line_start,
                "tier": tier_token(e.tier),
                "rule": e.rule,
                "resolution_source": e.resolution_source,
                "site": e.site.as_ref().map(|s| json!({ "file": s.file, "line": s.line }))
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

/// `reaches` (Q-19): with `to`, a single reachability answer plus its witness
/// path; without `to`, the `from → *` reachable set (identical machinery to
/// `callees`, bounded by the forest depth default). Same engine as `cgx reaches`.
fn reaches_call(args: &Value) -> Result<Value, ToolError> {
    let from = req_str(args, "from")?;
    let session = session_for(args)?;
    let view = &session.view;
    let from_id = resolve(view, from)?;

    let mut filter = EdgeFilter::calls();
    if let Some(c) = opt_str(args, "confidence") {
        filter = filter.with_min_confidence(parse_confidence(c)?);
    }

    match opt_str(args, "to").filter(|s| !s.is_empty()) {
        // `from → to`: reachability with a shortest witness path.
        Some(to) => {
            let to_id = resolve(view, to)?;
            let walker = PathWalker {
                filter,
                max_depth: opt_u32(args, "depth")?,
                max_paths: None,
                max_steps: None,
            };
            let result: ReachResult = reaches(view, from_id, to_id, &walker);
            let approximation = contract::for_reaches(view, &walker, from_id, &result);
            let body = json!({
                "from": from,
                "to": to,
                "reachable": result.reachable,
                "witness": result.witness.as_ref().map(path_json),
            });
            Ok(with_session_meta(with_contract(body, &approximation), &session))
        }
        // `from → *`: every reachable symbol, paginated.
        None => {
            let walker = PathWalker {
                filter,
                max_depth: Some(opt_u32(args, "depth")?.unwrap_or(DEFAULT_FOREST_DEPTH)),
                max_paths: None,
                max_steps: None,
            };
            let results = reaches_all(view, from_id, &walker);
            let approximation =
                contract::for_neighbors(view, &walker, from_id, Direction::Forward, &results);
            let limit = page_size(args, DEFAULT_MAX_RESULTS)?;
            let offset = cursor_offset(args)?;
            let (start, end, has_more, cursor) = paginate(results.len(), offset, limit);
            let page: Vec<Value> = results[start..end].iter().map(neighbor_json).collect();
            let body = json!({
                "from": from,
                "results": page,
                "total_matched": results.len(),
                "has_more": has_more,
                "cursor": cursor,
            });
            Ok(with_session_meta(with_contract(body, &approximation), &session))
        }
    }
}

/// `search` (B-1): a pure node-table scan resolving a name pattern (or `all`) to
/// exact definitions. Reuses `cgx_query::search_symbols`; empty/invalid patterns
/// return an actionable `invalid_params` (the CLI's exit-2 usage path).
fn search_call(args: &Value) -> Result<Value, ToolError> {
    let session = session_for(args)?;
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let regex = args.get("regex").and_then(Value::as_bool).unwrap_or(false);
    let pattern = opt_str(args, "pattern");

    let selector = match (all, pattern) {
        (true, Some(_)) => {
            return Err(ToolError::invalid_params(
                "pass either `pattern` or `all`, not both",
            ))
        }
        (false, None) => {
            return Err(ToolError::invalid_params(
                "a `pattern` is required (or set `all` to list every symbol)",
            ))
        }
        (true, None) => SearchMatch::All,
        (false, Some(p)) => SearchMatch::Pattern { pattern: p, regex },
    };

    let kind_filter = parse_symbol_kind(args)?;
    let hits = search_symbols(&session.view, selector, kind_filter)
        .map_err(|e| ToolError::invalid_params(e.to_string()))?;

    let limit = page_size(args, DEFAULT_MAX_RESULTS)?;
    let offset = cursor_offset(args)?;
    let (start, end, has_more, cursor) = paginate(hits.len(), offset, limit);
    let page: Vec<Value> = hits[start..end].iter().map(symbol_hit_json).collect();
    let body = json!({
        "results": page,
        "total_matched": hits.len(),
        "has_more": has_more,
        "cursor": cursor,
    });
    Ok(with_session_meta(body, &session))
}

/// `symbols` (B-2): rank symbols by reference count with a per-symbol edge
/// breakdown. Reuses `cgx_query::rank_symbols`; same shape the CLI JSON emits.
fn symbols_call(args: &Value) -> Result<Value, ToolError> {
    let session = session_for(args)?;
    let rank_by = match opt_str(args, "rank").unwrap_or("total") {
        "total" => RankBy::Total,
        "inbound" => RankBy::Inbound,
        "outbound" => RankBy::Outbound,
        other => return Err(ToolError::invalid_params(format!("unknown rank `{other}`"))),
    };
    let kind_filter = parse_symbol_kind(args)?;
    let ranks = rank_symbols(&session.view, rank_by, kind_filter);

    let limit = page_size(args, DEFAULT_MAX_RESULTS)?;
    let offset = cursor_offset(args)?;
    let (start, end, has_more, cursor) = paginate(ranks.len(), offset, limit);
    let page: Vec<Value> = ranks[start..end].iter().map(symbol_rank_json).collect();
    let body = json!({
        "results": page,
        "total_matched": ranks.len(),
        "has_more": has_more,
        "cursor": cursor,
    });
    Ok(with_session_meta(body, &session))
}

/// Data-flow direction for `flows_to` / `flows_from`.
enum FlowDir {
    /// `flows_to`: the forward slice — a value's DerivesFrom consumers, reached by
    /// walking DerivesFrom **backward** (via `callers`).
    To,
    /// `flows_from`: the pedigree — what a value derives from, reached by walking
    /// DerivesFrom **forward** (via `callees`).
    From,
}

/// `flows_to` / `flows_from` (docs/04): the same neighbor walk as
/// `callers`/`callees` with the edge scope swapped from the call family to
/// `DerivesFrom` — no second execution path, mirroring `cgx flows-to`/`flows-from`.
fn flow_call(args: &Value, dir: FlowDir) -> Result<Value, ToolError> {
    let symbol = req_str(args, "symbol")?;
    let session = session_for(args)?;
    let view = &session.view;
    let anchor = resolve(view, symbol)?;

    let mut filter = EdgeFilter::default().with_kinds(vec![EdgeKind::DerivesFrom]);
    if let Some(c) = opt_str(args, "confidence") {
        filter = filter.with_min_confidence(parse_confidence(c)?);
    }
    let walker = PathWalker {
        filter,
        max_depth: Some(opt_u32(args, "depth")?.unwrap_or(DEFAULT_FOREST_DEPTH)),
        max_paths: None,
        max_steps: None,
    };
    let (walk_dir, results) = match dir {
        FlowDir::To => (Direction::Backward, callers(view, anchor, &walker)),
        FlowDir::From => (Direction::Forward, callees(view, anchor, &walker)),
    };
    let approximation = contract::for_neighbors(view, &walker, anchor, walk_dir, &results);

    let limit = page_size(args, DEFAULT_MAX_RESULTS)?;
    let offset = cursor_offset(args)?;
    let (start, end, has_more, cursor) = paginate(results.len(), offset, limit);
    let page: Vec<Value> = results[start..end].iter().map(neighbor_json).collect();
    let body = json!({
        "symbol": symbol,
        "results": page,
        "total_matched": results.len(),
        "has_more": has_more,
        "cursor": cursor,
    });
    Ok(with_session_meta(with_contract(body, &approximation), &session))
}

// --- search/symbols result renderers ------------------------------------------

fn symbol_hit_json(h: &SymbolHit) -> Value {
    json!({
        "fqn": h.fqn,
        "file": h.file,
        "line": h.line,
        "kind": format!("{:?}", h.kind).to_lowercase(),
    })
}

fn symbol_rank_json(r: &SymbolRank) -> Value {
    json!({
        "fqn": r.fqn,
        "file": r.file,
        "line": r.line,
        "kind": format!("{:?}", r.kind).to_lowercase(),
        "in_degree": r.in_degree,
        "out_degree": r.out_degree,
        "inbound": breakdown_json(&r.inbound),
        "outbound": breakdown_json(&r.outbound),
    })
}

fn breakdown_json(b: &EdgeBreakdown) -> Value {
    json!({
        "total": b.total,
        "by_family": b.by_family,
        "by_condition": b.by_condition,
        "by_confidence": b.by_confidence,
    })
}

/// `graph_query` (P8): route to the live CQL engine (`cgx_cql::run`) — the same
/// engine `cgx query` drives, no fork. A tabular result surfaces `columns`/`rows`;
/// a `RETURN path` query surfaces a `paths` channel (the `paths`-tool shape).
/// Parse/plan/eval rejects map to an actionable `invalid_params` error.
fn graph_query_call(args: &Value) -> Result<Value, ToolError> {
    let query = req_str(args, "query")?;
    let session = session_for(args)?;
    let view = &session.view;

    let table =
        cgx_cql::run(view, query).map_err(|e| ToolError::invalid_params(e.to_string()))?;

    let limit = page_size(args, DEFAULT_MAX_RESULTS)?;
    let offset = cursor_offset(args)?;
    let truncated = table.truncation.is_some();
    let truncation_reason = table.truncation.map(|r| r.token());

    // A `RETURN path` query populates the path channel: surface paths (same shape
    // as the `paths` tool) rather than opaque table cells.
    if !table.paths.is_empty() {
        // Reuse the intrinsic path-set over/truncation reasons (a CQL query carries
        // its scope intrinsically, so no negative-scope envelope).
        let set = cgx_query::PathSet {
            paths: table.paths.iter().map(|p| cql_path_result(view, p)).collect(),
            truncation: table.truncation,
        };
        let approximation = contract::for_path_set(&set);
        let all: Vec<Value> = table.paths.iter().map(|p| cql_path_json(view, p)).collect();
        let (start, end, has_more, cursor) = paginate(all.len(), offset, limit);
        let body = json!({
            "columns": table.columns,
            "paths": all[start..end].to_vec(),
            "total_matched": all.len(),
            "has_more": has_more,
            "cursor": cursor,
            "truncated": truncated,
            "truncation_reason": truncation_reason,
        });
        return Ok(with_session_meta(with_contract(body, &approximation), &session));
    }

    let approximation = contract::over_only(table_has_over_approx_edge(view, &table));
    let all: Vec<Value> = table
        .rows
        .iter()
        .map(|row| Value::Array(row.iter().map(|v| cql_cell_json(view, v)).collect()))
        .collect();
    let (start, end, has_more, cursor) = paginate(all.len(), offset, limit);
    let body = json!({
        "columns": table.columns,
        "rows": all[start..end].to_vec(),
        "total_matched": all.len(),
        "has_more": has_more,
        "cursor": cursor,
        "truncated": truncated,
        "truncation_reason": truncation_reason,
    });
    Ok(with_session_meta(with_contract(body, &approximation), &session))
}

/// `coupling`: co-change over an explicit commit range, straight off
/// [`cgx_diff::coupling`] — the same typed report `cgx coupling --format json`
/// serializes, so both surfaces emit one schema from one type.
///
/// **Two deliberate departures from every other handler here, both decisions:**
///
/// 1. **No `with_contract(..)`.** The approximation contract already *is* a field
///    of `CouplingReport`, and it serializes through `serde_json::to_value` to the
///    same bytes `with_contract` would insert; calling it would emit the
///    `approximation` key twice.
/// 2. **No `session_for(..)`/`with_session_meta(..)`.** `graph_version`/`dirty`/
///    `dirty_files_analyzed` describe the ADR-06 working-tree overlay on the call
///    graph. Coupling never opens the graph and needs no index to exist at all, so
///    `session_for` would trigger an unnecessary — possibly failing — index for an
///    answer that does not use it, and `dirty: false` would assert something about
///    a graph this answer never consulted.
fn coupling_call(args: &Value) -> Result<Value, ToolError> {
    let root = req_str(args, "root")?;
    let base = req_str(args, "base")?;
    let head = req_str(args, "head")?;

    let defaults = cgx_diff::CouplingOptions::default();
    let options = cgx_diff::CouplingOptions {
        max_files_per_commit: opt_usize(args, "max_files_per_commit")?
            .unwrap_or(defaults.max_files_per_commit),
        min_cochanges: opt_u32(args, "min_cochanges")?.unwrap_or(defaults.min_cochanges),
        limit: opt_usize(args, "limit")?.unwrap_or(defaults.limit),
    };

    let repo = cgx_diff::BlameRepo::discover(std::path::Path::new(root))
        .map_err(|e| ToolError::invalid_params(format!("discovering repo {root:?}: {e}")))?;
    let report = cgx_diff::coupling(&repo, base, head, &options)
        .map_err(|e| ToolError::invalid_params(format!("coupling {base:?}..{head:?}: {e}")))?;

    serde_json::to_value(&report)
        .map_err(|e| ToolError::invalid_params(format!("serializing coupling report: {e}")))
}

/// Whether any bound edge cell in a CQL result was resolved over an
/// over-approximated candidate set (`possible` confidence) — the cheaply-derivable
/// over-approximation signal for a CQL table answer.
fn table_has_over_approx_edge(view: &GraphView, table: &cgx_cql::ResultTable) -> bool {
    fn cell_over(view: &GraphView, v: &cgx_cql::Value) -> bool {
        use cgx_cql::Value as V;
        match v {
            V::Edge(id) => view
                .edge(*id)
                .is_some_and(|e| e.confidence == Confidence::Possible),
            V::List(items) => items.iter().any(|i| cell_over(view, i)),
            _ => false,
        }
    }
    table
        .rows
        .iter()
        .flatten()
        .any(|v| cell_over(view, v))
}

/// Resolve one CQL result-table cell to self-contained JSON (node/edge/path ids
/// become their record fields) — the same contract `cgx query --format json` emits.
fn cql_cell_json(view: &GraphView, v: &cgx_cql::Value) -> Value {
    use cgx_cql::Value as V;
    match v {
        V::Null => Value::Null,
        V::Bool(b) => json!(b),
        V::Int(i) => json!(i),
        V::Float(f) => json!(f),
        V::Str(s) => json!(s),
        V::List(items) => Value::Array(items.iter().map(|i| cql_cell_json(view, i)).collect()),
        V::Node(id) => match view.try_node(*id) {
            Some(n) => json!({
                "fqn": n.fqn,
                "file": n.file,
                "line": n.line_start,
                "kind": format!("{:?}", n.kind).to_lowercase(),
            }),
            None => Value::Null,
        },
        V::Edge(id) => match view.edge(*id) {
            Some(e) => json!({
                "kind": cgx_cql::eval::edge_kind_token(e.kind),
                "condition": condition_token(e.condition),
                "confidence": confidence_token(e.confidence),
            }),
            None => Value::Null,
        },
        V::Path(p) => Value::Array(
            p.nodes
                .iter()
                .map(|n| match view.try_node(*n) {
                    Some(rec) => json!(rec.fqn),
                    None => json!(""),
                })
                .collect(),
        ),
    }
}

/// Rebuild a Layer-1 [`PathResult`] from a CQL `RETURN path` binding so a
/// `graph_query` path reuses the exact Layer-1 shape (rendering *and* the
/// approximation contract) rather than a fork.
fn cql_path_result(view: &GraphView, p: &cgx_cql::PathValue) -> PathResult {
    use cgx_query::PathStep;
    let mut steps = Vec::with_capacity(p.nodes.len());
    let mut min_confidence = Confidence::Certain;
    let mut crosses_exceptional = false;
    for (i, node) in p.nodes.iter().enumerate() {
        let rec = match view.try_node(*node) {
            Some(r) => r.clone(),
            None => continue,
        };
        let via = if i == 0 {
            None
        } else {
            p.edges.get(i - 1).and_then(|e| view.edge(*e)).cloned()
        };
        if let Some(e) = &via {
            min_confidence = min_confidence.min(e.confidence);
            if e.condition.is_exceptional() {
                crosses_exceptional = true;
            }
        }
        let exception_transient = crosses_exceptional;
        steps.push(PathStep {
            node: rec,
            via,
            exception_transient,
        });
    }
    PathResult {
        steps,
        min_confidence,
        crosses_exceptional,
    }
}

/// Render a CQL path through the shared [`path_json`], matching the `paths` tool.
fn cql_path_json(view: &GraphView, p: &cgx_cql::PathValue) -> Value {
    path_json(&cql_path_result(view, p))
}
