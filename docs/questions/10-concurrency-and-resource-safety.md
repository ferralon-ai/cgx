# Theme 10: Concurrency and Resource Safety

Concurrency bugs and resource leaks are notoriously hard to find by reading code:
the problematic interaction exists between execution contexts that never appear
together on the same screen. These six questions use `cgx`'s concurrency model
— spawn edges (GM-9), suspension points (GM-10), lock sets (GM-11), the function
effect system (GM-12), and resource lifecycle pairs (GM-13) — to surface the
patterns that cause data races, deadlocks, and leaked resources. All six questions
are NOVEL: no existing tool answers them at the call-graph level for arbitrary
user-declared constructs. Each query requires the schema-room features to be
populated, so every entry is clearly marked.

---

### Q89 — Is the same value validated before an `await` or file-system call and then used after it, with no re-validation on the resumed path?

**Personas:** PSE · **Status:** schema-room — depends on GM-10 suspension points (`suspends` property on call-site nodes, `schema-room`)

This is the classic time-of-check time-of-use (TOCTOU) pattern adapted for async
code. Between the validation call and the use, an `await` point introduces a
"world-change boundary": the file system or network state may have changed during
the suspension. The query finds paths where validation and use are separated by at
least one suspension point. Specified in docs/05 Q-24 (TOCTOU family).

**The query**

```cgx
-- illustrative: requires GM-10 (schema-room) for the `suspends` property on call-site nodes
MATCH path = (validate)-[:CALLS*]->(use)
WHERE validate.name = $validate_fn
  AND use.name = $use_fn
  AND ANY(n IN nodes(path)
          WHERE n.suspends = true
          AND position_in(n, path) > position_in(validate, path)
          AND position_in(n, path) < position_in(use, path))
RETURN path
```

Run as:

```bash
cgx query @toctou.cql --repo ./
```

Note: CQL query parameters (`$validate_fn`, `$use_fn`) must be substituted as literals directly in the `.cql` file before running — `cgx query` does not accept a `--param` flag in v0.3.0. The query above uses `n.suspends`, which requires GM-10 (schema-room); running it in v0.3.0 produces a plan error and exits 2. Substitute the query only after GM-10 is available.

For a broader sweep — any check-then-use with an `await` between them:

```cgx
-- illustrative: requires GM-10 (schema-room)
MATCH path = (check {name:"validate_path"})-[:CALLS*]->(use {name:"fs::open"})
WHERE ANY(n IN nodes(path) WHERE n.suspends = true)
RETURN check.file, check.line, use.file, use.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `validate.name = $validate_fn` | The validation call — parameterized so the same query covers `fs::metadata`, `access()`, permission-check functions, or any other check. |
| `use.name = $use_fn` | The use site that relies on the validated state. |
| `ANY(n IN nodes(path) WHERE n.suspends = true ...)` | The suspension-point check: at least one node on the path between validate and use has `suspends = true` — meaning an `await` or `yield` occurs there, allowing the runtime to interleave other work. |
| `position_in(n, path) > position_in(validate, path) AND position_in(n, path) < position_in(use, path)` | The suspension must occur strictly between the check and the use — not before the check or after the use. |

**Reading the result** — Each returned path is a potential TOCTOU window. The suspension-point node on the path (where `suspends = true`) is the location of the `await`; the gap between it and the `use` node is the interval where the world can change. Confidence is inherited from the path's weakest edge — `possible` edges from dynamic dispatch reduce certainty.

---

### Q90 — Are there resource-acquire sites from which no release path exists that covers every exception-class edge, including task-cancellation paths?

**Personas:** PSE · **Status:** schema-room — depends on GM-13 resource lifecycle pairs and Q-22 ordering and pairing predicates (`schema-room`, Phase 3 analysis)

Every acquire call — opening a file, locking a mutex, beginning a transaction —
must be followed by a matching release on every execution path, including paths
through exception handlers and task cancellation edges. This is the resource-leak
query: find acquire sites where exception-class paths miss the release. Specified
in docs/05 Q-22 (acquire/release pairing).

**The query**

```cgx
-- illustrative: requires GM-13 (schema-room) resource lifecycle pairs and Q-22 pairing predicates
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
```

Or using the Layer 1 subcommand (checks whether the acquire site reaches a release — does not cover the all-paths exhaustiveness check, which requires the CQL form above):

```bash
cgx paths db::Connection::begin db::Connection::commit --repo ./
```

Note: `cgx paths` takes two positional arguments (`<FROM>` and `<TO>`) and uses `--repo PATH`. The flags `--from`, `--to`, `--quantifier`, `--including-exception-paths`, and `--assert-all-reach-sink` do not exist in v0.3.0. One `<TO>` symbol at a time; to check rollback coverage run a second invocation with `db::Connection::rollback`.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `acquire {name:"db::Connection::begin"}` | The acquire call — the start of the resource's lifecycle. Replace with the relevant acquire symbol for the resource under audit. |
| `exit_node.is_return_site = true` | The path must end at a function exit site, not an arbitrary intermediate node. |
| `NONE(n IN nodes(path) WHERE n.name IN ["db::Connection::commit", "db::Connection::rollback"])` | The leak condition: no commit and no rollback on this path. Any path satisfying this predicate is a path that leaves the resource unreleased. |
| `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) AS on_exception_path` | (CQL form only) Distinguishes leaks that only occur on exception-class paths from leaks on the happy path. `cgx paths` does not have an `--including-exception-paths` flag; exhaustive exception-path coverage requires the CQL query form. |

**Reading the result** — Non-empty results indicate resource leaks. The `on_exception_path` field tells you whether the leak occurs only on error branches (`true`) or on normal execution too (`false`). Happy-path leaks are the highest priority. Exception-path leaks are still real bugs; they are the class that Infer Pulse addresses for known API pairs but does not expose as a user-parameterizable query.

---

### Q97 — Which shared fields are written from two or more spawn-distinct execution contexts under inconsistent lock sets?

**Personas:** PSE · **Status:** schema-room — depends on GM-11 synchronization context and lock sets, GM-9 spawn edges (`schema-room`, Phase 3 analysis)

A data race occurs when two threads write the same field while holding different
locks — or no lock at all. Existing tools like Infer RacerD use a boolean lock
abstraction (some lock is held vs. no lock held) and miss the case where field F
is always accessed under a lock, but different locks in different threads. `cgx`
tracks lock identity, enabling this query. Specified in docs/05 Q-24 (lock-set
consistency family).

**The query**

```cgx
-- illustrative: requires GM-9 (schema-room) spawn edges and GM-11 (schema-room) lock-set attribute
MATCH (a)-[:CALLS*]->(site_a)-[:READS_FIELD|WRITES_FIELD]->(field)
MATCH (b)-[:CALLS*]->(site_b)-[:READS_FIELD|WRITES_FIELD]->(field)
WHERE a.spawn_context <> b.spawn_context
  AND NOT ANY(lock IN site_a.lock_set WHERE lock IN site_b.lock_set)
  AND ((site_a)-[:WRITES_FIELD]->(field) OR (site_b)-[:WRITES_FIELD]->(field))
RETURN field.name, field.file,
       site_a.name AS access_a, site_a.lock_set AS locks_a,
       site_b.name AS access_b, site_b.lock_set AS locks_b
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `a.spawn_context <> b.spawn_context` | The two access sites `site_a` and `site_b` are reachable from different spawn contexts — different threads or tasks. GM-9 spawn edges populate `spawn_context` identities at index time. |
| `NOT ANY(lock IN site_a.lock_set WHERE lock IN site_b.lock_set)` | The inconsistent-lock-set condition: the lock sets at the two access sites share no lock in common. If they shared even one common lock, that lock would be the consistent protection. |
| `(site_a)-[:WRITES_FIELD]->(field) OR (site_b)-[:WRITES_FIELD]->(field)` | At least one of the two accesses is a write. A read-read pair with no writer is not a race; a write from either side with an inconsistent lock set is. |
| `site_a.lock_set AS locks_a, site_b.lock_set AS locks_b` | Return both lock sets so the reviewer can see which locks are being used and understand why they differ. |

**Reading the result** — Each row names a field that two spawn contexts access under different locks. `locks_a` and `locks_b` are the lock sets at each access site — compare them to understand which lock is the intended protection and which context is using the wrong one. Results carry confidence labels; filter to `certain` or `probable` to reduce false positives from conservative alias approximation (see docs/11 Risk 7).

---

### Q104 — Which async functions hold a mutex guard live at an `await` point, and which blocking calls (sync I/O, `thread::sleep`) are reachable from async entrypoints?

**Personas:** SSE · **Status:** schema-room — depends on GM-10 suspension points, GM-11 lock sets, GM-12 function effect system `blocking` effect (`schema-room`, Phase 3 analysis)

Holding a mutex across an `await` point in an async function prevents the executor
from running other tasks that need the same mutex — a potential deadlock. Calling
blocking sync I/O from an async function starves the executor's thread pool.
Clippy's `await_holding_lock` lint detects the first pattern intra-procedurally;
this query extends detection inter-procedurally across async call boundaries.
Specified in docs/05 Q-24 (await-holding-lock and blocking-in-async families).

**The query**

Await-holding-lock (inter-procedural):

```cgx
-- illustrative: requires GM-10 (schema-room) and GM-11 (schema-room)
-- Note: MATCH (site) with no relationship is a plan error in v0.3.0 (exit 2).
-- When GM-10/GM-11 are available this will require a relationship clause, e.g.:
-- MATCH (caller)-[:CALLS]->(site) WHERE site.suspends = true ...
MATCH (caller)-[:CALLS]->(site)
WHERE site.suspends = true
  AND size(site.lock_set) > 0
RETURN site.name, site.file, site.line,
       site.lock_set AS held_locks
ORDER BY site.file, site.line
```

Blocking-in-async:

```cgx
-- illustrative: requires GM-9 (schema-room), GM-10 (schema-room), and GM-12 (schema-room) blocking effect
MATCH path = (ep {kind:"entrypoint", async:true})-[:CALLS*]->(fn)
WHERE "blocking" IN fn.transitive_effects
  AND NONE(n IN nodes(path) WHERE n.name IN ["spawn_blocking",
                                              "tokio::task::spawn_blocking",
                                              "rayon::spawn"])
RETURN fn.name, fn.file, fn.line,
       length(path) AS depth
ORDER BY depth, fn.file
```

Via Layer 1:

These patterns require schema-room properties (GM-10, GM-11, GM-12) and have no dedicated Layer 1 shorthand in v0.3.0. Use `cgx query` with the CQL form above once those schema properties are available. The flag `--concurrency` does not exist.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `site.suspends = true` | The call site is an `await` or `yield` — a suspension point where the executor can run other tasks. |
| `size(site.lock_set) > 0` | At the suspension point, the lock set is non-empty — at least one mutex guard is still live. Holding it means other tasks needing that lock cannot run. |
| `ep {kind:"entrypoint", async:true}` | Start from async entrypoints — functions annotated with `async` or launched as async tasks. |
| `"blocking" IN fn.transitive_effects` | The callee carries the `blocking` effect transitively — it or one of its callees performs sync I/O, calls `thread::sleep`, or does CPU-intensive work that blocks the thread. |
| `NONE(n IN nodes(path) WHERE n.name IN ["spawn_blocking", ...])` | Exclude paths that deliberately dispatch blocking work to a thread pool via `spawn_blocking` — the correct pattern for blocking from async. |

**Reading the result** — Await-holding-lock results name each suspension point where a lock is still held; `held_locks` is the list of lock identities. Blocking-in-async results name each blocking function reachable from an async entrypoint, with `depth` showing how many hops through async calls the block is hidden. Both queries are inter-procedural — Clippy's intra-procedural `await_holding_lock` lint misses chains that span function boundaries.

---

### Q105 — Which functions carry a `writes-global` effect and are callable from two or more spawn-distinct execution contexts without a lock on every path?

**Personas:** SSE · **Status:** schema-room — depends on GM-12 function effect system `writes-global` effect, GM-11 lock sets, GM-9 spawn edges (`schema-room`, Phase 3 analysis)

A function that writes global or shared mutable state without consistent locking
is a data-race hazard when called from multiple threads. The `writes-global` effect
(GM-12) is propagated transitively through the call graph; this query finds
functions with that effect that two or more spawn contexts can call without
guaranteed lock coverage. Specified in docs/05 Q-24 (cross-spawn race family).

**The query**

```cgx
-- illustrative: requires GM-9 (schema-room), GM-11 (schema-room), and GM-12 (schema-room)
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

**Breaking it down**

| Fragment | What it means |
|---|---|
| `spawn_a {edge_type:"spawns"} / spawn_b` | Two distinct spawn sites — the locations where threads or tasks are created. |
| `fn_a = fn_b` | Both spawn contexts reach the same function — confirming shared access to the same global-writing code. |
| `"writes-global" IN fn_a.own_effects` | The function carries the `writes-global` effect label (either syntactically inferred in Phase 1 or propagated transitively from callees in Phase 2). |
| `NOT ANY(lock IN spawn_a.lock_set WHERE lock IN spawn_b.lock_set)` | At neither spawn site does the common lock set include a shared lock — there is no consistent lock protecting the write. |
| `spawn_a.file AS spawn_a_site, spawn_b.file AS spawn_b_site` | Return both spawn site file locations so the reviewer knows which threads are involved. |

**Reading the result** — Each row is a potential global-write data race: the same function with `writes-global` effect reached from two unlocked spawn contexts. The `spawn_a_site` and `spawn_b_site` columns locate the thread creation points. Because lock-set analysis uses a conservative alias approximation, some results may be false positives; filter with `--confidence certain` to reduce noise at the cost of missing some true positives (see docs/11 Risk 7).

---

### Q108 — Which functions annotated or inferred as pure, or reachable only from test entrypoints, transitively reach `nondeterministic` effects (time, random, env reads)?

**Personas:** SSE · **Status:** schema-room — depends on GM-12 function effect system `nondeterministic` effect (`schema-room`, Phase 3 analysis)

A function that claims to be pure but transitively reaches `time.Now()`,
`rand::random()`, or `env::var()` is not actually pure. Similarly, a function
reachable only from test entrypoints should not depend on system clock or
environment state if the tests must be deterministic. The GM-12 `nondeterministic`
effect label, propagated transitively through the call graph, makes this pattern
directly queryable. Specified in docs/05 Q-24 (effect-based queries).

**The query**

```cgx
-- illustrative: requires GM-12 (schema-room) for the `nondeterministic` transitive effect
MATCH path = (fn)-[:CALLS*]->(ndet_fn)
WHERE (fn.is_pure = true OR fn.reachable_from_test_only = true)
  AND "nondeterministic" IN ndet_fn.transitive_effects
RETURN fn.name, fn.file, fn.line,
       ndet_fn.name AS nondeterministic_callee,
       ndet_fn.file AS callee_file,
       length(path) AS depth
ORDER BY depth, fn.file
```

For a broader sweep of all functions whose transitively-computed effects include
`nondeterministic` (regardless of pure/test annotations):

```cgx
-- illustrative: requires GM-12 (schema-room)
-- Note: MATCH (fn) with no relationship is a plan error in v0.3.0 (exit 2).
-- When GM-12 is available this will require a relationship clause, e.g.:
-- MATCH (fn)-[:CALLS]->(callee) WHERE ...
MATCH (fn)-[:CALLS]->(callee)
WHERE "nondeterministic" IN fn.transitive_effects
  AND NOT "nondeterministic" IN fn.own_effects
RETURN DISTINCT fn.name, fn.file, fn.line,
       fn.transitive_effects
ORDER BY fn.file, fn.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.is_pure = true OR fn.reachable_from_test_only = true` | The function is annotated or inferred as pure, or is only reachable from test entrypoints — either property creates an expectation of determinism. |
| `"nondeterministic" IN ndet_fn.transitive_effects` | The callee (or one of its callees) carries the `nondeterministic` effect — it reads the system clock, generates random numbers, reads environment variables, or otherwise depends on external non-deterministic state. |
| `NOT "nondeterministic" IN fn.own_effects` | (Second query) Restrict to functions that are themselves innocent but inherit nondeterminism from a callee — the surprising cases. |
| `depth` | Hop count from the function to the nondeterministic callee; shallow is worse (the nondeterminism is close to the surface). |

**Reading the result** — Each row names a function that claims or is assumed to be deterministic but has a transitive call path to a nondeterministic operation. The `nondeterministic_callee` is the specific function introducing the nondeterminism. Review the call chain to determine whether the nondeterminism is intentional (a deliberate source of entropy that was missed in the purity analysis) or accidental (a dependency on external state that should be injected as a parameter).
