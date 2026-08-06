---
title: AI-Agent-Specific Queries
audience: AI agents (ACA/ASA) driving cgx with a limited tool-call budget
since: v0.1 (MCP core + CLI subcommands); v0.3 (DATA_FLOW edges, flows-to/flows-from, graph_query, the approximation contract and the index-freshness envelope)
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

## The MCP tool surface — 12 tools, all live

`tools/list` returns twelve tools, in this fixed order
(`tool_list`, `crates/cgx-mcp/src/tools.rs`):

```
callers  callees  reaches  paths  unused  explain  search  symbols
flows_to  flows_from  graph_query  coupling
```

Every one of them executes. Three spelling traps: the MCP names are `flows_to` / `flows_from` /
`graph_query` with **underscores**, while the CLI subcommands are `flows-to` / `flows-from` /
`query`.

**`graph_query` is live and is the tool to reach for when no fixed-shape tool asks your question.**
It routes to `cgx_cql::run` (`tools.rs:978`) — the same engine `cgx query` drives, not a fork — so a
query that works on one surface returns the same rows on the other. There is no reason to shell out
to the CLI for CQL from an MCP session. Verified live this cycle against the shipped
`fixtures/rust-sample`:

```
graph_query(query="MATCH (a)-[:CALLS]->(b) WHERE a.fqn = \"rust_sample::conditions::dispatch\" RETURN a.fqn, b.fqn",
            root="/path/to/fixture")
```
```json
{
  "columns": ["a.fqn", "b.fqn"],
  "rows": [
    ["rust_sample::conditions::dispatch", "rust_sample::conditions::log_info"],
    ["rust_sample::conditions::dispatch", "rust_sample::conditions::log_warn"],
    ["rust_sample::conditions::dispatch", "rust_sample::conditions::log_error"]
  ],
  "total_matched": 3, "has_more": false, "cursor": null,
  "truncated": false, "truncation_reason": null,
  "approximation": { "direction": "exact", "modeled_graph": "…", "reasons": [] },
  "freshness": { "dirty_files": 0, "dirty_files_base": "dd3ea2b…", "head_tree": "dd3ea2b…",
                 "indexed_tree": "workdir:89206f0…", "matches_head": null, "stale": false },
  "graph_version": "dd3ea2b+dirty.d17ae3cd4bee", "dirty": true, "dirty_files_analyzed": 3
}
```
(Elided: the `modeled_graph` sentence and the full OIDs. `matches_head: null` is the *expected*
value, not a fault — see "Reading the response envelope" below.)

**`graph_query` has two output channels and they are not interchangeable.** A `RETURN path` query
populates `paths` and **omits `rows`**; every other query populates `rows` and omits `paths`
(`tools.rs:987`). `columns` is present in both. Parse the channel you asked for; do not assume
`rows` exists.

The honesty report differs by channel too. The table channel's only over-approximation signal is
whether a **bound edge cell** carries `possible` (`table_has_over_approx_edge`, `tools.rs:1069`,
used at `:1009`), so the same edges can report two different directions depending on what you
project. Both halves run live this cycle over the two `possible` `calls:virtual` edges out of
`rust_sample::virtual_dispatch::make_speak`:

| `RETURN` clause | `approximation.direction` |
|---|---|
| `a.fqn, b.fqn` | `exact` |
| `a, r, b` | `over` (+ `over-approx-candidate-set`) |

If you need the contract to see the confidence of what you matched, **project the edge**. Treating
a projected-property `exact` as evidence the answer is trustworthy is the mistake this asymmetry
sets up.

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
| Read `approximation` before acting on a result — especially an empty one | every tool but `explain`, `search`, `symbols` |

**Depth-parameter traps.** The knob is named `depth` on `callers`/`callees`/`reaches`/`flows_to`/
`flows_from` and `max_depth` on `paths`, and `0` does not mean the same thing on the two surfaces of
this binary:

- CLI `cgx paths A B --depth 0` is **unbounded** (`paths_max_depth`, `crates/cgx-cli/src/main.rs`
  maps `Some(0) => None`). It is the only CLI command where `0` means unlimited.
- MCP `paths` with `"max_depth": 0` returns **nothing** — the `0` goes through as a literal bound
  (`paths_call`, `tools.rs`, no sentinel arm). Verified live: `paths: []`, `total_matched: 0`,
  `approximation.direction: "under"` with `depth-limit`.
- On every other CLI command `--depth 0` returns the **seed symbol only**. The CLI's own `--help`
  text says `0` is unlimited; it is wrong outside `paths`.

Never port a working `--depth 0` between the two surfaces by copying the argument.

---

## Reading the response envelope

Every MCP answer carries session metadata beside the result. Two fields decide whether the result
is safe to act on, and both are easy to parse past.

**`approximation` — which direction this answer can be wrong in.** Present on 9 of the 12 tools;
`explain`, `search` and `symbols` skip it by design (`with_contract` vs `with_session_meta`,
`with_contract` at `tools.rs:553`, `with_session_meta` at `tools.rs:575` — `with_session_meta` is
the one function every graph-backed tool's response passes through, which is why `freshness` has the
wider coverage of the two). Shape:

```json
"approximation": {
  "direction": "exact" | "over" | "under" | "over_under",
  "reasons": [ { "direction": "under", "code": "depth-limit", "detail": "…" } ],
  "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
  "scope": { "searched_edge_kinds": ["calls", …], "confidence_floor": "possible", "max_depth": null }
}
```

- `direction` is a **fold of `reasons[]`**, never computed independently: any `over` reason ⇒ `over`,
  any `under` ⇒ `under`, both ⇒ `over_under`, none ⇒ `exact`.
- `exact` means exact **within `modeled_graph`** — never that the answer is complete about the
  program. The qualifier is a required field precisely so a bare "exact" cannot be emitted.
- `scope` appears **only on a negative/absence answer** (empty result set, `reachable: false`, and
  always on `unused`). It is the field that tells you what an empty answer actually searched. An
  empty result with `"direction": "under"` is **not** a negative finding.
- `over_under` is the one token in the system spelled with an underscore rather than a hyphen.

**`freshness` — which tree the answer was computed over.** Present on 11 of the 12 tools; `coupling`
carries none because it never opens the index (it reads committed git history only). All six keys
are always emitted, `null` where unknown.

`matches_head` is **three-valued, and `null` is the default over MCP.** `cgx index` writes `.cgx/`,
after which the overlay walk (no ignore rules) and the dirty-file count (ignore rules) disagree
about whether the answered-over graph *is* `HEAD`'s tree, and the envelope declines to guess
(`crates/cgx-mcp/src/session.rs:196`). Any repo that has ever been indexed and is queried with the
default `include_dirty: true` reports `matches_head: null` — as the `graph_query` response above
does. Do not treat `null` as an error, and do not write a client that expects a boolean.

Two more distinctions worth holding: `stale` is a plain `bool` meaning "a divergence was actually
established", so `false` is not a clean bill of health on anything unexamined; and
`dirty_files_analyzed` (a property of the graph, ignore rules **not** applied) and
`freshness.dirty_files` (a property of the checkout, ignore rules applied) are expected to differ —
that difference is not a bug.

**Errors carry neither.** A rejected call is a JSON-RPC error object with no `structuredContent`, so
there is no envelope to read on the failure path.

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
the entire reachable subgraph. Four of the five are populated; **`panic` is in the schema but is
not emitted by any frontend today**, so filtering on it returns an empty set that means nothing —
see `recipes/failure-paths.md`.

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
the ones that pass arguments and must be updated.

**`--confidence` is a minimum floor, and raising it drops candidates.** `--confidence probable`
means "confidence ≥ probable", which **excludes** the `possible` edges that dynamic dispatch
produces — the opposite of widening the net. Verified on the shipped Rust fixture, whose
`make_speak(&dyn Speak)` resolves to two `possible` candidates:

```
$ cgx callees rust_sample::virtual_dispatch::make_speak --depth 1
rust_sample::virtual_dispatch::make_speak  src/virtual_dispatch.rs:36
├─ rust_sample::virtual_dispatch::Cat::speak  src/virtual_dispatch.rs:24  [possible]
└─ rust_sample::virtual_dispatch::Dog::speak  src/virtual_dispatch.rs:18  [possible]

$ cgx callees rust_sample::virtual_dispatch::make_speak --depth 1 --confidence probable
rust_sample::virtual_dispatch::make_speak  src/virtual_dispatch.rs:36
approximation: under-approximate — 2 edge(s) below the confidence>=probable floor were excluded from the search | scope: call edges, confidence>=probable, depth<=1
```
(Elided from both: the trailing `freshness:` line.)

For a refactor you want **no** floor: an unlisted call site is a compile error later. Use the floor
in the other direction — `--confidence certain` — only when you are deliberately trading recall for
precision, and read the `under-approximate` line it produces.

**Reading the result:** Each returned caller is a call site that must be updated. The total count
tells you the refactor scope before you begin. `caller.kind` distinguishes test functions from
production code.

Note: confidence banding does **not** require SCIP. A plain `cgx index .` produces `certain` edges
for direct, unambiguous calls; SCIP enrichment upgrades *ambiguous* ones, it is not a prerequisite
for `certain`.

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
| MCP token-efficiency (compact symbol IDs, resource_link, lazy schema) | full token-efficiency features | v0.5 |

**Note on structural dataflow (available today — Since: v0.3):** `DATA_FLOW` edges and
`:DATA_FLOW` CQL queries work today. `flows_to` / `flows_from` exist on **both** surfaces — CLI
`cgx flows-to` / `cgx flows-from`, MCP `flows_to` / `flows_from` — and traverse `derives-from` edges
to show which values a value node flows into or derives from. These operate on SSA value-node FQNs
(e.g. `fn::local#1`), not on function FQNs; a function FQN has no `derives-from` edges and returns
empty.

**Workaround for security-typed taint questions (deferred):** Use `cgx paths <from> <to>` to
check call reachability (whether a call path exists), and `cgx query` with `:DATA_FLOW` edges to
trace structural data flow. This answers "can control flow/data flow reach the sink" — not "is
the flow tainted and unsanitized". Security-typed taint with source/sink/sanitizer classification
is deferred beyond v0.3.

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
