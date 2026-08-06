# cgx query — CQL Reference

**Audience:** AI agents and engineers writing or debugging `cgx query` expressions.

Run `cgx --version` first. The `query` subcommand is available in all versions
(v0.1+) and the language is split across versions. Gate per clause, not per
subcommand. See `reference/versions.md` for the full version ladder.

---

## Verdict

`cgx query` is available in all versions (v0.1+) and dispatches CQL to the live graph. The
**CALLS-graph subset** of CQL produces results in v0.1. `DATA_FLOW` edges
return real rows at v0.3 and are **on by default as of SC6** — a plain
`cgx index` suffices; use `cgx index --no-dataflow` or `[index] data_flow = false`
in `cgx.toml` to produce a base/CALLS-only index. Most other deferred clauses in
`docs/05-queries.md` still error with exit 2 or return empty. The table below
is the gate:

| CQL surface | Since | Status |
|---|---|---|
| `MATCH (a)-[:CALLS]->(b) … WHERE … RETURN … LIMIT` | v0.1 | **runs** |
| Node props: `name` (alias `fqn`), `kind`, `file`, `line` | v0.1 | **runs** |
| Edge props: `condition`, `confidence`, `kind` | v0.1 | **runs** |
| `IN […]`, `<>` comparisons | v0.1 | **runs** |
| `ANY` / `NONE` quantifiers with **bounded** `*N` (anchor required; see note) | v0.1 | **runs** |
| `@file.cql` query files | v0.1 | **runs** |
| `--at <REF>` historical graph pin | v0.1 | **runs** |
| `--format human\|json\|sarif\|dot\|mermaid\|d2` | v0.1 | **runs** |
| `--confidence possible\|probable\|certain` discriminates | v0.2 | **runs** |
| `DATA_FLOW` edge type | v0.3 | **runs** (on by default as of SC6); use `cgx index --no-dataflow` to build a base index that returns empty for this edge type |
| `MUST PASS THROUGH` / `AVOIDING` | v0.3 | deferred — error |
| Path-set algebra (`COMPLEMENT`/`INTERSECT`/`DIFFERENCE`) | v0.3 | deferred |
| `CALLS:super`, `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` | v0.3 | deferred — error |
| `CALL cgx.pedigree(…)` / `cgx.mutation_fanout(…)` real rows | v0.3 | **runs** — see Part 1 |
| `entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class` **node** props | v0.3 | deferred — plan error exit 2 |
| `taint_label`, `via`, `site` **edge** props | v0.3 | deferred — plan error exit 2 |
| `NOT IN […]` | v0.3 | parse error exit 2 — use `NOT x = …` |

---

## Part 1 — Runs today (v0.1+): the CALLS-graph subset

`[:CALLS]` is the only **fully-populated** edge in v0.1 — almost every runnable query
traverses it. Three adjacent facts, all verified against the binary:

- **Node labels parse and run:** `MATCH (m:method)-[:CALLS]->(b) …` is accepted (a label
  is fine as long as the pattern still contains a relationship).
- **A few structural edges also parse and run but are *sparsely populated*** in v0.1 — e.g.
  `[:OVERRIDES]` executes (exit 0) and returns rows only for languages with explicit
  override syntax (empty otherwise). Treat them as runnable-but-thin, not as full analysis.
- **The `CALLS:<subtype>` qualifier is NOT supported** (`[:CALLS:super]`, `[:CALLS:virtual]`
  → exit 2, "deferred, Theme-13"). Match the family with plain `[:CALLS]`.
- **Anchor `path = (a)-[:CALLS*N]->(b)` when N > 1.** The `path = ` form materializes every
  matched path in memory, so on a large index an unanchored `CALLS*2`-or-higher enumerates a
  great deal of work. It does **not** hang — the walk is step-budgeted and truncates with a
  `PathCap`/`StepBudget` marker (see "Always bound var-length" below) — but it returns a
  truncated, non-deterministic slice. Anchor at least one endpoint with
  `WHERE a.fqn = "…"` (or `b.fqn`) so the result is complete and reproducible.

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

A bare `*` (`MATCH (a)-[:CALLS*]->(b)`) used to blow up by walking the full transitive
closure from every anchor — on a dense `DATA_FLOW` graph that produced ~1.87M rows in
~71s. It terminated; it was just unusable. The two caps that fixed it ship on `main`,
**after** the `0.3.0` bump:

- a **depth** cap of 8 hops on a bare `*` (or an open-ended `*N..`);
- a **row** cap of ~1024 on the *peer-set* branch — the form without `path =`, which
  returns a table.

A `0.3.0` binary predating those commits still walks the full closure, and
`cgx --version` cannot tell you which you have. The `path = …` enumeration form is a
different story: it has been budgeted by `DEFAULT_MAX_PATHS`/`DEFAULT_MAX_STEPS` since
before the bump, so it terminates on any `0.3.0` binary.

Two sharp edges on the row cap. An explicit `*N..M` raises or lowers the **depth** cap
only — the ~1024-row cap applies whatever range you write, so `[:CALLS*1..20]` is still
cut at ~1024. And that cap fires on the branch that returns a **table**, which is
precisely the branch whose truncation marker the CLI drops: `cgx query` surfaces
`[truncated]` / `truncation_reason` only for path-returning results. A row-capped table
is therefore indistinguishable from a complete one. Do not read a table of roughly 1024
rows as a complete answer — re-run it with a narrower anchor.

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
| `condition` | `"always"`, `"conditional"`, `"exception"`, `"loop"`, `"panic"`. **`"panic"` is accepted but matches nothing** — the label is schema-only and no edge in any language carries it (`reference/mental-model.md` §3), so a filter on it returns 0 rows at exit 0 |
| `confidence` | `"certain"`, `"probable"`, `"possible"` |
| `kind` | the kebab-case edge-kind token — `"calls"`, `"calls-virtual"`, `"calls-closure"`, `"calls-callback"`, `"calls-async"`, `"calls-indirect"`, `"spawns"`, `"derives-from"`, … (same serde convention as node `kind`) |

### Procedure calls: `cgx.pedigree` and `cgx.mutation_fanout`

Both procedures are in v0.3 — they landed before the `0.3.0` bump, so every `0.3.0` binary has
them and `cgx --version` gates them correctly. (Do not probe for them by calling one and looking
for rows: an argument that matches no value node returns **zero rows**, not a plan error, so an
empty result proves nothing about the binary.) Each takes one string argument naming a value node
and walks `DerivesFrom` from it:

| Procedure | Direction | YIELD columns |
|---|---|---|
| `cgx.pedigree(value)` | backward — what the value derives *from* | `source`, `confidence` |
| `cgx.mutation_fanout(value)` | forward — what derives *from* the value | `mutator`, `confidence` |

```cypher
CALL cgx.pedigree("rust_sample::dataflow::flow_example::b#1") YIELD source, confidence
WHERE confidence = "certain"
RETURN source
```

Notes that matter in practice:

- The walk is depth-capped at 8 hops (the same default bound as an unbounded `*`).
- A `CALL … WHERE` filters the yielded stream, and a yielded column can drive a following `MATCH`.
- An argument matching no node yields zero rows — an honest empty result, not an error.
- Only the columns above exist. `YIELD effect`, `YIELD transform`, and `YIELD evidence` are
  rejected by name at plan time and remain deferred.

### Copy-pastable runnable examples

**Example 1 — All direct callers of a function**

```bash
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_module::my_fn" RETURN a.name, a.file, a.line LIMIT 20'
```

**Example 2 — Filter by node kind and edge condition**

```bash
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE a.kind = "function" AND r.condition = "exception" RETURN a.name, b.name LIMIT 10'
```

**Example 3 — Bounded path traversal with ANY quantifier (anchored)**

```bash
cgx query 'MATCH path = (a)-[:CALLS*1]->(b) WHERE a.fqn = "rust_sample::errors::handle_with_match" AND ANY(r IN relationships(path) WHERE r.condition = "exception") RETURN a.name, b.name LIMIT 5' --repo /path/to/rust-sample
```

Anchor on `a.fqn`: `path = (a)-[:CALLS*N]->(b)` for N ≥ 2 materializes all paths on a large
graph. The walk is budget-bounded so it truncates rather than hanging, but an unanchored result
is an arbitrary truncated slice. Anchor at least one endpoint when using path-variable
quantifiers with `*2` or higher.

**Example 4 — NONE quantifier to exclude exception-path edges; historical graph**

```bash
cgx query 'MATCH path = (a)-[:CALLS*1]->(b) WHERE a.fqn = "rust_sample::conditions::dispatch" AND NONE(r IN relationships(path) WHERE r.condition = "exception") RETURN a.name, b.name LIMIT 10' --at HEAD --repo /path/to/rust-sample
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

The `"panic"` arm of that `IN` list contributes nothing on any repo — the label is schema-only
(`reference/mental-model.md` §3). It is kept here because the exceptional class is defined as both,
and dropping it would make the query stop matching the definition; the rows all come from
`"exception"`.

### NOT IN is unsupported — use NOT x = …

```cypher
-- parse error exit 2
WHERE a.kind NOT IN ["method", "function"]

-- correct
WHERE NOT a.kind = "method" AND NOT a.kind = "function"

-- also correct inside NONE (anchor required for N ≥ 2; use CALLS*1 for unanchored)
MATCH path = (a)-[:CALLS*1]->(b)
WHERE a.fqn = "rust_sample::conditions::dispatch"
  AND NONE(r IN relationships(path) WHERE NOT r.condition = "exception")
RETURN a.name LIMIT 5
```

### Shared query flags

| Flag | Default | Notes |
|---|---|---|
| `--repo <PATH>` | CWD | Repository root |
| `--at <REF>` | HEAD | Pin query to git ref |
| `--depth <N>` | — | **Inert on `query`.** Accepted by the parser and then discarded — a CQL walk is bounded by the query's own `*N..M` syntax (or, for a bare `*`, the engine's default cap). Bound the walk in the query, not on the command line |
| `--format <FMT>` | human | `human`, `json`, `sarif`, `dot`, `mermaid`, `d2` |
| `--confidence <TIER>` | — | **Inert on `query`.** Like `--depth`, parsed and discarded. Filter in the query instead — `WHERE r.confidence IN ["certain", "probable"]` (CQL compares the label, so spell out the tiers you want; there is no `>=` on confidence). It *is* a real floor filter on `callers`/`callees`/`paths`/`reaches`/`flows-*`/`unused` |
| `--tree <SHAPE>` | full | **Inert on `query`** — parsed and discarded. On the neighbor-set commands it selects `full` or `spanning` for the human forest |
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
| `site` | edge | v0.3 | plan error exit 2 (the call-site location is reachable via `cgx explain --format json`) |

### Deferred query clauses

| Clause | Since | Eventual general form |
|---|---|---|
| `MUST PASS THROUGH` | v0.3 | `MATCH ALL path = (s)-[:CALLS*]->(t) MUST PASS THROUGH (guard {name:"…"}) RETURN path` |
| `AVOIDING` | v0.3 | `MATCH ALL path = (s)-[:CALLS*]->(t) AVOIDING (node {name:"…"}) RETURN path` |
| Path-set algebra (`COMPLEMENT`/`INTERSECT`/`DIFFERENCE`) | v0.3 | Composed `WHERE` predicates over path sets; see `docs/05-queries.md` §Q-21. Unlike the two clauses above these have no dedicated interception — they fail as a generic **parse** error, not a "(deferred)" plan error |

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
| Expecting `graph_query` (MCP) to error | It does not — it runs the same `cgx_cql::run` engine the CLI drives | Use it directly; see `reference/mcp.md` |

Cross-references: `reference/output-and-exit.md` (exit codes, format samples),
`reference/versions.md` (full version ladder), `reference/cli.md` (all
subcommands and shared flags).
