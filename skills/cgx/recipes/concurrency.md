# Recipe: Concurrency & Resource Safety

**Requires cgx >= 0.3.** Verify with `cgx --version` before use.

The structural queries in this recipe (call-graph reachability, spawn-edge traversal,
`CALLS`/`DATA_FLOW`/`READS_FIELD`/`WRITES_FIELD` edge patterns) run on v0.3.0.
The semantic queries that rely on node properties from the effect system and lock-set
analysis (GM-9 through GM-13) are schema-reserved stubs in v0.3.0 — those queries
exit 2 with a plan error and are marked **deferred** below. See `reference/versions.md`.

---

## What is available in v0.3.0 vs deferred

| Feature | v0.3.0 | Deferred |
|---|---|---|
| Spawn edges (GM-9) — `SPAWNS` edge type | registered, queryable | spawn-context node prop (`spawn_context`) |
| Suspension points (GM-10) | — | `suspends` node prop |
| Lock-set attribute (GM-11) | — | `lock_set` node prop |
| Function effect system (GM-12) | — | `blocking`, `writes-global`, `nondeterministic` props |
| Resource lifecycle pairs (GM-13) | — | `is_return_site` node prop |
| Dataflow (`DATA_FLOW` edges, `derives-from`) | yes | — |
| Field edges (`READS_FIELD`, `WRITES_FIELD`) | registered, empty in practice | — |
| `CALLS` traversal | yes | — |

---

## Call-site inventory (heuristic — no lock-set analysis)

Find all direct callers of a known lock-acquire function. This is call-graph
reachability only — it does **not** tell you what lock is held, whether a guard
lives across an `await`, or whether a release exists on every path. Treat results
as a call-site inventory, not a concurrency analysis.

**Since: v0.3.0** (structural only)

Step 0: find the exact symbol name.

```bash
cgx search 'lock' --repo /path/to/repo
```

Step 1: list callers of the acquire function.

```bash
cgx callers 'my_crate::lock::MyMutex::acquire' --depth 3 --format json --repo /path/to/repo
```

This tells you which functions call the acquire method within 3 hops. It cannot
tell you whether those callers `await` while holding the guard.

---

## Cross-thread reachability via spawn edges

Find all functions called transitively from a spawn boundary.
The `SPAWNS` edge type is registered in v0.3.0 and populated when the indexer
detects thread spawn call sites (e.g. `std::thread::spawn`, `tokio::spawn`).

**Since: v0.3.0**

```bash
cgx query 'MATCH (spawner)-[:SPAWNS]->(task) RETURN spawner.name, spawner.file, task.name, task.file LIMIT 20' --repo /path/to/repo
```

To enumerate all symbols reachable from within a spawned task:

```bash
cgx callees 'my_crate::spawn::spawned_task_fn' --depth 0 --repo /path/to/repo
```

(`--depth 0` = unlimited, work-budgeted.)

---

## Data-flow slice through a shared value

Trace how a value produced in one function propagates through the call graph using
`DATA_FLOW` edges. This is structural data-flow provenance, not taint classification.

**Since: v0.3.0**

Step 0: find the value-node FQN with `cgx search`. Value nodes have the form
`module::fn::local#N`.

```bash
cgx search 'shared_state' --repo /path/to/repo
```

Step 1: forward slice.

```bash
cgx flows-to 'my_crate::spawn::producer::result#1' --repo /path/to/repo
```

Step 2: backward slice to see what feeds into the value.

```bash
cgx flows-from 'my_crate::spawn::consumer::input#2' --repo /path/to/repo
```

Or use CQL to query `DATA_FLOW` edges directly:

```bash
cgx query 'MATCH (a)-[:DATA_FLOW]->(b) WHERE a.name = "shared_buf" RETURN a.name, a.file, b.name, b.file LIMIT 20' --repo /path/to/repo
```

---

## Deferred queries (exit 2 on v0.3.0)

The queries below depend on node properties that are schema-reserved in v0.3.0
but not yet populated. Each will exit 2 with a plan error until the backing fields
ship. They are preserved here as the intended form once the schema is complete.

### Are there TOCTOU windows where an `await` separates validation from use?

**Deferred — requires GM-10 `suspends` node property (not in v0.3.0)**

`toctou.cql`:

```cypher
MATCH path = (validate)-[:CALLS*2]->(use)
WHERE validate.name = "fs::metadata"
  AND use.name = "fs::open"
  AND ANY(n IN nodes(path) WHERE n.suspends = true)
RETURN validate.file, validate.line, use.file, use.line
LIMIT 20
```

**Why this works (when `suspends` ships):** the `suspends = true` predicate on
`nodes(path)` identifies `await`/`yield` points between the check and the use —
the window where the validated state can change.

**Reading the result:** each row is a potential TOCTOU window. Confidence is
inherited from the weakest edge on the path; `possible` edges from dynamic dispatch
reduce certainty. See `reference/mental-model.md` for the confidence ladder.

---

### Are there acquire sites with no release on every exception path?

**Deferred — requires GM-13 `is_return_site` node property (not in v0.3.0)**

`resource-leak.cql`:

```cypher
MATCH path = (acquire {name:"db::Connection::begin"})-[:CALLS*2]->(exit_node)
WHERE exit_node.is_return_site = true
  AND NONE(n IN nodes(path)
           WHERE n.name IN ["db::Connection::commit",
                             "db::Connection::rollback"])
RETURN acquire.file, acquire.line,
       exit_node.name, exit_node.file, exit_node.line,
       ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
         AS on_exception_path
ORDER BY on_exception_path DESC, acquire.file
LIMIT 20
```

**Why this works (when `is_return_site` ships):** the `NONE` predicate finds exit
paths with no release node; `r.condition IN ["exception","panic"]` distinguishes
[exc]-path leaks from happy-path leaks.

**Reading the result:** non-empty results are resource leaks. `on_exception_path =
true` means the leak only occurs on error branches; `false` means the happy path
also leaks (higher priority). See `reference/mental-model.md` for edge-condition
labels.

---

### Which shared fields are written from two spawn contexts under inconsistent lock sets?

**Deferred — requires GM-9 `spawn_context` + GM-11 `lock_set` node properties (not in v0.3.0)**

`data-race.cql`:

```cypher
MATCH (a)-[:CALLS*2]->(site_a)-[:WRITES_FIELD]->(field)
MATCH (b)-[:CALLS*2]->(site_b)-[:READS_FIELD|WRITES_FIELD]->(field)
WHERE a.spawn_context <> b.spawn_context
  AND NOT ANY(lock IN site_a.lock_set WHERE lock IN site_b.lock_set)
RETURN field.name, field.file,
       site_a.name AS access_a, site_a.lock_set AS locks_a,
       site_b.name AS access_b, site_b.lock_set AS locks_b
LIMIT 20
```

**Why this works (when props ship):** `a.spawn_context <> b.spawn_context` selects
pairs reachable from different threads/tasks; the `NOT ANY` predicate confirms no
lock is shared between the two access sites.

**Reading the result:** each row names a field accessed from two spawn contexts with
no common lock. Filter with `--confidence certain` to reduce false positives from
conservative alias approximation.

---

### Which async functions hold a mutex guard live at an `await` point, or call blocking sync I/O?

**Deferred — requires GM-10 `suspends`, GM-11 `lock_set`, GM-12 `async`/`transitive_effects` (not in v0.3.0)**

Await-holding-lock (`blocking-in-async.cql`):

```cypher
MATCH path = (ep {kind:"entrypoint", async:true})-[:CALLS*2]->(fn)
WHERE "blocking" IN fn.transitive_effects
  AND NONE(n IN nodes(path) WHERE n.name IN ["spawn_blocking",
                                              "tokio::task::spawn_blocking",
                                              "rayon::spawn"])
RETURN fn.name, fn.file, fn.line,
       length(path) AS depth
ORDER BY depth, fn.file
LIMIT 20
```

**Why this works (when props ship):** `site.suspends = true AND size(site.lock_set) > 0` finds
suspension points where a guard is still live; `"blocking" IN fn.transitive_effects`
propagates the blocking effect across async call boundaries, where Clippy's
intra-procedural `await_holding_lock` lint cannot reach.

**Reading the result:** await-holding-lock rows name each suspension point with
`held_locks` identifying which guards are live. Blocking-in-async rows name each
blocking callee with `depth` indicating how many async hops hide it from the
entrypoint.

---

### Which functions with `writes-global` effect are callable from two or more spawn contexts without a shared lock?

**Deferred — requires GM-9 `spawn_context`, GM-11 `lock_set`, GM-12 `own_effects` (not in v0.3.0)**

`writes-global-race.cql`:

```cypher
MATCH (spawn_a)-[:CALLS*2]->(fn_a)
MATCH (spawn_b)-[:CALLS*2]->(fn_b)
WHERE spawn_a <> spawn_b
  AND fn_a = fn_b
  AND "writes-global" IN fn_a.own_effects
  AND NOT ANY(lock IN spawn_a.lock_set WHERE lock IN spawn_b.lock_set)
RETURN fn_a.name, fn_a.file, fn_a.line,
       spawn_a.file AS spawn_a_site,
       spawn_b.file AS spawn_b_site
LIMIT 20
```

**Why this works (when props ship):** `fn_a = fn_b` confirms both spawn contexts
reach the same function; `"writes-global" IN fn_a.own_effects` selects only
functions that write shared state; the `NOT ANY` lock predicate confirms no common
lock protection exists.

**Reading the result:** each row is a potential global-write data race. The
`spawn_a_site` and `spawn_b_site` columns locate the thread creation points.
Use `--confidence certain` to reduce false positives from alias approximation.

---

### Which functions annotated as pure transitively reach nondeterministic effects?

**Deferred — requires GM-12 `is_pure`/`transitive_effects` node properties (not in v0.3.0)**

`pure-nondeterministic.cql`:

```cypher
MATCH path = (fn)-[:CALLS*2]->(ndet_fn)
WHERE fn.is_pure = true
  AND "nondeterministic" IN ndet_fn.transitive_effects
RETURN fn.name, fn.file, fn.line,
       ndet_fn.name AS nondeterministic_callee,
       ndet_fn.file AS callee_file,
       length(path) AS depth
ORDER BY depth, fn.file
LIMIT 20
```

**Why this works (when props ship):** `fn.is_pure = true` selects functions annotated
or inferred as pure; `"nondeterministic" IN ndet_fn.transitive_effects` matches any
callee that reads the system clock, generates random numbers, or reads environment
variables.

**Reading the result:** each row names a function that claims to be deterministic
but reaches a nondeterministic operation. Shallow `depth` values are highest
priority — the nondeterminism is close to the surface.

---

## CQL notes

- All multi-hop patterns here use bounded `*2`. Raise the bound if your call graph
  is deeper, but always bound — unbounded `CALLS*` hangs. See `reference/query-language.md`.
- Edge types `CALLS` and `DATA_FLOW` are fully operational in v0.3.0.
  `READS_FIELD`, `WRITES_FIELD`, and `SPAWNS` are registered types (exit 0) but
  return empty results until the field-access and spawn-context indexing passes ship.
- Node properties `suspends`, `lock_set`, `spawn_context`, `own_effects`,
  `transitive_effects`, `is_pure`, `is_return_site`, `async` are schema-reserved
  stubs in v0.3.0. Any query using them exits 2 with a plan error. They are
  documented in the deferred sections above so the intended forms are preserved.
- Every `MATCH` pattern must contain at least one relationship clause. A bare
  `MATCH (n) WHERE ...` exits 2 with a plan error.
- Wrap the whole query in single quotes; use double quotes inside for string
  literals. Single quotes inside CQL → parse error.
