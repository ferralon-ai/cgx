# callers (MCP tool)

Symbols that (transitively, to `depth`) call the named symbol.

## Purpose

`callers` walks the call graph backward from `symbol` and returns every
symbol that reaches it within the traversal depth. Use it to answer: *what
calls this symbol?* — including transitive callers up to the specified depth.

Results are paginated. The default page size is 20; the hard cap is 200.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Qualified symbol name (FQN or unambiguous short name). |
| `root` | string | yes | — | Absolute path to the repository root (must contain a `.cgx/` store). |
| `depth` | integer | no | `1` | Maximum backward traversal depth. `0` is not a valid sentinel; omit for the default. |
| `max_results` | integer | no | `20` | Page size. Clamped to `[1, 200]`; values above 200 are silently capped. |
| `cursor` | string | no | — | Opaque pagination cursor returned by a previous response. Pass `"0"` to force an explicit first-page request. |
| `edge_condition` | string | no | — | Filter to a single edge-condition class. One of: `always`, `conditional`, `exception`, `loop`, `panic`. |
| `confidence` | string | no | — | Minimum confidence floor. One of: `certain`, `probable`, `possible`. No value = all tiers shown. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted. |

**Note:** The `at` (git ref) parameter documented in older references does not
exist in the MCP input schema. Passing `at` has no effect — it is silently
ignored.

**MCP vs CLI depth default:** The MCP default depth is `1` (direct callers
only). The CLI `cgx callers` default is `2`. Pass an explicit `depth` if you
need the same results as a CLI invocation.

## Output shape

The response envelope includes:

| Field | Type | Meaning |
|-------|------|---------|
| `symbol` | string | The symbol name as received. |
| `results` | array | Current page of caller records (see below). |
| `total_matched` | integer | Total callers found before pagination. |
| `has_more` | boolean | `true` if more pages remain. |
| `cursor` | string or null | Opaque cursor for the next page; `null` on the last page. |
| `graph_version` | string | Content hash of the graph at query time (ADR-06). |
| `dirty` | boolean | `true` if working-tree changes were analyzed. |
| `dirty_files_analyzed` | integer | Count of dirty files included in the overlay. |

Each record in `results`:

| Field | Type | Meaning |
|-------|------|---------|
| `name` | string | Fully-qualified name of the caller. |
| `file` | string | Source file path (relative to repo root). |
| `line` | integer | Line number of the call site. |
| `depth` | integer | Graph hops from the target symbol to this caller. |
| `edge_condition` | string | Edge condition: `always`, `conditional`, `exception`, `loop`, or `panic`. |
| `confidence` | string | Confidence of this specific edge: `certain`, `probable`, or `possible`. |
| `min_confidence_on_path` | string | Minimum confidence across all edges on the path from this caller to `symbol`. |
| `exception_transient` | boolean | `true` if this caller is reachable only via an exception edge on the traversal path. |

## Example

### Request

```json
{
  "symbol": "rust_sample::conditions::dispatch",
  "root": "/path/to/rust-sample",
  "depth": 1,
  "confidence": "probable"
}
```

### Response

```json
{
  "symbol": "rust_sample::conditions::dispatch",
  "results": [
    {
      "name": "rust_sample::conditions::run_all",
      "file": "src/conditions.rs",
      "line": 88,
      "depth": 1,
      "edge_condition": "always",
      "confidence": "certain",
      "min_confidence_on_path": "certain",
      "exception_transient": false
    },
    {
      "name": "rust_sample::main",
      "file": "src/main.rs",
      "line": 12,
      "depth": 1,
      "edge_condition": "conditional",
      "confidence": "probable",
      "min_confidence_on_path": "probable",
      "exception_transient": false
    }
  ],
  "total_matched": 2,
  "has_more": false,
  "cursor": null,
  "graph_version": "14a033de7e8fceefc799847fba8ee73f6aff64fb",
  "dirty": false,
  "dirty_files_analyzed": 0
}
```

## Notes

- **Pagination:** Increment through pages by passing the `cursor` value from
  each response as the `cursor` of the next request. The cursor is a decimal
  offset string; `null` cursor means the last page.
- **Hard cap:** `max_results` values above 200 are silently clamped to 200.
  There is no error.
- **Dirty overlay:** `include_dirty` defaults to `true`. The overlay is
  content-addressed and computed per call; it is never written to the `.cgx/`
  store. Set `include_dirty: false` to query only the last committed index.
- **Edge-condition rendering:** The MCP response returns the raw string token
  (`"always"`, `"conditional"`, etc.) for every edge. This differs from the
  CLI, which omits `always` and abbreviates `conditional` to `[if]` and
  `exception` to `[exc]`.
- **Cycle detection:** If a caller appears on its own ancestor path the
  traversal does not expand it further. Cycles do not produce duplicate
  entries.
- **Symbol resolution:** Both FQNs and unambiguous short names are accepted.
  If the pattern matches zero or more than one symbol, the tool returns a
  resolve error.

## See also

- [callees](callees.md) — the forward direction: symbols the named symbol calls
- [paths](paths.md) — enumerate every call path between two symbols
- [explain](explain.md) — full provenance for one symbol including all incident edges
- [unused](unused.md) — symbols not reachable from any entrypoint
- [cgx callers (CLI)](../commands/callers.md) — CLI equivalent with output format and exit-code reference
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
