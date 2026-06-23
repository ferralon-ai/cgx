# cgx recipe — Taint & Data Provenance (Theme 3)

**Audience:** AI agents and engineers asking "can tainted input reach this sink?" or "where does this value originate?"

Run `cgx --version` first. A capability tagged `Since: v0.N` requires `MINOR ≥ N`. See `reference/versions.md`.

---

## CRITICAL VERSION BOUNDARY

**In v0.1, cgx answers call reachability — not data flow.**

`cgx reaches SourceFn SinkFn` tells you a call path exists between two functions.
It does **not** tell you that tainted data flows from one to the other.

The full taint engine — `DATA_FLOW` edges, `source_class`/`sink_class`/`sanitizer_class`/`taint_label`
node and edge properties, `CALL cgx.pedigree(...)`, and `MUST PASS THROUGH`/`AVOIDING` path-set algebra —
is **Since: v0.3** and is inert or errors with exit 2 today.

The questions in this theme (Q26–Q36, Q86, Q91, Q94–Q96, Q102) are documented in
`docs/questions/03-provenance-and-taint.md`. That file tags them `answerable-today`; **that tag is wrong
for v0.1.** The skill corrects it: these questions are **Since: v0.3**.

---

## What v0.1 CAN do: call-reachability as a taint approximation

When you know a specific source symbol and a specific sink symbol by exact name, you can ask whether
**any call path** connects them. This is a structural approximation — it proves opportunity, not flow.

### Step 0 — find the exact symbol names first

cgx has no search, glob, or fuzzy match. A wrong name exits 2 with
`no symbol matched '<x>'`. Grep the source first:

```bash
# find the exact fully-qualified name for a source and a sink
rg -n "fn handle_request" --type rust
rg -n "fn log_info\|log::info!" --type rust
```

### "Does any call path exist from a known source to a known sink?"

**Since: v0.1** | **Status: runnable today (reachability approximation — not taint)**

```bash
# Does a call path exist from the handler to the log call?
cgx reaches MyModule::handle_request MyModule::log_info

# Show all call paths (up to default depth 6)
cgx paths MyModule::handle_request MyModule::log_info

# Unlimited depth (work-budgeted) — use with caution on large graphs
cgx paths MyModule::handle_request MyModule::log_info --depth 0

# JSON output for programmatic use
cgx reaches MyModule::handle_request MyModule::log_info --format json
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
- Edge-condition labels (`always`/`conditional`/`exception`) show when each hop executes.
  A path through `exception`-only edges is lower urgency than one on `always` edges.

### "Show callers of a sink that could receive external input"

**Since: v0.1** | **Status: runnable today (call-reachability — not taint)**

```bash
# Who calls the SQL execution function?
cgx callers MyDb::execute --depth 3 --format json

# Who calls the log sink?
cgx callers Logger::log_raw --depth 5

# Who calls eval or reflect dispatch?
cgx callers Evaluator::eval --confidence possible
```

**Why this works / breaking it down:** `callers` traverses the CALLS graph upward from the sink symbol.
The result is all symbols that can reach the sink through the call graph. Cross-reference with your
knowledge of which callers handle user input to identify risk.

**Reading the result:**
- This is a reverse-reachability query, not a taint query. Every caller is returned regardless of
  whether it carries user-controlled data.
- Use `--depth` to bound the traversal; the default is unlimited and can be slow on large graphs.
- Filter for `--confidence certain` to restrict to calls the graph resolved without ambiguity.

### CQL reachability approximation via `cgx query`

**Since: v0.1** | **Status: runnable today (CALLS graph only)**

When you need a structured query over callers of a sink — for example, to find all functions calling
`eval` that are themselves reachable from an HTTP handler — you can use the CALLS-graph CQL:

```bash
# Find callers of 'eval' that are themselves reached from 'handle_request'
# (two separate queries — cgx has no single-query taint path today)

# Step 1: all direct callers of the sink
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "eval" RETURN a.name, a.file, a.line LIMIT 20'

# Step 2: does the entry point reach any of those callers?
cgx reaches MyModule::handle_request MyModule::dispatch_command

# Exception-path calls into a sink
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE b.name = "execute" AND r.condition = "exception" RETURN a.name, a.file, a.line LIMIT 20'
```

**Why this works / breaking it down:** `MATCH (a)-[:CALLS]->(b) WHERE b.name = "…"` finds all
callers of a named symbol in the CALLS graph. Combining two queries manually gives a reachability
approximation; it is not equivalent to a taint path because intermediate data transformations are
invisible to the CALLS graph.

**Reading the result:**
- Double-quote string values inside the query; single-quote the outer shell argument.
- Always bound multi-hop patterns: `CALLS*3`, not bare `CALLS*` (unbounded hangs).
- Properties `source_class`, `sink_class`, `sanitizer_class`, `taint_label` are **not** available
  in v0.1 — using them produces exit 2 (plan error). See `reference/query-language.md`.

---

## Real taint queries — Since: v0.3

> Check `cgx --version`. These queries require `MINOR ≥ 3`. On v0.1 they return empty or exit 2.
> The `DATA_FLOW` edge type and all taint node/edge properties are inert until v0.3.

For detailed CQL syntax — `DATA_FLOW`, `MUST PASS THROUGH`, `AVOIDING`, `ANY`/`NONE` over nodes,
pedigree procedures — see `reference/query-language.md`.

### "Can user input reach a SQL sink without a sanitizer?" (SQL injection)

**Since: v0.3** | **Status: spec-only in v0.1**

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

**Since: v0.3** | **Status: spec-only in v0.1**

```cypher
MATCH path = (src {name:"decrypt"})-[:DATA_FLOW*]->(sink {sink_class:"log"})
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY src.file, src.line
```

### "Trace the pedigree of a specific parameter" (provenance)

**Since: v0.3** | **Status: spec-only in v0.1**

```cypher
-- Where does the 'amount' argument to processPayment() originate?
MATCH flow = (src)-[:DATA_FLOW*1..8]->(sink {name:"amount", scope:"processPayment"})
RETURN src.name, src.file, src.line,
       src.source_class,
       [r IN relationships(flow) | r.transformation_kind] AS transformations
```

### "Does secret key material reach any exposure sink?"

**Since: v0.3** | **Status: spec-only in v0.1**

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

**Since: v0.3** | **Status: spec-only in v0.1**

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

- **`source_class` / `sink_class` / `sanitizer_class`** are declared in `cgx.toml`. A sanitizer clears
  taint only when its class matches the sink class — an HTML-escape sanitizer (class `html`) does not
  clear a SQL-sink path.
- **`transformation_kind`** on each edge shows how the value changed: `copy`, `formatted`, `parsed`,
  `narrow`, `parameterize(sql)`, etc. A `copy` straight to a log sink is a confirmed exposure; a
  `parameterize(sql)` clears SQL injection risk.
- **Confidence** follows the standard ladder (`certain`/`probable`/`possible`). `certain` paths are
  direct assignments; `probable` paths cross function summaries. See `reference/mental-model.md`.
- **`NONE(n IN nodes(path) WHERE n.sanitizer_class = "X")`** implements a ∀-path "no sanitizer"
  predicate for a single path variable. Use `MUST PASS THROUGH`/`AVOIDING` (also v0.3) for a
  must-pass assertion across all paths in aggregate.
- Empty results mean no unsanitized paths were found in the indexed graph, not that the code is
  provably safe — dynamic dispatch and generated code may introduce paths the graph does not see.
