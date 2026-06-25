# unused (MCP tool)

Symbols not reachable from any entrypoint (the complement of entrypoint reachability).

## Purpose

`unused` queries the graph for every indexed symbol that no entrypoint can reach — directly or transitively — and returns them as a paginated flat list. Use it to surface dead code for review or to drive automated dead-code gates in CI.

A symbol is unused when no walk from any indexed entrypoint reaches it. If a project has no indexed entrypoints, every symbol may appear as unused; run `cgx doctor` to verify entrypoint coverage before interpreting results.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `root` | string | yes | — | Absolute path to the repository root containing a `.cgx/` store. |
| `kind` | string | no | `"all"` | Restrict results to one symbol kind. Enum: `"function"`, `"method"`, `"field"`, `"all"`. `"all"` returns every kind. |
| `entrypoint` | string | no | — | FQN or unambiguous short name of a single symbol to use as the sole reachability root, overriding the graph's indexed entrypoints. |
| `max_results` | integer | no | `20` | Maximum results per page. Clamped to `[1, 200]`; values above 200 are silently reduced to 200. |
| `cursor` | string | no | — | Opaque pagination cursor returned by a prior call. Omit for the first page. |
| `include_dirty` | boolean | no | `true` | When `true`, analyzes uncommitted working-tree changes via a per-call content-addressed overlay. The overlay is never persisted. |

**`kind` differs from CLI.** The MCP `kind` enum includes `"all"` (return every kind); the CLI `--kind` flag does not accept `"all"` — it accepts the full symbol-kind vocabulary (`function`, `method`, `type`, `field`, `variable`, `module`, `constant`, `macro`, `lambda`, `entrypoint`). The MCP enum is narrower and MCP-specific.

## Output shape

Every response includes the session metadata fields added by `with_session_meta()`:

| Field | Type | Meaning |
|-------|------|---------|
| `results` | array | Page of matching symbol records (see below). |
| `total_matched` | integer | Total symbols matched before pagination. |
| `has_more` | boolean | `true` when more pages are available. |
| `cursor` | string or null | Pass as `cursor` in the next call to fetch the next page. `null` on the last page. |
| `graph_version` | string | Content-addressed version of the graph snapshot used (ADR-06). |
| `dirty` | boolean | `true` if the response incorporated uncommitted working-tree changes (ADR-06). |
| `dirty_files_analyzed` | integer | Count of dirty files overlaid for this call (ADR-06). |

Each record in `results`:

| Field | Type | Meaning |
|-------|------|---------|
| `name` | string | Fully-qualified name of the unused symbol. |
| `file` | string | Source file path (relative to `root`). |
| `line` | integer | Line number of the symbol definition. |
| `kind` | string | Symbol kind (lowercased). |

## Example

### Request

```json
{
  "name": "unused",
  "arguments": {
    "root": "/path/to/my-project",
    "kind": "function",
    "max_results": 3
  }
}
```

### Response

```json
{
  "results": [
    {
      "name": "my_project::dead_code::orphan_computation",
      "file": "src/dead_code.rs",
      "line": 7,
      "kind": "function"
    },
    {
      "name": "my_project::dead_code::orphan_helper",
      "file": "src/dead_code.rs",
      "line": 11,
      "kind": "function"
    },
    {
      "name": "my_project::utils::legacy_format",
      "file": "src/utils.rs",
      "line": 203,
      "kind": "function"
    }
  ],
  "total_matched": 47,
  "has_more": true,
  "cursor": "3",
  "graph_version": "sha256:a3f9...",
  "dirty": false,
  "dirty_files_analyzed": 0
}
```

To fetch the next page, repeat the call with `"cursor": "3"`.

## Notes

- **No positional arguments.** `unused` takes only the `root` required parameter plus optional filters. There is no symbol to name — the query is always over the full graph.
- **Pagination.** Default page size is 20; maximum is 200. The cursor is a decimal offset string. `"cursor": "0"` is a valid first-page request and equivalent to omitting `cursor`.
- **`entrypoint` override.** When `entrypoint` is supplied, reachability is computed from that single symbol rather than all indexed entrypoints. This is useful for scoping dead-code analysis to a specific binary entry or test harness.
- **`include_dirty` default is `true` (MCP only).** The CLI has no equivalent flag; the MCP default of `true` means uncommitted edits are analyzed by default. Pass `"include_dirty": false` to query only the committed graph.
- **`kind: "all"` is MCP-only.** This value is not accepted by the CLI `--kind` flag.
- **Results are ordered by file.** Within a file, results are ordered by line number.

## See also

- [callers](callers.md) — find what calls a symbol (to investigate whether a candidate truly has no callers)
- [explain](explain.md) — inspect a specific symbol's caller/callee counts and incident edges
- [cgx unused (CLI)](../commands/unused.md) — CLI equivalent with additional `--kind` values, `--assert-empty`, `--confidence`, and `--format` options
- [03-code-graph-model.md](../03-code-graph-model.md) — entrypoints, confidence tiers, and the graph data model
