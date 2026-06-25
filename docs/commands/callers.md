# cgx callers

Symbols that (transitively) call `symbol`.

## Synopsis

```
cgx callers [OPTIONS] <SYMBOL>
```

## Description

`cgx callers` answers the question: *what calls this symbol?* It walks the call graph backward from the named symbol and returns every symbol that reaches it within the traversal depth, formatted as a tree rooted at the target.

Each result line shows the caller's FQN, its source location, and (when non-default) its [edge condition](../03-code-graph-model.md) and [confidence](../03-code-graph-model.md) tier. The default depth is 2, meaning direct callers and their callers. Pass `--depth 0` for unlimited traversal (work-budgeted; may show `[truncated]`).

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
| `--format` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. `dot`, `mermaid`, and `d2` are listed in help but are path-shaped renderers; forest results from `callers` may render empty or malformed in those formats. |
| `--at` | git ref | — | Pin the query to a specific commit's graph snapshot (Q-17). The binary materializes the ref in a detached worktree, indexes it, and queries the resulting graph. |
| `--depth` | integer | `2` | Maximum traversal depth. `0` = unlimited (work-budgeted, may show `[truncated]`). |
| `--tree` | `full\|spanning` | `full` | Tree rendering mode. `spanning` collapses duplicate subtrees to a single occurrence. |
| `--confidence` | `possible\|probable\|certain` | — | Floor filter: exclude edges below this confidence tier. No flag = all tiers shown. |
| `--assert-empty` | boolean flag | — | CI assertion mode: exit 1 if any results are found; exit 4 if the query passed vacuously (filters excluded all candidates). See [Exit codes](#exit-codes). |
| `--allow-vacuous` | boolean flag | — | Suppress the ADR-08 vacuity guard: converts exit 4 to exit 0 when `--assert-empty` passes vacuously. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### Common case: transitive callers of a function

```
cgx callers rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ ts_sample::closures::closureVariable  fixtures/ts-sample/src/closures.ts:6  [possible]
├─ ts_sample::closures::nestedClosures  fixtures/ts-sample/src/closures.ts:57  [possible]
└─ ts_sample::closures::nestedClosures::outer  fixtures/ts-sample/src/closures.ts:58  [possible]
   ├─ rust_sample::closures::nested_closures  fixtures/rust-sample/src/closures.rs:60  [probable]
   ├─ ts_sample::closures::closureVariable  fixtures/ts-sample/src/closures.ts:6  [possible]
   ├─ ts_sample::closures::nestedClosures  fixtures/ts-sample/src/closures.ts:57  [possible]
   └─ ↺ ts_sample::closures::nestedClosures::outer (cycle)
```

### Direct callers only (depth 1)

```
cgx callers rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample \
  --depth 1
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ ts_sample::closures::closureVariable  fixtures/ts-sample/src/closures.ts:6  [possible]
├─ ts_sample::closures::nestedClosures  fixtures/ts-sample/src/closures.ts:57  [possible]
└─ ts_sample::closures::nestedClosures::outer  fixtures/ts-sample/src/closures.ts:58  [possible]
```

### Filter to certain callers only (no speculative edges)

```
cgx callers rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample \
  --confidence certain
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
```

A root line with no children means no `certain`-confidence callers exist for this symbol. Exit code is still 0.

### CI assertion: verify a symbol has no certain-confidence callers

```
cgx callers rust_sample::dead_code::orphan_computation \
  --repo /path/to/rust-sample \
  --confidence certain \
  --assert-empty
```

```
rust_sample::dead_code::orphan_computation  fixtures/rust-sample/src/dead_code.rs:7
warning: --assert-empty passed vacuously (confidence/edge-condition filters excluded every candidate result); exit 4 (suppress with --allow-vacuous)
cgx: assertion passed vacuously (exit 4)
```

Exit 4 signals that the assertion passed vacuously — there were callers at lower confidence tiers, but `--confidence certain` excluded them all. The vacuity guard (ADR-08) surfaces this so CI does not silently pass on a misconfigured gate. Add `--allow-vacuous` to accept vacuous passes as exit 0.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty forest). Both are normal. |
| `1` | `--assert-empty` was given and results were present. |
| `2` | Bad input: no symbol matched the pattern, or the pattern was ambiguous. |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously: confidence or edge-condition filters excluded every candidate result. Suppress with `--allow-vacuous`. |

## See also

- [cgx callees](callees.md) — the forward direction: what a symbol calls
- [cgx reaches](reaches.md) — boolean reachability with a witness path
- [cgx paths](paths.md) — enumerate every call path between two symbols
- [cgx explain](explain.md) — full provenance for one symbol including all incident edges
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
