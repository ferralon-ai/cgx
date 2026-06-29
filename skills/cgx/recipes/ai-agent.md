---
title: AI-Agent-Specific Queries
audience: AI agents (ACA/ASA) driving cgx with a limited tool-call budget
since: v0.1 (MCP core + CLI subcommands); v0.3 (DATA_FLOW edges, flows-to/flows-from); v0.5 (working graph_query MCP tool)
---

# AI-Agent-Specific Queries

**Run `cgx --version` first.** A capability tagged `Since: v0.N` is available iff your minor
version is ≥ N. The current shipped binary is v0.3.0. See `reference/versions.md` for the full
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
| Use `--format json` for CLI results that will be parsed | `cgx callers MyFn --depth 2 --format json` |

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
cgx callers Config::load --depth 1 --confidence probable --format json

# MCP equivalent
callers(symbol="Config::load", root="/workspace", depth=1, max_results=50)
```

**Why this works:** `--depth 1` (CLI) / `depth=1` (MCP) returns direct call sites only —
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
cgx callees OrderController::create --depth 1 --format json
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

**CLI form (CALLS-graph CQL — Since: v0.1):**
```bash
# Bound the hop count when combining with path= variable binding (see CQL notes below)
# Use cgx paths for multi-hop chains; CQL is useful for filtering or tabular output
cgx query '
  MATCH (a)-[:CALLS*3]->(b)
  WHERE b.name = "payments::process_payment"
  RETURN a.name, a.file, a.line
  LIMIT 3
' --format json
```

**Why this works:** `paths` MCP + `explain` MCP covers the complete chain in 2 calls.
The CQL form captures entrypoints in 1 CLI call. Both avoid the need for repeated grep/LSP/file-read
tool calls.

**Reading the result:** Use `cgx paths <ep> payments::process_payment` to get per-hop
edge-condition labels on a specific chain after identifying the entrypoint with the CQL query.

**CQL notes — Since: v0.1:**
- Unbounded `CALLS*` (no `N`) is work-budget-capped and returns results, but may be slow on very large graphs. Prefer `CALLS*N` with an explicit N when you know the chain depth.
- `path = (a)-[:CALLS*N]->(b)` (path-variable binding) with `nodes(path)` and `relationships(path)`
  works for N=1. For N≥2 the work budget may be exceeded on large graphs; prefer `cgx paths` for
  multi-hop path enumeration.
- Object literals `{key: value}` are NOT supported in list comprehensions — a parse error (exit 2).
  Use `[n IN nodes(path) | n.name]` (scalar projections only).
- String literals must be double-quoted inside the query. Wrap the whole query in single quotes
  for the shell.
- Available node properties: `name`, `fqn`, `kind`, `file`, `line`.
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
cgx callers sendEmail --depth 1 --format json
```

For two hops back (setup chain visible):
```bash
cgx callers sendEmail --depth 2 --format json
```

**Reading the result:** Each caller row includes `file` and `line`. Load the top 2–3 call sites
to see the setup pattern before writing your new call. If all existing callers share a common
setup function (e.g., `validate_email_address`), your new call should follow the same sequence.

---

## Spec-only recipes (not runnable today)

These questions appear in the cookbook with `answerable-today` tags, but they require features
not yet available in v0.3.0. Do not emit these as runnable commands.

| Question | Requires | Since |
|----------|----------|-------|
| Codebase-wide command-injection scan (HTTP → shell sink, no sanitizer on path) | `source_class`/`sink_class`/`sanitizer_class` node props (plan error exit 2 in v0.3) | deferred |
| Generate SARIF taint report (HTTP input → SQL sinks, full call chain) | `--from-class`/`--to-class` (phantom flags), `sanitizer_class` CQL prop (plan error exit 2) | deferred |
| CVE reachability triage (cargo audit advisory → entrypoints, sanitizer on path) | `--to-package` (phantom flag), `sanitizer_class IS NOT NULL` (plan error exit 2) | deferred |
| Arbitrary CQL via MCP `graph_query` tool | working `graph_query` implementation | v0.5 |
| MCP token-efficiency (compact symbol IDs, resource_link, lazy schema) | full token-efficiency features | v0.5 |

**Note on structural dataflow (available today — Since: v0.3):** `DATA_FLOW` edges and
`:DATA_FLOW` CQL queries work today. `cgx flows-to` and `cgx flows-from` (CLI-only, no MCP
equivalent) traverse `derives-from` edges to show which values a value node flows into or derives
from. These operate on SSA value-node FQNs (e.g. `fn::local#1`), not on function FQNs.

**Workaround for security-typed taint questions (deferred):** Use `cgx paths <from> <to>` to
check call reachability (whether a call path exists), and `cgx query` with `:DATA_FLOW` edges to
trace structural data flow. This answers "can control flow/data flow reach the sink" — not "is
the flow tainted and unsanitized". Security-typed taint with source/sink/sanitizer classification
is deferred beyond v0.3.

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

# ANY quantifier: direct callers that have at least one exception edge
# Note: path= binding with N≥2 may exhaust the work budget on large graphs — keep N=1 for path-variable queries
cgx query '
  MATCH path = (a)-[:CALLS*1]->(b)
  WHERE b.name = "my_fn"
    AND ANY(r IN relationships(path) WHERE r.condition = "exception")
  RETURN a.name, a.file LIMIT 10
' --format json

# NONE quantifier: direct callers with no always-condition edge on the path
cgx query '
  MATCH path = (a)-[:CALLS*1]->(b)
  WHERE b.name = "my_fn"
    AND NONE(r IN relationships(path) WHERE r.condition = "always")
  RETURN a.name, a.file LIMIT 5
' --format json

# DATA_FLOW edges (Since: v0.3 — present by default; absent on --no-dataflow indexes)
cgx query 'MATCH (a)-[:DATA_FLOW]->(b) RETURN a.name, b.name LIMIT 20' --format json

# Filter by kind
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE a.kind = "function" RETURN a.name LIMIT 20' --format json

# Run a saved query file
cgx query @/path/to/query.cql --format json

# Query the graph at a prior git ref
cgx query 'MATCH (a)-[:CALLS]->(b) RETURN a.name LIMIT 5' --at HEAD~1
```

**Shell quoting rule:** Wrap the entire CQL expression in single quotes. Use double quotes for
string literals inside the query. Single quotes inside the query are a parse error.

**Never emit:**
- `path = (a)-[:CALLS*]->(b)` with no N (unbounded path-variable binding — work budget exceeded; use N=1 or `cgx paths`)
- `path = (a)-[:CALLS*N]->(b)` with N≥2 (path-variable binding with multi-hop — work budget exceeded on large graphs; use N=1 or use `cgx paths` instead)
- `{key: value}` object literals in list comprehensions — parse error exit 2 (use scalar projections: `[n IN nodes(path) | n.name]`)
- `NOT IN [...]` — parse error
- `MATCH (n) RETURN n` — no relationship, plan error
- `entrypoint_class`/`source_class`/`sink_class`/`sanitizer_class`/`taint_label` properties — plan error exit 2

See `reference/query-language.md` for the full supported/unsupported clause list.
