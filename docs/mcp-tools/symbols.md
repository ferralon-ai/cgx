# symbols (MCP tool)

Rank symbols by reference count (the hub/importance lens), each with its inbound/outbound edge breakdown.

## Purpose

`symbols` answers *what matters in this codebase?* — it ranks every indexed symbol
by how many edges touch it and returns, for each, a decomposition of those edges
by family, condition and confidence.

Use it to find hubs before reading a codebase, to spot the symbol a refactor will
be most expensive to change, or to rank candidates before spending a
[callers](callers.md) call on each. It is a whole-graph aggregation, not a search:
there is no name predicate — use [search](search.md) for that.

`rank_symbols` (`crates/cgx-query/src/symbols.rs:138`) visits every node's
incident edges exactly once, ranks descending by the chosen key, and breaks ties
on `(fqn, file, line)` so the order is byte-identical across runs.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `root` | string | yes | — | Repository root path. A `.cgx/` store is not required; the server indexes per call into an in-memory store. |
| `rank` | string | no | `"total"` | Ranking key. `total` = `in_degree + out_degree`; `inbound` = how depended-upon; `outbound` = how many things it depends on. |
| `kind` | string | no | `"all"` | Narrow to one symbol kind. Enum: `function`, `method`, `field`, `type`, `module`, `all`. Narrower than the kinds that appear in results — see the notes. |
| `max_results` | integer | no | `20` | Page size. Clamped to `[1, 200]` (`MAX_MAX_RESULTS`, `crates/cgx-mcp/src/tools.rs:28`); `0` clamps up to 1. |
| `cursor` | string | no | — | Opaque pagination cursor: a decimal offset string. Unparseable values are an `invalid_params` error. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted. |

An unknown `rank` is an `invalid_params` error — `unknown rank \`degree\`` —
not a fallback to the default (`crates/cgx-mcp/src/tools.rs:864-868`).

## Output shape

| Field | Type | Meaning |
|-------|------|---------|
| `results` | array | Current page of ranked symbols, highest first. |
| `total_matched` | integer | Total symbols ranked before pagination — every symbol passing `kind`, not just the interesting ones. |
| `has_more` | boolean | `true` when more pages remain. |
| `cursor` | string or null | Cursor for the next page; `null` on the last page. |
| `graph_version` | string | Content key of the graph queried (ADR-06): a short tree OID, or `<short-tree-oid>+dirty.<digest>` when the working-tree overlay changed the base. |
| `dirty` | boolean | Whether the overlay was active *and* changed the base. |
| `dirty_files_analyzed` | integer | How many paths the overlay fed the indexer differently from `HEAD`. |
| `freshness` | object | Index-freshness envelope: `indexed_tree`, `head_tree`, `matches_head`, `dirty_files`, `dirty_files_base`, `stale`. |

There is **no `approximation` key** on this tool, in any response — see
[Why there is no `approximation` field](#why-there-is-no-approximation-field).

Each record in `results` (`symbol_rank_json`, `crates/cgx-mcp/src/tools.rs:946`):

| Field | Type | Meaning |
|-------|------|---------|
| `fqn` | string | Fully-qualified name. |
| `file` | string | Source file path, relative to `root`. |
| `line` | integer | Line of the definition. |
| `kind` | string | Symbol kind, lowercased. |
| `in_degree` | integer | Inbound edge count — callers, plus `DerivesFrom` consumers. |
| `out_degree` | integer | Outbound edge count — callees, plus `DerivesFrom` sources. |
| `inbound` | object | Breakdown of the inbound edges. |
| `outbound` | object | Breakdown of the outbound edges. |

Each breakdown (`breakdown_json`, `crates/cgx-mcp/src/tools.rs:959`):

| Field | Type | Meaning |
|-------|------|---------|
| `total` | integer | Edge count; equals `in_degree` / `out_degree` respectively. |
| `by_family` | object | Counts keyed by edge family: `calls`, `derives_from`. |
| `by_condition` | object | Counts keyed by edge condition: `always`, `conditional`, `exception`, `loop`, `panic`. |
| `by_confidence` | object | Counts keyed by confidence tier: `certain`, `probable`, `possible`. |

Only two families are counted (`EdgeFamily::classify`,
`crates/cgx-query/src/symbols.rs:49`). Structural edges — `contains`, `imports`,
`overrides` — are deliberately excluded from the reference-count lens, so a
symbol's degree here is not its total edge count in the graph.

## Example

Captured against the shared two-fixture tree described in
[Reproducing the examples](README.md#reproducing-the-examples). The `root` is
written here as `/path/to/fixtures`; the response is the captured answer
verbatim.

### Request

```json
{
  "name": "symbols",
  "arguments": {
    "rank": "inbound",
    "kind": "function",
    "max_results": 2,
    "root": "/path/to/fixtures"
  }
}
```

### Response

```json
{
  "cursor": "2",
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
      "file": "fixtures/ts-sample/src/imports.ts",
      "fqn": "ts_sample::imports::speak",
      "in_degree": 12,
      "inbound": {
        "by_condition": {
          "always": 10,
          "conditional": 1,
          "loop": 1
        },
        "by_confidence": {
          "possible": 12
        },
        "by_family": {
          "calls": 12
        },
        "total": 12
      },
      "kind": "function",
      "line": 50,
      "out_degree": 7,
      "outbound": {
        "by_condition": {
          "always": 7
        },
        "by_confidence": {
          "possible": 7
        },
        "by_family": {
          "calls": 7
        },
        "total": 7
      }
    },
    {
      "file": "fixtures/ts-sample/src/spawn.ts",
      "fqn": "ts_sample::spawn::backgroundWork",
      "in_degree": 9,
      "inbound": {
        "by_condition": {
          "always": 5,
          "conditional": 2,
          "loop": 2
        },
        "by_confidence": {
          "certain": 6,
          "possible": 3
        },
        "by_family": {
          "calls": 9
        },
        "total": 9
      },
      "kind": "function",
      "line": 7,
      "out_degree": 1,
      "outbound": {
        "by_condition": {
          "always": 1
        },
        "by_confidence": {
          "certain": 1
        },
        "by_family": {
          "calls": 1
        },
        "total": 1
      }
    }
  ],
  "total_matched": 232
}
```

Read `by_confidence` before trusting either rank. The top symbol's 12 inbound
edges are **all `possible`** — the weakest tier — while the runner-up's 9 are 6
`certain` and 3 `possible`. The ranking counts edges and **does not weight them
by confidence**, so a symbol can top the list on edges that name-based
resolution proposed and nothing corroborated. `by_condition` qualifies it
further: 2 of the runner-up's inbound edges are `conditional` and 2 are `loop`,
so they do not all occur on every execution.

## Notes

### Why there is no `approximation` field

`symbols`, [search](search.md) and [explain](explain.md) are the three tools that
do not pass through `with_contract` (`crates/cgx-mcp/src/tools.rs:549-577`). The
approximation contract describes how a **traversal** can be wrong — an
over-approximated candidate set on a walked edge, or a frontier the walk could not
follow. `symbols` performs no traversal: it tallies edges that are already in the
graph, one pass, no walk.

The honesty signal it carries instead is per-record and finer-grained:
`by_confidence` on both breakdowns tells you exactly how many of a symbol's edges
are `certain`, `probable` or `possible`. Use it the way you would use
`approximation.direction` elsewhere — do not read a missing `approximation` as a
claim of exactness.

### The default ranks every node, including value nodes

`kind: "all"` means all: on the fixture above, `total_matched` is 232 with
`kind: "function"` and **948** without it. The other 716 records are 78 methods,
35 types, 18 modules, 5 fields — and 580 nodes whose kinds the filter does not
accept at all, `variable` (the `#N`-suffixed value nodes carrying the data-flow
graph) and `lambda` among them. They crowd out functions in a `total`-ranked
page.

Pass an explicit `kind` whenever the question is about callable symbols. The
filter accepts six tokens (`parse_symbol_kind`,
`crates/cgx-mcp/src/tools.rs:472`); `variable` and `lambda` are **not** among
them, so those nodes can be excluded but not selected for. Use [graph_query](graph_query.md) if
you need to rank them specifically.

### `inbound` means depended-upon in both families

For a call edge `caller → callee`, the callee gains the inbound. For a
`DerivesFrom` edge `derived → source`, cgx flips the orientation so the **source**
value gains the inbound (`crates/cgx-query/src/symbols.rs:163`). Both families
therefore agree on one meaning: `in_degree` counts *things that depend on this
symbol*, whether by calling it or by deriving a value from it.

A value node consequently reports its data-flow edges under `derives_from`. A
function parameter that one value is derived from carries an inbound edge and no
outbound one:

```json
{
  "fqn": "rust_sample::cfg_feature::authenticate::token#0",
  "file": "fixtures/rust-sample/src/cfg_feature.rs",
  "line": 26,
  "kind": "variable",
  "in_degree": 1,
  "inbound": { "total": 1, "by_family": { "derives_from": 1 }, "by_condition": { "always": 1 }, "by_confidence": { "probable": 1 } },
  "out_degree": 0,
  "outbound": { "total": 0, "by_family": {}, "by_condition": {}, "by_confidence": {} }
}
```

(The nested objects are shown inline here; the emitted JSON expands them. An
empty breakdown emits empty objects, not `null`.)

### `total_matched` is not a count of interesting symbols

Every symbol passing the `kind` filter is ranked, including those with degree 0.
`total_matched` is that population, so it is a poor proxy for "how many hubs are
there" — page through and read `in_degree` instead.

### `matches_head` is three-valued

`freshness.matches_head` is `true`, `false`, or `null` — never treat it as a
boolean. The example above shows `null` on a `git status`-clean fixture, which is
the ordinary case: once `cgx index` has written `.cgx/`, the overlay enumeration
(no ignore rules) sees those files while git's dirty count (ignore rules applied)
does not, and the two views disagree about whether the graph is `HEAD`'s tree
(`crates/cgx-mcp/src/session.rs:196`). `null` is the third honest answer, and the
envelope's verdict word for it is `unknown`.

`dirty_files: 0` (inspected, nothing diverged) and `dirty_files: null` (never
inspected — what `include_dirty: false` produces) are deliberately different
values.

### Errors carry no envelope

A tool error is a JSON-RPC error object with no `structuredContent`: no
`freshness`, and no `approximation` either. An unknown `rank`, an unknown `kind`
and an unparseable `cursor` each return one.

### Relationship to the CLI

`cgx symbols --format json` emits `{"freshness": …, "results": […]}` — an object,
with the same per-record shape as this tool. The v0.3 change that wrapped the
CLI's bare JSON array in that object did **not** touch this surface: the MCP
response has been an object with `results`/`total_matched`/`has_more`/`cursor`
since before the change (verified against `8af8bcf:crates/cgx-mcp/src/tools.rs`).

## See also

- [search](search.md) — resolve a name fragment to exact symbols; the tool to use when you know what you are looking for
- [explain](explain.md) — the same edge decomposition for one symbol, with every incident edge listed individually
- [callers](callers.md) / [callees](callees.md) — follow the edges a high degree points at
- [unused](unused.md) — the opposite lens: symbols no entrypoint reaches
- [cgx symbols (CLI)](../commands/symbols.md) — CLI equivalent with output formats and exit codes
- [03-code-graph-model.md](../03-code-graph-model.md) — edge families, conditions, and confidence tiers
