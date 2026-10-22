# cgx index

Index a repository or working tree and persist the graph under `.cgx/`.

## Synopsis

```
cgx index [OPTIONS] [PATH]
```

## Description

`cgx index` analyzes the source files in a repository and writes the call graph — and, by default, the SSA dataflow layer — to a `.cgx/` directory at the repository root. Every subsequent query command reads from this on-disk store.

**It indexes the committed `HEAD` tree.** Uncommitted edits are not part of the index: after editing a file and re-running `cgx index`, the indexed tree OID is unchanged and queries report `freshness: stale | indexed tree …, 1 dirty file`. Commit, then index, to bring the graph up to date. The index is content-addressed by git blob OID, so unchanged files are cache hits and cost nothing to re-analyze; indexing a commit that touched a few files extracts only those.

**Dataflow layer (v0.3).** The DATA_FLOW layer — SSA value nodes and `derives-from` edges — is built by default. It is required for `cgx flows-to` and `cgx flows-from`. Pass `--no-dataflow`, or set `data_flow = false` under `[index]` in a `cgx.toml` at the repository root, to produce a leaner base index that omits SSA nodes; the resulting index is byte-identical to the pre-v0.3 graph. An index built without dataflow makes value-node FQNs unresolvable: `flows-to` and `flows-from` exit 2 on any symbol.

**SCIP enrichment (optional).** `--scip` ingests a pre-built `.scip` file and upgrades edge confidence where SCIP gives a precise resolution. The producer is auto-detected from the index metadata: `rust-analyzer scip` for Rust, or `scip-clang` for C and C++ (overload, ADL, and template resolution). Without `--scip`, `cgx index` produces the Phase-1 syntactic graph, which is fully functional for all call-graph queries.

**scip-clang is fail-closed on the compilation database.** scip-clang resolves symbols only inside the translation units its `compile_commands.json` told it to compile; a TU missing from that compdb can make a symbol with several definitions look like it has one. So a scip-clang index's promotions are capped at `probable` unless cgx can establish that the compdb is complete:

- `--compdb <path>` names the `compile_commands.json`. cgx checks that every C++ translation unit it discovers in the tree (`.cpp`, `.cc`, `.cxx`; headers are not TUs and never count) has an entry. All covered → `COMPLETE`, promotions may reach `certain`. Any missing → `PARTIAL`, capped at `probable`, with a sample of the missing paths.
- `--compdb-complete` asserts completeness and skips the check, for builds that guarantee a full compdb.
- Neither, or an unreadable or malformed compdb → `PARTIAL`.

The compdb flags apply only to a scip-clang index. With a rust-analyzer index, or without `--scip`, they are ignored and no `compdb:` line is printed.

**Receiver narrowing (opt-in; Python and Go).** `--receiver-narrowing` restricts virtual call sites by what the receiver is, instead of linking every same-name method: in Python, `self`/`cls` calls to the enclosing class's cone, class-headed calls and `super()` to the class lookup, typed locals to their type, and literal or out-of-repo module receivers to no edge. Answers that lost candidates this way say so in their approximation contract, and [`cgx doctor`](doctor.md) reports the narrowed and dropped counts. The rules exist for Python and Go only; on other languages the flag changes nothing but still prints the `receiver lattice:` line.

**PATH is positional.** `cgx index` does not accept `--repo`; the target is the positional `[PATH]` argument, and `cgx index --repo <path>` is a clap parse error (exit 2, `unexpected argument '--repo' found`). All query commands (`callers`, `callees`, `paths`, etc.) accept `--repo` to point at the indexed directory.

**No approximation contract, no freshness envelope.** `cgx index` writes the snapshot the envelope later describes, so it reports on the indexing run instead — see [Reading an answer](README.md#reading-an-answer).

## Arguments

| Argument | Required | Meaning |
|----------|----------|---------|
| `PATH` | No | Path to the repository root. Defaults to the current directory. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--scip <SCIP_INDEX>` | file path | _(omitted)_ | Path to a `.scip` index for the Phase-2 semantic-precision re-label pass. The producer (`rust-analyzer scip` or `scip-clang`) is auto-detected. Name-matched call edges are upgraded to `certain`/`probable` where SCIP gives a precise resolution; cross-crate dependency edges are recorded. scip-clang promotions are capped at `probable` until the compdb is verified complete. Without this flag, the Phase-1 syntactic graph is produced unchanged. |
| `--compdb <COMPILE_COMMANDS>` | file path | _(omitted)_ | The `compile_commands.json` behind a scip-clang index. cgx verifies it covers every discovered C++ TU; only full coverage lets C++ promotions reach `certain`. Ignored for a rust-analyzer index or without `--scip`. |
| `--compdb-complete` | _(boolean)_ | _(off)_ | Assert, for CI, that the compdb covers every C++ TU, skipping the coverage check. Without it or `--compdb`, a scip-clang index stays capped at `probable`. |
| `--no-dataflow` | _(boolean)_ | _(off)_ | Skip the v0.3 DATA_FLOW layer (SSA value nodes and `derives-from` edges). Dataflow is built by default. Pass this flag for a leaner base index, byte-identical to the pre-dataflow graph. Value-node FQNs are absent from a `--no-dataflow` index; `flows-to`/`flows-from` exit 2. |
| `--receiver-narrowing` | _(boolean)_ | _(off)_ | Narrow virtual call sites by receiver kind (Python and Go). Also enabled by `CGX_RECEIVER_NARROWING=1`. |
| `-h`, `--help` | | | Print help. |

## Environment

| Variable | Effect |
|----------|--------|
| `CGX_RECEIVER_NARROWING=1` | Same as `--receiver-narrowing`. Only the exact value `1` enables it. |
| `CGX_PRECISION_TRACE=1` | With narrowing on, print one tab-separated line per narrowed call site to stderr: the rule, `file:line`, the call expression, then the edge count (`before->after` for the narrowing rules) and, where one survives, the kept target. |

## Output

```
Indexed <PATH> (<tree OID>)
  blobs: …
  graph: N nodes, N edges, N unresolved
```

then, each only when it applies:

| Line | Printed when |
|------|--------------|
| `scip: N certain, N probable, N dep edges, N collisions, N capped (partial compdb)` | `--scip` was given. |
| `compdb: COMPLETE — x/y C++ TUs covered; C++ promotions may reach certain` or `compdb: PARTIAL — x/y C++ TUs covered, n missing; C++ capped at probable (fail-closed)`, followed by `missing TUs: …` | The `--scip` index is from scip-clang. |
| `cha: …`, `rta: …`, `sig: …` | That pass rescoped, pruned, or resolved at least one site. |
| `receiver <rule>: …` | Narrowing is on and that rule touched at least one site. |
| `receiver lattice: …` | Narrowing is on, always. |
| `dataflow: …` | The dataflow layer was built. |

The 40-hex value after the path is the indexed **tree** OID, and it is the same value every later answer reports on its `freshness:` line — that is how a query says which snapshot it was computed over. `unresolved` counts references the resolver left dangling rather than guessing; those become the `unresolved` cut-marker inventory in [`cgx doctor`](doctor.md) and the `external/unindexed callee` reasons in query answers' approximation contracts.

## Examples

The first three examples were captured on a fresh copy of `fixtures/rust-sample` committed into its own repository.

### Index a repository (default — with dataflow)

```
cgx index /path/to/my-repo
```

```
Indexed /path/to/my-repo (c19123dd915dcb04a2735de3426b640ac2f21738)
  blobs: 18 indexed, 18 extracted, 0 cached, 0 stale, 1 unsupported
  graph: 494 nodes, 200 edges, 109 unresolved
  cha: 9 sites trait-scoped, 0 supernode (cut-marked)
  rta: 0 sites pruned (0 candidates dropped), 9 cut-guarded
  dataflow: 92 fns recomputed, 0 fns reused; ifds: 36 summaries, 14 interproc edges, 0 budget-exceeded SCCs
```

The `dataflow:` line confirms the DATA_FLOW layer was built. SSA value nodes are present; `cgx flows-to` and `cgx flows-from` work immediately. Re-running on the same commit extracts nothing (`0 extracted, 18 cached`) and reuses every function's dataflow (`0 fns recomputed, 92 fns reused`).

### Index from the repository root (current directory)

```
cd /path/to/my-repo
cgx index .
```

`cgx index` infers the path from the current directory. The `.cgx/` store is written at that root.

### Index without the dataflow layer

```
cgx index /path/to/my-repo --no-dataflow
```

```
Indexed /path/to/my-repo (c19123dd915dcb04a2735de3426b640ac2f21738)
  blobs: 18 indexed, 0 extracted, 18 cached, 0 stale, 1 unsupported
  graph: 216 nodes, 136 edges, 109 unresolved
  cha: 9 sites trait-scoped, 0 supernode (cut-marked)
  rta: 0 sites pruned (0 candidates dropped), 9 cut-guarded
```

No `dataflow:` line appears. All 18 blobs are cache hits — extraction is content-addressed and shared, and only the graph build changes. The index has fewer nodes (216 vs 494) because SSA value nodes are omitted. Call-graph queries (`callers`, `callees`, `paths`, etc.) work normally; `flows-to` and `flows-from` exit 2.

### Receiver narrowing on a Python project

Captured on a fresh copy of `fixtures/python-app`. Without narrowing the graph has `55 nodes, 24 edges, 4 unresolved`; with it:

```
cgx index . --receiver-narrowing
```

```
Indexed /path/to/python-app (545c3c86cca5f8f8284159cf11733ad8c1da8070)
  blobs: 3 indexed, 3 extracted, 0 cached, 0 stale, 0 unsupported
  graph: 55 nodes, 22 edges, 5 unresolved
  receiver self-cone: 1 sites (1 -> 1 edges), 0 dangling (0 edges dropped), 0 fallback (0 edges kept)
  receiver external-module: 0 sites (0 -> 0 edges), 3 dangling (1 edges dropped), 0 fallback (0 edges kept)
  receiver typed-local: 1 sites (2 -> 1 edges), 0 dangling (0 edges dropped), 0 fallback (0 edges kept)
  receiver legacy:local: 1 sites (0 -> 0 edges), 0 dangling (0 edges dropped), 0 fallback (0 edges kept)
  receiver lattice: 0 open:nonclass-base, 0 unknown-base, 0 fqn-collisions, 0 spec-ambiguous, 0 spec-shadowed, 0 floating-classes
  dataflow: 8 fns recomputed, 0 fns reused; ifds: 0 summaries, 0 interproc edges, 0 budget-exceeded SCCs
```

Two edges went: the `typed-local` rule cut `store.save` from two candidates to one, and the `external-module` rule dropped the edge on a call through an out-of-repo module. With `CGX_PRECISION_TRACE=1` the same run writes, among others:

```
recv-local	app.py:21	store.save	2->1	storage::FileStore::save
dangle:external-module	storage.py:51	subprocess.run	1->0
```

### Index with SCIP enrichment

```
rust-analyzer scip --output-file ra.scip
cgx index . --scip ra.scip
```

The SCIP pass upgrades call-edge confidence where `rust-analyzer` provides precise resolution. Call edges resolved to a single target are promoted to `certain`; name-matched edges with possible ambiguity become `probable`. Without `--scip`, all edges remain at Phase-1 confidence.

For C++ with scip-clang, pass the compdb the index was built from so promotions can reach `certain`:

```
cgx index . --scip index.scip --compdb build/compile_commands.json
```

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Index written successfully. |
| `2` | Bad invocation — an unknown flag such as `--repo`. |
| `3` | Indexing failed. No git repository found at or above `PATH` (`cgx: indexing failed: git error: Could not find a git repository in …` — `cgx index` requires the target to be inside a git working tree), or the `--scip` file could not be read or parsed (`cgx: indexing failed: scip error: reading SCIP index "…": No such file or directory`). |

## See also

- [cgx doctor](doctor.md) — verify and report on the quality of an existing index
- [cgx push](push.md), [cgx pull](pull.md) — share a built index through a git remote
- [cgx diff](diff.md) — diff the call graph between two git refs (uses the same `.cgx/` store)
- [cgx callers](callers.md), [cgx callees](callees.md) — traverse the call graph after indexing
- [cgx flows-to](flows-to.md), [cgx flows-from](flows-from.md) — dataflow traversal (requires index with DATA_FLOW layer)
- [docs/06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — incremental indexing, blob-OID caching, branch-aware sharing
