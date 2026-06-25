# cgx index

Index a repository or working tree and persist the graph under `.cgx/`.

## Synopsis

```
cgx index [OPTIONS] [PATH]
```

## Description

`cgx index` analyzes the source files in a repository and writes the call graph — and, by default, the SSA dataflow layer — to a `.cgx/` directory at the repository root. Every subsequent query command reads from this on-disk store.

The index is content-addressed by git blob OID: unchanged files are cache hits and cost nothing to re-analyze. Running `cgx index` again on a working tree with only a few edits re-indexes only the changed files.

**Dataflow layer (v0.3).** The DATA_FLOW layer — SSA value nodes and `derives-from` edges — is built by default. It is required for `cgx flows-to` and `cgx flows-from`. Pass `--no-dataflow` to produce a leaner base index that omits SSA nodes; the resulting index is byte-identical to the pre-v0.3 graph. An index built with `--no-dataflow` makes value-node FQNs unresolvable: `flows-to` and `flows-from` exit 2 on any symbol.

**SCIP enrichment (optional).** The `--scip` pass ingests a pre-built `.scip` file (produced by, e.g., `rust-analyzer scip`) and upgrades edge confidence where SCIP provides precise resolution. Without `--scip`, `cgx index` produces the Phase-1 syntactic graph, which is fully functional for all call-graph queries.

**PATH is positional.** `cgx index` does not accept `--repo`; the target is the positional `[PATH]` argument. All query commands (`callers`, `callees`, `paths`, etc.) accept `--repo` to point at the indexed directory.

## Arguments

| Argument | Required | Meaning |
|----------|----------|---------|
| `PATH` | No | Path to the repository root. Defaults to the current directory. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--scip <SCIP_INDEX>` | file path | _(omitted)_ | Path to a `.scip` index file (e.g. from `rust-analyzer scip`) for the Phase-2 semantic-precision re-label pass. Name-matched call edges are upgraded to `certain`/`probable` where SCIP gives a precise resolution; cross-crate dependency edges are recorded. Without this flag, the Phase-1 syntactic graph is produced unchanged. |
| `--no-dataflow` | _(boolean)_ | _(off)_ | Skip the v0.3 DATA_FLOW layer (SSA value nodes and `derives-from` edges). Dataflow is built by default (v0.3 SC6). Pass this flag for a leaner base index, byte-identical to the pre-dataflow graph. Value-node FQNs are absent from a `--no-dataflow` index; `flows-to`/`flows-from` exit 2. |
| `-h`, `--help` | | | Print help. |

## Examples

### Index a repository (default — with dataflow)

```
cgx index /tmp/cgx-index-scratch
```

```
Indexed /private/tmp/cgx-index-scratch (14a033de7e8fceefc799847fba8ee73f6aff64fb)
  blobs: 17 indexed, 17 extracted, 0 cached, 3 unsupported
  graph: 485 nodes, 193 edges, 108 unresolved
  cha: 9 sites trait-scoped, 0 supernode (cut-marked)
  rta: 0 sites pruned (0 candidates dropped), 9 cut-guarded
  dataflow: 89 fns recomputed, 0 fns reused; ifds: 32 summaries, 10 interproc edges, 0 budget-exceeded SCCs
```

The `dataflow:` line confirms the DATA_FLOW layer was built. SSA value nodes are present; `cgx flows-to` and `cgx flows-from` work immediately.

### Index from the repository root (current directory)

```
cd /path/to/my-repo
cgx index .
```

`cgx index` infers the path from the current directory. The `.cgx/` store is written at that root.

### Index without the dataflow layer

```
cgx index /tmp/cgx-index-scratch --no-dataflow
```

```
Indexed /private/tmp/cgx-index-scratch (14a033de7e8fceefc799847fba8ee73f6aff64fb)
  blobs: 17 indexed, 0 extracted, 17 cached, 3 unsupported
  graph: 212 nodes, 134 edges, 108 unresolved
  cha: 9 sites trait-scoped, 0 supernode (cut-marked)
  rta: 0 sites pruned (0 candidates dropped), 9 cut-guarded
```

No `dataflow:` line appears. The resulting index has fewer nodes (212 vs 485) because SSA value nodes are omitted. Call-graph queries (`callers`, `callees`, `paths`, etc.) work normally; `flows-to` and `flows-from` exit 2.

### Index with SCIP enrichment

```
rust-analyzer scip --output-file ra.scip
cgx index . --scip ra.scip
```

The SCIP pass upgrades call-edge confidence where `rust-analyzer` provides precise resolution. Call edges resolved to a single target are promoted to `certain`; name-matched edges with possible ambiguity become `probable`. Without `--scip`, all edges remain at Phase-1 confidence.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Index written successfully. |
| `3` | No git repository found at or above `PATH`. `cgx index` requires the target to be inside a git working tree. |

## See also

- [cgx doctor](doctor.md) — verify and report on the quality of an existing index
- [cgx diff](diff.md) — diff the call graph between two git refs (uses the same `.cgx/` store)
- [cgx callers](callers.md), [cgx callees](callees.md) — traverse the call graph after indexing
- [cgx flows-to](flows-to.md), [cgx flows-from](flows-from.md) — dataflow traversal (requires index with DATA_FLOW layer)
- [docs/06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — incremental indexing, blob-OID caching, branch-aware sharing
