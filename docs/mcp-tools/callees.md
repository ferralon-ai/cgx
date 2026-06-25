# callees (MCP tool)

Symbols the named symbol (transitively, to `depth`) calls.

## Purpose

`callees` walks the call graph forward from a named symbol and returns every symbol it transitively invokes, up to the requested depth. Each result carries the edge condition, confidence tier, and source location of the callee.

This is the forward counterpart of [`callers`](callers.md). Use `callees` when you want to understand the blast radius of a change to a given symbol, enumerate what a function depends on, or verify that a function is a leaf.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Fully-qualified or short symbol name to expand as the call-graph root. Partial matches are accepted; ambiguous or unmatched input returns an error. |
| `root` | string | yes | — | Absolute path to the repository root. Must point to a directory that contains a `.cgx/` store (or a working tree that can be auto-indexed). |
| `depth` | integer | no | `1` | Maximum traversal depth. `1` returns direct callees only. There is no unlimited-depth option via MCP; set a large integer to approximate it (subject to the work budget). |
| `max_results` | integer | no | `20` | Maximum results per page. Clamped to `200`; values above `200` are silently capped, not rejected. |
| `cursor` | string | no | — | Pagination cursor from a prior response. Pass the `cursor` value returned in a previous result to fetch the next page. Omit or pass `null` for the first page. |
| `edge_condition` | string | no | — | Filter to a single edge condition. One of `always`, `conditional`, `exception`, `loop`, `panic`. When omitted, all edge conditions are returned. |
| `confidence` | string | no | — | Minimum confidence floor. One of `certain`, `probable`, `possible`. Edges below this tier are excluded. When omitted, all tiers are returned. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay. The overlay is never persisted to the index. Set `false` to query only the last committed index state. |

## Output shape

A successful response is a JSON object with the following fields:

| Field | Type | Meaning |
|-------|------|---------|
| `symbol` | string | The input symbol name as supplied. |
| `results` | array | The current page of callee results (see item fields below). |
| `total_matched` | integer | Total number of callees matched before pagination. |
| `has_more` | boolean | `true` when a subsequent page exists. |
| `cursor` | string or null | Opaque pagination cursor. Pass to the next request to retrieve the next page. `null` when `has_more` is `false`. |
| `graph_version` | string | Graph snapshot identifier (ADR-06). |
| `dirty` | boolean | `true` when the response reflects uncommitted working-tree changes. |
| `dirty_files_analyzed` | integer | Count of dirty files overlaid onto the index for this call. |

Each object in `results` contains:

| Field | Type | Meaning |
|-------|------|---------|
| `name` | string | Fully-qualified name of the callee. |
| `file` | string | Source file path. |
| `line` | integer | Line number of the callee's definition. |
| `depth` | integer | Traversal depth from the root symbol (1 = direct callee). |
| `edge_condition` | string | Call-edge condition: `always`, `conditional`, `exception`, `loop`, or `panic`. |
| `confidence` | string | Confidence of this specific edge: `certain`, `probable`, or `possible`. |
| `min_confidence_on_path` | string | Lowest confidence of any edge on the path from root to this callee. Useful for filtering full-path quality. |
| `exception_transient` | boolean | `true` when this callee is reachable only via an exception-path edge somewhere on the path. |

## Example

Request:

```json
{
  "symbol": "rust_sample::conditions::dispatch",
  "root": "/path/to/rust-sample",
  "depth": 1
}
```

Response:

```json
{
  "symbol": "rust_sample::conditions::dispatch",
  "results": [
    {
      "name": "rust_sample::conditions::log_info",
      "file": "fixtures/rust-sample/src/conditions.rs",
      "line": 4,
      "depth": 1,
      "edge_condition": "conditional",
      "confidence": "certain",
      "min_confidence_on_path": "certain",
      "exception_transient": false
    },
    {
      "name": "rust_sample::conditions::log_warn",
      "file": "fixtures/rust-sample/src/conditions.rs",
      "line": 8,
      "depth": 1,
      "edge_condition": "conditional",
      "confidence": "certain",
      "min_confidence_on_path": "certain",
      "exception_transient": false
    },
    {
      "name": "rust_sample::conditions::log_error",
      "file": "fixtures/rust-sample/src/conditions.rs",
      "line": 12,
      "depth": 1,
      "edge_condition": "conditional",
      "confidence": "certain",
      "min_confidence_on_path": "certain",
      "exception_transient": false
    }
  ],
  "total_matched": 3,
  "has_more": false,
  "cursor": null,
  "graph_version": "a3f1c2d",
  "dirty": false,
  "dirty_files_analyzed": 0
}
```

## Notes

**MCP vs CLI defaults.** The MCP `callees` tool defaults `depth` to `1`; the CLI `cgx callees` defaults `--depth` to `2`. Set `depth` explicitly when you need behavior consistent with the CLI.

**Pagination.** Results are paged at `max_results` per call (default 20, hard cap 200). When `has_more` is `true`, pass the returned `cursor` value in a follow-up request with all other parameters unchanged to retrieve the next page. The cursor is a decimal offset string; `"0"` is a valid first-page cursor equivalent to omitting the field.

**Edge-condition rendering in results.** The `edge_condition` field always returns the raw token string (`always`, `conditional`, `exception`, `loop`, `panic`). This differs from CLI human output, which omits `always` and abbreviates `conditional` to `[if]` and `exception` to `[exc]`.

**`include_dirty` default.** The MCP tool defaults `include_dirty` to `true`, meaning uncommitted edits are reflected in results by default. This is the opposite of the CLI, which has no equivalent flag and always queries the last committed index. Pass `"include_dirty": false` when you need a stable, reproducible result tied to the index state.

**No `at` parameter.** The `--at` git-ref flag available on the CLI is not wired into the MCP tool schema. Passing `at` in the request body is silently ignored.

**Symbol resolution.** The tool accepts both fully-qualified names (FQNs) and unambiguous short names. If the pattern matches zero or more than one symbol, the call returns a resolution error.

## See also

- [callers](callers.md) — reverse direction: symbols that transitively call the named symbol
- [paths](paths.md) — enumerate every call path between two specific symbols
- [explain](explain.md) — full provenance for one symbol, including all incident edges
- [unused](unused.md) — symbols not reachable from any entrypoint
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and transience semantics
