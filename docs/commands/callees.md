# cgx callees

Symbols that `symbol` (transitively) calls.

## Synopsis

```
cgx callees [OPTIONS] <SYMBOL>
```

## Description

`cgx callees` answers the forward call-graph question: starting from a given symbol, what does it call, and what do those callees call in turn? The result is a depth-bounded forest rooted at `<SYMBOL>` where each child is a transitive callee.

Every edge in the result carries:

- An **edge condition** tag when the call is not unconditional. The tag is omitted for `always` edges; `conditional` renders as `[if]`; `exception` renders as `[exc]`; `loop` and `panic` appear verbatim.
- A **confidence** tag (`[probable]` or `[possible]`) when the resolution is not `certain`. Certain edges carry no confidence tag.

The command is the forward counterpart of `cgx callers`. It is a Layer 1 interface to the same graph traversal that `cgx query` exposes at Layer 2.

**Depth:** The default traversal depth is 2. Pass `--depth 0` for unlimited depth; that mode is work-budgeted and may append `[truncated]` on a dense graph.

**Dataflow:** The call-graph index includes the v0.3 DATA_FLOW layer by default. `cgx callees` traverses `calls`-family edges, not `derives-from` edges; dataflow value-node FQNs (e.g. `fn::local#1`) are not call-graph nodes and will not appear in callee results.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `<SYMBOL>` | yes | FQN or unique short name of the symbol to expand as the call-graph root. Substring and prefix matches are accepted; if the pattern is ambiguous, cgx emits an error (exit 2). |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository. Must point to a directory that contains (or is) the repo root; cgx locates the `.cgx/` store from here. |
| `--format` | `human` \| `json` \| `sarif` \| `dot` \| `mermaid` \| `d2` | `human` | Output format. `dot`, `mermaid`, and `d2` are listed in help but are path-shaped formats; forest results from `callees` may render empty or malformed in those formats. Use `json` or `sarif` for machine consumption. |
| `--at` | git ref | (working HEAD) | Pin the query to a specific git ref's graph (e.g. `HEAD~1`, a tag, or a commit SHA) instead of the auto-indexed working HEAD. |
| `--depth` | integer | `2` | Maximum traversal depth. `0` = unlimited (work-budgeted; may show `[truncated]`). |
| `--tree` | `full` \| `spanning` | `full` | Forest shape. `full` expands every call edge so a callee reached from multiple paths appears under each. `spanning` renders each symbol once under its shortest-path parent and annotates extra call sites as `(+N call sites)`. Ignored by machine formats. |
| `--confidence` | `possible` \| `probable` \| `certain` | (no filter) | Minimum confidence floor. Edges below this tier are excluded from results. When combined with `--assert-empty`, exclusion of all candidates triggers exit 4 (vacuous pass) rather than exit 0. |
| `--assert-empty` | (flag) | off | CI assertion mode: require zero results. Exit 1 if any callees are found; exit 0 if none; exit 4 if a confidence filter excluded every candidate (vacuous pass). |
| `--allow-vacuous` | (flag) | off | Suppress exit 4 from the vacuity guard; a vacuous `--assert-empty` pass exits 0 instead. |
| `--no-auto-index` | (flag) | off | Do not auto-index when `.cgx/` is missing or stale. A missing index exits 3 instead of triggering a build. |

## Examples

### Common case: callees of a function with conditional branches

```
cgx callees rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ rust_sample::conditions::log_error  fixtures/rust-sample/src/conditions.rs:12  [if]
├─ rust_sample::conditions::log_info  fixtures/rust-sample/src/conditions.rs:4  [if]
└─ rust_sample::conditions::log_warn  fixtures/rust-sample/src/conditions.rs:8  [if]
```

All three callees are tagged `[if]` (edge condition `conditional`): each is invoked only on one branch of a dispatch function.

### Deeper traversal with exception-path callees

```
cgx callees rust_sample::async_main \
  --repo /path/to/rust-sample
```

```
rust_sample::async_main  fixtures/rust-sample/src/main.rs:41
└─ rust_sample::async_calls::fetch_data  fixtures/rust-sample/src/async_calls.rs:7  [probable]
   ├─ cgx_store::token::&mut::Ok  crates/cgx-store/src/token.rs:53  [probable]
   ├─ rust_sample::async_calls::http_get  fixtures/rust-sample/src/async_calls.rs:13  [exc]
   ├─ rust_sample::async_calls::http_get  fixtures/rust-sample/src/async_calls.rs:13
   ├─ rust_sample::async_calls::parse_response  fixtures/rust-sample/src/async_calls.rs:21  [exc]
   └─ rust_sample::async_calls::parse_response  fixtures/rust-sample/src/async_calls.rs:21
```

`[exc]` marks exception-path callees (edge condition `exception`). `http_get` and `parse_response` each appear twice: once on the normal path and once on the exception path, as separate call edges.

### JSON output for machine consumption

```
cgx callees rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample \
  --format json
```

```json
{
  "count": 3,
  "results": [
    {
      "condition": "conditional",
      "confidence": "certain",
      "depth": 1,
      "file": "fixtures/rust-sample/src/conditions.rs",
      "fqn": "rust_sample::conditions::log_info",
      "kind": "function",
      "line": 4
    },
    {
      "condition": "conditional",
      "confidence": "certain",
      "depth": 1,
      "file": "fixtures/rust-sample/src/conditions.rs",
      "fqn": "rust_sample::conditions::log_warn",
      "kind": "function",
      "line": 8
    },
    {
      "condition": "conditional",
      "confidence": "certain",
      "depth": 1,
      "file": "fixtures/rust-sample/src/conditions.rs",
      "fqn": "rust_sample::conditions::log_error",
      "kind": "function",
      "line": 12
    }
  ],
  "vacuous": false
}
```

Each result object includes `fqn`, `file`, `line`, `kind`, `condition`, `confidence`, and `depth`.

### CI assertion: fail if a function still has callees

```
cgx callees rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample \
  --assert-empty
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ rust_sample::conditions::log_error  fixtures/rust-sample/src/conditions.rs:12  [if]
├─ rust_sample::conditions::log_info  fixtures/rust-sample/src/conditions.rs:4  [if]
└─ rust_sample::conditions::log_warn  fixtures/rust-sample/src/conditions.rs:8  [if]
cgx: assertion failed: results found when none expected
```

Exit code 1 — callees were found. Use this pattern in CI to guard that a function remains a leaf.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. Zero or more callees found (including the empty case). |
| `1` | `--assert-empty` failed: one or more callees were present. |
| `2` | Bad argument: no symbol matched the pattern, or the pattern is ambiguous. |
| `3` | Missing index and `--no-auto-index` was passed. |
| `4` | `--assert-empty` passed vacuously: a `--confidence` filter excluded every candidate result. Suppress with `--allow-vacuous`. |

## See also

- [cgx callers](callers.md) — the reverse direction: symbols that transitively call `symbol`
- [cgx paths](paths.md) — enumerate all call paths between two specific symbols
- [cgx reaches](reaches.md) — boolean reachability check with an optional witness path
- [cgx explain](explain.md) — full provenance for one symbol including direct incident edges
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge condition labels, confidence tiers, and transience semantics
- [docs/05-queries.md](../05-queries.md) — Layer 1 and Layer 2 query interfaces
