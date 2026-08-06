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
- **File coverage** — how many of the files the indexer walked it could not parse, as `unsupported: N/M (P%)`. A plain `cgx index` populates it. A high unsupported share is not a failure; it is the honest denominator for every other number in the report, because an unparsed file contributes no nodes and no edges.
- **Cut-marker inventory** — counts of known blind spots by category: `unresolved`, `unexpanded-macro`, `via-ffi`, `dynamic`, `reflective`, `via-di`. Non-zero counts flag locations where the graph is intentionally incomplete.
- **Anomalies** — any structural inconsistencies detected by internal invariant checks.

`doctor` takes no positional arguments and supports no traversal flags (`--depth`, `--at`, `--confidence`, `--assert-empty`, etc.). It is a diagnostic command, not a graph query.

**`doctor` does not auto-index.** Every other read command builds a missing `.cgx/` store on demand; `doctor` reads the store pointer directly and exits 3 if there is none — it has no `--no-auto-index` flag because it never auto-indexes in the first place. Run [`cgx index`](index.md) first.

**No approximation contract, no freshness envelope.** The whole report is an index-quality verdict, so it would be reporting on itself. Where those two lines appear on other commands, `doctor` gives you `trust:`, the confidence distribution, and the cut-marker inventory instead — see [Reading an answer](README.md#reading-an-answer).

## Arguments

`doctor` takes no positional arguments.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json` | `human` | Output format. Only `json` differs from the default: `sarif`, `dot`, `mermaid`, and `d2` are accepted, silently render the human report, and exit 0. Do not pipe `cgx doctor --format sarif` into a SARIF consumer — it will receive plain text with a zero exit status. |

## Examples

### Default human report

```
cgx doctor --repo /path/to/worktree
```

```
cgx doctor
trust:  HIGH   — index looks sound

nodes:          19013
edges:          37103  (total)
call edges:     33371  (call-family)

confidence distribution (call edges):
  certain:     3067  (9.2%)
  probable:    3686  (11.0%)
  possible:   26618  (79.8%)

unresolved references:
  4797/33371 refs unresolved  (14.4%)

file coverage:
  unsupported: 134/416  (32.2%)

cut-marker inventory (known blind spots):
  unresolved:         4797
  unexpanded-macro:      0
  via-ffi:               2
  dynamic:               0
  reflective:            0
  via-di:                0

anomalies:  none
```

The `possible` share of 79.8 % is high; these edges arise from CHA/RTA speculation and bare-name resolution over a multi-language corpus that includes a TypeScript fixture. The 14.4 % unresolved rate reflects the same cross-language boundary — Rust callers of TypeScript symbols cannot resolve to definitions in a Rust-only index. The 32.2 % unsupported file share is mostly non-source files the walker saw and skipped.

Read this report as the standing explanation for the approximation contracts you will see on individual queries. A corpus where four call edges in five are `possible` is a corpus where `cgx callers` will routinely answer `over- and under-approximate`, and that is the index saying so up front rather than each answer having to.

### Fresh single-language index (lower unresolved rate)

After indexing a pure Rust crate with no external cross-language symbols the unresolved rate drops to zero. (This one was built with [`cgx index --no-dataflow`](index.md), so it carries no SSA value nodes — hence 212 nodes rather than 485.)

```
cgx doctor
```

```
cgx doctor
trust:  HIGH   — index looks sound

nodes:            212
edges:            134  (total)
call edges:       107  (call-family)

confidence distribution (call edges):
  certain:       75  (70.1%)
  probable:      13  (12.1%)
  possible:      19  (17.8%)

unresolved references:
  0/107 refs unresolved  (0.0%)

file coverage:
  unsupported: 1/18  (5.6%)

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
cgx doctor --repo /path/to/worktree --format json
```

```json
{"node_count":19013,"edge_count":37103,"call_edge_count":33371,"confidence":{"certain":3067,"probable":3686,"possible":26618},"total_refs":33371,"unresolved_count":4797,"unresolved_rate":0.14374756525126606,"unsupported_files":134,"total_files":416,"unsupported_share":0.32211538461538464,"cut_markers":{"unresolved":4797,"unexpanded_macro":0,"via_ffi":2,"dynamic":0,"reflective":0,"via_di":0},"anomalies":[],"trust":"high"}
```

The `trust` field is `"high"`, `"medium"`, or `"low"`. Pipe to `jq .trust` to gate CI on index quality. This is the only command whose JSON is emitted on a single line rather than pretty-printed, and the only one with no `freshness` or `approximation` key.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Report generated successfully (including when anomalies are present — the report itself is the output). Also returned for `--format sarif\|dot\|mermaid\|d2`, which silently render the human report. |
| `2` | Bad argument or malformed option. |
| `3` | No index at the target path (`cgx: no index found at "…/.cgx"; run \`cgx index\` first`). Unlike every other read command, `doctor` never auto-indexes. |

`doctor` does not support `--assert-empty`, so exit codes 1 and 4 are not applicable.

## See also

- [cgx index](index.md) — build or refresh the `.cgx/` index that `doctor` inspects
- [cgx unused](unused.md) — enumerate symbols unreachable from any entrypoint
- [03-code-graph-model.md](../03-code-graph-model.md) — confidence tiers, cut-markers, and the graph data model
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — how indexing works, VCS integration, and incremental rebuild
