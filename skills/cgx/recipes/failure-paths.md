# cgx recipe — Failure-Path Behavior (Theme 4)

**Audience:** AI agents and engineers auditing exception, panic, and error-recovery paths.
**Since baseline:** Run `cgx --version`; check `reference/versions.md` for the full capability ladder.

Edge-condition labels (`always`, `conditional`, `exception`, `loop`, `panic`) are a first-class queryable
dimension of the call graph. See `reference/mental-model.md` for the full edge-condition vocabulary.
See `reference/query-language.md` for CQL syntax rules (bounded paths, double-quoted strings, supported
predicates). See `reference/cli.md` for shared flag reference.

**Key gating for this theme:**

| Capability | Since | Notes |
|---|---|---|
| `r.condition = "exception"` / `r.condition = "panic"` edge filtering | v0.1 | Syntactic labels; run today |
| `ANY` / `NONE` quantifiers over bounded `*N` paths | v0.1 | Always bound hops — unbounded `CALLS*` hangs |
| Direct-edge `WHERE r.condition` (no path variable) | v0.1 | Simplest form for single-hop queries |
| `MUST PASS THROUGH` / `AVOIDING` (∀-path / must-pass-through) | v0.3 | Binary rejects as deferred today |
| `DATA_FLOW` edges, `transitive_effects`, `sink_class`, taint props | v0.3 | Inert or plan-error today |
| Effect/resource pairing (`acquire`→`release` analysis) | v0.3 | Deferred |

---

## Core CQL shapes (v0.1 runnable)

**Shell quoting rule:** wrap the whole query in single quotes; use double quotes inside CQL.

### Single-hop edge-condition filter

```bash
# Since: v0.1
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE r.condition = "exception" RETURN a.name, b.name, a.file, a.line LIMIT 20'
```

### ANY quantifier — path contains at least one exception edge

```bash
# Since: v0.1
cgx query 'MATCH path = (a)-[:CALLS*3]->(b) WHERE ANY(r IN relationships(path) WHERE r.condition = "exception") RETURN a.name, b.name LIMIT 10'
```

### NONE quantifier — path contains no exception or panic edge (happy-path filter)

```bash
# Since: v0.1
cgx query 'MATCH path = (a)-[:CALLS*3]->(b) WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) RETURN a.name, b.name LIMIT 10'
```

### NOT in a quantifier predicate — use `NOT x = …`, never `NOT IN`

```bash
# Since: v0.1 — NOT IN [...] is a parse error; use NOT r.condition = "always"
cgx query 'MATCH path = (a)-[:CALLS*2]->(b) WHERE NONE(r IN relationships(path) WHERE NOT r.condition = "exception") RETURN a.name LIMIT 5'
```

**Why bounding matters:** `CALLS*` (no number) hangs. Always write `CALLS*N` with an explicit hop count.
Adjust `N` to the depth appropriate for your codebase; start small (2–4) and increase if results are sparse.

---

## Step 0 — Find exact symbol names first

cgx has no search, glob, or fuzzy match. Commands need the fully-qualified symbol name and fail with
`no symbol matched '<x>'` (exit 2) otherwise. Grep the source first:

```bash
grep -r "processOrder" src/ --include="*.rs" -l
# Then use the exact qualified name, e.g. orders::handler::processOrder
```

---

## Recipes

### Which call sites are only reachable on exception/panic paths?

**Status:** runnable today (v0.1 — bounded CQL)  **Personas:** PSE, SSE

Find direct callers reached via an exception-conditioned edge to a known target:

```bash
# Since: v0.1 — direct-edge form (fastest)
cgx query 'MATCH (caller)-[r:CALLS]->(b) WHERE r.condition IN ["exception","panic"] AND b.name = "cleanup_fn" RETURN caller.name, caller.file, caller.line LIMIT 20'
```

Find all symbols reachable from a function only on error paths (bounded path form):

```bash
# Since: v0.1 — substitute the exact FQN of the function under inspection
cgx query 'MATCH path = (src {name:"orders::handler::processOrder"})-[:CALLS*4]->(callee) WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) RETURN callee.name, callee.file, callee.line LIMIT 20'
```

**Why this works:** `r.condition IN ["exception","panic"]` filters edges to only the syntactic error-class
labels. The `ANY` quantifier requires at least one such edge anywhere on the path.

**Reading the result:** Confidence on exception-path edges is typically `possible` at v0.1 (syntactic
analysis, no runtime evidence). A result here means a call path exists; it does not mean that path is taken
at runtime. False positives from dead error handlers are possible — cross-reference `reference/mental-model.md`
(confidence ladder) and consider `--confidence probable` (discriminating as of v0.2).

---

### Do any exception handlers call a specific sensitive function?

**Status:** runnable today (v0.1)  **Personas:** PSE

```bash
# Since: v0.1 — check if authenticate is reached via an exception edge
cgx query 'MATCH path = (ep)-[:CALLS*4]->(auth {name:"authenticate"}) WHERE ANY(r IN relationships(path) WHERE r.condition = "exception") RETURN ep.name, ep.file, ep.line LIMIT 20'
```

Pair with a happy-path query to compare call contexts:

```bash
# Since: v0.1 — happy-path callers of authenticate (no exception edge)
cgx query 'MATCH path = (ep)-[:CALLS*4]->(auth {name:"authenticate"}) WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) RETURN ep.name, ep.file, ep.line LIMIT 20'
```

**Reading the result:** Rows appearing only in the exception-path result (absent from the happy-path result)
indicate authentication called inside error recovery — a divergence worth manual review.

---

### Which functions have panic-conditioned edges to panic sites?

**Status:** runnable today (v0.1)  **Personas:** SSE

```bash
# Since: v0.1 — direct call to a known panic site on a panic-conditioned edge
cgx query 'MATCH (caller)-[r:CALLS]->(panic_site) WHERE r.condition = "panic" AND panic_site.name IN ["panic!", "unwrap", "expect"] RETURN caller.name, caller.file, caller.line, panic_site.name LIMIT 20'
```

To narrow to functions that also write to state, use `--format json` and post-filter by `file` in your
pipeline, since `own_effects` is a v0.3 node property (not available today):

```bash
# Since: v0.1 — pipe JSON output to jq for post-filtering
cgx query 'MATCH (caller)-[r:CALLS]->(panic_site) WHERE r.condition = "panic" AND panic_site.name IN ["panic!", "unwrap", "expect"] RETURN caller.name, caller.file, caller.line' --format json | jq '.results[]'
```

**Reading the result:** These are functions with syntactically-identified panic-conditioned calls. At v0.1,
`panic`-conditioned labels are syntactic; they do not confirm runtime reachability under real inputs.

---

### Which error paths skip a required call (e.g., audit_log)?

**Status:** v0.1 for the NONE-on-nodes form; v0.3 for `MUST PASS THROUGH`/`AVOIDING`

The `AVOIDING` keyword and `MATCH ALL … MUST PASS THROUGH` are v0.3 — the binary rejects them today.
Use the `NONE(n IN nodes(path) …)` form as the v0.1 equivalent:

```bash
# Since: v0.1 — paths to a sensitive operation that include an exception edge AND skip audit_log
# Substitute your actual entrypoint kind, sensitive operation name, and audit function name
cgx query 'MATCH path = (ep {kind:"entrypoint"})-[:CALLS*5]->(sink {name:"sensitive_operation"}) WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) AND NONE(n IN nodes(path) WHERE n.name = "audit_log") RETURN ep.name, ep.file, ep.line, sink.name LIMIT 10'
```

**Why this works:** `NONE(n IN nodes(path) WHERE n.name = "audit_log")` asserts that `audit_log` does not
appear anywhere on the path. Combined with the `ANY` exception-edge filter, this finds error paths that
bypass audit logging.

**Reading the result:** Non-empty results are error paths that skip `audit_log`. Substitute the real
qualified name of your audit function. Confidence is `possible` at v0.1 — treat results as candidates for
manual confirmation.

**Since: v0.3** — `MUST PASS THROUGH` / `AVOIDING` (∀-path quantification):

```
# NOT runnable today — binary returns: "not supported in this release (deferred)"
MATCH ALL path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"sensitive_operation"})
AVOIDING (audit {name:"audit_log"})
RETURN ep.name, sink.name
```

---

### Are there transaction paths without commit or rollback?

**Status:** v0.1 for reachability shape; v0.3 for full acquire/release pairing analysis

Use `cgx paths` to check whether `commit` is reachable from `begin`:

```bash
# Since: v0.1 — is there a path from transaction begin to commit?
# Substitute your actual transaction-begin and commit symbol names
cgx paths db::Connection::begin db::Connection::commit --max-depth 6
cgx paths db::Connection::begin db::Connection::rollback --max-depth 6
```

Use `--assert-empty` in CI to assert no path exists (i.e., they are unreachable — inverts the check):

```bash
# Since: v0.1 — CI: fail if NO commit path exists would require the inverse pattern;
# use paths to confirm reachability then audit missing-commit paths manually at v0.1
cgx paths db::Connection::begin db::Connection::commit --format json --max-depth 6
```

For the full "all exit paths include commit-or-rollback" query (exception-path variant), use `cgx query`
with a bounded hop count and the `NONE` predicate over nodes:

```bash
# Since: v0.1 — exit paths from begin that skip commit and rollback (exception paths only)
cgx query 'MATCH path = (begin {name:"db::Connection::begin"})-[:CALLS*6]->(exit_node) WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) AND NONE(n IN nodes(path) WHERE n.name IN ["db::Connection::commit","db::Connection::rollback"]) RETURN begin.file, begin.line, exit_node.name, exit_node.file LIMIT 10'
```

**Reading the result:** Non-empty results indicate exception-path routes from the transaction begin that
reach a callee (possibly an exit site or error handler) without passing through commit or rollback. This is
a heuristic at v0.1 — `exit_node.is_return_site` and `last_node()` are v0.3 predicates. Use the node-name
approach as a proxy.

**Since: v0.3** — full acquire/release pairing analysis with `CALL cgx.pedigree(...)` and
`exit_node.is_return_site` predicates.

---

## Questions deferred to v0.3

These questions from the upstream cookbook are tagged `answerable-today` but require v0.3 features.
Do not emit them as runnable in a v0.1 environment.

| Question | What it needs | Since |
|---|---|---|
| Q38 — catch/recover blocks calling network/disk I/O | `transitive_effects` node property | v0.3 |
| Q40 — exception-path values flowing to log sinks | `DATA_FLOW` edges, `sink_class` property | v0.3 |
| Q44 — `#[must_use]` return values dropped on exception path | `DATA_FLOW*1..1`, `returns_result`, `derives_from_call` | v0.3 |
| Q45 full — transaction lifecycle with `is_return_site` | `exit_node.is_return_site`, `last_node()` | v0.3 |
| Q76 / Q98 — fail-open auth bypass with `position_in()` ordering | `position_in()` path helper | v0.3 |
| Q77 — catch-all handlers with `NOT EXISTS {}` subqueries | `NOT EXISTS` subquery | v0.3 |
| Q78 / Q103 — compensating-call pairing on all exit paths | `last_node()`, full acquire/release pairing | v0.3 |
| Q106 — shared-state mutation + panic (`own_effects`) | `own_effects` node property | v0.3 |
| `MUST PASS THROUGH` / `AVOIDING` (∀-path forms) | Path-set algebra engine | v0.3 |

The v0.1 workaround for most of these is to run a `NONE(n IN nodes(path) …)` approximation and accept
that it is a heuristic, not a proven ∀-path guarantee.

---

## CI pattern

```bash
# Since: v0.1 — assert that a known sensitive function has no direct exception-conditioned callers
# Exit 1 if results found; exit 4 if the pattern matches 0 nodes (use --allow-vacuous to suppress)
cgx query 'MATCH (caller)-[r:CALLS]->(b {name:"sensitive_fn"}) WHERE r.condition IN ["exception","panic"] RETURN caller.name' --assert-empty --allow-vacuous
```

See `reference/output-and-exit.md` for the full exit-code contract.
