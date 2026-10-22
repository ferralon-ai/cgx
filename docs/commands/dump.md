# cgx dump

Print the raw on-disk graph — nodes, edges, and candidate sets — for inspection.

## Synopsis

```
cgx dump [OPTIONS] [SYMBOL]
```

## Description

`cgx dump` decodes the graph the current `.cgx/` store points at and prints it, either whole or restricted to the nodes matching a symbol filter and the edges incident to them. It is a debugging view of the stored data, not a query: there is no traversal, no confidence floor, and no depth.

**Read-only, and it never indexes.** `dump` reads the `HEAD.json` pointer and the objects it names. It does not auto-index and has no `--no-auto-index` flag; with no index it exits 3. Run [`cgx index`](index.md) first. Because it shows exactly what is stored, a stale index is dumped as-is.

**No approximation contract, no freshness envelope.** The output is the stored graph itself, so there is no answer to qualify.

Node ids in the output are ids within this one graph. They are stable for a given index but are not a public identifier; re-indexing can renumber them. Use FQNs to refer to symbols anywhere else.

## Arguments

| Argument | Required | Meaning |
|----------|----------|---------|
| `SYMBOL` | no | Symbol filter: an FQN, a `::`-suffix of one, or a substring. Only matching nodes and the edges incident to them are printed. Omitted, the whole graph is printed. A filter that matches nothing prints zero counts and exits 0. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `text\|json` | `text` | `text` is a tab-separated listing; `json` is one document with `graph_key`, `counts`, `nodes`, `edges`, and `candidates`. |

## Examples

### One symbol, text

```
cgx dump rust_sample::direct::chain
```

```
graph_key: c19123dd915dcb04a2735de3426b640ac2f21738
nodes: 5  edges: 7  candidates: 0

# nodes (id  kind  fqn  file:line)
217	Function	rust_sample::direct::chain	src/direct.rs:47
218	Variable	rust_sample::direct::chain::n#0	src/direct.rs:47
219	Variable	rust_sample::direct::chain::a#1	src/direct.rs:48
220	Variable	rust_sample::direct::chain::b#1	src/direct.rs:49
221	Variable	rust_sample::direct::chain::return#1	src/direct.rs:50

# edges (src -> dst  kind  condition  confidence)
217 -> 222	Calls	Always	Certain
217 -> 225	Calls	Always	Certain
217 -> 228	Calls	Always	Certain
219 -> 218	DerivesFrom	Always	Probable
220 -> 219	DerivesFrom	Always	Probable
221 -> 220	DerivesFrom	Always	Probable
301 -> 217	Calls	Always	Probable
```

Captured on a fresh copy of `fixtures/rust-sample` indexed on its own. The filter matched `chain` and its four SSA value nodes; the edges include those leaving the matched set (`217 -> 222`, the call to `step_a`) and entering it (`301 -> 217`, a caller). The unfiltered dump of the same index reports `nodes: 494  edges: 200  candidates: 18`.

### JSON

```
cgx dump --format json rust_sample::direct::chain
```

The top-level keys are `graph_key`, `counts` (`nodes`, `edges`, `candidates`), `nodes`, `edges`, and `candidates`. Each edge carries every stored field — `src`, `dst`, `kind`, `condition`, `confidence`, `tier`, `rule`, `site_id`, `candidate_group`, `cut_markers`, `cfg_condition`, `macro_origin`, `established_by`, `implicit`, `stmt_index`, `transform`, `id`:

```json
{"candidate_group": null, "cfg_condition": null, "condition": "always", "confidence": "certain", "cut_markers": [], "dst": 222, "established_by": null, "id": 77, "implicit": null, "kind": "calls", "macro_origin": null, "rule": "scope-ref", "site_id": 9810684976228922815, "src": 217, "stmt_index": 0, "tier": "scope_graph", "transform": null}
```

Nodes carry the same fields [`cgx explain`](explain.md) summarises, including `signature`, `visibility`, `lang`, the per-symbol `narrowing` counters, and the `unresolved_calls`/`external_calls` counts.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Graph printed, including a filter that matched nothing. |
| `2` | Bad argument or malformed option. |
| `3` | No index at the target path (`cgx: no index found at "…/.cgx"; run \`cgx index\` first`). |

## See also

- [cgx explain](explain.md) — one symbol's provenance and incident edges, resolved and formatted
- [cgx doctor](doctor.md) — aggregate quality report over the same stored graph
- [03-code-graph-model.md](../03-code-graph-model.md) — node kinds, edge kinds, confidence tiers, and cut-markers
