# cgx explain

Full provenance for one symbol: its definition, caller/callee counts, and every direct incident edge with its condition and confidence.

## Synopsis

```
cgx explain [OPTIONS] <SYMBOL>
```

## Description

`explain` answers the question "what do I know about this one symbol?" It returns:

- The symbol's definition location (file and line).
- Its kind (`function`, `method`, `type`, etc.).
- The total count of direct callers and callees.
- Every incident edge — both incoming (`<-`) and outgoing (`->`) — with its edge
  condition, confidence, resolution tier, rule, and call site.

This is the starting point for understanding an unfamiliar symbol before running a
directed query. Because `explain` covers all direct edges at once (not a traversal),
it has no `--depth`, `--tree`, or `--confidence` flags. Use `cgx callers` or
`cgx callees` for multi-hop traversal or confidence filtering.

### Edge conditions

Each edge carries a condition label in brackets. The five values:

| Condition     | Meaning |
|---------------|---------|
| `always`      | The call executes on every path through the caller. |
| `conditional` | The call executes only on some branches. |
| `exception`   | The call executes only on an exceptional (error/panic) path. |
| `loop`        | The call appears inside a loop body. |
| `panic`       | The call is a panic site. |

Confidence appears in a second bracket: `[possible]`, `[probable]`, or `[certain]`.
See [docs/03-code-graph-model.md](../03-code-graph-model.md) for the full definitions.

### Provenance fields per edge

Each edge line also carries:

| Field  | Meaning |
|--------|---------|
| `tier` | Resolution method: `scope_graph` (direct static), `cha_rta` (class-hierarchy / RTA) |
| `rule` | Matching rule: `scope-ref`, `sig-compat`, `name-method`, etc. |
| `site` | Call-site location (file:line) where the call expression appears |

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `<SYMBOL>` | yes | The symbol to explain. Accepts a fully-qualified name (FQN) or a short name. Short names resolve if unambiguous; when multiple symbols share a short name, use the full FQN. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo <REPO>` | path | CWD | Path to the indexed repository root. |
| `--format <FORMAT>` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. `sarif` is accepted by the parser but is not meaningful for a single-symbol lookup; prefer `human` or `json`. `dot`, `mermaid`, and `d2` are accepted by the parser but render as human output (explain has no path-shaped results to graph). |
| `--no-auto-index` | — | off | Require an explicit `cgx index` first. Without this flag cgx auto-indexes on first use. When set and no `.cgx/` index exists, the command exits with code 3. |
| `-h, --help` | — | — | Print help. |

**Flags not present on `explain`:** `--at`, `--depth`, `--tree`, `--confidence`,
`--assert-empty`, `--allow-vacuous`. These appear on traversal commands
(`callers`, `callees`, `paths`, etc.) but not here.

## Examples

### Common case: explain a function with callers and callees

```
cgx explain rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample
```

```
rust_sample::conditions::dispatch  (fixtures/rust-sample/src/conditions.rs:42)
  kind: function
  callers: 3, callees: 3
  edges:
    <- ts_sample::closures::closureVariable  (fixtures/ts-sample/src/closures.ts:6)  [always]  [possible]  tier=cha_rta  rule=sig-compat  site=fixtures/ts-sample/src/closures.ts:6
    <- ts_sample::closures::nestedClosures  (fixtures/ts-sample/src/closures.ts:57)  [always]  [possible]  tier=cha_rta  rule=sig-compat  site=fixtures/ts-sample/src/closures.ts:57
    <- ts_sample::closures::nestedClosures::outer  (fixtures/ts-sample/src/closures.ts:58)  [always]  [possible]  tier=cha_rta  rule=sig-compat  site=fixtures/ts-sample/src/closures.ts:58
    -> rust_sample::conditions::log_info  (fixtures/rust-sample/src/conditions.rs:4)  [conditional]  [certain]  tier=scope_graph  rule=scope-ref  site=fixtures/rust-sample/src/conditions.rs:42
    -> rust_sample::conditions::log_warn  (fixtures/rust-sample/src/conditions.rs:8)  [conditional]  [certain]  tier=scope_graph  rule=scope-ref  site=fixtures/rust-sample/src/conditions.rs:42
    -> rust_sample::conditions::log_error  (fixtures/rust-sample/src/conditions.rs:12)  [conditional]  [certain]  tier=scope_graph  rule=scope-ref  site=fixtures/rust-sample/src/conditions.rs:42
```

The three incoming edges are `cha_rta` (`sig-compat`) matches at `possible`
confidence with condition `always` (executes on every path). The three outgoing
callees are `scope_graph` (`scope-ref`) at `certain` confidence with condition
`conditional` (each callee is called only on some branch of `dispatch`).

### JSON output for programmatic use

```
cgx explain rust_sample::conditions::dispatch \
  --repo /path/to/rust-sample \
  --format json
```

The actual output is a single JSON object. The `edges` array contains all 6 direct
edges in the order they appear in the graph. Representative entries (one per
direction) showing all fields:

```
{
  "callees_count": 3,
  "callers_count": 3,
  "edges": [
    {
      "condition": "always",
      "confidence": "possible",
      "direction": "incoming",
      "peer": "ts_sample::closures::closureVariable",
      "peer_file": "fixtures/ts-sample/src/closures.ts",
      "peer_line": 6,
      "resolution_source": null,
      "rule": "sig-compat",
      "site": { "file": "fixtures/ts-sample/src/closures.ts", "line": 6 },
      "tier": "cha_rta"
    },
    {
      "condition": "conditional",
      "confidence": "certain",
      "direction": "outgoing",
      "peer": "rust_sample::conditions::log_info",
      "peer_file": "fixtures/rust-sample/src/conditions.rs",
      "peer_line": 4,
      "resolution_source": null,
      "rule": "scope-ref",
      "site": { "file": "fixtures/rust-sample/src/conditions.rs", "line": 42 },
      "tier": "scope_graph"
    }
  ],
  "file": "fixtures/rust-sample/src/conditions.rs",
  "kind": "function",
  "line": 42,
  "symbol": "rust_sample::conditions::dispatch"
}
```

The `condition` field carries the raw string (`"always"`, `"conditional"`, etc.).
The `direction` field is `"incoming"` for callers and `"outgoing"` for callees.

### Explain a function with only callers

```
cgx explain rust_sample::dataflow::flow_example \
  --repo /path/to/rust-sample
```

```
rust_sample::dataflow::flow_example  (fixtures/rust-sample/src/dataflow.rs:5)
  kind: function
  callers: 3, callees: 0
  edges:
    <- ts_sample::closures::closureVariable  (fixtures/ts-sample/src/closures.ts:6)  [always]  [possible]  tier=cha_rta  rule=sig-compat  site=fixtures/ts-sample/src/closures.ts:6
    <- ts_sample::closures::nestedClosures  (fixtures/ts-sample/src/closures.ts:57)  [always]  [possible]  tier=cha_rta  rule=sig-compat  site=fixtures/ts-sample/src/closures.ts:57
    <- ts_sample::closures::nestedClosures::outer  (fixtures/ts-sample/src/closures.ts:58)  [always]  [possible]  tier=cha_rta  rule=sig-compat  site=fixtures/ts-sample/src/closures.ts:58
```

`callees: 0` and no outgoing edges confirm this function makes no direct calls.

### Bad symbol: no match

```
cgx explain nonexistent::bad::symbol \
  --repo /path/to/rust-sample
```

```
cgx: no symbol matched pattern `nonexistent::bad::symbol`
```

Exit code 2. Use `cgx search <name>` to discover the correct FQN.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Symbol found and explained. Also returned when the symbol has zero incident edges. |
| `2` | No symbol matched the given name or FQN. Also returned for argument parsing errors. |
| `3` | Index missing and `--no-auto-index` was passed. |

`explain` does not support `--assert-empty` or `--allow-vacuous`, so exit codes 1
and 4 do not apply.

## See also

- [cgx search](search.md) — discover the exact FQN before running `explain`
- [cgx callers](callers.md) — transitive caller traversal with depth and confidence filtering
- [cgx callees](callees.md) — transitive callee traversal with depth and confidence filtering
- [cgx paths](paths.md) — enumerate call paths between two symbols
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge condition and confidence definitions
