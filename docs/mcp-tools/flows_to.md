# flows_to (MCP tool)

Forward data-flow slice: symbols a value flows into (its DerivesFrom consumers).

## Purpose

`flows_to` answers *where does this value end up?* Given a value node, it returns
every value that derives from it, transitively, within `depth`.

Use it for taint-style questions in the forward direction: a request parameter, a
decoded payload, a secret read from the environment — `flows_to` names the values
downstream of it. For the opposite question (*where did this value come from?*)
use [flows_from](flows_from.md).

Two properties decide whether a call to this tool returns anything at all:

- **The anchor is a value node, not a function.** Data-flow edges connect
  `DerivesFrom` value nodes — the `#N`-suffixed nodes such as
  `svc::parse::cache_key::cleaned#1`. A function FQN has no `DerivesFrom` edges
  and returns an empty result. Find value nodes with [search](search.md).
- **The walk sees `DerivesFrom` edges only** (`crates/cgx-mcp/src/tools.rs:905`).
  Call edges are not traversed, so a value that reaches a callee through an
  argument cgx did not model is not in the answer.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `symbol` | string | yes | — | The value node to slice from (FQN or unambiguous short name). |
| `root` | string | yes | — | Repository root path. A `.cgx/` store is not required; the server indexes per call into an in-memory store. |
| `depth` | integer | no | `2` | Maximum traversal depth (`DEFAULT_FOREST_DEPTH`). Applied in both directions of this tool pair. |
| `confidence` | string | no | — | Minimum confidence floor. One of `certain`, `probable`, `possible`. Omitted = every edge. |
| `max_results` | integer | no | `20` | Page size. Clamped to `[1, 200]` (`MAX_MAX_RESULTS`, `crates/cgx-mcp/src/tools.rs:28`); `0` clamps up to 1. |
| `cursor` | string | no | — | Opaque pagination cursor: a decimal offset string. Unparseable values are an `invalid_params` error. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted. |

There is no `kind` or `edge_condition` filter: the edge scope is fixed to
`DerivesFrom`.

## Output shape

| Field | Type | Meaning |
|-------|------|---------|
| `symbol` | string | Echo of the `symbol` input, as received. |
| `results` | array | Current page of consumer records. |
| `total_matched` | integer | Total consumers found before pagination. |
| `has_more` | boolean | `true` when more pages remain. |
| `cursor` | string or null | Cursor for the next page; `null` on the last page. |
| `approximation` | object | Which direction this answer can be wrong, and why. `scope` is present **only** when `results` is empty. |
| `graph_version` | string | Content key of the graph queried (ADR-06): a short tree OID, or `<short-tree-oid>+dirty.<digest>` when the working-tree overlay changed the base. |
| `dirty` | boolean | Whether the overlay was active *and* changed the base. |
| `dirty_files_analyzed` | integer | How many paths the overlay fed the indexer differently from `HEAD`. |
| `freshness` | object | Index-freshness envelope: `indexed_tree`, `head_tree`, `matches_head`, `dirty_files`, `dirty_files_base`, `stale`. |

Each record in `results` (`neighbor_json`, `crates/cgx-mcp/src/tools.rs:499`) is
the same shape `callers`/`callees` return:

| Field | Type | Meaning |
|-------|------|---------|
| `name` | string | FQN of the consuming value node. |
| `file` | string | Source file path, relative to `root`. |
| `line` | integer | Line of the consuming binding. |
| `depth` | integer | Data-flow hops from `symbol` to this value. |
| `edge_condition` | string | Condition on the incident edge: `always`, `conditional`, `exception`, `loop`, `panic`. |
| `confidence` | string | Confidence of that edge: `certain`, `probable`, `possible`. |
| `min_confidence_on_path` | string | Weakest confidence across the whole path from `symbol` to this value. |
| `exception_transient` | boolean | `true` when the value is reached only across an exception edge. |

## Example

The shared two-fixture tree used by the other tool docs
([Reproducing the examples](README.md#reproducing-the-examples)) has only
single-hop data-flow chains, which cannot show what `depth` does here. These
examples therefore use a purpose-built fixture, `svc`, whose `parse::cache_key`
is a four-node chain:

```rust
pub fn cache_key(raw: &str) -> String {
    let cleaned = normalize(raw);
    let normalized = cleaned;
    let key = normalized;
    key
}
```

Build it in a `mktemp -d`, `git init`, commit, and `cgx index .`. The `root` is
written below as `/path/to/svc`; every response is the captured answer verbatim.

### Request

```json
{
  "name": "flows_to",
  "arguments": {
    "symbol": "svc::parse::cache_key::cleaned#1",
    "root": "/path/to/svc"
  }
}
```

### Response

```json
{
  "approximation": {
    "direction": "under",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": [
      {
        "code": "depth-limit",
        "detail": "search stopped at depth 2; deeper edges were not explored",
        "direction": "under"
      }
    ]
  },
  "cursor": null,
  "dirty": true,
  "dirty_files_analyzed": 3,
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "ab9a1e49b93824d18f27ae20c87f8cf6687c3456",
    "head_tree": "ab9a1e49b93824d18f27ae20c87f8cf6687c3456",
    "indexed_tree": "workdir:d22e79fc2f7f8ab9b48db7b013a5df3d2ce5f411",
    "matches_head": null,
    "stale": false
  },
  "graph_version": "ab9a1e4+dirty.8286a40c7b12",
  "has_more": false,
  "results": [
    {
      "confidence": "certain",
      "depth": 1,
      "edge_condition": "always",
      "exception_transient": false,
      "file": "src/parse.rs",
      "line": 28,
      "min_confidence_on_path": "certain",
      "name": "svc::parse::cache_key::normalized#1"
    },
    {
      "confidence": "certain",
      "depth": 2,
      "edge_condition": "always",
      "exception_transient": false,
      "file": "src/parse.rs",
      "line": 29,
      "min_confidence_on_path": "certain",
      "name": "svc::parse::cache_key::key#1"
    }
  ],
  "symbol": "svc::parse::cache_key::cleaned#1",
  "total_matched": 2
}
```

The value reached `normalized#1` at depth 1 and `key#1` at depth 2. Note the
`depth-limit` reason: the default bound of 2 stopped the walk, so
`svc::parse::cache_key::return#1` — one hop further along the same chain — is
**not** in this answer. The slice is not "everything downstream"; it is
"everything downstream within `depth`", and the contract says so.

## Notes

### The `depth` default silently truncates a longer chain

`depth` defaults to 2 (`DEFAULT_FOREST_DEPTH`,
`crates/cgx-mcp/src/tools.rs:37`), which is a deliberately small neighborhood.
The example above is a four-node chain, and the default answer contains two of
them. The same call with `"depth": 3` returns three results — `normalized#1`,
`key#1`, `return#1` — and its `approximation.direction` drops from `under` to
`exact` with an empty `reasons` array, because nothing was cut.

Raise `depth` when the question is "does this value reach a sink anywhere", and
read `approximation.reasons` for `depth-limit` before concluding that it does
not.

### An empty answer can report `direction: "exact"`

Calling this tool with a function FQN rather than a value node returns:

```json
{
  "approximation": {
    "direction": "exact",
    "reasons": [],
    "scope": {
      "confidence_floor": "possible",
      "max_depth": 2,
      "searched_edge_kinds": [
        "derives-from"
      ]
    }
  },
  "results": [],
  "symbol": "cache_key",
  "total_matched": 0
}
```

(Envelope fields elided.) `exact` here means *within the modeled graph, nothing
was found* — not *this symbol has no data flow*. The claim's whole content is in
`scope`: the search covered `derives-from` edges only, to depth 2, at every
confidence tier. A function node has no such edges, so the honest answer is
empty. **Read `scope` before treating an empty data-flow answer as a clean bill
of health.**

### Direction, in one line

`flows_to` walks the `DerivesFrom` edge **backward** — via the `callers` walk
(`crates/cgx-mcp/src/tools.rs:916`) — because a `DerivesFrom` edge points from
the derived value to its source. Walking it backward from a value therefore
yields the values derived *from* it: its consumers. [flows_from](flows_from.md)
walks the same edges forward.

If an answer looks like it went the wrong way, check which of the two tools you
called; the edge orientation makes the naming counter-intuitive at the code level
and correct at the question level.

### `matches_head` is three-valued

`freshness.matches_head` is `true`, `false`, or `null` — never treat it as a
boolean. The example above shows `null` on a `git status`-clean fixture, which is
the ordinary case: once `cgx index` has written `.cgx/`, the overlay enumeration
(no ignore rules) sees those files while git's dirty count (ignore rules applied)
does not, and the two views disagree about whether the graph is `HEAD`'s tree
(`crates/cgx-mcp/src/session.rs:196`). `null` is the third honest answer, and the
envelope's verdict word for it is `unknown`.

### Errors carry no envelope

A tool error is a JSON-RPC error object with no `structuredContent`: neither
`approximation` nor `freshness`. An anchor that resolves to nothing returns
``{"code": -32602, "message": "no symbol matched pattern `nosuch#1`"}`` rather
than an empty result — an empty `results` array always means the anchor resolved
and the walk found nothing.

### Data flow must be in the index

The MCP server indexes with dataflow on by default (v0.3 SC6,
`dataflow_default`, `crates/cgx-mcp/src/session.rs:253`); a repository whose `cgx.toml` sets
`[index] data_flow = false` produces no `DerivesFrom` edges at all, and this tool
returns empty for every anchor.

## See also

- [flows_from](flows_from.md) — the pedigree direction: what a value derives from
- [search](search.md) — find the `#N` value-node FQNs this tool takes as anchors
- [graph_query](graph_query.md) — `MATCH (a)-[:DATA_FLOW]->(b)` when the slice needs a predicate this tool cannot express
- [paths](paths.md) — the call-graph analogue: enumerate routes between two symbols
- [cgx flows-to (CLI)](../commands/flows-to.md) — CLI equivalent with output formats and exit codes
- [04-dataflow-and-provenance.md](../04-dataflow-and-provenance.md) — the data-flow model, value nodes, and what `DerivesFrom` does and does not capture
