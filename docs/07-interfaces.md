# cgx Interfaces

**Status:** Draft  
**Audience:** Software engineers, security engineers, AI coding agents, CI operators  
**Working name:** `cgx` (placeholder; see `README.md`)  
**Cross-references:** `docs/05-queries.md` (query capabilities, feature IDs Q-), `docs/03-code-graph-model.md` (GM- features, edge condition labels), `docs/04-dataflow-and-provenance.md` (DF- features)

---

## Overview

`cgx` exposes two interfaces:

1. **CLI** — a human-friendly and script-friendly command-line tool. TTY-aware defaults. Composable with standard Unix tools.
2. **MCP STDIO server** — a Machine-readable Control Protocol server over standard input/output. Designed for AI coding and security agents. Conforms to the 2025-06-18 MCP specification.

A TUI (terminal user interface) is explicitly out of scope for v1.

---

## Feature List

| ID | Feature |
|----|---------|
| IF-1 | CLI entry point: `cgx <subcommand> [flags] <path>` |
| IF-2 | TTY-aware output: text on TTY, jsonl when piped |
| IF-3 | `--format` flag: text, tree, json, jsonl, csv, dot, mermaid, graphml, sarif |
| IF-4 | Exit-code contract (0/1/2/3) |
| IF-5 | CI assertion mode: `--assert-empty`, `--assert-count N`, `--assert-max N` |
| IF-6 | `--explain` flag: per-fact provenance, rule annotation, index version |
| IF-7 | `--at <ref>` flag: query against a specific commit |
| IF-8 | `--order` flag: stable result ordering |
| IF-9 | MCP STDIO server mode: `cgx mcp` |
| IF-10 | MCP tool: `graph_query` |
| IF-11 | MCP tool: `callers` |
| IF-12 | MCP tool: `callees` |
| IF-13 | MCP tool: `paths` |
| IF-14 | MCP tool: `unused` |
| IF-15 | MCP tool: `explain` |
| IF-16 | MCP resources: `callgraph://symbols/{root}`, `callgraph://schema/{root}` |
| IF-17 | MCP structured output: `structuredContent` + `outputSchema` per 2025-06-18 spec |
| IF-18 | MCP pagination: `cursor` + `has_more` |
| IF-19 | MCP token-efficiency: `max_results`, compact symbol IDs, `resource_link` for bulk evidence |

---

## Part 1: CLI Interface

### IF-1: Entry point

```
cgx <subcommand> [flags] <path>
```

`<path>` is the repository root or any subdirectory. `cgx` locates the graph index by walking up to the `.git` directory (or the configured index root; see `docs/06-indexing-and-vcs.md`).

**Top-level subcommands:**

| Subcommand | Description |
|------------|-------------|
| `callers` | Find symbols that call a given symbol |
| `callees` | Find symbols a given symbol calls |
| `paths` | Find call paths between two symbols |
| `unused` | Find symbols not reachable from any entrypoint |
| `pedigree` | Trace inbound value provenance for a variable |
| `explain` | Show full provenance record for a symbol |
| `diff` | Compute graph diff across commits or branches |
| `query` | Execute a full query-language expression |
| `index` | Build or update the graph index |
| `prune` | Remove stale index entries (branches deleted, files removed) |
| `mcp` | Start MCP STDIO server |

See `docs/05-queries.md` for complete per-subcommand signatures and worked examples.

### IF-2: TTY-aware defaults

`cgx` detects whether stdout is a TTY and adjusts defaults accordingly — the same pattern used by `bat`, `delta`, and `gh`.

| stdout | Default format | Color |
|--------|---------------|-------|
| TTY (interactive) | `text` | enabled |
| Pipe / redirect | `jsonl` | disabled |

To override: `--format <fmt>` or `--color always|never|auto`.

### IF-3: `--format` flag

All subcommands accept `--format`. The format applies to the result stream, not to progress or error output (those always go to stderr).

| Format | Description | Primary use case |
|--------|-------------|-----------------|
| `text` | Column-aligned human-readable text with file:line | Interactive terminal |
| `tree` | Indented call tree (`cargo tree` style) | Visualizing call hierarchy |
| `json` | JSON array of result objects, pretty-printed | Scripting, debugging |
| `jsonl` | One JSON object per line (streaming) | Large results, piping to `jq` |
| `csv` | Tabular results with header row | Spreadsheet, pandas |
| `dot` | Graphviz DOT language | CI pipelines, headless SVG generation |
| `mermaid` | Mermaid graph syntax | GitHub/Notion docs, README graphs |
| `graphml` | GraphML XML format | Gephi, yEd, graph analysis tools |
| `sarif` | SARIF 2.1.0 (OASIS standard) | GitHub Advanced Security, VS Code SARIF viewer |

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
| 1 | CI assertion failed: `--assert-empty` fired (results found when none expected), or `--assert-count` / `--assert-max` threshold exceeded |
| 2 | Query parse error or invalid flag combination |
| 3 | Graph error: index missing, corrupt, or build failed |

Exit code 1 is the CI-gate signal. Exit codes 2 and 3 indicate tool or configuration problems, not result conditions.

### IF-5: CI assertion mode

`--assert-empty`, `--assert-count N`, and `--assert-max N` turn a query into a boolean CI check.

```bash
# Fail CI if any call path reaches a dangerous sink
cgx paths --from '**' --to dangerous::sink ./ \
    --assert-empty \
    --format sarif > security.sarif
# Exit 1 if paths found; exit 0 if none

# Fail CI if more than 5 paths reach the sink
cgx paths --from '**' --to dangerous::sink ./ \
    --assert-max 5

# Fail CI if the number of public unused methods changes
cgx unused ./ --kind method --confidence certain \
    --assert-count 0
```

These flags compose with any subcommand. When `--assert-empty` fires, output is still written (to stdout or `--output <file>`); the exit code signals the assertion result.

**GitHub Actions integration example:**

```yaml
- name: Security regression gate
  run: |
    cgx paths --from '**' --to vulnerable::bar ./ \
        --assert-empty --format sarif > findings.sarif
  continue-on-error: false

- name: Upload SARIF
  uses: github/codeql-action/upload-sarif@v3
  with:
    sarif_file: findings.sarif
```

### IF-6: `--explain` flag

Adding `--explain` to any subcommand or query command annotates each result fact with its full provenance record:

```
Call edge: main::baz -> crypto::hash
  Source:       src/main.rs:42:8
  Evidence:     call expression AST node #1337
  Edge type:    CALLS
  Condition:    always
  Confidence:   certain
  Rule:         direct_call_resolution v1.2
  Graph build:  commit abc1234, indexed 2026-06-11T13:00:00Z
  Index:        .callgraph/index.v2.db (sha256: deadbeef...)
```

Without `--explain`, provenance is available in `--format json` as structured fields on each result object. `--explain` adds the verbose rule annotation to human-readable output.

Every result node always carries `{file, line, col}` provenance. Every graph edge carries the AST node reference that generated it. Both are stored in the index at index time, not computed at query time.

### IF-7: `--at <ref>` flag

Any subcommand accepts `--at <ref>` to query the graph as it existed at a specific commit:

```bash
cgx callers crypto::hash ./ --at abc1234
cgx callers crypto::hash ./ --at HEAD~5
cgx paths --from main::foo --to vulnerable::bar ./ --at v1.2.3
```

`--at` enables: "Did this path exist before this PR?" and "Which commit introduced this edge?" When used with `diff`, `--base` and `--head` replace `--at`.

### IF-8: `--order` flag

```
--order file        (default) Sort by (file_path, line, col) — deterministic
--order alpha       Sort by qualified symbol name
--order discovery   BFS/DFS traversal order from the query root
--order relevance   Depth-ascending from the query root
```

The default `file` ordering is stable across runs on identical graph data. See `docs/05-queries.md` Q-17 for the full determinism contract.

---

## Part 2: MCP STDIO Server

### IF-9: Starting the MCP server

```bash
cgx mcp [--root <path>] [--log-level debug|info|warn]
```

`cgx mcp` reads from stdin and writes to stdout using the MCP 2025-06-18 STDIO transport. It speaks NDJSON (newline-delimited JSON). The MCP server is single-process, single-client; for multi-client use, wrap it in an MCP proxy (future work, v2).

**Recommended v1 transport:** STDIO. SSE/HTTP transport is a v2 feature. STDIO works in Claude Code, Cursor, Continue.dev, and any MCP-compatible agent harness without network configuration.

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

The server exposes six tools and two resources.

#### IF-10: `graph_query`

Execute a full query-language expression and return structured results.

```json
{
  "name": "graph_query",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query":       { "type": "string", "description": "Cypher-subset query expression" },
      "root":        { "type": "string", "description": "Repository root path" },
      "at":          { "type": "string", "description": "Commit ref (default: HEAD)" },
      "max_results": { "type": "integer", "default": 20 },
      "cursor":      { "type": "string", "description": "Pagination cursor from prior call" },
      "format":      { "type": "string", "enum": ["json", "jsonl"], "default": "json" }
    },
    "required": ["query", "root"]
  }
}
```

**Response** (`structuredContent` per 2025-06-18 spec):

```json
{
  "structuredContent": {
    "results": [
      {
        "name": "main::baz",
        "file": "src/main.rs",
        "line": 42,
        "col": 8,
        "edge_condition": "always",
        "confidence": "certain"
      }
    ],
    "has_more": false,
    "cursor": null,
    "graph_version": "abc1234",
    "total_matched": 1
  }
}
```

The `structuredContent` field is the canonical structured result. For backward compatibility with clients that only consume `TextContent`, the server also serializes the same data as the `content` array entry.

#### IF-11: `callers`

```json
{
  "name": "callers",
  "inputSchema": {
    "type": "object",
    "properties": {
      "symbol":          { "type": "string", "description": "Qualified symbol name" },
      "root":            { "type": "string" },
      "depth":           { "type": "integer", "default": 1 },
      "max_results":     { "type": "integer", "default": 20 },
      "cursor":          { "type": "string" },
      "edge_condition":  { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":      { "type": "string", "enum": ["certain","probable","possible"] },
      "at":              { "type": "string" }
    },
    "required": ["symbol", "root"]
  }
}
```

**Example call:**

```json
{ "symbol": "crypto::hash", "root": "/workspace/myproject", "depth": 3, "confidence": "certain" }
```

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
      "exclude_edge_condition":  { "type": "string" },
      "only_edge_condition":     { "type": "string" },
      "cursor":                  { "type": "string" },
      "at":                      { "type": "string" }
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
      "type_filter":    { "type": "string", "description": "Restrict to members of this type" },
      "entrypoint":     { "type": "string" },
      "confidence":     { "type": "string", "enum": ["certain","probable"] },
      "max_results":    { "type": "integer", "default": 20 },
      "cursor":         { "type": "string" }
    },
    "required": ["root"]
  }
}
```

#### IF-15: `explain`

Returns the full provenance record for a single symbol: definition location, caller count, callee count, all edges with their condition and confidence labels, index version, and build metadata.

```json
{
  "name": "explain",
  "inputSchema": {
    "type": "object",
    "properties": {
      "symbol": { "type": "string" },
      "root":   { "type": "string" },
      "at":     { "type": "string" }
    },
    "required": ["symbol", "root"]
  }
}
```

**Response structure:**

```json
{
  "structuredContent": {
    "symbol": "crypto::hash",
    "file": "src/crypto.rs",
    "line": 15,
    "callers_count": 12,
    "callees_count": 3,
    "edges": [
      {
        "direction": "incoming",
        "from": "main::baz",
        "condition": "always",
        "confidence": "certain",
        "from_file": "src/main.rs",
        "from_line": 42
      }
    ],
    "graph_version": "abc1234",
    "index_file": ".callgraph/index.v2.db",
    "index_sha256": "deadbeef..."
  }
}
```

### IF-16: MCP Resources

Resources let an agent load schema or symbol lists once and cache them, avoiding repeated tool calls.

**`callgraph://symbols/{root}`**  
Returns the full symbol index for the repository at `{root}`: qualified names, kinds, file locations, and entrypoint flags. Agents use this to resolve unqualified names before calling focused tools.

**`callgraph://schema/{root}`**  
Returns the graph schema: node types, edge types, condition label vocabulary, confidence tier definitions, and supported query language constructs. Agents load this once to understand what queries are valid without schema trial-and-error.

Both resources are subscribable; the server sends a `notifications/resources/updated` event when the index changes (after `cgx index` completes).

### IF-17: Structured output per 2025-06-18 MCP spec

All tools declare an `outputSchema` (JSON Schema) in their tool registration. The server response includes:

- `structuredContent` — the primary structured result (typed per `outputSchema`)
- `content` — the same data serialized as `TextContent` for backward compatibility with older MCP clients

Agents that understand `structuredContent` can validate responses against the schema and do client-side type checking. Agents that only understand `TextContent` receive the JSON-serialized string and parse it themselves.

### IF-18: Pagination

All tools that return multiple results support pagination via `cursor` / `has_more`.

**Request:** Include `cursor` from the prior response to fetch the next page.  
**Response:** Includes `has_more: true` and a new `cursor` value when more results exist.  
**Default page size:** 20 results per call. Override with `max_results` (max 200).

Agents implement multi-hop traversal by chaining tool calls (callers of result → callers of those callers) rather than requesting deep recursive expansion in a single call.

### IF-19: Token-efficiency features

Token budgets are a real constraint for AI agent contexts. `cgx mcp` implements several token-efficiency patterns:

**Compact symbol IDs.** Results use qualified names (`module::Type::method`) not file paths as identifiers. A qualified name is stable across renames of the containing file and is shorter than a file path.

**`max_results` parameter.** Every tool accepts `max_results`. When the result set exceeds the limit, `has_more: true` and a cursor let the agent decide whether to paginate or summarize.

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
      "uri": "callgraph://callers/db%3A%3Aquery/workspace"
    }
  }
}
```

**Lazy schema loading.** Tool schemas are registered but the `callgraph://schema/{root}` resource is not pushed unless requested. This avoids consuming the agent's context window with schema text on every session start.

---

## Non-goals (v1)

- **TUI (terminal user interface).** Not in scope for v1. A TUI mode is a separate future feature.
- **SSE/HTTP transport.** MCP STDIO is the v1 transport. SSE/HTTP (for multi-client shared graph cache) is a v2 feature.
- **Web UI or graphical viewer.** The `--format dot` and `--format mermaid` outputs feed existing tools (Graphviz, GitHub, Notion) rather than bundling a viewer.
- **Natural-language query interface.** The query language is deterministic; there is no LLM in the answer path. Natural-language wrapping is the responsibility of the calling agent.
