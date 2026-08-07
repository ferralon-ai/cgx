# cgx callers

Symbols that (transitively) call `symbol`.

## Synopsis

```
cgx callers [OPTIONS] <SYMBOL>
```

## Description

`cgx callers` answers the question: *what calls this symbol?* It walks the call graph backward from the named symbol and returns every symbol that reaches it within the traversal depth, formatted as a tree rooted at the target.

Each result line shows the caller's FQN, its source location, and (when non-default) its [edge condition](../03-code-graph-model.md) and [confidence](../03-code-graph-model.md) tier. The default depth is 2, meaning direct callers and their callers. Widen it by passing a larger `--depth`; `--depth 0` bounds the walk *to* zero hops and returns the seed symbol alone, which the approximation contract reports as `under-approximate … depth<=0`. There is no unlimited setting for `callers` — the walk is always bounded, and the traversal is work-budgeted on top of that (an exhausted budget shows `[truncated]`).

Every answer carries the approximation contract and the index-freshness envelope as its last two lines (or as top-level `approximation`/`freshness` keys under `--format json`). See [Reading an answer](README.md#reading-an-answer).

**Edge-condition rendering:** `always` edges are omitted from output. `conditional` edges render as `[if]`. `exception` edges render as `[exc]`. `loop` and `panic` edges render verbatim. The tag appears only when the edge is non-default.

**Confidence filtering:** By default all three confidence tiers are shown (`possible`, `probable`, `certain`). Pass `--confidence` to set a floor — for example `--confidence probable` excludes `possible` results.

**Cycle detection:** If a caller appears on its own ancestor path, the cycle is noted inline as `↺ <fqn> (cycle)` rather than expanding indefinitely.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `SYMBOL` | yes | FQN or unambiguous short name of the symbol whose callers to enumerate. Partial matches resolve if exactly one symbol matches; ambiguous or unmatched input exits 2. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json\|sarif` | `human` | Output format. `dot`, `mermaid`, and `d2` are listed in `--help` (clap prints the whole enum on every command) but are rejected here with exit 2: they render a path walk, and `callers` returns a forest. |
| `--at` | git ref | — | Pin the query to a specific commit's graph snapshot (Q-17). The binary materializes the ref in a detached worktree, indexes it, and queries the resulting graph. Under `--at`, the freshness envelope measures drift against the *pinned* tree, so files changed by intervening commits count as dirty. |
| `--depth` | integer | `2` | Maximum traversal depth, honoured as given. `0` returns the seed symbol only — it does **not** mean unlimited. (The `--help` text claims `0` = unlimited; that is true only for [`cgx paths`](paths.md).) |
| `--tree` | `full\|spanning` | `full` | Tree rendering mode. `spanning` collapses duplicate subtrees to a single occurrence. |
| `--confidence` | `possible\|probable\|certain` | — | Floor filter: exclude edges below this confidence tier. No flag = all tiers shown. |
| `--assert-empty` | boolean flag | — | CI assertion mode: exit 1 if any results are found; exit 4 if the query passed vacuously (filters excluded all candidates). See [Exit codes](#exit-codes). |
| `--allow-vacuous` | boolean flag | — | Suppress the ADR-08 vacuity guard: converts exit 4 to exit 0 when `--assert-empty` passes vacuously. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### Common case: transitive callers of a function

```
cgx callers rust_sample::conditions::dispatch \
  --repo /path/to/worktree
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ ts_sample::closures::closureVariable  fixtures/ts-sample/src/closures.ts:6  [possible]
├─ ts_sample::closures::nestedClosures  fixtures/ts-sample/src/closures.ts:57  [possible]
└─ ts_sample::closures::nestedClosures::outer  fixtures/ts-sample/src/closures.ts:58  [possible]
   ├─ rust_sample::closures::nested_closures  fixtures/rust-sample/src/closures.rs:60  [possible]
   ├─ ts_sample::closures::closureVariable  fixtures/ts-sample/src/closures.ts:6  [possible]
   ├─ ts_sample::closures::nestedClosures  fixtures/ts-sample/src/closures.ts:57  [possible]
   └─ ↺ ts_sample::closures::nestedClosures::outer (cycle)
approximation: over- and under-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur; external/unindexed callees not modeled (no SCIP) (6 site(s) on the searched frontier)
freshness: current | indexed tree 5ea331d, working tree clean
```

Every caller here is `[possible]`: the TypeScript callers resolve through a same-name candidate set with no receiver type in the graph, and the contract says so — `over- and under-approximate`, over because the candidate set may include edges that never occur at runtime, under because unindexed callees are not modeled at all. Reading this as "`dispatch` has 4 callers" overstates it; reading it as "4 symbols may call `dispatch`, none confirmed" is what the answer says.

### Direct callers only (depth 1)

```
cgx callers rust_sample::conditions::dispatch \
  --repo /path/to/worktree \
  --depth 1
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ ts_sample::closures::closureVariable  fixtures/ts-sample/src/closures.ts:6  [possible]
├─ ts_sample::closures::nestedClosures  fixtures/ts-sample/src/closures.ts:57  [possible]
└─ ts_sample::closures::nestedClosures::outer  fixtures/ts-sample/src/closures.ts:58  [possible]
approximation: over- and under-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur; external/unindexed callees not modeled (no SCIP) (6 site(s) on the searched frontier); search stopped at depth 1; deeper edges were not explored
freshness: current | indexed tree 5ea331d, working tree clean
```

Narrowing the depth adds a reason to the contract rather than silently shrinking the answer: `search stopped at depth 1; deeper edges were not explored`.

### Filter to certain callers only (no speculative edges)

```
cgx callers rust_sample::conditions::dispatch \
  --repo /path/to/worktree \
  --confidence certain
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
approximation: under-approximate — external/unindexed callees not modeled (no SCIP) (3 site(s) on the searched frontier); 3 edge(s) below the confidence>=certain floor were excluded from the search | scope: call edges, confidence>=certain, depth<=2
freshness: current | indexed tree 5ea331d, working tree clean
```

A root line with no children means no `certain`-confidence callers exist for this symbol. Exit code is still 0. Because this is an absence answer, the contract adds a `| scope:` tail naming exactly what was searched — call edges, at or above `certain`, to depth 2 — so "no callers" can be read as the bounded claim it is.

### CI assertion: verify a symbol has no certain-confidence callers

```
cgx callers rust_sample::dead_code::orphan_computation \
  --repo /path/to/worktree \
  --confidence certain \
  --assert-empty
```

```
rust_sample::dead_code::orphan_computation  fixtures/rust-sample/src/dead_code.rs:7
approximation: under-approximate — external/unindexed callees not modeled (no SCIP) (3 site(s) on the searched frontier); 3 edge(s) below the confidence>=certain floor were excluded from the search | scope: call edges, confidence>=certain, depth<=2
freshness: current | indexed tree 5ea331d, working tree clean
warning: --assert-empty passed vacuously (confidence/edge-condition filters excluded every candidate result); exit 4 (suppress with --allow-vacuous)
cgx: assertion passed vacuously (exit 4)
```

Exit 4 signals that the assertion passed vacuously — there were callers at lower confidence tiers, but `--confidence certain` excluded them all. The vacuity guard (ADR-08) surfaces this so CI does not silently pass on a misconfigured gate. Add `--allow-vacuous` to accept vacuous passes as exit 0.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty forest). Both are normal. |
| `1` | `--assert-empty` was given and results were present. |
| `2` | Bad input: no symbol matched the pattern, the pattern was ambiguous, `--repo` was inaccessible, or `--format dot\|mermaid\|d2` was passed (forest results are not path-shaped). |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously: confidence or edge-condition filters excluded every candidate result. Suppress with `--allow-vacuous`. |

## See also

- [cgx callees](callees.md) — the forward direction: what a symbol calls
- [cgx reaches](reaches.md) — boolean reachability with a witness path
- [cgx paths](paths.md) — enumerate every call path between two symbols
- [cgx explain](explain.md) — full provenance for one symbol including all incident edges
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
