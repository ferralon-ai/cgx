# cgx flows-from

Values that `symbol` derives from (backward data-flow pedigree over `derives-from` edges).

## Synopsis

```
cgx flows-from [OPTIONS] <SYMBOL>
```

## Description

`cgx flows-from` answers the question: *where did this value come from?* It walks the data-flow graph backward from the named value node and returns every value node that contributes to it, within the traversal depth, formatted as a tree rooted at the target.

**Since:** v0.3. Dataflow is built by default; no extra flag is required. To opt out of dataflow at index time, pass `--no-dataflow` to `cgx index`.

**Value nodes, not function symbols.** `flows-from` operates on SSA value nodes, not function FQNs. A function FQN has no `derives-from` edges and returns an empty result (exit 0). Value-node FQNs look like `rust_sample::dataflow::flow_example::b#2` — a scoped name followed by `#N` indicating the SSA version. Use `cgx search <name>` to discover the exact FQNs for a variable.

Each result line shows the value node's FQN, its source location, and (when non-default) its [edge condition](../03-code-graph-model.md) and [confidence](../03-code-graph-model.md) tier. The default traversal depth is 2, and the walk is always bounded — `--depth 0` bounds it to zero hops and returns the seed node alone. Widen with a larger `--depth`; the walk is work-budgeted on top of that and may show `[truncated]`.

Every pedigree ends with the approximation contract and the index-freshness envelope. The contract's scope line names *data-flow* edges, because that is what this walk followed — see [Reading an answer](README.md#reading-an-answer).

**Edge-condition rendering:** `always` edges are omitted from output. `conditional` edges render as `[if]`. `exception` edges render as `[exc]`. `loop` and `panic` edges render verbatim. The tag appears only when the edge is non-default.

**No-dataflow index.** On an index built with `cgx index --no-dataflow`, value nodes are absent. Querying a value-node FQN against such an index exits 2 ("no symbol matched").
> **If a value-node FQN suddenly stops resolving, check whether you ran [`cgx diff`](diff.md).**
> `cgx diff` currently wipes the persisted dataflow layer from `.cgx/` — silently, exit 0 — after
> which every value-node FQN exits 2 with `no symbol matched pattern …`, exactly as though it were
> mistyped. Re-run [`cgx index`](index.md) to restore it. See the warning in
> [diff.md](diff.md#description) for the full behaviour.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `SYMBOL` | yes | FQN of the value node whose data-flow ancestry to enumerate (e.g. `rust_sample::dataflow::flow_example::b#2`). Function FQNs are accepted but return an empty result. Unmatched patterns exit 2. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json\|sarif` | `human` | Output format. `dot`, `mermaid`, and `d2` are listed in `--help` (clap prints the whole enum on every command) but are rejected here with exit 2: they render a path walk, and `flows-from` returns a forest. |
| `--at` | git ref | — | Pin the query to a git ref's graph (Q-17). Materializes that ref in a detached worktree, indexes it, and queries the resulting graph. |
| `--depth` | integer | `2` | Maximum traversal depth, honoured as given. `0` returns the seed value node only — it is **not** unlimited, despite the `--help` text (that holds only for [`cgx paths`](paths.md)). |
| `--tree` | `full\|spanning` | `full` | Tree rendering mode. `spanning` collapses duplicate subtrees to a single occurrence. |
| `--confidence` | `possible\|probable\|certain` | — | Floor filter: exclude edges below this confidence tier. No flag = all tiers shown. |
| `--assert-empty` | boolean flag | — | CI assertion mode: exit 1 if any results are found; exit 4 if the query passed vacuously (filters excluded all candidates). See [Exit codes](#exit-codes). |
| `--allow-vacuous` | boolean flag | — | Suppress the ADR-08 vacuity guard: converts exit 4 to exit 0 when `--assert-empty` passes vacuously. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### Common case: backward data-flow pedigree

Trace where `b#2` (the value `b = b + 1`) came from.

```
cgx flows-from "rust_sample::dataflow::flow_example::b#2" \
  --repo /path/to/worktree
```

```
rust_sample::dataflow::flow_example::b#2  fixtures/rust-sample/src/dataflow.rs:7
└─ rust_sample::dataflow::flow_example::b#1  fixtures/rust-sample/src/dataflow.rs:6  [probable]
   └─ rust_sample::dataflow::flow_example::a#0  fixtures/rust-sample/src/dataflow.rs:5
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

The full ancestry: `b#2` derives from `b#1` (the intermediate assignment), which derives from parameter `a#0`. Exit 0.

### Branched phi-node: multiple contributing values

A conditional expression (`if c { a } else { b }`) creates a phi node. `flows-from` on its result shows all branches that contribute.

```
cgx flows-from "rust_sample::dataflow::select::return#1" \
  --repo /path/to/worktree
```

```
rust_sample::dataflow::select::return#1  fixtures/rust-sample/src/dataflow.rs:26
└─ rust_sample::dataflow::select::v#1  fixtures/rust-sample/src/dataflow.rs:25
   ├─ rust_sample::dataflow::select::a#0  fixtures/rust-sample/src/dataflow.rs:24  [probable]
   ├─ rust_sample::dataflow::select::b#0  fixtures/rust-sample/src/dataflow.rs:24  [probable]
   └─ rust_sample::dataflow::select::c#0  fixtures/rust-sample/src/dataflow.rs:24  [probable]
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

Three parameters (`a`, `b`, `c`) all reach the return value through the phi merge. Exit 0.

### Machine-readable output

```
cgx flows-from "rust_sample::dataflow::flow_example::b#2" \
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
  "count": 2,
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
      "confidence": "certain",
      "depth": 2,
      "file": "fixtures/rust-sample/src/dataflow.rs",
      "fqn": "rust_sample::dataflow::flow_example::a#0",
      "kind": "variable",
      "line": 5
    },
    {
      "condition": "always",
      "confidence": "probable",
      "depth": 1,
      "file": "fixtures/rust-sample/src/dataflow.rs",
      "fqn": "rust_sample::dataflow::flow_example::b#1",
      "kind": "variable",
      "line": 6
    }
  ],
  "vacuous": false
}
```

The `depth` field in each result record is the number of `derives-from` hops from the query node. Exit 0.

### Function FQN returns empty (not an error)

```
cgx flows-from "rust_sample::dataflow::through_call" \
  --repo /path/to/worktree
```

```
rust_sample::dataflow::through_call  fixtures/rust-sample/src/dataflow.rs:31
approximation: exact (within modeled graph) | scope: data-flow edges, confidence>=possible, depth<=2
freshness: current | indexed tree 5ea331d, working tree clean
```

A function FQN resolves but has no `derives-from` edges, so the tree has no children. This is a normal empty result (exit 0), not an error. Use `cgx search` to find the value-node FQNs inside the function instead.

The `| scope:` tail is the tell that this is an absence answer, and it names why the absence is uninformative here: the search followed data-flow edges, and a function node has none. Reading "no results" as "this function's inputs come from nowhere" would be the mistake; the scope line rules it out.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty forest). Both are normal. |
| `1` | `--assert-empty` was given and results were present. |
| `2` | Bad input: no symbol matched the pattern, `--repo` was inaccessible, `--format dot\|mermaid\|d2` was passed, or the index was built with `--no-dataflow` (value nodes absent). |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously: confidence or edge-condition filters excluded every candidate result. Suppress with `--allow-vacuous`. |

## See also

- [cgx flows-to](flows-to.md) — the forward direction: what values this node flows into
- [cgx search](search.md) — discover value-node FQNs by name substring
- [cgx explain](explain.md) — full provenance for one symbol including all incident edges
- [cgx callers](callers.md) — backward traversal over call edges (not data-flow edges)
- [04-dataflow-and-provenance.md](../04-dataflow-and-provenance.md) — value pedigree, SSA nodes, and `derives-from` edge semantics
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
