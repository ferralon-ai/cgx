---
title: cgx MCP Reference
audience: AI agents driving cgx over STDIO MCP
since: v0.1 (core tools); v0.5 (token-efficiency features; working graph_query)
---

# cgx MCP Reference

This file covers the MCP STDIO interface for AI agents driving cgx programmatically.
For CLI flag detail, see `reference/cli.md`. For CQL syntax, see `reference/query-language.md`.
For MCP-specific usage patterns, see `recipes/ai-agent.md`.

Run `cgx --version` first. A capability tagged `Since: v0.N` is available iff your minor version
is ≥ N. The current shipped binary is v0.1.

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
| `paths` | functional | `from`, `to`, `root` | `max_depth` (default 10), `max_results` (default 10), `exclude_edge_condition`, `only_edge_condition`, `cursor`, `include_dirty` (default true) |
| `unused` | functional | `root` | `kind` (`function`/`method`/`field`/`all`, default `all`), `entrypoint`, `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `explain` | functional | `symbol`, `root` | `include_dirty` (default true) |
| `graph_query` | **UNIMPLEMENTED** | `query`, `root` | — |

### graph_query caveat

`graph_query` is listed in `tools/list` but **always returns a ToolError** — it is not
implemented in v0.1 or any released version. For arbitrary CQL queries, shell out to the CLI:

```bash
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_fn" RETURN a.name LIMIT 10'
```

A working `graph_query` MCP tool (with prompts and subscriptions) is **Since: v0.5**.

---

## MCP vs CLI defaults

These defaults differ between the MCP tools and the equivalent CLI subcommands. Using MCP
defaults without adjusting them can produce narrower or different results than the CLI.

| Parameter | MCP default | CLI equivalent | CLI default |
|-----------|-------------|----------------|-------------|
| `depth` (callers/callees) | **1** | `--max-depth` | unlimited |
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

Finds symbols that call the named symbol, up to `depth` hops.

```json
{
  "name": "callers",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":         { "type": "string", "description": "Fully-qualified symbol name" },
      "root":           { "type": "string", "description": "Repository root path" },
      "depth":          { "type": "integer", "default": 1 },
      "max_results":    { "type": "integer", "default": 20 },
      "cursor":         { "type": "string" },
      "edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":     { "type": "string", "enum": ["certain","probable","possible"] },
      "at":             { "type": "string", "description": "Git ref (default: HEAD)" },
      "include_dirty":  { "type": "boolean", "default": true }
    }
  }
}
```

### callees

Finds symbols the named symbol calls. Identical schema to `callers`.

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
      "at":             { "type": "string" },
      "include_dirty":  { "type": "boolean", "default": true }
    }
  }
}
```

### paths

Finds call paths between two symbols.

```json
{
  "name": "paths",
  "inputSchema": {
    "type": "object",
    "required": ["from", "to", "root"],
    "properties": {
      "from":                   { "type": "string", "description": "Source symbol (fully-qualified)" },
      "to":                     { "type": "string", "description": "Target symbol (fully-qualified)" },
      "root":                   { "type": "string" },
      "max_depth":              { "type": "integer", "default": 10 },
      "max_results":            { "type": "integer", "default": 10 },
      "exclude_edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "only_edge_condition":    { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "cursor":                 { "type": "string" },
      "at":                     { "type": "string" },
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

```json
{
  "name": "explain",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":        { "type": "string" },
      "root":          { "type": "string" },
      "at":            { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Example response:

```json
{
  "structuredContent": {
    "symbol": "crypto::hash",
    "file": "src/crypto.rs",
    "line": 15,
    "callers_count": 12,
    "callees_count": 3,
    "edges": [
      {
        "direction": "incoming",
        "from": "main::baz",
        "condition": "always",
        "confidence": "certain",
        "from_file": "src/main.rs",
        "from_line": 42
      }
    ],
    "graph_version": "abc1234",
    "dirty": false,
    "dirty_files_analyzed": 0
  }
}
```

---

## Token-efficiency guidance — Since: v0.1 (basic); v0.5 (full)

Token budgets constrain agent context. Follow these practices:

| Practice | How |
|----------|-----|
| Start with `explain` | One call returns caller count, callee count, and all edges. Decide whether deeper queries are needed before paginating. |
| Use small `depth` | MCP default is 1. Increase only when you need transitive results. Depth 2–3 is usually enough. |
| Set `max_results` explicitly | Default 20 is reasonable. Cap at 50 unless you need the full set. Hard cap is 200. |
| Paginate on demand | Check `has_more`; fetch the next page only if the current page does not answer the question. |
| Filter by `edge_condition` | Use `edge_condition: "exception"` to see only exceptional paths. Narrow the result set before paginating. |
| Avoid `graph_query` | It always errors. Use CLI `cgx query` for arbitrary CQL. |

Richer token-efficiency features (compact symbol IDs, `resource_link` for bulk evidence,
lazy schema loading via `cgx://schema/{root}`) are **Since: v0.5**.

For a complete worked agent session using these tools, see `recipes/ai-agent.md`.
