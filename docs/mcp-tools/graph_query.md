# graph_query (MCP tool)

Execute a CQL (Cypher-subset) query expression against the call graph and return a result table.

## Purpose

`graph_query` is the ad-hoc query surface: when no fixed-shape tool asks the
question you have, express it in CQL. It routes to `cgx_cql::run`
(`crates/cgx-mcp/src/tools.rs:978`) — the same engine `cgx query` drives, not a
fork — so a query that works on one surface works identically on the other, down
to row content and ordering.

The tool is read-only. Parse-time and plan-time rejections of unsupported
constructs come back as actionable errors rather than partial results.

Use [callers](callers.md), [callees](callees.md), [reaches](reaches.md),
[paths](paths.md) or [unused](unused.md) when one of them fits: they are cheaper
to call, and their answers carry a negative-completeness `scope` that a CQL
answer does not.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `query` | string | yes | — | Cypher-subset query expression. Executed. |
| `root` | string | yes | — | Path to the repository root. Must be inside a git repository; **no `.cgx/` store is required** — each call indexes in memory. See [shared parameters](README.md#shared-parameters). |
| `max_results` | integer | no | `20` | Page size. Clamped to `[1, 200]`; `0` clamps up to 1 and values above 200 clamp down, neither an error. |
| `cursor` | string | no | — | Opaque 0-based decimal offset from a prior response's `cursor`. Pass `"0"` to request the first page explicitly. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted. |

Pagination is applied **after** the query executes: `max_results` bounds the page,
not the query's own work. `total_matched` reports the full result size.

## Output shape

One tool, two output channels. `columns` is present in both; `rows` and `paths`
are mutually exclusive. A query whose `RETURN` binds a path variable populates
`paths` and omits `rows` (`tools.rs:987`); every other query emits `rows` and
omits `paths`.

| Field | Type | Meaning |
|-------|------|---------|
| `columns` | array of string | The `RETURN` expressions, in order, as written. |
| `rows` | array of array | Table channel: one array of cells per row, aligned to `columns`. |
| `paths` | array | Path channel: path objects in the same shape [paths](paths.md) returns. |
| `total_matched` | integer | Full result size before pagination. |
| `has_more` | boolean | `true` when a further page exists. |
| `cursor` | string or null | Offset to pass as `cursor` for the next page; `null` on the last page. |
| `truncated` | boolean | `true` when the query hit an evaluation budget before exhausting the graph. |
| `truncation_reason` | string or null | Machine-readable code when `truncated`; `null` otherwise. |
| `approximation` | object | Which direction this answer can be wrong in — see [Approximation on a CQL answer](#approximation-on-a-cql-answer). |
| `freshness` | object | Which tree the answer was computed over — see [the response envelope](README.md#the-response-envelope). |
| `graph_version` | string | Cache key for the graph queried (ADR-06). |
| `dirty` | boolean | `true` when the working-tree overlay was active and changed the base. |
| `dirty_files_analyzed` | integer | Paths the overlay fed the indexer differently from `HEAD`. |

### Cell shapes in the table channel

`cql_cell_json` (`tools.rs:1089`) renders each cell by its CQL value type:

| CQL value | JSON |
|-----------|------|
| node | `{"fqn", "file", "line", "kind"}` |
| edge | `{"kind", "condition", "confidence"}` — `kind` is **hyphenated** (`calls-closure`) |
| path | array of FQN strings |
| scalar (`Str`/`Int`/`Float`/`Bool`/`Null`) | the bare JSON scalar |
| list | JSON array of the above |

Projecting a property (`RETURN a.fqn`) yields a scalar cell; projecting the
binding itself (`RETURN a`) yields the object form.

## Examples

All three were captured against the two-fixture tree described in
[README — Reproducing the examples](README.md#reproducing-the-examples).

### Table channel — projected properties

Request:

```json
{
  "name": "graph_query",
  "arguments": {
    "query": "MATCH (a)-[:CALLS]->(b) WHERE a.fqn = \"rust_sample::conditions::dispatch\" RETURN a.fqn, b.fqn",
    "root": "/path/to/fixture"
  }
}
```

Response (`structuredContent`):

```json
{
  "approximation": {
    "direction": "exact",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": []
  },
  "columns": ["a.fqn", "b.fqn"],
  "cursor": null,
  "dirty": true,
  "dirty_files_analyzed": 3,
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "head_tree": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "indexed_tree": "workdir:615bbb57f594774e2f6f9ef3783699ebc299e9ab",
    "matches_head": null,
    "stale": false
  },
  "graph_version": "03575d1+dirty.6a49a1506e74",
  "has_more": false,
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

`matches_head: null` is the expected value here, not a fault: the fixture was
indexed before the call, and `include_dirty` defaults to `true`. See
[README](README.md#matches_head-is-three-valued-and-null-is-the-default-over-mcp).

### Table channel — bound edges, and an honest `over`

Binding the edge rather than only its endpoints changes the answer's honesty
report. `"query": "MATCH (a)-[r:CALLS]->(b) WHERE b.fqn = \"rust_sample::conditions::dispatch\" RETURN a, r, b"`:

```json
{
  "approximation": {
    "direction": "over",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": [
      {
        "code": "over-approx-candidate-set",
        "detail": "resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur",
        "direction": "over"
      }
    ]
  },
  "columns": ["a", "r", "b"],
  "rows": [
    [
      { "file": "fixtures/ts-sample/src/closures.ts", "fqn": "ts_sample::closures::closureVariable", "kind": "function", "line": 6 },
      { "condition": "always", "confidence": "possible", "kind": "calls-closure" },
      { "file": "fixtures/rust-sample/src/conditions.rs", "fqn": "rust_sample::conditions::dispatch", "kind": "function", "line": 42 }
    ]
  ],
  "total_matched": 3,
  "truncated": false,
  "truncation_reason": null
}
```

Elided: two further rows of the same shape (`nestedClosures` and
`nestedClosures::outer`), and the `freshness`/`graph_version`/`dirty`/
`dirty_files_analyzed`/`has_more`/`cursor` keys, identical to the previous
example.

These three cross-language edges are `possible`, `calls-closure` matches — the
TypeScript closure calls could not be resolved to a definite target, so the
candidate set is an over-approximation. The contract says so. The first example
returned `direction: "exact"` for the same graph because projecting `a.fqn` binds
no edge, and the table channel's only over-approximation signal is whether a
**bound** edge cell carries `Confidence::Possible`.

### Path channel — `RETURN path`

`"query": "MATCH path = (a)-[:CALLS*1..2]->(b) WHERE a.fqn = \"rust_sample::async_calls::fetch_data\" RETURN path"`, `"max_results": 2`:

```json
{
  "approximation": {
    "direction": "exact",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": []
  },
  "columns": ["path"],
  "cursor": "2",
  "has_more": true,
  "paths": [
    {
      "crosses_exceptional": true,
      "hops": 1,
      "min_confidence": "certain",
      "steps": [
        {
          "confidence": null,
          "edge_condition": null,
          "exception_transient": false,
          "file": "fixtures/rust-sample/src/async_calls.rs",
          "line": 7,
          "name": "rust_sample::async_calls::fetch_data"
        },
        {
          "confidence": "certain",
          "edge_condition": "exception",
          "exception_transient": true,
          "file": "fixtures/rust-sample/src/async_calls.rs",
          "line": 13,
          "name": "rust_sample::async_calls::http_get"
        }
      ]
    }
  ],
  "total_matched": 4,
  "truncated": false,
  "truncation_reason": null
}
```

Elided: the second path on this page (the `always`-edge sibling to `http_get`),
and the session-metadata keys. Note `rows` is absent, `columns` is `["path"]`,
and `has_more`/`cursor` page the path channel exactly as they page rows.

### Errors

A rejected query is a JSON-RPC error (`invalid_params`, code `-32602`) with no
result document and therefore no envelope. Parse and plan rejection are distinct
and both name a byte range:

```
parse error: expected WITH or RETURN, found identifier `RETRN` (at bytes 24..29)
plan error: a MATCH pattern must contain at least one relationship (at bytes 0..0)
```

The second is worth internalizing: a bare `MATCH (a) RETURN a.fqn` is a
plan-time rejection, not an empty result. CQL requires at least one relationship
in a `MATCH` pattern — use [search](search.md) or [symbols](symbols.md) for
node-only questions.

## Approximation on a CQL answer

`graph_query` builds its contract two different ways, matched to the channel
(`tools.rs:972-1025`):

- **Path channel** — `contract::for_path_set`: the intrinsic over-approximation
  of the enumerated paths plus any truncation marker.
- **Table channel** — `contract::over_only(table_has_over_approx_edge(..))`: the
  answer is `over` when any **bound edge cell** in the result resolved at
  `possible`, and `exact` otherwise.

Neither channel attaches a `scope` object. Every other tool's `scope` describes
the walk it performed before concluding nothing was there; a CQL query states its
own scope in its text, so restating it would be redundant rather than honest.

Two consequences to hold onto:

- **An empty table is not a negative-completeness claim.** Zero rows means the
  pattern matched nothing in the modeled graph. There is no `scope` telling you
  what edge kinds or confidence floor were searched — the query says.
- **A table answer's honesty signal is edge-shaped.** Project the edge (`RETURN
  r`, or `RETURN a, r, b`) when you need the contract to see the confidence of
  what you matched. Projecting only endpoint properties can report `exact` over
  edges that are themselves `possible`, as the two table examples above show.

`direction: "exact"` here means exact *within* `modeled_graph`, never that the
result is complete — and it does not cover the B-5 false-exact described in
[README](README.md#approximation--which-direction-the-answer-can-be-wrong-in).

## Notes

- **Same engine as `cgx query`.** Row content is identical between the two
  surfaces for the same query and tree; only the framing differs (MCP wraps the
  table in the response envelope, the CLI applies `--format`).
- **No `at` parameter.** The CLI's git-ref pinning flag is not in this tool's
  input schema. Unknown arguments are ignored without error, so passing `at` is a
  silent no-op, not a pin.
- **Read-only.** The CQL subset has no mutating construct; `graph_query` reads
  the graph and returns, exactly as `cgx query` does.
- **`truncated` is a budget signal, not a page signal.** `has_more` means "more
  rows on the next page"; `truncated` means the evaluator stopped early and rows
  you never saw may not exist in `total_matched` either.

## See also

- [callers](callers.md) — symbols that (transitively) call a named symbol
- [callees](callees.md) — symbols a named symbol (transitively) calls
- [paths](paths.md) — enumerate call paths between two symbols, with a negative-completeness scope
- [reaches](reaches.md) — a single reachability answer plus its witness path
- [unused](unused.md) — symbols unreachable from any entrypoint
- [cgx query (CLI)](../commands/query.md) — the same engine with `--format` and exit codes
- [docs/05-queries.md](../05-queries.md) — query capabilities taxonomy and the CQL subset
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge conditions and confidence tiers
