# cgx diff

Diff the call graph between two git refs.

## Synopsis

```
cgx diff [OPTIONS] <BASE> <HEAD>
```

## Description

`cgx diff` answers the question: *what changed in the call graph between two commits?* It re-indexes the repository at each ref, compares the resulting graphs, and reports added, removed, and changed edges and nodes.

The command has two modes:

**Diff mode (default):** Reports added, removed, and changed edges and nodes. Output is narrowed by S0 post-filters (set math over the computed diff; no re-parse). Any edge predicate (`--from`, `--to`, `--kind`, `--edge-condition`) suppresses node output.

**Structural gate mode (`--path-added`):** Reports whether a new call/dataflow reachability path from `--from` to `--to` exists at `HEAD` but not at `BASE`. Used as a CI gate. Requires both `--from` and `--to`; exits 2 if either is absent.

**No traversal flags:** `diff` does not accept `--depth`, `--at`, `--confidence`, `--assert-empty`, `--allow-vacuous`, or `--no-auto-index`. It always operates over full graph snapshots at both refs.

**Index requirement:** `diff` re-indexes the repository internally at each ref; it does not require a pre-built `.cgx/` index and does not read one. Run from inside the target git repository or pass `--repo`.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `BASE` | yes | The base git ref: a branch name, `HEAD~N`, a tag, or a full or abbreviated commit SHA. |
| `HEAD` | yes | The head git ref: typically `HEAD`, a branch name, or a commit SHA. |

## Options

### Global options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the git repository to diff. Must be a git working tree; `.cgx/` is not required. |
| `--format` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. `dot`, `mermaid`, and `d2` are path-shaped renderers; use `human`, `json`, or `sarif` for diff and gate output. |

### S0 post-filters (diff mode)

These flags narrow the diff output after the graph comparison is computed. They apply set math over the result; the graphs are not re-parsed. Any edge predicate suppresses node output.

| Flag | Value | Default | Repeatable | Meaning |
|------|-------|---------|-----------|---------|
| `--added` | — | (all) | no | Print only the added bucket (edges and nodes present at `HEAD` but not `BASE`). |
| `--removed` | — | (all) | no | Print only the removed bucket (edges and nodes present at `BASE` but not `HEAD`). |
| `--changed` | — | (all) | no | Print only the changed-edge bucket (edges present on both sides whose attributes differ). |
| `--newer-than` | — | off | no | Sugar for `--added`; retained for compatibility. |
| `--kind` | edge-kind | (any) | **yes** | Keep only edges of this kind. Repeatable: an edge matches if its kind is any of those given. `data-flow` maps to the `DerivesFrom` dataflow edge. Possible values: `calls`, `calls-virtual`, `calls-closure`, `calls-callback`, `calls-async`, `calls-indirect`, `spawns`, `contains`, `imports`, `data-flow`, `overrides`, `implements`, `inherits`, `references`, `instantiates`, `throws`, `catches`, `reads-field`, `writes-field`. |
| `--edge-condition` | condition | (any) | no | Keep only edges with exactly this edge condition. For changed edges, an edge matches if either side's condition matches. Possible values: `always`, `conditional`, `loop`, `exception`, `panic`. |
| `--from` | glob | (any) | no | Keep only edges whose source FQN matches this glob (e.g. `'*::handler::*'`). With `--path-added`, this is the path source anchor. |
| `--to` | glob | (any) | no | Keep only edges whose destination FQN matches this glob. With `--path-added`, this is the path sink anchor. |

**Bucket-flag rule:** If none of `--added`, `--removed`, `--changed`, or `--newer-than` are passed, all buckets print. Passing any bucket flag restricts output to that bucket only; multiple bucket flags combine as a union.

### S1 structural gate flags

| Flag | Value | Required with `--path-added` | Meaning |
|------|-------|------------------------------|---------|
| `--path-added` | — | — | Enter gate mode: report a new call/dataflow reachability path from `--from` to `--to` that exists at `HEAD` but not `BASE`. Requires both `--from` and `--to`. An unanchored search is rejected (exit 2) to avoid dense-graph blowup. This is reachability — NOT a soundness or security guarantee. |
| `--from` | glob | **yes** | Path source anchor: the glob pattern matching the starting node(s). |
| `--to` | glob | **yes** | Path sink anchor: the glob pattern matching the ending node(s). |
| `--require-anchor-match` | — | no | With `--path-added`: treat a `--from` or `--to` glob that matches zero graph nodes as a hard error (exit 2) instead of a stderr warning. See [Zero-match anchors](#zero-match-anchors-and-the-scip-caveat). |

## Exit codes

`diff` uses an extended exit-code contract when `--path-added` is active. This contract is a CI commitment.

| Code | Condition |
|------|-----------|
| `0` | Diff completed (diff mode), or gate ran and found no new path (gate mode). Includes the no-diff case. |
| `1` | Gate mode (`--path-added`): at least one new reachability path was found. Output names the path(s) and the introducing commit. |
| `2` | Usage or anchor error: an unresolvable git ref, a malformed option, or `--path-added` missing `--from`/`--to`. Also fires when `--require-anchor-match` is set and an anchor glob matches zero graph nodes. |
| `3` | Graph error: missing or corrupt index data during ref resolution. |

Diff mode does not support `--assert-empty`, so exit codes 1 and 4 from the standard assertion contract are not applicable. Gate mode reuses exit code 1 (the existing `AssertionFailed` convention) to mean "new path found."

Result output (human or JSON) is written to stdout before any nonzero exit. The gate status message goes to stderr.

## Zero-match anchors and the SCIP caveat

**This is the most important honesty constraint for `--path-added` in CI.**

A `--from` or `--to` glob matches against resolved FQNs in the head graph. If the glob matches zero nodes, no reachability search is attempted, and the gate returns "no new path" — which would exit 0 by default.

**This is not a clean bill of health.** A zero-match anchor means the symbol is not a node in the indexed graph. External standard-library symbols (for example `std::process::Command::new`) are not indexed without SCIP data, so a glob like `--to 'std::process::Command::*'` will match nothing on a plain `cgx index` and silently return exit 0. That exit 0 means "the sink is not indexed," not "no path to the sink exists."

**Default behavior (warning, not error):** When an anchor matches zero nodes, `cgx` emits a stderr warning:

```
cgx: warning: --to glob "std::process::Command::*" matched 0 nodes in the head graph
— a "clean" result means the symbol is not indexed (external/std symbols are not
graph nodes without SCIP index data), NOT that no path exists
```

The exit code remains 0. The silent false-negative is gone; the warning makes it visible.

**`--require-anchor-match` (recommended for CI):** Pass this flag to turn a zero-match anchor into a hard error (exit 2). This causes CI to fail closed when a configured sink is not in the graph — the correct behavior for a security gate.

| Anchor result | default | `--require-anchor-match` |
|---|---|---|
| Both anchors match ≥1 node; new path found | exit 1, path printed | same |
| Both anchors match ≥1 node; no new path | exit 0, clean | same |
| Either anchor matches 0 nodes | exit 0 + stderr warning | **exit 2** (hard error) |

**Resolving external sinks:** To match a call to `std::process::Command::new`, the called symbol must appear as a graph node. On a plain index, only in-repo (or SCIP-resolved) symbols are nodes. To gate on standard-library or third-party sinks, either:
1. Use an in-repo wrapper (e.g., a `sys::Command::run` thin wrapper around the external call) and gate on the wrapper's FQN.
2. Produce a SCIP index that covers the relevant dependencies, so external symbols become graph nodes.

## JSON output shape

### Diff mode (`--format json`)

```json
{
  "added_edges": [
    {
      "src": "my_crate::handler::process",
      "dst": "my_crate::sys::Command::run",
      "kind": "Calls",
      "file": "src/handler.rs"
    }
  ],
  "added_nodes": [
    {
      "fqn": "my_crate::handler::process",
      "file": "src/handler.rs"
    }
  ],
  "changed_edges": [],
  "removed_edges": [],
  "removed_nodes": []
}
```

### Gate mode (`--path-added --format json`)

```json
{
  "kind": "path-added",
  "semantics": "call/dataflow reachability (not a soundness/security guarantee)",
  "added_paths": [
    {
      "from": "my_crate::handler::process_request",
      "to": "my_crate::sys::Command::run",
      "via": [
        "my_crate::handler::process_request",
        "my_crate::handler::run_command",
        "my_crate::sys::Command::run"
      ],
      "introducing_commit": "a1b2c3d",
      "introducing_author": "Dev Name",
      "introducing_email": "dev@example.com",
      "author_time": 1580515200
    }
  ]
}
```

The `semantics` field is always present and always carries the reachability caveat. The `via` array lists the node FQNs on the shortest witness path at `HEAD`. `introducing_commit` is the git commit SHA that first established the path; it is present on the path even when `--format human` is used.

## Examples

### Common case: full diff between two commits

```
cgx diff HEAD~1 HEAD --repo /path/to/my-repo
```

```
+ edge  my_crate::handler::process  ->  my_crate::logger::log_warn  (src/handler.rs)
+ node  my_crate::handler::process
```

Each `+ edge` line names the caller FQN, the callee FQN, and the source file. Each `+ node` line records a symbol that exists at `HEAD` but not at `BASE`.

### Show only newly added edges (`--added`)

```
cgx diff HEAD~1 HEAD --added --repo /path/to/my-repo
```

Equivalent to `--newer-than` (retained for compatibility). Only added edges are shown; removed edges and node entries are omitted.

### Filter added call-family edges by source module

```
cgx diff main HEAD --added --kind calls --from '*::handler::*' --repo /path/to/my-repo
```

Returns only call edges added at `HEAD` whose source FQN matches `*::handler::*`.

### Filter to dataflow edges added on this branch

```
cgx diff main HEAD --added --kind data-flow --repo /path/to/my-repo
```

### Filter to exception-path edges only

```
cgx diff main HEAD --edge-condition exception --repo /path/to/my-repo
```

Returns edges (added or removed) where the edge condition is `exception`. Useful for auditing new error-path connections.

### Gate mode: detect a new handler-to-exec path

```
cgx diff main HEAD \
    --path-added \
    --from '*::handler::*' \
    --to '*::sys::Command::*' \
    --require-anchor-match \
    --repo /path/to/my-repo
```

Exit 0 means no new reachability path was introduced. Exit 1 means a new path was found; the output names the path and the introducing commit. Exit 2 means the anchor matched no nodes (with `--require-anchor-match`).

### Gate mode: JSON output for CI artifact upload

```
cgx diff main HEAD \
    --path-added \
    --from '*::handler::*' \
    --to '*::sys::Command::*' \
    --require-anchor-match \
    --format json \
    --repo /path/to/my-repo
```

### No changes between refs

When the call graph is identical at both refs:

```
cgx diff d34930c HEAD~1 --repo /path/to/my-repo
```

```
(no diff)
```

Exit code is 0.

## See also

- [cgx index](index.md) — build or refresh a `.cgx/` index for traversal queries
- [cgx doctor](doctor.md) — inspect the quality of the current on-disk index
- [cgx reaches](reaches.md) — single-snapshot reachability between two symbols
- [cgx paths](paths.md) — enumerate all call paths between two symbols at a single ref
- [cgx callers](callers.md) — query callers of a symbol in the current index
- [cgx callees](callees.md) — query callees of a symbol in the current index
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — VCS integration, git-ref pinning, and how snapshots are stored
- [questions/06-temporal-and-vcs-graph-diffs.md](../questions/06-temporal-and-vcs-graph-diffs.md) — cookbook recipes for graph diff questions (Q54–Q61)
