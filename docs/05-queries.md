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

## Known Limitations

The query engine reports **graph reachability**, not path feasibility. A path is returned if no edge on it is statically provable to be unreachable; the tool does not perform symbolic execution or SMT solving to rule out infeasible paths. This is an honest soundy approximation: no false negatives for graph-reachable paths, but some returned paths may be infeasible at runtime due to branch conditions not modeled at the call-graph level.

Dynamic dispatch, function pointers, closures, and `dyn Trait` objects are over-approximated (CHA-style candidate sets) and produce edges labeled `possible`. Filter with `--confidence certain` to remove them, with the understanding that some real edges may be excluded.

Reflection, runtime code generation, and foreign-function interface calls are labeled at the edge level and do not produce transitive call edges. The tool marks these as **blind spots** in the `explain` output.
