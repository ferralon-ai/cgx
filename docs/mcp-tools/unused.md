# unused (MCP tool)

Symbols not reachable from any entrypoint (the complement of entrypoint reachability).

## Purpose

`unused` queries the graph for every indexed symbol that no entrypoint can reach — directly or transitively — and returns them as a paginated flat list. Use it to surface dead code for review or to drive automated dead-code gates in CI.

A symbol is unused when no walk from any indexed entrypoint reaches it. If a project has no indexed entrypoints, every symbol may appear as unused; run `cgx doctor` to verify entrypoint coverage before interpreting results.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `root` | string | yes | — | Path to the repository root. Must be inside a git repository; **no `.cgx/` store is required** — each call indexes in memory. See [shared parameters](README.md#shared-parameters). |
| `kind` | string | no | `"all"` | Restrict results to one symbol kind. Enum: `"function"`, `"method"`, `"field"`, `"all"`. `"all"` returns every kind. |
| `entrypoint` | string | no | — | FQN or unambiguous short name of a single symbol to use as the sole reachability root, overriding the graph's indexed entrypoints. |
| `max_results` | integer | no | `20` | Maximum results per page. Clamped to `[1, 200]`; values above 200 are silently reduced to 200. |
| `cursor` | string | no | — | Opaque pagination cursor returned by a prior call. Omit for the first page. |
| `include_dirty` | boolean | no | `true` | When `true`, analyzes uncommitted working-tree changes via a per-call content-addressed overlay. The overlay is never persisted. |

**`kind` differs from CLI.** The MCP `kind` enum includes `"all"` (return every kind); the CLI `--kind` flag does not accept `"all"` — it accepts the full symbol-kind vocabulary (`function`, `method`, `type`, `field`, `variable`, `module`, `constant`, `macro`, `lambda`, `entrypoint`). The MCP enum is narrower and MCP-specific.

## Output shape

Every response includes the approximation contract added by `with_contract()` and the session metadata plus freshness envelope added by `with_session_meta()`:

| Field | Type | Meaning |
|-------|------|---------|
| `results` | array | Page of matching symbol records (see below). |
| `total_matched` | integer | Total symbols matched before pagination. |
| `has_more` | boolean | `true` when more pages are available. |
| `cursor` | string or null | Pass as `cursor` in the next call to fetch the next page. `null` on the last page. |
| `approximation` | object | The answer-honesty contract. On `unused` it **always** carries a `scope` — see below. |
| `freshness` | object | Which tree the answer was computed over and how it relates to `HEAD`. See [the response envelope](README.md#freshness--which-tree-the-answer-was-computed-over). |
| `graph_version` | string | Cache key for the graph queried: a 7-character tree OID, or `<oid>+dirty.<digest>` when the working-tree overlay changed the base (ADR-06). |
| `dirty` | boolean | `true` when the overlay was active **and** changed the base (ADR-06). |
| `dirty_files_analyzed` | integer | Count of paths the overlay fed the indexer differently from `HEAD` (ADR-06) — a property of the graph, so it need not equal `freshness.dirty_files`. |

Each record in `results`:

| Field | Type | Meaning |
|-------|------|---------|
| `name` | string | Fully-qualified name of the unused symbol. |
| `file` | string | Source file path (relative to `root`). |
| `line` | integer | Line number of the symbol definition. |
| `kind` | string | Symbol kind (lowercased). |

### `unused` is a negative claim, so it always states its scope

The traversal tools attach `approximation.scope` only when they found nothing; `graph_query` and `coupling` never attach it at all. `unused` attaches it **unconditionally — including on a non-empty list** (`contract::for_unused`, `crates/cgx-query/src/contract.rs:601`, whose doc comment reads "Scope is always attached"). The whole answer *is* an assertion that nothing reaches these symbols, so the bounds of the search that concluded it are part of the claim whether the list has 0 entries or 199.

The worked example below shows `scope` on a populated result — three entries out of 199 matched. Do not code a client to look for it only when `results` is empty; see [when `scope` is attached](README.md#when-scope-is-attached).

```json
"scope": {
  "confidence_floor": "possible",
  "max_depth": null,
  "searched_edge_kinds": ["calls", "calls-virtual", "calls-closure", "calls-callback", "calls-async", "calls-indirect"]
}
```

`max_depth: null` means the reachability walk was unbounded, `confidence_floor: "possible"` that it followed even the weakest edges — the most generous search available, which is what makes a negative answer worth something. Note `spawns` is **not** among the searched kinds: a symbol reached only by a thread spawn is reported as unused.

`unused` never reports `direction: "over"` — an over-approximated candidate set can only *add* reachability, which removes symbols from this list rather than adding them. What it does report is `under`, and an `under` on a dead-code list means the opposite of the usual worry: some symbols listed here may in fact be reachable through a call the graph could not follow. Treat the list as candidates for review, not as a proof of deadness.

## Interpreting `freshness` on a dead-code gate

If you drive a CI gate off `unused`, read `freshness` before acting on the list. `stale: true` means a divergence was established between the graph and the working tree; `matches_head: null` — the ordinary value over MCP for an indexed repository — means the surface declined to decide. Neither invalidates the answer, but both mean the answer describes a tree that may not be the one under review. See [README](README.md#matches_head-is-three-valued-and-null-is-the-default-over-mcp).

## Example

Captured against the two-fixture tree described in [README — Reproducing the examples](README.md#reproducing-the-examples).

### Request

```json
{
  "name": "unused",
  "arguments": {
    "root": "/path/to/fixture",
    "kind": "function",
    "max_results": 3
  }
}
```

### Response (`structuredContent`)

```json
{
  "approximation": {
    "direction": "under",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": [
      {
        "code": "unresolved-external-calls",
        "detail": "40 call(s) in the searched region resolved to no in-repo target (external/unindexed callee; no SCIP) and could not be followed",
        "direction": "under"
      }
    ],
    "scope": {
      "confidence_floor": "possible",
      "max_depth": null,
      "searched_edge_kinds": ["calls", "calls-virtual", "calls-closure", "calls-callback", "calls-async", "calls-indirect"]
    }
  },
  "cursor": "3",
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
  "has_more": true,
  "results": [
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "kind": "function",
      "line": 49,
      "name": "rust_sample::async_calls::http_post"
    },
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "kind": "function",
      "line": 58,
      "name": "rust_sample::async_calls::sequential_awaits"
    },
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "kind": "function",
      "line": 65,
      "name": "rust_sample::async_calls::delay_then_run"
    }
  ],
  "total_matched": 199
}
```

To fetch the next page, repeat the call with `"cursor": "3"`.

The `under` reason is the one to read before acting: 40 call sites in the searched region resolved to no in-repo target, so any symbol reached *only* from one of those sites is on this list wrongly. 199 unused functions out of a two-fixture sample tree is itself the signal — this fixture has almost no entrypoints, which is the case `cgx doctor` exists to catch.

## Notes

- **No positional arguments.** `unused` takes only the `root` required parameter plus optional filters. There is no symbol to name — the query is always over the full graph.
- **Pagination.** Default page size is 20; maximum is 200. The cursor is a decimal offset string. `"cursor": "0"` is a valid first-page request and equivalent to omitting `cursor`.
- **`entrypoint` override.** When `entrypoint` is supplied, reachability is computed from that single symbol rather than all indexed entrypoints. This is useful for scoping dead-code analysis to a specific binary entry or test harness.
- **`include_dirty` default is `true` (MCP only).** The CLI has no equivalent flag; the MCP default of `true` means uncommitted edits are analyzed by default. Pass `"include_dirty": false` to index and query the committed tree (`HEAD`) instead — which also changes the `freshness` envelope, since the working tree is then never inspected (`dirty_files: null`, `matches_head: true`).
- **`kind: "all"` is MCP-only.** This value is not accepted by the CLI `--kind` flag.
- **`max_results` clamping is silent.** Values above 200 are reduced to 200 and `0` is raised to 1; neither is an error.
- **Results are ordered by `(file, line, fqn)`** (`result_key`, `crates/cgx-query/src/engine.rs:384`) — by file, then by line within a file.
- **Errors carry no envelope.** An unknown `kind`, an unresolvable `entrypoint`, or an unparseable `cursor` comes back as a JSON-RPC error object (`-32602`) with no result document, so no `freshness` and no `approximation`.

## See also

- [callers](callers.md) — find what calls a symbol (to investigate whether a candidate truly has no callers)
- [reaches](reaches.md) — the positive form of the same question, with a witness path
- [explain](explain.md) — inspect a specific symbol's caller/callee counts and incident edges
- [cgx unused (CLI)](../commands/unused.md) — CLI equivalent with additional `--kind` values, `--assert-empty`, `--confidence`, and `--format` options
- [README](README.md#the-response-envelope) — the shared response envelope in full
- [03-code-graph-model.md](../03-code-graph-model.md) — entrypoints, confidence tiers, and the graph data model
