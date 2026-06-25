# Theme 4: Failure-Path Behavior

When a program takes an error path — a `catch` block, a Rust `Err(e) =>` arm, a Go `if err != nil` block, or a panic handler — the functions it calls and the operations it performs are often less tested and less reviewed than the happy path. This theme covers sixteen questions that audit those failure paths: which code is reachable only through error handling, which cleanup logic is skipped, which transactions are left uncommitted, and which security-critical operations behave differently when errors occur. The enabling capability is `cgx`'s edge-condition labels: every call edge carries one of exactly five labels — `always`, `conditional`, `loop`, `exception`, or `panic` — making failure paths a first-class queryable dimension of the call graph. No existing tool exposes these as queryable edge attributes; this theme maps directly to OWASP Top 10 2025 A10: Mishandling of Exceptional Conditions.

---

### Q37 — Which functions are ONLY reachable through exception/error handlers — never from the happy path?

**Personas:** PSE · **Status:** deferred (v0.3) — `collect(distinct ...)` inside a `WITH` clause causes a parse error (exit 2) in v0.3.0; the `distinct` modifier inside aggregate functions is not yet supported. When `collect(distinct ...)` ships, this query becomes fully answerable. For now, use `collect()` without deduplication and accept duplicate names in the intermediate set (false negatives are possible but rare).

Functions that are exclusively reachable via error handlers are often less scrutinized for security properties. An attacker who can trigger error conditions can reach code that developers implicitly treat as "never called in production." This question finds functions where every path from every entrypoint passes through at least one `exception` or `panic` edge before reaching them.

**The query**

The "ONLY reachable through exception handlers" question is a path-set difference: the set of
targets reachable on an exception path, minus the set reachable on any happy path. Compute the
happy-path set first, then keep exception-reachable targets that are not in it.

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(target)
WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
WITH collect(target.name) AS happy_path_targets
MATCH exc_path = (ep2 {kind:"entrypoint"})-[:CALLS*]->(exc_target)
WHERE ANY(r IN relationships(exc_path) WHERE r.condition IN ["exception","panic"])
  AND NOT exc_target.name IN happy_path_targets
RETURN exc_target.name, exc_target.file, exc_target.line
ORDER BY exc_target.file, exc_target.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | The path from entrypoint to target must contain at least one exception-class edge. |
| `NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Used in the second part to collect all targets reachable without any exception-class edge — the happy-path reachable set. |
| `NOT exc_target.name IN happy_path_targets` | The target must not appear in the happy-path set, meaning it is exclusively reachable via error handling. |

**Reading the result** — Each returned function is only callable via error paths. These are candidates for security review: do they have authorization checks? Do they assume invariants that may be violated in error conditions? Dead-code analysis (Theme 5) is a complement: if one of these functions is also unreachable from most error paths, it may effectively be dead code that can be reactivated.

---

### Q38 — Do any catch/recover blocks call functions that make network requests or write to disk?

**Personas:** PSE · **Status:** deferred (v0.3) — the `transitive_effects` node property causes a plan error (exit 2) in v0.3.0 ("node property `transitive_effects` is not supported in this release"). Until effect-set properties ship, substitute an explicit name list of known I/O functions for the `transitive_effects` membership test (see rewrite below).

Error handlers that make network requests or disk writes can introduce secondary failures, extend recovery time, or create side effects with security implications (e.g., logging sensitive error details to a remote endpoint). This question finds functions with `io.net` or `io.file` effects reachable via exception-conditioned edges.

**The query (v0.3.0 workaround — name-list form)**

```cgx
MATCH path = (handler)-[:CALLS*]->(io_fn)
WHERE ANY(r IN relationships(path) WHERE r.condition = "exception")
  AND io_fn.name IN ["std::fs::write", "std::fs::read", "std::net::TcpStream::connect",
                     "reqwest::get", "hyper::Client::request",
                     "tokio::fs::write", "tokio::net::TcpStream::connect"]
  AND handler.kind = "function"
RETURN handler.name, handler.file, handler.line,
       io_fn.name, io_fn.file, io_fn.line,
       length(path) AS hops
ORDER BY handler.file, handler.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ANY(r IN relationships(path) WHERE r.condition = "exception")` | The path must include at least one `exception`-conditioned edge, placing the caller inside an error handler. |
| `io_fn.name IN [...]` | Explicit name list substitutes for `io_fn.transitive_effects` (not supported in v0.3.0). Extend with the network and filesystem call sites relevant to your codebase. |
| `handler.kind = "function"` | Restrict to function-level catch sites (rather than individual call sites) to reduce result verbosity. |

**Reading the result** — Each row names an error handler that calls a known I/O function. Review: Does the network call send error details to an external endpoint? Does the disk write reveal exception state to a log that is world-readable? Are there secondary failures if the network or disk is unavailable during recovery?

---

### Q39 — What is the complete set of cleanup/defer functions executed when `processOrder()` returns an error?

**Personas:** SSE · **Status:** answerable-today (NOVEL)

When a function returns an error, defer statements, `finally` blocks, and explicit cleanup calls execute. Engineers planning refactors or debugging resource leaks need to know exactly which cleanup functions run on the error path. This question enumerates callees reachable via exception-conditioned edges from `processOrder`.

**The query**

`cgx callees processOrder --repo ./` — see the Layer-2 form for exception-path filtering (no `--edge-condition` flag exists; use the CQL WHERE clause below):

```cgx
MATCH path = (src {name:"processOrder"})-[:CALLS*]->(callee)
WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
RETURN callee.name, callee.file, callee.line,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       [r IN relationships(path) | r.kind] AS edge_kinds,
       length(path) AS hops
ORDER BY hops, callee.file
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src {name:"processOrder"})-[:CALLS*]->` | Traverse all call edges transitively from `processOrder`. |
| `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Restrict to callees that are only reachable via at least one exception-class edge — these are the error-path callees. |
| `edge_kinds` | Shows `implicit:defer`, `implicit:drop`, `implicit:context-exit`, etc. (GM-16) to identify language-native cleanup constructs. |

**Reading the result** — Each returned callee executes on the error return path. Cross-reference against Q41 (which happy-path callees are missing) to identify audit-logging or state-persistence calls that are absent on the error path. The `edge_kinds` column distinguishes explicit cleanup calls from implicit ones like `Drop::drop` or `defer`.

---

### Q40 — Are there logging calls inside exception handlers that log the exception object, which might contain sensitive data?

**Personas:** PSE · **Status:** deferred (v0.3) — the `sink_class` node property causes a plan error (exit 2) in v0.3.0; security-typed taint properties (`sink_class`, `sanitizer_class`, `source_class`, `taint_label`) are not yet supported. Additionally, `r.transformation_kind` is an unknown edge property (plan error, exit 2) in v0.3.0. Rewrite with explicit name matching and omit the `transforms` column until both taint typing and transformation_kind ship.

Exception objects often carry stack traces, function arguments, or internal state that should not appear in externally-accessible logs. This question combines exception-path filtering with data-flow to find cases where exception-path values flow to known log call sites.

**The query**

```cgx
MATCH path = (exc_source)-[:DATA_FLOW*]->(log_sink)
WHERE ANY(r IN relationships(path) WHERE r.condition = "exception")
  AND exc_source.kind IN ["variable", "field"]
  AND log_sink.name IN ["log::error!", "log::warn!", "tracing::error!",
                         "eprintln!", "println!", "logger.error",
                         "logger.warn"]
RETURN exc_source.name, exc_source.file, exc_source.line,
       log_sink.name, log_sink.file, log_sink.line,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS hops
ORDER BY exc_source.file, exc_source.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(exc_source)-[:DATA_FLOW*]->(log_sink)` | Find data-flow paths from any value to a known log call site. |
| `ANY(r IN relationships(path) WHERE r.condition = "exception")` | The path must traverse at least one `exception`-conditioned edge, meaning the value is on the error path (e.g., the `err` variable in a Go `if err != nil` block). |
| `exc_source.kind IN ["variable", "field"]` | Focus on value nodes — the `err` binding, exception fields, or error-message strings — rather than function nodes. |
| `log_sink.name IN [...]` | Explicit name list substitutes for `sink_class:"log"` until security-typed taint ships. Extend with your codebase's logging functions. |

**Reading the result** — Each row is a value that lives on an error path and reaches a log call site. The `transforms` column indicates whether the value was formatted (e.g., `format!("{:?}", err)` produces a `formatted` transformation) or passed directly. Formatted error values that include internal stack data are high-priority findings. The `sanitizer_class` predicate for log-redaction filtering is not available in v0.3.0; substitute `NONE(n IN nodes(path) WHERE n.name IN ["redact", "mask", "sanitize"])` as a name-based heuristic.

---

### Q41 — Which error handling paths skip the audit logging that the happy path always calls?

**Personas:** SSE · **Status:** answerable-today (NOVEL) — the `NONE`-based query below is fully supported. Note: the `AVOIDING` keyword is deferred in v0.3.0 (parse error, exit 2); use the `NONE(n IN nodes(path) ...)` idiom instead.

Security-critical operations should be audit-logged on every path, including error paths. If audit logging is only called on the happy path, an attacker who triggers an error condition can perform an audited operation without leaving a trace. This question finds paths from entrypoints to sensitive sinks that lack the audit log call.

**The query**

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"sensitive_operation"})
WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  AND NONE(n IN nodes(path) WHERE n.name = "audit_log")
RETURN ep.name, ep.file, ep.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
ORDER BY ep.file, ep.line
```

To assert the ∀-path audit guarantee (any path to the sensitive operation that avoids `audit_log`), use the `NONE` predicate without the exception filter:

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"sensitive_operation"})
WHERE NONE(n IN nodes(path) WHERE n.name = "audit_log")
RETURN ep.name, ep.file, ep.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Restrict to error-path routes to the sensitive operation — the error-only variant of the bypass. |
| `NONE(n IN nodes(path) WHERE n.name = "audit_log")` | Require that `audit_log` (or your actual audit-logging function) does NOT appear on the path. |
| Second query (no condition filter) | The ∀-path form: returns any path from an entrypoint to the sensitive operation that avoids the audit node entirely. Zero results means the ∀ guarantee holds. The `AVOIDING` keyword is not available in v0.3.0; this `NONE`-based form is the supported equivalent. |

**Reading the result** — In the first query form, non-empty results identify specific error-path routes that bypass audit logging. In the second (∀-path) form, non-empty results mean there exists at least one path (possibly on the error path) where audit logging is absent — the strongest assertion failure.

---

### Q42 — What is the set of all functions called during program shutdown/panic that are NOT called during normal execution?

**Personas:** SSE · **Status:** deferred (v0.3) — `collect(distinct ...)` inside a `WITH` clause causes a parse error (exit 2) in v0.3.0. The first `MATCH` also references an unbound `path` variable in its `WHERE` clause (the path variable is not bound until a `path =` binding is added). Use `collect()` without `distinct` and add a `path =` binding as a workaround until `collect(distinct ...)` ships.

Panic handlers, shutdown hooks, and signal handlers often call functions that are never exercised during normal testing. These functions may have bugs, security issues, or incomplete implementations that only surface under adversarial conditions. This question identifies functions exclusively reachable via `panic`-conditioned edges.

**The query (v0.3.0 workaround)**

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(normal_callee)
WHERE NONE(r IN relationships(path)
           WHERE r.condition IN ["exception","panic"])
WITH collect(normal_callee.name) AS normal_set
MATCH path = (ep2 {kind:"entrypoint"})-[:CALLS*]->(panic_callee)
WHERE ANY(r IN relationships(path) WHERE r.condition = "panic")
  AND NOT panic_callee.name IN normal_set
RETURN panic_callee.name, panic_callee.file, panic_callee.line
ORDER BY panic_callee.file, panic_callee.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| First `MATCH` block | Collect all callees reachable from entrypoints on non-exception paths — these are the functions exercised during normal execution. |
| `r.condition = "panic"` | In the second query, restrict to paths containing at least one `panic`-conditioned edge. |
| `NOT panic_callee.name IN normal_set` | Exclude any function that also appears on the normal execution set — keep only exclusively-panic-path callees. |

**Reading the result** — Each returned function is only exercised under panic conditions. Review these functions for: correct state assumptions (the program may be in a partially-mutated state when a panic fires — see Q106), resource release (is the function releasing a resource that may not have been acquired?), and security controls (does a panic-path function bypass checks that the happy path enforces?).

---

### Q43 — Do any exception handlers call `authenticate()` or `authorize()` in ways that differ from the happy path?

**Personas:** PSE · **Status:** deferred (v0.3) — `UNION` causes a plan error (exit 2) in v0.3.0 ("UNION is not supported in this release"). Run the two `MATCH` queries separately and compare their result sets manually until `UNION` ships.

Security checks called inside error handlers may operate on different state than their happy-path counterparts, potentially allowing bypasses. This question compares how `authenticate` and `authorize` are called on exception-conditioned paths versus non-exception paths.

**The query (run as two separate queries in v0.3.0)**

Exception-path calls:
```cgx
MATCH exc_path = (ep {kind:"entrypoint"})-[:CALLS*]->(auth {name:"authenticate"})
WHERE ANY(r IN relationships(exc_path) WHERE r.condition IN ["exception","panic"])
RETURN ep.name, ep.file, ep.line,
       auth.name, auth.file, auth.line,
       [r IN relationships(exc_path) | r.condition] AS edge_conditions,
       length(exc_path) AS hops
```

Happy-path calls:
```cgx
MATCH happy_path = (ep2 {kind:"entrypoint"})-[:CALLS*]->(auth2 {name:"authenticate"})
WHERE NONE(r IN relationships(happy_path) WHERE r.condition IN ["exception","panic"])
RETURN ep2.name, ep2.file, ep2.line,
       auth2.name, auth2.file, auth2.line,
       [r IN relationships(happy_path) | r.condition] AS edge_conditions,
       length(happy_path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| First query | Paths to `authenticate` that include at least one exception-class edge — exception-path calls. |
| Second query | Happy-path calls to the same function for side-by-side comparison. |
| `UNION` (v0.3.0 deferred) | Once `UNION` ships, these two queries can be combined with a `path_type` label column to distinguish the two sets in one result. |

**Reading the result** — Compare the two result sets: if `authenticate` is called on exception paths with different entrypoints, different depths, or from different callers than the happy path, those divergences warrant manual review. An `authenticate` call that only appears on exception paths (i.e., only in the first result, absent from the second) may indicate a logic error in error-recovery code.

---

### Q44 — Which functions have been marked with `#[must_use]` but whose return values are dropped on the exception path?

**Personas:** SSE · **Status:** deferred (v0.3) — two blockers: (1) `must_use` is an unknown node property (plan error, exit 2) in v0.3.0; (2) `NOT EXISTS { MATCH ... }` subquery syntax is a parse error (exit 2) in v0.3.0. Both features are required for this query. Until they ship, narrow the search to known must-use functions by name and use the data-flow layer to check for dropped results.

`#[must_use]` in Rust (and equivalent annotations in other languages) signals that the caller must handle the return value. Ignoring it on the exception path means error information or cleanup obligations are silently discarded. This question finds `must_use`-annotated functions whose return values are not consumed on exception-conditioned paths.

**The query (when `must_use` and `NOT EXISTS` subqueries ship)**

```cgx
MATCH path = (caller)-[:CALLS]->(fn {must_use: true})
WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  AND NOT EXISTS {
    MATCH (caller)-[:DATA_FLOW*1..1]->(use_site)
    WHERE use_site.scope = caller.name
  }
RETURN caller.name, caller.file, caller.line,
       fn.name, fn.file, fn.line,
       [r IN relationships(path) | r.condition] AS edge_conditions
ORDER BY caller.file, caller.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn {must_use: true}` | Functions annotated with `#[must_use]` (Rust), `@CheckReturnValue` (Java), or equivalent — not yet a supported node property in v0.3.0. |
| `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | The call site is on an exception-conditioned path — inside a `catch`, `Err` arm, or panic handler. |
| `NOT EXISTS { MATCH (caller)-[:DATA_FLOW*1..1]->(use_site) }` | The return value has no outgoing data-flow edge from the call site — it is immediately dropped. Not supported in v0.3.0. |

**Reading the result** — Each row is a call to a `#[must_use]` function on an error path where the return value is discarded. In Rust, this typically means error propagation is silently swallowed: a `Result<_, E>` is computed and thrown away, leaving the caller in a potentially inconsistent state. Cross-reference with Q101 for the broader ignored-error pattern.

---

### Q45 — What transactions are left uncommitted if `saveOrder()` throws?

**Personas:** SSE · **Status:** deferred (v0.3) — `is_return_site` is an unknown node property (plan error, exit 2) in v0.3.0. The NONE-based path query is otherwise supported; the workaround is to drop the `exit_node.is_return_site = true` filter and traverse to all reachable callees, accepting slightly broader results. The layer-1 `cgx paths` command is answerable today for verifying single-arm reachability.

If a function that begins a database transaction throws before committing or rolling back, the transaction leaks — the database holds locks and the application state is inconsistent. This question uses the Q-22 acquire/release pairing predicate to find paths from `db::Connection::begin` (or your transaction-begin call) that reach an exit without a `commit` or `rollback`. This is the resource-leak pattern from docs/05 Worked Example 2.

**The query**

`cgx paths db::Connection::begin db::Connection::commit --repo ./` — checks one arm; use the Layer-2 form below to test both outcomes and filter by exception path (no `--from`/`--to`, `--quantifier`, `--including-exception-paths`, or `--assert-all-reach-sink` flags exist):

```cgx
MATCH path = (acquire {name:"db::Connection::begin"})-[:CALLS*]->(any_node)
WHERE NONE(n IN nodes(path)
           WHERE n.name IN ["db::Connection::commit",
                             "db::Connection::rollback"])
RETURN acquire.file, acquire.line,
       any_node.name, any_node.file, any_node.line,
       ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
         AS on_exception_path
ORDER BY on_exception_path DESC, acquire.file
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(acquire {name:"db::Connection::begin"})-[:CALLS*]->(any_node)` | Traverse all paths from the transaction-begin call transitively. Substitute your actual transaction-begin symbol. |
| `is_return_site = true` (deferred) | This filter would restrict to actual return/exit sites; omitted in v0.3.0 since the property is unsupported. Results include all reachable callees, not just exit nodes. |
| `NONE(n IN nodes(path) WHERE n.name IN ["...commit", "...rollback"])` | Require that neither commit nor rollback appears on the path — meaning the transaction is leaked. |
| `on_exception_path` | Boolean: whether the leaked path is in the exceptional class. Exception-path leaks are the most common category. |

**Reading the result** — Non-empty results indicate paths from the transaction-begin that lack a commit or rollback. Rows where `on_exception_path = true` are the exception-path leaks (the most common case). Rows where `on_exception_path = false` are leaks even on the happy path — higher severity. In CI, run the query with `--assert-empty` to enforce that no leaked-transaction path exists.

---

### Q76 — Which exception handlers implement fail-open logic (catch block allows execution to continue without re-checking auth)?

**Personas:** PSE · **Status:** answerable-today (NOVEL)

A fail-open error handler catches an exception from an authorization check and allows execution to continue as if the check had passed. This is the classic "catch-and-ignore" authorization bypass: the exception from the auth check is swallowed, and the protected resource is accessed without a valid authorization result. This question finds exception-path routes to protected resources that bypass the auth check.

**The query**

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(resource {name:"protected_resource"})
WHERE ANY(r IN relationships(path) WHERE r.condition = "exception")
  AND NONE(n IN nodes(path)
           WHERE n.name IN ["authorize", "authenticate", "require_auth",
                             "check_permission"]
             AND position_in(n, path) < position_in(resource, path))
RETURN ep.name, ep.file, ep.line,
       resource.name, resource.file, resource.line,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS hops
ORDER BY ep.file, ep.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ANY(r IN relationships(path) WHERE r.condition = "exception")` | The path includes at least one exception-class edge, placing part of the execution inside an error handler. |
| `NONE(n IN nodes(path) WHERE n.name IN ["authorize", ...] AND position_in(n, path) < position_in(resource, path))` | No authorization check appears *before* the protected resource on this path. The `position_in` helper ensures ordering — an auth check that appears after the resource access does not satisfy the predicate. |

**Reading the result** — Each row is an exception-path route to a protected resource that skips all authorization checks. These are fail-open bypasses: the catch block either re-raises to a handler that grants access, or simply allows the code to fall through. Substitute your actual protected-resource and auth-check names.

---

### Q77 — Are there catch-all exception handlers (`catch Exception`, `recover()`) that silently swallow errors on security-critical paths?

**Personas:** PSE · **Status:** deferred (v0.3) — the `sink_class` node property causes a plan error (exit 2) in v0.3.0. Rewrite the log-detection inner match with explicit name filters (see below) until security-typed taint ships.

Catch-all handlers that swallow exceptions on paths to security-critical operations hide errors, suppress audit events, and can mask exploitable conditions. This question finds exception-conditioned paths to sensitive operations where the only exception handling is a broad catch-all that does not re-raise or log.

**The query**

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sensitive)
WHERE sensitive.name IN ["db::write", "exec", "authenticate", "crypto::decrypt"]
  AND ANY(r IN relationships(path) WHERE r.condition = "exception")
  AND ANY(n IN nodes(path)
          WHERE n.name IN ["recover", "catch_unwind", "catch_all"]
            AND NOT EXISTS {
              MATCH (n)-[:CALLS*1..3]->(log_or_rethrow)
              WHERE log_or_rethrow.name IN ["log::error!", "log::warn!",
                                            "tracing::error!", "eprintln!",
                                            "raise", "throw", "panic!", "return Err"]
            })
RETURN ep.name, ep.file, ep.line,
       sensitive.name, sensitive.file, sensitive.line,
       length(path) AS hops
ORDER BY sensitive.name, ep.file
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `sensitive.name IN [...]` | Substitute the set of security-critical operations relevant to your codebase: database writes, shell execution, auth functions, crypto operations. |
| `ANY(n IN nodes(path) WHERE n.name IN ["recover", "catch_unwind", "catch_all"])` | The path passes through a catch-all handler pattern — `recover()` in Go, `catch_unwind` in Rust, a bare `catch (Exception e)` in Java. |
| `NOT EXISTS { ... log_or_rethrow ... }` | The catch-all does NOT call a known log function or re-raise the error within 3 call hops — confirming it silently swallows the exception. `sink_class:"log"` is replaced by explicit name matching until taint typing is available. |

**Reading the result** — Each row is a security-critical path where a broad catch-all swallows the exception without logging or re-raising. The absence of a log call within 3 hops of the catch-all is a heuristic; widen or narrow the depth as needed for your codebase.

---

### Q78 — Which multi-step transaction functions do NOT have compensating calls (rollback/undo) on their exception paths?

**Personas:** SSE · **Status:** deferred (v0.3) — `is_return_site` is an unknown node property (plan error, exit 2) in v0.3.0. Drop the `exit_node.is_return_site = true` filter as a workaround; results will include all reachable callees on exception paths rather than only exit nodes.

Multi-step operations that modify state — create order, charge card, update inventory — must have compensating logic (rollback, undo, compensating transaction) on every error path to avoid leaving the system in a partially-updated inconsistent state. This question extends the Q45 transaction-pairing pattern to user-defined compensating operations.

**The query (v0.3.0 workaround — omit is_return_site filter)**

```cgx
MATCH path = (begin_op {name:"start_multi_step"})-[:CALLS*]->(any_node)
WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  AND NONE(n IN nodes(path)
           WHERE n.name IN ["rollback", "compensate", "undo",
                             "reverse_charge", "restore_inventory"])
RETURN begin_op.name, begin_op.file, begin_op.line,
       any_node.name, any_node.file, any_node.line,
       [r IN relationships(path) | r.condition] AS edge_conditions
ORDER BY begin_op.file, begin_op.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(begin_op {name:"start_multi_step"})` | The starting point of the multi-step operation. Substitute your actual function name, or use `name STARTS WITH "begin_"` for a broader sweep. |
| `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Restrict to exception-class paths — the failure paths that most commonly miss compensating logic. |
| `NONE(n IN nodes(path) WHERE n.name IN ["rollback", "compensate", ...])` | The path must not include any compensating operation. Extend this list with your domain-specific undo functions. |
| `is_return_site = true` (deferred) | Omitted in v0.3.0; would restrict to function exit sites only. Results include all exception-path callees. |

**Reading the result** — Each row is an exception-path route from the multi-step operation that lacks compensation. For each finding, trace which state mutations (charge, inventory decrement, order creation) were performed before the exit to assess the business impact of the missing rollback.

---

### Q98 — Which catch sites on authentication code paths discard the error and allow execution to continue on the happy path?

**Personas:** PSE · **Status:** deferred (v0.3) — two blockers: (1) the `sink_class` node property causes a plan error (exit 2); (2) `path =` binding over a multi-relationship pattern (`(a)-[:CALLS*]->(b)-[:CALLS*]->(c)`) causes a plan error ("a `path =` binding over a multi-relationship pattern is not yet supported") in v0.3.0. Until multi-hop path binding ships, the query cannot be expressed as written. Replace `n.sink_class = "log"` with an explicit name list when adapted to a supported pattern. Q76 covers fail-open authorization handlers; Q98 is distinct: it targets authentication code specifically and requires the discard-and-continue pattern.

An exception thrown during authentication — a database timeout, a network error reaching the identity provider — can be caught and discarded, allowing execution to proceed as if authentication succeeded. This question targets authentication specifically (not authorization) and looks for the discard-and-continue pattern: a catch site that does not re-raise and does not set a failure indicator.

**The query**

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(authn_call)-[:CALLS*]->(post_authn)
WHERE authn_call.name IN ["authenticate", "verify_credentials",
                           "check_token", "validate_session"]
  AND ANY(r IN relationships(path) WHERE r.condition = "exception")
  AND NONE(n IN nodes(path)
           WHERE (n.name IN ["raise", "throw", "return Err", "return false",
                              "set_auth_failure",
                              "log::error!", "log::warn!", "tracing::error!",
                              "eprintln!"])
             AND position_in(n, path) > position_in(authn_call, path)
             AND position_in(n, path) < position_in(post_authn, path))
RETURN ep.name, ep.file, ep.line,
       authn_call.name, authn_call.file, authn_call.line,
       post_authn.name, post_authn.file, post_authn.line,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `authn_call.name IN ["authenticate", ...]` | The authentication call site — substitute with the function names for your authentication layer. |
| `ANY(r IN relationships(path) WHERE r.condition = "exception")` | An exception-class edge appears between the entrypoint and the post-authn code, meaning a catch site is present. |
| `NONE(n IN nodes(path) WHERE (n.name IN ["raise", ...]) AND position_in(n, path) > position_in(authn_call, path))` | Between the auth call and the next operation, no re-raise or known log call appears — the exception is silently discarded. `n.sink_class = "log"` is not available in v0.3.0; the name list substitutes until taint typing ships. |

**Reading the result** — Each row names an authentication call site where an exception can be caught and execution continues into `post_authn` without a confirmed authentication result. This is the authentication-specific variant of fail-open: the error is discarded, not propagated, and the flow continues as if the auth check passed.

---

### Q101 — Which call sites ignore this function's error or `Result` return value — the return is dropped without any match or `?`?

**Personas:** SSE · **Status:** deferred (v0.3) — three blockers: (1) `call` is a reserved keyword; using it as a relationship variable name causes a parse error (exit 2) — use `edge` or another non-keyword name instead; (2) `returns_result` and `transitive_effects` are unknown node properties (plan error, exit 2); (3) `NOT EXISTS { MATCH ... }` subquery syntax causes a parse error (exit 2) in v0.3.0. All three features are required for the full query. A partial workaround with name-based filtering is feasible once the subquery syntax ships.

In Rust, ignoring a `Result` means the error case is silently discarded; in Go, assigning to `_` has the same effect. Engineers auditing error-handling completeness need to find all call sites where a fallible function's return value is not consumed. This question finds call edges to functions with `io.*` or `io.db` effects where the return value has no downstream data-flow.

**The query (blocked until NOT EXISTS subqueries, returns_result, and transitive_effects ship)**

```cgx
MATCH (caller)-[edge:CALLS]->(fn)
WHERE ("io.file" IN fn.transitive_effects
       OR "io.net" IN fn.transitive_effects
       OR "io.db" IN fn.transitive_effects
       OR fn.returns_result = true)
  AND NOT EXISTS {
    MATCH (caller)-[:DATA_FLOW*1..1]->(use_site)
    WHERE use_site.scope = caller.name
      AND use_site.derives_from_call = edge.id
  }
RETURN caller.name, caller.file, caller.line,
       fn.name, fn.file, fn.line,
       edge.condition AS call_condition
ORDER BY caller.file, caller.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.returns_result = true` | Functions that return `Result<_, _>` (Rust), `(T, error)` (Go), or equivalent — not yet a supported node property in v0.3.0. |
| `fn.transitive_effects` | Effect-set membership — not yet a supported node property in v0.3.0. |
| `NOT EXISTS { MATCH (caller)-[:DATA_FLOW*1..1]->(use_site) ... }` | The call site has no outgoing data-flow edge carrying the return value — not yet supported as subquery syntax in v0.3.0. |
| `edge.condition` | The edge condition of the ignored call (note: `call` is a reserved keyword; use `edge` or another non-keyword variable name). |

**Reading the result** — Each row is a call site where a fallible function's return value is discarded. In Rust, the compiler warns about `#[must_use]` specifically, but not all `Result`-returning functions carry the attribute. This query catches the broader set. Filter by `call_condition = "always"` to prioritize unconditional discards.

---

### Q103 — Are there execution paths on which `commit()` is called more than once, or on which neither `commit()` nor `rollback()` is called?

**Personas:** SSE · **Status:** deferred (v0.3) — the first sub-query uses `last_node(path).is_exit` which causes a parse error (exit 2): property access on a function call result (`last_node(path).is_exit`) is not supported syntax in v0.3.0. The second sub-query (double-commit, using `position_in` and node inequality) is answerable-today. Drop `last_node(path).is_exit = true` from the first query until property access on function-call results ships.

A double-commit corrupts database state; a missing commit-or-rollback leaks the transaction. This question uses the Q-22 ordering and pairing predicates to check transaction lifecycle correctness. The two sub-questions are complementary.

**The query**

For the missing-commit/rollback case (v0.3.0 workaround — omit last_node filter):

```cgx
MATCH path = (acquire {name:"db::Connection::begin"})-[:CALLS*]->(any_node)
WHERE NOT ANY(n IN nodes(path)
              WHERE n.name IN ["db::Connection::commit",
                               "db::Connection::rollback"])
RETURN path
```

For the double-commit case (answerable-today):

```cgx
MATCH path = (acquire {name:"db::Connection::begin"})-[:CALLS*]->(commit2)
WHERE commit2.name = "db::Connection::commit"
  AND ANY(n IN nodes(path)
          WHERE n.name = "db::Connection::commit"
            AND n <> commit2
            AND position_in(n, path) < position_in(commit2, path))
RETURN acquire.file, acquire.line,
       commit2.file, commit2.line,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `NOT ANY(n IN nodes(path) WHERE n.name IN ["...commit", "...rollback"])` | No commit or rollback appears anywhere on the path from `begin` — the transaction is leaked. |
| `last_node(path).is_exit = true` (deferred) | Would restrict to function exit sites; omitted in v0.3.0 since property access on function-call results is not yet supported. |
| Second query: `n <> commit2 AND position_in(n, path) < position_in(commit2, path)` | A different `commit` call appears earlier on the same path — a double-commit. This form is supported in v0.3.0. |

**Reading the result** — Missing-commit/rollback results (first query) are the most common case and often appear on exception paths. Double-commit results (second query) indicate logic errors in retry or error-recovery code. Both can be converted to CI assertions using `--assert-empty`.

---

### Q106 — Which functions mutate shared state and then reach a `panic!` or `unwrap` call, leaving invariants in a partially-mutated state?

**Personas:** SSE · **Status:** deferred (v0.3) — `own_effects` is an unknown node property (plan error, exit 2) in v0.3.0. The `transitive_effects` property is also unsupported. Until effect-set properties ship, the state-mutation filter cannot be applied; the panic-path portion of the query (name-based match on panic sites with `condition = "panic"`) is answerable-today.

Rust's `Mutex` poisoning exists specifically because a function can mutate shared state and then panic before restoring invariants. This leaves the mutex in a poisoned state. More generally, any function that writes to shared state and then panics risks corrupting invariants. This question finds functions that both write global/shared state (via `writes-global` or `writes-receiver` effect) and have a `panic`-conditioned edge to a `panic!`/`unwrap` call.

**The query (when own_effects ships)**

```cgx
MATCH path = (fn)-[:CALLS*]->(panic_site)
WHERE ("writes-global" IN fn.own_effects
       OR "writes-receiver" IN fn.own_effects)
  AND panic_site.name IN ["panic!", "unwrap", "expect", "assert!",
                           "unreachable!", "todo!"]
  AND ANY(r IN relationships(path) WHERE r.condition = "panic")
RETURN fn.name, fn.file, fn.line,
       fn.own_effects,
       panic_site.name, panic_site.file, panic_site.line,
       length(path) AS hops
ORDER BY fn.file, fn.line
```

**v0.3.0 workaround (name-based panic-path scan, no mutation filter)**

```cgx
MATCH path = (fn)-[:CALLS*]->(panic_site)
WHERE panic_site.name IN ["panic!", "unwrap", "expect", "assert!",
                           "unreachable!", "todo!"]
  AND ANY(r IN relationships(path) WHERE r.condition = "panic")
RETURN fn.name, fn.file, fn.line,
       panic_site.name, panic_site.file, panic_site.line,
       length(path) AS hops
ORDER BY fn.file, fn.line
```

Note: `(fn)-[:CALLS*]->` without a starting-node constraint is a full-graph scan. Use `--depth 2` or `--depth 3` to bound the search on large codebases. This workaround returns all functions with a panic-conditioned path to a panic site, without filtering by mutation effects. Manually review results for mutation patterns.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `"writes-global" IN fn.own_effects OR "writes-receiver" IN fn.own_effects` | The function writes to module-level state or mutates `self` — not yet a supported node property in v0.3.0. |
| `panic_site.name IN ["panic!", "unwrap", ...]` | Panic-inducing call sites. In Rust these produce `panic`-conditioned edges. |
| `ANY(r IN relationships(path) WHERE r.condition = "panic")` | Confirm that the path from the function to the panic site includes a `panic`-conditioned edge — supported in v0.3.0. |

**Reading the result** — Each row is a function that can reach a panic site via a panic-conditioned edge. With `own_effects` available, results will be narrowed to functions with direct mutation effects. Review: Is there a `Drop` implementation or a `defer` statement that restores the invariant even on the panic path? Does the `Mutex` poisoning check propagate correctly to callers? High-priority findings are functions with `writes-global` effects (cross-function scope) rather than just `writes-receiver` (instance scope).

