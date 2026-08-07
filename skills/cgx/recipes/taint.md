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
and edge properties and `MUST PASS THROUGH`/`AVOIDING` path-set algebra — remains deferred past
v0.3 and errors with exit 2 today.

**`CALL cgx.pedigree(...)` and `CALL cgx.mutation_fanout(...)` are *not* deferred — both run.**
They are also both direction-swapped relative to their names; read the next section before using
either.

### The two CQL procedures answer the opposite of what they are called

Verified live this cycle on the shipped `fixtures/rust-sample`, whose `dataflow::flow_example` is
the chain `a#0 → b#1 → b#2` (`let b = a; let b = b + 1;`):

| Procedure | YIELD column | What it actually returns | The command that matches it |
|---|---|---|---|
| `cgx.pedigree(v)` | `source` | the values that derive **from** `v` — its **consumers** | `cgx flows-to` |
| `cgx.mutation_fanout(v)` | `mutator` | the values `v` derives from — its **sources** | `cgx flows-from` |

```
$ cgx query 'CALL cgx.pedigree("rust_sample::dataflow::flow_example::a#0") YIELD source RETURN source'
source
rust_sample::dataflow::flow_example::b#1
rust_sample::dataflow::flow_example::b#2

$ cgx query 'CALL cgx.mutation_fanout("rust_sample::dataflow::flow_example::b#2") YIELD mutator RETURN mutator'
mutator
rust_sample::dataflow::flow_example::a#0
rust_sample::dataflow::flow_example::b#1
```
(Elided from both: the `approximation:` and `freshness:` footer lines.)

`pedigree` on the *first* value in the chain returns the two values downstream of it. That is a
forward slice, not a pedigree. The mismatch is between the **column name** and **what the returned
nodes are**, which is why reading the procedure's own module comment does not surface it: in edge
terms the code is self-consistent.

**Use `cgx flows-to` / `cgx flows-from` (or the MCP `flows_to` / `flows_from` tools) for provenance
work.** Their direction is what the rest of the product means by the words. If you must use the
procedures, establish the direction on a two-value fixture first rather than trusting the name — and
re-check it before citing this note, because the fix would most likely swap them.

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
- The default depth is **2**. Pass `--depth N` to expand the traversal. **`--depth 0` on `callers`
  is not unlimited — it returns the seed symbol and nothing else**, with an
  `under-approximate … depth<=0` contract. `paths` is the one CLI command where `0` means unbounded;
  the CLI's own `--help` text over-generalizes that. Verified:

  ```
  $ cgx callers rust_sample::conditions::log_info --depth 0
  rust_sample::conditions::log_info  src/conditions.rs:4
  approximation: under-approximate — search stopped at depth 0; deeper edges were not explored | scope: call edges, confidence>=possible, depth<=0
  freshness: current | indexed tree dd3ea2b, working tree clean
  ```

  To widen a caller search, raise `--depth` to a real number.
- Filter with `--confidence certain` to restrict to calls the graph resolved without ambiguity —
  remembering it is a **minimum floor**, so it drops the `possible` dynamic-dispatch candidates that
  a taint question usually most wants to see.

### CQL reachability approximation via `cgx query`

**Since: v0.1** | **Status: runnable today (CALLS graph only)**

When you need a structured query over callers of a sink — for example, to find all functions calling
`log_error` on exception paths — use the CALLS-graph CQL.

**`.name` and `.fqn` both hold the full FQN, and either matches in `WHERE`.** There is no short-name
property. Verified back-to-back on the shipped fixture: `WHERE b.name = "rust_sample::conditions::log_info"`
and `WHERE b.fqn = "…"` return the same three rows, while `WHERE b.name = "log_info"` — the short
name — returns none. Prefer `.fqn` for readability, but do not expect `.name` to give you short-name
matching, and do not treat a `.name` predicate as broken.

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
- `.fqn` and `.name` are the same string. Match on the **fully-qualified** name either way.
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
`taint_label`) and `MUST PASS THROUGH`/`AVOIDING` path constraints remain deferred past v0.3;
queries using those properties still produce plan error exit 2. The `cgx.pedigree` and
`cgx.mutation_fanout` procedures **run** — see the direction warning at the top of this file.

Worked structural provenance on the shipped fixture, both directions, run twice byte-identical:

```
$ cgx flows-to rust_sample::dataflow::flow_example::a#0 --depth 5
rust_sample::dataflow::flow_example::a#0  src/dataflow.rs:5
└─ rust_sample::dataflow::flow_example::b#1  src/dataflow.rs:6
   └─ rust_sample::dataflow::flow_example::b#2  src/dataflow.rs:7  [probable]
approximation: exact (within modeled graph)
freshness: current | indexed tree dd3ea2b, working tree clean

$ cgx flows-from rust_sample::dataflow::flow_example::b#2 --depth 5
rust_sample::dataflow::flow_example::b#2  src/dataflow.rs:7
└─ rust_sample::dataflow::flow_example::b#1  src/dataflow.rs:6  [probable]
   └─ rust_sample::dataflow::flow_example::a#0  src/dataflow.rs:5
approximation: exact (within modeled graph)
freshness: current | indexed tree dd3ea2b, working tree clean
```

The `[probable]` tag marks the `let b = b + 1;` edge. Untagged edges are `certain`.

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
- **Confidence** follows the standard ladder (`certain`/`probable`/`possible`), and on
  `derives-from` edges it bands the **transformation**, not the function boundary. On the shipped
  fixture the 59 dataflow edges split 41 `certain` / 18 `probable`; every `probable` one is
  intraprocedural — arithmetic (`let b = b + 1`), struct composition (`Point { x: a, y: b }`),
  conditional select (`if c { a } else { b }`). A plain copy is `certain`. Do not read `probable` as
  "crossed a function summary". See `reference/mental-model.md` for the ladder.
- **`NONE(n IN nodes(path) WHERE n.sanitizer_class = "X")`** is the intended ∀-path "no sanitizer"
  predicate for a single path variable. It is deferred past v0.3 because `sanitizer_class` is not
  yet a supported node property (exit 2). Use `MUST PASS THROUGH`/`AVOIDING` (also deferred) for a
  must-pass assertion across all paths in aggregate.
- Empty results mean no unsanitized paths were found in the indexed graph, not that the code is
  provably safe — dynamic dispatch and generated code may introduce paths the graph does not see.
- **Read the `approximation` before acting on an empty answer.** Every `reaches`/`paths`/`callers`/
  `callees`/`flows-to`/`flows-from` answer carries it, in JSON as an object and in human output as
  the second-to-last line. On a negative answer it also carries a `scope` object naming the edge
  kinds, confidence floor and depth actually searched. `"direction": "under"` with an
  `unresolved-external-calls` or `depth-limit` reason means the search stopped short — that empty
  result is not a negative finding. `exact` means exact *within* `modeled_graph`, never that the
  answer is complete about the program.
- **`freshness` is the last line of every answer** (human) or a top-level object (JSON), and tells
  you which tree you actually queried. Over MCP with the default `include_dirty: true`,
  `matches_head` is `null` on any repo that has ever been indexed — the expected value, not a fault.
  A stale index answering a taint question is the failure mode worth checking for first.
