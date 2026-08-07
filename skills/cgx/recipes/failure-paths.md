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
| `r.condition = "exception"` edge filtering | v0.1 | Syntactic label, **populated**; the primary tool of this theme |
| `r.condition = "panic"` edge filtering | v0.1 (parses) | **Schema-present, never populated** — see the warning below before you use it |
| `ANY` / `NONE` quantifiers over bounded `*N` paths | v0.1 | Always bound hops — unbounded `CALLS*` hangs |
| Direct-edge `WHERE r.condition` (no path variable) | v0.1 | Simplest form for single-hop queries |
| `MUST PASS THROUGH` / `AVOIDING` (∀-path / must-pass-through) | deferred | Plan error (exit 2) today, fired at the `ALL` token |
| `DATA_FLOW` edges in CQL (`[:DATA_FLOW]`) | v0.3 | Works; returns empty when no flows indexed |
| `transitive_effects`, `sink_class`, taint props | deferred | Plan error (exit 2) today |
| Effect/resource pairing (`acquire`→`release` analysis) | deferred | Not shipped in v0.3.0 |

---

## Before you query `panic`: an empty result here is not a safety finding

`panic` is one of the five edge-condition labels the schema declares, and CQL accepts it. **No
frontend emits it.** A query filtering on `r.condition = "panic"` returns zero rows on a codebase
full of panics, exits 0, and reports `approximation: exact` — the shape that reads as "checked,
nothing found" when nothing was ever checkable.

Verified this cycle against the shipped `fixtures/rust-sample`, whose `src/panics.rs` exists
specifically to exercise `panic!`, `.unwrap()`, `.expect()`, `assert!` and `unreachable!` and
annotates each with a `// panic edge` comment:

```
$ cgx query 'MATCH (a)-[r:CALLS]->(b) RETURN r.condition' --format json | jq -r '.rows[][]' | sort | uniq -c
  73 always
  13 conditional
   9 exception
   7 loop
```
That is the entire condition distribution over all 102 call edges. Zero `panic`.

Two independent consequences, both live:

1. **No `panic`-conditioned edge exists.** The count above is the whole graph.
2. **Panic sites are not nodes.** `.unwrap()` / `.expect()` / `panic!()` produce no callee edge at
   all — `cgx explain rust_sample::panics::check_invariant` reports `callers: 1, callees: 0` for a
   function whose entire body is an `assert!`. `cgx search unwrap` returns `(no results)`.

So a `panic`-shaped query is under-approximate in a way the approximation contract **cannot** tell
you about: the contract reports incompleteness of the *search*, not of the *model*, and this gap is
in the model. The empty answer is honest about the graph and silent about the program.

**Use `exception` as this theme's primary label.** It is populated (9 edges on the same fixture),
covers Rust's `Result`/`?`/`Err` arms, and is what the recipes below are built on. Treat `panic`
filtering as a capability to re-test — run `cgx search unwrap` against your own index and expect
zero hits — rather than as a query to trust. This is the same failure shape as `cgx coupling` on a
shallow clone: exit 0, empty answer, and the emptiness is an artifact of what was never available.

---

## Core CQL shapes (v0.1 runnable)

**Shell quoting rule:** wrap the whole query in single quotes; use double quotes inside CQL.

### Single-hop edge-condition filter

```bash
# Since: v0.1
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE r.condition = "exception" RETURN a.name, b.name, a.file, a.line LIMIT 20'
```

### ANY quantifier — path contains at least one exception edge

Anchor the source node (`{name:"…"}`) to avoid a full-graph traversal, which hangs on real codebases.

```bash
# Since: v0.1 — anchor 'a' to a specific source symbol
cgx query 'MATCH path = (a {name:"orders::handler::processOrder"})-[:CALLS*3]->(b) WHERE ANY(r IN relationships(path) WHERE r.condition = "exception") RETURN a.name, b.name LIMIT 10'
```

### NONE quantifier — path contains no exception or panic edge (happy-path filter)

Anchor the source node to avoid a full-graph traversal.

```bash
# Since: v0.1 — anchor 'a' to a specific source symbol
cgx query 'MATCH path = (a {name:"orders::handler::processOrder"})-[:CALLS*3]->(b) WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) RETURN a.name, b.name LIMIT 10'
```

### NOT in a quantifier predicate — use `NOT x = …`, never `NOT IN`

```bash
# Since: v0.1 — NOT IN [...] is a parse error; use NOT r.condition = "always"
cgx query 'MATCH path = (a)-[:CALLS*2]->(b) WHERE NONE(r IN relationships(path) WHERE NOT r.condition = "exception") RETURN a.name LIMIT 5'
```

**Why bounding and anchoring matter:** `CALLS*` (no number) hangs. Unanchored `CALLS*N` over a full graph
(no source predicate) is also very slow — always anchor at least one endpoint via a node property predicate
(`{name:"…"}` or `{fqn:"…"}`). Start with `CALLS*2` and increase hop count if results are sparse.

---

## Step 0 — Find exact symbol names first

Commands need the fully-qualified symbol name and fail with `no symbol matched '<x>'` (exit 2) if the name
does not match. Use `cgx search` to discover the exact FQN:

```bash
# Since: v0.3 — case-insensitive substring search
cgx search "processOrder" --repo /path/to/repo
# Returns FQNs, e.g. orders::handler::processOrder

# Filter to functions only
cgx search "processOrder" --kind function --repo /path/to/repo
```

`cgx search` is available in v0.3.0. As a fallback for older index builds, grep the source:

```bash
grep -r "processOrder" src/ --include="*.rs" -l
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

Worked against the shipped `fixtures/rust-sample`, whose `errors::pipeline` propagates with `?`
(run twice, byte-identical):

```
$ cgx query 'MATCH path = (a {name:"rust_sample::errors::pipeline"})-[:CALLS*2]->(b) WHERE ANY(r IN relationships(path) WHERE r.condition = "exception") RETURN a.name, b.name LIMIT 10'
a.name                         b.name
rust_sample::errors::pipeline  rust_sample::errors::read_string
rust_sample::errors::pipeline  rust_sample::errors::read_string
rust_sample::errors::pipeline  rust_sample::errors::read_string
rust_sample::errors::pipeline  rust_sample::errors::parse_int
rust_sample::errors::pipeline  rust_sample::errors::parse_int
rust_sample::errors::pipeline  rust_sample::errors::parse_int
rust_sample::errors::pipeline  rust_sample::errors::check_range
rust_sample::errors::pipeline  rust_sample::errors::check_range
rust_sample::errors::pipeline  rust_sample::errors::check_range
approximation: exact (within modeled graph)
freshness: current | indexed tree dd3ea2b, working tree clean
```
Nothing is elided: those two footer lines are the last two lines of a `cgx query` human answer, and
every doc example that ends on the `approximation:` line is missing one.

**One row per path, not per symbol.** `read_string` appears three times because three distinct
2-hop paths reach it. Deduplicate client-side, or project the path and count it, but do not read the
row count as a symbol count.

**Why this works:** `r.condition IN ["exception","panic"]` filters edges to the syntactic error-class
labels. The `ANY` quantifier requires at least one such edge anywhere on the path.

**The `"panic"` element in these lists contributes nothing today** — no edge carries that condition
(see the warning at the top). It is kept in the predicate so the query stays correct if the frontend
starts emitting the label, but every row you get back today came from `exception` alone. The
consequence bites hardest in the `NONE(...)` "happy-path" form below: excluding `panic` from a path
does not exclude code that panics, because panicking calls are not in the graph at all.

**Reading the result:** Exception-path edges are **not** inherently low-confidence. On the shipped
Rust fixture the nine `exception` edges band 8 `certain` / 1 `possible` — the condition label and
the confidence ladder are independent axes. A result here means a call path exists; it does not mean
that path is taken at runtime, and false positives from dead error handlers are possible.
Cross-reference `reference/mental-model.md` for the confidence ladder. Note that `--confidence` is a
**minimum floor**: raising it discards `possible` edges rather than adding them.

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

**Status:** **not answerable today** — the query runs and returns an empty set that means nothing.
**Personas:** SSE

The natural query is this, and it is the one to understand rather than to run:

```bash
# Runs, exits 0, returns zero rows on any codebase — see the warning at the top of this file
cgx query 'MATCH (caller)-[r:CALLS]->(panic_site) WHERE r.condition = "panic" AND panic_site.name IN ["panic!", "unwrap", "expect"] RETURN caller.name, caller.file, caller.line, panic_site.name LIMIT 20'
```

Real output on the shipped Rust fixture — a header row, nothing under it, and a contract that says
the answer is exact:

```
caller.name  caller.file  caller.line  panic_site.name
approximation: exact (within modeled graph)
freshness: current | indexed tree dd3ea2b, working tree clean
```

Both halves of the predicate fail independently: no edge carries `condition = "panic"`, and no node
is named `panic!`/`unwrap`/`expect`. **Do not report this empty set as "no panic paths found."**

**What to do instead.** Panic sites are a source-text property in this build, not a graph property,
so answer it with a text search and use cgx for the reachability half:

```bash
# 1. Locate panic sites textually (cgx does not model them as nodes)
rg -n 'panic!|\.unwrap\(\)|\.expect\(' src/

# 2. For each enclosing function, ask cgx who can reach it
cgx callers rust_sample::panics::check_invariant --depth 3
```

`exception`-conditioned edges — the recipes above — remain fully queryable and are the supported
way to ask a failure-path question today.

---

### Which error paths skip a required call (e.g., audit_log)?

**Status:** NONE-on-nodes form runnable today; `MUST PASS THROUGH`/`AVOIDING` deferred past v0.3.0

`AVOIDING` and `MATCH ALL … MUST PASS THROUGH` produce a plan error (exit 2) in v0.3.0.
Use the `NONE(n IN nodes(path) …)` form as the structural approximation:

```bash
# Since: v0.1 — paths to a sensitive operation that include an exception edge AND skip audit_log
# Anchor on the entrypoint's real FQN. Do NOT anchor on {kind:"entrypoint"} — see below.
cgx query 'MATCH path = (ep {name:"my_crate::http::handle_request"})-[:CALLS*5]->(sink {name:"sensitive_operation"}) WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) AND NONE(n IN nodes(path) WHERE n.name = "audit_log") RETURN ep.name, ep.file, ep.line, sink.name LIMIT 10'
```

**`{kind:"entrypoint"}` is a second guaranteed-empty filter, and the same trap as `panic`.**
`SymbolKind::Entrypoint` exists in the schema and is accepted by CQL and by `--kind`, but **no
language frontend ever constructs it** — the only references to it in the tree are the CLI's
`--kind` mapping and the CQL lowering. Entrypoint-ness is carried on a *separate* field
(`NodeRecord.entrypoint_kind`), while the node's `kind` stays `function`. On the shipped fixture
`cgx search rust_sample::main` reports `[function]`, and `cgx unused --kind entrypoint` returns
`(no results)`. Any query anchored on `kind:"entrypoint"` returns an empty set at exit 0 regardless
of the rest of the pattern. Anchor on the entrypoint's FQN instead.

**Why this works:** `NONE(n IN nodes(path) WHERE n.name = "audit_log")` asserts that `audit_log` does not
appear anywhere on the path. Combined with the `ANY` exception-edge filter, this finds error paths that
bypass audit logging.

**Reading the result:** Non-empty results are error paths that skip `audit_log`. Substitute the real
qualified name of your audit function. Confidence is `possible` at v0.1 — treat results as candidates for
manual confirmation.

**Deferred past v0.3.0** — `MUST PASS THROUGH` / `AVOIDING` (∀-path quantification):

```
# NOT runnable — exit 2: plan error: `MATCH ALL … MUST PASS THROUGH/AVOIDING` (Q-20 guarded-cut)
#                         is not supported in this release (deferred)
MATCH ALL path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"sensitive_operation"})
AVOIDING (audit {name:"audit_log"})
RETURN ep.name, sink.name
```

---

### Are there transaction paths without commit or rollback?

**Status:** reachability shape runnable today; full acquire/release pairing analysis is deferred past v0.3.0

Use `cgx paths` to check whether `commit` is reachable from `begin`:

```bash
# Since: v0.1 — is there a path from transaction begin to commit?
# Substitute your actual transaction-begin and commit symbol names
cgx paths db::Connection::begin db::Connection::commit --depth 6
cgx paths db::Connection::begin db::Connection::rollback --depth 6
```

Use `--assert-empty` in CI to assert no path exists (i.e., they are unreachable — inverts the check):

```bash
# Since: v0.1 — CI: fail if NO commit path exists would require the inverse pattern;
# use paths to confirm reachability then audit missing-commit paths manually at v0.1
cgx paths db::Connection::begin db::Connection::commit --format json --depth 6
```

For the full "all exit paths include commit-or-rollback" query (exception-path variant), use `cgx query`
with a bounded hop count and the `NONE` predicate over nodes:

```bash
# Since: v0.1 — exit paths from begin that skip commit and rollback (exception paths only)
cgx query 'MATCH path = (begin {name:"db::Connection::begin"})-[:CALLS*6]->(exit_node) WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"]) AND NONE(n IN nodes(path) WHERE n.name IN ["db::Connection::commit","db::Connection::rollback"]) RETURN begin.file, begin.line, exit_node.name, exit_node.file LIMIT 10'
```

**Reading the result:** Non-empty results indicate exception-path routes from the transaction begin that
reach a callee (possibly an exit site or error handler) without passing through commit or rollback. This is
a heuristic — `exit_node.is_return_site` and `last_node()` are not supported in v0.3.0. Use the
node-name approach as a proxy until those predicates are available.

**`CALL cgx.pedigree(...)` is not deferred — it runs today.** What is deferred is the acquire/release
*pairing* analysis built on top of it, and `exit_node.is_return_site` (plan error, exit 2). Before
using the procedure, read the direction warning in `recipes/taint.md`: `cgx.pedigree` returns a
value's **consumers** under a column named `source`, which is the opposite of what the name implies.

---

## Questions deferred past v0.3.0

These questions require node properties or CQL constructs that are plan errors in v0.3.0.
The `cgx query` command exits 2 with a "not supported in this release" message when they are used.

| Question | What it needs | Status in v0.3.0 |
|---|---|---|
| Q38 — catch/recover blocks calling network/disk I/O | `transitive_effects` node property | plan error (exit 2) |
| Q40 — exception-path values flowing to log sinks | `sink_class` property | plan error (exit 2) |
| Q44 — `#[must_use]` return values dropped on exception path | `returns_result`, `derives_from_call` props | plan error (exit 2) |
| Q45 full — transaction lifecycle with `is_return_site` | `exit_node.is_return_site`, `last_node()` | plan error (exit 2) |
| Q76 / Q98 — fail-open auth bypass with `position_in()` ordering | `position_in()` path helper | plan error (exit 2) |
| Q77 — catch-all handlers with `NOT EXISTS {}` subqueries | `NOT EXISTS` subquery | plan error (exit 2) |
| Q78 / Q103 — compensating-call pairing on all exit paths | `last_node()`, full acquire/release pairing | plan error (exit 2) |
| Q106 — shared-state mutation + panic (`own_effects`) | `own_effects` node property | plan error (exit 2) |
| `MUST PASS THROUGH` / `AVOIDING` (∀-path forms) | Path-set algebra engine | plan error (exit 2) |

Note: `[:DATA_FLOW]` edges themselves work in v0.3.0 (`MATCH (a)-[:DATA_FLOW]->(b)` is valid CQL and
returns rows when the index was built with dataflow enabled). It is the semantic taint properties
(`source_class`, `sink_class`, `taint_label`, etc.) layered on top of those edges that remain deferred.

The structural workaround for most of these is to run a `NONE(n IN nodes(path) …)` approximation and accept
that it is a heuristic, not a proven ∀-path guarantee.

---

## CI pattern

```bash
# Since: v0.1 — assert that a known sensitive function has no direct exception-conditioned callers
# Exit 1 if results found; exit 4 if the pattern matches 0 nodes (use --allow-vacuous to suppress)
cgx query 'MATCH (caller)-[r:CALLS]->(b {name:"sensitive_fn"}) WHERE r.condition IN ["exception","panic"] RETURN caller.name' --assert-empty --allow-vacuous
```

Both arms verified on the shipped fixture: a matching caller gives
`cgx: assertion failed: results found when none expected`, exit 1; no match gives exit 0.

**Read the contract before you trust the pass.** Every negative answer this theme produces carries a
`scope` object naming what the search actually covered — it is absent on a positive answer and
present on an empty one. On a `reaches` miss:

```
$ cgx reaches rust_sample::errors::log_error rust_sample::conditions::dispatch --format json | jq '.approximation'
{
  "direction": "exact",
  "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
  "reasons": [],
  "scope": {
    "confidence_floor": "possible",
    "max_depth": null,
    "searched_edge_kinds": ["calls", "calls-virtual", "calls-closure", "calls-callback", "calls-async", "calls-indirect"]
  }
}
```
(Elided: the `searched_edge_kinds` array is reflowed onto one line; its six entries are verbatim.)

A green gate is only as strong as that scope. `"direction": "under"` with a `depth-limit` or
`unresolved-external-calls` reason means the search stopped short — the gate passed because nothing
was found *in what was searched*, which is a weaker claim than the exit code suggests. And `scope`
says nothing about the model: it cannot tell you that panic edges were never in the graph.

See `reference/output-and-exit.md` for the full exit-code contract.
