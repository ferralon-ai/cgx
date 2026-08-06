# cgx flows-to

Values that `symbol` flows into (forward data-flow slice over `derives-from` edges).

## Synopsis

```
cgx flows-to [OPTIONS] <SYMBOL>
```

## Description

`cgx flows-to` answers the question: *where does this value propagate?* It walks the data-flow graph forward from a named value node and returns every value node that the starting value reaches, formatted as a tree rooted at the source.

**Value nodes, not function symbols.** This command operates on SSA value nodes, not function FQNs. Value node FQNs look like `rust_sample::dataflow::flow_example::b#1` — a function-scoped local with a version number. Passing a function FQN (e.g. `rust_sample::dataflow::flow_example`) returns an empty result (exit 0), not an error: functions have no outgoing `derives-from` edges. Use `cgx search <name>` to discover value-node FQNs in a function before querying.

**Dataflow index required.** The DATA_FLOW layer (SSA value nodes and `derives-from` edges) is built by default since v0.3. An index built with `cgx index --no-dataflow` contains no value nodes; `flows-to` on any value-node FQN will exit 2 ("no symbol matched").
> **If a value-node FQN suddenly stops resolving, check whether you ran [`cgx diff`](diff.md).**
> `cgx diff` currently wipes the persisted dataflow layer from `.cgx/` — silently, exit 0 — after
> which every value-node FQN exits 2 with `no symbol matched pattern …`, exactly as though it were
> mistyped. Re-run [`cgx index`](index.md) to restore it. See the warning in
> [diff.md](diff.md#description) for the full behaviour.

**Edge-condition rendering:** `always` edges are omitted from output. `conditional` edges render as `[if]`. `exception` edges render as `[exc]`. `loop` and `panic` edges render verbatim. The tag appears only when the edge is non-default.

**Confidence filtering:** By default all three confidence tiers are shown (`possible`, `probable`, `certain`). Pass `--confidence` to set a floor — for example `--confidence certain` excludes `possible` and `probable` results.

**Default depth:** The traversal applies a default depth of 2, and is always bounded — `--depth 0` bounds it to zero hops and returns the seed node alone. Widen with a larger `--depth`; the walk is work-budgeted on top of the depth limit and may show `[truncated]` on large graphs.

**Approximation and freshness.** Every slice ends with the approximation contract and the index-freshness envelope, and the contract's scope line names *data-flow* edges rather than call edges, because that is what this walk followed. See [Reading an answer](README.md#reading-an-answer).

**Since:** v0.3

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `SYMBOL` | yes | FQN of a value node (e.g. `rust_sample::dataflow::flow_example::b#1`). Function FQNs resolve but return empty. Unmatched input exits 2. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json\|sarif` | `human` | Output format. `dot`, `mermaid`, and `d2` are listed in `--help` (clap prints the whole enum on every command) but are rejected here with exit 2: they render a path walk, and `flows-to` returns a forest. |
| `--at` | git ref | — | Pin the query to a specific commit's graph snapshot (Q-17). Requires the ref to have been indexed. |
| `--depth` | integer | `2` | Maximum traversal depth, honoured as given. `0` returns the seed value node only — it is **not** unlimited, despite the `--help` text (that holds only for [`cgx paths`](paths.md)). |
| `--tree` | `full\|spanning` | `full` | Tree rendering mode. `spanning` collapses duplicate subtrees to a single occurrence. |
| `--confidence` | `possible\|probable\|certain` | — | Floor filter: exclude edges below this confidence tier. No flag = all tiers shown. |
| `--assert-empty` | boolean flag | — | CI assertion mode: exit 1 if any results are found; exit 4 if the query passed vacuously (filters excluded all candidates). See [Exit codes](#exit-codes). |
| `--allow-vacuous` | boolean flag | — | Suppress the ADR-08 vacuity guard: converts exit 4 to exit 0 when `--assert-empty` passes vacuously. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### Discover value-node FQNs with search

Before querying data flow, find the value-node FQNs in a function:

```
cgx search flow_example --repo /path/to/worktree
```

```
rust_sample::dataflow::flow_example       fixtures/rust-sample/src/dataflow.rs:5  [function]
rust_sample::dataflow::flow_example::a#0  fixtures/rust-sample/src/dataflow.rs:5  [variable]
rust_sample::dataflow::flow_example::b#1  fixtures/rust-sample/src/dataflow.rs:6  [variable]
rust_sample::dataflow::flow_example::b#2  fixtures/rust-sample/src/dataflow.rs:7  [variable]
freshness: current | indexed tree 5ea331d, working tree clean
```

The `[variable]` entries with `#N` suffixes are value nodes. The `[function]` entry has no `derives-from` edges and returns empty from `flows-to`.

### Common case: forward slice from a single value node

```
cgx flows-to "rust_sample::dataflow::flow_example::b#1" \
  --repo /path/to/worktree
```

```
rust_sample::dataflow::flow_example::b#1  fixtures/rust-sample/src/dataflow.rs:6
└─ rust_sample::dataflow::flow_example::b#2  fixtures/rust-sample/src/dataflow.rs:7  [probable]
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

`b#1` is the first assignment of `b` (a copy of `a`). It flows into `b#2` (after arithmetic), shown with `[probable]` confidence.

### Forward slice from a function parameter

```
cgx flows-to "rust_sample::dataflow::flow_example::a#0" \
  --repo /path/to/worktree
```

```
rust_sample::dataflow::flow_example::a#0  fixtures/rust-sample/src/dataflow.rs:5
└─ rust_sample::dataflow::flow_example::b#1  fixtures/rust-sample/src/dataflow.rs:6
   └─ rust_sample::dataflow::flow_example::b#2  fixtures/rust-sample/src/dataflow.rs:7  [probable]
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

Starting from parameter `a#0`, the full propagation chain is visible: `a` copies into `b#1`, which feeds `b#2`.

### Depth-limited slice

```
cgx flows-to "rust_sample::dataflow::flow_example::a#0" \
  --repo /path/to/worktree \
  --depth 1
```

```
rust_sample::dataflow::flow_example::a#0  fixtures/rust-sample/src/dataflow.rs:5
└─ rust_sample::dataflow::flow_example::b#1  fixtures/rust-sample/src/dataflow.rs:6
approximation: under-approximate — search stopped at depth 1; deeper edges were not explored
freshness: current | indexed tree 5ea331d, working tree clean
```

`--depth 1` returns only direct downstream nodes, stopping before `b#2` — and the contract drops from `exact` to `under-approximate` to say so. A depth-limited slice is never a complete answer about where a value ends up; the contract is what separates "the value stops here" from "we stopped looking here".

### JSON output for tooling

```
cgx flows-to "rust_sample::dataflow::flow_example::b#1" \
  --repo /path/to/worktree \
  --format json
```

```json
{
  "approximation": {
    "direction": "exact",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": []
  },
  "count": 1,
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "head_tree": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "indexed_tree": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "matches_head": true,
    "stale": false
  },
  "results": [
    {
      "condition": "always",
      "confidence": "probable",
      "depth": 1,
      "file": "fixtures/rust-sample/src/dataflow.rs",
      "fqn": "rust_sample::dataflow::flow_example::b#2",
      "kind": "variable",
      "line": 7
    }
  ],
  "vacuous": false
}
```

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty forest — including when a function FQN is given). Both are normal. |
| `1` | `--assert-empty` was given and results were present. |
| `2` | Bad input: no symbol matched the pattern, `--repo` was inaccessible, or `--format dot\|mermaid\|d2` was passed. Also returned when the index was built with `--no-dataflow` and the value-node FQN is absent. |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously: confidence or edge-condition filters excluded every candidate result. Suppress with `--allow-vacuous`. |

## See also

- [cgx flows-from](flows-from.md) — the backward direction: what values this node derives from
- [cgx callers](callers.md) — call-graph traversal (backward over function calls, not data flow)
- [cgx callees](callees.md) — call-graph traversal (forward over function calls, not data flow)
- [cgx explain](explain.md) — full provenance for one symbol including all incident edges
- [04-dataflow-and-provenance.md](../04-dataflow-and-provenance.md) — value pedigree, def-use chains, and derives-from edge semantics
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
