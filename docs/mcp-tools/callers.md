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
| `root` | string | yes | — | Path to the repository root. Must be inside a git repository; **no `.cgx/` store is required** — each call indexes in memory. See [shared parameters](README.md#shared-parameters). |
| `depth` | integer | no | `1` | Maximum backward traversal depth. `0` is not an "unlimited" sentinel — it bounds the walk at the seed and returns an empty result. |
| `max_results` | integer | no | `20` | Page size. Clamped to `[1, 200]`; values above 200 are silently capped and `0` is silently raised to 1. |
| `cursor` | string | no | — | Opaque pagination cursor returned by a previous response. Pass `"0"` to force an explicit first-page request. |
| `edge_condition` | string | no | — | Restrict the walk to a single edge-condition class. One of: `always`, `conditional`, `exception`, `loop`, `panic`. |
| `confidence` | string | no | — | Minimum confidence floor. One of: `certain`, `probable`, `possible`. No value = all tiers shown. |
| `kind` | array of string | no | — | Restrict the walk to these call-edge kinds. Tokens: `calls`, `calls_virtual`, `calls_closure`, `calls_callback`, `calls_async`, `calls_indirect`, `spawns`. An empty array is treated as absent. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted. |

**`edge_condition` restricts, it does not exclude.** `"edge_condition":
"conditional"` returns only edges whose condition is `conditional`; every other
edge, `always` included, is filtered out (`neighbor_filter`,
`crates/cgx-mcp/src/tools.rs:427`).

**`kind` tokens are underscored on input and hyphenated on output.** The schema
accepts `calls_virtual`; passing `calls-virtual` is rejected with
`unknown edge kind \`calls-virtual\``. The same kind is reported back
hyphenated in `approximation.scope.searched_edge_kinds`. Input and output
vocabularies differ here; do not round-trip one into the other.

**Note:** The `at` (git ref) parameter documented in older references does not
exist in the MCP input schema. Unknown arguments are ignored without error, so
passing `at` is a silent no-op rather than a pin.

**MCP vs CLI depth default:** The MCP default depth is `1` (direct callers
only). The CLI `cgx callers` default is `2`. Pass an explicit `depth` if you
need the same results as a CLI invocation.

## Output shape

The response envelope includes:

| Field | Type | Meaning |
|-------|------|---------|
| `symbol` | string | The symbol name as received — the input string, not the resolved FQN. |
| `results` | array | Current page of caller records (see below). |
| `total_matched` | integer | Total callers found before pagination. |
| `has_more` | boolean | `true` if more pages remain. |
| `cursor` | string or null | Opaque cursor for the next page; `null` on the last page. |
| `approximation` | object | Which direction this answer can be wrong in, and why. See [the response envelope](README.md#approximation--which-direction-the-answer-can-be-wrong-in). |
| `freshness` | object | Which tree the answer was computed over and how it relates to `HEAD`. See [the response envelope](README.md#freshness--which-tree-the-answer-was-computed-over). |
| `graph_version` | string | Cache key for the graph queried: a 7-character tree OID, or `<oid>+dirty.<digest>` when the working-tree overlay changed the base (ADR-06). |
| `dirty` | boolean | `true` if working-tree changes were analyzed **and** changed the base. |
| `dirty_files_analyzed` | integer | Count of paths the overlay fed the indexer differently from `HEAD`. Not the same number as `freshness.dirty_files`, and not expected to be. |

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

### Reading the two honesty objects

`approximation` reports the answer's direction of error. For a caller walk both
directions are reachable at once: `over` when any reported path traversed an
over-approximated candidate set, `under` when the frontier hit unresolved calls
or the depth limit. A depth-bounded `callers` call is therefore `over_under`
more often than not — that is the contract working, not a defect.

`approximation.scope` appears **only when `results` is empty** on this tool (the
rule differs per tool — [`unused`](unused.md) always carries one; see [when
`scope` is attached](README.md#when-scope-is-attached)): it names the edge
kinds, confidence floor and depth the walk actually searched before concluding
there were no callers. Read it before treating an empty answer as evidence of
dead code.

`freshness.matches_head` is three-valued and `null` is the ordinary value over
MCP for any repository that has been indexed. See
[README](README.md#matches_head-is-three-valued-and-null-is-the-default-over-mcp).

## Examples

Both were captured against the two-fixture tree described in
[README — Reproducing the examples](README.md#reproducing-the-examples).

### A positive answer

Request:

```json
{
  "name": "callers",
  "arguments": {
    "symbol": "rust_sample::conditions::dispatch",
    "root": "/path/to/fixture"
  }
}
```

Response (`structuredContent`):

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
    "dirty_files_base": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "head_tree": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "indexed_tree": "workdir:615bbb57f594774e2f6f9ef3783699ebc299e9ab",
    "matches_head": null,
    "stale": false
  },
  "graph_version": "03575d1+dirty.6a49a1506e74",
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

All three callers arrive at `possible` — they are cross-language closure calls
resolved through a candidate set, not definite edges. `min_confidence_on_path`
carries the same value, so a client filtering on `certain` sees nothing here.
The contract's `over` reason is exactly this, stated once at the top rather than
left for the reader to infer from three per-edge fields.

### An empty answer, and the scope that makes it readable

Restricting the same call to plain `calls` edges filters those three closure
edges out. The result is empty and `approximation` grows a `scope`:

```json
{
  "approximation": {
    "direction": "exact",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": [],
    "scope": {
      "confidence_floor": "possible",
      "max_depth": 1,
      "searched_edge_kinds": ["calls"]
    }
  },
  "results": [],
  "symbol": "rust_sample::conditions::dispatch",
  "total_matched": 0,
  "has_more": false,
  "cursor": null
}
```

Elided: the `freshness`/`graph_version`/`dirty`/`dirty_files_analyzed` keys,
identical to the previous example. The request differed only by
`"kind": ["calls"]`.

Without `scope`, "no callers at `calls`, floor `possible`, depth 1" and "no
callers at all" are the same empty array. With it, they are distinguishable —
which is the whole point of the negative-completeness envelope.

## Notes

- **Pagination:** Increment through pages by passing the `cursor` value from
  each response as the `cursor` of the next request. The cursor is a decimal
  offset string; `null` cursor means the last page.
- **Hard cap:** `max_results` values above 200 are silently clamped to 200.
  There is no error.
- **Dirty overlay:** `include_dirty` defaults to `true`. The overlay is
  content-addressed and computed per call; it is never written to disk. Set
  `include_dirty: false` to index and query the committed tree (`HEAD`) instead —
  which also changes the `freshness` envelope, since the working tree is then
  never inspected (`dirty_files: null`, `matches_head: true`).
- **Edge-condition rendering:** The MCP response returns the raw string token
  (`"always"`, `"conditional"`, etc.) for every edge. This differs from the
  CLI, which omits `always` and abbreviates `conditional` to `[if]` and
  `exception` to `[exc]`.
- **Cycles and ordering:** The walk is a breadth-first frontier expansion
  (`neighbor_walk`, `crates/cgx-query/src/engine.rs:51`), so each reachable node
  is discovered once at its shortest depth — a cycle cannot produce duplicate
  entries. `results` is sorted by `(file, line, fqn)`, not by depth.
- **Symbol resolution:** Both FQNs and unambiguous short names are accepted —
  the FQN is tried first, then the short name. If neither matches, the call
  returns a JSON-RPC error with no envelope.
- **Errors carry no envelope.** An unresolvable symbol or a bad argument is a
  JSON-RPC error object (`-32602`), not a result document with `freshness` on
  it.
- **Method calls on an untyped receiver.** Where a receiver's type is not
  resolvable in-repo and exactly one method anywhere shares the short name, the
  edge is banded `probable` — which does **not** trip the contract's `over`
  predicate, so such an answer can report `direction: "exact"` while containing
  one wrong edge. Backlog **B-5**. Duck-typed and cross-class same-name method
  calls are the shapes where a `callers` answer's `exact` deserves least trust.

## See also

- [callees](callees.md) — the forward direction: symbols the named symbol calls
- [reaches](reaches.md) — whether one symbol reaches another, with a witness path
- [paths](paths.md) — enumerate every call path between two symbols
- [explain](explain.md) — full provenance for one symbol including all incident edges
- [unused](unused.md) — symbols not reachable from any entrypoint
- [cgx callers (CLI)](../commands/callers.md) — CLI equivalent with output format and exit-code reference
- [README](README.md#the-response-envelope) — the shared response envelope in full
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
