# cgx CLI — Command Reference

cgx provides 16 subcommands organised into six groups. Most read from a `.cgx/` index built by `cgx index`; `cgx coupling` reads committed git history only and needs no index.

## Commands

### Indexing

| Command | Purpose | Reference |
|---------|---------|-----------|
| `cgx index` | Analyze source files and write the call graph (and dataflow layer) to `.cgx/` | [index.md](index.md) |
| `cgx doctor` | Report on the quality and trust level of the current on-disk index | [doctor.md](doctor.md) |
| `cgx diff` | Diff the call graph between two git refs | [diff.md](diff.md) |

### Reachability

| Command | Purpose | Reference |
|---------|---------|-----------|
| `cgx callers` | Symbols that (transitively) call a named symbol | [callers.md](callers.md) |
| `cgx callees` | Symbols that a named symbol (transitively) calls | [callees.md](callees.md) |
| `cgx reaches` | Boolean reachability check with a witness path, or full reachable-symbol enumeration | [reaches.md](reaches.md) |
| `cgx paths` | Enumerate every distinct call path from one symbol to another | [paths.md](paths.md) |

### Dataflow

| Command | Purpose | Reference |
|---------|---------|-----------|
| `cgx flows-to` | Forward data-flow slice: values a named SSA node flows into | [flows-to.md](flows-to.md) |
| `cgx flows-from` | Backward data-flow pedigree: values a named SSA node derives from | [flows-from.md](flows-from.md) |

### Introspection

| Command | Purpose | Reference |
|---------|---------|-----------|
| `cgx explain` | Full provenance for one symbol: definition, caller/callee counts, all incident edges | [explain.md](explain.md) |
| `cgx search` | Search the symbol table by FQN substring or regex | [search.md](search.md) |
| `cgx symbols` | Rank symbols by degree, with a per-symbol edge breakdown | [symbols.md](symbols.md) |
| `cgx unused` | Symbols not reachable from any indexed entrypoint | [unused.md](unused.md) |
| `cgx coupling` | Which files historically change together across a commit range | [coupling.md](coupling.md) |

### Query

| Command | Purpose | Reference |
|---------|---------|-----------|
| `cgx query` | Run a Layer-2 CQL (Cypher-subset) query against the call graph or dataflow graph | [query.md](query.md) |

### Server

| Command | Purpose | Reference |
|---------|---------|-----------|
| `cgx mcp` | Start the MCP STDIO server and expose graph tools to AI agents and IDE extensions | [mcp.md](mcp.md) |

---

## Reading an answer

A graph traversal answers two questions besides the one you asked: *how approximate is this answer* and *how stale is the index it came from*. Where a command can answer them, the answer travels with the result rather than in a separate report, in whichever format you asked for.

```
cgx callers rust_sample::conditions::dispatch --repo /path/to/worktree
```

```
rust_sample::conditions::dispatch  fixtures/rust-sample/src/conditions.rs:42
├─ ts_sample::closures::closureVariable  fixtures/ts-sample/src/closures.ts:6  [possible]
...
approximation: over- and under-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur; external/unindexed callees not modeled (no SCIP) (6 site(s) on the searched frontier)
freshness: current | indexed tree 5ea331d, working tree clean
```

*(tree body elided — the full output is in [callers.md](callers.md).)*

- **`approximation:`** — the approximation contract: `exact (within modeled graph)`, `under-approximate`, `over-approximate`, or both, plus the reasons. On an absence answer (empty result, `--assert-empty`) it also carries a `| scope: …` tail naming the edge family, confidence floor, and depth the search actually covered, so a "nothing found" answer states what it looked at.
- **`freshness:`** — `current`, `stale`, or `unknown`, the indexed tree, and how far the working tree has drifted from it. The verdict reads `current` only when both halves were established clean; anything uninspected reads `unknown`, never `current`.

In `--format json` the same two objects appear as top-level `approximation` and `freshness` keys; in `--format sarif` they appear as `cgx/approximation-contract` and `cgx/index-freshness` `note` results.

Not every command carries both, and which it carries follows from what it computes rather than from when it was written:

| Command | `approximation` | `freshness` |
|---------|-----------------|-------------|
| `callers`, `callees`, `flows-to`, `flows-from`, `reaches`, `paths`, `unused`, `query` | yes | yes |
| `search`, `symbols`, `explain` | no — a table scan or a single symbol's incident edges, with no traversal to approximate | yes |
| `doctor` | no — the report *is* an index-quality verdict | no |
| `diff` | no — a graph-to-graph delta, not a query answer | no |
| `coupling` | yes — but scoped to a *history* model, not the call graph: co-change is file-level and the rev range is bounded | no — it reads committed git history and never opens the index, so there is no index freshness to report |

`dot`, `mermaid`, and `d2` output carries neither: those formats render a graph, and computing freshness would mean walking the working tree for a renderer that has nowhere to print it.

## Reproducing the examples

All command reference examples query the same corpus: **the cgx worktree itself**, indexed at its root. That covers the crate sources under `crates/` and both language fixtures under `fixtures/` in one graph, which is what the shown output was captured from — file paths in every example are worktree-relative (`fixtures/rust-sample/src/conditions.rs:42`, `crates/cgx-cli/src/forest.rs:345`).

**Index the corpus** (run once, from the worktree root):

```bash
cgx index .
```

Every example then passes `--repo /path/to/worktree` to point at it.

Indexing `fixtures/rust-sample` alone is not enough. Several examples — the `callers` and `explain` trees in particular — show TypeScript callers of Rust symbols, which only exist when `fixtures/ts-sample` is in the same graph. A rust-sample-only index reproduces those invocations with zero callers and no error.

The fixture package names are `rust-sample` and `ts-sample`; their FQNs carry the `rust_sample::` and `ts_sample::` prefixes. Use `cgx search rust_sample` to list the Rust fixture's symbols, or [`cgx symbols`](symbols.md) to rank the whole corpus by degree.

**Counts move.** Examples that report graph-wide totals — `doctor`'s node and edge counts, `unused`'s result count — were captured against one commit of this worktree and scale with it. Your numbers will differ; the shape, field names, and verdicts are what the examples are documenting.
