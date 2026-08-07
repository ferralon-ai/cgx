# explain (MCP tool)

Full provenance for one symbol: its definition location, caller/callee counts, and every incident edge with its condition and confidence.

## Purpose

`explain` answers the question "what do I know about this one symbol?" in a single tool call. It returns the symbol's definition file and line, its kind, the total count of direct callers and callees, and every incident edge — both incoming (callers) and outgoing (callees) — with its edge condition, confidence, resolution tier, rule, and call-site location.

Because `explain` returns all direct edges rather than performing a traversal, it has no `depth`, `max_results`, `cursor`, `edge_condition`, or `confidence` parameters. Use `callers` or `callees` for multi-hop traversal or confidence filtering; use `paths` to enumerate routes between two symbols.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `symbol` | string | yes | — | Fully-qualified name (FQN) or unambiguous short name of the symbol to explain. The FQN is tried first, then the short name. |
| `root` | string | yes | — | Path to the repository root. Must be inside a git repository; **no `.cgx/` store is required** — each call indexes in memory. See [shared parameters](README.md#shared-parameters). |
| `include_dirty` | boolean | no | `true` | When `true`, overlay uncommitted working-tree changes onto the index before answering. The overlay is never persisted. |

No other parameters are accepted. Passing unknown fields has no effect.

## Output shape

A single JSON object. The session-metadata fields (`graph_version`, `dirty`, `dirty_files_analyzed`) and the `freshness` envelope are always present.

### Top-level fields

| Field | Type | Meaning |
|-------|------|---------|
| `symbol` | string | FQN of the resolved symbol. |
| `file` | string | Source file containing the definition (relative to `root`). |
| `line` | integer | Line number of the definition. |
| `kind` | string | Symbol kind: `function`, `method`, `field`, etc. |
| `callers_count` | integer | Number of direct incoming edges. |
| `callees_count` | integer | Number of direct outgoing edges. |
| `edges` | array | All incident edges (see below). |
| `freshness` | object | Which tree the answer was computed over and how it relates to `HEAD`. See [the response envelope](README.md#freshness--which-tree-the-answer-was-computed-over). |
| `graph_version` | string | Cache key for the graph queried: a 7-character tree OID, or `<oid>+dirty.<digest>` when the working-tree overlay changed the base (ADR-06). |
| `dirty` | boolean | Whether the overlay was active **and** changed the base (ADR-06). |
| `dirty_files_analyzed` | integer | Count of paths the overlay fed the indexer differently from `HEAD` (ADR-06) — a property of the graph, not the checkout, so it need not equal `freshness.dirty_files`. |

### `explain` carries no `approximation`, by design

`explain` is one of three tools — with [`search`](search.md) and [`symbols`](symbols.md) — that skip the approximation contract deliberately (`crates/cgx-mcp/src/tools.rs:566`). It performs no traversal: there is no frontier to under-approximate and no path-level candidate set to over-approximate. What it reports is the incident-edge list as stored, and each edge already carries its own `confidence`, `tier` and `rule`, which is the same honesty at edge granularity.

Read those per-edge fields the way you would read a contract. An edge at `possible` was resolved through an over-approximated candidate set and may not occur at runtime. An edge with `tier: "scope_graph"` and `rule: "name-method"` is the one to distrust most: it is a global short-name method lookup with no receiver-type corroboration, yet it is banded `probable` (backlog **B-5**). `explain` is the right tool for spotting one, because it shows `tier` and `rule` where the traversal tools show only `confidence`.

The `freshness` envelope still applies: `explain` reads the graph, so which tree it read is exactly as material here as anywhere else. `freshness.matches_head` is three-valued and `null` is the ordinary MCP value for an indexed repository — see [README](README.md#matches_head-is-three-valued-and-null-is-the-default-over-mcp).

### Edge object fields

Each element of `edges` describes one incident call edge.

| Field | Type | Meaning |
|-------|------|---------|
| `direction` | string | `"incoming"` for a caller edge; `"outgoing"` for a callee edge. |
| `peer` | string | FQN of the other symbol on the edge. |
| `peer_file` | string | Source file of the peer symbol. |
| `peer_line` | integer | Line number of the peer symbol's definition. |
| `condition` | string | Edge condition: `always`, `conditional`, `exception`, `loop`, or `panic`. See [docs/03-code-graph-model.md](../03-code-graph-model.md). |
| `confidence` | string | Resolution confidence: `certain`, `probable`, or `possible`. See [docs/03-code-graph-model.md](../03-code-graph-model.md). |
| `tier` | string | Resolution method: `scope_graph`, `cha_rta`, `scip`, `name_syntactic`, or `points_to`. |
| `rule` | string | Matching rule applied: e.g. `scope-ref`, `sig-compat`, `name-method`. |
| `resolution_source` | string or null | Additional provenance from the resolver, or `null`. |
| `site` | object or null | Call-site location: `{"file": "...", "line": N}`, or `null` if unavailable. |

## Example

Captured against the two-fixture tree described in [README — Reproducing the examples](README.md#reproducing-the-examples).

### Request

```json
{
  "name": "explain",
  "arguments": {
    "symbol": "rust_sample::conditions::dispatch",
    "root": "/path/to/fixture"
  }
}
```

### Response (`structuredContent`)

```json
{
  "callees_count": 3,
  "callers_count": 3,
  "dirty": true,
  "dirty_files_analyzed": 3,
  "edges": [
    {
      "condition": "always",
      "confidence": "possible",
      "direction": "incoming",
      "peer": "ts_sample::closures::closureVariable",
      "peer_file": "fixtures/ts-sample/src/closures.ts",
      "peer_line": 6,
      "resolution_source": null,
      "rule": "sig-compat",
      "site": { "file": "fixtures/ts-sample/src/closures.ts", "line": 6 },
      "tier": "cha_rta"
    },
    {
      "condition": "conditional",
      "confidence": "certain",
      "direction": "outgoing",
      "peer": "rust_sample::conditions::log_info",
      "peer_file": "fixtures/rust-sample/src/conditions.rs",
      "peer_line": 4,
      "resolution_source": null,
      "rule": "scope-ref",
      "site": { "file": "fixtures/rust-sample/src/conditions.rs", "line": 42 },
      "tier": "scope_graph"
    }
  ],
  "file": "fixtures/rust-sample/src/conditions.rs",
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "head_tree": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "indexed_tree": "workdir:615bbb57f594774e2f6f9ef3783699ebc299e9ab",
    "matches_head": null,
    "stale": false
  },
  "graph_version": "03575d1+dirty.6a49a1506e74",
  "kind": "function",
  "line": 42,
  "symbol": "rust_sample::conditions::dispatch"
}
```

Elided: four further entries in `edges` — two more incoming (`ts_sample::closures::nestedClosures` at line 57 and `ts_sample::closures::nestedClosures::outer` at line 58) and two more outgoing (`log_warn` at line 8, `log_error` at line 12), each identical in shape to the one shown for its direction.

All three incoming edges are `cha_rta` / `sig-compat` matches at `possible` confidence with condition `always`: cross-language closure calls from TypeScript that could not be resolved to a definite target. All three outgoing edges are `scope_graph` / `scope-ref` at `certain` confidence with condition `conditional` — each callee is called only on some branch of `dispatch`.

This is what the missing `approximation` object would have summarized. Asked the inbound half of the same question, [`callers`](callers.md#a-positive-answer) folds exactly these three candidate-set edges into an `over-approx-candidate-set` reason and reports `direction: "over_under"` (its frontier also carries unresolved calls and a depth limit). `explain` shows you the edges themselves instead.

`dirty_files_analyzed: 3` beside `freshness.dirty_files: 0` is the expected pairing for an indexed repository: the overlay saw the three files `cgx index` wrote into `.cgx/`, and git's ignore rules exclude all three from the dirty count. That disagreement is also why `matches_head` is `null`.

## Notes

- **No pagination.** `explain` returns all incident edges in one response. There is no `cursor`, `has_more`, or `max_results` field. For large symbols with many edges, use `callers` or `callees` (which do paginate) instead.
- **No `at` parameter.** The `--at` git-ref flag exists on the CLI but is not registered in the MCP tool's `inputSchema`. Unknown arguments are ignored without error, so passing `at` is a silent no-op rather than a pin.
- **Short-name resolution.** If `symbol` is a short name that matches exactly one symbol, it resolves successfully. Ambiguous or unmatched names return a tool error.
- **`include_dirty` defaults to `true`.** MCP differs from the CLI here — the tool analyzes uncommitted changes by default. Pass `false` to index and query the committed tree (`HEAD`) instead; that also changes the `freshness` envelope, since the working tree is then never inspected (`dirty_files: null`, `matches_head: true`).
- **Errors carry no envelope.** An unresolvable symbol or a missing `root` comes back as a JSON-RPC error object (`-32602`) with no result document — so no `freshness` and no `graph_version` either.
- **Edge condition rendering.** The MCP tool returns raw condition strings (`"always"`, `"conditional"`, etc.). The CLI renders these with abbreviations (`[if]`, `[exc]`) and omits `always` from human output; the MCP tool does not apply those transformations.

## See also

- [callers](callers.md) — transitive caller traversal with depth, confidence, and pagination
- [callees](callees.md) — transitive callee traversal with depth, confidence, and pagination
- [paths](paths.md) — enumerate call paths between two symbols
- [reaches](reaches.md) — whether one symbol reaches another, with a witness path
- [search](search.md) — the other contract-free tool: resolve a partial name to definitions
- [symbols](symbols.md) — rank symbols by reference count with per-symbol edge breakdowns
- [unused](unused.md) — symbols not reachable from any entrypoint
- [cgx explain (CLI)](../commands/explain.md) — CLI equivalent with `--format` and exit codes
- [README](README.md#the-response-envelope) — the shared response envelope in full
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge condition and confidence definitions
