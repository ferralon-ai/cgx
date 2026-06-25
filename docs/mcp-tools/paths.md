# paths (MCP tool)

Enumerate call paths from one symbol to another, surfacing per-path confidence and
exceptional-class crossing.

**Version:** v0.3.0  
**Audience:** AI coding agents, AI security agents, tool integrators

---

## Purpose

`paths` answers: "What call sequences lead from symbol A to symbol B?"

It performs a bounded depth-first search over the call graph and returns every distinct
path found, each annotated with its hop count, minimum edge confidence, and whether any
hop crosses an exceptional execution class. The tool is the MCP equivalent of `cgx paths`
(CLI) with different defaults — see [MCP vs CLI defaults](#mcp-vs-cli-defaults).

---

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `from` | string | yes | — | FQN or short name of the source symbol |
| `to` | string | yes | — | FQN or short name of the target symbol |
| `root` | string | yes | — | Absolute path to the indexed repository root |
| `max_depth` | integer | no | `10` | Maximum traversal depth. `0` = unlimited (work-budgeted) |
| `max_results` | integer | no | `10` | Maximum paths to return per page (hard cap: 200) |
| `exclude_edge_condition` | string | no | — | Exclude paths that contain any edge with this condition. One of: `always`, `conditional`, `exception`, `loop`, `panic` |
| `only_edge_condition` | string | no | — | Return only paths where every edge has this condition. One of: `always`, `conditional`, `exception`, `loop`, `panic` |
| `confidence` | string | no | — | Minimum confidence floor; edges below this tier are excluded. One of: `certain`, `probable`, `possible` |
| `cursor` | string | no | — | Pagination cursor returned by a previous response. `"0"` requests the first page explicitly |
| `include_dirty` | boolean | no | `true` | When `true`, includes facts from dirty (unindexed) files via the incremental overlay |

`exclude_edge_condition` and `only_edge_condition` are mutually exclusive. If both are
supplied the tool returns a parameter error.

---

## Output shape

Every response wraps the result in a session-metadata envelope (ADR-06):

```
{
  "from":              string,
  "to":                string,
  "paths":             PathResult[],
  "truncated":         boolean,
  "truncation_reason": string | null,
  "has_more":          boolean,
  "cursor":            string | null,
  "graph_version":     string,
  "dirty":             boolean,
  "dirty_files_analyzed": integer
}
```

Each `PathResult` in the `paths` array:

```
{
  "hops":               integer,
  "min_confidence":     "certain" | "probable" | "possible",
  "crosses_exceptional": boolean,
  "steps":              Step[]
}
```

Each `Step` in the `steps` array:

```
{
  "name":              string,       // FQN of the symbol at this hop
  "file":              string,       // repo-relative path
  "line":              integer,
  "edge_condition":    string | null, // null on first step (no incoming edge)
  "confidence":        string | null, // null on first step
  "exception_transient": boolean
}
```

Top-level fields:

| Field | Type | Meaning |
|-------|------|---------|
| `from` | string | Echo of the `from` input |
| `to` | string | Echo of the `to` input |
| `paths` | array | Current page of path results |
| `truncated` | boolean | `true` if the search hit its work budget before exhausting the graph |
| `truncation_reason` | string or null | Machine-readable reason code when `truncated` is `true`; null otherwise |
| `has_more` | boolean | `true` if additional pages exist; paginate with `cursor` |
| `cursor` | string or null | Opaque offset string to pass as `cursor` for the next page; null when no more pages |
| `graph_version` | string | Content-hash of the graph snapshot queried (ADR-06) |
| `dirty` | boolean | `true` if the response includes facts from dirty files |
| `dirty_files_analyzed` | integer | Count of dirty files included in this response |

---

## Example

### Request

```json
{
  "name": "paths",
  "arguments": {
    "from": "rust_sample::async_calls::fetch_data",
    "to":   "rust_sample::async_calls::http_get",
    "root": "/path/to/rust-sample"
  }
}
```

### Response

```json
{
  "from": "rust_sample::async_calls::fetch_data",
  "to":   "rust_sample::async_calls::http_get",
  "paths": [
    {
      "hops": 1,
      "min_confidence": "probable",
      "crosses_exceptional": true,
      "steps": [
        {
          "name": "rust_sample::async_calls::fetch_data",
          "file": "fixtures/rust-sample/src/async_calls.rs",
          "line": 7,
          "edge_condition": null,
          "confidence": null,
          "exception_transient": false
        },
        {
          "name": "rust_sample::async_calls::http_get",
          "file": "fixtures/rust-sample/src/async_calls.rs",
          "line": 13,
          "edge_condition": "exception",
          "confidence": "probable",
          "exception_transient": true
        }
      ]
    },
    {
      "hops": 1,
      "min_confidence": "probable",
      "crosses_exceptional": false,
      "steps": [
        {
          "name": "rust_sample::async_calls::fetch_data",
          "file": "fixtures/rust-sample/src/async_calls.rs",
          "line": 7,
          "edge_condition": null,
          "confidence": null,
          "exception_transient": false
        },
        {
          "name": "rust_sample::async_calls::http_get",
          "file": "fixtures/rust-sample/src/async_calls.rs",
          "line": 13,
          "edge_condition": "always",
          "confidence": "probable",
          "exception_transient": false
        }
      ]
    }
  ],
  "truncated": false,
  "truncation_reason": null,
  "has_more": false,
  "cursor": null,
  "graph_version": "a1b2c3d4",
  "dirty": false,
  "dirty_files_analyzed": 0
}
```

Two paths exist between the symbols. The first traverses an `exception` edge
(`crosses_exceptional: true`); the second traverses the unconditional `always` edge.

---

## Notes

### MCP vs CLI defaults

| Property | MCP `paths` | CLI `cgx paths` |
|----------|-------------|-----------------|
| depth parameter name | `max_depth` | `--depth` |
| depth default | `10` | `6` |
| result limit | `10` (paginated, hard cap 200) | unlimited |
| `include_dirty` | `true` | no equivalent |

The property name difference is significant: passing `depth` instead of `max_depth`
to this tool is silently ignored.

### Pagination

Results are paginated. When `has_more` is `true`, pass the returned `cursor` value
as `cursor` in the next request with identical other parameters to retrieve the next
page. The cursor is a decimal offset string; `"0"` is a valid explicit first-page
request. Values above 200 for `max_results` are clamped to 200.

### Empty result

An empty `paths` array with `truncated: false` means no call path exists between
the two symbols in the queried graph. This is a successful response (no error).

### Work-budget truncation

Setting `max_depth: 0` lifts the depth limit but does not remove the internal work
budget. On dense graphs the search may halt early; `truncated` will be `true` and
`truncation_reason` will carry a machine-readable code.

### Confidence filtering

The `confidence` parameter sets a minimum floor. Edges below the floor are excluded
from all path searches. Filtering to `certain` removes speculative edges and may
produce an empty result set even when paths exist at lower confidence tiers.

### Edge-condition filtering

`only_edge_condition` and `exclude_edge_condition` operate on the edges within each
path, not on the path set as a whole. A path is included when it satisfies the
condition predicate for every one of its edges (for `only_edge_condition`) or for
none of them (for `exclude_edge_condition`).

---

## See also

- [callers](callers.md) — all symbols that (transitively) call a target
- [callees](callees.md) — all symbols a source (transitively) calls
- [explain](explain.md) — full provenance for one symbol including all incident edges
- [cgx paths](../commands/paths.md) — CLI equivalent with `--format`, `--assert-empty`, exit codes
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and exceptional-class semantics
