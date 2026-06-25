# cgx recipe — Taint & Data Provenance (Theme 3)

**Audience:** AI agents and engineers asking "can tainted input reach this sink?" or "where does this value originate?"

Run `cgx --version` first. A capability tagged `Since: v0.N` requires `MINOR ≥ N`. See `reference/versions.md`.

---

## CRITICAL VERSION BOUNDARY

**In v0.1, cgx answers call reachability — not data flow.**

`cgx reaches SourceFn SinkFn` tells you a call path exists between two functions.
It does **not** tell you that tainted data flows from one to the other.

**As of v0.3, structural dataflow is on by default.** A plain `cgx index .`
now builds SSA value nodes and `DerivesFrom` edges. The structural `DerivesFrom` walk
is available immediately via `flows-to`/`flows-from` subcommands and
`MATCH (a)-[:DATA_FLOW*1..8]->(b)` CQL — no extra flag needed.

To build a base/CALLS-only index without dataflow, use `cgx index --no-dataflow .`
or set `[index] data_flow = false` in `cgx.toml`.

The full taint engine — `source_class`/`sink_class`/`sanitizer_class`/`taint_label` node
and edge properties, `CALL cgx.pedigree(...)`, and `MUST PASS THROUGH`/`AVOIDING`
path-set algebra — remains deferred past v0.3 and errors with exit 2 today.

### Known limitations (v0.3)

Two narrow edge cases produce incomplete results in the current engine:

1. **Method-call arithmetic** (e.g. `a.wrapping_add(b)`): built-in method calls are
   treated as opaque — arguments do not flow through them. `DerivesFrom` edges for
   arithmetic/bitwise builtins are absent. Deferred.
2. **Trailing `//` comment on a tail-expression line**: a `//` comment immediately
   following the final expression of a function can mask that function's return fact,
   causing `flows-to` to miss the return edge from that function. Narrow frontend edge
   case; deferred.

The questions in this theme (Q26–Q36, Q86, Q91, Q94–Q96, Q102) are documented in
`docs/questions/03-provenance-and-taint.md`. That file tags them `answerable-today`; **that tag is wrong
for v0.1.** The skill corrects it: these questions are **Since: v0.3**.

---

## What v0.1 CAN do: call-reachability as a taint approximation

When you know a specific source symbol and a specific sink symbol by exact name, you can ask whether
**any call path** connects them. This is a structural approximation — it proves opportunity, not flow.

### Step 0 — find the exact symbol names first

Use `cgx search` to discover the exact fully-qualified name before running any query.
A wrong name exits 2 with `no symbol matched '<x>'`.

```bash
# Find the exact FQN for a source and a sink
cgx search "handle_request" --repo /path/to/repo --kind function
cgx search "log_info" --repo /path/to/repo --kind function
```

For value nodes (data-flow queries, Since: v0.3), value-node FQNs look like
`module::function::local#ver`. `cgx search` returns them alongside function symbols:

```bash
cgx search "flow_example" --repo /path/to/repo
# rust_sample::dataflow::flow_example       src/dataflow.rs:5  [function]
# rust_sample::dataflow::flow_example::a#0  src/dataflow.rs:5  [variable]
# rust_sample::dataflow::flow_example::b#1  src/dataflow.rs:6  [variable]
# rust_sample::dataflow::flow_example::b#2  src/dataflow.rs:7  [variable]
```

The `[variable]` entries with `#N` suffixes are value nodes. The `[function]` entry has no
`derives-from` edges and returns empty from `flows-to` (exit 0, not error).

### "Does any call path exist from a known source to a known sink?"

**Since: v0.1** | **Status: runnable today (reachability approximation — not taint)**

```bash
# Does a call path exist from the handler to the log call?
cgx reaches rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --repo /path/to/repo

# Show all call paths (up to default depth 6)
cgx paths rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --repo /path/to/repo

# Unlimited depth (work-budgeted) — use with caution on large graphs
cgx paths rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --repo /path/to/repo --depth 0

# JSON output for programmatic use
cgx reaches rust_sample::conditions::dispatch rust_sample::conditions::log_info \
  --repo /path/to/repo --format json
```

**Why this works / breaking it down:** `reaches` performs a graph reachability check in the CALLS graph
and returns a witness path if one exists. `paths` enumerates all call paths up to `--depth` (default 6).

**Reading the result:**
- A result means a call path exists; it does **not** mean tainted data flows on that path.
  The actual argument values may never carry user-controlled data through the call chain.
- An empty result (exit 0, no output) means no call path connects those symbols in the indexed graph —
  but the graph is syntactic, so dynamic dispatch or generated code can introduce paths cgx does not see.
- Confidence labels (`certain`/`probable`/`possible`) reflect call-edge confidence, not taint certainty.
  See `reference/mental-model.md` for the confidence ladder.
- Edge-condition tags show when each hop executes: `[if]` = conditional path, `[exc]` = exception path.
  `always` edges have no tag. A path through `[exc]`-only edges is lower urgency than one on untagged edges.

### "Show callers of a sink that could receive external input"

**Since: v0.1** | **Status: runnable today (call-reachability — not taint)**

```bash
# Who calls the SQL execution function? (substitute the exact FQN from cgx search)
cgx callers MyDb::execute --depth 3 --format json --repo /path/to/repo

# Who calls the log sink?
cgx callers rust_sample::conditions::log_info --depth 5 --repo /path/to/repo

# Who calls a dispatch function?
cgx callers rust_sample::conditions::dispatch --confidence possible --repo /path/to/repo
```

**Why this works / breaking it down:** `callers` traverses the CALLS graph upward from the sink symbol.
The result is all symbols that can reach the sink through the call graph. Cross-reference with your
knowledge of which callers handle user input to identify risk.

**Reading the result:**
- This is a reverse-reachability query, not a taint query. Every caller is returned regardless of
  whether it carries user-controlled data.
- The default depth is **2**. Pass `--depth N` to expand the traversal; `--depth 0` is unlimited
  (work-budgeted, may be slow on large graphs).
- Filter with `--confidence certain` to restrict to calls the graph resolved without ambiguity.

### CQL reachability approximation via `cgx query`

**Since: v0.1** | **Status: runnable today (CALLS graph only)**

When you need a structured query over callers of a sink — for example, to find all functions calling
`log_error` on exception paths — use the CALLS-graph CQL. Always use `.fqn` for exact symbol matches
in `WHERE` predicates; `.name` (short name) does not match in WHERE clauses.

```bash
# Find callers of a sink by exact FQN
# (two separate queries — cgx has no single-query taint path today)

# Step 1: all direct callers of the sink
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.fqn = "rust_sample::conditions::log_info" RETURN a.fqn, a.file, a.line LIMIT 20' \
  --repo /path/to/repo

# Step 2: does the entry point reach any of those callers?
cgx reaches rust_sample::conditions::dispatch rust_sample::conditions::cleanup \
  --repo /path/to/repo

# Exception-path calls into a sink ([exc] edges only)
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE b.fqn = "rust_sample::errors::log_error" AND r.condition = "exception" RETURN a.fqn, a.file, a.line LIMIT 20' \
  --repo /path/to/repo
```

**Why this works / breaking it down:** `MATCH (a)-[:CALLS]->(b) WHERE b.fqn = "…"` finds all
callers of a named symbol in the CALLS graph. Combining two queries manually gives a reachability
approximation; it is not equivalent to a taint path because intermediate data transformations are
invisible to the CALLS graph.

**Reading the result:**
- Double-quote string values inside the query; single-quote the outer shell argument.
- Use `.fqn` for exact-match predicates. `.name` in WHERE returns empty results.
- Always bound multi-hop patterns: `[:CALLS*1..3]`, not bare `[:CALLS*]` (unbounded can be slow).
- Properties `source_class`, `sink_class`, `sanitizer_class`, `taint_label` are **not** available
  in v0.3 — using them produces exit 2 (plan error). See `reference/query-language.md`.

---

## Real taint queries — Since: v0.3

> Check `cgx --version`. These queries require `MINOR ≥ 3`. On v0.1 they return empty or exit 2.

**v0.3 status:** `DATA_FLOW` edges are live at v0.3 and **on by default** — a plain
`cgx index .` suffices. The `flows-to <value-node>` and `flows-from <value-node>` CLI
subcommands surface the forward and backward `DerivesFrom` walks out of the box.
Value-node names have the synthetic FQN form `<fn>::<local>#<ver>`; use
`cgx search <pattern>` to locate them. `flows-to`/`flows-from` accept `--confidence`,
`--depth`, `--tree`, `--at`, `--repo`, `--format`.

Taint source/sink/sanitizer classes (`source_class`, `sink_class`, `sanitizer_class`,
`taint_label`), `MUST PASS THROUGH`/`AVOIDING` path constraints, and the `pedigree`
procedure remain deferred past v0.3. Queries using those properties still produce plan error exit 2.

For detailed CQL syntax — `DATA_FLOW`, `MUST PASS THROUGH`, `AVOIDING`, `ANY`/`NONE` over nodes,
pedigree procedures — see `reference/query-language.md`.

### "Can user input reach a SQL sink without a sanitizer?" (SQL injection)

**Since: v0.3** | **Status: deferred past v0.3 — exits 2 today**

`source_class` and `sink_class` node properties are not yet implemented. This query
pattern is reserved for when security-typed taint lands.

```cypher
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"sql"})
WHERE src.source_class = "network"
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class = "sql")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
```

### "Does decrypted plaintext reach a log sink?" (secret exposure)

**Since: v0.3** | **Status: deferred past v0.3 — exits 2 today**

`sink_class` node property is not yet implemented. This query pattern is reserved
for when security-typed taint lands.

```cypher
MATCH path = (src {name:"decrypt"})-[:DATA_FLOW*]->(sink {sink_class:"log"})
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY src.file, src.line
```

### "Trace the pedigree of a specific parameter" (provenance)

**Since: v0.3** | **Status: deferred past v0.3 — exits 2 today**

`source_class` node property is not yet implemented. For structural (untyped) pedigree
today, use `cgx flows-from <value-node>` directly.

```cypher
-- Where does the 'amount' argument to processPayment() originate?
MATCH flow = (src)-[:DATA_FLOW*1..8]->(sink {name:"amount", scope:"processPayment"})
RETURN src.name, src.file, src.line,
       src.source_class,
       [r IN relationships(flow) | r.transformation_kind] AS transformations
```

### "Does secret key material reach any exposure sink?"

**Since: v0.3** | **Status: deferred past v0.3 — exits 2 today**

`source_class` and `sink_class` node properties are not yet implemented.

```cypher
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"log"})
WHERE src.source_class = "secret"
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "log")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY src.file, src.line
```

Run again replacing `sink_class:"log"` with `sink_class:"format-string"` and `sink_class:"serialize"`
to cover the full exposure-risk surface.

### "Are loop bounds or allocation sizes tainted?" (DoS / overflow-to-alloc)

**Since: v0.3** | **Status: deferred past v0.3 — exits 2 today**

`source_class` and `sink_class` node properties are not yet implemented.

```cypher
MATCH path = (src)-[:DATA_FLOW*]->(sink)
WHERE src.source_class IN ["network", "cli"]
  AND (sink.sink_class = "regex"
       OR sink.name IN ["Vec::with_capacity", "alloc", "malloc", "Box::new"])
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class IN ["regex", "numeric-bound"])
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line, sink.sink_class,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
```

For the integer-overflow-to-alloc variant (numeric narrowing), add:

```cypher
  AND ANY(r IN relationships(path) WHERE r.transformation_kind = "narrow")
```

### Other documented taint questions (all Since: v0.3)

The following questions from the cookbook are documented designs for v0.3. They are not runnable in v0.1:

| Question | What it asks |
|---|---|
| Q26 | Pedigree of the value passed to `log.Info()` in a handler — does it include user-controlled input? |
| Q27 | Does user input reach `eval()` or `reflect.Call()` without type validation? |
| Q28 | What values does `generateToken()` return and which callers log or expose them? |
| Q30 | Which function parameters flow into globally mutable state? |
| Q31 | Does network input reach a serialization sink without schema validation? |
| Q34 | Which functions mutate shared state read by the authentication check? |
| Q86 | All network-to-SQL paths without a class-matched sanitizer (the canonical typed-taint query). |
| Q94 | Pedigree of the nonce/IV passed to a cipher — does it originate from a CSPRNG or attacker data? |
| Q102 | Are there unwrap/deref call sites with no dominating null check on every path? |

The canonical CQL forms for these are in `docs/questions/03-provenance-and-taint.md`. Source-class
and sink-class declarations live in `cgx.toml` in your repo root.

---

## Reading taint results (v0.3 guidance)

These caveats apply once DATA_FLOW edges are available:

- **`source_class` / `sink_class` / `sanitizer_class`** are deferred past v0.3. Queries
  referencing these properties exit 2 today. When they land, they will be declared in `cgx.toml`.
  A sanitizer clears taint only when its class matches the sink class — an HTML-escape sanitizer
  (class `html`) does not clear a SQL-sink path.
- **`transformation_kind`** on each edge shows how the value changed: `copy`, `formatted`, `parsed`,
  `narrow`, `parameterize(sql)`, etc. A `copy` straight to a log sink is a confirmed exposure; a
  `parameterize(sql)` clears SQL injection risk. This edge property is deferred; not yet in v0.3.
- **Confidence** follows the standard ladder (`certain`/`probable`/`possible`). `certain` paths are
  direct assignments; `probable` paths cross function summaries. See `reference/mental-model.md`.
- **`NONE(n IN nodes(path) WHERE n.sanitizer_class = "X")`** is the intended ∀-path "no sanitizer"
  predicate for a single path variable. It is deferred past v0.3 because `sanitizer_class` is not
  yet a supported node property (exit 2). Use `MUST PASS THROUGH`/`AVOIDING` (also deferred) for a
  must-pass assertion across all paths in aggregate.
- Empty results mean no unsanitized paths were found in the indexed graph, not that the code is
  provably safe — dynamic dispatch and generated code may introduce paths the graph does not see.
