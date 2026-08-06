---
title: cgx MCP Reference
audience: AI agents driving cgx over STDIO MCP
since: mixed — per tool/capability; see "Tool surface" below
---

# cgx MCP Reference

**Historical reference, superseded by this file:** [`docs/mcp-tools/`](../../../docs/mcp-tools/)
and its [`README`](../../../docs/mcp-tools/README.md). It carries per-tool schemas and error
semantics for the original six tools, but it is stale in three ways, two of them worse than
an undercount — as of this writing it states:

- it documents 6 of the 11 tools registered today;
- "`graph_query` is unimplemented … every `tools/call` invocation returns a `ToolError`"
  ([README.md:24-27](../../../docs/mcp-tools/README.md)) — **false**: `graph_query` routes to
  the live CQL engine (see "graph_query" below);
- "`flows-to` and `flows-from` are CLI-only … not exposed as MCP tools in v0.3"
  ([README.md:33](../../../docs/mcp-tools/README.md)) — **false**: both are registered and
  dispatched.

Prefer this file. Consult that one only for the base-six edge cases it still describes
accurately, and never for what does or does not exist.

This file covers the MCP STDIO interface for AI agents driving cgx programmatically.
For CLI flag detail, see `reference/cli.md`. For CQL syntax, see `reference/query-language.md`.
For MCP-specific usage patterns, see `recipes/ai-agent.md`.

Run `cgx --version` first. The current shipped binary is v0.3.0 — but that string has not
moved since `graph_query`'s live CQL execution, the `reaches`/`search`/`symbols`/`flows_to`/
`flows_from` tools, the `kind` edge-filter, and the approximation contract shipped on `main`.
`cgx --version` cannot discriminate any of them, and **no single check covers all four**.
They landed in the order below, each needs its own probe, and a lower one passing does not
imply a higher one:

1. **Live `graph_query`** — call it with a trivial query and see whether a table comes back
   instead of an error. A binary at this point still registers **6** tools and has no `search`.
2. **The five added tools** — `tools/list` returns 11 entries with `search` present. A binary
   at this point still has **no** `kind` filter.
3. **The `kind` edge-filter** — a `kind` property in the `callers` `inputSchema`.
4. **The approximation contract** — an `approximation` object in a `callers` result.

Check 3 by hand and not by inference. The server performs **no** schema validation on tool
arguments, so passing `kind` to a binary that predates it is silently dropped and an
**unfiltered** answer comes back with no error.

---

## Starting the server

```bash
cgx mcp                        # no default root — every tool call must carry its own `root`
cgx mcp --root /path/to/repo   # `root` is injected into calls that omit it
```

**There is no CWD fallback.** `cgx mcp` without `--root` sets no default: `root` is a required
argument on all eleven tool schemas, and a call that omits it fails. The MCP crate reads no
working directory and no environment variable — the root comes from `--root` or from the call.

The server speaks MCP 2025-06-18 STDIO transport (NDJSON over stdin/stdout). One process,
one client. Configure in `.mcp.json` or your agent harness:

```json
{
  "cgx": {
    "command": "cgx",
    "args": ["mcp", "--root", "/path/to/project"],
    "cwd": "/path/to/project"
  }
}
```

The `--root` argument is what sets the graph root. `cwd` is a property of the client's process
launch and cgx never consults it; without `--root`, every call must supply `root` itself.
(The surrounding shape of `.mcp.json` — wrapper keys, field names — is a property of your
client harness, not of cgx, which neither reads nor validates this file.)

`--root` sets the *default* repository root. A tool call that supplies its own `root` always
wins; the default is injected only when the argument is absent or empty.

---

## Tool surface

11 tools are registered. **All are functional**, including `graph_query` (see below).

The base 6 (`callers`, `callees`, `paths`, `unused`, `explain`, `graph_query`) shipped in
v0.1. `reaches`, `search`, `symbols`, `flows_to`, `flows_from`, the `kind` edge-filter on
`callers`/`callees`, and `graph_query`'s live CQL execution all ship on `main` ahead of the
`0.3.0` version string — do not gate them on `Since: v0.3` (see the version note above).

| Tool | Required args | Optional args |
|------|---------------|----------------|
| `callers` | `symbol`, `root` | `depth` (default 1), `max_results` (default 20), `cursor`, `edge_condition`, `confidence`, `kind`, `include_dirty` (default true) |
| `callees` | `symbol`, `root` | same as `callers` |
| `reaches` | `from`, `root` | `to`, `depth` (declared default 2 — applied on the `from → *` form only; **with `to` set, an omitted `depth` walks unbounded**), `confidence`, `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `paths` | `from`, `to`, `root` | `max_depth` (default 10), `max_results` (default 10), `confidence`, `exclude_edge_condition`, `only_edge_condition`, `cursor`, `include_dirty` (default true) |
| `unused` | `root` | `kind` (`function`/`method`/`field`/`all`, default `all`), `entrypoint`, `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `explain` | `symbol`, `root` | `include_dirty` (default true) |
| `search` | `root` | `pattern`, `all` (default false), `regex` (default false), `kind` (`function`/`method`/`field`/`type`/`module`/`all`, default `all`), `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `symbols` | `root` | `rank` (`total`/`inbound`/`outbound`, default `total`), `kind` (same enum as `search`), `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `flows_to` | `symbol`, `root` | `depth` (default 2), `confidence`, `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `flows_from` | `symbol`, `root` | same as `flows_to` |
| `graph_query` | `query`, `root` | `max_results` (default 20), `cursor`, `include_dirty` (default true) |

### graph_query

`graph_query` executes real CQL against the graph: it routes to the same `cgx_cql::run`
engine the `cgx query` CLI subcommand drives — no second execution path. A tabular query
returns `columns`/`rows`; a `RETURN path` query returns a `paths` channel in the same shape
the `paths` tool emits. Parse/plan/eval rejects map to an `invalid_params` error, not a
silent wrong answer.

```json
{ "query": "MATCH (a)-[:CALLS]->(b) WHERE b.name = \"my_fn\" RETURN a.name LIMIT 10",
  "root": "/workspace" }
```

Double quotes are the only string form in CQL. The equivalent CLI invocation is
`cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_fn" RETURN a.name LIMIT 10'`.

`graph_query` **is** paginated — both the table and the path branch page like any other tool
(default `max_results` 20, `cursor`, `has_more`). What it does not do is push your filter down:
it computes the whole result set server-side and then pages it, so a narrower tool
(`callers`/`callees`/`paths`/`unused`/`reaches`) is cheaper whenever one answers the question.

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

Every tool response carries `graph_version`/`dirty`/`dirty_files_analyzed` in
`structuredContent`. `has_more`/`cursor` appear on paginated tools. `approximation` appears
on 8 of the 11 tools — see its row below for the exception list.

| Field | Type | Meaning |
|-------|------|---------|
| `graph_version` | string | Short git **tree** OID of the indexed tree — *not* a commit SHA. `"<tree-oid>+dirty.<digest>"` when the working tree actually differed from `HEAD`. Use it as an opaque cache key; do not feed it to `git show` or compare it against `git rev-parse HEAD` |
| `dirty` | boolean | True when uncommitted working-tree changes were included |
| `dirty_files_analyzed` | integer | Count of dirty files overlaid in this call |
| `has_more` | boolean | True when more results exist beyond this page |
| `cursor` | string or null | Opaque pagination token; pass as `cursor` in the next call |
| `approximation` | object | The A3/A4 approximation contract: `direction` (`exact`/`over`/`under`/`over_under`), `reasons` (machine-readable causes), `modeled_graph` (the standing modeling-boundary string), and `scope` (present only on a negative answer). Present on `callers`/`callees`/`reaches`/`paths`/`unused`/`flows_to`/`flows_from`/`graph_query`. **Not present** on `explain`, `search`, or `symbols` — their handlers attach `graph_version`/`dirty`/`dirty_files_analyzed` only, not the contract. |

Example envelope (callers result, page 1 of 2):

```json
{
  "structuredContent": {
    "results": [ ... ],
    "has_more": true,
    "cursor": "20",
    "graph_version": "abc1234",
    "dirty": false,
    "dirty_files_analyzed": 0,
    "approximation": {
      "direction": "exact",
      "reasons": [],
      "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph"
    }
  }
}
```

---

## Pagination

Cursor pagination is on every multi-result tool except `explain`, which returns all incident
edges in one unbounded call, and `reaches` with `to` set, which returns a single answer.

- **Default page size:** 20 results (10 for `paths`).
- **Hard cap:** 200 results per call. `max_results` above 200 is **silently clamped** to 200,
  not rejected — no error and no envelope field distinguishes "clamped" from "that was all
  there was", so read `has_more` rather than inferring completeness from the row count.
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

**Note:** `kind` restricts traversal to specific call-family edge kinds (default: all
seven — `calls`, `calls_virtual`, `calls_closure`, `calls_callback`, `calls_async`,
`calls_indirect`, `spawns`).

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
      "kind":           { "type": "array", "items": { "type": "string", "enum": ["calls","calls_virtual","calls_closure","calls_callback","calls_async","calls_indirect","spawns"] }, "description": "Restrict traversal to these call-family edge kinds (default: all call edges)" },
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
      "kind":           { "type": "array", "items": { "type": "string", "enum": ["calls","calls_virtual","calls_closure","calls_callback","calls_async","calls_indirect","spawns"] }, "description": "Restrict traversal to these call-family edge kinds (default: all call edges)" },
      "include_dirty":  { "type": "boolean", "default": true }
    }
  }
}
```

### reaches

Reachability over call edges: with `to`, whether `from` reaches `to` (with a witness
path); without `to`, every symbol `from` reaches. Same engine as `cgx reaches`.

**Note:** the declared `depth` default is 2 (the forest-default bound used by
`flows_to`/`flows_from` too), not 1 like `callers`/`callees` — but the schema default is only
applied on the `from → *` form. **With `to` set, an omitted `depth` walks unbounded.** Pass
`depth` explicitly on the witness-path form if you want a bound.

```json
{
  "name": "reaches",
  "inputSchema": {
    "type": "object",
    "required": ["from", "root"],
    "properties": {
      "from":          { "type": "string", "description": "Source symbol (qualified name)" },
      "to":            { "type": "string", "description": "Optional target symbol; omit for the from→* reachable set" },
      "root":          { "type": "string", "description": "Repository root path" },
      "depth":         { "type": "integer", "default": 2, "description": "Max traversal depth (from→* form)" },
      "confidence":    { "type": "string", "enum": ["certain","probable","possible"] },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

With `to` set, the response is `{ "from", "to", "reachable": <bool>, "witness": <path
or null> }` — a single answer, no pagination. Without `to`, the response is the same
paginated `results`/`total_matched`/`has_more`/`cursor` shape as `callers`/`callees`.

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

### search

Resolve a partial/half-remembered name to exact symbol definitions: a pure node-table
scan, no graph walk.

**Note:** pass exactly one of `pattern` or `all`; passing both, or neither, is an
`invalid_params` error. `search`'s `kind` is a symbol-kind string enum (`function`/
`method`/`field`/`type`/`module`/`all`) — distinct from `callers`/`callees`'s `kind`,
which is an array of call-edge-kind tokens.

```json
{
  "name": "search",
  "inputSchema": {
    "type": "object",
    "required": ["root"],
    "properties": {
      "pattern":       { "type": "string", "description": "Name predicate over the whole FQN (case-insensitive substring, or regex when `regex` is true)" },
      "all":           { "type": "boolean", "default": false, "description": "List every symbol (mutually exclusive with `pattern`)" },
      "regex":         { "type": "boolean", "default": false, "description": "Treat `pattern` as an unanchored regex" },
      "root":          { "type": "string" },
      "kind":          { "type": "string", "enum": ["function","method","field","type","module","all"], "default": "all" },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Result rows: `{ "fqn", "file", "line", "kind" }`. No `approximation` field (see
"Result envelope" above).

### symbols

Rank symbols by reference count (the hub/importance lens), each with its inbound/
outbound edge breakdown.

```json
{
  "name": "symbols",
  "inputSchema": {
    "type": "object",
    "required": ["root"],
    "properties": {
      "rank":          { "type": "string", "enum": ["total","inbound","outbound"], "default": "total" },
      "kind":          { "type": "string", "enum": ["function","method","field","type","module","all"], "default": "all" },
      "root":          { "type": "string" },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Result rows: `{ "fqn", "file", "line", "kind", "in_degree", "out_degree", "inbound":
{ "total", "by_family", "by_condition", "by_confidence" }, "outbound": { ... } }`. No
`approximation` field (see "Result envelope" above).

### flows_to / flows_from

`flows_to`: forward data-flow slice — symbols a value flows into (its `DerivesFrom`
consumers). `flows_from`: pedigree — the symbols a value derives from (its
`DerivesFrom` sources). Same neighbor-walk machinery as `callers`/`callees`, with the
edge scope swapped from the call family to `DerivesFrom`; mirrors the CLI's
`flows-to`/`flows-from`.

```json
{
  "name": "flows_to",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":        { "type": "string", "description": "Qualified symbol name" },
      "root":          { "type": "string" },
      "depth":         { "type": "integer", "default": 2 },
      "confidence":    { "type": "string", "enum": ["certain","probable","possible"] },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

`flows_from` declares a byte-identical `inputSchema`; its `name` and `description` differ.

---

## Token-efficiency guidance

Token budgets constrain agent context. Follow these practices:

| Practice | How |
|----------|-----|
| Start with `explain` | One call returns caller count, callee count, and all edges. Decide whether deeper queries are needed before paginating. |
| Use small `depth` | Default is 1 on `callers`/`callees`, 2 on `reaches`/`flows_to`/`flows_from`; `paths` uses `max_depth` 10. Increase only when you need transitive results. Depth 2–3 is usually enough. |
| Set `max_results` explicitly | Default 20 (10 for `paths`) is reasonable. Cap at 50 unless you need the full set. Hard cap 200, silently clamped. |
| Paginate on demand | Check `has_more`; fetch the next page only if the current page does not answer the question. |
| Filter by edge condition — **use the right parameter name** | `edge_condition: "exception"` on `callers`/`callees`. On `paths` the parameters are `only_edge_condition` / `exclude_edge_condition`. Passing `edge_condition` to `paths` is **silently dropped** — the server validates no arguments — and you get an unfiltered path set back with no error. |
| Prefer a narrower tool over `graph_query` | `graph_query` pages like any other tool, but it computes the whole result set before paging it; prefer `callers`/`callees`/`paths`/`unused`/`reaches` when one answers the question more cheaply. |

For a complete worked agent session using these tools, see `recipes/ai-agent.md`.
