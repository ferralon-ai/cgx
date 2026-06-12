# cgx Query Capabilities

**Status:** Draft  
**Audience:** Software engineers, security engineers, AI coding agents  
**Working name:** `cgx` (placeholder; see `README.md`)  
**Cross-references:** `docs/03-code-graph-model.md` (node/edge taxonomy, edge condition labels), `docs/04-dataflow-and-provenance.md` (DF- features), `docs/07-interfaces.md` (CLI UX, output formats)

---

## Overview

`cgx` answers graph questions about source code through a two-layer query interface:

- **Layer 1 — Focused subcommands.** Common questions expressed as dedicated commands with no query-language syntax overhead. Subcommands cover the most frequent patterns: who calls what, what paths connect two points, what is never called.
- **Layer 2 — Full query language.** An escape hatch for questions that do not fit a subcommand. Cypher-subset syntax (ISO GQL-compatible) expressed inline or from a file.

Both layers produce identical result objects: every result node and edge carries file:line provenance and an edge condition label (`always`, `conditional`, `exception`, `loop`, `panic`) from the taxonomy in `docs/03-code-graph-model.md`. The labels `exception` and `panic` together form the exceptional class; query predicates that filter "non-exception paths" exclude both.

---

## Feature List

| ID | Feature | Layer |
|----|---------|-------|
| Q-1 | `callers` subcommand | 1 |
| Q-2 | `callees` subcommand | 1 |
| Q-3 | `paths` subcommand with edge-condition filter | 1 |
| Q-4 | `unused` subcommand | 1 |
| Q-5 | `pedigree` subcommand (value provenance fan-in) | 1 |
| Q-6 | `explain` subcommand | 1 |
| Q-7 | `diff` subcommand (graph diff across commits or branches) | 1 |
| Q-8 | Full query language: `cgx query '<q>' ./` | 2 |
| Q-9 | Query from file: `cgx query @file.cql ./` | 2 |
| Q-10 | SQL alternative: `cgx query --sql 'WITH RECURSIVE ...' ./` | 2 |
| Q-11 | Edge-condition filters on any query | 1 + 2 |
| Q-12 | Path-relative transience predicates | 2 |
| Q-13 | Depth limits | 1 + 2 |
| Q-14 | Negative path constraints (`reaches X WITHOUT passing through Y`) | 2 |
| Q-15 | Reachability matrix queries | 2 |
| Q-16 | Graph diff queries across commit or branch ranges | 1 + 2 |
| Q-17 | Determinism guarantee: stable ordering, query against a specific commit (`--at`) | 1 + 2 |
| Q-18 | Confidence-tier filtering (`--confidence certain|probable|possible`) | 1 + 2 |
| Q-19 | Entrypoint-scoped reachability | 1 + 2 |
| Q-20 | Path quantifiers and must-pass-through (∀-path / must-analysis) | 1 + 2 |
| Q-21 | Path-set algebra (complement, intersection, difference) | 2 |
| Q-22 | Ordering and pairing predicates (A-then-B on all paths; acquire/release) | 1 + 2 |
| Q-23 | Typed taint queries (class-matched source → sink with sanitizer-class clearing) | 1 + 2 |
| Q-24 | Concurrency queries (lock sets, await-holding-lock, blocking-in-async, cross-spawn races) | 1 + 2 |
| Q-25 | Dependency and CVE reachability queries | 1 + 2 |

---

## Layer 1: Focused Subcommands

All subcommands accept a **path** argument (the repository root or subdirectory) as their final positional argument. The graph index is automatically located from that path (see `docs/06-indexing-and-vcs.md`).

### Q-1: callers

Find all symbols that call a given symbol, up to a specified depth.

```
cgx callers <symbol> <path> [--depth N] [--confidence certain|probable|possible]
                             [--edge-condition always|conditional|exception|loop|panic]
                             [--at <ref>] [--format <fmt>]
```

**Examples:**

```bash
# Direct callers of crypto::hash
cgx callers crypto::hash ./

# All callers within 3 hops, only certain edges
cgx callers crypto::hash ./ --depth 3 --confidence certain

# Callers that reach the function through an exception path
cgx callers crypto::hash ./ --edge-condition exception

# Callers as of a specific commit
cgx callers crypto::hash ./ --at abc1234
```

### Q-2: callees

Find all symbols a given symbol calls, transitively to a depth limit.

```
cgx callees <symbol> <path> [--depth N] [--confidence certain|probable|possible]
                             [--edge-condition always|conditional|exception|loop|panic]
                             [--at <ref>] [--format <fmt>]
```

**Examples:**

```bash
# All direct callees of main::process_request
cgx callees main::process_request ./

# Transitive callees up to depth 5
cgx callees main::process_request ./ --depth 5

# Only callees reachable via exception-conditioned edges
cgx callees main::process_request ./ --edge-condition exception
```

### Q-3: paths

Find all call paths between a source and a sink, with optional edge-condition filtering.

```
cgx paths --from <symbol> --to <symbol> <path>
          [--exclude-edge-condition exception|conditional|loop]
          [--only-edge-condition exception]
          [--max-depth N] [--max-paths N]
          [--at <ref>] [--format <fmt>]
```

**Examples:**

```bash
# Non-exception paths from main::foo to vulnerable::bar
cgx paths --from main::foo --to vulnerable::bar ./ --exclude-edge-condition exception

# Only exception paths to vulnerable::bar
cgx paths --from main::foo --to vulnerable::bar ./ --only-edge-condition exception

# Paths with depth limit to keep output manageable
cgx paths --from '**' --to crypto::decrypt ./ --max-depth 8
```

### Q-4: unused

Find symbols with no incoming call edges from the reachable graph rooted at a declared set of entrypoints.

```
cgx unused <path> [--kind fn|method|field|all]
                  [--type <TypeName>]
                  [--entrypoint <symbol>]
                  [--confidence certain|probable]
                  [--format <fmt>]
```

**Examples:**

```bash
# All unreachable functions from declared entrypoints
cgx unused ./

# Unreachable methods on UserService
cgx unused ./ --kind method --type UserService

# Unreachable fields on a specific struct
cgx unused ./ --kind field --type OrderState

# Only certainly-unreachable (no probable/possible callers anywhere)
cgx unused ./ --confidence certain
```

### Q-5: pedigree

Trace all inbound provenance for a value at a named program point: which callers, fields, return values, and transformations populate it. This is the fan-in slice of `docs/04-dataflow-and-provenance.md` DF-* features.

```
cgx pedigree <symbol-or-var> <path>
             [--at-function <fn>]
             [--depth N]
             [--format <fmt>]
```

**Examples:**

```bash
# Where does the value of `user_input` come from in AuthHandler::handle?
cgx pedigree user_input ./ --at-function AuthHandler::handle

# Full provenance tree for config object in init_database
cgx pedigree config ./ --at-function init_database --depth 5
```

### Q-6: explain

Show the full provenance record for a single symbol: its definition location, incoming caller count, outgoing callee count, all edges with their condition labels and confidence tiers, and the index version that produced the answer.

```
cgx explain <symbol> <path> [--at <ref>] [--format <fmt>]
```

**Examples:**

```bash
# Explain what cgx knows about crypto::hash
cgx explain crypto::hash ./

# Explain at a prior commit
cgx explain crypto::hash ./ --at HEAD~5
```

### Q-7: diff

Compute the graph diff between two commits or branches: which edges were added, which were removed, and which symbols changed reachability status.

```
cgx diff --base <ref> [--head <ref>] <path>
         [--calls-to <symbol>]
         [--edge-condition exception|always]
         [--format <fmt>]
```

**Examples:**

```bash
# All edge changes between main and a feature branch
cgx diff --base main --head feature/new-auth ./

# New edges that reach vulnerable::bar on an exception path
cgx diff --base main --head feature/new-auth ./ --calls-to vulnerable::bar --edge-condition exception

# Edge changes between two specific commits
cgx diff --base abc1234 --head def5678 ./
```

---

## Layer 2: Full Query Language

The `cgx query` command accepts a Cypher-subset expression (ISO GQL-compatible) or a file reference. The query language is parsed and evaluated in-process against the indexed graph — no external graph engine is required.

```
cgx query '<expression>' <path> [--at <ref>] [--format <fmt>]
cgx query @file.cql <path> [--at <ref>] [--format <fmt>]
cgx query --sql '<recursive-cte>' <path> [--at <ref>] [--format <fmt>]
```

### Query language syntax

Pattern: `MATCH (a)-[r:EDGE_TYPE*min..max]->(b) WHERE <filter> RETURN <projection>`

**Node properties available:**
- `name` — qualified name (e.g. `"crypto::hash"`)
- `file` — file path relative to root
- `line`, `col` — source location
- `kind` — `"function"`, `"method"`, `"field"`, `"type"`, `"module"`
- `confidence` — `"certain"`, `"probable"`, `"possible"`

**Edge types:**
- `CALLS` — direct or transitive call edge
- `DATA_FLOW` — value flow (from `docs/04-dataflow-and-provenance.md`)
- `MEMBER_OF` — member-of-type relationship

**Edge properties:**
- `condition` — `"always"`, `"conditional"`, `"exception"`, `"loop"`, `"panic"`
- `confidence` — `"certain"`, `"probable"`, `"possible"`
- `via` — specific condition kind for filtering

**Path functions:**
- `nodes(path)` — list of nodes on a path
- `relationships(path)` — list of edges on a path
- `length(path)` — hop count
- `position_in(node, path)` — 0-based index of `node` within `path`; used by the ordering predicates in Q-22 and the TOCTOU query in Q-24
- `last_node(path)` — the final node of `path` (e.g. `last_node(path).is_exit`); used by the acquire/release query in Q-22

**Aggregates over list comprehensions:**
- `MIN([e IN relationships(path) | e.<attr>])` / `MAX(...)` — minimum/maximum over a projected list. For `confidence`, the ordering is `certain > probable > possible`, so `MIN` returns the weakest-link confidence on a path (see Q-25 Worked Example 5).

**Predicates:**
- `NONE(n IN nodes(path) WHERE <condition>)` — negative path constraint
- `ANY(r IN relationships(path) WHERE <condition>)` — path-relative transience check

### Shell quoting convention

Wrap the query in single quotes; use double quotes for string values inside the query:

```bash
cgx query 'MATCH (c)-[:CALLS*1..5]->(fn {name:"foo"}) RETURN c.name, c.file, c.line' ./
```

### Q-9: Query files

For queries longer than one line, use `@file.cql`:

```bash
cgx query @queries/exception-sinks.cql ./
```

Contents of `exception-sinks.cql`:

```cypher
MATCH path = (src)-[:CALLS* {condition: "exception"}]->(sink)
WHERE sink.name IN ["exec", "system", "popen"]
  AND src.kind = "function"
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
ORDER BY src.file, src.line
```

### Q-10: SQL alternative

For v1 implementations that use SQLite storage, a recursive-CTE interface is available:

```bash
cgx query --sql '
  WITH RECURSIVE callers(caller, callee, depth) AS (
    SELECT caller, callee, 1 FROM call_edges WHERE callee = "crypto::hash"
    UNION ALL
    SELECT e.caller, r.callee, r.depth + 1
    FROM call_edges e JOIN callers r ON e.callee = r.caller
    WHERE r.depth < 5
  )
  SELECT DISTINCT caller, depth FROM callers ORDER BY depth, caller
' ./
```

---

## Worked Examples for Every Major Question Class

### Canonical Example 1: Non-exception paths from main::foo to vulnerable::bar

The prompt's first canonical question: find call paths between two points that do not traverse any exception-conditioned edge.

**Subcommand:**

```bash
cgx paths --from main::foo --to vulnerable::bar ./ \
  --exclude-edge-condition exception --exclude-edge-condition panic
```

**Full query:**

```bash
cgx query '
  MATCH path = (src {name:"main::foo"})-[:CALLS*]->(dst {name:"vulnerable::bar"})
  WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN path
' ./
```

The `NONE` predicate implements a negative path constraint (Q-14): the path is only returned when no edge on it carries a label in the exceptional class (`exception` or `panic`). This is a condition on the complete path, not on any individual edge in isolation.

---

### Canonical Example 2: Pedigree of a variable in a function

Find all inbound data sources for a named variable at a specific program point.

**Subcommand:**

```bash
cgx pedigree amount ./ --at-function PaymentProcessor::charge
```

**Full query:**

```bash
cgx query '
  MATCH flow = (src)-[:DATA_FLOW*1..8]->(sink {name:"amount", scope:"PaymentProcessor::charge"})
  RETURN src.name, src.file, src.line,
         [r IN relationships(flow) | r.transformation] AS transformations
' ./
```

Each edge in the `DATA_FLOW` chain carries a `transformation` tag: `identity`, `mapped`, `aggregated`, or `parsed`. The sequence of tags describes how the value was transformed along each hop of its pedigree.

---

### Canonical Example 3: Members never referenced downstream from a given instance

Find methods or fields on a specific type that have no callers reachable from a declared entrypoint.

**Subcommand:**

```bash
cgx unused ./ --kind method --type MyStruct --entrypoint main
```

**Full query:**

```bash
cgx query '
  MATCH (m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
  WHERE NOT (m)<-[:CALLS]-()
  RETURN m.name, m.file, m.line
  ORDER BY m.name
' ./
```

To restrict to "never referenced downstream from a specific instance allocation site":

```bash
cgx query '
  MATCH (alloc {name:"new", type:"MyStruct"})-[:CALLS*]->(m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
  WITH collect(m.name) AS reached
  MATCH (all_m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
  WHERE NOT all_m.name IN reached
  RETURN all_m.name, all_m.file, all_m.line
' ./
```

---

### Canonical Example 4: Which branches introduced calls to vulnerable::bar in the exception path

This question combines graph diff (Q-16) with edge-condition filtering (Q-11).

**Subcommand:**

```bash
# Compare each feature branch against main; show only exception-path edges to the target
cgx diff --base main --head feature/risky-change ./ \
    --calls-to vulnerable::bar \
    --edge-condition exception
```

**Full query (per-branch comparison):**

```bash
cgx query '
  MATCH path = (src)-[:CALLS* {condition: "exception"}]->(dst {name:"vulnerable::bar"})
  WHERE src.introduced_in_branch = "feature/risky-change"
  RETURN src.name, src.file, src.line, length(path) AS hops
' ./
```

To check all branches at once against main:

```bash
cgx diff --base main --head "refs/heads/*" ./ --calls-to vulnerable::bar --edge-condition exception
```

---

### Question class: Reachability from a set of entrypoints

```bash
# Is crypto::decrypt reachable from any HTTP handler entrypoint?
cgx query '
  MATCH (ep {kind:"entrypoint", entrypoint_class:"http"})-[:CALLS*]->(target {name:"crypto::decrypt"})
  RETURN ep.name, ep.file, ep.line
  LIMIT 10
' ./
```

---

### Question class: Paths NOT passing through a sanitizer (negative path constraint, Q-14)

```bash
cgx query '
  MATCH path = (src {entrypoint_class:"http"})-[:CALLS*]->(sink {name:"sql_query"})
  WHERE NONE(n IN nodes(path) WHERE n.name IN ["validate_input", "sanitize", "escape_sql"])
  RETURN src.name, sink.name, length(path) AS hops
' ./
```

---

### Question class: Path-relative transience predicate (Q-12)

An edge `C→D` is exception-transient relative to path P if any upstream edge on P has `condition = "exception"`. The query uses `ANY` over the path's edges upstream of the node of interest:

```bash
cgx query '
  MATCH path = (root {name:"main"})-[:CALLS*]->(target {name:"vulnerable::bar"})
  WHERE ANY(r IN relationships(path) WHERE r.condition = "exception")
  RETURN path
' ./
```

This returns paths where `vulnerable::bar` is exception-transient relative to the path, meaning the call to it is only reached via at least one exception-conditioned edge. Compare with the canonical example 1 which returns paths where it is NOT exception-transient.

---

### Question class: Depth-limited blast radius

```bash
# Callers up to depth 2, callees up to depth 3 — minimal subgraph for editing a function
cgx callers OrderService::submit ./ --depth 2 --format json > callers.json
cgx callees OrderService::submit ./ --depth 3 --format json > callees.json
```

Or as a single subgraph query:

```bash
cgx query '
  MATCH (c)-[:CALLS*1..2]->(fn {name:"OrderService::submit"})-[:CALLS*1..3]->(d)
  RETURN c, fn, d
' ./
```

---

### Question class: Reachability matrix (Q-15)

A matrix of entrypoint classes versus sink classes shows which attack surfaces connect to which dangerous operations.

```bash
cgx query '
  MATCH (ep)-[:CALLS*]->(sink)
  WHERE ep.entrypoint_class IN ["http", "grpc", "cli", "cron"]
    AND sink.sink_class IN ["sql", "shell", "file_write", "network_send", "crypto"]
  RETURN ep.entrypoint_class, sink.sink_class, count(*) AS path_count
  ORDER BY ep.entrypoint_class, sink.sink_class
' ./
```

With `--format csv`, this output is directly usable in a spreadsheet.

---

### Question class: Security regression gate

Find all paths from any symbol to a known-dangerous function; assert none exist for CI:

```bash
cgx paths --from '**' --to dangerous::sink ./ \
    --assert-empty \
    --format sarif > security.sarif
echo $?   # 0 = clean; 1 = paths found (CI fail)
```

See `docs/07-interfaces.md` for the full exit-code contract and `--assert-count` / `--assert-max` variants.

---

### Question class: Graph diff across commit range (Q-16)

```bash
# All edge changes between two commits
cgx diff --base HEAD~10 --head HEAD ./

# New exception-path edges introduced in the last commit
cgx diff --base HEAD~1 --head HEAD ./ --edge-condition exception

# Diff expressed as a query (compare graph at two commits)
cgx query --at HEAD '
  MATCH (a)-[:CALLS {condition:"exception"}]->(b {name:"vulnerable::bar"})
  RETURN a.name, a.file, a.line
' ./ > head.json

cgx query --at HEAD~1 '
  MATCH (a)-[:CALLS {condition:"exception"}]->(b {name:"vulnerable::bar"})
  RETURN a.name, a.file, a.line
' ./ > prior.json
```

---

## Q-17: Determinism Guarantees

`cgx` provides a determinism contract: the same query against the same commit produces identical output on every run.

- **Stable ordering.** All result sets are sorted by `(file_path, line, col)` by default. No hash-map ordering, no pointer-order traversal.
- **Reproducibility via `--at`.** Queries accept `--at <ref>` (commit SHA or branch name) to pin the query to a specific graph snapshot. `--at HEAD` is the default; `--at abc1234` re-queries a prior state.
- **No randomness.** No random tie-breaking. No timestamp in default output. The JSON output includes a `graph_version` hash (commit SHA + index schema version) for audit trails.
- **Same index, same answer.** The graph DB is append-write during indexing and read-only during query. Concurrent reads are safe; writes lock only during index updates.

**Order control flags:**

```
--order file        Sort by (file, line, col) — default, stable
--order alpha       Sort by qualified name
--order discovery   BFS/DFS discovery order (useful for path rendering)
--order relevance   Closest-to-query-root first (depth ascending)
```

---

## Edge-Condition Filter Reference (Q-11)

All subcommands and the query language support filtering on edge condition labels from `docs/03-code-graph-model.md`:

| Label | Meaning | Use case |
|-------|---------|----------|
| `always` | Edge traversed on all paths | Happy-path-only analysis |
| `conditional` | Edge guarded by a data or branch condition | Conditional reachability |
| `exception` | Edge traversed only during exceptional control flow | Exception-path analysis |
| `loop` | Back-edge in a loop | Loop body analysis |

Subcommand flags:

- `--exclude-edge-condition exception` — skip any edge with this condition (finds non-exception paths)
- `--only-edge-condition exception` — require at least one such edge on the path
- `--edge-condition always` — restrict to edges with exactly this label

Query language predicates:

```cypher
-- Exclude exception edges
WHERE NONE(r IN relationships(path) WHERE r.condition = "exception")

-- Require at least one exception edge upstream
WHERE ANY(r IN relationships(path) WHERE r.condition = "exception")

-- Inline edge filter
MATCH (a)-[:CALLS {condition: "always"}]->(b)
```

---

## Confidence-Tier Filtering (Q-18)

Every edge carries a confidence tier from `docs/03-code-graph-model.md`: `certain`, `probable`, `possible`.

```bash
# Only certain edges (statically resolved, no ambiguity)
cgx callers foo::bar ./ --confidence certain

# Certain and probable (excludes over-approximated edges)
cgx callers foo::bar ./ --confidence probable

# All edges including possible (maximum coverage, may include false positives)
cgx callers foo::bar ./ --confidence possible
```

Query language:

```cypher
MATCH (c)-[r:CALLS*1..5 {confidence: "certain"}]->(fn {name:"foo"})
RETURN c.name
```

---

## Entrypoint-Scoped Reachability (Q-19)

Reachability queries can be scoped to a declared entrypoint set rather than the entire graph.

```bash
# Unreachable from any declared entrypoint
cgx unused ./

# Unreachable from HTTP handler entrypoints specifically
cgx unused ./ --entrypoint-class http

# Custom entrypoint root
cgx unused ./ --entrypoint main::start
```

In the query language, entrypoints are nodes with `kind="entrypoint"` and an `entrypoint_class` property set during indexing. Users can declare additional entrypoints:

```bash
cgx index --entrypoint 'my_module::start_server' ./
```

---

## Q-20: Path Quantifiers and Must-Pass-Through (∀-Path / Must-Analysis)

**Status: core-extension** — the ∃-path default already covers "exists a path from A to B"; Q-20 adds the dual must-analysis (∀-path) that security questions almost always require.

### Motivation

The queries throughout this document return paths that *exist* in the graph — standard may-analysis (∃-path). Most security questions need the opposite guarantee:

- "Is there **any** handler that reaches `db.write` **without** passing through `require_admin`?" (authorization bypass)
- "Do **all** paths from an acquire call reach a release call?" (resource leak)
- "Does **every** path to `log_event` pass through `redact_pii`?" (secret exposure)

These are ∀-path (all-paths, must-analysis) questions. The existing `NONE` predicate (Q-14) is one tool for them, but Q-20 introduces first-class subcommand flags and query-language clauses that express must-analysis directly.

### Prior art position

CodeQL's `Dominance.qll` provides `dominates` / `postDominates` predicates and Joern's CPGQL provides `dominates` / `dominatedBy` / `postDominates` / `postDominatedBy` steps. Both operate **intra-procedurally only** — they compute dominator trees within a single function's CFG and cannot express "every call-graph path from any entrypoint to sink S passes through check node N."

CodeQL's `BarrierGuard` / `isSanitizerGuard` is ∃-path-negation at the CFG level ("block flow when a guard is known to hold"), not a ∀-path assertion over the full call graph. Semgrep's sanitizer model similarly over-approximates all paths and cannot guarantee that a sanitizer was traversed on *every* path.

cgx extends the standard may-analysis model with must-analysis primitives operating over the full call graph, filling a gap not addressed by CodeQL's intra-procedural `Dominance.qll`, Joern's per-method `dominates` steps, or Semgrep's sanitizer model.

### Layer 1 — Subcommand flags

The `paths` subcommand gains two new flags:

```
cgx paths --from <symbol> --to <symbol> <path>
          [--must-pass-through <symbol>]    # ∀-path: ALL paths must include this node
          [--avoiding <symbol>]             # ∀-path: ALL paths must NOT include this node
          [--quantifier exists|all]         # default: exists (∃-path); use all for ∀-path
```

`--must-pass-through <symbol>` returns only paths that include the named node; it then asserts (with `--assert-empty` if desired) that no path exists without it.

`--avoiding <symbol>` returns only paths that do NOT include the named node. When used with `--assert-empty`, it acts as a must-pass-through assertion: if any path avoids the node, the assertion fires.

`--quantifier all` returns paths only if **every** path from source to sink satisfies the filter predicates. Combined with `--avoiding`, it answers "no path avoids X."

**Example: authorization bypass — does any handler reach `db.write` without `require_admin`?**

```bash
# Find paths that avoid require_admin (authorization bypass candidates)
cgx paths --from '**::handle' --to db::write ./ \
    --avoiding require_admin \
    --confidence certain \
    --format sarif > authz-bypass.sarif

# Assert there are none (CI gate — exit 1 if any bypass path found)
cgx paths --from '**::handle' --to db::write ./ \
    --avoiding require_admin \
    --assert-empty
```

### Layer 2 — Query language

Two new path-quantifier clauses:

```cypher
-- ∀-path: ALL paths from handler to db::write pass through require_admin
-- (expressed as: no path from handler to db::write avoids require_admin)
MATCH path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
WHERE NONE(n IN nodes(path) WHERE n.name = "require_admin")
RETURN path

-- ∀-path with ALL quantifier (explicit form)
MATCH ALL path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
MUST PASS THROUGH (check {name:"require_admin"})
RETURN path
```

The `MATCH ALL path` + `MUST PASS THROUGH` clause is the explicit ∀-path form. It is syntactic sugar for the `NONE`-based complement at the path-set level; both forms produce identical results.

`AVOIDING` is the complement:

```cypher
MATCH ALL path = (src)-[:CALLS*]->(sink {name:"log_event"})
AVOIDING (redactor {name:"redact_pii"})
RETURN src.name, src.file, src.line
```

When `MATCH ALL ... AVOIDING` returns zero paths, every path passes through the named node — the ∀ guarantee holds.

---

## Q-21: Path-Set Algebra (Complement, Intersection, Difference)

**Status: core-extension** — path-set algebra extends Q-14 (negative path constraints) and Q-20 (quantifiers) to a composable predicate algebra over path sets.

### Overview

A **path set** is the set of all paths in the graph satisfying a `MATCH` pattern. Q-21 provides operations over path sets:

| Operation | Meaning | Query form |
|-----------|---------|-----------|
| Complement | All paths in the graph **not** in set P | `MATCH path = ... WHERE NOT <predicate>` |
| Intersection | Paths in both set P and set Q | `WHERE <predicate-P> AND <predicate-Q>` |
| Difference | Paths in P but not in Q | `WHERE <predicate-P> AND NOT <predicate-Q>` |

These are standard boolean combinations of path predicates — no new syntax is required. Q-21 names the operations explicitly so that query authors reason clearly about which set of paths they are operating on.

### Composing with edge-condition predicates

Path predicates compose with edge-condition filters (Q-11) and quantifiers (Q-20):

```cypher
-- Intersection: paths that are BOTH on the exception class AND avoid the handler
MATCH path = (src {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  AND NONE(n IN nodes(path) WHERE n.name = "error_handler")
RETURN src.name, sink.name, length(path) AS hops

-- Difference: all-paths EXCEPT those that pass through the sanitizer
-- (i.e., the unsafe paths in the complement of the sanitized set)
MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink {sink_class:"sql"})
WHERE NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
RETURN path
```

### Non-exception path complement

The most common Q-21 pattern: restrict to non-exception paths (the complement of the exceptional-class path set):

```cypher
-- Non-exception paths from entrypoint to sink
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
RETURN ep.name, ep.file, ep.line
```

This computes the difference between all paths and the exception-class path set — the "happy path" set from the entrypoint to the sink.

### Composing algebra with Q-20 quantifiers

```cypher
-- All non-exception paths must pass through require_admin
-- (no non-exception path avoids require_admin)
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  AND NONE(n IN nodes(path) WHERE n.name = "require_admin")
RETURN path
-- Zero results = the ∀ guarantee holds on non-exception paths
```

---

## Q-22: Ordering and Pairing Predicates

**Status: core-extension** (A-then-B on all paths and acquire/release pairs); the acquire/release subset is the tractable 2-state case. Full typestate ordering is `roadmap` — see GM-13.4.

### A-then-B on all paths

The ordering predicate `A BEFORE B ON ALL PATHS` asserts that every path reaching B also passed through A before reaching B. This is the ∀-path complement of "B reachable without A" (Q-20):

**Layer 2 — Query language:**

```cypher
-- Assertion: every path from any entrypoint to sensitive::write
-- passes through authz::check before reaching sensitive::write
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(write {name:"sensitive::write"})
WHERE NONE(n IN nodes(path) WHERE n.name = "authz::check"
           AND position_in(n, path) < position_in(write, path))
RETURN path
-- Zero results = ordering guarantee holds
```

The helper `position_in(node, path)` returns the 0-based index of the node in the path. The predicate fires when `authz::check` appears on the path but *after* `sensitive::write` — or does not appear at all.

**Never-B-after-C variant:**

```cypher
-- Assert that no write() call appears after any close() call on any path
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(w {name:"resource::write"})
WHERE ANY(n IN nodes(path) WHERE n.name = "resource::close"
          AND position_in(n, path) < position_in(w, path))
RETURN path
-- Non-empty results = write-after-close found
```

### Acquire/release pairing (resource pairs, GM-13)

The acquire/release predicate checks whether every path from an acquire call reaches a release call — including exception-class paths. This is the Q-22 counterpart to the GM-13 resource-pair graph representation.

**Layer 1 — Subcommand:**

```bash
# Check that every path from db::Connection::begin reaches commit or rollback
cgx paths --from db::Connection::begin \
          --to db::Connection::commit db::Connection::rollback \
          --quantifier all \
          --including-exception-paths \
          <path>

# Assert no leak (exit 1 if any path from begin lacks a commit/rollback)
cgx paths --from db::Connection::begin \
          --to db::Connection::commit db::Connection::rollback \
          --quantifier all \
          --including-exception-paths \
          --assert-all-reach-sink \
          <path>
```

`--including-exception-paths` ensures that exception-class edges (the paths that most commonly miss the release) are included. `--assert-all-reach-sink` exits with code 1 if any path from the acquire call does not reach one of the named release symbols.

**Layer 2 — Query language:**

```cypher
-- Find all paths from acquire that do NOT reach any release
-- on ANY edge condition (including exception and panic paths)
MATCH path = (acquire {name:"db::Connection::begin"})-[:CALLS*]->(any_node)
WHERE NOT ANY(n IN nodes(path)
              WHERE n.name IN ["db::Connection::commit",
                               "db::Connection::rollback"])
  AND (last_node(path).is_exit = true OR length(path) >= 1)
RETURN path
-- Non-empty results = resource leak found (no release on this path)
```

### Prior art note

Meta Infer Pulse detects resource leaks interprocedurally for known API pairs (open/close, malloc/free). CodeQL's `java/input-resource-leak` query checks intra-method resource closure on all paths within a single method. Neither exposes the check as a composable query primitive parameterizable with arbitrary user-declared acquire/release symbols. cgx's pairing predicate generalises this to user-declared pairs, composable with taint and effect queries.

### Typestate ordering (roadmap note)

Full typestate — "no `write()` after `close()`", "every `begin()` reaches exactly one of `commit()` or `rollback()`" — is `roadmap` status (GM-13.4). The acquire/release subset (a 2-state automaton: acquired, released) is the tractable case and is the scope of Q-22.

---

## Q-23: Typed Taint Queries

**Status: core-extension** — builds on DF-11 (typed taint labels and class-matched sanitization) and DF-12 (source and sink classes). Adds the query-layer surface for composing class-matched taint predicates.

### Overview

Standard taint queries (∃-path: "does any source reach any sink without a sanitizer?") are already available via DF-5. Q-23 adds class-aware taint queries: the sanitizer class must match the sink class, taint labels carry source-class and sink-class attributes, and queries can specify both.

### Prior art position

CodeQL's `DataFlow::StateConfigSig` / `TaintTracking::GlobalWithState` allows per-query flow states where `isBarrier(node, state)` clears only a named state. This is functionally equivalent to class-matched sanitizer/sink pairs, but defined per query configuration — there is no cross-query class schema. CodeQL's models-as-data (April 2026) adds extensible `barrierModel` / `barrierGuardModel` predicates for external barrier declarations, still per query category.

Semgrep taint labels (experimental feature) allow sources to carry a `label:` string and sinks to specify `requires:` (boolean over labels), with propagators that can replace labels. Each rule defines its own label strings independently; there is no shared schema across rules.

cgx promotes this to a first-class cross-query class schema: source classes, sanitizer classes, and sink classes are declared once in `cgx.toml` (DF-11.4 / DF-12) and automatically linked. HTML-escaping is a sanitizer of class `html` and clears only taint at `html` sinks, not `sql` sinks. Neither CodeQL nor Semgrep enforces this linkage globally across queries.

### Layer 1 — Subcommand

```bash
# Find all network-source → sql-sink paths without a sql-class sanitizer
cgx paths --from-class network --to-class sql \
          --require-sanitizer-class sql \
          --negate-sanitizer \
          <path>

# Secret-to-log pedigree: secret sources reaching log sinks
cgx paths --from-class secret --to-class log \
          --confidence probable \
          <path>

# Class-matched taint: deserialization sources reaching eval sinks
cgx paths --from-class deserialization --to-class eval \
          <path>
```

The `--require-sanitizer-class <class>` / `--negate-sanitizer` combination finds paths where a sanitizer of the matching class is absent. Omitting `--negate-sanitizer` finds paths where a matching sanitizer is present (useful for auditing sanitizer coverage).

### Layer 2 — Query language

New node properties for taint queries:

- `source_class` — on source nodes, the trust-boundary class (DF-12.1)
- `sink_class` — on sink nodes, the vulnerability class (DF-12.2)
- `sanitizer_class` — on sanitizer nodes, the class this sanitizer clears
- `taint_label` — on data-flow path edges, the propagating taint label (carries `source_class` and `sink_class` attributes)

```cypher
-- Injection: network → sql with no sql-class sanitizer on the path
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"sql"})
WHERE src.source_class = "network"
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class = "sql")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops

-- Class-matched sanitizer present: auditing correct sanitization
MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink {sink_class:"html"})
WHERE ANY(n IN nodes(path)
          WHERE n.sanitizer_class = "html")
RETURN src.name, sink.name
```

### Taint-label predicate on DATA_FLOW edges

```cypher
-- Find paths where the taint label's sink_class does not match
-- the sanitizer_class of any node on the path
-- (cross-context sanitizer misuse: html-escape applied to a sql-destined value)
MATCH path = (src)-[r:DATA_FLOW*]->(sink)
WHERE ANY(edge IN relationships(path) WHERE edge.taint_label.sink_class = "sql")
  AND ANY(n IN nodes(path) WHERE n.sanitizer_class = "html"
                              AND NOT n.sanitizer_class = "sql")
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
RETURN src.name, sink.name, sink.sink_class
```

---

## Q-24: Concurrency Queries

**Status: schema-room** — lock-set (GM-11), suspension points (GM-10), spawn edges (GM-9), and the `blocking` effect (GM-12) must all be populated before these queries execute. The query surface is specified here so that schema decisions in those features can be validated against real query patterns.

### Query families

#### Await-holding-lock

A suspension point (`suspends = true` on the call-site node, GM-10) reached while the lock set is non-empty (GM-11):

**Layer 1:**

```bash
cgx query --concurrency await-holding-lock <path>
```

**Layer 2:**

```cypher
-- Find all call sites that suspend while holding at least one lock
MATCH (site)
WHERE site.suspends = true
  AND size(site.lock_set) > 0
RETURN site.name, site.file, site.line,
       site.lock_set AS held_locks
ORDER BY site.file, site.line
```

**Prior art note:** Clippy's `await_holding_lock` lint detects this pattern intra-procedurally within a single async function. It fires when a `MutexGuard`, `RwLockReadGuard`, or `RwLockWriteGuard` is live in the generator's interior types at an `await` point. cgx's query is inter-procedural: it fires when a lock acquired in a calling function is still in the lock set when a suspension point is reached in a callee.

#### Blocking-in-async

A function with the `blocking` effect (GM-12) reachable from an async entrypoint via `calls:async` or `spawns` edges:

**Layer 1:**

```bash
cgx query --concurrency blocking-in-async <path>
```

**Layer 2:**

```cypher
-- Find blocking functions reachable from any async entrypoint
MATCH path = (ep {kind:"entrypoint", async:true})-[:CALLS*]->(fn)
WHERE "blocking" IN fn.transitive_effects
  AND NONE(n IN nodes(path) WHERE n.name IN ["spawn_blocking",
                                              "tokio::task::spawn_blocking",
                                              "rayon::spawn"])
RETURN fn.name, fn.file, fn.line,
       length(path) AS depth
ORDER BY depth, fn.file
```

The `NONE` predicate excludes paths where the blocking call was intentionally dispatched to a thread pool (the correct pattern for calling blocking code from async).

#### Lock-set consistency (cross-thread)

A field accessed from two or more spawn-distinct contexts whose lock sets do not share a common lock:

```cypher
-- Find fields accessed from multiple spawn contexts with inconsistent lock sets
MATCH (a)-[:CALLS*]->(site_a)-[:READS_FIELD|WRITES_FIELD]->(field)
MATCH (b)-[:CALLS*]->(site_b)-[:READS_FIELD|WRITES_FIELD]->(field)
WHERE a.spawn_context <> b.spawn_context
  AND NOT ANY(lock IN site_a.lock_set WHERE lock IN site_b.lock_set)
  AND (site_a)-[:WRITES_FIELD]->(field) OR (site_b)-[:WRITES_FIELD]->(field)
RETURN field.name, field.file,
       site_a.name AS access_a, site_a.lock_set AS locks_a,
       site_b.name AS access_b, site_b.lock_set AS locks_b
```

**Prior art note:** Meta Infer RacerD uses a boolean lock abstraction — it tracks whether *some* lock is held, not *which* lock — and cannot detect the inconsistent-lock-set pattern where field F is protected by lock A in one thread and lock B in another. cgx's lock-set model (GM-11) tracks lock identity, enabling this query.

#### TOCTOU across a suspension point

A value validated at one call site used at a later call site with a suspension point between them (the `suspends` property from GM-10):

```cypher
-- TOCTOU: value validated, then suspension, then value used again
MATCH path = (validate {name:$validate_fn})-[:CALLS*]->(use {name:$use_fn})
WHERE ANY(n IN nodes(path)
          WHERE n.suspends = true
          AND position_in(n, path) > position_in(validate, path)
          AND position_in(n, path) < position_in(use, path))
RETURN path
```

#### Cross-spawn race (writes-global effect)

Functions with `writes-global` effect reachable from two or more distinct spawn sites without consistent lock protection:

```cypher
MATCH (spawn_a {edge_type:"spawns"})-[:CALLS*]->(fn_a)
MATCH (spawn_b {edge_type:"spawns"})-[:CALLS*]->(fn_b)
WHERE spawn_a <> spawn_b
  AND fn_a = fn_b
  AND "writes-global" IN fn_a.own_effects
  AND NOT ANY(lock IN spawn_a.lock_set WHERE lock IN spawn_b.lock_set)
RETURN fn_a.name, fn_a.file, fn_a.line,
       spawn_a.file AS spawn_a_site,
       spawn_b.file AS spawn_b_site
```

---

## Q-25: Dependency and CVE Reachability Queries

**Status: schema-room** — dependency edges (GM-14.3) must be populated (package, version, ecosystem attributes) before these queries execute. The confidence labels and edge-condition context described here depend on the GM-14 schema.

### Overview

Dependency and CVE reachability queries answer: "Is the vulnerable symbol in `package@version` actually reachable from any entrypoint in this application, on what path, with what confidence, and under what conditions?"

This is function-level reachability-based SCA (Software Composition Analysis), extended with taint context and trust-boundary awareness.

### Prior art position

govulncheck (Go-specific) uses VTA (Variable Type Analysis) from `golang.org/x/tools/go/callgraph/vta` to build a call graph and traces paths from entrypoints to vulnerable symbols. It provides function-level reachability (∃-path) and shows the call stack. It does not provide data-flow taint context (what data reaches the vulnerable function), trust-boundary information, or edge-condition context. VTA falls back to RTA then CHA on timeout. govulncheck is Go-only.

Snyk Reachability and Endor Labs Reachability similarly provide function-level ∃-path reachability for Java, JavaScript/TypeScript, and Python. Neither provides data-flow taint or edge-condition context. The reachability algorithms are not publicly documented.

cgx's CVE reachability adds: multi-language support, edge-condition annotation (is the vulnerable symbol reachable only on exception paths?), taint context (does tainted data from a network source reach the vulnerable symbol?), confidence labels from the GM-5 resolution ladder, and composability with Q-20 must-analysis (does every path to the vulnerable symbol pass through a mitigation check?).

### Layer 1 — Subcommand

```bash
# Is any symbol in serde_json@1.0.x reachable from any entrypoint?
cgx paths --from '**' --to-package serde_json --confidence probable <path>

# CVE-specific: is the named symbol reachable?
cgx paths --from '**' \
          --to 'serde_json::de::from_str' \
          --to-package 'serde_json@>=1.0.0,<1.0.96' \
          --format sarif <path>

# With taint context: does tainted network data reach the vulnerable symbol?
cgx paths --from-class network \
          --to 'vulnerable_lib::parse_header' \
          --confidence probable \
          <path>

# CI gate: fail if any entrypoint reaches the CVE symbol
cgx paths --from '**' --to 'log4j_core::PatternLayout::toSerializable' \
          --assert-empty <path>
```

### Layer 2 — Query language

Dependency attributes from GM-14.3 (`package`, `version`, `ecosystem`) are available on call-edge nodes:

```cypher
-- All entrypoint paths to any symbol in package libfoo, version 2.x
MATCH path = (ep {kind:"entrypoint"})-[r:CALLS*]->(vuln)
WHERE ANY(edge IN relationships(path)
          WHERE edge.dependency.package = "libfoo"
            AND edge.dependency.version STARTS WITH "2.")
RETURN ep.name, vuln.name, vuln.file,
       [e IN relationships(path) | e.confidence] AS confidences,
       [e IN relationships(path) | e.condition] AS conditions

-- CVE triage: reachable, with what edge conditions?
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
RETURN ep.name, ep.file, ep.line,
       length(path) AS hops,
       [e IN relationships(path) | e.condition] AS conditions,
       ANY(e IN relationships(path) WHERE e.confidence = "certain") AS any_certain
ORDER BY length(path)

-- With taint context: does network-tainted data reach the vulnerable symbol?
MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(arg)-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
RETURN src.name, src.file, src.line,
       arg.name, vuln.name,
       length(path) AS hops
```

### Confidence labels on dependency paths

Confidence on dependency-crossing edges follows the GM-5 resolution ladder (GM-14.3). When the dependency has a SCIP index, edges are `probable` to `certain`. When the dependency has no SCIP index, edges fall to `probable` (unique name match) or `possible` (ambiguous). All findings include the confidence tier in the result.

```bash
# Only report findings with at least probable confidence (exclude CHA over-approximation)
cgx paths --from '**' --to 'libfoo::parse_header' \
          --confidence probable <path>
```

---

## Security Query Worked Examples

The following six worked examples show both query layers where sensible. They address the security question bank from the design; cross-references name the feature IDs that implement each capability.

---

### Worked Example 1: Authorization Bypass

**Question:** Which handlers reach `db::write` on some path that avoids `require_admin`?

This is the ∀-path authorization bypass pattern: find the complement of "all paths pass through `require_admin`."

**Subcommand:**

```bash
cgx paths --from '**::handle' --to db::write ./ \
    --avoiding require_admin \
    --confidence certain \
    --format sarif > authz-bypass.sarif

# Fail CI if any bypass path exists
cgx paths --from '**::handle' --to db::write ./ \
    --avoiding require_admin \
    --assert-empty
```

**Full query:**

```bash
cgx query '
  MATCH path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
  WHERE NONE(n IN nodes(path) WHERE n.name = "require_admin")
    AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN h.name, h.file, h.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY h.file, h.line
' ./
```

The `NONE` on `require_admin` finds bypass paths (Q-20 / Q-14). The second `NONE` restricts to non-exception paths (Q-21) — exception-path-only bypasses are reported separately to distinguish structural bypasses from error-path bypasses.

---

### Worked Example 2: Resource Leak on Exception-Class Paths

**Question:** Is there any path from `db::Connection::begin` that reaches a function exit without passing through `commit` or `rollback`?

This directly uses Q-22 acquire/release pairing. The critical paths are those in the exceptional class (Q-11, Q-21) — the paths most commonly missing release calls.

**Subcommand:**

```bash
cgx paths --from db::Connection::begin \
          --to db::Connection::commit db::Connection::rollback \
          --quantifier all \
          --including-exception-paths \
          --assert-all-reach-sink \
          ./
```

**Full query:**

```bash
cgx query '
  MATCH path = (acquire {name:"db::Connection::begin"})-[:CALLS*]->(exit_node)
  WHERE exit_node.is_return_site = true
    AND NONE(n IN nodes(path)
             WHERE n.name IN ["db::Connection::commit",
                               "db::Connection::rollback"])
  RETURN acquire.file, acquire.line,
         exit_node.name, exit_node.file, exit_node.line,
         ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
           AS on_exception_path
  ORDER BY on_exception_path DESC, acquire.file
' ./
```

Non-empty results indicate a resource leak. The `on_exception_path` field distinguishes leaks that only occur on error paths (the most common category) from leaks on the happy path.

---

### Worked Example 3: TOCTOU Across a Suspension Point

**Question:** Is there a path where a file or value is validated and then used again after an `await` point?

Uses GM-10 (`suspends` property), Q-20 (ordering predicates), and Q-24 (TOCTOU query family).

**Full query:**

```bash
cgx query '
  MATCH path = (validate)-[:CALLS*]->(use)
  WHERE validate.name = $validate_fn
    AND use.name = $use_fn
    AND ANY(n IN nodes(path)
            WHERE n.suspends = true
            AND position_in(n, path) > position_in(validate, path)
            AND position_in(n, path) < position_in(use, path))
  RETURN path
' ./ --param validate_fn=fs::metadata --param use_fn=fs::read
```

**Variation — any value validated by a check function and then used after an await:**

```bash
cgx query '
  MATCH path = (check {name:"validate_path"})-[:CALLS*]->(use {name:"fs::open"})
  WHERE ANY(n IN nodes(path) WHERE n.suspends = true)
  RETURN check.file, check.line, use.file, use.line
' ./
```

---

### Worked Example 4: Secret-to-Log Pedigree

**Question:** Does any value originating from key material or credentials reach a `log`-class sink?

Uses DF-13 (secret pedigree), DF-11/12 (typed taint), Q-23 (typed taint queries).

**Subcommand:**

```bash
cgx paths --from-class secret --to-class log \
          --confidence probable \
          --format sarif > secret-exposure.sarif
```

**Full query:**

```bash
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"log"})
  WHERE src.source_class = "secret"
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "log")
  RETURN src.name, src.file, src.line,
         sink.name, sink.file, sink.line,
         [e IN relationships(path) | e.transformation_kind] AS transforms,
         length(path) AS hops
  ORDER BY src.file, src.line
' ./
```

The `transformation_kind` sequence on each path shows whether the secret passed through `encode`, `format-string`, or `copy` transformations — useful for triage. A secret that reaches a `log` sink via `concat` is a direct exposure; one via `encode(log)` may be intentionally redacted (if a redact sanitizer class is declared).

---

### Worked Example 5: CVE Reachability Triage

**Question:** Is `libfoo::parse_header` (the vulnerable symbol named in CVE-XXXX-YYYY) actually reachable from any entrypoint in this application?

Uses Q-25 (dependency and CVE reachability), GM-14.3 (dependency edges).

**Subcommand:**

```bash
# Quick reachability check — any entrypoint reaching the vulnerable symbol?
cgx paths --from '**' \
          --to 'libfoo::parse_header' \
          --confidence probable \
          ./

# With edge-condition context — is it reachable only in exception paths?
cgx paths --from '**' \
          --to 'libfoo::parse_header' \
          --only-edge-condition exception \
          --confidence probable \
          ./
```

**Full query:**

```bash
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
  RETURN ep.name, ep.file, ep.line,
         length(path) AS hops,
         [e IN relationships(path) | e.condition] AS conditions,
         MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
  ORDER BY hops, ep.name
  LIMIT 20
' ./
```

The `MIN` over edge confidences gives the weakest-link confidence for the entire path. A path where all edges are `certain` is a confirmed reachable path; one where the weakest edge is `possible` may be a false positive from dynamic dispatch over-approximation.

**With taint context — does attacker-controlled data reach the vulnerable symbol?**

```bash
cgx query '
  MATCH taint_path = (src {source_class:"network"})-[:DATA_FLOW*]->(arg)
  MATCH call_path = (arg)-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
  RETURN src.name, src.file, arg.name,
         vuln.name, vuln.file,
         length(taint_path) + length(call_path) AS total_hops
' ./
```

---

### Worked Example 6: Branch-Diff Security Gate

**Question:** Did this branch introduce any new source → sink path, any new edge into a sensitive sink, or any edge with a newer introducing commit?

Uses Q-16 (graph diff), IX-9 (edge age and author attribution, `docs/06-indexing-and-vcs.md`), Q-20 (quantifiers for regression checking).

**Subcommand (CI gate):**

```bash
# Step 1: Find any new paths from network sources to sql sinks introduced on this branch
cgx diff --base main --head HEAD ./ \
    --new-paths-only \
    --from-class network \
    --to-class sql \
    --assert-empty

# Step 2: Find any new edges into sensitive sinks, annotated with introducing commit
cgx diff --base main --head HEAD ./ \
    --calls-to-sink-class sql \
    --calls-to-sink-class shell \
    --calls-to-sink-class eval \
    --format sarif > new-sink-edges.sarif

# Step 3: General new-edge security gate using IX-9 edge age
cgx query --at HEAD '
  MATCH (a)-[r:CALLS]->(sink)
  WHERE sink.sink_class IN ["sql","shell","eval","log","net-request"]
    AND r.introducing_commit IN $branch_commits
  RETURN a.name, a.file, a.line,
         sink.name, sink.sink_class,
         r.introducing_commit, r.introducing_author
  ORDER BY sink.sink_class, a.file
' ./ --param branch_commits=$(git log main..HEAD --format='%H' | jq -Rs 'split("\n")')
```

**Full query (comparing edge sets between base and head):**

```bash
# At HEAD: new edges since merge-base
cgx query --at HEAD '
  MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink {sink_class:"sql"})
  WHERE NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
  RETURN src.name, sink.name, length(path) AS hops
' ./ > head-findings.json

# At merge-base: baseline findings
cgx query --at $(git merge-base main HEAD) '
  MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink {sink_class:"sql"})
  WHERE NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
  RETURN src.name, sink.name, length(path) AS hops
' ./ > base-findings.json

# Diff the findings sets to identify regressions introduced by this branch
# (new findings in head that were not in base)
```

The `introducing_commit` and `introducing_author` fields on edges (IX-9) let CI annotate new sink-reachable edges with the exact commit and author that introduced them, enabling targeted security review assignments.

---

## Known Limitations

The query engine reports **graph reachability**, not path feasibility. A path is returned if no edge on it is statically provable to be unreachable; the tool does not perform symbolic execution or SMT solving to rule out infeasible paths. This is an honest soundy approximation: no false negatives for graph-reachable paths, but some returned paths may be infeasible at runtime due to branch conditions not modeled at the call-graph level.

Dynamic dispatch, function pointers, closures, and `dyn Trait` objects are over-approximated (CHA-style candidate sets) and produce edges labeled `possible`. Filter with `--confidence certain` to remove them, with the understanding that some real edges may be excluded.

Reflection, runtime code generation, and foreign-function interface calls are labeled at the edge level and do not produce transitive call edges. The tool marks these as **blind spots** in the `explain` output.
