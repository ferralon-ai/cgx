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

> **Known defect — `cgx diff` destroys the persisted dataflow layer.** Running `cgx diff` against a
> repository whose `.cgx/` index was built with dataflow (the default) leaves that index with **zero
> SSA value nodes**. It exits 0 and prints no warning. Both diff modes do it.
>
> Observed on a `fixtures/rust-sample` index: 276 value nodes before `cgx diff HEAD~1 HEAD`, **0**
> after; total nodes 489 → 213.
>
> The damage is silent in three directions at once. [`cgx flows-to`](flows-to.md) and
> [`cgx flows-from`](flows-from.md) then exit 2 with `no symbol matched pattern …`, which reads like
> a mistyped FQN rather than data loss. A `[:DATA_FLOW]` [`cgx query`](query.md) returns zero rows
> **while reporting `approximation: exact (within modeled graph)`** — an exactness claim over a layer
> that is no longer there. And [`cgx doctor`](doctor.md) still reports `trust: HIGH — index looks
> sound`.
>
> **Recovery:** re-run [`cgx index`](index.md). Verified to restore the layer in full (276 value
> nodes, `flows-to` working again); the rebuild is cheap because extraction is content-addressed and
> cached — only the dataflow pass recomputes.
>
> Until this is fixed, treat `cgx diff` as invalidating the dataflow layer: re-index before any
> `flows-to`, `flows-from`, or `DATA_FLOW` query that follows a `diff` on the same repository, or run
> `diff` against a throwaway clone.

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
| `--format` | per mode — see below | `human` | Output format. The accepted set differs by mode; anything outside it is rejected with exit 2 **before** any indexing happens. |

**Format support is an allow-list per mode, not a single list.** A full `cgx diff` is a set of edge and node buckets; `--path-added` is a graph walk. Only the second is path-shaped, so only the second accepts the graph renderers — and neither accepts `sarif`, which is not implemented for either shape.

| Mode | Accepted | Rejected (exit 2) |
|------|----------|-------------------|
| full diff (default) | `human`, `json` | `sarif`, `dot`, `mermaid`, `d2` |
| `--path-added` | `human`, `json`, `dot`, `mermaid`, `d2` | `sarif` |

```
$ cgx diff HEAD~1 HEAD --format sarif --repo /path/to/my-repo
cgx: sarif format is not supported by `cgx diff` — use --format human|json
$ echo $?
2
$ cgx diff HEAD~1 HEAD --path-added --from '**::handler::*' --to '**::command::run' --format sarif --repo /path/to/my-repo
cgx: sarif format is not supported by `cgx diff --path-added` — use --format human|json|dot|mermaid|d2
$ echo $?
2
```

The rejection is deliberate: an unimplemented format fails loudly rather than falling through to human text with a zero exit, which is what a CI job piping `--format sarif` into a SARIF consumer needs.

**`diff` carries no approximation contract and no freshness envelope,** in either mode and either format. It never reads the on-disk index — it builds a graph at each ref itself — so there is no index freshness to report, and a graph-to-graph delta is not a traversal answer. See [Reading an answer](README.md#reading-an-answer).

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
| `--from` | glob | (any) | no | Keep only edges whose source FQN matches this glob (e.g. `'**::handler::*'`). With `--path-added`, this is the path source anchor. |
| `--to` | glob | (any) | no | Keep only edges whose destination FQN matches this glob. With `--path-added`, this is the path sink anchor. |

**Glob syntax — `*` does not cross `::`.** The matcher is segment-aware, and this is the single most common reason an anchor silently matches nothing:

| Token | Matches |
|-------|---------|
| `**` | any run of characters, **including** `::` separators |
| `*` | any run of characters containing **no** `::` |
| `?` | exactly one non-separator character |
| anything else | itself, literally |

So `'*::command::run'` does **not** match `my_crate::sys::command::run` — `*` cannot span `my_crate::sys`. Write `'**::command::run'`. A glob with a fixed number of `*`-separated segments only matches FQNs of exactly that depth; when in doubt, use `**`.

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
| `2` | Usage or anchor error: an unresolvable git ref, an inaccessible `--repo`, a malformed option, a `--format` the selected mode does not implement, or `--path-added` missing `--from`/`--to`. Also fires when `--require-anchor-match` is set and an anchor glob matches zero graph nodes. |
| `3` | Graph error: missing or corrupt index data during ref resolution. |

Diff mode does not support `--assert-empty`, so exit codes 1 and 4 from the standard assertion contract are not applicable. Gate mode reuses exit code 1 (the existing `AssertionFailed` convention) to mean "new path found."

Result output (human or JSON) is written to stdout before any nonzero exit. The gate status message goes to stderr.

## Zero-match anchors and the SCIP caveat

**This is the most important honesty constraint for `--path-added` in CI.**

A `--from` or `--to` glob matches against resolved FQNs in the head graph. If the glob matches zero nodes, no reachability search is attempted, and the gate returns "no new path" — which would exit 0 by default.

**This is not a clean bill of health.** A zero-match anchor means the symbol is not a node in the indexed graph. External standard-library symbols (for example `std::process::Command::new`) are not indexed without SCIP data, so a glob like `--to 'std::process::Command::*'` will match nothing on a plain `cgx index` and silently return exit 0. That exit 0 means "the sink is not indexed," not "no path to the sink exists."

**Default behavior (warning, not error):** When an anchor matches zero nodes, `cgx` emits a stderr warning:

```
cgx: warning: --to glob "**::command::run" matched 0 nodes in the head graph — a "clean" result means the symbol is not indexed (external/std symbols are not graph nodes without SCIP index data), NOT that no path exists
(no new call/dataflow reachability path)
```

*(Wrapped here for width; the warning is one line.)* Under `--require-anchor-match` the same condition is a hard error instead, and no result is printed at all:

```
cgx: --to glob "**::command::run" matched 0 nodes in the head graph — a "clean" result means the symbol is not indexed (external/std symbols are not graph nodes without SCIP index data), NOT that no path exists (--require-anchor-match is set, so this is a hard error)
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
      "dst": "my_crate::handler::run_command",
      "file": "src/handler.rs",
      "kind": "Calls",
      "src": "my_crate::handler::process_request"
    },
    {
      "dst": "my_crate::sys::command::run",
      "file": "src/handler.rs",
      "kind": "Calls",
      "src": "my_crate::handler::run_command"
    }
  ],
  "added_nodes": [
    {
      "file": "src/handler.rs",
      "fqn": "my_crate::handler::run_command"
    }
  ],
  "changed_edges": [],
  "removed_edges": [],
  "removed_nodes": []
}
```

All five buckets are always present, empty when there is nothing in them. A `changed_edges` entry carries an extra `changes` object naming which of `condition`, `confidence`, or `tier` moved.

### Gate mode (`--path-added --format json`)

```json
{
  "added_paths": [
    {
      "author_time": 1786048703,
      "from": "my_crate::handler::process_request",
      "introducing_author": "Dev Name",
      "introducing_commit": "6c77e0e16e3bc3eedaff156203c8f50837a8e073",
      "introducing_email": "dev@example.com",
      "to": "my_crate::sys::command::run",
      "via": [
        "my_crate::handler::process_request",
        "my_crate::handler::run_command",
        "my_crate::sys::command::run"
      ]
    },
    {
      "author_time": 1786048703,
      "from": "my_crate::handler::run_command",
      "introducing_author": "Dev Name",
      "introducing_commit": "6c77e0e16e3bc3eedaff156203c8f50837a8e073",
      "introducing_email": "dev@example.com",
      "to": "my_crate::sys::command::run",
      "via": [
        "my_crate::handler::run_command",
        "my_crate::sys::command::run"
      ]
    }
  ],
  "kind": "path-added",
  "semantics": "call/dataflow reachability (not a soundness/security guarantee)"
}
```

The `semantics` field is always present and always carries the reachability caveat. The `via` array lists the node FQNs on the shortest witness path at `HEAD`. `introducing_commit` is the full 40-character SHA of the commit blamed for the call site that established the path; the human view abbreviates it to 12 characters. Every anchor pair that gained a path is reported, so a single new call site can produce several `added_paths` entries — here both `process_request → run` and the shorter `run_command → run`.

> **Known limitation.** `introducing_commit` is derived by blaming the call site's location, and on a call added at `HEAD` it can report the *base* commit rather than the commit that introduced the call. Treat the attribution as a pointer for review, not as provenance you can gate on; `via`, `from`, and `to` are the reliable fields.

## Examples

### Common case: full diff between two commits

```
cgx diff HEAD~1 HEAD --repo /path/to/my-repo
```

```
+ edge  my_crate::handler::process_request  ->  my_crate::handler::run_command  (src/handler.rs)
+ edge  my_crate::handler::run_command  ->  my_crate::sys::command::run  (src/handler.rs)
+ node  my_crate::handler::run_command
```

Each `+ edge` line names the caller FQN, the callee FQN, and the source file. Each `+ node` line records a symbol that exists at `HEAD` but not at `BASE`. `-` marks removals and `~` marks a changed edge — one present on both sides whose condition, confidence, or tier moved.

*(The examples in this section were run against a two-commit fixture crate named `my_crate`, in which the head commit adds a `run_command` helper that shells out through `sys::command::run`.)*

### Show only newly added edges (`--added`)

```
cgx diff HEAD~1 HEAD --added --repo /path/to/my-repo
```

Equivalent to `--newer-than` (retained for compatibility). Only added edges are shown; removed edges and node entries are omitted.

### Filter added call-family edges by source module

```
cgx diff main HEAD --added --kind calls --from '**::handler::*' --repo /path/to/my-repo
```

Returns only call edges added at `HEAD` whose source FQN matches `**::handler::*`. Because this sets an edge predicate, node output is suppressed — see the bucket rule above.

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
cgx diff HEAD~1 HEAD \
    --path-added \
    --from '**::handler::*' \
    --to '**::command::run' \
    --require-anchor-match \
    --repo /path/to/my-repo
```

```
2 new call/dataflow reachability path(s) [reachability, not a security guarantee]:
+ path  my_crate::handler::process_request  ->  my_crate::handler::run_command  ->  my_crate::sys::command::run  [introduced by 6c77e0e16e3b (Dev Name)]
+ path  my_crate::handler::run_command  ->  my_crate::sys::command::run  [introduced by 6c77e0e16e3b (Dev Name)]
cgx: path-added gate: 2 new call/dataflow reachability path(s) found
```

Exit 1 — a new path was found. Exit 0 means no new reachability path was introduced. Exit 2 means an anchor matched no nodes (with `--require-anchor-match`).

Note the `[reachability, not a security guarantee]` banner on the header line. It is not decoration: the gate proves a path exists in the modeled graph, and nothing about whether that path is feasible at runtime or whether the sink is dangerous. A passing gate is also bounded by what got indexed — see [Zero-match anchors](#zero-match-anchors-and-the-scip-caveat).

### Gate mode: JSON output for CI artifact upload

```
cgx diff HEAD~1 HEAD \
    --path-added \
    --from '**::handler::*' \
    --to '**::command::run' \
    --require-anchor-match \
    --format json \
    --repo /path/to/my-repo
```

The shape is under [Gate mode](#gate-mode---path-added---format-json) above; exit 1 accompanies a non-empty `added_paths`.

### Gate mode: render the new path as a graph

```
cgx diff HEAD~1 HEAD \
    --path-added \
    --from '**::handler::*' \
    --to '**::command::run' \
    --format dot \
    --repo /path/to/my-repo
```

```
digraph cgx {
  rankdir=LR;
  n0 [label="my_crate::handler::process_request"];
  n1 [label="my_crate::handler::run_command"];
  n2 [label="my_crate::sys::command::run"];
  n0 -> n1;
  n1 -> n2;
}
```

Exit 1 (a path was found; the gate status still goes to stderr). This is the same emitter [`cgx paths`](paths.md) uses, so a `--path-added` graph and a `cgx paths` graph over the same nodes are byte-identical — with one difference: an added path carries no edge condition, so these edges are always unlabelled. `mermaid` and `d2` render the same walk in their own syntax.

### No changes between refs

When the call graph is identical at both refs:

```
cgx diff HEAD HEAD --repo /path/to/my-repo
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
