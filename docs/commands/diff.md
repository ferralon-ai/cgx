# cgx diff

Diff the call graph between two git refs.

## Synopsis

```
cgx diff [OPTIONS] <BASE> <HEAD>
```

## Description

`cgx diff` answers the question: *what changed in the call graph between two commits?* It re-indexes the repository at each ref, compares the resulting graphs, and reports added and removed nodes and edges.

Each output line is prefixed with `+` (added) or `-` (removed), followed by the kind (`edge` or `node`) and the relevant FQNs and source location.

**`--newer-than` mode:** Pass `--newer-than` to restrict output to edges that exist at `HEAD` but were absent at `BASE`. Removed edges and all node-only changes are suppressed. This is useful for auditing newly introduced call relationships.

**No traversal flags:** `diff` is a graph-comparison command, not a traversal command. It does not accept `--depth`, `--at`, `--confidence`, `--assert-empty`, `--allow-vacuous`, or `--no-auto-index`. It always operates over the full graph snapshot at each ref.

**Index requirement:** `diff` re-indexes the repository internally at each ref; it does not require a pre-built `.cgx/` index and does not read one. Run this command from inside the target git repository or pass `--repo`.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `BASE` | yes | The base git ref: a branch name, `HEAD~N`, a tag, or a full or abbreviated commit SHA. |
| `HEAD` | yes | The head git ref: typically `HEAD`, a branch name, or a commit SHA. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the git repository to diff. Must be a git working tree; `.cgx/` is not required. |
| `--format` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. `dot`, `mermaid`, and `d2` are path-shaped renderers; `diff` produces a change report, not paths, so use `human`, `json`, or `sarif` in practice. |
| `--newer-than` | boolean flag | — | Show only edges added at `HEAD` that were absent at `BASE`. Suppresses removed edges and node-only entries. |

## Examples

### Common case: changes between two commits

```
cgx diff HEAD~1 HEAD --repo /path/to/my-repo
```

```
+ edge  rust_sample::conditions::warn_about  ->  rust_sample::conditions::log_warn  (src/conditions.rs)
+ node  rust_sample::conditions::warn_about
```

Each `+ edge` line names the caller FQN, the callee FQN, and the source file. Each `+ node` line records a symbol that exists at `HEAD` but not at `BASE`.

### No changes between refs

When the call graph is identical at both refs:

```
cgx diff d34930c HEAD~1 --repo /path/to/my-repo
```

```
(no diff)
```

Exit code is still 0.

### Show only newly added edges (--newer-than)

```
cgx diff HEAD~1 HEAD --newer-than --repo /path/to/my-repo
```

```
+ edge  rust_sample::conditions::warn_about  ->  rust_sample::conditions::log_warn  (src/conditions.rs)
```

Only added edges are shown; removed edges and added-node entries are omitted.

### JSON output for scripted analysis

```
cgx diff HEAD~1 HEAD --format json --repo /path/to/my-repo
```

```json
{
  "added_edges": [
    {
      "dst": "rust_sample::conditions::log_warn",
      "file": "src/conditions.rs",
      "kind": "Calls",
      "src": "rust_sample::conditions::warn_about"
    }
  ],
  "added_nodes": [
    {
      "file": "src/conditions.rs",
      "fqn": "rust_sample::conditions::warn_about"
    }
  ],
  "changed_edges": [],
  "removed_edges": [],
  "removed_nodes": []
}
```

The top-level keys are `added_edges`, `added_nodes`, `changed_edges`, `removed_edges`, and `removed_nodes`. Each is an array; empty arrays are always present.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Diff completed successfully. This includes the no-diff case (`(no diff)`). |
| `2` | Bad argument: an unresolvable git ref, a malformed option, or a path that is not a git repository. |

`diff` does not support `--assert-empty`, so exit codes 1 and 4 are not applicable.

## See also

- [cgx index](index.md) — build or refresh a `.cgx/` index for traversal queries
- [cgx doctor](doctor.md) — inspect the quality of the current on-disk index
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — VCS integration, git-ref pinning, and how snapshots are stored
- [cgx callers](callers.md) — query callers of a symbol in the current index
- [cgx callees](callees.md) — query callees of a symbol in the current index
