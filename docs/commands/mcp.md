# cgx mcp

Start the MCP STDIO server (docs/07 IF-9).

## Synopsis

```
cgx mcp [OPTIONS]
```

## Description

`cgx mcp` starts a long-running [Model Context Protocol](../07-interfaces.md) server that communicates over standard input and output. It exposes the cgx call-graph as a set of MCP tools that AI coding agents, security agents, and IDE extensions can call using standard JSON-RPC 2.0 framing.

The server conforms to the 2025-06-18 MCP specification and responds to `tools/list` and `tools/call`. It registers **twelve** tools, all functional:

| Tool | CLI equivalent | Per-tool reference |
|------|----------------|--------------------|
| `callers` | [`cgx callers`](callers.md) | [mcp-tools/callers.md](../mcp-tools/callers.md) |
| `callees` | [`cgx callees`](callees.md) | [mcp-tools/callees.md](../mcp-tools/callees.md) |
| `reaches` | [`cgx reaches`](reaches.md) | [mcp-tools/reaches.md](../mcp-tools/reaches.md) |
| `paths` | [`cgx paths`](paths.md) | [mcp-tools/paths.md](../mcp-tools/paths.md) |
| `unused` | [`cgx unused`](unused.md) | [mcp-tools/unused.md](../mcp-tools/unused.md) |
| `explain` | [`cgx explain`](explain.md) | [mcp-tools/explain.md](../mcp-tools/explain.md) |
| `search` | [`cgx search`](search.md) | [mcp-tools/search.md](../mcp-tools/search.md) |
| `symbols` | [`cgx symbols`](symbols.md) | [mcp-tools/symbols.md](../mcp-tools/symbols.md) |
| `flows_to` | [`cgx flows-to`](flows-to.md) | [mcp-tools/flows_to.md](../mcp-tools/flows_to.md) |
| `flows_from` | [`cgx flows-from`](flows-from.md) | [mcp-tools/flows_from.md](../mcp-tools/flows_from.md) |
| `graph_query` | [`cgx query`](query.md) | [mcp-tools/graph_query.md](../mcp-tools/graph_query.md) |
| `coupling` | [`cgx coupling`](coupling.md) | [mcp-tools/coupling.md](../mcp-tools/coupling.md) |

Tool names use underscores where the CLI uses hyphens (`flows_to`, not `flows-to`). The registry in `tools/list` is the authoritative list; the schemas below cover the tools this page documents in detail, and [docs/mcp-tools/](../mcp-tools/README.md) is the full per-tool reference.

Each tool call returns a JSON result envelope containing session metadata alongside the tool-specific result payload. The server does not maintain state between tool calls; every call loads from the on-disk `.cgx/` index.

**MCP vs CLI differences:**

- Default depth for `callers`/`callees` is **1** in MCP (CLI default is 2).
- The `paths` tool uses the property name `max_depth` (CLI flag is `--depth`), and its default is **10** (CLI default is 6).
- **`max_depth: 0` means zero hops over MCP.** The CLI's `paths --depth 0` means *unlimited*; the MCP `paths` tool with `max_depth: 0` searches nothing and returns an empty `paths` array with an `under-approximate … depth-limit` contract. The two surfaces genuinely differ here — do not carry a CLI habit across.
- `max_results` caps results per page (default 20 for most tools; 10 for `paths`). The hard ceiling is 200; values above 200 are clamped.
- `include_dirty` defaults to `true` — the server analyses uncommitted working-tree changes via a per-call content-addressed overlay. This overlay is never persisted to the index.
- No `at` parameter exists in any MCP tool (git-ref pinning is CLI-only in v0.3).

## Arguments

`cgx mcp` takes no positional arguments.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--root` | path | — | Default repository root injected into tool calls that omit the `root` property. When set, agents may omit `root` from individual tool call arguments. When unset, `root` is required in every tool call. |

## Tools

Input schemas for the tools this page covers in detail. `root` and any required symbol argument are marked required; all other properties are optional. Every registered tool accepts `root` and `include_dirty` on the same terms.

### callers

Symbols that (transitively, to `depth`) call the named symbol.

| Property | Type | Required | Default | Meaning |
|----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Qualified symbol name (FQN or unambiguous short name). |
| `root` | string | yes* | — | Repository root path. May be omitted if `--root` was passed to `cgx mcp`. |
| `depth` | integer | no | `1` | Maximum traversal depth. |
| `max_results` | integer | no | `20` | Page size. Capped at 200. |
| `cursor` | string | no | — | Pagination cursor from a prior response's `cursor` field. `"0"` is a valid first-page value. |
| `kind` | array of string | no | — | Restrict results to these symbol kinds. |
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

### graph_query

Run a CQL statement (the Cypher subset [`cgx query`](query.md) accepts) and return its result table.

| Property | Type | Required | Default | Meaning |
|----------|------|----------|---------|---------|
| `query` | string | yes | — | The CQL statement. |
| `root` | string | yes* | — | Repository root path. |
| `max_results` | integer | no | `20` | Page size. Capped at 200. |
| `cursor` | string | no | — | Pagination cursor. |
| `include_dirty` | boolean | no | `true` | Include uncommitted working-tree changes. |

The tool routes to the same CQL planner and evaluator the CLI uses, so a statement returns the same rows through either surface, and a malformed statement comes back as a real plan error (`plan error: a MATCH pattern must contain at least one relationship`) rather than a blanket "not implemented". A tabular result carries `columns` and `rows` in place of `results`.

### flows_to / flows_from

Forward data-flow slice and backward data-flow pedigree over `derives-from` edges, on SSA value nodes. `symbol` names a value node (`rust_sample::dataflow::flow_example::b#1`) — see [`cgx flows-to`](flows-to.md) for how to discover those FQNs.

| Property | Type | Required | Default | Meaning |
|----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Value-node FQN. |
| `root` | string | yes* | — | Repository root path. |
| `depth` | integer | no | `2` | Maximum traversal depth. Note this differs from `callers`/`callees`, whose MCP default is `1`. |
| `max_results` | integer | no | `20` | Page size. Capped at 200. |
| `cursor` | string | no | — | Pagination cursor. |
| `confidence` | string | no | — | Floor filter. |
| `include_dirty` | boolean | no | `true` | Include uncommitted working-tree changes. |

There is no `edge_condition` or `kind` property on these two: the walk follows a single edge family.

### reaches, search, symbols, coupling

Registered and functional; their schemas live in [docs/mcp-tools/](../mcp-tools/README.md) alongside the four above.

## Response envelope

Every successful tool call includes these session metadata fields alongside the tool-specific payload:

| Field | Type | Meaning |
|-------|------|---------|
| `graph_version` | string | Index commit hash, with `+dirty.<hash>` suffix when `include_dirty` is true and the working tree has changes (ADR-06). |
| `dirty` | boolean | Whether uncommitted changes were analysed. |
| `dirty_files_analyzed` | integer | Count of working-tree files included in the overlay. |
| `has_more` | boolean | Whether additional pages exist (IF-18 pagination). |
| `cursor` | string or null | Pass to the next call to retrieve the next page. |
| `freshness` | object | The index-freshness envelope: `indexed_tree`, `head_tree`, `matches_head`, `dirty_files_base`, `dirty_files`, `stale`. Present on eleven of the twelve tools — `coupling` omits it, because it reads committed git history and never opens the index. |
| `approximation` | object | The approximation contract: `direction` (`exact`, `over`, `under`, or `over_under`), `modeled_graph`, and `reasons[]`, plus a `scope` object on a negative answer. Present on nine of the twelve tools. |

`explain`, `search`, and `symbols` carry `freshness` but **no** `approximation`, by design: none of them walks a frontier, so there is nothing to over- or under-approximate. `explain` returns one symbol's recorded incident edges, and `search`/`symbols` are table scans.

**`matches_head: null` is the normal case over MCP, not an edge case.** With the default `include_dirty: true` the server analyses the working tree, and `indexed_tree` comes back as `workdir:<hash>` rather than a git tree OID — so it cannot be compared against `head_tree`, and `matches_head` is `null` (verdict `unknown`). Treat the field as three-valued (`true` / `false` / `null`); a client that tests it as a boolean reads the common case as "does not match HEAD".

**Errors carry neither field.** A failed `tools/call` comes back as a JSON-RPC error object, not as a result envelope:

```json
{"jsonrpc":"2.0","id":3,"error":{"code":-32602,"message":"no symbol matched pattern `no_such_symbol_zzz`"}}
```

`callers`, `callees`, `flows_to`, and `flows_from` results include `total_matched`. Each result item includes: `name`, `file`, `line`, `depth`, `edge_condition`, `confidence`, `min_confidence_on_path`, `exception_transient`.

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

> Note: `dirty_files_analyzed`, `indexed_tree`, and the dirty suffix in `graph_version` are per-invocation values that vary with working-tree state.

The response's `content[0].text` is a JSON string. Decoded, it reads:

```json
{
  "approximation": {
    "direction": "over_under",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": [
      {
        "code": "over-approx-candidate-set",
        "detail": "resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur",
        "direction": "over"
      },
      {
        "code": "unresolved-call",
        "detail": "external/unindexed callees not modeled (no SCIP) (6 site(s) on the searched frontier)",
        "direction": "under"
      },
      {
        "code": "depth-limit",
        "detail": "search stopped at depth 1; deeper edges were not explored",
        "direction": "under"
      }
    ]
  },
  "cursor": null,
  "dirty": true,
  "dirty_files_analyzed": 3,
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "head_tree": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "indexed_tree": "workdir:3ebe0d25ac01b61fb0e4b5405c1d6ed4ea336866",
    "matches_head": null,
    "stale": false
  },
  "graph_version": "5ea331d+dirty.ad8411387c82",
  "has_more": false,
  "results": [
    {
      "confidence": "possible",
      "depth": 1,
      "edge_condition": "always",
      "exception_transient": false,
      "file": "fixtures/ts-sample/src/closures.ts",
      "line": 6,
      "min_confidence_on_path": "possible",
      "name": "ts_sample::closures::closureVariable"
    },
    {
      "confidence": "possible",
      "depth": 1,
      "edge_condition": "always",
      "exception_transient": false,
      "file": "fixtures/ts-sample/src/closures.ts",
      "line": 57,
      "min_confidence_on_path": "possible",
      "name": "ts_sample::closures::nestedClosures"
    },
    {
      "confidence": "possible",
      "depth": 1,
      "edge_condition": "always",
      "exception_transient": false,
      "file": "fixtures/ts-sample/src/closures.ts",
      "line": 58,
      "min_confidence_on_path": "possible",
      "name": "ts_sample::closures::nestedClosures::outer"
    }
  ],
  "symbol": "rust_sample::conditions::dispatch",
  "total_matched": 3
}
```

An agent driving this tool should read `approximation` before `results`. Three callers came back, all `possible`, and the contract says why in machine-matchable form: an over-approximated candidate set produced them (`over-approx-candidate-set`), six call sites on the frontier could not be followed (`unresolved-call`), and the walk stopped at depth 1 (`depth-limit`). `reasons[].code` is a stable token; `detail` is prose and may be reworded.

`matches_head` is `null` here — the default `include_dirty: true` made the server analyse the working tree, so `indexed_tree` is a `workdir:` hash with no git tree to compare against.

### Call the paths tool

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"paths","arguments":{"from":"rust_sample::conditions::dispatch","to":"rust_sample::conditions::log_info","root":"/path/to/worktree"}}}
```

Decoded `content[0].text`:

```json
{
  "approximation": {
    "direction": "exact",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": []
  },
  "cursor": null,
  "dirty": true,
  "dirty_files_analyzed": 3,
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "head_tree": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "indexed_tree": "workdir:3ebe0d25ac01b61fb0e4b5405c1d6ed4ea336866",
    "matches_head": null,
    "stale": false
  },
  "from": "rust_sample::conditions::dispatch",
  "graph_version": "5ea331d+dirty.ad8411387c82",
  "has_more": false,
  "paths": [
    {
      "crosses_exceptional": false,
      "hops": 1,
      "min_confidence": "certain",
      "steps": [
        {
          "confidence": null,
          "edge_condition": null,
          "exception_transient": false,
          "file": "fixtures/rust-sample/src/conditions.rs",
          "line": 42,
          "name": "rust_sample::conditions::dispatch"
        },
        {
          "confidence": "certain",
          "edge_condition": "conditional",
          "exception_transient": false,
          "file": "fixtures/rust-sample/src/conditions.rs",
          "line": 4,
          "name": "rust_sample::conditions::log_info"
        }
      ]
    }
  ],
  "to": "rust_sample::conditions::log_info",
  "total_matched": 1,
  "truncated": false,
  "truncation_reason": null
}
```

The path from `dispatch` to `log_info` is one hop, `certain` confidence, via a `conditional` edge (rendered as `[if]` in the CLI human format). The first step carries `confidence: null` and `edge_condition: null` because it is the source node, not an edge. `direction: "exact"` with an empty `reasons` array is the strongest claim available: nothing on this walk was over-approximated, cut short, or filtered.

### Call the graph_query tool

```json
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"graph_query","arguments":{"query":"MATCH (a)-[:CALLS]->(b) WHERE a.fqn = \"rust_sample::conditions::dispatch\" RETURN a.fqn, b.fqn","root":"/path/to/worktree"}}}
```

Decoded `content[0].text` (envelope fields elided; they are as above):

```json
{
  "approximation": {"direction": "exact", "modeled_graph": "…", "reasons": []},
  "columns": ["a.fqn", "b.fqn"],
  "rows": [
    ["rust_sample::conditions::dispatch", "rust_sample::conditions::log_info"],
    ["rust_sample::conditions::dispatch", "rust_sample::conditions::log_warn"],
    ["rust_sample::conditions::dispatch", "rust_sample::conditions::log_error"]
  ],
  "total_matched": 3,
  "truncated": false,
  "truncation_reason": null
}
```

Row content is identical to the same statement run through [`cgx query`](query.md). A malformed statement returns a planner error, not a stub response:

```
plan error: a MATCH pattern must contain at least one relationship
```

### Call the explain tool

```json
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"explain","arguments":{"symbol":"rust_sample::conditions::dispatch","root":"/path/to/worktree"}}}
```

The response `edges` array uses `peer`, `peer_file`, and `peer_line` for the neighbour symbol. Decoded, with the array elided to one entry per direction:

```json
{
  "callees_count": 3,
  "callers_count": 3,
  "dirty": true,
  "dirty_files_analyzed": 3,
  "edges": [
    {
      "condition": "always",
      "confidence": "possible",
      "direction": "incoming",
      "peer": "ts_sample::closures::closureVariable",
      "peer_file": "fixtures/ts-sample/src/closures.ts",
      "peer_line": 6,
      "resolution_source": null,
      "rule": "sig-compat",
      "site": {"file": "fixtures/ts-sample/src/closures.ts", "line": 6},
      "tier": "cha_rta"
    },
    {
      "condition": "conditional",
      "confidence": "certain",
      "direction": "outgoing",
      "peer": "rust_sample::conditions::log_info",
      "peer_file": "fixtures/rust-sample/src/conditions.rs",
      "peer_line": 4,
      "resolution_source": null,
      "rule": "scope-ref",
      "site": {"file": "fixtures/rust-sample/src/conditions.rs", "line": 42},
      "tier": "scope_graph"
    }
  ],
  "file": "fixtures/rust-sample/src/conditions.rs",
  "kind": "function",
  "line": 42,
  "symbol": "rust_sample::conditions::dispatch"
}
```

`explain` carries no `approximation` object. Its per-edge `tier`, `rule`, and `confidence` fields are the finer-grained form of the same honesty: `cha_rta`/`sig-compat`/`possible` on the inbound edges is exactly what makes `callers` on this symbol report `over_under`.

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
- [cgx query](query.md) — CLI equivalent of the `graph_query` MCP tool
- [cgx flows-to](flows-to.md), [cgx flows-from](flows-from.md) — CLI equivalents of `flows_to` / `flows_from`
- [docs/mcp-tools/](../mcp-tools/README.md) — per-tool schema and response reference for all twelve tools
