# Theme 10: Concurrency and Resource Safety

Concurrency bugs and resource leaks are notoriously hard to find by reading code:
the problematic interaction exists between execution contexts that never appear
together on the same screen. These six questions use `cgx`'s concurrency model
— spawn edges (GM-9), suspension points (GM-10), lock sets (GM-11), the function
effect system (GM-12), and resource lifecycle pairs (GM-13) — to surface the
patterns that cause data races, deadlocks, and leaked resources. All six questions
are NOVEL: no existing tool answers them at the call-graph level for arbitrary
user-declared constructs. Two of the five underlying mechanisms are not what they
look like below: GM-9 spawn edges (`EdgeKind::Spawns`, real and queryable today via
`-[:SPAWNS]->`) and GM-12's function effect system (a real 7-value lattice —
`Blocking`, `Spawns`, `IoFile`, `IoNet`, `IoProc`, `DynamicCode`, `Nondeterministic`
— computed and stored per function, `crates/cgx-core/src/effect.rs:27-55`) both
ship and are populated at index time. What blocks every query below that reaches
for `own_effects`/`transitive_effects`, `suspends`, `lock_set`, or a per-access
`spawn_context` identity is narrower than "the fact doesn't exist" — it is that
CQL's `lower.rs` does not expose those specific node properties yet (`unknown node
property` / `not supported in this release`), even where the underlying field is
real and populated. GM-10 (suspension points), GM-11 (lock sets), and GM-13
(resource lifecycle pairs) are genuine schema-room: no code anywhere constructs a
populated instance of any of them. Each entry below states which kind of gap it
actually has.

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

**Personas:** PSE · **Status:** schema-room — depends on GM-11 lock-set and per-access spawn-context identity, neither of which any code populates yet

A data race occurs when two threads write the same field while holding different
locks — or no lock at all. Existing tools like Infer RacerD use a boolean lock
abstraction (some lock is held vs. no lock held) and miss the case where field F
is always accessed under a lock, but different locks in different threads. `cgx`
tracks lock identity, enabling this query. Specified in docs/05 Q-24 (lock-set
consistency family).

**The query**

GM-9 spawn edges are not the blocker here — `EdgeKind::Spawns` is real, shipped, and
queryable today via `-[:SPAWNS]->` (`crates/cgx-resolve/src/link.rs:1192`,
`crates/cgx-cql/src/lower.rs:217`). What's missing is `site_a.lock_set` (GM-11, no
populating code anywhere) and `a.spawn_context` (not a field on any node or edge
record at all — `grep -rn spawn_context crates/` is empty):

```cgx
-- illustrative: requires GM-11 (schema-room) lock-set attribute and a per-access
-- spawn-context identity, which does not exist as a field anywhere yet
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
| `a.spawn_context <> b.spawn_context` | The two access sites `site_a` and `site_b` are reachable from different spawn contexts — different threads or tasks. `EdgeKind::Spawns` itself is real and populated at index time; `spawn_context` — a per-node identity naming *which* spawn site an access descends from — is not: it is not a field on any record in `cgx-core` today. |
| `NOT ANY(lock IN site_a.lock_set WHERE lock IN site_b.lock_set)` | The inconsistent-lock-set condition: the lock sets at the two access sites share no lock in common. If they shared even one common lock, that lock would be the consistent protection. |
| `(site_a)-[:WRITES_FIELD]->(field) OR (site_b)-[:WRITES_FIELD]->(field)` | At least one of the two accesses is a write. A read-read pair with no writer is not a race; a write from either side with an inconsistent lock set is. |
| `site_a.lock_set AS locks_a, site_b.lock_set AS locks_b` | Return both lock sets so the reviewer can see which locks are being used and understand why they differ. |

**Reading the result** — Each row names a field that two spawn contexts access under different locks. `locks_a` and `locks_b` are the lock sets at each access site — compare them to understand which lock is the intended protection and which context is using the wrong one. Results carry confidence labels; filter to `certain` or `probable` to reduce false positives from conservative alias approximation (see docs/11 Risk 7).

---

### Q104 — Which async functions hold a mutex guard live at an `await` point, and which blocking calls (sync I/O, `thread::sleep`) are reachable from async entrypoints?

**Personas:** SSE · **Status:** schema-room — depends on GM-10 suspension points and GM-11 lock sets, neither populated by any code today. GM-12's `blocking` effect is a different case: it *is* a real, populated effect label (`crates/cgx-core/src/effect.rs:27`) — what blocks this query is that CQL does not expose `own_effects`/`transitive_effects` as queryable node properties (`unknown node property`, live-verified), not that the effect itself is unbuilt

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
-- illustrative: the `blocking` effect itself ships and is populated (GM-12,
-- effect.rs:27); what's not queryable is fn.transitive_effects (CQL rejects it,
-- "unknown node property") and the async-entrypoint `async` property (GM-10-
-- adjacent, also CQL-rejected)
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

GM-10 (`suspends`) and GM-11 (`lock_set`) are genuine schema-room — no code populates either. GM-12's effect labels are not: they ship and are populated, but `own_effects`/`transitive_effects` are not exposed as queryable CQL properties (`unknown node property`). Neither gap has a dedicated Layer 1 shorthand in v0.3.0. The flag `--concurrency` does not exist.

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

**Personas:** SSE · **Status:** schema-room — `writes-global` is genuinely absent from the shipped 7-value effect lattice (`Blocking, Spawns, IoFile, IoNet, IoProc, DynamicCode, Nondeterministic` — `crates/cgx-core/src/effect.rs:27-42`; no global/shared-state write label exists at all), and GM-11 lock sets are unpopulated. GM-9 spawn edges are not part of the gap — `EdgeKind::Spawns` ships and is queryable via `-[:SPAWNS]->`; what's missing alongside `writes-global` is the per-access `spawn_context` identity this query also needs, which is not a field anywhere

A function that writes global or shared mutable state without consistent locking
is a data-race hazard when called from multiple threads. The `writes-global` effect
(GM-12) is propagated transitively through the call graph; this query finds
functions with that effect that two or more spawn contexts can call without
guaranteed lock coverage. Specified in docs/05 Q-24 (cross-spawn race family).

**The query**

```cgx
-- illustrative: `writes-global` does not exist in the effect lattice (real gap);
-- GM-11 lock sets and a per-access spawn_context identity are unpopulated (real gap).
-- GM-9 itself is not a gap: EdgeKind::Spawns ships — `(spawn_a)-[:CALLS*]->(fn_a)`
-- would need `-[:SPAWNS]->` as its first hop to actually anchor on a spawn site
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
| `"writes-global" IN fn_a.own_effects` | Two independent gaps stacked: `writes-global` is not one of the 7 shipped effect labels at all (no global/shared-state write label exists), and `own_effects` is also not a queryable CQL property today even for the 7 labels that do exist (`unknown node property`). |
| `NOT ANY(lock IN spawn_a.lock_set WHERE lock IN spawn_b.lock_set)` | At neither spawn site does the common lock set include a shared lock — there is no consistent lock protecting the write. |
| `spawn_a.file AS spawn_a_site, spawn_b.file AS spawn_b_site` | Return both spawn site file locations so the reviewer knows which threads are involved. |

**Reading the result** — Each row is a potential global-write data race: the same function with `writes-global` effect reached from two unlocked spawn contexts. The `spawn_a_site` and `spawn_b_site` columns locate the thread creation points. Because lock-set analysis uses a conservative alias approximation, some results may be false positives; filter with `--confidence certain` to reduce noise at the cost of missing some true positives (see docs/11 Risk 7).

---

### Q108 — Which functions annotated or inferred as pure, or reachable only from test entrypoints, transitively reach `nondeterministic` effects (time, random, env reads)?

**Personas:** SSE · **Status:** schema-room — but not because `nondeterministic` is unbuilt. It is the 7th value of the shipped effect lattice (`crates/cgx-core/src/effect.rs:27-42`) and is computed and stored per function like the other six. Two narrower things actually block this query: `own_effects`/`transitive_effects` are not exposed as queryable CQL node properties yet, and `is_pure`/`reachable_from_test_only` are not fields on any node record at all (zero hits anywhere in `crates/`)

A function that claims to be pure but transitively reaches `time.Now()`,
`rand::random()`, or `env::var()` is not actually pure. Similarly, a function
reachable only from test entrypoints should not depend on system clock or
environment state if the tests must be deterministic. The GM-12 `nondeterministic`
effect label is real and is propagated transitively through the call graph at
index time; what makes this pattern not-yet-queryable is CQL's surface, not the
underlying fact. Specified in docs/05 Q-24 (effect-based queries).

**The query**

```cgx
-- illustrative: the `nondeterministic` effect ships and is populated (effect.rs:27).
-- Blocked by two narrower gaps instead: `own_effects`/`transitive_effects` are not
-- queryable CQL node properties (`unknown node property`, live-verified), and
-- `is_pure`/`reachable_from_test_only` are not fields on any node record at all
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
-- illustrative: blocked by `own_effects`/`transitive_effects` not being queryable
-- CQL node properties, not by `nondeterministic` itself (which ships).
-- Note: MATCH (fn) with no relationship is a plan error in v0.3.0 (exit 2) — the
-- relationship clause below is required regardless of the property gap.
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
| `fn.is_pure = true OR fn.reachable_from_test_only = true` | The function is annotated or inferred as pure, or is only reachable from test entrypoints — either property creates an expectation of determinism. Neither field exists on any node record today; this half is genuine schema-room. |
| `"nondeterministic" IN ndet_fn.transitive_effects` | The callee (or one of its callees) carries the `nondeterministic` effect — a real, populated label (system clock, random numbers, env reads). What's missing is CQL exposure of `transitive_effects`, not the label. |
| `NOT "nondeterministic" IN fn.own_effects` | (Second query) Restrict to functions that are themselves innocent but inherit nondeterminism from a callee — the surprising cases. |
| `depth` | Hop count from the function to the nondeterministic callee; shallow is worse (the nondeterminism is close to the surface). |

**Reading the result** — Each row names a function that claims or is assumed to be deterministic but has a transitive call path to a nondeterministic operation. The `nondeterministic_callee` is the specific function introducing the nondeterminism. Review the call chain to determine whether the nondeterminism is intentional (a deliberate source of entropy that was missed in the purity analysis) or accidental (a dependency on external state that should be injected as a parameter).
