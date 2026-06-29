# cgx unused

Symbols not reachable from any entrypoint.

## Synopsis

```
cgx unused [OPTIONS]
```

## Description

`cgx unused` answers the question: *what code is dead?* It queries the graph for every indexed symbol that no entrypoint can reach — directly or transitively — and returns them as a flat list ordered by file.

A symbol is unused when no walk from any entrypoint reaches it at the confidence floor in effect. Lowering the floor with `--confidence possible` (the default) casts the widest net; raising it to `--confidence certain` reports only symbols that are provably unreachable at the highest-certainty edges.

**No positional arguments.** `unused` takes only option flags; there is nothing to name because the query is always over the full graph.

**Kind filtering.** Pass `--kind` to restrict results to one symbol kind (`function`, `method`, `type`, etc.). Without `--kind`, every symbol kind is included, which may produce a large list on a large index.

**Confidence and entrypoints.** The graph marks entrypoints (e.g. `main`, `#[tokio::main]`, exported public symbols) during `cgx index`. Reachability is computed relative to those nodes. If a project has no indexed entrypoints, every symbol may appear as unused; run `cgx doctor` to verify entrypoint coverage.

**Edge-condition rendering.** `unused` returns flat symbol lines, not edge trees — edge condition tags do not appear in its output.

**CI use.** `--assert-empty` makes the command exit 1 if any unused symbols are found, suitable for CI gates. Pair with `--kind` to scope the gate precisely.

## Arguments

This subcommand takes no positional arguments.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--kind` | `function\|method\|type\|field\|variable\|module\|constant\|macro\|lambda\|entrypoint` | — | Restrict output to one symbol kind. No flag = all kinds. |
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. `dot`, `mermaid`, and `d2` are accepted but not meaningful for flat-list results. |
| `--at` | git ref | — | Pin the query to a specific commit's graph snapshot (Q-17). Requires the ref to have been indexed. |
| `--depth` | integer | — | Not meaningful for `unused` (no traversal); accepted but has no effect on results. |
| `--tree` | `full\|spanning` | `full` | Ignored by `unused` (flat list, not a tree). |
| `--confidence` | `possible\|probable\|certain` | — | Floor filter for reachability edges. No flag = all tiers counted as reaching. |
| `--assert-empty` | boolean flag | — | CI assertion mode: exit 1 if any results are found; exit 4 if the query passed vacuously (filters excluded all candidates). See [Exit codes](#exit-codes). |
| `--allow-vacuous` | boolean flag | — | Suppress the ADR-08 vacuity guard: converts exit 4 to exit 0 when `--assert-empty` passes vacuously. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### List unused functions in a repository

```
cgx unused --kind function \
  --repo /path/to/rust-sample
```

```
cgx_cli::forest::node_sort_key  (crates/cgx-cli/src/forest.rs:345)
cgx_cli::diff_edge_json  (crates/cgx-cli/src/main.rs:1235)
...
rust_sample::async_calls::http_post  (fixtures/rust-sample/src/async_calls.rs:49)
rust_sample::async_calls::sequential_awaits  (fixtures/rust-sample/src/async_calls.rs:58)
rust_sample::async_calls::delay_then_run  (fixtures/rust-sample/src/async_calls.rs:65)
rust_sample::async_calls::simulate_sleep  (fixtures/rust-sample/src/async_calls.rs:70)
rust_sample::cfg_feature::linux_only_init  (fixtures/rust-sample/src/cfg_feature.rs:39)
... (94 rust_sample functions total across the fixture; 307 total including cgx crates)
```

Each line is `<fqn>  (<file>:<line>)`. Exit 0 even when results are present.

### Dead-code module: verify a known-orphan appears

```
cgx unused --kind function \
  --repo /path/to/rust-sample
```

```
...
rust_sample::dead_code::orphan_computation  (fixtures/rust-sample/src/dead_code.rs:7)
rust_sample::dead_code::orphan_helper  (fixtures/rust-sample/src/dead_code.rs:11)
...
```

The `dead_code` module's functions appear because no entrypoint calls them.

### Machine-readable output with JSON format

```
cgx unused --kind function \
  --repo /path/to/rust-sample \
  --format json
```

```json
{
  "count": 307,
  "results": [
    {
      "file": "crates/cgx-cli/src/forest.rs",
      "fqn": "cgx_cli::forest::node_sort_key",
      "kind": "function",
      "line": 345
    },
    {
      "file": "crates/cgx-cli/src/main.rs",
      "fqn": "cgx_cli::diff_edge_json",
      "kind": "function",
      "line": 1235
    }
  ],
  "vacuous": false
}
```

The top-level object has `count` (total results), `results` (array of symbol records), and `vacuous` (whether the result set was vacuously empty due to filters).

### CI gate: fail if any unused functions exist

```
cgx unused --kind function \
  --repo /path/to/rust-sample \
  --assert-empty
```

```
cgx_cli::forest::node_sort_key  (crates/cgx-cli/src/forest.rs:345)
...
cgx: assertion failed: results found when none expected
```

Exit 1. Results are printed before the assertion error so the CI log shows what was found.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty list). Both are normal. |
| `1` | `--assert-empty` was given and unused symbols were found. |
| `2` | Bad input: unrecognized `--kind` value, or invalid `--format` argument. |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously: confidence filters excluded every candidate. Suppress with `--allow-vacuous`. |

## See also

- [cgx callers](callers.md) — find what calls a symbol (to investigate whether it truly has no callers)
- [cgx reaches](reaches.md) — boolean reachability check from a specific starting symbol
- [cgx doctor](doctor.md) — verify entrypoint coverage and index quality before interpreting unused results
- [cgx explain](explain.md) — inspect a specific symbol's caller/callee counts and incident edges
- [03-code-graph-model.md](../03-code-graph-model.md) — entrypoints, confidence tiers, and the graph data model
