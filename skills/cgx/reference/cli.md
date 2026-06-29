# cgx CLI — Quick Reference

**Audience:** Engineers and AI agents driving cgx from the command line.

**Authoritative, comprehensive reference:** [`docs/commands/`](../../../docs/commands/)
and its [`README`](../../../docs/commands/README.md) (14 command pages with full flag tables,
output samples, and exit-code details).

This file is a **cheat-sheet only** — one line per subcommand and the flags you reach for
most. Look up edge cases in `docs/commands/`.

---

## All 14 subcommands

| Subcommand | One-job summary | Most-used flags |
|---|---|---|
| `index` | Analyze source and write the call+dataflow graph to `.cgx/` | `--no-dataflow` |
| `callers` | Symbols that (transitively) call a named symbol | `--depth`, `--format`, `--confidence` |
| `callees` | Symbols that a named symbol (transitively) calls | `--depth`, `--format`, `--confidence` |
| `flows-to` | Forward data-flow slice from a value node (v0.3) | `--depth`, `--confidence`, `--repo` |
| `flows-from` | Backward data-flow pedigree from a value node (v0.3) | `--depth`, `--confidence`, `--repo` |
| `reaches` | Boolean reachability check with witness path, or full reachable-symbol enumeration | `--depth`, `--format`, `--confidence` |
| `paths` | Enumerate every distinct call path from one symbol to another | `--depth` (default 6), `--format` |
| `explain` | Full provenance for one symbol: definition, edge counts, all incident edges | `--format`, `--repo` |
| `query` | Run a CQL (Cypher-subset) expression against the call or dataflow graph | `--at`, `--depth`, `--format` |
| `search` | Find symbols by FQN substring or regex; no graph walk | `--kind`, `--regex`, `--limit` |
| `unused` | Symbols not reachable from any indexed entrypoint | `--kind`, `--format` |
| `doctor` | Report on the quality and trust level of the current on-disk index | `--format` |
| `diff` | Diff the call graph between two git refs | `--newer-than`, `--format` |
| `mcp` | Start the MCP STDIO server for AI agents and IDE extensions | `--root` |

---

## Global gotchas

### Dataflow is ON by default

A plain `cgx index .` builds the full dataflow layer (SSA value nodes + `derives-from`
edges). Pass `--no-dataflow` to produce a lighter, CALLS-only index. There is **no**
`--dataflow` flag — the flag to know is `--no-dataflow`.

### Depth flag is always `--depth`

The depth limit is `--depth` on every CLI subcommand. `--max-depth` does **not** exist on
the CLI (the MCP `paths` tool uses the JSON property `max_depth`, but that is MCP-only).

Default depth by command group:
- `callers`, `callees`, `reaches`, `flows-to`, `flows-from`: **2**
- `paths`, `query`: **6**
- `--depth 0` on any command = unlimited (work-budgeted; may report `[truncated]`)

### Phantom flags — never emit these

These look plausible but cause exit 2 in the real binary:

`--max-depth`, `--base`, `--head`, `--from`, `--to`, `--from-class`, `--to-class`,
`--avoiding`, `--only-edge-condition`, `--dataflow`, `--calls-to-sink-class`,
`--kind fn` (correct: `--kind function`), a trailing `./` positional in place of
`--repo ./`.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success (including empty results) |
| 1 | `--assert-empty` triggered: results were found |
| 2 | Bad symbol / CQL parse or plan error / bad flag value |
| 3 | Index missing + `--no-auto-index` |
| 4 | `--assert-empty` vacuous: confidence filter excluded all results (suppress with `--allow-vacuous`) |

### CQL deferred clauses (exit 2 today)

Taint properties (`source_class`, `sink_class`, `sanitizer_class`, `taint_label`) and
the keywords `MUST PASS THROUGH` / `AVOIDING` are not implemented — they produce a plan
or parse error (exit 2). `:CALLS` and `:DATA_FLOW` edges work. Node properties `name`,
`kind`, `file`, `line`, edge properties `condition`, `confidence` work.

---

## Verified one-liners (corpus: `fixtures/rust-sample`)

```bash
# Index — full dataflow (default)
cgx index fixtures/rust-sample

# Callers — depth 2, human forest
cgx callers rust_sample::conditions::dispatch --repo fixtures/rust-sample

# Callees
cgx callees rust_sample::conditions::dispatch --repo fixtures/rust-sample

# Paths between two symbols
cgx paths rust_sample::conditions::dispatch rust_sample::conditions::log_error \
  --repo fixtures/rust-sample

# Reaches — witness path
cgx reaches rust_sample::conditions::dispatch rust_sample::conditions::log_error \
  --repo fixtures/rust-sample

# Explain a symbol
cgx explain rust_sample::conditions::dispatch --repo fixtures/rust-sample

# Search by substring
cgx search dispatch --repo fixtures/rust-sample --limit 5

# Unused functions
cgx unused --kind function --repo fixtures/rust-sample

# CQL query
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.fqn = "rust_sample::conditions::dispatch" RETURN a.name, a.file LIMIT 5' \
  --repo fixtures/rust-sample

# Dataflow — forward slice from a value node
# (requires an index built with dataflow: cgx index <path>)
cgx flows-to "rust_sample::dataflow::flow_example::b#1" --repo <path-indexed-with-dataflow>

# Dataflow — backward pedigree
cgx flows-from "rust_sample::dataflow::flow_example::b#1" --repo <path-indexed-with-dataflow>

# Index health
cgx doctor --repo fixtures/rust-sample
```

> **`flows-to` / `flows-from` require value-node FQNs** (form: `<fn>::<local>#<ver>`),
> not function symbols. Use `cgx search <name> --kind variable` to find the exact FQN.
> The index must have been built without `--no-dataflow` (the default); on a `--no-dataflow`
> index, value nodes are absent and the symbol does not resolve (exit 2).

---

## See also

- [`docs/commands/`](../../../docs/commands/) — per-command pages with full flag tables
- `reference/query-language.md` — supported and unsupported CQL clauses
- `reference/output-and-exit.md` — format options, exit-code contract, CI assertion mode
- `reference/mcp.md` — MCP tool schemas, MCP-vs-CLI defaults, pagination
- `reference/versions.md` — version ladder and capability since-tags
