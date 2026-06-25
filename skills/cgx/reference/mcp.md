---
title: cgx MCP Reference
audience: AI agents driving cgx over STDIO MCP
since: v0.1 (core tools)
---

# cgx MCP Reference

**Authoritative, comprehensive reference:** [`docs/mcp-tools/`](../../../docs/mcp-tools/)
and its [`README`](../../../docs/mcp-tools/README.md) (6 tool pages with full input/output
schemas and error semantics). This file is a cheat-sheet — look up edge cases there.

This file covers the MCP STDIO interface for AI agents driving cgx programmatically.
For CLI flag detail, see `reference/cli.md`. For CQL syntax, see `reference/query-language.md`.
For MCP-specific usage patterns, see `recipes/ai-agent.md`.

Run `cgx --version` first. The current shipped binary is v0.3.0.

---

## Starting the server

```bash
cgx mcp                        # serve from CWD
cgx mcp --root /path/to/repo   # serve a specific root
```

The server speaks MCP 2025-06-18 STDIO transport (NDJSON over stdin/stdout). One process,
one client. Configure in `.mcp.json` or your agent harness:

```json
{
  "cgx": {
    "command": "cgx",
    "args": ["mcp"],
    "cwd": "/path/to/project"
  }
}
```

`--root` sets the default repository root for all tools. Individual tool calls may override
it with their own `root` parameter.

---

## Tool surface — Since: v0.1

Six tools are registered. **Five are functional.** One (`graph_query`) always errors unimplemented.

| Tool | Status | Required args | Optional args |
|------|--------|---------------|---------------|
| `callers` | functional | `symbol`, `root` | `depth` (default 1), `max_results` (default 20), `cursor`, `edge_condition`, `confidence`, `include_dirty` (default true) |
| `callees` | functional | `symbol`, `root` | same as `callers` |
| `paths` | functional | `from`, `to`, `root` | `max_depth` (default 10), `max_results` (default 10), `confidence`, `exclude_edge_condition`, `only_edge_condition`, `cursor`, `include_dirty` (default true) |
| `unused` | functional | `root` | `kind` (`function`/`method`/`field`/`all`, default `all`), `entrypoint`, `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `explain` | functional | `symbol`, `root` | `include_dirty` (default true) |
| `graph_query` | **UNIMPLEMENTED** | `query`, `root` | `max_results` (default 20), `cursor`, `include_dirty` (default true) — all ignored (always errors) |

### graph_query caveat

`graph_query` is listed in `tools/list` but **always returns a ToolError** — it is not
implemented in v0.3.0 or any released version. For arbitrary CQL queries, shell out to the CLI:

```bash
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_fn" RETURN a.name LIMIT 10'
```

A working `graph_query` MCP tool is a post-Phase-1 deliverable with no scheduled release date.

---

## MCP vs CLI defaults

These defaults differ between the MCP tools and the equivalent CLI subcommands. Using MCP
defaults without adjusting them can produce narrower or different results than the CLI.

| Parameter | MCP default | CLI equivalent | CLI default |
|-----------|-------------|----------------|-------------|
| `depth` (callers/callees) | **1** | `--depth` | 2 (forest) |
| `max_depth` (paths) | **10** | `--depth` | 6 |
| `include_dirty` | **true** | (no CLI flag) | false |
| `max_results` (callers/callees/unused) | **20** | — | unlimited |
| `max_results` (paths) | **10** | — | unlimited |

`include_dirty: true` overlays uncommitted working-tree changes via a per-call
content-addressed overlay that is never persisted to the index. The `dirty` and
`dirty_files_analyzed` envelope fields report whether dirty files were analyzed.

---

## Result envelope

Every functional tool response includes these envelope fields in `structuredContent`:

| Field | Type | Meaning |
|-------|------|---------|
| `graph_version` | string | Commit SHA of the indexed HEAD, or `"<sha>+dirty.<digest>"` when dirty files were analyzed |
| `dirty` | boolean | True when uncommitted working-tree changes were included |
| `dirty_files_analyzed` | integer | Count of dirty files overlaid in this call |
| `has_more` | boolean | True when more results exist beyond this page |
| `cursor` | string or null | Opaque pagination token; pass as `cursor` in the next call |

Example envelope (callers result, page 1 of 2):

```json
{
  "structuredContent": {
    "results": [ ... ],
    "has_more": true,
    "cursor": "20",
    "graph_version": "abc1234",
    "dirty": false,
    "dirty_files_analyzed": 0
  }
}
```

---

## Pagination

All tools that return multiple results support cursor pagination.

- **Default page size:** 20 results (10 for `paths`).
- **Hard cap:** 200 results per call (`max_results` ≤ 200).
- **Cursor:** opaque decimal offset string returned in `cursor`. Pass it unchanged as
  `cursor` in the next call to fetch the next page.
- **Stop condition:** `has_more: false` (or `cursor: null`).

```json
// First call
{ "symbol": "db::query", "root": "/workspace", "max_results": 20 }

// Next page
{ "symbol": "db::query", "root": "/workspace", "max_results": 20, "cursor": "20" }
```

---

## Tool schemas

### callers

Symbols that (transitively, to `depth`) call the named symbol.

**Note:** The `at` (git ref) parameter is not in the MCP input schema. Passing `at`
is silently ignored. Use the CLI `cgx callers --at <ref>` for ref-pinned queries.

```json
{
  "name": "callers",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":         { "type": "string", "description": "Qualified symbol name" },
      "root":           { "type": "string", "description": "Repository root path" },
      "depth":          { "type": "integer", "default": 1 },
      "max_results":    { "type": "integer", "default": 20 },
      "cursor":         { "type": "string" },
      "edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":     { "type": "string", "enum": ["certain","probable","possible"] },
      "include_dirty":  { "type": "boolean", "default": true }
    }
  }
}
```

### callees

Symbols the named symbol (transitively, to `depth`) calls. Identical schema to `callers`.

```json
{
  "name": "callees",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":         { "type": "string" },
      "root":           { "type": "string" },
      "depth":          { "type": "integer", "default": 1 },
      "max_results":    { "type": "integer", "default": 20 },
      "cursor":         { "type": "string" },
      "edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":     { "type": "string", "enum": ["certain","probable","possible"] },
      "include_dirty":  { "type": "boolean", "default": true }
    }
  }
}
```

### paths

Enumerate call paths from one symbol to another, surfacing per-path confidence and
exceptional-class crossing.

**Note:** MCP `paths` uses `max_depth` (not `depth`). The CLI equivalent flag is
`--depth`. Passing `depth` to this tool is silently ignored.

```json
{
  "name": "paths",
  "inputSchema": {
    "type": "object",
    "required": ["from", "to", "root"],
    "properties": {
      "from":                   { "type": "string", "description": "Source symbol (FQN or short name)" },
      "to":                     { "type": "string", "description": "Target symbol (FQN or short name)" },
      "root":                   { "type": "string" },
      "max_depth":              { "type": "integer", "default": 10 },
      "max_results":            { "type": "integer", "default": 10 },
      "exclude_edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "only_edge_condition":    { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":             { "type": "string", "enum": ["certain","probable","possible"] },
      "cursor":                 { "type": "string" },
      "include_dirty":          { "type": "boolean", "default": true }
    }
  }
}
```

### unused

Finds symbols not reachable from any entrypoint.

```json
{
  "name": "unused",
  "inputSchema": {
    "type": "object",
    "required": ["root"],
    "properties": {
      "root":          { "type": "string" },
      "kind":          { "type": "string", "enum": ["function","method","field","all"], "default": "all" },
      "entrypoint":    { "type": "string", "description": "Restrict reachability root to this symbol" },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Note: `unused` has no name or pattern filter. To find unused symbols in a specific module,
run `unused` then grep its output. There is no one-shot flag for this.

### explain

Returns the full provenance record for one symbol: definition location, caller count,
callee count, all edges with their condition and confidence labels, and index metadata.
Use this for a one-shot overview of a symbol before deciding which follow-up queries to run.

**Note:** `explain` has no `depth`, `max_results`, `cursor`, `edge_condition`, `confidence`,
or `at` parameters. All incident edges are returned in one call; there is no pagination.

```json
{
  "name": "explain",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":        { "type": "string" },
      "root":          { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Each element of the `edges` array uses `peer`/`peer_file`/`peer_line` for the other
symbol on the edge (not `from`/`from_file`/`from_line`).

Example response:

```json
{
  "structuredContent": {
    "symbol": "rust_sample::conditions::dispatch",
    "file": "fixtures/rust-sample/src/conditions.rs",
    "line": 42,
    "kind": "function",
    "callers_count": 3,
    "callees_count": 3,
    "edges": [
      {
        "direction": "incoming",
        "peer": "ts_sample::closures::closureVariable",
        "peer_file": "fixtures/ts-sample/src/closures.ts",
        "peer_line": 6,
        "condition": "always",
        "confidence": "possible",
        "tier": "cha_rta",
        "rule": "sig-compat",
        "resolution_source": null,
        "site": { "file": "fixtures/ts-sample/src/closures.ts", "line": 6 }
      },
      {
        "direction": "outgoing",
        "peer": "rust_sample::conditions::log_info",
        "peer_file": "fixtures/rust-sample/src/conditions.rs",
        "peer_line": 4,
        "condition": "conditional",
        "confidence": "certain",
        "tier": "scope_graph",
        "rule": "scope-ref",
        "resolution_source": null,
        "site": { "file": "fixtures/rust-sample/src/conditions.rs", "line": 42 }
      }
    ],
    "graph_version": "abc1234",
    "dirty": false,
    "dirty_files_analyzed": 0
  }
}
```

---

## Token-efficiency guidance

Token budgets constrain agent context. Follow these practices:

| Practice | How |
|----------|-----|
| Start with `explain` | One call returns caller count, callee count, and all edges. Decide whether deeper queries are needed before paginating. |
| Use small `depth` | MCP default is 1. Increase only when you need transitive results. Depth 2–3 is usually enough. |
| Set `max_results` explicitly | Default 20 is reasonable. Cap at 50 unless you need the full set. Hard cap is 200. |
| Paginate on demand | Check `has_more`; fetch the next page only if the current page does not answer the question. |
| Filter by `edge_condition` | Use `edge_condition: "exception"` to see only exceptional paths. Narrow the result set before paginating. |
| Avoid `graph_query` | It always errors on call. Use CLI `cgx query` for arbitrary CQL. |

For a complete worked agent session using these tools, see `recipes/ai-agent.md`.
