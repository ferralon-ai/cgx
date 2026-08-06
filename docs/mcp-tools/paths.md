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
| `root` | string | yes | — | Path to the repository root. Must be inside a git repository; **no `.cgx/` store is required** — each call indexes in memory. See [shared parameters](README.md#shared-parameters) |
| `max_depth` | integer | no | `10` | Maximum traversal depth. **`0` is not an unlimited sentinel over MCP** — see below |
| `max_results` | integer | no | `10` | Maximum paths to return per page. Clamped to `[1, 200]`; `0` is raised to 1 and values above 200 reduced to 200, neither an error |
| `exclude_edge_condition` | string | no | — | Exclude paths that contain any edge with this condition. One of: `always`, `conditional`, `exception`, `loop`, `panic` |
| `only_edge_condition` | string | no | — | Return only paths where every edge has this condition. One of: `always`, `conditional`, `exception`, `loop`, `panic` |
| `confidence` | string | no | — | Minimum confidence floor; edges below this tier are excluded. One of: `certain`, `probable`, `possible` |
| `cursor` | string | no | — | Pagination cursor returned by a previous response. `"0"` requests the first page explicitly |
| `include_dirty` | boolean | no | `true` | When `true`, includes facts from dirty (unindexed) files via the incremental overlay |

**`max_depth: 0` returns nothing.** The handler passes the value straight through as
a bound (`paths_call`, `crates/cgx-mcp/src/tools.rs:642`), so `0` bounds the walk at
the seed node and the result is an empty `paths` array. This differs from the CLI,
where `cgx paths --depth 0` maps to *unbounded* (`paths_max_depth`,
`crates/cgx-cli/src/main.rs:2036`). The MCP tool has no unlimited sentinel; pass a
large integer instead, subject to the work budget.

**Supplying both edge-condition filters is not an error.** `only_edge_condition`
silently wins — the handler tests it first and only falls through to
`exclude_edge_condition` when it is absent (`tools.rs:632-636`). Pass exactly one.

**No `at` parameter.** The CLI's git-ref pinning flag is not in this tool's input
schema. Unknown arguments are ignored without error, so passing `at` is a silent
no-op rather than a pin.

---

## Output shape

Every response wraps the result in the approximation contract, the index-freshness
envelope, and the ADR-06 session metadata:

```
{
  "from":              string,
  "to":                string,
  "paths":             PathResult[],
  "total_matched":     integer,
  "truncated":         boolean,
  "truncation_reason": string | null,
  "has_more":          boolean,
  "cursor":            string | null,
  "approximation":     ApproximationContract,
  "freshness":         FreshnessEnvelope,
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
| `from` | string | Echo of the `from` input — the string as received, not the resolved FQN |
| `to` | string | Echo of the `to` input |
| `paths` | array | Current page of path results |
| `total_matched` | integer | Total paths enumerated before pagination |
| `truncated` | boolean | `true` if the search hit its work budget before exhausting the graph |
| `truncation_reason` | string or null | Machine-readable reason code when `truncated` is `true`; null otherwise |
| `has_more` | boolean | `true` if additional pages exist; paginate with `cursor` |
| `cursor` | string or null | Opaque offset string to pass as `cursor` for the next page; null when no more pages |
| `approximation` | object | Which direction this answer can be wrong in, and why. See [Approximation on a path answer](#approximation-on-a-path-answer) |
| `freshness` | object | Which tree the answer was computed over and how it relates to `HEAD`. See [the response envelope](README.md#freshness--which-tree-the-answer-was-computed-over) |
| `graph_version` | string | Cache key for the graph queried: a 7-character tree OID, or `<oid>+dirty.<digest>` when the overlay changed the base (ADR-06) |
| `dirty` | boolean | `true` when the working-tree overlay was active **and** changed the base (ADR-06) |
| `dirty_files_analyzed` | integer | Count of paths the overlay fed the indexer differently from `HEAD` (ADR-06) — a property of the graph, so it need not equal `freshness.dirty_files` |

### Approximation on a path answer

`paths` builds its contract from the enumerated path set (`contract::for_paths`,
`crates/cgx-query/src/contract.rs:559`):

- **`over`** when any enumerated path traversed an over-approximated candidate set —
  its `min_confidence` dropped to `possible`, or an edge on it carried a candidate
  group. The path is reported; the contract says it may not occur.
- **`under`** from the truncation marker when the enumeration was cut short, and from
  a frontier scan when the answer is **empty**.
- **`scope`** is attached **only when `paths` is empty** on this tool — the rule is
  per-builder, and [`unused`](unused.md) carries one on every answer ([when `scope`
  is attached](README.md#when-scope-is-attached)). It names the edge kinds,
  confidence floor and depth the search covered before concluding no path exists.
  Without it, "no path within depth 10 over call edges" and "no path" are the same
  empty array.

`truncated` and the contract's truncation reason are the same fact from two angles,
in two vocabularies: `truncation_reason` carries the bare token `"step-budget"` or
`"path-cap"` (`TruncationReason::token`, `crates/cgx-query/src/walk.rs:59`), while
`approximation.reasons` carries the same event as the `under` code
`truncated-step-budget` or `truncated-path-cap` (`contract.rs:312`). A truncated
*negative* answer is not a proof of unreachability.

---

## Example

Captured against the two-fixture tree described in
[README — Reproducing the examples](README.md#reproducing-the-examples).

### Request

```json
{
  "name": "paths",
  "arguments": {
    "from": "rust_sample::async_calls::fetch_data",
    "to":   "rust_sample::async_calls::http_get",
    "root": "/path/to/fixture"
  }
}
```

### Response (`structuredContent`)

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
    "dirty_files_base": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "head_tree": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "indexed_tree": "workdir:615bbb57f594774e2f6f9ef3783699ebc299e9ab",
    "matches_head": null,
    "stale": false
  },
  "from": "rust_sample::async_calls::fetch_data",
  "graph_version": "03575d1+dirty.6a49a1506e74",
  "has_more": false,
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
    },
    {
      "crosses_exceptional": false,
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
          "edge_condition": "always",
          "exception_transient": false,
          "file": "fixtures/rust-sample/src/async_calls.rs",
          "line": 13,
          "name": "rust_sample::async_calls::http_get"
        }
      ]
    }
  ],
  "to": "rust_sample::async_calls::http_get",
  "total_matched": 2,
  "truncated": false,
  "truncation_reason": null
}
```

Two paths exist between the symbols. The first traverses an `exception` edge
(`crosses_exceptional: true`); the second traverses the unconditional `always` edge.
Both are `certain`, no candidate set was traversed and no truncation occurred, so
`reasons` is empty and `direction` folds to `exact` — within `modeled_graph`, which
still excludes external and unindexed callees. Because `paths` is non-empty, no
`scope` object is attached.

`matches_head: null` beside `dirty_files: 0` is the ordinary shape here, not a
warning: the fixture was indexed before the call and `include_dirty` defaults to
`true`, so the overlay saw the three files in `.cgx/` that git's ignore rules hide.
See [README](README.md#matches_head-is-three-valued-and-null-is-the-default-over-mcp).

---

## Notes

### MCP vs CLI defaults

| Property | MCP `paths` | CLI `cgx paths` |
|----------|-------------|-----------------|
| depth parameter name | `max_depth` | `--depth` |
| depth default | `10` (`DEFAULT_PATHS_MAX_DEPTH`) | `6` (`PATHS_DEFAULT_MAX_DEPTH`) |
| depth `0` | seed only — empty result | **unbounded** |
| result limit | `10` (paginated, hard cap 200) | unlimited |
| `include_dirty` | `true` | no equivalent |

The property name difference is significant: passing `depth` instead of `max_depth`
to this tool is silently ignored, and the walk runs at the default `10`.

The depth-`0` row is the trap. The two surfaces disagree deliberately — the CLI maps
`--depth 0` to "no bound" (`paths_max_depth`, `crates/cgx-cli/src/main.rs:2036`),
while the MCP handler passes the integer through as a bound. A client that ports a
working CLI invocation to this tool by copying `0` gets an empty result, not an
unbounded search.

### Pagination

Results are paginated. When `has_more` is `true`, pass the returned `cursor` value
as `cursor` in the next request with identical other parameters to retrieve the next
page. The cursor is a decimal offset string; `"0"` is a valid explicit first-page
request. Values above 200 for `max_results` are clamped to 200.

### Empty result

An empty `paths` array with `truncated: false` means no call path was found between
the two symbols **within the searched scope**. This is a successful response, not an
error — and it is the case where `approximation.scope` appears, naming the edge
kinds, confidence floor and depth that were searched. Read `scope` before treating
an empty result as an absence proof; a `certain` floor or a low `max_depth` narrows
the claim considerably.

### Work-budget truncation

The depth bound and the internal work budget are separate limits. Raising `max_depth`
does not remove the budget: on dense graphs the search may halt early, `truncated`
becomes `true`, and `truncation_reason` carries `"step-budget"` or `"path-cap"`. The
same event appears in `approximation.reasons` as an `under` code.

### Confidence filtering

The `confidence` parameter sets a minimum floor. Edges below the floor are excluded
from all path searches. Filtering to `certain` removes speculative edges and may
produce an empty result set even when paths exist at lower confidence tiers — the
`confidence_floor` field of `approximation.scope` records which floor produced the
empty answer.

### Edge-condition filtering

`only_edge_condition` and `exclude_edge_condition` operate on the edges within each
path, not on the path set as a whole. A path is included when it satisfies the
condition predicate for every one of its edges (for `only_edge_condition`) or for
none of them (for `exclude_edge_condition`). Supplying both is not rejected:
`only_edge_condition` silently wins.

### Errors

An unresolvable `from` or `to`, an unknown condition token, or an unparseable
`cursor` comes back as a JSON-RPC error object (`-32602`) with no result document —
so no `paths`, no `freshness`, no `approximation`. Do not code a client to look for
the envelope on a failed call.

### Method calls on an untyped receiver

A virtual-receiver call whose type cannot be resolved in-repo, where exactly one
method anywhere shares the short name, is banded `probable`. `probable` does not trip
the contract's `over` predicate, so a path built on such an edge can appear inside an
answer reported as `direction: "exact"` with `min_confidence: "probable"`. Backlog
**B-5**. Where a path crosses duck-typed or cross-class same-name method calls, read
the per-step `confidence` rather than trusting the contract's summary.

---

## See also

- [callers](callers.md) — all symbols that (transitively) call a target
- [callees](callees.md) — all symbols a source (transitively) calls
- [reaches](reaches.md) — whether a path exists at all, with a single witness instead of an enumeration
- [explain](explain.md) — full provenance for one symbol including all incident edges
- [graph_query](graph_query.md) — `RETURN path` produces the same path shape from a CQL pattern
- [cgx paths](../commands/paths.md) — CLI equivalent with `--format`, `--assert-empty`, exit codes
- [README](README.md#the-response-envelope) — the shared response envelope in full
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and exceptional-class semantics
