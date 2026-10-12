# cgx Interfaces

**Status:** Draft  
**Audience:** Software engineers, security engineers, AI coding agents, CI operators  
**Cross-references:** `docs/05-queries.md` (query capabilities, feature IDs Q-), `docs/03-code-graph-model.md` (GM- features, edge condition labels), `docs/04-dataflow-and-provenance.md` (DF- features)

---

## Overview

`cgx` exposes two interfaces:

1. **CLI** — a human-friendly and script-friendly command-line tool. TTY-aware defaults. Composable with standard Unix tools.
2. **MCP STDIO server** — a Machine-readable Control Protocol server over standard input/output. Designed for AI coding and security agents. Conforms to the 2025-06-18 MCP specification.

A TUI (terminal user interface) is explicitly out of scope for v1.

---

## Feature List

Shipped status reflects v0.3.0. Features without a "Planned" note are available in the current binary.

| ID | Feature | v0.3.0 status |
|----|---------|---------------|
| IF-1 | CLI entry point: `cgx <subcommand> [flags]`, repository via `--repo` | Shipped |
| IF-2 | TTY-aware output: text on TTY, jsonl when piped | **Not shipped, and not planned in this form** — `human` is the default on every stream; there is no TTY detection and no `--color` flag |
| IF-3 | `--format` flag: `human`, `json`, `sarif`, `dot`, `mermaid`, `d2` | Shipped (six values; `text`/`tree`/`jsonl`/`csv`/`graphml` do not exist) |
| IF-4 | Exit-code contract (0/1/2/3/4) | Shipped |
| IF-5 | CI assertion mode: `--assert-empty` | Shipped |
| IF-5a | CI assertion mode: `--assert-count N`, `--assert-max N` | **Planned (not yet shipped in v0.3.0)** |
| IF-6 | Per-symbol provenance | Shipped as the `explain` subcommand (and MCP tool); there is no `--explain` flag |
| IF-7 | `--at <ref>` flag: query against a specific commit | Shipped |
| IF-8 | `--order` flag: stable result ordering | **Planned (not yet shipped in v0.3.0)** — engine already returns rows in a fixed deterministic order; no CLI flag exists |
| IF-9 | MCP STDIO server mode: `cgx mcp` | Shipped |
| IF-10 | MCP tool: `graph_query` | **Shipped** — routes to the same CQL engine as `cgx query`; it is not a stub and does not error |
| IF-11 | MCP tool: `callers` | Shipped |
| IF-12 | MCP tool: `callees` | Shipped |
| IF-13 | MCP tool: `paths` | Shipped |
| IF-14 | MCP tool: `unused` | Shipped |
| IF-15 | MCP tool: `explain` | Shipped |
| IF-15a | MCP tools: `reaches`, `search`, `symbols`, `flows_to`, `flows_from`, `coupling`, `impacted_tests` | Shipped |
| IF-16 | MCP resources: `cgx://symbols/{root}`, `cgx://schema/{root}` | **Planned** — `resources/list` returns `method not found` |
| IF-17 | MCP structured output: `structuredContent` per 2025-06-18 spec | **Shipped** — every tool result carries `structuredContent` alongside the `content` text mirror. A declared `outputSchema` is still Planned |
| IF-18 | MCP pagination: `cursor` + `has_more` | **Shipped** — both keys are present on every list-shaped tool result; `explain`, `reaches` with `to`, and `coupling` return a single document without them (see IF-18) |
| IF-19 | MCP token-efficiency: `max_results` | **Shipped** for `max_results` / `total_matched`. Compact symbol IDs and `resource_link` bulk evidence are Planned |
| IF-20 | `paths` subcommand: must-analysis flags (`--must-pass-through`, `--avoiding`, `--quantifier`, `--including-exception-paths`, `--assert-all-reach-sink`) | **Planned (not yet shipped in v0.3.0)** |
| IF-21 | `paths` subcommand: typed-taint flags (`--from-class`, `--to-class`, `--require-sanitizer-class`, `--negate-sanitizer`, `--to-package`) | **Planned (not yet shipped in v0.3.0)** |
| IF-22 | `diff` subcommand: security-gate flags (`--new-paths-only`, `--calls-to-sink-class`) | **Planned (not yet shipped in v0.3.0)** |
| IF-23 | MCP tool: `taint_paths` | **Planned (not yet shipped in v0.3.0)** |
| IF-24 | MCP tool: `diff_security` | **Planned (not yet shipped in v0.3.0)** |

---

## Part 1: CLI Interface

### IF-1: Entry point

```
cgx <subcommand> [ARGS] [--repo <path>] [flags]
```

**The repository is an option, not a trailing positional.** `--repo <path>` names the repository root or any subdirectory and defaults to the current directory; a trailing positional path is a clap parse error, exit 2. Three exceptions: `cgx index` takes a **positional** `[PATH]` and rejects `--repo`; `cgx configure-remote` has no `--repo` and acts on the repository containing the current directory; and `cgx mcp` takes `--root`, with each tool call naming its own `root`. `cgx` locates the graph index by walking up from the resolved root (see `docs/06-indexing-and-vcs.md`).

**Top-level subcommands — twenty-two, shipped in v0.3.0:**

| Subcommand | Description |
|------------|-------------|
| `index` | Build or update the graph index (dataflow on by default; use `--no-dataflow` to disable) |
| `callers` | Find symbols that call a given symbol |
| `callees` | Find symbols a given symbol calls |
| `reaches` | Whether one symbol reaches another (with a witness path) |
| `paths` | Find call paths between two symbols |
| `flows-to` | Forward data-flow walk from a value node (`Since: v0.3`) |
| `flows-from` | Backward data-flow pedigree from a value node (`Since: v0.3`) |
| `unused` | Find symbols not reachable from any entrypoint |
| `explain` | Show full provenance record for a symbol |
| `diff` | Compute graph diff between two git refs (`<BASE> <HEAD>` positional), with `--added/--removed/--changed`, `--kind`, `--edge-condition`, and the `--path-added` gate |
| `coupling` | File-level co-change frequency over a git rev range (`<BASE> <HEAD>` positional). **Needs no index** |
| `query` | Execute a full CQL query-language expression |
| `search` | Search the symbol table by FQN substring or regex (`Since: v0.2`) |
| `symbols` | Rank symbols by reference count, with a per-symbol edge breakdown |
| `impacted-tests` | Test entrypoints whose call graph reaches a symbol changed between two refs (`<BASE> [<HEAD>]` positional) or in the uncommitted working tree |
| `pack` | Generate an agent-context pack of JSON cards over the graph (`--json`) |
| `doctor` | Report on the quality of the current on-disk index. **Does not auto-index** — exit 3 on an unindexed repo |
| `dump` | Dump the current on-disk graph (nodes, edges, candidates) for inspection. **Does not auto-index** — exit 3 on an unindexed repo |
| `push` | Publish the local `.cgx` committable index under `refs/cgx/index` and push it to a remote |
| `pull` | Fetch `refs/cgx/index` from a remote and materialize `.cgx/` locally |
| `configure-remote` | Configure a remote so `refs/cgx/*` round-trips through plain `git fetch`/`git push` |
| `mcp` | Start MCP STDIO server |

**Planned subcommands (not yet shipped in v0.3.0):**

| Subcommand | Description |
|------------|-------------|
| `prune` | Remove stale index entries (branches deleted, files removed) — **Planned (v0.4)** |
| `policy check` | Evaluate `.cgx/policy.cql` assertions — **Planned (v0.5)** |

> **Note:** `pedigree` was a working name for the value-provenance subcommand. The shipped subcommands are `flows-to` (forward) and `flows-from` (backward pedigree).

See `docs/05-queries.md` for complete per-subcommand signatures and worked examples.

### IF-2: TTY-aware defaults — **Not shipped**

The design below was not built, and the shipped behaviour is deliberately simpler: **`--format human` is the default on every stream**, TTY or pipe. There is no `IsTerminal` check anywhere in `cgx-cli`, no `--color` flag, and no ANSI colour in any output path. A script that pipes `cgx` and expects machine-readable output must pass `--format json` explicitly.

That is the right default for a tool whose identity property is byte-identical repeat output: a format that changes depending on whether a terminal is attached is a format that is not reproducible.

*(Original design, retained for the record: `text` on a TTY, `jsonl` when piped, with `--color always|never|auto`. `jsonl` is not a format cgx has.)*

### IF-3: `--format` flag

Fifteen subcommands accept this six-value `--format`; `index`, `mcp`, `push`, `pull`, `configure-remote` and `pack` do not (`pack` has `--json` instead), and `dump` has its own `--format text|json`. The format applies to the result stream; progress and error output always go to stderr.

**The enum has exactly six values:**

| Format | Description | Primary use case |
|--------|-------------|-----------------|
| `human` | Column-aligned human-readable text with file:line, or an indented ASCII forest for traversals | Interactive terminal (default) |
| `json` | One pretty-printed JSON document | Scripting, debugging |
| `sarif` | SARIF 2.1.0 (OASIS standard) | GitHub Advanced Security, VS Code SARIF viewer |
| `dot` | Graphviz DOT source (`digraph`) | CI pipelines, headless SVG generation |
| `mermaid` | Mermaid flowchart source (`graph TD`) | GitHub/Notion docs, README graphs |
| `d2` | D2 (d2lang) source | Diagram-as-code toolchains |

`text`, `tree`, `jsonl`, `csv` and `graphml` do not exist. The `tree` rendering is not a separate format — it is what `human` produces for `callers`/`callees`/`reaches`/`flows-*`, controlled by `--tree full|spanning`.

> **`--help` is not the accept-list for `--format`.** clap's `ValueEnum` prints all six values on every command that has the flag, including commands that reject four of them at runtime. The real answer is four different enforcement regimes:
>
> | Regime | Commands | Accepted | Rejected → exit 2 |
> |---|---|---|---|
> | **A** — full six, graph formats gated by result shape | `callers`, `callees`, `flows-to`, `flows-from`, `reaches`, `paths`, `unused`, `query`, `impacted-tests` | `human`/`json`/`sarif` always; `dot`/`mermaid`/`d2` **only when the result is path-shaped** | `dot`/`mermaid`/`d2` on a non-path result |
> | **B** — allow-list `human\|json` | `search`, `symbols`, `coupling` | `human`, `json` | `sarif`, `dot`, `mermaid`, `d2` |
> | **C** — allow-list per diff mode | `diff` | full mode `human`/`json`; `--path-added` adds `dot`/`mermaid`/`d2` | everything else, checked before any indexing |
> | **D** — no gate, silent fallthrough to human, **exit 0** | `explain`, `doctor` | `json` renders JSON | *nothing* — every other value renders human text |
>
> Regime D is a known defect of the same class already fixed for `diff`: `cgx doctor --format sarif` gives you human text and a successful exit, not SARIF. Do not build a CI step on it.
>
> Path-shaped results (the regime-A gate) are: `paths` always, `reaches <FROM> <TO>`, and `query` when the CQL is `RETURN path`. `callers`, `callees`, `reaches <FROM>`, `flows-*`, `unused`, `impacted-tests` and tabular `query` are never path-shaped.

**`dot` vs `mermaid` guidance:**
- Use `--format mermaid` for documentation embedded in GitHub Markdown or Notion. Mermaid renders inline in both. Human-writeable and diff-friendly.
- Use `--format dot` for CI pipelines that generate SVG or PNG artifacts (`dot -Tsvg`), for large graphs (Mermaid has a node limit), and for algorithmic graph layout tools.

**`sarif` note:**
SARIF 2.1.0 maps each result to a `physicalLocation` (file:line), a `logicalLocation` (qualified name), and a rule ID. GitHub Actions uploads SARIF to the Security tab with inline annotations. Exit code 1 when `--assert-empty` fires makes SARIF upload and build failure composable.

**Tree format example:**

```
main::process_request
├─ auth::validate_token  (src/auth.rs:42)   [always] [certain]
│  └─ crypto::hash       (vendor/crypto.rs:7)  [always] [certain]
└─ db::query             (src/db.rs:15)     [always] [probable]
   └─ sql::execute       (vendor/sql.rs:88) [conditional] [probable]
```

### IF-4: Exit-code contract

| Code | Meaning |
|------|---------|
| 0 | Query completed; results returned (or zero results when that is expected) |
| 1 | CI assertion failed: `--assert-empty` fired (results found when none expected) |
| 2 | Query parse error or invalid flag combination |
| 3 | Graph error: index missing, corrupt, or build failed |
| 4 | Assertion vacuously satisfied: the gate passed but matched zero symbols or filters excluded all candidate results; see IF-5. Suppress with `--allow-vacuous`. |

Exit code 1 is the CI-gate signal. Exit codes 2 and 3 indicate tool or configuration problems, not result conditions. Exit code 4 indicates a vacuous pass — the assertion matched nothing, which is itself a warning condition (see IF-5).

> **Note:** `--assert-count N` and `--assert-max N` are **Planned (not yet shipped in v0.3.0)**. Only `--assert-empty` is available today.

### IF-5: CI assertion mode

**Shipped in v0.3.0:** `--assert-empty` turns a query into a boolean CI check (exit 1 when results are found).

**Planned (not yet shipped in v0.3.0):** `--assert-count N` and `--assert-max N`.

```bash
# Fail CI if any call path reaches a dangerous sink (shipped)
cgx paths '**' dangerous::sink --repo . \
    --assert-empty \
    --format sarif > security.sarif
# Exit 1 if paths found; exit 0 if none

# Fail CI if more than 5 paths reach the sink (PLANNED — not yet shipped)
# cgx paths '**' dangerous::sink --repo . --assert-max 5

# Fail CI if the number of public unused methods changes (PLANNED — not yet shipped)
# cgx unused ./ --kind method --confidence certain --assert-count 0
```

`--assert-empty` composes with any subcommand. When it fires, output is still written to stdout; the exit code signals the assertion result.

#### Vacuity guard

**Definition (vacuous pass):** an `--assert-empty` assertion passed AND at least one of:

- **(a) Zero-symbol match:** the `--from`/`--to`/symbol pattern resolved to zero matching symbols.
- **(b) Filter exclusion:** confidence/edge-condition filters excluded every result the same query returns unfiltered (unfiltered count > 0, filtered count = 0). The unfiltered count is computed by re-running the (cheap, linear) reachability/cut test without the filters — never by path enumeration.

**Default behavior:** a vacuous pass exits with **code 4** ("assertion vacuously satisfied") unless `--allow-vacuous` is given. With `--allow-vacuous`, the tool exits 0 and emits a stderr warning; JSON/`structuredContent` output includes `"vacuous": true`; SARIF output carries a `note`-level result.

**`--assert-count N` / `--assert-max N` (Planned — not yet shipped in v0.3.0):** receive clause (a) only (zero-symbol match is always suspicious); clause (b) does not apply to count assertions.

**Warning (confidence narrowing):** confidence filters such as `--confidence certain` narrow the assertion gate toward false safety. Before SCIP enrichment (Phase 2), almost no edge is labeled `certain`, so an `--assert-empty --confidence certain` gate passes vacuously on any pre-SCIP corpus — exit code 4 catches this. See docs/07-interfaces.md IF-9 and docs/05-queries.md Q-18.

**GitHub Actions integration example:**

```yaml
- name: Security regression gate
  run: |
    cgx paths '**' vulnerable::bar --repo . \
        --assert-empty --format sarif > findings.sarif
  continue-on-error: false

- name: Upload SARIF
  uses: github/codeql-action/upload-sarif@v3
  with:
    sarif_file: findings.sarif
```

### IF-6: Per-symbol provenance — the `explain` subcommand

There is no `--explain` flag; passing one is a clap parse error, exit 2. Provenance is a subcommand of its own: `cgx explain <SYMBOL>` prints the symbol's definition, its caller and callee counts, and every direct incident edge with its condition, confidence, resolution tier, resolution rule and call site:

```
$ cgx explain rust_sample::direct::chain --repo .
rust_sample::direct::chain  (src/direct.rs:47)
  kind: function
  callers: 1, callees: 3
  edges:
    <- rust_sample::imports::utils::run_chain  (src/imports.rs:41)  [always]  [probable]  tier=scope_graph  rule=import-ref  site=src/imports.rs:41
    -> rust_sample::direct::step_a  (src/direct.rs:53)  [always]  [certain]  tier=scope_graph  rule=scope-ref  site=src/direct.rs:47
    -> rust_sample::direct::step_b  (src/direct.rs:54)  [always]  [certain]  tier=scope_graph  rule=scope-ref  site=src/direct.rs:47
    -> rust_sample::direct::step_c  (src/direct.rs:55)  [always]  [certain]  tier=scope_graph  rule=scope-ref  site=src/direct.rs:47
freshness: current | indexed tree c19123d, working tree clean
```

`--format json` returns the same record as a document; the MCP `explain` tool returns it as `structuredContent` (IF-15). For traversal results, provenance is the per-result `file`, `line`, `edge_condition` and `confidence` fields in `--format json`.

### IF-7: `--at <ref>` flag

The eight subcommands sharing the query flag block — `callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `query`, `unused` — accept `--at <ref>` to query the graph as it existed at a specific commit. `explain`, `search`, `symbols`, `doctor`, `diff`, `coupling`, `dump`, `pack` and the transport commands do **not**, and no MCP tool does. `impacted-tests` lists `--at` in `--help` (it shares the flag block) but rejects it at runtime with exit 2 — pass the refs positionally instead:

```bash
cgx callers crypto::hash --repo . --at abc1234
cgx callers crypto::hash --repo . --at HEAD~5
cgx paths main::foo vulnerable::bar --repo . --at v1.2.3
```

`--at` enables: "Did this path exist before this PR?" and "Which commit introduced this edge?" The `diff` subcommand takes `<BASE> <HEAD>` as positional arguments rather than `--at`; see Q-7 in `docs/05-queries.md`.

### IF-8: `--order` flag — **Planned (not yet shipped in v0.3.0)**

There is no `--order` CLI flag today. What *is* shipped is an internal determinism invariant: the engine always returns rows in a fixed `(file_path, line, col)` order, stable across runs on identical graph data. The flag below is a Planned surface for selecting alternate orderings; it does not exist yet.

```
--order file        (default) Sort by (file_path, line, col) — deterministic   [Planned]
--order alpha       Sort by qualified symbol name                              [Planned]
--order discovery   BFS/DFS traversal order from the query root                [Planned]
--order relevance   Depth-ascending from the query root                        [Planned]
```

The default `file` ordering described above is the current shipped behavior even without a flag. See `docs/05-queries.md` Q-17 for the full determinism contract.

---

## Part 2: MCP STDIO Server

### IF-9: Starting the MCP server

```bash
cgx mcp [--root <path>]
```

`cgx mcp` reads from stdin and writes to stdout using the MCP 2025-06-18 STDIO transport. It speaks NDJSON (newline-delimited JSON). The MCP server is single-process, single-client; for multi-client use, wrap it in an MCP proxy (future work, v2).

**Recommended v1 transport:** STDIO. SSE/HTTP transport is a v2 feature. STDIO works in Claude Code, Cursor, Continue.dev, and any MCP-compatible agent harness without network configuration.

All graph-querying MCP tools accept `include_dirty: true` (the MCP default) to analyze uncommitted working-tree changes via a per-call content-addressed overlay that is never persisted to the index. There is no CLI `--include-dirty` flag. See docs/06-indexing-and-vcs.md IX-3 for the overlay lifetime and memoization semantics.

Configuration in Claude Code (`.mcp.json` or project settings):

```json
{
  "cgx": {
    "command": "cgx",
    "args": ["mcp"],
    "cwd": "/path/to/project"
  }
}
```

### MCP tool surface

**Thirteen tools are registered.** `tools/list` returns them in the order `callers`, `callees`, `reaches`, `paths`, `unused`, `explain`, `search`, `symbols`, `flows_to`, `flows_from`, `graph_query`, `coupling`, `impacted_tests`, with these required arguments:

| Tool | Required arguments |
|---|---|
| `callers`, `callees`, `explain`, `flows_to`, `flows_from` | `symbol`, `root` |
| `reaches` | `from`, `root` (`to` optional — omitting it switches to forward enumeration) |
| `paths` | `from`, `to`, `root` |
| `unused`, `search`, `symbols` | `root` |
| `graph_query` | `query`, `root` |
| `coupling` | `root`, `base`, `head` |
| `impacted_tests` | `root` (`base` optional — omitting it compares the working tree to `HEAD`) |

`root` is required on every tool: the MCP server is addressed per call, not per session.

**Still Planned:** MCP resources (IF-16), a declared `outputSchema` (IF-17), and the tools `taint_paths` (IF-23) and `diff_security` (IF-24).

**The CLI surface is larger than the MCP surface.** `index`, `doctor`, `diff`, `dump`, `pack`, `push`, `pull`, `configure-remote` and `mcp` have no MCP equivalent. A statement that the MCP server "covers the same capability set as the CLI" is not true today.

#### The MCP response envelope

Every tool result carries the same outer shape, verified by a live call against the shared example repository defined in [`docs/05-queries.md` → "The example repository"](05-queries.md#the-example-repository) (its recipe reproduces tree OID `7380e245ab98db2dfed12ed3ee3b005f0fbf7036` exactly):

```json
{
  "content": [ { "type": "text", "text": "<the same document, serialized>" } ],
  "isError": false,
  "structuredContent": {
    "approximation": {
      "direction": "under",
      "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
      "reasons": [
        { "code": "depth-limit", "detail": "search stopped at depth 1; deeper edges were not explored", "direction": "under" }
      ]
    },
    "cursor": null,
    "dirty": false,
    "dirty_files_analyzed": 0,
    "freshness": {
      "dirty_files": 0,
      "dirty_files_base": "7380e245ab98db2dfed12ed3ee3b005f0fbf7036",
      "head_tree": "7380e245ab98db2dfed12ed3ee3b005f0fbf7036",
      "indexed_tree": "7380e245ab98db2dfed12ed3ee3b005f0fbf7036",
      "matches_head": true,
      "stale": false
    },
    "graph_version": "7380e24",
    "has_more": false,
    "results": [
      {
        "confidence": "certain", "depth": 1, "edge_condition": "always",
        "exception_transient": false, "file": "src/main.rs", "line": 15,
        "min_confidence_on_path": "certain", "name": "rust_sample::middle"
      }
    ],
    "symbol": "helper",
    "total_matched": 1
  }
}
```

Five things in that document are worth stating explicitly, because a doc that omits them misleads:

1. **`structuredContent` is shipped**, and it is the canonical result. The `content[0].text` entry is a serialization of the same document, kept for clients that only consume `TextContent`. A declared `outputSchema` is not shipped.
2. **`cursor`, `has_more` and `total_matched` are shipped** — pagination is real, driven by `max_results`.
3. **`matches_head` is three-valued (`true` / `false` / `null`); a client that treats it as a boolean is wrong.** In the run above the working tree was clean and had just been indexed, so the server served the stored committed graph: `dirty: false`, `matches_head: true`. `null`, with a freshness verdict of `unknown`, arises under the MCP default `include_dirty: true` when a gitignored or untracked source file is on disk: the working-tree walk (no ignore rules beyond skipping `.git` and the root `.cgx/`) sees it and the dirty-file count (ignore rules applied) does not, so the two disagree.
4. **`graph_version` exists on MCP and on no CLI format.** When an overlay is in play it takes the form `<indexed-tree>+dirty.<overlay-digest>`.
5. **`approximation` is not universal.** Ten of the thirteen tools carry it; `explain`, `search` and `symbols` omit it by design, and `coupling` carries `approximation` but no index `freshness` because it never opens the index. MCP *errors* carry neither.

> **On the CLI/MCP JSON breaking change:** `cgx search --format json` and `cgx symbols --format json` changed from a bare array to `{"results": […], "freshness": {…}}`. The element shape did not change, so a `jq '.[0]'` becomes `jq '.results[0]'`. **This affected the CLI only.** The MCP `search` and `symbols` tools have always returned an object and did not break — the `results` array has always been a field of the response document, as the live output above shows. Do not "port" a fix to MCP clients that never needed one.

#### IF-10: `graph_query` — **Shipped**

Execute a full query-language expression and return structured results. `graph_query` routes to the same `cgx-cql` engine that backs `cgx query`; it is not a stub and does not return an error.

```json
{
  "name": "graph_query",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query":         { "type": "string", "description": "Cypher-subset query expression" },
      "root":          { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true, "description": "Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted" },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" }
    },
    "required": ["query", "root"]
  }
}
```

That is the schema as `tools/list` reports it, verbatim. Note what is **absent**: there is no `at` parameter on `graph_query` (pinning to a ref is CLI-only, via `--at`) and no `format` parameter (the response document is the format).

**Response** (`structuredContent` per 2025-06-18 spec). The result is a table: `columns` names the `RETURN` projections and `rows` holds one array per match, in column order. A `RETURN path` query yields a paths channel instead. Live call against a clean, unindexed checkout of the Rust sample fixture, query `MATCH (a)-[:CALLS]->(b {name:"chain"}) RETURN a.name, a.file, a.line`:

```json
{
  "structuredContent": {
    "approximation": { "direction": "exact", "modeled_graph": "…", "reasons": [] },
    "columns": ["a.name", "a.file", "a.line"],
    "rows": [
      ["rust_sample::imports::utils::run_chain", "src/imports.rs", 41]
    ],
    "cursor": null,
    "dirty": false,
    "dirty_files_analyzed": 0,
    "freshness": {
      "dirty_files": 0,
      "dirty_files_base": "c19123dd915dcb04a2735de3426b640ac2f21738",
      "head_tree": "c19123dd915dcb04a2735de3426b640ac2f21738",
      "indexed_tree": "c19123dd915dcb04a2735de3426b640ac2f21738",
      "matches_head": true,
      "stale": false
    },
    "graph_version": "c19123d",
    "has_more": false,
    "total_matched": 1,
    "truncated": false,
    "truncation_reason": null
  }
}
```

When `include_dirty: true` and uncommitted changes are present, `graph_version` takes the form `"<tree-oid>+dirty.<overlay-digest>"` where `overlay-digest` is a deterministic hash of the sorted synthetic OIDs for the dirty files analyzed. `dirty` is `true` and `dirty_files_analyzed` reports the count of analyzed dirty files.

The `structuredContent` field is the canonical structured result. For backward compatibility with clients that only consume `TextContent`, the server also serializes the same data as the `content` array entry.

#### IF-11: `callers`

```json
{
  "name": "callers",
  "inputSchema": {
    "type": "object",
    "properties": {
      "symbol":          { "type": "string", "description": "Qualified symbol name" },
      "root":            { "type": "string", "description": "Repository root path" },
      "depth":           { "type": "integer", "default": 1 },
      "max_results":     { "type": "integer", "default": 20 },
      "cursor":          { "type": "string" },
      "edge_condition":  { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":      { "type": "string", "enum": ["certain","probable","possible"] },
      "kind":            { "type": "array", "items": { "type": "string", "enum": ["calls","calls_virtual","calls_closure","calls_callback","calls_async","calls_indirect","spawns"] },
                           "description": "Restrict traversal to these call-family edge kinds (default: all call edges)" },
      "max_candidates":  { "type": "integer", "minimum": 1, "description": "Drop call edges from an over-approximated candidate set larger than N targets (fan-out cap, orthogonal to confidence); excluded edges are reported in the approximation contract" },
      "include_dirty":   { "type": "boolean", "default": true, "description": "Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted" }
    },
    "required": ["symbol", "root"]
  }
}
```

**Example call:**

```json
{ "symbol": "helper", "root": "/workspace/myproject", "depth": 3, "confidence": "certain" }
```

**The two surfaces are not flag-for-flag equivalents, and MCP is richer in two places.** `callers`/`callees` over MCP accept `edge_condition` and `kind` filters that the CLI subcommands do not have; MCP `paths` accepts `exclude_edge_condition` and `only_edge_condition`, which on the CLI exist nowhere. Defaults differ too: MCP `callers` defaults to `depth: 1` where the CLI defaults to 2, and MCP `paths` defaults to `max_depth: 10` where the CLI defaults to 6. There is no `at` parameter on any tool — pinning to a ref is CLI-only.

Two rough edges in that surface, found by running it and not yet fixed: MCP `paths` accepts **both** `exclude_edge_condition` and `only_edge_condition` in one call and silently lets `only_edge_condition` win rather than rejecting the combination; and MCP `paths` with `max_depth: 0` returns nothing, where CLI `paths --depth 0` lifts the depth limit. Do not assume the two `0`s mean the same thing — and note that the CLI's `0` is not "unbounded" either: it moves the bound to the shared work budget, which can truncate the answer to fewer results than a bounded depth returned (see `docs/05-queries.md` Q-3).

#### IF-12: `callees`

Same schema as `callers` with identical parameters. Returns symbols the named symbol calls, not symbols that call it.

#### IF-13: `paths`

```json
{
  "name": "paths",
  "inputSchema": {
    "type": "object",
    "properties": {
      "from":                    { "type": "string" },
      "to":                      { "type": "string" },
      "root":                    { "type": "string" },
      "max_depth":               { "type": "integer", "default": 10 },
      "max_results":             { "type": "integer", "default": 10 },
      "exclude_edge_condition":  { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "only_edge_condition":     { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":              { "type": "string", "enum": ["certain","probable","possible"] },
      "max_candidates":          { "type": "integer", "minimum": 1 },
      "cursor":                  { "type": "string" },
      "include_dirty":           { "type": "boolean", "default": true, "description": "Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted" }
    },
    "required": ["from", "to", "root"]
  }
}
```

**Example call:**

```json
{
  "from": "main::foo",
  "to": "vulnerable::bar",
  "root": "/workspace/myproject",
  "exclude_edge_condition": "exception",
  "max_results": 5
}
```

#### IF-14: `unused`

```json
{
  "name": "unused",
  "inputSchema": {
    "type": "object",
    "properties": {
      "root":           { "type": "string" },
      "kind":           { "type": "string", "enum": ["function","method","field","all"], "default": "all" },
      "entrypoint":     { "type": "string" },
      "max_results":    { "type": "integer", "default": 20 },
      "cursor":         { "type": "string" },
      "include_dirty":  { "type": "boolean", "default": true, "description": "Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted" }
    },
    "required": ["root"]
  }
}
```

#### IF-15: `explain`

Returns the full provenance record for a single symbol: definition location, caller count, callee count, and every direct incident edge with its condition, confidence, resolution tier, rule and call site.

```json
{
  "name": "explain",
  "inputSchema": {
    "type": "object",
    "properties": {
      "symbol":        { "type": "string" },
      "root":          { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true, "description": "Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted" }
    },
    "required": ["symbol", "root"]
  }
}
```

**Response structure** (live call, same clean checkout as IF-10; two of the four edges shown):

```json
{
  "structuredContent": {
    "symbol": "rust_sample::direct::chain",
    "kind": "function",
    "file": "src/direct.rs",
    "line": 47,
    "callers_count": 1,
    "callees_count": 3,
    "edges": [
      {
        "direction": "incoming",
        "peer": "rust_sample::imports::utils::run_chain",
        "peer_file": "src/imports.rs",
        "peer_line": 41,
        "condition": "always",
        "confidence": "probable",
        "tier": "scope_graph",
        "rule": "import-ref",
        "resolution_source": null,
        "site": { "file": "src/imports.rs", "line": 41 }
      },
      {
        "direction": "outgoing",
        "peer": "rust_sample::direct::step_a",
        "peer_file": "src/direct.rs",
        "peer_line": 53,
        "condition": "always",
        "confidence": "certain",
        "tier": "scope_graph",
        "rule": "scope-ref",
        "resolution_source": null,
        "site": { "file": "src/direct.rs", "line": 47 }
      }
    ],
    "dirty": false,
    "dirty_files_analyzed": 0,
    "freshness": { "matches_head": true, "stale": false, "…": "…" },
    "graph_version": "c19123d"
  }
}
```

`peer` is the other end of the edge whichever way it points; `direction` says which way. `explain` carries no `approximation` and no pagination keys — it is one document per symbol.

### IF-16: MCP Resources — **Planned (not yet shipped in v0.3.0)**

Not implemented at all: a `resources/list` request returns `{"error":{"code":-32601,"message":"method not found: resources/list"}}`. The server advertises no resources capability. Everything below is design.

Resources let an agent load schema or symbol lists once and cache them, avoiding repeated tool calls.

**`cgx://symbols/{root}`**  
Returns the full symbol index for the repository at `{root}`: qualified names, kinds, file locations, and entrypoint flags. Agents use this to resolve unqualified names before calling focused tools.

**`cgx://schema/{root}`**  
Returns the graph schema: node types, edge types, condition label vocabulary, confidence tier definitions, and supported query language constructs. Agents load this once to understand what queries are valid without schema trial-and-error.

Both resources are subscribable; the server sends a `notifications/resources/updated` event when the index changes (after `cgx index` completes).

### IF-17: Structured output per 2025-06-18 MCP spec — **Shipped, except `outputSchema`**

The server response already includes both halves of this design:

- `structuredContent` — the primary structured result, present on every tool result (see "The MCP response envelope" above for a verbatim example)
- `content` — the same data serialized as `TextContent` for clients that do not consume `structuredContent`

**Planned:** the `outputSchema` declaration in tool registration. `tools/list` does not carry one today, so a client can consume `structuredContent` but cannot validate it against a declared schema.

### IF-18: Pagination — **Shipped**

Every tool that returns a result list supports pagination via `cursor` / `has_more` / `total_matched`. Three responses are single documents and carry none of the three keys: `explain`, `reaches` with `to` (a verdict plus one witness), and `coupling` (its `pairs` list is bounded by `limit` instead).

**Request:** Include `cursor` from the prior response to fetch the next page.  
**Response:** Includes `has_more: true` and a new `cursor` value when more results exist.  
**Default page size:** 20 results per call (`paths`: 10). Override with `max_results`, clamped to 1–200.

Agents implement multi-hop traversal by chaining tool calls (callers of result → callers of those callers) rather than requesting deep recursive expansion in a single call.

### IF-19: Token-efficiency features — **Partly shipped:** `max_results` / `total_matched` are real; compact symbol IDs and `resource_link` bulk evidence are Planned

Token budgets are a real constraint for AI agent contexts. `cgx mcp` implements several token-efficiency patterns:

**Compact symbol IDs.** Results use qualified names (`module::Type::method`) not file paths as identifiers. A qualified name is stable across renames of the containing file and is shorter than a file path.

**`max_results` parameter.** Every tool except `explain` and `coupling` (which takes `limit`) accepts `max_results`. When the result set exceeds the limit, `has_more: true` and a cursor let the agent decide whether to paginate or summarize.

**`resource_link` for bulk evidence.** When a tool result would return large evidence sets (e.g., all 200 callers), the response can include a `resource_link` pointing to a resource URI instead of inlining the full list. The agent fetches the resource only if it needs the full set.

```json
{
  "structuredContent": {
    "symbol": "db::query",
    "callers_count": 200,
    "callers_preview": [
      { "name": "main::handle_request", "file": "src/main.rs", "line": 42 }
    ],
    "callers_resource_link": {
      "type": "resource_link",
      "uri": "cgx://callers/db%3A%3Aquery/workspace"
    }
  }
}
```

**Lazy schema loading.** Tool schemas are registered but the `cgx://schema/{root}` resource is not pushed unless requested. This avoids consuming the agent's context window with schema text on every session start.

---

## IF-20: `paths` Must-Analysis Flags — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — surface for Q-20 (must-pass-through / ∀-path) and Q-22 (acquire/release pairing). These flags extend the existing `paths` subcommand; no new subcommand is introduced.

```
cgx paths --from <symbol> --to <symbol> <path>
          [--must-pass-through <symbol>]
          [--avoiding <symbol>]
          [--quantifier exists|all]
          [--including-exception-paths]
          [--assert-all-reach-sink]
```

| Flag | Meaning |
|------|---------|
| `--must-pass-through <symbol>` | Return only paths that include the named node; with `--assert-empty` this asserts no path avoids it |
| `--avoiding <symbol>` | Return only paths that do NOT include the named node (complement set) |
| `--quantifier exists` | Default; returns any path satisfying the predicates (∃-path) |
| `--quantifier all` | Applies predicates over the full path set; use with `--to` to assert all paths reach the target |
| `--including-exception-paths` | Include exception- and panic-conditioned paths in the path set (Q-11 complement of `--exclude-edge-condition exception`) |
| `--assert-all-reach-sink` | Exit code 1 if any path from `--from` does NOT reach any symbol in `--to`; used for acquire/release completeness assertion |

`--must-pass-through` and `--avoiding` can each be repeated to apply multiple node constraints. All constraints combine as AND predicates.

The `--must-pass-through` and `--avoiding` / `--quantifier all` flags compile to the normative guarded-cut reachability semantics defined in docs/05-queries.md Q-20 (ADR-02). Results include a `guard_analysis` field in the output: `"cut+dominance"` when all callers on the path had dominance facts available (finding confidence = min edge confidence on path); `"cut-only"` when ≥1 caller lacked dominance facts (finding confidence capped at `probable`). See docs/05-queries.md Q-20 and Q-22 for worked examples.

---

## IF-21: `paths` Typed-Taint Flags — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — surface for Q-23 (typed taint queries). Adds class-based source, sink, and sanitizer selection to the `paths` subcommand.

```
cgx paths [--from-class <source-class>] [--to-class <sink-class>]
          [--require-sanitizer-class <sanitizer-class>]
          [--negate-sanitizer]
          [--to-package <pkg[@version-range]>]
          <path>
```

| Flag | Meaning |
|------|---------|
| `--from-class <class>` | Restrict sources to nodes with the named source class (DF-12.1). Values: `network`, `file`, `env`, `cli`, `db`, `deserialization`, `ipc`, `secret` |
| `--to-class <class>` | Restrict sinks to nodes with the named sink class (DF-12.2). Values: `sql`, `shell`, `path`, `html`, `header`, `redirect-url`, `format-string`, `regex`, `deserialize`, `eval`, `log`, `net-request` |
| `--require-sanitizer-class <class>` | Restrict to paths that include (or, with `--negate-sanitizer`, exclude) a sanitizer of the named class |
| `--negate-sanitizer` | Invert the sanitizer filter: return paths where NO matching-class sanitizer is present (the injection-finding mode) |
| `--to-package <spec>` | Restrict sinks to symbols in the named package; `spec` may include a version constraint (`libfoo@>=1.0.0,<2.0.0`) |

`--from-class` and `--to-class` traverse `DATA_FLOW` edges instead of `CALLS` edges. They require DF-11 and DF-12 to be active in the index.

See `docs/05-queries.md` Q-23 and Q-25 for worked examples.

---

## IF-22: `diff` Security-Gate Flags — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — surface for Q-25 Worked Example 6 (branch-diff security gate) and IX-9 (edge age attribution). Extends the existing `diff` subcommand.

> **Current shipped syntax:** `cgx diff <BASE> <HEAD>` (positional refs) with `--newer-than` only. The `--base`/`--head` named flags and the security-gate flags below are planned.

```
cgx diff --base <ref> [--head <ref>] <path>
         [--new-paths-only]
         [--calls-to-sink-class <class>]
         [--order introducing_commit]
```

| Flag | Meaning |
|------|---------|
| `--new-paths-only` | Restrict diff output to edges that introduce a new source → sink path not present at the base ref |
| `--calls-to-sink-class <class>` | Restrict diff output to new edges whose target has the named sink class; repeatable for multiple classes |
| `--order introducing_commit` | Sort results by `introducing_commit` timestamp (newest first); requires IX-9 edge-age attributes |

`--calls-to-sink-class` may be repeated. Multiple values combine as OR: `--calls-to-sink-class sql --calls-to-sink-class shell` returns new edges into either class.

See `docs/05-queries.md` Worked Example 6 and `docs/06-indexing-and-vcs.md` IX-9 for full examples.

---

## IF-23: MCP Tool `taint_paths` — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — MCP surface for Q-23 typed taint queries. Security and AI coding agents use this tool to find source → sink paths with class-matched sanitizer checking.

```json
{
  "name": "taint_paths",
  "inputSchema": {
    "type": "object",
    "properties": {
      "from_class":            { "type": "string", "description": "Source class (network, file, env, cli, db, deserialization, ipc, secret)" },
      "to_class":              { "type": "string", "description": "Sink class (sql, shell, path, html, log, eval, net-request, ...)" },
      "root":                  { "type": "string", "description": "Repository root path" },
      "require_sanitizer":     { "type": "boolean", "default": false, "description": "When true, return only paths WITH a matching sanitizer (coverage audit). When false (default), return paths WITHOUT a matching sanitizer (finding mode)." },
      "sanitizer_class":       { "type": "string", "description": "Override sanitizer class; defaults to to_class" },
      "confidence":            { "type": "string", "enum": ["certain","probable","possible"], "default": "probable" },
      "max_results":           { "type": "integer", "default": 20 },
      "cursor":                { "type": "string" },
      "at":                    { "type": "string" },
      "include_dirty":         { "type": "boolean", "default": true, "description": "Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted" }
    },
    "required": ["from_class", "to_class", "root"]
  }
}
```

**Example call:**

```json
{ "from_class": "network", "to_class": "sql", "root": "/workspace/myproject", "confidence": "probable" }
```

**Response** (same `structuredContent` schema as other path tools):

```json
{
  "structuredContent": {
    "results": [
      {
        "source": { "name": "http::request::body", "file": "src/handlers.rs", "line": 12 },
        "sink":   { "name": "db::query", "file": "src/db.rs", "line": 88 },
        "hops": 3,
        "has_sanitizer": false,
        "edge_conditions": ["always","always","conditional"]
      }
    ],
    "has_more": false,
    "cursor": null,
    "graph_version": "abc1234",
    "vacuous": false,
    "dirty": false,
    "dirty_files_analyzed": 0
  }
}
```

`vacuous: true` is set when the assertion passed vacuously (zero source or sink symbols matched, or all candidates were filtered out). See IF-5 vacuity guard for semantics.

---

## IF-24: MCP Tool `diff_security` — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — MCP surface for the branch-diff security gate (Q-25 Worked Example 6, IX-9). Lets AI security agents query which edges and paths are new on a branch relative to a base ref, with sink-class filtering and author attribution.

```json
{
  "name": "diff_security",
  "inputSchema": {
    "type": "object",
    "properties": {
      "root":               { "type": "string" },
      "base":               { "type": "string", "description": "Base ref (branch name, commit SHA, or 'main')" },
      "head":               { "type": "string", "default": "HEAD" },
      "sink_classes":       { "type": "array", "items": { "type": "string" }, "description": "Sink classes to filter new edges by (e.g. [\"sql\",\"shell\"])" },
      "new_paths_only":     { "type": "boolean", "default": true },
      "max_results":        { "type": "integer", "default": 20 },
      "cursor":             { "type": "string" },
      "include_dirty":      { "type": "boolean", "default": true, "description": "Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted" }
    },
    "required": ["root", "base"]
  }
}
```

**Example call:**

```json
{ "root": "/workspace/myproject", "base": "main", "sink_classes": ["sql","shell","eval"] }
```

**Response:**

```json
{
  "structuredContent": {
    "new_edges": [
      {
        "caller": { "name": "api::handler", "file": "src/api.rs", "line": 55 },
        "sink":   { "name": "db::execute", "file": "src/db.rs", "line": 22, "sink_class": "sql" },
        "introducing_commit": "a3f92b1",
        "introducing_author": "dev@example.com",
        "edge_condition": "always",
        "confidence": "certain"
      }
    ],
    "has_more": false,
    "cursor": null,
    "graph_version": "abc1234",
    "vacuous": false,
    "dirty": false,
    "dirty_files_analyzed": 0
  }
}
```

`vacuous: true` is set when the diff returned zero results because no source/sink symbols matched or all candidates were filtered. See IF-5 vacuity guard.

The `introducing_commit` and `introducing_author` fields come from IX-9 edge-age attribution. An AI agent can use this to identify which developer introduced a security-relevant edge and request a focused review.

---

## Non-goals (v1)

- **TUI (terminal user interface).** Not in scope for v1. A TUI mode is a separate future feature.
- **SSE/HTTP transport.** MCP STDIO is the v1 transport. SSE/HTTP (for multi-client shared graph cache) is a v2 feature.
- **Web UI or graphical viewer.** The `--format dot` and `--format mermaid` outputs feed existing tools (Graphviz, GitHub, Notion) rather than bundling a viewer.
- **Natural-language query interface.** The query language is deterministic; there is no LLM in the answer path. Natural-language wrapping is the responsibility of the calling agent.
