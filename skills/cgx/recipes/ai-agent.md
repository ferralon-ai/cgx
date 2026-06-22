---
title: AI-Agent-Specific Queries
audience: AI agents (ACA/ASA) driving cgx with a limited tool-call budget
since: v0.1 (MCP core + CLI subcommands); v0.3 (taint/sanitizer); v0.5 (working graph_query MCP tool)
---

# AI-Agent-Specific Queries

**Run `cgx --version` first.** A capability tagged `Since: v0.N` is available iff your minor
version is ≥ N. The current shipped binary is v0.1. See `reference/versions.md` for the full
ladder.

This recipe targets agents (ACA/ASA) with a small tool-call budget. Every pattern here minimizes
round-trips and context-window consumption. For MCP tool schemas and pagination detail, see
`reference/mcp.md`. For CQL syntax rules and which clauses are supported today, see
`reference/query-language.md`.

---

## Critical: graph_query MCP tool is unimplemented — Since: v0.5

`graph_query` is listed in `tools/list` but **always returns a ToolError in every released
version**. Never call it. For arbitrary CQL queries, shell out to the CLI:

```bash
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_fn" RETURN a.name LIMIT 10'
```

A working `graph_query` MCP tool is Since: v0.5. Until then, the 5 functional MCP tools
(`callers`, `callees`, `paths`, `unused`, `explain`) cover the common agent patterns, and the
CLI `cgx query` covers arbitrary CALLS-graph CQL.

---

## Step 0 — Find the exact symbol name before every query

A wrong symbol name produces exit 2 (`no symbol matched '<x>'`). Use `cgx search <pattern>`
(Since: v0.2) to resolve a partial name to the exact FQN; grep/ripgrep is the fallback for
older binaries.

```bash
# Since: v0.2 — resolves partial name to exact FQN(s) in one call
cgx search process_payment
# Fallback for cgx < v0.2
rg --type rust -n "fn process_payment" .
# → crates/payments/src/lib.rs:42:    pub fn process_payment(
# Fully-qualified: payments::process_payment  (or the crate-path prefix)
```

The index auto-builds on first query into `.cgx/`. Force a rebuild with `cgx index .`.

---

## Token-efficiency defaults

MCP tool defaults differ from the CLI. Set these explicitly to avoid over-fetching:

| Practice | MCP call |
|----------|----------|
| Start with `explain` — one call returns all edges, caller/callee counts, location | `explain(symbol, root)` |
| Keep depth small (MCP default is 1; increase only when you need transitive results) | `callers(symbol, root, depth=2)` |
| Set `max_results` explicitly | `callers(symbol, root, max_results=10)` |
| Paginate on demand — check `has_more`; fetch the next page only if needed | pass `cursor` from the prior response |
| Filter by edge condition to narrow result sets | `callers(symbol, root, edge_condition="exception")` |
| Use `--format json` for CLI results that will be parsed | `cgx callers MyFn --max-depth 2 --format json` |

---

## Runnable-today recipes — Since: v0.1

### Get the edit context for a function in two MCP calls

**Status:** runnable today **Personas:** ACA

Before editing a function, fetch its caller context and callee context. Two MCP calls give the
minimal subgraph needed. The MCP `depth` default is 1; increase it only as far as you need.

```
# Call 1 — who calls the function (2 hops back)
callers(symbol="payments::process_payment", root="/workspace", depth=2, max_results=20)

# Call 2 — what the function calls (3 hops forward)
callees(symbol="payments::process_payment", root="/workspace", depth=3, max_results=20)
```

**Why this works:** Edge-condition labels (`always`, `conditional`, `exception`, `loop`, `panic`)
on each result edge tell you which callees are happy-path vs. exception-path, without fetching
the entire reachable subgraph.

**Reading the result:** `edge_condition` on each result edge tells you which paths are
unconditional vs. exception-only. A `confidence: "possible"` edge may be a dynamic-dispatch
approximation (see `reference/mental-model.md`). Keep depth ≤ 3 unless the chain is known to
be deeper.

---

### Get a one-shot overview before deciding which queries to run

**Status:** runnable today **Personas:** ACA · ASA

`explain` returns the full provenance record for one symbol in a single call: definition
location, caller count, callee count, and all edges with condition and confidence labels. Use it
to decide whether deeper `callers`/`callees` queries are needed at all.

```
# MCP
explain(symbol="payments::process_payment", root="/workspace")

# CLI equivalent
cgx explain payments::process_payment --format json
```

**Reading the result:** If `callers_count` is 0 the function is an entrypoint or is unreachable.
If `callers_count` is large (> 50) keep `callers` depth at 1 and paginate on demand rather than
fetching all hops at once.

---

### Find all call sites of a function (refactor scope)

**Status:** runnable today **Personas:** ACA

Before adding a parameter to a function, find every call site so you know the refactor scope.

```bash
# CLI — all direct callers
cgx callers Config::load --max-depth 1 --confidence probable --format json

# MCP equivalent
callers(symbol="Config::load", root="/workspace", depth=1, max_results=50)
```

**Why this works:** `--max-depth 1` (CLI) / `depth=1` (MCP) returns direct call sites only —
the ones that pass arguments and must be updated. `--confidence probable` / `confidence="probable"`
includes callers reached via dynamic dispatch (trait objects), not just statically-certain callers.

**Reading the result:** Each returned caller is a call site that must be updated. The total count
tells you the refactor scope before you begin. `caller.kind` distinguishes test functions from
production code.

Note: `--confidence` filtering discriminates between confidence levels starting at Since: v0.2
(SCIP enrichment). In v0.1, `probable` is accepted as a flag but may not fully discriminate.

---

### Find callees in files not yet loaded (context-window gap detection)

**Status:** runnable today **Personas:** ACA

After loading a set of files, find which callees of a function are in files you have not yet
loaded. The MCP `callees` tool at depth 1 returns all direct callees with their file locations.

```
# MCP — direct callees, one hop
callees(symbol="OrderController::create", root="/workspace", depth=1, max_results=20)
```

Then filter the response client-side: discard rows where `file` matches a file you have already
loaded. Each remaining row is a callee in an unloaded file.

**CLI form:**
```bash
cgx callees OrderController::create --max-depth 1 --format json
```

**Reading the result:** Each result row includes `file` and `line` for the callee's definition.
A callee with `confidence: "possible"` is an over-approximation from dynamic dispatch — treat it
as a candidate, not a certainty.

---

### Trace the call path from entrypoint to a target function

**Status:** runnable today **Personas:** ACA · ASA

Find every path from any caller to a specific function, with edge-condition labels on each hop.
This is the pattern behind the CIE benchmark (3 MCP calls vs. 34 tool calls with grep/LSP).

```
# MCP — Call 1: find paths (adjust max_depth to the known chain depth)
paths(from="http::Router::handle", to="payments::process_payment",
      root="/workspace", max_depth=10, max_results=5)

# MCP — Call 2: explain the target for signatures and all edge detail
explain(symbol="payments::process_payment", root="/workspace")
```

**CLI form (arbitrary CALLS-graph CQL):**
```bash
# Bounded hop count is required — never omit the N in CALLS*N (unbounded hangs)
cgx query '
  MATCH path = (ep)-[:CALLS*5]->(fn)
  WHERE fn.name = "payments::process_payment"
  RETURN ep.name, ep.file, ep.line,
         [n IN nodes(path) | {name: n.name, file: n.file, line: n.line}] AS chain,
         [r IN relationships(path) | r.condition] AS edge_conditions,
         length(path) AS hops
  ORDER BY hops
  LIMIT 3
' --format json
```

**Why this works:** `paths` MCP + `explain` MCP covers the complete chain in 2 calls.
The CQL form captures the chain in 1 CLI call. Both avoid the need for repeated grep/LSP/file-read
tool calls.

**Reading the result:** `edge_conditions` on each hop tells you which steps are always-condition
vs. exception-only — the critical context for safe edits. `chain` gives all intermediate
function locations without additional reads.

**CQL notes (v0.1 CALLS-graph CQL only):**
- Always bound the hop count: `CALLS*5`, `CALLS*10`. Unbounded `CALLS*` hangs.
- String literals must be double-quoted inside the query. Wrap the whole query in single quotes
  for the shell.
- Available node properties: `name`, `kind`, `file`, `line`.
- Available edge properties: `condition`, `confidence`.
- `MATCH` must contain at least one relationship — `MATCH (n) RETURN n` is a plan error.

---

### Find existing callers to understand the established calling pattern

**Status:** runnable today **Personas:** ACA

Before implementing a new call to a function, inspect existing call sites to understand the
established pattern: what state is set up before the call, what arguments are passed.

```bash
# Direct callers — MCP
callers(symbol="sendEmail", root="/workspace", depth=1, max_results=20)

# CLI equivalent
cgx callers sendEmail --max-depth 1 --format json
```

For two hops back (setup chain visible):
```bash
cgx callers sendEmail --max-depth 2 --format json
```

**Reading the result:** Each caller row includes `file` and `line`. Load the top 2–3 call sites
to see the setup pattern before writing your new call. If all existing callers share a common
setup function (e.g., `validate_email_address`), your new call should follow the same sequence.

---

## Spec-only recipes (not runnable today)

These questions appear in the cookbook with `answerable-today` tags, but they require features
not present in the v0.1 binary. Do not emit these as runnable commands.

| Question | Requires | Since |
|----------|----------|-------|
| Codebase-wide command-injection scan (HTTP → shell sink, no sanitizer on path) | `DATA_FLOW` edges, `source_class`/`sink_class`/`sanitizer_class` node props | v0.3 |
| Generate SARIF taint report (HTTP input → SQL sinks, full call chain) | `DATA_FLOW`, `--from-class`/`--to-class`, `sanitizer_class` | v0.3 |
| CVE reachability triage (cargo audit advisory → entrypoints, sanitizer on path) | `--to-package`, `sanitizer_class IS NOT NULL`, unbounded CALLS* (v0.3 guard) | v0.4 |
| Arbitrary CQL via MCP `graph_query` tool | working `graph_query` implementation | v0.5 |
| MCP token-efficiency (compact symbol IDs, resource_link, lazy schema) | full token-efficiency features | v0.5 |

**Workaround for taint questions (until v0.3):** Use `cgx paths <from> <to>` to check call
reachability (whether a call path exists). This answers "can control flow reach the sink" — not
"can data flow there". State that boundary explicitly when reporting findings. Real taint with
source/sink/sanitizer classification is Since: v0.3.

**Workaround for graph_query (until v0.5):** Shell out to `cgx query '<CQL>'` using the
CALLS-graph CQL subset documented in `reference/query-language.md`.

---

## CQL quick reference for agent use — Since: v0.1

These patterns run today. All others require a higher version.

```bash
# Who calls my_fn (direct callers)
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_fn" RETURN a.name, a.file, a.line LIMIT 20' --format json

# Filter by edge condition (exception-path callers only)
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE b.name = "my_fn" AND r.condition = "exception" RETURN a.name, a.file LIMIT 20' --format json

# Bounded transitive callers (N hops — always specify N)
cgx query 'MATCH (a)-[:CALLS*3]->(b) WHERE b.name = "my_fn" RETURN a.name, a.file LIMIT 10' --format json

# ANY quantifier: paths that contain at least one exception edge
cgx query '
  MATCH path = (a)-[:CALLS*3]->(b)
  WHERE b.name = "my_fn"
    AND ANY(r IN relationships(path) WHERE r.condition = "exception")
  RETURN a.name, a.file LIMIT 10
' --format json

# NONE quantifier: paths with no always-condition edge
cgx query '
  MATCH path = (a)-[:CALLS*3]->(b)
  WHERE b.name = "my_fn"
    AND NONE(r IN relationships(path) WHERE r.condition = "always")
  RETURN a.name, a.file LIMIT 5
' --format json

# Filter by kind
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE a.kind = "function" RETURN a.name LIMIT 20' --format json

# Run a saved query file
cgx query @/path/to/query.cql --format json

# Query the graph at a prior git ref
cgx query 'MATCH (a)-[:CALLS]->(b) RETURN a.name LIMIT 5' --at HEAD~1
```

**Shell quoting rule:** Wrap the entire CQL expression in single quotes. Use double quotes for
string literals inside the query. Single quotes inside the query are a parse error.

**Never emit:** `CALLS*` (unbounded — hangs), `DATA_FLOW` edges, `NOT IN [...]` (parse error),
`MATCH (n) RETURN n` (no relationship — plan error), `entrypoint_class`/`source_class`/
`sink_class`/`sanitizer_class`/`taint_label` properties (plan error exit 2).

See `reference/query-language.md` for the full supported/unsupported clause list.
