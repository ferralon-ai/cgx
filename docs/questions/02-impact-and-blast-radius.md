# Theme 2: Impact and Blast Radius

This theme covers the 12 questions that ask how far the effects of a change, a failure, or a refactor will ripple through the codebase. The blast radius of a function is the set of all callers that depend on it — directly or transitively. These questions help engineers plan refactors safely, understand what breaks when a signature changes, and give AI coding agents the minimal subgraph context needed to edit a function without introducing regressions. The common capability across this theme is `cgx`'s `callers`, `callees`, and subgraph-extraction queries, combined with depth limits and intersection operations.

Readers familiar with "find all usages" in an IDE will recognize the pattern — these queries extend it to multi-hop transitive reach, test coverage mapping, and graph intersection, which IDE tooling does not provide.

---

### Q14 — If I change the signature of `UserRepository.findById()`, which callers will break and which tests cover those callers?

**Personas:** SSE · **Status:** partial — caller enumeration is answerable-today; `has_test_coverage` subquery is deferred (v0.3 CQL parse error)

A signature change — adding a parameter, changing a return type — will break every direct caller. But knowing the immediate callers is not enough: an engineer also needs to know whether each affected caller has test coverage, because uncovered callers represent regression risk.

**The query**

```cgx
cgx callers UserRepository::findById --repo ./ --depth 3 --format json
```

The CQL form below uses `EXISTS { MATCH ... }` as a RETURN expression, which is not supported in v0.3 (parse error, exit 2). The `entrypoint_class` node property is also not supported in v0.3 (plan error, exit 2). Both are deferred.

```cgx
# NOTE: exits 2 in v0.3 — EXISTS subquery and entrypoint_class are deferred
cgx query '
  MATCH (caller)-[:CALLS*1..3]->(fn {name:"UserRepository::findById"})
  RETURN caller.name, caller.file, caller.line,
         caller.kind,
         EXISTS {
           MATCH (test {kind:"entrypoint", entrypoint_class:"test"})-[:CALLS*]->(caller)
         } AS has_test_coverage
  ORDER BY has_test_coverage, caller.file
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(caller)-[:CALLS*1..3]->(fn {name:"UserRepository::findById"})` | Find all callers of the function within 3 hops — depth 3 catches direct callers (1 hop), callers of callers (2 hops), and one level deeper. Adjust `1..3` for the desired blast radius depth. |
| `caller.kind` | The node kind — `function`, `method`, `lambda` — helps distinguish production code from test helpers. |
| `EXISTS { MATCH (test {kind:"entrypoint", entrypoint_class:"test"})-[:CALLS*]->(caller) }` | A subquery that checks whether any test entrypoint can reach this caller. The result is a boolean `has_test_coverage`. — deferred in v0.3 (exits 2): `EXISTS { MATCH ... }` subqueries and the `entrypoint_class` node property are not supported. |
| `ORDER BY has_test_coverage` | Puts uncovered callers (`false`) first — those are the highest-risk changes. |

**Reading the result** — Each row is a caller that will break. The `has_test_coverage` boolean shows whether a test can reach it. Callers with `has_test_coverage = false` have no test coverage and represent silent regressions if the signature changes break them.

---

### Q15 — Which modules depend on `PaymentProcessor` and would be affected by extracting it to a microservice?

**Personas:** SSE · **Status:** partial — caller enumeration is answerable-today; `count(distinct ...)` and `collect(distinct ...)` inside aggregates are deferred (v0.3 parse error)

Extracting a module to a microservice severs all direct in-process call edges to it and replaces them with network calls. To plan this extraction, an engineer needs to know every module that calls into `PaymentProcessor` — these modules will require changes to switch from direct calls to client library calls. Call-graph callers are the *structural* half of that plan; the *historical* half — which files tend to get touched in the same commit as `PaymentProcessor`'s, whether or not a call edge connects them (a shared migration script, a feature-flag file, deployment config) — comes from `cgx coupling`, which is worth pulling before scoping the extraction, not after.

**The query**

```cgx
cgx callers PaymentProcessor --repo ./ --depth 5 --format json
```

For the historical half, walk the commit range that matters (a release window, or the module's whole lifetime) and look for files that co-change with `payment_processor.rs` above the noise floor:

```cgx
cgx coupling v1.0.0 HEAD --repo ./ --min-cochanges 3
```

This reads committed history only — no index, no working tree — so it runs even before the module is indexed. A file that co-changes with `payment_processor.rs` at a high rate but never appears in the `cgx callers` output is exactly the case a pure call-graph analysis misses: something outside the call graph (a test fixture, a config schema, an IaC file) that extraction will also have to touch.

The CQL form below uses `count(distinct ...)` and `collect(distinct ...)` inside aggregates, which are not supported in v0.3 (parse error, exit 2). `DISTINCT` inside aggregate functions is deferred; use `RETURN DISTINCT` at the top level instead.

```cgx
# NOTE: exits 2 in v0.3 — count(distinct) and collect(distinct) are deferred
cgx query '
  MATCH (caller)-[:CALLS*1..5]->(callee)-[:MEMBER_OF]->(t {name:"PaymentProcessor"})
  RETURN distinct caller.file AS module_file,
         count(distinct callee.name) AS distinct_entry_points_called,
         collect(distinct callee.name) AS called_methods
  ORDER BY distinct_entry_points_called DESC
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(callee)-[:MEMBER_OF]->(t {name:"PaymentProcessor"})` | Restrict the sink to nodes that are members of the `PaymentProcessor` type — methods and functions defined on it. |
| `RETURN distinct caller.file AS module_file` | Group by file (module) rather than by individual function — gives the list of modules, not individual call sites. |
| `count(distinct callee.name) AS distinct_entry_points_called` | How many different `PaymentProcessor` methods this module calls. A module that calls many methods has a deeper coupling and will require more migration work. — deferred in v0.3 (exits 2): `count(distinct ...)` inside aggregate functions is not supported; use `RETURN DISTINCT` at the top level instead. |
| `collect(distinct callee.name) AS called_methods` | The specific methods called, for the migration checklist. — deferred in v0.3 (exits 2): `collect(distinct ...)` inside aggregate functions is not supported. |
| `--min-cochanges 3` | Noise floor: only report file pairs that changed together at least 3 times in the range — a single shared commit (a repo-wide rename, say) is not a coupling signal. |

**Reading the result** — Each row from `cgx callers`/the CQL form is a module (file) and the `PaymentProcessor` methods it depends on; sort by `distinct_entry_points_called` descending to prioritize the highest-coupling modules for migration planning. Each row from `cgx coupling` is a file pair with `cochanges`/`changes_a`/`changes_b` counts and carries its own `approximation:` line (file-granularity, bounded to the given rev range, renames not tracked as continuity) — read it as file-level co-change, not a same-symbol claim: two files can co-change because unrelated symbols in each were touched in the same commit. Cross-reference the two: a caller with high co-change *and* a direct call edge is a confirmed dependency; a high co-change file with no call edge is the hidden coupling extraction planning would otherwise miss.

---

### Q16 — I'm about to edit function `F`. Give me the minimal subgraph (callers up to depth 2, callees up to depth 3) needed to understand the change impact.

**Personas:** ACA · **Status:** answerable-today

An AI coding agent editing a function needs to understand what calls into it (callers) and what it calls out to (callees) to reason about the change impact. The conventional IDE approach of loading entire files is token-inefficient; `cgx` returns exactly the graph neighborhood. This is the primary AI agent use case described in docs/05 (depth-limited blast radius pattern).

**The query**

From docs/05 (depth-limited blast radius question class):

```cgx
cgx callers OrderService::submit --repo ./ --depth 2 --format json > callers.json
cgx callees OrderService::submit --repo ./ --depth 3 --format json > callees.json
```

Or as a single subgraph query:

```cgx
cgx query '
  MATCH (c)-[:CALLS*1..2]->(fn {name:"OrderService::submit"})-[:CALLS*1..3]->(d)
  RETURN c, fn, d
' --repo ./
```

`cgx callers F --repo ./ --depth 2` with a Layer 1 note: the full query form above also exists and lets you adjust depth in both directions in one call.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(c)-[:CALLS*1..2]->(fn {name:"OrderService::submit"})` | Find all callers within 2 hops of the target function — the functions that directly call it, and the functions that call those callers. |
| `(fn)-[:CALLS*1..3]->(d)` | From the target function, follow call edges up to 3 hops into its callees — the functions it calls, and the functions those call. |
| `RETURN c, fn, d` | Return all three node sets in one result — the complete neighborhood. |

**Reading the result** — The two-direction subgraph gives the agent the context needed to reason about change impact: callers show who will be affected by changes to the function's signature or behavior; callees show what the function depends on. The `--format json` output is structured for programmatic consumption.

---

### Q17 — Which functions have no test coverage when traced from test entrypoints through the call graph?

**Personas:** SSE · **Status:** partial — `cgx unused` is answerable-today; the CQL form is deferred (`EXISTS { MATCH ... }` and `entrypoint_class` property exit 2 in v0.3)

Call-graph-based test coverage is more precise than line-coverage tools: it identifies functions that are never reachable from any test entrypoint, not just functions whose lines are not executed. A function that is "covered" by a test that calls a high-level wrapper but never reaches the function itself shows up as covered in line tools but uncovered here.

**The query**

```cgx
cgx unused --repo ./ --kind function --confidence certain
```

The CQL form below uses `EXISTS { MATCH ... }` subqueries and the `entrypoint_class` node property, neither of which is supported in v0.3 (parse error / plan error, exit 2). Both are deferred.

```cgx
# NOTE: exits 2 in v0.3 — EXISTS subquery and entrypoint_class are deferred
cgx query '
  MATCH (fn)
  WHERE fn.kind IN ["function","method"]
    AND NOT EXISTS {
      MATCH (test {kind:"entrypoint", entrypoint_class:"test"})-[:CALLS*]->(fn)
    }
    AND EXISTS {
      MATCH (prod_ep {kind:"entrypoint"})-[:CALLS*]->(fn)
      WHERE prod_ep.entrypoint_class <> "test"
    }
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `NOT EXISTS { MATCH (test {entrypoint_class:"test"})-[:CALLS*]->(fn) }` | No test entrypoint can reach this function — it has no test coverage on the call graph. — deferred in v0.3 (exits 2): `NOT EXISTS { MATCH ... }` subqueries and the `entrypoint_class` node property are not supported. |
| `EXISTS { MATCH (prod_ep)-[:CALLS*]->(fn) WHERE prod_ep.entrypoint_class <> "test" }` | A production entrypoint can reach it — the function is production code, not test infrastructure. This filter excludes test-only helper functions from the "no test coverage" report. — deferred in v0.3 (exits 2): `EXISTS { MATCH ... }` subqueries and the `entrypoint_class` node property are not supported. |

**Reading the result** — Each row is a production function that is reachable from production entrypoints but not from any test entrypoint. These are the true test coverage gaps at the call-graph level.

---

### Q18 — If `ConfigLoader.parse()` returns an error, which functions in the startup sequence won't be executed?

**Personas:** SSE · **Status:** partial — the `NONE()` path filter is answerable-today; the `EXISTS { MATCH ... }` NOT-EXISTS form is deferred (v0.3 parse error, exit 2)

Early errors in a startup sequence can cause later initialization functions to be skipped. Understanding which functions are short-circuited by an early failure helps an engineer reason about partial-initialization bugs and the cleanup required on error paths.

**The query**

The complement — functions that are always executed regardless of the error — uses `NONE()`, which is supported in v0.3:

```cgx
cgx query '
  MATCH path = (startup {name:"main"})-[:CALLS*]->(fn)
  WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' --repo ./
```

The primary query (finding functions reachable only via error paths) uses `EXISTS { MATCH ... }` / `NOT EXISTS { MATCH ... }`, which is not supported in v0.3 (parse error, exit 2). It is deferred.

```cgx
# NOTE: exits 2 in v0.3 — NOT EXISTS { MATCH ... } subquery is deferred
cgx query '
  MATCH (startup {name:"main"})-[:CALLS*]->(fn)
  WHERE NOT EXISTS {
    MATCH path = (startup)-[:CALLS*]->(fn)
    WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  }
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(startup {name:"main"})-[:CALLS*]->(fn)` | Find all functions reachable from the startup entrypoint. |
| `NOT EXISTS { MATCH path … NONE(r … exception/panic) }` | The function has no non-exception path from `main` — it is only reachable if the startup takes an error branch at some point. — deferred in v0.3 (exits 2): `NOT EXISTS { MATCH ... }` subqueries are not supported. |

**Reading the result** — Functions in the result set are those that will not run if an error occurs early in startup. Compare this with the cleanup and resource-release functions to find initialization that is skipped on error, potentially leaving resources uninitialized or partially configured.

---

### Q19 — What is the set of all functions that could be executing when we crash at `panic_handler`?

**Personas:** SSE · **Status:** answerable-today

When a Rust `panic!` fires, the execution context may involve a deep call stack. To understand the crash, an engineer needs to know the full set of functions that are in the call chain at the time of the panic — everything that was executing and would be unwound. This is the inverse blast radius: all callers of `panic_handler`, transitively.

**The query**

```cgx
cgx callers panic_handler --repo ./ --depth 20 --format json

cgx query '
  MATCH (caller)-[:CALLS*]->(ph {name:"panic_handler"})
  RETURN caller.name, caller.file, caller.line,
         caller.kind
  ORDER BY caller.file, caller.name
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(caller)-[:CALLS*]->(ph {name:"panic_handler"})` | Find every function in the graph that can reach `panic_handler` through any chain of call edges. |
| `--depth 20` | Set a generous depth limit to capture long call chains. Adjust based on the codebase's typical call depth. |

**Reading the result** — The result set is the complete set of functions that could be on the call stack when a panic fires. This is the "blast radius from the panic" — every function that could be interrupted. Sort by file to cluster related functions for review.

---

### Q20 — After I edit function `G`, show me any new call edges that were introduced and whether they reach any dangerous sinks.

**Personas:** ACA · **Status:** partial — new-edge detection is answerable-today; sink-class filtering is deferred (v0.3 plan error)

An AI coding agent that has just edited a function needs to verify its own work. After the edit, the agent queries the post-edit call graph to check whether any new edges were introduced — a self-verification step before committing. Filtering those new edges by sink class (sql, shell, etc.) requires the security-typed taint layer, which is deferred past v0.3.

**The query — new edges (answerable today)**

```cgx
cgx diff HEAD~1 HEAD --repo ./
```

`cgx diff` takes `<BASE> <HEAD>` as positional arguments followed by `--repo`. It has no `--base`, `--head`, or `--calls-to-sink-class` flags. Use `--newer-than` to restrict output to edges added at HEAD but absent at BASE:

```cgx
cgx diff HEAD~1 HEAD --repo ./ --newer-than
```

To find what the edited function now calls that it did not before, combine with `cgx callees` at each ref:

```cgx
cgx callees G --repo ./ --at HEAD~1 --format json > before.json
cgx callees G --repo ./ --at HEAD   --format json > after.json
```

**The query — sink-class filtering (deferred, exits 2 in v0.3)**

The following pattern uses `b.sink_class`, which is a security-typed taint property not yet backed in v0.3. Running it produces a plan error (exit 2):

```cgx
# NOTE: exits 2 in v0.3 — sink_class is deferred
cgx query '
  MATCH (a)-[r:CALLS]->(b)
  WHERE b.sink_class IN ["sql","shell","path","net-request","eval"]
  RETURN a.name, a.file, a.line,
         b.name, b.file, b.line
  ORDER BY a.file
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff HEAD~1 HEAD --repo ./` | Compare the call graph at the prior commit against the current HEAD — shows what edges were added or removed by the most recent commit. BASE and HEAD are positional arguments. |
| `--newer-than` | Boolean flag; restricts output to edges present at HEAD but absent at BASE. |
| `--at HEAD` / `--at HEAD~1` | Pin `callees` to a specific git ref's graph — compare the function's callee set before and after the edit. |
| `b.sink_class IN [...]` | Deferred taint property — plan error (exit 2) in v0.3; available in a future release. |

**Reading the result** — The `cgx diff` output shows every edge that was added (`+`) or removed (`-`) between the two refs. New `+` edges from the edited function are the primary concern. Until sink-class classification is available, review new edges manually against known sensitive operations.

---

### Q21 — Which functions are called by both the request-processing path and the background job path?

**Personas:** SSE · **Status:** answerable-today

Functions shared between the request-processing path and the background job path are a source of subtle bugs: they may have different concurrency requirements, different performance constraints, or different assumptions about caller state. Identifying the intersection helps engineers reason about whether a change to a shared function will affect both paths.

**The query**

```cgx
cgx query '
  MATCH (req_ep {name:"request_handler"})-[:CALLS*]->(shared)
  MATCH (bg_ep {name:"background_job_runner"})-[:CALLS*]->(shared)
  WHERE req_ep <> bg_ep
  RETURN shared.name, shared.file, shared.line
  ORDER BY shared.name
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| First `MATCH` | Find all functions reachable from the request-processing entrypoint. |
| Second `MATCH` | Find all functions reachable from the background job entrypoint. |
| `(shared)` | The same node appears in both result sets — it is in the intersection of the two call graphs. |
| `WHERE req_ep <> bg_ep` | Exclude the trivial case where both paths start at the same node. |

**Reading the result** — Each row is a function called from both the request path and the background job path. For concurrency safety, check whether these shared functions access mutable shared state without appropriate synchronization. For performance, check whether they assume low-latency callers.

---

### Q22 — Show me all callers of deprecated functions so I can plan the migration.

**Personas:** SSE · **Status:** partial — `cgx callers` on a named deprecated function is answerable-today; the CQL `deprecated.deprecated` property filter is deferred (unknown node property, exit 2 in v0.3)

Before removing a deprecated function, an engineer needs the full list of call sites to plan the migration. This is a direct blast radius query with a filter on the deprecation attribute.

**The query**

```cgx
cgx callers DeprecatedModule::old_function --repo ./ --depth 1 --format json
```

The CQL form below filters by `deprecated.deprecated = true`, which is an unknown node property in v0.3 (plan error, exit 2). The `deprecated` attribute is deferred.

```cgx
# NOTE: exits 2 in v0.3 — deprecated node property is deferred
cgx query '
  MATCH (caller)-[:CALLS]->(deprecated)
  WHERE deprecated.deprecated = true
  RETURN deprecated.name AS deprecated_fn,
         caller.name, caller.file, caller.line
  ORDER BY deprecated.name, caller.file
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(caller)-[:CALLS]->(deprecated)` | Find direct callers — depth 1 is sufficient here since we want the actual call sites, not the transitive blast radius. |
| `deprecated.deprecated = true` | Filter to functions marked as deprecated in the graph — this attribute is set when the function carries a `#[deprecated]` attribute (Rust), `@Deprecated` annotation (Java), or equivalent. — deferred in v0.3 (exits 2): the `deprecated` node property is not supported (plan error). |
| `ORDER BY deprecated.name` | Group results by deprecated function name, making the migration checklist easy to follow. |

**Reading the result** — Each row is a direct call site of a deprecated function. The result grouped by `deprecated.name` gives a migration checklist: each deprecated function and the files that must be updated.

---

### Q23 — Which tests become invalid if I rename `OrderService.submit()`?

**Personas:** SSE · **Status:** partial — the CQL query is deferred (`entrypoint_class` node property exits 2 in v0.3); use `cgx callers` with `--confidence certain` to approximate

Renaming a function will break any test that directly calls it or indirectly depends on it through a call chain. This is a blast radius query scoped to test entrypoints.

**The query**

Use `cgx callers` to enumerate all callers of the function (tests and production callers combined); the `entrypoint_class` property needed to filter to test-only callers is not supported in v0.3:

```cgx
cgx callers OrderService::submit --repo ./ --depth 5 --format json
```

The CQL form below uses `entrypoint_class`, which is not supported in v0.3 (plan error, exit 2). It is deferred.

```cgx
# NOTE: exits 2 in v0.3 — entrypoint_class node property is deferred
cgx query '
  MATCH path = (test {kind:"entrypoint", entrypoint_class:"test"})-[:CALLS*]->(fn {name:"OrderService::submit"})
  RETURN test.name, test.file, test.line,
         length(path) AS hops
  ORDER BY hops, test.file
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(test {kind:"entrypoint", entrypoint_class:"test"})` | Start from test entrypoints only — functions declared as test roots. — deferred in v0.3 (exits 2): the `entrypoint_class` node property is not supported (plan error). |
| `-[:CALLS*]->` | Follow call edges transitively from the test. |
| `(fn {name:"OrderService::submit"})` | The test must reach the function being renamed. |
| `ORDER BY hops` | Tests that call the function directly (1 hop) will break immediately; tests that reach it through helper chains (higher hop counts) may break more subtly. |

**Reading the result** — Each row is a test that will become invalid when `OrderService::submit` is renamed. The `hops` column tells you how directly the test depends on it: 1-hop tests call it directly and will fail at compilation; higher-hop tests may fail because an intermediate helper breaks.

---

### Q24 — What is the complete set of functions I must understand to safely implement a new middleware that intercepts `authenticate()`?

**Personas:** ACA · **Status:** answerable-today

A new middleware that wraps `authenticate()` must not break any existing caller. To implement it safely, the agent needs to understand: every function that currently calls `authenticate()` (so the middleware is compatible with all call sites), and every function that `authenticate()` calls (so the middleware can preserve the behavioral contract).

**The query**

```cgx
cgx callers authenticate --repo ./ --depth 2 --format json > callers.json
cgx callees authenticate --repo ./ --depth 3 --format json > callees.json

cgx query '
  MATCH (c)-[:CALLS*1..2]->(fn {name:"authenticate"})-[:CALLS*1..3]->(d)
  RETURN c.name AS caller, c.file AS caller_file,
         d.name AS callee, d.file AS callee_file
  ORDER BY c.file, d.file
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(c)-[:CALLS*1..2]->(fn {name:"authenticate"})` | Callers within 2 hops — the functions that call `authenticate()` and the functions that call those. The middleware must be compatible with all of these. |
| `(fn)-[:CALLS*1..3]->(d)` | Callees within 3 hops — what `authenticate()` calls. The middleware must preserve or correctly wrap all of this behavior. |

**Reading the result** — The caller set defines the API contract the middleware must honor; every caller must work the same way after the middleware is inserted. The callee set defines the behavior the middleware wraps; any side effect in that set is now the middleware's responsibility.

---

### Q25 — Which public API methods have their implementation entirely contained within a single module vs. spanning multiple modules?

**Personas:** SSE · **Status:** deferred — the CQL query requires `EXISTS { MATCH ... }`, `OPTIONAL MATCH`, `collect(distinct ...)`, arithmetic `+`, and `CASE WHEN`, all of which exit 2 in v0.3

API methods whose implementations span multiple modules create cross-module coupling and are harder to refactor or extract. Methods fully contained in a single module are better candidates for extraction. This query partitions the public API by implementation scope.

**The query**

The CQL query requires several features not supported in v0.3: `EXISTS { MATCH ... }` (parse error), `OPTIONAL MATCH` (plan error: deferred), `collect(distinct ...)` (parse error), arithmetic `+` (plan error: deferred), and `CASE WHEN` (parse error). All exit 2. This query is fully deferred.

```cgx
# NOTE: exits 2 in v0.3 — EXISTS subquery, OPTIONAL MATCH, collect(distinct), arithmetic, and CASE WHEN are all deferred
cgx query '
  MATCH (api:method {visibility:"public"})
  WHERE EXISTS {
    MATCH (ep {kind:"entrypoint"})-[:CALLS*]->(api)
  }
  OPTIONAL MATCH reach = (api)-[:CALLS*]->(callee)
  WHERE callee.kind IN ["function","method"]
  WITH api,
       collect(distinct callee.file) + [api.file] AS all_files
  RETURN api.name, api.file, api.line,
         size(all_files) AS module_count,
         all_files,
         CASE WHEN size(all_files) <= 1 THEN "single-module"
              ELSE "multi-module" END AS scope
  ORDER BY module_count DESC, api.name
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(api:method {visibility:"public"})` | Start from public methods — the externally visible API surface. |
| `EXISTS { MATCH (ep {kind:"entrypoint"})-[:CALLS*]->(api) }` | Confirm the method is reachable from a declared entrypoint — it is live public API, not dead code. — deferred in v0.3 (exits 2): `EXISTS { MATCH ... }` subqueries are not supported. |
| `OPTIONAL MATCH reach = (api)-[:CALLS*]->(callee)` | Transitively find all functions the API method calls. `OPTIONAL` ensures methods with no callees still appear. — deferred in v0.3 (exits 2): `OPTIONAL MATCH` is not supported. |
| `collect(distinct callee.file) + [api.file] AS all_files` | Collect every distinct file touched by the method and its full call tree, including the file where the API method itself is defined. — deferred in v0.3 (exits 2): `collect(distinct ...)` and list arithmetic `+` are not supported. |
| `size(all_files) <= 1` | Single-module: the entire implementation lives in one file. Multi-module: the implementation spans multiple files. — deferred in v0.3 (exits 2): `CASE WHEN` and `size()` on a collected list are not supported. |

**Reading the result** — Methods in the `single-module` group are the best candidates for extraction to a separate crate or microservice. Methods in the `multi-module` group have cross-module dependencies that would need to be resolved first. Sort by `module_count` descending to find the most entangled methods.
