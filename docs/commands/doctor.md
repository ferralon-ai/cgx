# cgx doctor

Report on the quality of the current on-disk index.

## Synopsis

```
cgx doctor [OPTIONS]
```

## Description

`cgx doctor` inspects the `.cgx/` index stored under the target repository and emits a structured quality report. It answers the question: *how trustworthy is this index?*

The report covers:

- **Trust summary** — a single `HIGH` / `MEDIUM` / `LOW` verdict based on the checks below.
- **Node and edge counts** — total nodes, total edges, and the call-family edge subset.
- **Confidence distribution** — breakdown of call edges into `certain`, `probable`, and `possible` tiers, with percentages. A high `possible` share (> 50 %) indicates significant CHA/RTA speculation and warrants scrutiny before using the index for security-critical queries.
- **Unresolved references** — count and rate of edges where the callee could not be resolved to a definition. High unresolved rates (> 20 %) typically indicate missing dependencies or cross-language boundaries.
- **File coverage** — populated when the index was built with pipeline stats available. Shown as `(pipeline stats unavailable — run via cgx index to populate)` when not present.
- **Cut-marker inventory** — counts of known blind spots by category: `unresolved`, `unexpanded-macro`, `via-ffi`, `dynamic`, `reflective`, `via-di`. Non-zero counts flag locations where the graph is intentionally incomplete.
- **Anomalies** — any structural inconsistencies detected by internal invariant checks.

`doctor` takes no positional arguments and supports no traversal flags (`--depth`, `--at`, `--confidence`, `--assert-empty`, etc.). It is a diagnostic command, not a graph query.

## Arguments

`doctor` takes no positional arguments.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json\|sarif` | `human` | Output format. `dot`, `mermaid`, and `d2` appear in the help text but are path-shaped renderers; `doctor` produces a report, not paths, and falls through to `human` rendering for those values. Use `human`, `json`, or `sarif` in practice. |

## Examples

### Default human report

```
cgx doctor --repo /path/to/rust-sample
```

```
cgx doctor
trust:  HIGH   — index looks sound

nodes:          12659
edges:          21476  (total)
call edges:     18802  (call-family)

confidence distribution (call edges):
  certain:     2051  (10.9%)
  probable:    3691  (19.6%)
  possible:   13060  (69.5%)

unresolved references:
  3381/18802 refs unresolved  (18.0%)

file coverage:
  (pipeline stats unavailable — run via `cgx index` to populate)

cut-marker inventory (known blind spots):
  unresolved:         3381
  unexpanded-macro:      0
  via-ffi:               2
  dynamic:               0
  reflective:            0
  via-di:                0

anomalies:  none
```

The `possible` share of 69.5 % is high; these edges arise from CHA/RTA speculation over a multi-language corpus that includes a TypeScript fixture. The 18 % unresolved rate reflects the same cross-language boundary — Rust callers of TypeScript symbols cannot resolve to definitions in a Rust-only index.

### Fresh single-language index (lower unresolved rate)

After indexing a pure Rust crate with no external cross-language symbols the unresolved rate drops to zero:

```
cgx doctor
```

```
cgx doctor
trust:  HIGH   — index looks sound

nodes:            485
edges:            193  (total)
call edges:       107  (call-family)

confidence distribution (call edges):
  certain:       75  (70.1%)
  probable:      14  (13.1%)
  possible:      18  (16.8%)

unresolved references:
  0/107 refs unresolved  (0.0%)

file coverage:
  (pipeline stats unavailable — run via `cgx index` to populate)

cut-marker inventory (known blind spots):
  unresolved:            0
  unexpanded-macro:      0
  via-ffi:               2
  dynamic:               0
  reflective:            0
  via-di:                0

anomalies:  none
```

The 2 `via-ffi` cut-markers indicate FFI call sites whose callees cannot be followed into native code — expected and benign for most codebases.

### JSON output for CI or scripted checks

```
cgx doctor --repo /path/to/rust-sample --format json
```

```json
{"node_count":12659,"edge_count":21476,"call_edge_count":18802,"confidence":{"certain":2051,"probable":3691,"possible":13060},"total_refs":18802,"unresolved_count":3381,"unresolved_rate":0.17982129560685034,"unsupported_files":0,"total_files":null,"unsupported_share":null,"cut_markers":{"unresolved":3381,"unexpanded_macro":0,"via_ffi":2,"dynamic":0,"reflective":0,"via_di":0},"anomalies":[],"trust":"high"}
```

The `trust` field is `"high"`, `"medium"`, or `"low"`. Pipe to `jq .trust` to gate CI on index quality.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Report generated successfully (including when anomalies are present — the report itself is the output). |
| `2` | Bad argument or malformed option. |

`doctor` does not support `--assert-empty`, so exit codes 1 and 4 are not applicable.

## See also

- [cgx index](index.md) — build or refresh the `.cgx/` index that `doctor` inspects
- [cgx unused](unused.md) — enumerate symbols unreachable from any entrypoint
- [03-code-graph-model.md](../03-code-graph-model.md) — confidence tiers, cut-markers, and the graph data model
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — how indexing works, VCS integration, and incremental rebuild
