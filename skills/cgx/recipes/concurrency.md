# Recipe: Concurrency & Resource Safety

**Not answerable in v0.1; requires cgx >= 0.3.**

Run `cgx --version` first. If `MINOR < 3`, none of these queries run — the schema
features they depend on (GM-9 spawn edges, GM-10 suspension points, GM-11 lock
sets, GM-12 effect system, GM-13 resource lifecycle pairs) are schema-reserved
stubs in v0.1 that carry no populated data. See `reference/versions.md`.

---

## What becomes available at v0.3

| Feature | Since | Needed for |
|---|---|---|
| Spawn edges (GM-9) | v0.3 | Cross-thread reachability, spawn-context identity |
| Suspension points (GM-10) | v0.3 | `await`/`yield` site detection; TOCTOU checks |
| Lock-set attribute (GM-11) | v0.3 | Held-lock set at each call site |
| Function effect system (GM-12) | v0.3 | `blocking`, `writes-global`, `nondeterministic` effects |
| Resource lifecycle pairs (GM-13) | v0.3 | Acquire/release pairing across exception paths |

---

## v0.1 heuristic (no lock-set analysis)

If you need a rough approximation today, you can find all direct callers of a known
lock-acquire function using `callers`. This is call-graph reachability only — it
does **not** tell you what lock is held, whether a guard lives across an `await`,
or whether any release exists on every path. Treat results as a call-site inventory,
not a concurrency analysis.

**Since: v0.1** (heuristic only — not lock-set analysis)

Step 0: find the exact symbol name.

```bash
# There is no cgx search. Find the real symbol name first.
rg --type rust "fn lock\b" path/to/repo/src
```

Step 1: list callers of the acquire function.

```bash
cgx callers 'MyMutex::lock' --depth 3 --format json
```

This tells you which functions call `MyMutex::lock` within 3 hops. It cannot tell
you whether those callers `await` while holding the guard.

---

## Documented v0.3 queries (not runnable today)

Each entry below shows the canonical query form from the specification. Gate every
invocation on `cgx --version`:

```bash
cgx_minor=$(cgx --version | awk '{print $2}' | cut -d. -f2)
if [ "$cgx_minor" -lt 3 ]; then
  echo "Requires cgx >= 0.3. Current: $(cgx --version)"; exit 1
fi
```

---

### Are there TOCTOU windows where an `await` separates validation from use?

**Since: v0.3** — requires GM-10 suspension points (`suspends` property on call-site nodes).

```bash
cgx query @toctou.cql --repo /path/to/repo
```

`toctou.cql`:

```cypher
MATCH path = (validate)-[:CALLS*2]->(use)
WHERE validate.name = "fs::metadata"
  AND use.name = "fs::open"
  AND ANY(n IN nodes(path) WHERE n.suspends = true)
RETURN validate.file, validate.line, use.file, use.line
LIMIT 20
```

**Why this works:** the `suspends = true` predicate on nodes(path) identifies
`await`/`yield` points between the check and the use — the window where the
validated state can change.

**Reading the result:** each row is a potential TOCTOU window. The suspension-point
node marks where the runtime may interleave other work. Confidence is inherited
from the weakest edge on the path; `possible` edges from dynamic dispatch reduce
certainty. See `reference/mental-model.md` for the confidence ladder.

---

### Are there acquire sites with no release on every exception path?

**Since: v0.3** — requires GM-13 resource lifecycle pairs and Q-22 ordering/pairing
predicates.

```bash
cgx query @resource-leak.cql --repo /path/to/repo
```

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

**Why this works:** the `NONE` predicate finds exit paths with no release node;
`r.condition IN ["exception","panic"]` distinguishes exception-path leaks from
happy-path leaks.

**Reading the result:** non-empty results are resource leaks. `on_exception_path =
true` means the leak only occurs on error branches; `false` means the happy path
also leaks (higher priority). See `reference/mental-model.md` for edge-condition
labels.

---

### Which shared fields are written from two spawn contexts under inconsistent lock sets?

**Since: v0.3** — requires GM-9 spawn edges and GM-11 lock-set attribute.

```bash
cgx query @data-race.cql --repo /path/to/repo
```

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

**Why this works:** `a.spawn_context <> b.spawn_context` selects pairs reachable
from different threads/tasks; the `NOT ANY` predicate confirms no lock is shared
between the two access sites.

**Reading the result:** each row names a field accessed from two spawn contexts with
no common lock. Compare `locks_a` and `locks_b` to identify which lock is the
intended protection. Filter with `--confidence certain` to reduce false positives
from conservative alias approximation.

---

### Which async functions hold a mutex guard live at an `await` point, or call blocking sync I/O?

**Since: v0.3** — requires GM-10 suspension points, GM-11 lock sets, GM-12 `blocking`
effect.

Await-holding-lock:

```bash
cgx query 'MATCH (site) WHERE site.suspends = true AND size(site.lock_set) > 0 RETURN site.name, site.file, site.line, site.lock_set AS held_locks ORDER BY site.file, site.line LIMIT 20' --repo /path/to/repo
```

Blocking-in-async:

```bash
cgx query @blocking-in-async.cql --repo /path/to/repo
```

`blocking-in-async.cql`:

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

**Why this works:** `site.suspends = true AND size(site.lock_set) > 0` finds
suspension points where a guard is still live; `"blocking" IN fn.transitive_effects`
propagates the blocking effect across async call boundaries, where Clippy's
intra-procedural `await_holding_lock` lint cannot reach.

**Reading the result:** await-holding-lock rows name each suspension point with
`held_locks` identifying which guards are live. Blocking-in-async rows name each
blocking callee with `depth` indicating how many async hops hide it from the
entrypoint.

---

### Which functions with `writes-global` effect are callable from two or more spawn contexts without a shared lock?

**Since: v0.3** — requires GM-9 spawn edges, GM-11 lock sets, GM-12 `writes-global`
effect.

```bash
cgx query @writes-global-race.cql --repo /path/to/repo
```

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

**Why this works:** `fn_a = fn_b` confirms both spawn contexts reach the same
function; `"writes-global" IN fn_a.own_effects` selects only functions that write
shared state; the `NOT ANY` lock predicate confirms no common lock protection
exists.

**Reading the result:** each row is a potential global-write data race. The
`spawn_a_site` and `spawn_b_site` columns locate the thread creation points.
Use `--confidence certain` to reduce false positives from alias approximation.

---

### Which functions annotated as pure transitively reach nondeterministic effects?

**Since: v0.3** — requires GM-12 `nondeterministic` transitive effect.

```bash
cgx query @pure-nondeterministic.cql --repo /path/to/repo
```

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

**Why this works:** `fn.is_pure = true` selects functions annotated or inferred as
pure; `"nondeterministic" IN ndet_fn.transitive_effects` matches any callee that
reads the system clock, generates random numbers, or reads environment variables.

**Reading the result:** each row names a function that claims to be deterministic
but reaches a nondeterministic operation. Shallow `depth` values are the highest
priority — the nondeterminism is close to the surface. Review whether the
dependency is intentional (a deliberate entropy source) or accidental (state that
should be injected as a parameter).

---

## CQL notes for v0.3 concurrency queries

- All multi-hop patterns here use bounded `*2`. Raise the bound if your call graph
  is deeper, but always bound — unbounded `CALLS*` hangs. See `reference/query-language.md`.
- Node props used here (`suspends`, `lock_set`, `spawn_context`, `own_effects`,
  `transitive_effects`, `is_pure`, `is_return_site`) are schema-reserved in v0.1
  and populated at v0.3. Querying them in v0.1 returns empty or errors exit 2.
- Edge types `READS_FIELD`, `WRITES_FIELD` are v0.3 additions. In v0.1 only
  `CALLS` edges exist. See `reference/mental-model.md`.
- Wrap the whole query in single quotes; use double quotes inside for string
  literals. Single quotes inside CQL → parse error.
