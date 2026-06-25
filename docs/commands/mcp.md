# cgx mcp

Start the MCP STDIO server (docs/07 IF-9).

## Synopsis

```
cgx mcp [OPTIONS]
```

## Description

`cgx mcp` starts a long-running [Model Context Protocol](../07-interfaces.md) server that communicates over standard input and output. It exposes the cgx call-graph as a set of MCP tools that AI coding agents, security agents, and IDE extensions can call using standard JSON-RPC 2.0 framing.

The server conforms to the 2025-06-18 MCP specification. It registers six tools — `callers`, `callees`, `paths`, `unused`, `explain`, and `graph_query` — and responds to `tools/list` and `tools/call` requests. Five tools are fully functional; `graph_query` is registered but always returns an error (see [Tools](#tools) below).

Each tool call returns a JSON result envelope containing session metadata alongside the tool-specific result payload. The server does not maintain state between tool calls; every call loads from the on-disk `.cgx/` index.

**MCP vs CLI differences:**

- Default depth for `callers`/`callees` is **1** in MCP (CLI default is 2).
- The `paths` tool uses the property name `max_depth` (CLI flag is `--depth`), and its default is **10** (CLI default is 6).
- `max_results` caps results per page (default 20 for most tools; 10 for `paths`). The hard ceiling is 200; values above 200 are clamped.
- `include_dirty` defaults to `true` — the server analyses uncommitted working-tree changes via a per-call content-addressed overlay. This overlay is never persisted to the index.
- No `at` parameter exists in any MCP tool (git-ref pinning is CLI-only in v0.3).
- `flows-to` and `flows-from` are CLI-only subcommands in v0.3; they are not exposed as MCP tools.

## Arguments

`cgx mcp` takes no positional arguments.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--root` | path | — | Default repository root injected into tool calls that omit the `root` property. When set, agents may omit `root` from individual tool call arguments. When unset, `root` is required in every tool call. |

## Tools

The six registered MCP tools and their input schemas are listed below. `root` and any required symbol argument are marked required; all other properties are optional.

### callers

Symbols that (transitively, to `depth`) call the named symbol.

| Property | Type | Required | Default | Meaning |
|----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Qualified symbol name (FQN or unambiguous short name). |
| `root` | string | yes* | — | Repository root path. May be omitted if `--root` was passed to `cgx mcp`. |
| `depth` | integer | no | `1` | Maximum traversal depth. |
| `max_results` | integer | no | `20` | Page size. Capped at 200. |
| `cursor` | string | no | — | Pagination cursor from a prior response's `cursor` field. `"0"` is a valid first-page value. |
| `edge_condition` | string | no | — | Filter to one edge condition: `always`, `conditional`, `exception`, `loop`, or `panic`. |
| `confidence` | string | no | — | Floor filter: `certain`, `probable`, or `possible`. |
| `include_dirty` | boolean | no | `true` | Include uncommitted working-tree changes in analysis. |

### callees

Symbols the named symbol (transitively, to `depth`) calls. Identical schema to `callers`.

### paths

Enumerate call paths from one symbol to another, surfacing per-path confidence and exceptional-class crossing.

| Property | Type | Required | Default | Meaning |
|----------|------|----------|---------|---------|
| `from` | string | yes | — | Source symbol FQN. |
| `to` | string | yes | — | Target symbol FQN. |
| `root` | string | yes* | — | Repository root path. |
| `max_depth` | integer | no | `10` | Maximum path depth. Note: the CLI equivalent flag is `--depth` with a default of 6. |
| `max_results` | integer | no | `10` | Page size. Capped at 200. |
| `cursor` | string | no | — | Pagination cursor. |
| `exclude_edge_condition` | string | no | — | Omit paths that include this edge condition. |
| `only_edge_condition` | string | no | — | Return only paths that include this edge condition. |
| `confidence` | string | no | — | Minimum confidence floor for path inclusion. |
| `include_dirty` | boolean | no | `true` | Include uncommitted working-tree changes. |

### unused

Symbols not reachable from any entrypoint.

| Property | Type | Required | Default | Meaning |
|----------|------|----------|---------|---------|
| `root` | string | yes* | — | Repository root path. |
| `kind` | string | no | `all` | Filter by kind: `function`, `method`, `field`, or `all`. Note: `all` is MCP-specific; the CLI `--kind` flag does not accept `all`. |
| `entrypoint` | string | no | — | Restrict reachability analysis to a specific entrypoint symbol. |
| `max_results` | integer | no | `20` | Page size. Capped at 200. |
| `cursor` | string | no | — | Pagination cursor. |
| `include_dirty` | boolean | no | `true` | Include uncommitted working-tree changes. |

### explain

Full provenance for one symbol: definition location, caller/callee counts, and every incident edge with its condition and confidence.

| Property | Type | Required | Default | Meaning |
|----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Qualified symbol name. |
| `root` | string | yes* | — | Repository root path. |
| `include_dirty` | boolean | no | `true` | Include uncommitted working-tree changes. |

Edge fields in the response use `peer`, `peer_file`, and `peer_line` (not `from`/`from_file`/`from_line`).

### graph_query (unimplemented)

Registered in `tools/list` but always returns a JSON-RPC error on `tools/call`:

```
method not found: graph_query: the Cypher-subset query language is not implemented in Phase 1; use callers/callees/paths/unused
```

Use `callers`, `callees`, `paths`, or `unused` for all current queries. For CQL-based queries, use the `cgx query` CLI subcommand.

## Response envelope

Every functional tool call includes these session metadata fields alongside the tool-specific payload:

| Field | Type | Meaning |
|-------|------|---------|
| `graph_version` | string | Index commit hash, with `+dirty.<hash>` suffix when `include_dirty` is true and the working tree has changes (ADR-06). |
| `dirty` | boolean | Whether uncommitted changes were analysed. |
| `dirty_files_analyzed` | integer | Count of working-tree files included in the overlay. |
| `has_more` | boolean | Whether additional pages exist (IF-18 pagination). |
| `cursor` | string or null | Pass to the next call to retrieve the next page. |

`callers` and `callees` results also include `total_matched`. Each result item includes: `name`, `file`, `line`, `depth`, `edge_condition`, `confidence`, `min_confidence_on_path`, `exception_transient`.

`paths` results also include `truncated` (boolean) and `truncation_reason` (string or null). Each path item includes: `hops`, `min_confidence`, `crosses_exceptional`, and a `steps` array. Each step includes: `name`, `file`, `line`, `edge_condition`, `confidence`, `exception_transient`.

## Examples

### Start the server

```
cgx mcp
```

The server blocks, reading JSON-RPC 2.0 messages on stdin and writing responses to stdout. Send an `initialize` request first; the server responds with its capabilities and version.

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"my-agent","version":"1.0"}}}
```

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "capabilities": {"tools": {"listChanged": false}},
    "protocolVersion": "2025-06-18",
    "serverInfo": {"name": "cgx", "version": "0.3.0"}
  }
}
```

### Start with a default root

When a single repository is always the target, pass `--root` to avoid repeating it in every tool call:

```
cgx mcp --root /path/to/my-repo
```

Tool calls may then omit the `root` property entirely:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"callers","arguments":{"symbol":"rust_sample::conditions::dispatch","depth":1}}}
```

> Note: `dirty_files_analyzed` and the dirty suffix in `graph_version` are per-invocation values that vary based on working-tree state; the values below are illustrative.

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "content": [{"type": "text", "text": "{\"cursor\":null,\"dirty\":true,\"dirty_files_analyzed\":24,\"graph_version\":\"9c824d0+dirty.140762eb71f4\",\"has_more\":false,\"results\":[{\"confidence\":\"possible\",\"depth\":1,\"edge_condition\":\"always\",\"exception_transient\":false,\"file\":\"fixtures/ts-sample/src/closures.ts\",\"line\":6,\"min_confidence_on_path\":\"possible\",\"name\":\"ts_sample::closures::closureVariable\"},{\"confidence\":\"possible\",\"depth\":1,\"edge_condition\":\"always\",\"exception_transient\":false,\"file\":\"fixtures/ts-sample/src/closures.ts\",\"line\":57,\"min_confidence_on_path\":\"possible\",\"name\":\"ts_sample::closures::nestedClosures\"},{\"confidence\":\"possible\",\"depth\":1,\"edge_condition\":\"always\",\"exception_transient\":false,\"file\":\"fixtures/ts-sample/src/closures.ts\",\"line\":58,\"min_confidence_on_path\":\"possible\",\"name\":\"ts_sample::closures::nestedClosures::outer\"}],\"symbol\":\"rust_sample::conditions::dispatch\",\"total_matched\":3}"}],
    "isError": false
  }
}
```

### Call the paths tool

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"paths","arguments":{"from":"rust_sample::conditions::dispatch","to":"rust_sample::conditions::log_info","root":"/path/to/rust-sample"}}}
```

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "content": [{"type": "text", "text": "{\"cursor\":null,\"dirty\":true,\"dirty_files_analyzed\":26,\"from\":\"rust_sample::conditions::dispatch\",\"graph_version\":\"9c824d0+dirty.a5e1fa3dede3\",\"has_more\":false,\"paths\":[{\"crosses_exceptional\":false,\"hops\":1,\"min_confidence\":\"certain\",\"steps\":[{\"confidence\":null,\"edge_condition\":null,\"exception_transient\":false,\"file\":\"fixtures/rust-sample/src/conditions.rs\",\"line\":42,\"name\":\"rust_sample::conditions::dispatch\"},{\"confidence\":\"certain\",\"edge_condition\":\"conditional\",\"exception_transient\":false,\"file\":\"fixtures/rust-sample/src/conditions.rs\",\"line\":4,\"name\":\"rust_sample::conditions::log_info\"}]}],\"to\":\"rust_sample::conditions::log_info\",\"total_matched\":1,\"truncated\":false,\"truncation_reason\":null}"}],
    "isError": false
  }
}
```

The path from `dispatch` to `log_info` is one hop, `certain` confidence, via a `conditional` edge (rendered as `[if]` in the CLI human format).

### Call the explain tool

```json
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"explain","arguments":{"symbol":"rust_sample::conditions::dispatch","root":"/path/to/rust-sample"}}}
```

The response `edges` array uses `peer`, `peer_file`, and `peer_line` for the neighbour symbol:

```json
{
  "symbol": "rust_sample::conditions::dispatch",
  "kind": "function",
  "file": "fixtures/rust-sample/src/conditions.rs",
  "line": 42,
  "callers_count": 3,
  "callees_count": 3,
  "edges": [
    {
      "direction": "outgoing",
      "peer": "rust_sample::conditions::log_info",
      "peer_file": "fixtures/rust-sample/src/conditions.rs",
      "peer_line": 4,
      "condition": "conditional",
      "confidence": "certain",
      "tier": "scope_graph",
      "rule": "scope-ref"
    }
  ]
}
```

## Exit codes

`cgx mcp` itself does not exit until the client closes its end of the pipe. Startup errors (unrecognised flags, bad `--root` path) exit immediately.

| Code | Condition |
|------|-----------|
| `0` | Client closed the connection normally. |
| `2` | Bad argument to `cgx mcp` (unrecognised flag, invalid `--root`). |

Tool-level errors are returned as JSON-RPC error objects within the protocol; they do not affect the process exit code.

## See also

- [07-interfaces.md](../07-interfaces.md) — MCP interface specification (IF-9) and full feature list
- [cgx callers](callers.md) — CLI equivalent of the `callers` MCP tool
- [cgx callees](callees.md) — CLI equivalent of the `callees` MCP tool
- [cgx paths](paths.md) — CLI equivalent of the `paths` MCP tool
- [cgx unused](unused.md) — CLI equivalent of the `unused` MCP tool
- [cgx explain](explain.md) — CLI equivalent of the `explain` MCP tool
- `cgx query` — CQL query interface (CLI-only in v0.3; reference doc pending)
