# cgx query — CQL Reference

**Audience:** AI agents and engineers writing or debugging `cgx query` expressions.

Run `cgx --version` first. The `query` subcommand ships in **v0.1**, but the
language is split across versions. Gate per clause, not per subcommand. See
`reference/versions.md` for the full version ladder.

---

## Verdict

`cgx query` **runs in v0.1** and dispatches CQL to the live graph. The
**CALLS-graph subset** of CQL produces results in v0.1. `DATA_FLOW` edges
return real rows at v0.3 and are **on by default as of SC6** — a plain
`cgx index` suffices; use `cgx index --no-dataflow` or `[index] data_flow = false`
in `cgx.toml` to produce a base/CALLS-only index. Most other deferred clauses in
`docs/05-queries.md` still error with exit 2 or return empty. The table below
is the gate:

| CQL surface | Since | Status |
|---|---|---|
| `MATCH (a)-[:CALLS]->(b) … WHERE … RETURN … LIMIT` | v0.1 | **runs** |
| Node props: `name`, `kind`, `file`, `line` | v0.1 | **runs** |
| Edge props: `condition`, `confidence` | v0.1 | **runs** |
| `IN […]`, `<>` comparisons | v0.1 | **runs** |
| `ANY` / `NONE` quantifiers with **bounded** `*N` | v0.1 | **runs** |
| `@file.cql` query files | v0.1 | **runs** |
| `--at <REF>` historical graph pin | v0.1 | **runs** |
| `--format human\|json\|sarif\|dot\|mermaid\|d2` | v0.1 | **runs** |
| `--confidence possible\|probable\|certain` discriminates | v0.2 | **runs** |
| `DATA_FLOW` edge type | v0.3 | **runs** (on by default as of SC6); use `cgx index --no-dataflow` to build a base index that returns empty for this edge type |
| `MUST PASS THROUGH` / `AVOIDING` | v0.3 | deferred — error |
| Path-set algebra (`COMPLEMENT`/`INTERSECT`/`DIFFERENCE`) | v0.3 | deferred |
| `CALLS:super`, `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` | v0.3 | deferred — error |
| `CALL cgx.pedigree(…)` / `cgx.mutation_fanout(…)` real rows | v0.3 | deferred — stub only |
| `entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class`, `taint_label` props | v0.3 | deferred — plan error exit 2 |
| `NOT IN […]` | v0.3 | parse error exit 2 — use `NOT x = …` |

---

## Part 1 — Runs today (v0.1): the CALLS-graph subset

`[:CALLS]` is the only **fully-populated** edge in v0.1 — almost every runnable query
traverses it. Two adjacent facts, both verified against the binary:

- **Node labels parse and run:** `MATCH (m:method)-[:CALLS]->(b) …` is accepted (a label
  is fine as long as the pattern still contains a relationship).
- **A few structural edges also parse and run but are *sparsely populated*** in v0.1 — e.g.
  `[:OVERRIDES]` executes (exit 0) and returns rows only for languages with explicit
  override syntax (empty otherwise). Treat them as runnable-but-thin, not as full analysis.
- **The `CALLS:<subtype>` qualifier is NOT supported** (`[:CALLS:super]`, `[:CALLS:virtual]`
  → exit 2, "deferred, Theme-13"). Match the family with plain `[:CALLS]`.

### Shell quoting rule

Wrap the entire query in **single quotes**. Use **double quotes** for string
values inside the query. Single quotes inside a query string cause a parse error.

```bash
# correct
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_fn" RETURN a.name LIMIT 10'

# wrong — single quotes inside the query
cgx query "MATCH (a)-[:CALLS]->(b) WHERE b.name = 'my_fn' RETURN a.name"
```

### Required: MATCH must contain a relationship

A node-only MATCH is illegal and fails with a plan error (exit 2):

```cypher
-- illegal — no relationship
MATCH (f) RETURN f

-- legal — relationship required
MATCH (a)-[:CALLS]->(b) RETURN a.name LIMIT 10
```

### Always bound var-length `CALLS*` / `DATA_FLOW*`

A bare `*` (`MATCH (a)-[:CALLS*]->(b)`) used to hang by walking the full transitive
closure from every anchor — on a dense `DATA_FLOW` graph this was the ~1.87M-row /
~71s blowup. As of v0.3 an unbounded `*` is **capped by default**: depth 8 hops and
~1024 result rows, after which the walk *truncates* and surfaces a `PathCap`
truncation marker (it no longer hangs and never silently errors).

The cap is a safety net, not a substitute for a real bound — it drops results past
the cap. Always write the bound you mean, and raise it explicitly when you need more
reach (an explicit `*N..M` overrides the default):

```cypher
-- runs, but depth/row-capped at the default; deeper results are truncated
MATCH (a)-[:CALLS*]->(b) RETURN a.name

-- correct — explicit bound
MATCH (a)-[:CALLS*3]->(b) RETURN a.name LIMIT 10

-- escape hatch — explicit range raises the default cap
MATCH (a)-[:DATA_FLOW*1..12]->(b) RETURN a.name
```

### Supported node properties

| Property | Type | Example value |
|---|---|---|
| `name` | string | `"crypto::hash"` |
| `kind` | string | `"function"`, `"method"`, `"type"`, `"field"`, `"module"` |
| `file` | string | `"src/crypto.rs"` |
| `line` | integer | `42` |

### Supported edge properties (on `CALLS` edges)

| Property | Values |
|---|---|
| `condition` | `"always"`, `"conditional"`, `"exception"`, `"loop"`, `"panic"` |
| `confidence` | `"certain"`, `"probable"`, `"possible"` |

### Copy-pastable runnable examples

**Example 1 — All direct callers of a function**

```bash
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_module::my_fn" RETURN a.name, a.file, a.line LIMIT 20'
```

**Example 2 — Filter by node kind and edge condition**

```bash
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE a.kind = "function" AND r.condition = "exception" RETURN a.name, b.name LIMIT 10'
```

**Example 3 — Multi-hop bounded traversal with ANY quantifier**

```bash
cgx query 'MATCH path = (a)-[:CALLS*3]->(b) WHERE ANY(r IN relationships(path) WHERE r.condition = "exception") RETURN a.name, b.name LIMIT 5'
```

**Example 4 — NONE quantifier to exclude exception-path callers; historical graph**

```bash
cgx query 'MATCH path = (a)-[:CALLS*2]->(b) WHERE b.name = "db::write" AND NONE(r IN relationships(path) WHERE r.condition = "exception") RETURN a.name LIMIT 10' --at HEAD~1
```

**Example 5 — Query from file**

```bash
cgx query @/path/to/query.cql
```

Contents of `query.cql`:

```cypher
MATCH (a)-[r:CALLS]->(b)
WHERE r.condition IN ["exception", "panic"]
  AND b.kind = "function"
RETURN a.name, a.file, a.line, b.name
LIMIT 25
```

### NOT IN is unsupported — use NOT x = …

```cypher
-- parse error exit 2
WHERE a.kind NOT IN ["method", "function"]

-- correct
WHERE NOT a.kind = "method" AND NOT a.kind = "function"

-- also correct inside NONE
MATCH path = (a)-[:CALLS*2]->(b)
WHERE NONE(r IN relationships(path) WHERE NOT r.condition = "exception")
RETURN a.name LIMIT 5
```

### Shared query flags

| Flag | Default | Notes |
|---|---|---|
| `--repo <PATH>` | CWD | Repository root |
| `--at <REF>` | HEAD | Pin query to git ref |
| `--format <FMT>` | human | `human`, `json`, `sarif`, `dot`, `mermaid`, `d2` |
| `--assert-empty` | off | CI: exit 1 if results found |
| `--allow-vacuous` | off | Suppress exit 4 vacuity guard |
| `--no-auto-index` | off | Error (exit 3) if index missing |

`dot`, `mermaid`, and `d2` formats are only meaningful for path-shaped results
(`RETURN path`). See `reference/output-and-exit.md` for exit-code details and
output format samples.

---

## Part 2 — Deferred clauses (v0.3+)

The following clauses and properties appear in `docs/05-queries.md` and the
cookbook. Most produce exit 2 or empty results unless gated correctly. Check
`cgx --version` and the "Since" column in `reference/versions.md` before
using any of them. When the binary rejects a deferred clause it returns exit 2
with a message ending in `(deferred)`.

### Edge types — shipped or still deferred

| Edge type | Since | Notes |
|---|---|---|
| `DATA_FLOW` | v0.3 | Returns real rows (on by default as of SC6); use `cgx index --no-dataflow` or `[index] data_flow = false` in `cgx.toml` to produce a base index that returns empty for this edge type |
| `CALLS:super` | v0.3 | Object-model frontend not yet shipped |
| `RESOLVES_TO` | v0.3 | Same |
| `PROVIDES_BODY` | v0.3 | Same |
| `SHADOWS_FIELD` | v0.3 | Same |
| `FULFILLS` | v0.3 | Same |

### Deferred node and edge properties

| Property | Kind | Since | Effect today |
|---|---|---|---|
| `entrypoint_class` | node | v0.3 | plan error exit 2 |
| `source_class` | node | v0.3 | plan error exit 2 |
| `sink_class` | node | v0.3 | plan error exit 2 |
| `sanitizer_class` | node | v0.3 | plan error exit 2 |
| `taint_label` | edge | v0.3 | plan error exit 2 |
| `via` | edge | v0.3 | plan error exit 2 |

### Deferred query clauses

| Clause | Since | Eventual general form |
|---|---|---|
| `MUST PASS THROUGH` | v0.3 | `MATCH ALL path = (s)-[:CALLS*]->(t) MUST PASS THROUGH (guard {name:"…"}) RETURN path` |
| `AVOIDING` | v0.3 | `MATCH ALL path = (s)-[:CALLS*]->(t) AVOIDING (node {name:"…"}) RETURN path` |
| Path-set algebra (`COMPLEMENT`/`INTERSECT`/`DIFFERENCE`) | v0.3 | Composed `WHERE` predicates over path sets; see `docs/05-queries.md` §Q-21 |
| `CALL cgx.pedigree(value) YIELD source, confidence` | v0.3 | Procedure call returns real provenance rows |
| `CALL cgx.mutation_fanout(value) YIELD mutator, confidence` | v0.3 | Procedure call returns outbound mutation rows |

### Version gate pattern

`DATA_FLOW` requires v0.3. As of SC6 the dataflow index is **on by default** — a plain
`cgx index` is sufficient. To opt out of dataflow indexing, run `cgx index --no-dataflow`
or set `[index] data_flow = false` in `cgx.toml`; a base/CALLS-only index returns empty
for `DATA_FLOW` queries.

```bash
VERSION=$(cgx --version | awk '{print $2}' | cut -d. -f2)
if [ "$VERSION" -ge 3 ]; then
  # A plain 'cgx index' now includes dataflow (on by default since SC6).
  # To disable: cgx index --no-dataflow .
  cgx query 'MATCH (s)-[:DATA_FLOW*1..5]->(t) RETURN s.name, t.name LIMIT 10'
else
  echo "DATA_FLOW requires cgx >= 0.3; current: 0.$VERSION"
fi
```

Note: `sink_class`, `source_class`, and `taint_label` are still deferred (plan
error exit 2). The example above omits them intentionally.

---

## CQL trap summary

| Trap | Effect | Fix |
|---|---|---|
| Unbounded `CALLS*`/`DATA_FLOW*` | Depth/row-capped by default (8 hops, ~1024 rows), truncates with `PathCap` — deeper results dropped | Always bound: `CALLS*3` or `DATA_FLOW*1..8`; raise with explicit `*N..M` |
| Single-quoted strings inside query | Parse error exit 2 | Use double quotes inside; wrap query in single quotes |
| `NOT IN […]` | Parse error exit 2 | Use `NOT x = …` or repeated `AND NOT x = …` |
| Node-only MATCH (no relationship) | Plan error exit 2 | Add `[:CALLS]->` or any valid relationship |
| Deferred props (`entrypoint_class`, `taint_label`, …) | Plan error exit 2 | Still deferred at v0.3; gate on a later taint cycle |
| `graph_query` MCP tool | Always ToolError | Use CLI `cgx query` instead; see `reference/mcp.md` |

Cross-references: `reference/output-and-exit.md` (exit codes, format samples),
`reference/versions.md` (full version ladder), `reference/cli.md` (all
subcommands and shared flags).
