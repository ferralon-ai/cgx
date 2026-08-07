# reaches (MCP tool)

Reachability over call edges: with `to`, whether `from` reaches `to` (with a witness path); without `to`, every symbol `from` reaches.

## Purpose

`reaches` answers two different questions off one tool, selected by whether `to`
is supplied:

- **`from → to`** — *can control reach this symbol from that one?* Returns a
  boolean plus, when reachable, one **witness path** (the shortest, by hop count)
  that proves it. Use it as a gate: a positive answer is proven by its witness, so
  no scan of the rest of the graph is needed.
- **`from → *`** — *what can this symbol reach?* Returns the paginated set of
  every symbol reachable within `depth`, ordered by `(file, line, col, fqn)`.

The two forms return **different result keys** (see [Output shape](#output-shape))
and apply **different `depth` defaults** (see
[The `depth` default is form-dependent](#the-depth-default-is-form-dependent)).

Traversal is over the call-edge family only. `reaches` accepts no `kind` or
`edge_condition` filter — the walk is `EdgeFilter::calls()` plus an optional
confidence floor (`crates/cgx-mcp/src/tools.rs:767`).

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `from` | string | yes | — | Source symbol (FQN or unambiguous short name). |
| `to` | string | no | — | Target symbol. Omit — or pass `""` — for the `from → *` reachable set. |
| `root` | string | yes | — | Repository root path. A `.cgx/` store is not required; the server indexes per call into an in-memory store. |
| `depth` | integer | no | `2` in the `from → *` form; **unbounded** in the `from → to` form | Maximum traversal depth. The registry advertises `2` for both; the handler applies it to one. See the note below. |
| `confidence` | string | no | — | Minimum confidence floor. One of `certain`, `probable`, `possible`. Omitted = every edge. |
| `max_results` | integer | no | `20` | Page size, `from → *` form only. Clamped to `[1, 200]` (`MAX_MAX_RESULTS`, `crates/cgx-mcp/src/tools.rs:28`); `0` clamps up to 1. |
| `cursor` | string | no | — | Opaque pagination cursor, `from → *` form only. A decimal offset string; an unparseable value is an `invalid_params` error. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted. |

## Output shape

### `from → to` form

| Field | Type | Meaning |
|-------|------|---------|
| `from` | string | Echo of the `from` input, as received. |
| `to` | string | Echo of the `to` input, as received. |
| `reachable` | boolean | Whether a path was found within the filter and depth bound. |
| `witness` | object or null | One proving path when `reachable` is `true`; `null` otherwise. |

This form has **no pagination keys** — no `total_matched`, no `has_more`, no
`cursor`. One witness is the whole answer; use [paths](paths.md) to enumerate the
rest.

`witness` is the same path object [paths](paths.md) returns: `hops`,
`min_confidence`, `crosses_exceptional`, and a `steps` array whose first step
carries `edge_condition: null` and `confidence: null` (it has no incoming edge).

### `from → *` form

| Field | Type | Meaning |
|-------|------|---------|
| `from` | string | Echo of the `from` input. |
| `results` | array | Current page of reachable-symbol records. |
| `total_matched` | integer | Total reachable symbols before pagination. |
| `has_more` | boolean | `true` when more pages remain. |
| `cursor` | string or null | Cursor for the next page; `null` on the last page. |

Each record in `results` is the neighbor shape `callers`/`callees` use: `name`,
`file`, `line`, `depth`, `edge_condition`, `confidence`,
`min_confidence_on_path`, `exception_transient`.

### Response envelope

Both forms carry the same five envelope fields, in every response:

| Field | Type | Meaning |
|-------|------|---------|
| `approximation` | object | Which direction this answer can be wrong, and why. `direction` is a pure fold of `reasons[]`; `scope` is present **only** on an answer that found nothing. |
| `graph_version` | string | Content key of the graph queried (ADR-06): a short tree OID, or `<short-tree-oid>+dirty.<digest>` when the working-tree overlay changed the base. |
| `dirty` | boolean | Whether the overlay was active *and* changed the base. |
| `dirty_files_analyzed` | integer | How many paths the overlay fed the indexer differently from `HEAD`. Ignore rules are **not** applied here. |
| `freshness` | object | Index-freshness envelope: `indexed_tree`, `head_tree`, `matches_head`, `dirty_files`, `dirty_files_base`, `stale`. |

A **positive** `from → to` answer is proven by its witness, so the contract needs
no frontier scan: only over-approximation can apply, and `scope` is absent
(`for_reaches`, `crates/cgx-query/src/contract.rs:511`). A **negative** answer
carries `scope` — the searched edge kinds, the confidence floor, and the depth
bound — because that is what makes "not reachable" a checkable claim rather than
an assertion.

## Example

Captured against the shared two-fixture tree described in
[Reproducing the examples](README.md#reproducing-the-examples). The `root` is
written here as `/path/to/fixtures`; every response below is the captured answer
verbatim.

### Request — `from → to`, no `depth`

```json
{
  "name": "reaches",
  "arguments": {
    "from": "rust_sample::closures::nested_closures",
    "to": "rust_sample::direct::add",
    "root": "/path/to/fixtures"
  }
}
```

### Response

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
  "from": "rust_sample::closures::nested_closures",
  "graph_version": "03575d1+dirty.6a49a1506e74",
  "reachable": true,
  "to": "rust_sample::direct::add",
  "witness": {
    "crosses_exceptional": false,
    "hops": 3,
    "min_confidence": "possible",
    "steps": [
      {
        "confidence": null,
        "edge_condition": null,
        "exception_transient": false,
        "file": "fixtures/rust-sample/src/closures.rs",
        "line": 60,
        "name": "rust_sample::closures::nested_closures"
      },
      {
        "confidence": "possible",
        "edge_condition": "always",
        "exception_transient": false,
        "file": "fixtures/ts-sample/src/closures.ts",
        "line": 58,
        "name": "ts_sample::closures::nestedClosures::outer"
      },
      {
        "confidence": "possible",
        "edge_condition": "always",
        "exception_transient": false,
        "file": "fixtures/rust-sample/src/imports.rs",
        "line": 15,
        "name": "rust_sample::imports::double_via_reexport"
      },
      {
        "confidence": "possible",
        "edge_condition": "always",
        "exception_transient": false,
        "file": "fixtures/rust-sample/src/direct.rs",
        "line": 6,
        "name": "rust_sample::direct::add"
      }
    ]
  }
}
```

**`reachable: true` is not the whole answer here.** Every edge on the witness is
`possible`, the weakest tier, and the contract reports `direction: "over"` with
`over-approx-candidate-set` — the walk crossed edges resolved through a candidate
set rather than a corroborated binding, and some of them may not occur at
runtime. A positive `reaches` answer means *cgx found a route in the modeled
graph*; whether that route exists in the program is exactly what `min_confidence`
and `approximation.direction` are there to tell you. Read them before treating a
`true` as a proof.

(The witness crosses from Rust into TypeScript and back — the two fixtures are
indexed together, and cross-language edges resolve at `possible`.)

### Request — the same query with the advertised default made explicit

```json
{
  "name": "reaches",
  "arguments": {
    "from": "rust_sample::closures::nested_closures",
    "to": "rust_sample::direct::add",
    "depth": 2,
    "root": "/path/to/fixtures"
  }
}
```

### Response (envelope fields elided — `…` marks the elision)

```json
{
  "approximation": {
    "direction": "under",
    "modeled_graph": "…",
    "reasons": [
      {
        "code": "unresolved-external-calls",
        "detail": "…",
        "direction": "under"
      },
      {
        "code": "foreign-function",
        "detail": "…",
        "direction": "under"
      },
      {
        "code": "unresolved-call",
        "detail": "…",
        "direction": "under"
      },
      {
        "code": "depth-limit",
        "detail": "search stopped at depth 2; deeper edges were not explored",
        "direction": "under"
      }
    ],
    "scope": {
      "confidence_floor": "possible",
      "max_depth": 2,
      "searched_edge_kinds": [
        "calls",
        "calls-virtual",
        "calls-closure",
        "calls-callback",
        "calls-async",
        "calls-indirect"
      ]
    }
  },
  "from": "rust_sample::closures::nested_closures",
  "reachable": false,
  "to": "rust_sample::direct::add",
  "witness": null
}
```

Same graph, same symbols, opposite answer — and the difference is entirely the
`depth` argument the schema says is the default. The negative answer is honest
about why: `depth-limit` names the truncation, `scope.max_depth: 2` states the
bound the search ran under, and three further `under` reasons name frontier the
walk could not follow at all.

## Notes

### The `depth` default is form-dependent

The registry declares `"depth": {"default": 2}` for the whole tool. The handler
applies that default to **one** form:

| Form | Handler | Absent `depth` means |
|------|---------|----------------------|
| `from → to` | `max_depth: opt_u32(args, "depth")?` (`crates/cgx-mcp/src/tools.rs:778`) | `None` — **unbounded** |
| `from → *` | `.unwrap_or(DEFAULT_FOREST_DEPTH)` (`crates/cgx-mcp/src/tools.rs:796`) | `2` |

This is a schema-vs-behaviour mismatch, not a documentation shortcut: the two
requests in the example above differ only by the value the schema calls the
default, and they return different answers. Read the emitted
`approximation.scope.max_depth` — `null` for unbounded, an integer for a bound —
rather than inferring the depth from the schema.

Practical consequence: a `from → to` gate is unbounded by default, which is
usually what a caller wants; pass an explicit `depth` only to bound the work, and
expect a bounded negative to mean "not within `depth`", never "not at all".

### An empty `to` is an omitted `to`

`to` is read through `.filter(|s| !s.is_empty())` (`crates/cgx-mcp/src/tools.rs:772`),
so `"to": ""` selects the `from → *` form rather than failing or searching for a
symbol named `""`. A client that always sends every key with a default of `""`
gets the forest, not an error.

### No `kind` or `edge_condition` filter

Unlike `callers`/`callees`, `reaches` exposes no edge-kind or edge-condition
narrowing. The traversal is the full call family — the six kinds listed in
`scope.searched_edge_kinds` above — filtered only by `confidence`. For a
condition-filtered reachability question, use [paths](paths.md)
(`only_edge_condition` / `exclude_edge_condition`) or
[graph_query](graph_query.md).

### `matches_head` is three-valued, and `null` is the common case

`freshness.matches_head` is `true`, `false`, or `null` — never treat it as a
boolean. The example above shows `null` on a `git status`-clean fixture, which is
the ordinary case, not an edge case: once `cgx index` has written `.cgx/`, the
overlay enumeration (no ignore rules) sees those files while git's dirty count
(ignore rules applied) does not, and the two views disagree about whether the
graph is `HEAD`'s tree (`crates/cgx-mcp/src/session.rs:196`). Asserting either
answer would be a claim nobody established, so the envelope declines.

Three live states from the same fixture, same query:

| Call | `indexed_tree` | `matches_head` | `dirty_files` |
|------|----------------|----------------|---------------|
| default `include_dirty: true`, tree contains `.cgx/` | `workdir:615bbb5…` | `null` | `0` |
| default `include_dirty: true`, tree never indexed on disk | `03575d1…` | `true` | `0` |
| `include_dirty: false` | `03575d1…` | `true` | `null` |

`dirty_files: 0` (the working tree was inspected and found clean) and
`dirty_files: null` (never inspected) are deliberately distinguishable.

### Pagination applies to one form

`max_results` and `cursor` are ignored by the `from → to` form, which returns at
most one witness. In the `from → *` form the cursor is a decimal offset string;
`"0"` is a valid explicit first page, and `cursor` is `null` when `has_more` is
`false`.

### Errors carry no envelope

A tool error is a JSON-RPC error object with no `structuredContent`, so it
carries neither `approximation` nor `freshness`. An unresolvable symbol returns
`{"code": -32602, "message": "no symbol matched pattern \`nosuch\`"}`.

### Symbol resolution

`from` and `to` accept an FQN or an unambiguous short name (FQN is tried first).
A pattern matching zero symbols — or more than one — is a resolve error, not an
empty answer.

## See also

- [paths](paths.md) — enumerate *every* path between two symbols, not just one witness
- [callees](callees.md) — the same forward neighborhood, named for the "what does this call" question
- [callers](callers.md) — the backward direction
- [explain](explain.md) — full provenance for one symbol including all incident edges
- [cgx reaches (CLI)](../commands/reaches.md) — CLI equivalent with output formats and exit codes
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
