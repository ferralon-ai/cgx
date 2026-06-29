# explain (MCP tool)

Full provenance for one symbol: its definition location, caller/callee counts, and every incident edge with its condition and confidence.

## Purpose

`explain` answers the question "what do I know about this one symbol?" in a single tool call. It returns the symbol's definition file and line, its kind, the total count of direct callers and callees, and every incident edge — both incoming (callers) and outgoing (callees) — with its edge condition, confidence, resolution tier, rule, and call-site location.

Because `explain` returns all direct edges rather than performing a traversal, it has no `depth`, `max_results`, `cursor`, `edge_condition`, or `confidence` parameters. Use `callers` or `callees` for multi-hop traversal or confidence filtering; use `paths` to enumerate routes between two symbols.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Fully-qualified name (FQN) or unambiguous short name of the symbol to explain. |
| `root` | string | yes | — | Absolute path to the indexed repository root. |
| `include_dirty` | boolean | no | `true` | When `true`, overlay uncommitted working-tree changes onto the index before answering. The overlay is never persisted. |

No other parameters are accepted. Passing unknown fields has no effect.

## Output shape

A single JSON object. The session-metadata fields (`graph_version`, `dirty`, `dirty_files_analyzed`) are always present.

### Top-level fields

| Field | Type | Meaning |
|-------|------|---------|
| `symbol` | string | FQN of the resolved symbol. |
| `file` | string | Source file containing the definition (relative to `root`). |
| `line` | integer | Line number of the definition. |
| `kind` | string | Symbol kind: `function`, `method`, `field`, etc. |
| `callers_count` | integer | Number of direct incoming edges. |
| `callees_count` | integer | Number of direct outgoing edges. |
| `edges` | array | All incident edges (see below). |
| `graph_version` | string | Content-addressed version of the graph snapshot (ADR-06). |
| `dirty` | boolean | Whether uncommitted changes were included (ADR-06). |
| `dirty_files_analyzed` | integer | Count of dirty files overlaid (ADR-06). |

### Edge object fields

Each element of `edges` describes one incident call edge.

| Field | Type | Meaning |
|-------|------|---------|
| `direction` | string | `"incoming"` for a caller edge; `"outgoing"` for a callee edge. |
| `peer` | string | FQN of the other symbol on the edge. |
| `peer_file` | string | Source file of the peer symbol. |
| `peer_line` | integer | Line number of the peer symbol's definition. |
| `condition` | string | Edge condition: `always`, `conditional`, `exception`, `loop`, or `panic`. See [docs/03-code-graph-model.md](../03-code-graph-model.md). |
| `confidence` | string | Resolution confidence: `certain`, `probable`, or `possible`. See [docs/03-code-graph-model.md](../03-code-graph-model.md). |
| `tier` | string | Resolution method: `scope_graph`, `cha_rta`, `scip`, `name_syntactic`, or `points_to`. |
| `rule` | string | Matching rule applied: e.g. `scope-ref`, `sig-compat`, `name-method`. |
| `resolution_source` | string or null | Additional provenance from the resolver, or `null`. |
| `site` | object or null | Call-site location: `{"file": "...", "line": N}`, or `null` if unavailable. |

## Example

### Request

```json
{
  "name": "explain",
  "arguments": {
    "symbol": "rust_sample::conditions::dispatch",
    "root": "/path/to/rust-sample"
  }
}
```

### Response

```json
{
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
  "graph_version": "a3f9c1...",
  "dirty": false,
  "dirty_files_analyzed": 0
}
```

The three incoming edges are `cha_rta` / `sig-compat` matches at `possible` confidence with condition `always`. The three outgoing edges are `scope_graph` / `scope-ref` at `certain` confidence with condition `conditional` (each callee is called only on some branch of `dispatch`).

## Notes

- **No pagination.** `explain` returns all incident edges in one response. There is no `cursor`, `has_more`, or `max_results` field. For large symbols with many edges, use `callers` or `callees` (which do paginate) instead.
- **No `at` parameter.** The `--at` git-ref flag exists on the CLI but is not registered in the MCP tool's `inputSchema`. Passing `at` has no effect.
- **Short-name resolution.** If `symbol` is a short name that matches exactly one symbol, it resolves successfully. Ambiguous or unmatched names return a tool error.
- **`include_dirty` defaults to `true`.** MCP differs from the CLI here — the tool analyzes uncommitted changes by default. Pass `false` to query only the persisted index.
- **Edge condition rendering.** The MCP tool returns raw condition strings (`"always"`, `"conditional"`, etc.). The CLI renders these with abbreviations (`[if]`, `[exc]`) and omits `always` from human output; the MCP tool does not apply those transformations.

## See also

- [callers](callers.md) — transitive caller traversal with depth, confidence, and pagination
- [callees](callees.md) — transitive callee traversal with depth, confidence, and pagination
- [paths](paths.md) — enumerate call paths between two symbols
- [unused](unused.md) — symbols not reachable from any entrypoint
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge condition and confidence definitions
