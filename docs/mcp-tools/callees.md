# callees (MCP tool)

Symbols the named symbol (transitively, to `depth`) calls.

## Purpose

`callees` walks the call graph forward from a named symbol and returns every symbol it transitively invokes, up to the requested depth. Each result carries the edge condition, confidence tier, and source location of the callee.

This is the forward counterpart of [`callers`](callers.md). Use `callees` when you want to understand the blast radius of a change to a given symbol, enumerate what a function depends on, or verify that a function is a leaf.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Fully-qualified or short symbol name to expand as the call-graph root. The FQN is tried first, then the short name; if neither resolves the call returns an error. |
| `root` | string | yes | — | Path to the repository root. Must be inside a git repository; **no `.cgx/` store is required** — each call indexes in memory. See [shared parameters](README.md#shared-parameters). |
| `depth` | integer | no | `1` | Maximum traversal depth. `1` returns direct callees only. There is no unlimited-depth option via MCP, and `0` is not one: it bounds the walk at the seed and returns an empty result. Set a large integer to approximate unlimited, subject to the work budget. |
| `max_results` | integer | no | `20` | Maximum results per page. Clamped to `[1, 200]`; values above 200 are silently capped and `0` is silently raised to 1, neither rejected. |
| `cursor` | string | no | — | Pagination cursor from a prior response. Pass the `cursor` value returned in a previous result to fetch the next page. Omit or pass `null` for the first page. |
| `edge_condition` | string | no | — | Restrict the walk to a single edge condition. One of `always`, `conditional`, `exception`, `loop`, `panic`. When omitted, all edge conditions are returned. |
| `confidence` | string | no | — | Minimum confidence floor. One of `certain`, `probable`, `possible`. Edges below this tier are excluded. When omitted, all tiers are returned. |
| `kind` | array of string | no | — | Restrict the walk to these call-edge kinds: `calls`, `calls_virtual`, `calls_closure`, `calls_callback`, `calls_async`, `calls_indirect`, `spawns`. An empty array is treated as absent; an unrecognized token is an error. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay. The overlay is never persisted. Set `false` to index and query the committed tree (`HEAD`) instead. |

**`edge_condition` is a restriction, not an exclusion.** `"edge_condition": "conditional"` returns only `conditional` edges; `always` edges are filtered out with everything else (`neighbor_filter`, `crates/cgx-mcp/src/tools.rs:427`).

**`kind` tokens are underscored on input.** `calls_virtual` is accepted; `calls-virtual` is rejected as an unknown edge kind. The hyphenated spelling is the *output* vocabulary, used in `approximation.scope.searched_edge_kinds`.

## Output shape

A successful response is a JSON object with the following fields:

| Field | Type | Meaning |
|-------|------|---------|
| `symbol` | string | The input symbol name as supplied — not the resolved FQN. |
| `results` | array | The current page of callee results (see item fields below). |
| `total_matched` | integer | Total number of callees matched before pagination. |
| `has_more` | boolean | `true` when a subsequent page exists. |
| `cursor` | string or null | Opaque pagination cursor. Pass to the next request to retrieve the next page. `null` when `has_more` is `false`. |
| `approximation` | object | The answer-honesty contract: which direction this answer can be wrong in, and why. See [the response envelope](README.md#approximation--which-direction-the-answer-can-be-wrong-in). |
| `freshness` | object | Which tree the answer was computed over and how it relates to `HEAD`. See [the response envelope](README.md#freshness--which-tree-the-answer-was-computed-over). |
| `graph_version` | string | Cache key for the graph queried: a 7-character tree OID, or `<oid>+dirty.<digest>` when the overlay changed the base (ADR-06). |
| `dirty` | boolean | `true` when the overlay was active **and** changed the base. |
| `dirty_files_analyzed` | integer | Count of paths the overlay fed the indexer differently from `HEAD` — a property of the graph, not of the checkout, and not the same number as `freshness.dirty_files`. |

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

**Reading `approximation` on a forward walk.** `callees` traverses out-edges, so it is the direction in which dangling references matter: a call site whose target left no edge at all is reported as an `under` reason (`unresolved-call` on the frontier, `unresolved-external-calls` for a dangling ref). An `over` reason appears when any reported path went through an over-approximated candidate set. When `results` is empty the contract grows a `scope` object naming the edge kinds, confidence floor and depth the walk searched — the difference between "this is a leaf" and "this is a leaf *within these bounds*".

## Example

Captured against the two-fixture tree described in [README — Reproducing the examples](README.md#reproducing-the-examples).

Request:

```json
{
  "name": "callees",
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
  "graph_version": "03575d1+dirty.6a49a1506e74",
  "has_more": false,
  "results": [
    {
      "confidence": "certain",
      "depth": 1,
      "edge_condition": "conditional",
      "exception_transient": false,
      "file": "fixtures/rust-sample/src/conditions.rs",
      "line": 4,
      "min_confidence_on_path": "certain",
      "name": "rust_sample::conditions::log_info"
    },
    {
      "confidence": "certain",
      "depth": 1,
      "edge_condition": "conditional",
      "exception_transient": false,
      "file": "fixtures/rust-sample/src/conditions.rs",
      "line": 8,
      "min_confidence_on_path": "certain",
      "name": "rust_sample::conditions::log_warn"
    },
    {
      "confidence": "certain",
      "depth": 1,
      "edge_condition": "conditional",
      "exception_transient": false,
      "file": "fixtures/rust-sample/src/conditions.rs",
      "line": 12,
      "min_confidence_on_path": "certain",
      "name": "rust_sample::conditions::log_error"
    }
  ],
  "symbol": "rust_sample::conditions::dispatch",
  "total_matched": 3
}
```

Three `certain` edges, each `conditional` because `dispatch` calls each logger on a different branch. `direction: "exact"` here has a precise meaning: within `modeled_graph`, at depth 1, nothing was over- or under-counted. It is not a claim that `dispatch` calls nothing else — deeper edges were simply not searched, and at depth 1 the depth horizon coincides with the answer.

Contrast the [`callers`](callers.md#a-positive-answer) direction on the same symbol, which reports `over_under`: its inbound edges are cross-language closure calls resolved through a candidate set, and its frontier carries unresolved calls.

`matches_head: null` is the ordinary value here, not a fault — the fixture was indexed before the call and `include_dirty` defaults to `true`. See [README](README.md#matches_head-is-three-valued-and-null-is-the-default-over-mcp).

## Notes

**MCP vs CLI defaults.** The MCP `callees` tool defaults `depth` to `1`; the CLI `cgx callees` defaults `--depth` to `2`. Set `depth` explicitly when you need behavior consistent with the CLI.

**Pagination.** Results are paged at `max_results` per call (default 20, hard cap 200). When `has_more` is `true`, pass the returned `cursor` value in a follow-up request with all other parameters unchanged to retrieve the next page. The cursor is a decimal offset string; `"0"` is a valid first-page cursor equivalent to omitting the field.

**Ordering.** `results` is sorted by `(file, line, fqn)`, not by depth (`neighbor_walk`, `crates/cgx-query/src/engine.rs:51`). The walk itself is a breadth-first frontier expansion, so each reachable node appears once, at its shortest depth; a cycle cannot produce duplicate entries.

**Edge-condition rendering in results.** The `edge_condition` field always returns the raw token string (`always`, `conditional`, `exception`, `loop`, `panic`). This differs from CLI human output, which omits `always` and abbreviates `conditional` to `[if]` and `exception` to `[exc]`.

**`include_dirty` default.** The MCP tool defaults `include_dirty` to `true`, meaning uncommitted edits are reflected in results by default. This is the opposite of the CLI, which has no equivalent flag and always queries the last committed index. Pass `"include_dirty": false` when you need a result tied to the committed tree rather than the working directory — note that this also changes the `freshness` envelope, since the working tree is then never inspected (`dirty_files: null`, `matches_head: true`).

**No `at` parameter.** The `--at` git-ref flag available on the CLI is not wired into the MCP tool schema. Unknown arguments are ignored without error, so passing `at` is a silent no-op rather than a pin.

**Errors carry no envelope.** An unresolved symbol, an unknown `kind` token, or an unparseable `cursor` comes back as a JSON-RPC error object (`-32602`) with no result document — so no `freshness`, no `approximation`, no `graph_version`.

**Method calls on an untyped receiver.** A virtual-receiver call whose type cannot be resolved in-repo, where exactly one method anywhere shares the short name, is banded `probable`. `probable` does not trip the contract's `over` predicate, so such an answer can report `direction: "exact"` while carrying one wrong edge. Backlog **B-5** — treat `exact` with least confidence on duck-typed and cross-class same-name method calls.

## See also

- [callers](callers.md) — reverse direction: symbols that transitively call the named symbol
- [reaches](reaches.md) — whether one symbol reaches another, with a witness path
- [paths](paths.md) — enumerate every call path between two specific symbols
- [explain](explain.md) — full provenance for one symbol, including all incident edges
- [unused](unused.md) — symbols not reachable from any entrypoint
- [cgx callees (CLI)](../commands/callees.md) — CLI equivalent with `--format`, `--assert-empty`, and exit codes
- [README](README.md#the-response-envelope) — the shared response envelope in full
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and transience semantics
