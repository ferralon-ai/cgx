# cgx unused

Symbols not reachable from any entrypoint.

## Synopsis

```
cgx unused [OPTIONS]
```

## Description

`cgx unused` answers the question: *what code is dead?* It queries the graph for every indexed symbol that no entrypoint can reach — directly or transitively — and returns them as a flat list ordered by file.

A symbol is unused when no walk from any entrypoint reaches it at the confidence floor in effect. No `--confidence` flag means no floor, which is the widest net (equivalent to `--confidence possible`); raising it to `--confidence certain` reports every symbol unreachable over `certain` edges alone, which is a much larger and much weaker list.

**`unused` is the command where the approximation contract matters most.** Every result is a *negative* claim — "nothing reaches this" — and a negative claim from a graph with unresolved call sites is under-approximate by construction: a call the resolver could not follow might have reached the symbol. The contract states the count and the searched scope on every run. Treat the list as a candidate set for review, never as a delete list. See [Reading an answer](README.md#reading-an-answer).

**Value nodes are symbols too.** Without `--kind`, a default (dataflow-on) index reports SSA value nodes — `fn::local#N` variable rows — alongside functions, and they dominate the output: on the corpus used here, 16,184 results without `--kind` versus 373 with `--kind function`. Nearly every example below therefore passes `--kind function`.

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
| `--format` | `human\|json\|sarif` | `human` | Output format. `dot`, `mermaid`, and `d2` are listed in `--help` but rejected here with exit 2: they render a path walk, and `unused` returns a flat list. |
| `--at` | git ref | — | Pin the query to a specific commit's graph snapshot (Q-17). Requires the ref to have been indexed. |
| `--depth` | integer | unbounded | Bounds the entrypoint-reachability walk. Left unset the walk is unbounded, which is what makes the answer meaningful; `--depth 0` searches zero hops, so nothing but the entrypoints counts as reached — on the corpus used here that turns 373 unused functions into 1,139. |
| `--tree` | `full\|spanning` | `full` | Ignored by `unused` (flat list, not a tree). |
| `--confidence` | `possible\|probable\|certain` | — | Floor filter for reachability edges. No flag = no floor, all tiers counted as reaching. Raising the floor *grows* the result list, because fewer edges count as reaching. |
| `--assert-empty` | boolean flag | — | CI assertion mode: exit 1 if any unused symbols are found, exit 0 if none. Filter exclusion cannot make this pass vacuously — see [Exit codes](#exit-codes). |
| `--allow-vacuous` | boolean flag | — | Suppress the ADR-08 vacuity guard: converts exit 4 to exit 0 when `--assert-empty` passes vacuously. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### List unused functions in a repository

```
cgx unused --kind function \
  --repo /path/to/worktree
```

```
cgx_cli::forest::node_sort_key  (crates/cgx-cli/src/forest.rs:345)
cgx_cli::format_name  (crates/cgx-cli/src/main.rs:1618)
cgx_cli::diff_edge_json  (crates/cgx-cli/src/main.rs:1961)
cgx_cli::changed_edge_json  (crates/cgx-cli/src/main.rs:1970)
cgx_cli::diff_node_json  (crates/cgx-cli/src/main.rs:1984)
cgx_cli::output::confidence_str  (crates/cgx-cli/src/output.rs:52)
...
ts::effects::add  (fixtures/ts/src/effects.ts:10)
approximation: under-approximate — 10245 call(s) in the searched region resolved to no in-repo target (external/unindexed callee; no SCIP) and could not be followed | scope: call edges, confidence>=possible, depth=unbounded
freshness: current | indexed tree 5ea331d, working tree clean
```

*(373 result lines; the middle is elided here.)* Each line is `<fqn>  (<file>:<line>)`. Exit 0 even when results are present.

The contract is the part to read first. 10,245 call sites in the searched region resolved to no in-repo target, and every one of them is a way this list could be wrong in the direction that costs you: a function listed as unused may in fact be called through an edge the resolver could not follow. The `scope:` tail says the walk covered call edges at every confidence tier with no depth bound — so the *search* was as wide as it gets, and the under-approximation is entirely a resolution limit, not a traversal one.

### Dead-code module: verify a known-orphan appears

```
cgx unused --kind function \
  --repo /path/to/worktree
```

```
...
rust_sample::dead_code::orphan_computation  (fixtures/rust-sample/src/dead_code.rs:7)
rust_sample::dead_code::orphan_helper  (fixtures/rust-sample/src/dead_code.rs:11)
...
ts_sample::dead_code::orphanComputation  (fixtures/ts-sample/src/dead_code.ts:6)
ts_sample::dead_code::orphanHelper  (fixtures/ts-sample/src/dead_code.ts:10)
...
```

*(Excerpt from the same 373-line result; the approximation and freshness lines close it as above.)* The `dead_code` modules' functions appear because no entrypoint calls them — the Rust and TypeScript fixtures both carry a deliberate one, which makes them the fixed point to check a gate against.

### Machine-readable output with JSON format

```
cgx unused --kind function \
  --repo /path/to/worktree \
  --format json
```

```json
{
  "approximation": {
    "direction": "under",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": [
      {
        "code": "unresolved-external-calls",
        "detail": "10245 call(s) in the searched region resolved to no in-repo target (external/unindexed callee; no SCIP) and could not be followed",
        "direction": "under"
      }
    ],
    "scope": {
      "confidence_floor": "possible",
      "max_depth": null,
      "searched_edge_kinds": [
        "calls",
        "calls-virtual",
        "calls-closure",
        "calls-callback",
        "calls-async",
        "calls-indirect"
      ]
    }
  },
  "count": 373,
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
      "file": "crates/cgx-cli/src/forest.rs",
      "fqn": "cgx_cli::forest::node_sort_key",
      "kind": "function",
      "line": 345
    },
    {
      "file": "crates/cgx-cli/src/main.rs",
      "fqn": "cgx_cli::format_name",
      "kind": "function",
      "line": 1618
    }
  ],
  "vacuous": false
}
```

*(`results` elided to its first two of 373 entries.)* The top-level object has `count` (total results), `results` (array of symbol records), `vacuous` (whether the result set was vacuously empty due to filters), and the `approximation`/`freshness` pair. `approximation.scope` is the machine-readable form of the human `| scope:` tail; `max_depth: null` means the walk was unbounded, and `reasons[].code` is a stable token (`unresolved-external-calls`) a CI job can match on without parsing prose.

### CI gate: fail if any unused functions exist

```
cgx unused --kind function \
  --repo /path/to/worktree \
  --assert-empty
```

```
cgx_cli::forest::node_sort_key  (crates/cgx-cli/src/forest.rs:345)
...
approximation: under-approximate — 10245 call(s) in the searched region resolved to no in-repo target (external/unindexed callee; no SCIP) and could not be followed | scope: call edges, confidence>=possible, depth=unbounded
freshness: current | indexed tree 5ea331d, working tree clean
cgx: assertion failed: results found when none expected
```

Exit 1. Results are printed before the assertion error so the CI log shows what was found — and so does the contract, which is what tells a reviewer whether a *passing* gate meant anything. A gate that passes on an `under-approximate` answer with 10,000 unfollowed call sites has proved less than one that passes on an `exact` one.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty list). Both are normal. |
| `1` | `--assert-empty` was given and unused symbols were found. |
| `2` | Bad input: unrecognized `--kind` value, inaccessible `--repo`, or `--format dot\|mermaid\|d2` (flat-list results are not path-shaped). |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously because the graph had **no nodes at all**. Filters cannot cause this: `unused` computes the complement of entrypoint reachability, so its filtered and unfiltered counts are the same number and the "filters excluded every candidate" clause can never fire. Raising `--confidence` *grows* the result list, which yields exit 1, not exit 4. Suppress with `--allow-vacuous`. |

## See also

- [cgx callers](callers.md) — find what calls a symbol (to investigate whether it truly has no callers)
- [cgx reaches](reaches.md) — boolean reachability check from a specific starting symbol
- [cgx doctor](doctor.md) — verify entrypoint coverage and index quality before interpreting unused results
- [cgx explain](explain.md) — inspect a specific symbol's caller/callee counts and incident edges
- [03-code-graph-model.md](../03-code-graph-model.md) — entrypoints, confidence tiers, and the graph data model
