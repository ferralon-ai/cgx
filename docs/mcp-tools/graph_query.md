# graph_query (MCP tool)

Execute a Cypher-subset query expression against the call graph.

**Status in v0.3.0:** Registered in `tools/list` but always returns a `ToolError`
when called. The Cypher-subset query language is a post-Phase-1 deliverable. Use
[callers](callers.md), [callees](callees.md), [paths](paths.md), or [unused](unused.md)
for all current query needs.

## Purpose

`graph_query` is the reserved slot for a future ad-hoc query interface. When
implemented, it will accept a Cypher-subset expression and return matching nodes
or paths from the call graph index. The tool schema is already registered so MCP
clients can detect its presence, but any `tools/call` invocation returns an error.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `query` | string | yes | — | Cypher-subset query expression. Parsed but not executed in v0.3.0. |
| `root` | string | yes | — | Absolute path to the indexed repository root (must contain a `.cgx/` store). |
| `max_results` | integer | no | `20` | Maximum result rows to return. Hard cap is `200`; values above `200` are clamped. |
| `cursor` | string | no | — | Pagination cursor from a prior response's `cursor` field. Pass `"0"` to request the first page explicitly. |
| `include_dirty` | boolean | no | `true` | When `true`, include results from files modified since the last index run. |

## Output shape

In v0.3.0, `graph_query` never returns a result envelope. Every call returns a
`ToolError` with `isError: true` and the message:

```
graph_query: the Cypher-subset query language is not implemented in Phase 1;
use callers/callees/paths/unused
```

The result envelope documented below is the intended future shape (not yet
implemented).

When implemented, each response will include the session metadata fields present
on all functional tools:

| Field | Type | Description |
|-------|------|-------------|
| `graph_version` | string | Graph snapshot version (ADR-06). |
| `dirty` | boolean | Whether any dirty-file results are included (ADR-06). |
| `dirty_files_analyzed` | integer | Count of dirty files included (ADR-06). |
| `has_more` | boolean | Whether additional pages exist (IF-18). |
| `cursor` | string or null | Opaque cursor for the next page, or `null` on the last page (IF-18). |

## JSON request + response example

### Request

```json
{
  "name": "graph_query",
  "arguments": {
    "query": "MATCH (a)-[:CALLS]->(b) WHERE a.name = 'dispatch' RETURN a, b",
    "root": "/path/to/my-repo",
    "max_results": 10
  }
}
```

### Response (v0.3.0 — always an error)

```json
{
  "content": [
    {
      "type": "text",
      "text": "graph_query: the Cypher-subset query language is not implemented in Phase 1; use callers/callees/paths/unused"
    }
  ],
  "isError": true
}
```

## Notes

- `graph_query` appears in `tools/list` responses so MCP clients can discover the
  tool and understand its intended interface, but it is not callable in v0.3.0.
- The `at` parameter (git ref pinning) is **not** in the input schema for this
  tool. Do not pass it; it will be silently ignored.
- Default `include_dirty: true` differs from the CLI, where dirty-file inclusion
  has no equivalent flag and defaults off. This is consistent across all six MCP
  tools.
- The `max_results` hard cap of `200` is enforced by the server; clients cannot
  request more than 200 rows per page.

## See also

- [callers](callers.md) — find symbols that (transitively) call a named symbol
- [callees](callees.md) — find symbols a named symbol (transitively) calls
- [paths](paths.md) — enumerate call paths between two symbols
- [unused](unused.md) — find symbols unreachable from any entrypoint
- [explain](explain.md) — full provenance for one symbol
- [docs/05-queries.md](../05-queries.md) — query capabilities taxonomy and language design
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge conditions and confidence tiers
