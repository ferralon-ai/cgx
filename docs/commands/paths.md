# cgx paths

Enumerate the call paths from `from` to `to`.

## Synopsis

```
cgx paths [OPTIONS] <FROM> <TO>
```

## Description

`paths` answers the question: "What call sequences lead from symbol A to symbol B?" It performs a bounded depth-first search over the call graph and returns every distinct path it finds, each annotated with its hop count, minimum edge confidence, and whether any hop crosses an exceptional execution class.

The default depth limit is **6 hops**. Pass `--depth 0` to lift the limit; the search is still protected by an internal work budget and appends `[truncated]` to the output if the budget is exhausted on a dense graph. `paths` is the only command where `--depth 0` means unlimited — on `callers`, `callees`, `reaches`, `flows-to`, and `flows-from` it bounds the walk to zero hops.

Every answer ends with the approximation contract and the index-freshness envelope; under `--format json` they are the top-level `approximation` and `freshness` keys. See [Reading an answer](README.md#reading-an-answer).

Edge-condition tags appear inline on each hop when the condition is non-default:

| Condition | Rendered as |
|-----------|-------------|
| always (unconditional) | `[always]` |
| conditional | `[conditional]` |
| exception / exceptional path | `[exception]` |
| loop | `[loop]` |
| panic | `[panic]` |

An empty result set (no path exists) is a successful outcome: exit 0.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `<FROM>` | yes | FQN or short name of the source symbol |
| `<TO>` | yes | FQN or short name of the target symbol |

Both arguments accept a partial name; cgx resolves to the matching symbol. If the pattern matches no symbol, cgx exits 2. If the pattern is ambiguous and multiple symbols match, cgx resolves to the best match; use `cgx search` to find exact FQNs first.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository |
| `--format` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. All six work: `paths` always returns a path walk, which is what `dot`, `mermaid`, and `d2` render. Those three carry no approximation or freshness footer |
| `--at` | git ref | working HEAD | Pin the query to a historical graph snapshot (e.g. `HEAD~1`, a tag, a commit SHA) |
| `--depth` | integer | `6` | Maximum traversal depth. `0` = unlimited here (work-budgeted) — this is the one command where that is true |
| `--confidence` | `possible\|probable\|certain` | — | Minimum confidence floor; edges below this tier are excluded. No flag = no floor, equivalent to `possible` (the lowest tier) |
| `--assert-empty` | — | off | CI assertion: exit 1 if any paths are found |
| `--allow-vacuous` | — | off | Suppress the ADR-08 vacuity guard (vacuous pass exits 0 instead of 4) |
| `--no-auto-index` | — | off | Require an explicit `cgx index` first; a missing index exits 3 |
| `--tree` | `full\|spanning` | `full` | Forest shape — ignored by `paths` (paths are not forest-shaped) |
| `-h, --help` | — | — | Print help |

## Examples

### Single-hop conditional path

```
cgx paths rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --repo /path/to/worktree
```

```
path 1 (1 hops, min-confidence=certain):
     rust_sample::conditions::dispatch  (fixtures/rust-sample/src/conditions.rs:42)
  -> rust_sample::conditions::log_info  (fixtures/rust-sample/src/conditions.rs:4) [conditional]
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

The `[conditional]` tag indicates this call edge has a conditional edge condition (the call is guarded by a runtime check inside `dispatch`).

### Multi-hop path with an exceptional edge

```
cgx paths rust_sample::async_main rust_sample::async_calls::http_get \
  --repo /path/to/worktree
```

```
path 1 (2 hops, min-confidence=probable, crosses-exceptional):
     rust_sample::async_main  (fixtures/rust-sample/src/main.rs:41)
  -> rust_sample::async_calls::fetch_data  (fixtures/rust-sample/src/async_calls.rs:7) [always]
  -> rust_sample::async_calls::http_get  (fixtures/rust-sample/src/async_calls.rs:13) [exception]
path 2 (2 hops, min-confidence=probable):
     rust_sample::async_main  (fixtures/rust-sample/src/main.rs:41)
  -> rust_sample::async_calls::fetch_data  (fixtures/rust-sample/src/async_calls.rs:7) [always]
  -> rust_sample::async_calls::http_get  (fixtures/rust-sample/src/async_calls.rs:13) [always]
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

Two distinct paths exist. Path 1 traverses an exceptional edge (`[exception]`); cgx notes this in the path header as `crosses-exceptional`. Path 2 uses the unconditional edge (`[always]`). Both paths share the same symbols but differ in edge condition.

### JSON output for tool integration

```
cgx paths rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --format json \
  --repo /path/to/worktree
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
      "crosses_exceptional": false,
      "hops": 1,
      "min_confidence": "certain",
      "steps": [
        {
          "file": "fixtures/rust-sample/src/conditions.rs",
          "fqn": "rust_sample::conditions::dispatch",
          "line": 42
        },
        {
          "condition": "conditional",
          "file": "fixtures/rust-sample/src/conditions.rs",
          "fqn": "rust_sample::conditions::log_info",
          "line": 4
        }
      ]
    }
  ],
  "truncated": false,
  "truncation_reason": null,
  "vacuous": false
}
```

The JSON envelope includes `truncated` and `vacuous` fields for CI consumers; `crosses_exceptional` and `min_confidence` are per-path properties in the `results` array. `truncated` and `truncation_reason` are specific to path results — no other command's JSON carries them — and they are the machine-readable form of the `[truncated: …]` marker in the human view.

### Filtering by confidence

```
cgx paths rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --confidence certain \
  --repo /path/to/worktree
```

```
path 1 (1 hops, min-confidence=certain):
     rust_sample::conditions::dispatch  (fixtures/rust-sample/src/conditions.rs:42)
  -> rust_sample::conditions::log_info  (fixtures/rust-sample/src/conditions.rs:4) [conditional]
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

Paths whose minimum edge confidence falls below `certain` are excluded. Use this when you want to rule out speculative resolution paths. Here the floor excluded nothing — the one path was already `certain` — so the contract still reads `exact`. Had the floor dropped a candidate, it would read `under-approximate` and name the excluded edge count, which is how you tell "no speculative path exists" apart from "the filter removed the paths".

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success — zero or more paths found (empty result is not an error) |
| `1` | `--assert-empty` is set and at least one path was found |
| `2` | Symbol not matched, malformed argument, inaccessible `--repo`, or bad git ref |
| `3` | Index missing and `--no-auto-index` is set |
| `4` | Vacuous pass — the confidence filter excluded all candidate paths; use `--allow-vacuous` to suppress |

## See also

- [cgx reaches](reaches.md) — boolean reachability check with a witness path; use when you only need to know whether a path exists, not enumerate all of them
- [cgx callers](callers.md) — all symbols that (transitively) call a target
- [cgx callees](callees.md) — all symbols a source (transitively) calls
- [cgx search](search.md) — discover exact FQNs before running a paths query
- [cgx query](query.md) — CQL `MATCH … RETURN path` for path queries with custom predicates
