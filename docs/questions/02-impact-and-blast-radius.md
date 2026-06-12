# Theme 2: Impact and Blast Radius

This theme covers the 12 questions that ask how far the effects of a change, a failure, or a refactor will ripple through the codebase. The blast radius of a function is the set of all callers that depend on it — directly or transitively. These questions help engineers plan refactors safely, understand what breaks when a signature changes, and give AI coding agents the minimal subgraph context needed to edit a function without introducing regressions. The common capability across this theme is `cgx`'s `callers`, `callees`, and subgraph-extraction queries, combined with depth limits and intersection operations.

Readers familiar with "find all usages" in an IDE will recognize the pattern — these queries extend it to multi-hop transitive reach, test coverage mapping, and graph intersection, which IDE tooling does not provide.

---

### Q14 — If I change the signature of `UserRepository.findById()`, which callers will break and which tests cover those callers?

**Personas:** SSE · **Status:** answerable-today

A signature change — adding a parameter, changing a return type — will break every direct caller. But knowing the immediate callers is not enough: an engineer also needs to know whether each affected caller has test coverage, because uncovered callers represent regression risk.

**The query**

```cgx
cgx callers UserRepository::findById ./ --depth 3 --format json

cgx query '
  MATCH (caller)-[:CALLS*1..3]->(fn {name:"UserRepository::findById"})
  RETURN caller.name, caller.file, caller.line,
         caller.kind,
         EXISTS {
           MATCH (test {kind:"entrypoint", entrypoint_class:"test"})-[:CALLS*]->(caller)
         } AS has_test_coverage
  ORDER BY has_test_coverage, caller.file
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(caller)-[:CALLS*1..3]->(fn {name:"UserRepository::findById"})` | Find all callers of the function within 3 hops — depth 3 catches direct callers (1 hop), callers of callers (2 hops), and one level deeper. Adjust `1..3` for the desired blast radius depth. |
| `caller.kind` | The node kind — `function`, `method`, `lambda` — helps distinguish production code from test helpers. |
| `EXISTS { MATCH (test {kind:"entrypoint", entrypoint_class:"test"})-[:CALLS*]->(caller) }` | A subquery that checks whether any test entrypoint can reach this caller. The result is a boolean `has_test_coverage`. |
| `ORDER BY has_test_coverage` | Puts uncovered callers (`false`) first — those are the highest-risk changes. |

**Reading the result** — Each row is a caller that will break. The `has_test_coverage` boolean shows whether a test can reach it. Callers with `has_test_coverage = false` have no test coverage and represent silent regressions if the signature changes break them.

---

### Q15 — Which modules depend on `PaymentProcessor` and would be affected by extracting it to a microservice?

**Personas:** SSE · **Status:** answerable-today

Extracting a module to a microservice severs all direct in-process call edges to it and replaces them with network calls. To plan this extraction, an engineer needs to know every module that calls into `PaymentProcessor` — these modules will require changes to switch from direct calls to client library calls.

**The query**

```cgx
cgx callers PaymentProcessor ./ --depth 5 --format json

cgx query '
  MATCH (caller)-[:CALLS*1..5]->(callee)-[:MEMBER_OF]->(t {name:"PaymentProcessor"})
  RETURN distinct caller.file AS module_file,
         count(distinct callee.name) AS distinct_entry_points_called,
         collect(distinct callee.name) AS called_methods
  ORDER BY distinct_entry_points_called DESC
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(callee)-[:MEMBER_OF]->(t {name:"PaymentProcessor"})` | Restrict the sink to nodes that are members of the `PaymentProcessor` type — methods and functions defined on it. |
| `RETURN distinct caller.file AS module_file` | Group by file (module) rather than by individual function — gives the list of modules, not individual call sites. |
| `count(distinct callee.name) AS distinct_entry_points_called` | How many different `PaymentProcessor` methods this module calls. A module that calls many methods has a deeper coupling and will require more migration work. |
| `collect(distinct callee.name) AS called_methods` | The specific methods called, for the migration checklist. |

**Reading the result** — Each row is a module (file) and the `PaymentProcessor` methods it depends on. Sort by `distinct_entry_points_called` descending to prioritize the highest-coupling modules for migration planning.

---

### Q16 — I'm about to edit function `F`. Give me the minimal subgraph (callers up to depth 2, callees up to depth 3) needed to understand the change impact.

**Personas:** ACA · **Status:** answerable-today

An AI coding agent editing a function needs to understand what calls into it (callers) and what it calls out to (callees) to reason about the change impact. The conventional IDE approach of loading entire files is token-inefficient; `cgx` returns exactly the graph neighborhood. This is the primary AI agent use case described in docs/05 (depth-limited blast radius pattern).

**The query**

From docs/05 (depth-limited blast radius question class):

```cgx
cgx callers OrderService::submit ./ --depth 2 --format json > callers.json
cgx callees OrderService::submit ./ --depth 3 --format json > callees.json
```

Or as a single subgraph query:

```cgx
cgx query '
  MATCH (c)-[:CALLS*1..2]->(fn {name:"OrderService::submit"})-[:CALLS*1..3]->(d)
  RETURN c, fn, d
' ./
```

`cgx callers F ./ --depth 2` with a Layer 1 note: the full query form above also exists and lets you adjust depth in both directions in one call.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(c)-[:CALLS*1..2]->(fn {name:"OrderService::submit"})` | Find all callers within 2 hops of the target function — the functions that directly call it, and the functions that call those callers. |
| `(fn)-[:CALLS*1..3]->(d)` | From the target function, follow call edges up to 3 hops into its callees — the functions it calls, and the functions those call. |
| `RETURN c, fn, d` | Return all three node sets in one result — the complete neighborhood. |

**Reading the result** — The two-direction subgraph gives the agent the context needed to reason about change impact: callers show who will be affected by changes to the function's signature or behavior; callees show what the function depends on. The `--format json` output is structured for programmatic consumption.

---

### Q17 — Which functions have no test coverage when traced from test entrypoints through the call graph?

**Personas:** SSE · **Status:** answerable-today

Call-graph-based test coverage is more precise than line-coverage tools: it identifies functions that are never reachable from any test entrypoint, not just functions whose lines are not executed. A function that is "covered" by a test that calls a high-level wrapper but never reaches the function itself shows up as covered in line tools but uncovered here.

**The query**

```cgx
cgx unused ./ --kind fn --confidence certain

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
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `NOT EXISTS { MATCH (test {entrypoint_class:"test"})-[:CALLS*]->(fn) }` | No test entrypoint can reach this function — it has no test coverage on the call graph. |
| `EXISTS { MATCH (prod_ep)-[:CALLS*]->(fn) WHERE prod_ep.entrypoint_class <> "test" }` | A production entrypoint can reach it — the function is production code, not test infrastructure. This filter excludes test-only helper functions from the "no test coverage" report. |

**Reading the result** — Each row is a production function that is reachable from production entrypoints but not from any test entrypoint. These are the true test coverage gaps at the call-graph level.

---

### Q18 — If `ConfigLoader.parse()` returns an error, which functions in the startup sequence won't be executed?

**Personas:** SSE · **Status:** answerable-today

Early errors in a startup sequence can cause later initialization functions to be skipped. Understanding which functions are short-circuited by an early failure helps an engineer reason about partial-initialization bugs and the cleanup required on error paths.

**The query**

```cgx
cgx query '
  MATCH (startup {name:"main"})-[:CALLS*]->(fn)
  WHERE NOT EXISTS {
    MATCH path = (startup)-[:CALLS*]->(fn)
    WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  }
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' ./
```

This finds functions that are only reachable from `main` via at least one exception-conditioned edge — meaning they are not on the "all-clear" startup path from the origin.

The complement — functions that are always executed regardless of the error — uses:

```cgx
cgx query '
  MATCH path = (startup {name:"main"})-[:CALLS*]->(fn)
  WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(startup {name:"main"})-[:CALLS*]->(fn)` | Find all functions reachable from the startup entrypoint. |
| `NOT EXISTS { MATCH path … NONE(r … exception/panic) }` | The function has no non-exception path from `main` — it is only reachable if the startup takes an error branch at some point. |

**Reading the result** — Functions in the result set are those that will not run if an error occurs early in startup. Compare this with the cleanup and resource-release functions to find initialization that is skipped on error, potentially leaving resources uninitialized or partially configured.

---

### Q19 — What is the set of all functions that could be executing when we crash at `panic_handler`?

**Personas:** SSE · **Status:** answerable-today

When a Rust `panic!` fires, the execution context may involve a deep call stack. To understand the crash, an engineer needs to know the full set of functions that are in the call chain at the time of the panic — everything that was executing and would be unwound. This is the inverse blast radius: all callers of `panic_handler`, transitively.

**The query**

```cgx
cgx callers panic_handler ./ --depth 20 --format json

cgx query '
  MATCH (caller)-[:CALLS*]->(ph {name:"panic_handler"})
  RETURN caller.name, caller.file, caller.line,
         caller.kind
  ORDER BY caller.file, caller.name
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(caller)-[:CALLS*]->(ph {name:"panic_handler"})` | Find every function in the graph that can reach `panic_handler` through any chain of call edges. |
| `--depth 20` | Set a generous depth limit to capture long call chains. Adjust based on the codebase's typical call depth. |

**Reading the result** — The result set is the complete set of functions that could be on the call stack when a panic fires. This is the "blast radius from the panic" — every function that could be interrupted. Sort by file to cluster related functions for review.

---

### Q20 — After I edit function `G`, show me any new call edges that were introduced and whether they reach any dangerous sinks.

**Personas:** ACA · **Status:** answerable-today

An AI coding agent that has just edited a function needs to verify its own work. After the edit, the agent queries the post-edit call graph to check whether any new edges were introduced that reach dangerous operations — a self-verification step before committing.

**The query**

```cgx
cgx diff --base HEAD~1 --head HEAD ./ --calls-to-sink-class sql
cgx diff --base HEAD~1 --head HEAD ./ --calls-to-sink-class shell

cgx query --at HEAD '
  MATCH (a)-[r:CALLS]->(b)
  WHERE r.introducing_commit = "HEAD"
    AND b.sink_class IN ["sql","shell","path","net-request","eval"]
  RETURN a.name, a.file, a.line,
         b.name, b.sink_class, b.file, b.line
  ORDER BY b.sink_class, a.file
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff --base HEAD~1 --head HEAD ./` | Compare the call graph at the prior commit against the current HEAD — shows what edges were added or removed by the most recent commit. |
| `--calls-to-sink-class sql` | Filter the diff to show only new edges that reach SQL-class sinks. |
| `r.introducing_commit = "HEAD"` | In the query form, select only edges whose introducing commit is the current HEAD — the edges added by the edit. |
| `b.sink_class IN [...]` | Check whether the new edge targets a dangerous sink class. |

**Reading the result** — New edges that reach dangerous sinks are the agent's primary concern. A non-empty result means the edit introduced a call path to a sensitive operation that did not previously exist. The agent should review whether that new path is intentional and whether it is guarded appropriately.

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
' ./
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

**Personas:** SSE · **Status:** answerable-today

Before removing a deprecated function, an engineer needs the full list of call sites to plan the migration. This is a direct blast radius query with a filter on the deprecation attribute.

**The query**

```cgx
cgx callers DeprecatedModule::old_function ./ --depth 1 --format json

cgx query '
  MATCH (caller)-[:CALLS]->(deprecated)
  WHERE deprecated.deprecated = true
  RETURN deprecated.name AS deprecated_fn,
         caller.name, caller.file, caller.line
  ORDER BY deprecated.name, caller.file
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(caller)-[:CALLS]->(deprecated)` | Find direct callers — depth 1 is sufficient here since we want the actual call sites, not the transitive blast radius. |
| `deprecated.deprecated = true` | Filter to functions marked as deprecated in the graph — this attribute is set when the function carries a `#[deprecated]` attribute (Rust), `@Deprecated` annotation (Java), or equivalent. |
| `ORDER BY deprecated.name` | Group results by deprecated function name, making the migration checklist easy to follow. |

**Reading the result** — Each row is a direct call site of a deprecated function. The result grouped by `deprecated.name` gives a migration checklist: each deprecated function and the files that must be updated.

---

### Q23 — Which tests become invalid if I rename `OrderService.submit()`?

**Personas:** SSE · **Status:** answerable-today

Renaming a function will break any test that directly calls it or indirectly depends on it through a call chain. This is a blast radius query scoped to test entrypoints.

**The query**

```cgx
cgx query '
  MATCH path = (test {kind:"entrypoint", entrypoint_class:"test"})-[:CALLS*]->(fn {name:"OrderService::submit"})
  RETURN test.name, test.file, test.line,
         length(path) AS hops
  ORDER BY hops, test.file
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(test {kind:"entrypoint", entrypoint_class:"test"})` | Start from test entrypoints only — functions declared as test roots. |
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
cgx callers authenticate ./ --depth 2 --format json > callers.json
cgx callees authenticate ./ --depth 3 --format json > callees.json

cgx query '
  MATCH (c)-[:CALLS*1..2]->(fn {name:"authenticate"})-[:CALLS*1..3]->(d)
  RETURN c.name AS caller, c.file AS caller_file,
         d.name AS callee, d.file AS callee_file
  ORDER BY c.file, d.file
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(c)-[:CALLS*1..2]->(fn {name:"authenticate"})` | Callers within 2 hops — the functions that call `authenticate()` and the functions that call those. The middleware must be compatible with all of these. |
| `(fn)-[:CALLS*1..3]->(d)` | Callees within 3 hops — what `authenticate()` calls. The middleware must preserve or correctly wrap all of this behavior. |

**Reading the result** — The caller set defines the API contract the middleware must honor; every caller must work the same way after the middleware is inserted. The callee set defines the behavior the middleware wraps; any side effect in that set is now the middleware's responsibility.

---

### Q25 — Which public API methods have their implementation entirely contained within a single module vs. spanning multiple modules?

**Personas:** SSE · **Status:** answerable-today

API methods whose implementations span multiple modules create cross-module coupling and are harder to refactor or extract. Methods fully contained in a single module are better candidates for extraction. This query partitions the public API by implementation scope.

**The query**

```cgx
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
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(api:method {visibility:"public"})` | Start from public methods — the externally visible API surface. |
| `EXISTS { MATCH (ep {kind:"entrypoint"})-[:CALLS*]->(api) }` | Confirm the method is reachable from a declared entrypoint — it is live public API, not dead code. |
| `OPTIONAL MATCH reach = (api)-[:CALLS*]->(callee)` | Transitively find all functions the API method calls. `OPTIONAL` ensures methods with no callees still appear. |
| `collect(distinct callee.file) + [api.file] AS all_files` | Collect every distinct file touched by the method and its full call tree, including the file where the API method itself is defined. |
| `size(all_files) <= 1` | Single-module: the entire implementation lives in one file. Multi-module: the implementation spans multiple files. |

**Reading the result** — Methods in the `single-module` group are the best candidates for extraction to a separate crate or microservice. Methods in the `multi-module` group have cross-module dependencies that would need to be resolved first. Sort by `module_count` descending to find the most entangled methods.
