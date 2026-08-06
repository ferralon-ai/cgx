# cgx reaches

Whether `from` reaches `to` (with a witness path), or all symbols `from` reaches when `to` is omitted.

## Synopsis

```
cgx reaches [OPTIONS] <FROM> [TO]
```

## Description

`cgx reaches` answers two related reachability questions against the call graph:

- **Reachability check** (`FROM` + `TO`): Does a call path exist from `FROM` to `TO`? If yes, the command prints the shortest witness path and exits 0. If no path exists, it prints `(no results)` and exits 0. Use this to confirm that one function can reach another before auditing or refactoring.

- **Reachability enumeration** (`FROM` only): What is the full set of symbols `FROM` can transitively call? The result is rendered as a forest rooted at `FROM`. Default traversal depth is 2; widen it with a larger `--depth`. `--depth 0` bounds the enumeration to zero hops and returns `FROM` alone — it is not an unlimited setting. The two-argument witness form is the one mode that defaults to unbounded depth.

Both modes carry edge-condition tags on each step: edges that are always taken are unlabeled; `[if]` marks conditional edges; `[exc]` marks exception-path edges. Confidence is shown in brackets when it is below `certain`.

Both modes also end with the approximation contract and the index-freshness envelope. On the check form these matter most on a *negative* answer: `(no results)` is only as strong as the region searched, and the contract's `| scope:` tail states that region. See [Reading an answer](README.md#reading-an-answer).

`reaches` queries the call-graph layer. It does not operate on dataflow (`derives-from`) edges; for value-propagation questions use `cgx flows-to` and `cgx flows-from`.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `FROM` | yes | FQN (or unambiguous short name) of the source symbol to traverse from |
| `TO` | no | FQN (or unambiguous short name) of the target symbol; when omitted the command enumerates every reachable symbol |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository |
| `--format` | `human\|json\|sarif` (+ `dot\|mermaid\|d2` in the check form) | `human` | Output format. `dot`, `mermaid`, and `d2` render the two-argument reachability-check form, whose result is a path walk. In the one-argument enumeration form the result is a forest, and those three are rejected with exit 2 |
| `--at` | git ref | — | Pin the query to a specific git ref's graph instead of the auto-indexed working HEAD. Under `--at`, freshness measures drift against the pinned tree |
| `--depth` | integer | `2` (enumeration) / unbounded (check) | Maximum traversal depth, honoured as given. `--depth 0` searches zero hops — the enumeration form then returns `FROM` alone and the check form returns `(no results)`. It is **not** unlimited, despite the `--help` text (that holds only for [`cgx paths`](paths.md)) |
| `--tree` | `full\|spanning` | `full` | Forest shape for enumeration. `full` expands every call edge so a callee reached from multiple callers appears under each. `spanning` renders each symbol once under its shortest-path parent and annotates extra call sites with `(+N call sites)`. Ignored when `TO` is supplied and by machine formats |
| `--confidence` | `possible\|probable\|certain` | — | Minimum confidence floor; edges below this level are excluded. No flag = no floor, which is equivalent to `possible` (the lowest tier) |
| `--assert-empty` | — | off | CI assertion: exit 1 if any results are found, otherwise exit 0 — except in the **enumeration** form, where a filter that excluded every candidate is a vacuous pass and exits 4. The **check** form never exits 4 on filter exclusion: it has no unfiltered re-run to compare against, so `--confidence certain` killing the only path is an ordinary empty result (exit 0) |
| `--allow-vacuous` | — | off | Suppress the vacuity guard: converts exit 4 to exit 0 |
| `--no-auto-index` | — | off | Do not auto-index when the `.cgx/` store is missing or stale; a missing index then exits 3 |

## Examples

### Reachability check: does `dispatch` reach `log_info`?

```
cgx reaches rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --repo /path/to/worktree
```

```
path 1 (1 hops, min-confidence=certain):
     rust_sample::conditions::dispatch  (fixtures/rust-sample/src/conditions.rs:42)
  -> rust_sample::conditions::log_info  (fixtures/rust-sample/src/conditions.rs:4) [conditional]
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

Exit 0. The `[conditional]` tag means the edge is taken only on some paths. Edge-condition rendering rules: `always` edges are unlabeled; `conditional` renders as `[if]` in the forest view (used in examples below) but is spelled out in the path view here.

### No-path case: symbol does not reach the target

```
cgx reaches rust_sample::dead_code::orphan_computation rust_sample::conditions::log_info \
  --repo /path/to/worktree
```

```
(no results)
approximation: exact (within modeled graph) | scope: call edges, confidence>=possible, depth=unbounded
freshness: current | indexed tree 5ea331d, working tree clean
```

Exit 0. An empty result is a normal outcome — it means no call path exists.

This is the answer the contract exists for. `(no results)` on its own is not a negative claim anyone should gate on; the `| scope:` tail turns it into one — the search followed call edges, accepted every confidence tier, and was not depth-bounded, and `exact` says no cut marker, floor, or bound truncated it. Narrow any of those and the direction drops to `under-approximate`, which is the signal that "no path" now means "no path *that this search could see*".

### Enumerate all reachable symbols (forest view, default depth 2)

```
cgx reaches rust_sample::conditions::dispatch \
  --repo /path/to/worktree
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ rust_sample::conditions::log_error  fixtures/rust-sample/src/conditions.rs:12  [if]
├─ rust_sample::conditions::log_info  fixtures/rust-sample/src/conditions.rs:4  [if]
└─ rust_sample::conditions::log_warn  fixtures/rust-sample/src/conditions.rs:8  [if]
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

Exit 0. All three callees are reached on conditional paths (`[if]`).

### Spanning forest with deeper traversal

```
cgx reaches rust_sample::async_main \
  --repo /path/to/worktree \
  --tree spanning
```

```
rust_sample::async_main  fixtures/rust-sample/src/main.rs:41
└─ rust_sample::async_calls::fetch_data  fixtures/rust-sample/src/async_calls.rs:7  [probable]
   ├─ cgx_store::token::&mut::Ok  crates/cgx-store/src/token.rs:53  [possible]  (+2 call sites)
   ├─ rust_sample::async_calls::http_get  fixtures/rust-sample/src/async_calls.rs:13  [exc]  (+1 call sites)
   └─ rust_sample::async_calls::parse_response  fixtures/rust-sample/src/async_calls.rs:21  [exc]  (+1 call sites)
approximation: over- and under-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur; 5 call(s) in the searched region resolved to no in-repo target (external/unindexed callee; no SCIP) and could not be followed; search stopped at depth 2; deeper edges were not explored
freshness: current | indexed tree 5ea331d, working tree clean
```

Exit 0. `--tree spanning` renders each reachable symbol once. The `(+N call sites)` annotation counts duplicate in-edges that were folded. `[exc]` marks exception-path edges; `[possible]` and `[probable]` mark edges resolved below `certain` confidence — here `fetch_data` was resolved by scope to `probable`, while `cgx_store::token::&mut::Ok` came from a bare-name candidate set and is only `possible`.

### Render a witness path as a graph

```
cgx reaches rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --repo /path/to/worktree \
  --format dot
```

```
digraph cgx {
  rankdir=LR;
  n0 [label="rust_sample::conditions::dispatch"];
  n1 [label="rust_sample::conditions::log_info"];
  n0 -> n1 [label="conditional"];
}
```

`dot`, `mermaid`, and `d2` are available in the two-argument check form only, because only that form returns a path walk. The same command without `TO` returns a forest and exits 2 with `Dot format is only valid for path-returning results`. Graph formats carry no approximation or freshness footer — there is nowhere in a `digraph` to put one — so gate CI on `--format json` and render `dot` for humans.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success — results found, or empty result (no path / no reachable symbols) |
| 1 | `--assert-empty` failed: results were found when none were expected |
| 2 | Bad argument — no symbol matched `FROM` or `TO`, the argument could not be parsed, `--repo` was inaccessible, or `--format dot\|mermaid\|d2` was passed in the enumeration form |
| 3 | Missing index and `--no-auto-index` was set |
| 4 | `--assert-empty` passed vacuously. Two causes, and they differ by form: `FROM` did not match any symbol (either form), or — **enumeration form only** — a confidence/edge-condition filter excluded every candidate. In the check form (`FROM TO`) a filter that excludes every path exits **0**, not 4. Suppress with `--allow-vacuous` |

## See also

- [cgx paths](paths.md) — enumerate every distinct call path between two symbols (not just a witness)
- [cgx callers](callers.md) — symbols that transitively call a given symbol (reverse direction)
- [cgx callees](callees.md) — symbols that a given symbol transitively calls (same direction as `reaches` enumeration, without the two-argument reachability-check mode)
- [cgx flows-to](flows-to.md) — forward data-flow slice over `derives-from` edges (value nodes, not function symbols)
- [cgx flows-from](flows-from.md) — backward data-flow pedigree over `derives-from` edges
