# cgx CLI — Command Reference

cgx provides 14 subcommands organised into five groups. Each command reads from a `.cgx/` index built by `cgx index`.

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
| `cgx unused` | Symbols not reachable from any indexed entrypoint | [unused.md](unused.md) |

### Query

| Command | Purpose | Reference |
|---------|---------|-----------|
| `cgx query` | Run a Layer-2 CQL (Cypher-subset) query against the call graph or dataflow graph | [query.md](query.md) |

### Server

| Command | Purpose | Reference |
|---------|---------|-----------|
| `cgx mcp` | Start the MCP STDIO server and expose graph tools to AI agents and IDE extensions | [mcp.md](mcp.md) |

---

## Reproducing the examples

All command reference examples query the same corpus.

**Corpus:** `fixtures/rust-sample` — a multi-file Rust crate included in the repository.

**Index the corpus** (run once from the worktree root):

```bash
cgx index fixtures/rust-sample
```

Or with an explicit path from any directory:

```bash
cgx index /path/to/worktree/fixtures/rust-sample
```

Every example then passes `--repo /path/to/worktree/fixtures/rust-sample` to point at the indexed directory.

The corpus package name is `rust-sample`; all fixture FQNs carry the `rust_sample::` prefix. Use `cgx search rust_sample` to list all indexed symbols.
