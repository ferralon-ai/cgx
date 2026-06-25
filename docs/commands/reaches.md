# cgx reaches

Whether `from` reaches `to` (with a witness path), or all symbols `from` reaches when `to` is omitted.

## Synopsis

```
cgx reaches [OPTIONS] <FROM> [TO]
```

## Description

`cgx reaches` answers two related reachability questions against the call graph:

- **Reachability check** (`FROM` + `TO`): Does a call path exist from `FROM` to `TO`? If yes, the command prints the shortest witness path and exits 0. If no path exists, it prints `(no results)` and exits 0. Use this to confirm that one function can reach another before auditing or refactoring.

- **Reachability enumeration** (`FROM` only): What is the full set of symbols `FROM` can transitively call? The result is rendered as a forest rooted at `FROM`. Default traversal depth is 2; pass `--depth 0` for unlimited traversal (work-budgeted).

Both modes carry edge-condition tags on each step: edges that are always taken are unlabeled; `[if]` marks conditional edges; `[exc]` marks exception-path edges. Confidence is shown in brackets when it is below `certain`.

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
| `--format` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. `dot`, `mermaid`, and `d2` are listed in help but are path-shaped renderers; they work correctly for the two-argument reachability-check form and may produce empty or malformed output for the forest (enumeration) form |
| `--at` | git ref | — | Pin the query to a specific git ref's graph instead of the auto-indexed working HEAD |
| `--depth` | integer | `2` | Maximum traversal depth for the enumeration form. `--depth 0` is unlimited (work-budgeted; may report `[truncated]` on dense graphs) |
| `--tree` | `full\|spanning` | `full` | Forest shape for enumeration. `full` expands every call edge so a callee reached from multiple callers appears under each. `spanning` renders each symbol once under its shortest-path parent and annotates extra call sites with `(+N call sites)`. Ignored when `TO` is supplied and by machine formats |
| `--confidence` | `possible\|probable\|certain` | `possible` | Minimum confidence floor; edges below this level are excluded |
| `--assert-empty` | — | off | CI assertion: exit 1 if any results are found; exit 4 if the confidence/edge-condition filters excluded every candidate (vacuous pass); exit 0 only when no results exist and no filter is in play |
| `--allow-vacuous` | — | off | Suppress the vacuity guard: converts exit 4 to exit 0 |
| `--no-auto-index` | — | off | Do not auto-index when the `.cgx/` store is missing or stale; a missing index then exits 3 |

## Examples

### Reachability check: does `dispatch` reach `log_info`?

```
cgx reaches rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --repo /path/to/rust-sample
```

```
path 1 (1 hops, min-confidence=certain):
     rust_sample::conditions::dispatch  (fixtures/rust-sample/src/conditions.rs:42)
  -> rust_sample::conditions::log_info  (fixtures/rust-sample/src/conditions.rs:4) [conditional]
```

Exit 0. The `[conditional]` tag means the edge is taken only on some paths. Edge-condition rendering rules: `always` edges are unlabeled; `conditional` renders as `[if]` in the forest view (used in examples below) but is spelled out in the path view here.

### No-path case: symbol does not reach the target

```
cgx reaches rust_sample::dead_code::orphan_computation rust_sample::conditions::log_info \
  --repo /path/to/rust-sample
```

```
(no results)
```

Exit 0. An empty result is a normal outcome — it means no call path exists.

### Enumerate all reachable symbols (forest view, default depth 2)

```
cgx reaches rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ rust_sample::conditions::log_error  fixtures/rust-sample/src/conditions.rs:12  [if]
├─ rust_sample::conditions::log_info  fixtures/rust-sample/src/conditions.rs:4  [if]
└─ rust_sample::conditions::log_warn  fixtures/rust-sample/src/conditions.rs:8  [if]
```

Exit 0. All three callees are reached on conditional paths (`[if]`).

### Spanning forest with deeper traversal

```
cgx reaches rust_sample::async_main \
  --repo /path/to/rust-sample \
  --tree spanning
```

```
rust_sample::async_main  fixtures/rust-sample/src/main.rs:41
└─ rust_sample::async_calls::fetch_data  fixtures/rust-sample/src/async_calls.rs:7  [probable]
   ├─ cgx_store::token::&mut::Ok  crates/cgx-store/src/token.rs:53  [probable]  (+2 call sites)
   ├─ rust_sample::async_calls::http_get  fixtures/rust-sample/src/async_calls.rs:13  [exc]  (+1 call sites)
   └─ rust_sample::async_calls::parse_response  fixtures/rust-sample/src/async_calls.rs:21  [exc]  (+1 call sites)
```

Exit 0. `--tree spanning` renders each reachable symbol once. The `(+N call sites)` annotation counts duplicate in-edges that were folded. `[exc]` marks exception-path edges; `[probable]` marks edges resolved below `certain` confidence.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success — results found, or empty result (no path / no reachable symbols) |
| 1 | `--assert-empty` failed: results were found when none were expected |
| 2 | Bad argument — no symbol matched `FROM` or `TO`, or the argument could not be parsed |
| 3 | Missing index and `--no-auto-index` was set |
| 4 | `--assert-empty` passed vacuously: the confidence or edge-condition filter excluded every candidate result (suppress with `--allow-vacuous`) |

## See also

- [cgx paths](paths.md) — enumerate every distinct call path between two symbols (not just a witness)
- [cgx callers](callers.md) — symbols that transitively call a given symbol (reverse direction)
- [cgx callees](callees.md) — symbols that a given symbol transitively calls (same direction as `reaches` enumeration, without the two-argument reachability-check mode)
- [cgx flows-to](flows-to.md) — forward data-flow slice over `derives-from` edges (value nodes, not function symbols)
- [cgx flows-from](flows-from.md) — backward data-flow pedigree over `derives-from` edges
