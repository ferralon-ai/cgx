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

Each result line shows the value node's FQN, its source location, and (when non-default) its [edge condition](../03-code-graph-model.md) and [confidence](../03-code-graph-model.md) tier. The default traversal depth is 2. Pass `--depth 0` for unlimited traversal (work-budgeted; may show `[truncated]`).

**Edge-condition rendering:** `always` edges are omitted from output. `conditional` edges render as `[if]`. `exception` edges render as `[exc]`. `loop` and `panic` edges render verbatim. The tag appears only when the edge is non-default.

**No-dataflow index.** On an index built with `cgx index --no-dataflow`, value nodes are absent. Querying a value-node FQN against such an index exits 2 ("no symbol matched").

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `SYMBOL` | yes | FQN of the value node whose data-flow ancestry to enumerate (e.g. `rust_sample::dataflow::flow_example::b#2`). Function FQNs are accepted but return an empty result. Unmatched patterns exit 2. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. `dot`, `mermaid`, and `d2` are listed in help but are path-shaped renderers; forest results from `flows-from` may render empty or malformed in those formats. |
| `--at` | git ref | — | Pin the query to a git ref's graph (Q-17). Materializes that ref in a detached worktree, indexes it, and queries the resulting graph. |
| `--depth` | integer | `2` | Maximum traversal depth. `0` = unlimited (work-budgeted, may show `[truncated]`). |
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
  --repo /path/to/rust-sample
```

```
rust_sample::dataflow::flow_example::b#2  src/dataflow.rs:7
└─ rust_sample::dataflow::flow_example::b#1  src/dataflow.rs:6  [probable]
   └─ rust_sample::dataflow::flow_example::a#0  src/dataflow.rs:5
```

The full ancestry: `b#2` derives from `b#1` (the intermediate assignment), which derives from parameter `a#0`. Exit 0.

### Branched phi-node: multiple contributing values

A conditional expression (`if c { a } else { b }`) creates a phi node. `flows-from` on its result shows all branches that contribute.

```
cgx flows-from "rust_sample::dataflow::select::return#1" \
  --repo /path/to/rust-sample
```

```
rust_sample::dataflow::select::return#1  src/dataflow.rs:26
└─ rust_sample::dataflow::select::v#1  src/dataflow.rs:25
   ├─ rust_sample::dataflow::select::a#0  src/dataflow.rs:24  [probable]
   ├─ rust_sample::dataflow::select::b#0  src/dataflow.rs:24  [probable]
   └─ rust_sample::dataflow::select::c#0  src/dataflow.rs:24  [probable]
```

Three parameters (`a`, `b`, `c`) all reach the return value through the phi merge. Exit 0.

### Machine-readable output

```
cgx flows-from "rust_sample::dataflow::flow_example::b#2" \
  --repo /path/to/rust-sample \
  --format json
```

```json
{
  "count": 2,
  "results": [
    {
      "condition": "always",
      "confidence": "certain",
      "depth": 2,
      "file": "src/dataflow.rs",
      "fqn": "rust_sample::dataflow::flow_example::a#0",
      "kind": "variable",
      "line": 5
    },
    {
      "condition": "always",
      "confidence": "probable",
      "depth": 1,
      "file": "src/dataflow.rs",
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
  --repo /path/to/rust-sample
```

```
rust_sample::dataflow::through_call  src/dataflow.rs:31
```

A function FQN resolves but has no `derives-from` edges, so the tree has no children. This is a normal empty result (exit 0), not an error. Use `cgx search` to find the value-node FQNs inside the function instead.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty forest). Both are normal. |
| `1` | `--assert-empty` was given and results were present. |
| `2` | Bad input: no symbol matched the pattern, or the index was built with `--no-dataflow` (value nodes absent). |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously: confidence or edge-condition filters excluded every candidate result. Suppress with `--allow-vacuous`. |

## See also

- [cgx flows-to](flows-to.md) — the forward direction: what values this node flows into _(doc forthcoming)_
- [cgx search](search.md) — discover value-node FQNs by name substring _(doc forthcoming)_
- [cgx explain](explain.md) — full provenance for one symbol including all incident edges
- [cgx callers](callers.md) — backward traversal over call edges (not data-flow edges)
- [04-dataflow-and-provenance.md](../04-dataflow-and-provenance.md) — value pedigree, SSA nodes, and `derives-from` edge semantics
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
