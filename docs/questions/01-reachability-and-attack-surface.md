# Theme 1: Reachability and Attack Surface

This theme covers the 18 questions that ask whether a dangerous operation is reachable from an attacker-accessible entry into the system, and whether that reachability is blocked by a required check. The core capability is `cgx`'s ability to enumerate all call paths between two points in the graph, to filter those paths by edge condition (distinguishing happy-path calls from exception-path calls), and to assert that every path — not just some path — passes through a required check node. Readers who know what a function call graph is, but have not written graph queries before, will find each entry's breakdown table explains exactly what each fragment of the query is testing.

Questions Q1–Q13 cover the most common reachability patterns: shell-sink reach, database bypass, CVE reachability, exception-only paths, taint to sensitive sinks, and unauthorized access. Questions Q87–Q88 and Q92–Q93, Q100 add the stricter ∀-path guarantee — "does every path pass through the check?" — which is the question authorization reviews actually require.

---

### Q1 — Which HTTP handler entrypoints can reach `exec()`, `system()`, or shell subprocess calls?

**Personas:** PSE · **Status:** answerable-today

A security engineer wants to know whether any of the application's HTTP entry points can ever call a shell command — directly or through a chain of function calls. Even a single reachable path is a signal worth investigating, because it may be exploitable if user input reaches that path.

**The query**

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink)
  WHERE sink.name IN ["exec", "system", "popen"]
    AND sink.kind IN ["function","method"]
  RETURN ep.name, ep.file, ep.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY ep.file, ep.line
' --repo ./
```

To emit SARIF for CI integration, add `--format sarif`.

```cgx
cgx query '...' --repo ./ --format sarif
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(ep {kind:"entrypoint"})` | Find every node that `cgx` has recorded as an entrypoint. Filter to HTTP handlers by adding a name or module filter, or rely on the `entrypoint` kind covering all declared entry points. |
| `-[:CALLS*]->` | Follow call edges transitively — any chain length from one hop to many. |
| `(sink)` | Arrive at any node in the graph. |
| `WHERE sink.name IN ["exec", "system", "popen"]` | Keep only paths that end at one of the named shell-execution sinks. |
| `RETURN … length(path) AS hops` | Report the source handler, the sink, and how many call hops separate them — a shorter hop count means the path is simpler to exploit. |

**Reading the result** — Each row is a confirmed reachable path from an HTTP handler to a shell sink. The `hops` column tells you how direct the path is. Because `cgx` returns graph-reachable paths (not path-feasibility), some paths may be guarded by conditions not visible to static analysis; use edge condition labels and `--confidence certain` to triage.

---

### Q2 — Can any internet-facing endpoint reach our database query builders without going through the input validation layer?

**Personas:** PSE · **Status:** answerable-today

This question asks for the negative guarantee: are there paths that bypass the validation layer entirely? It is not enough to know that some paths pass through validation — the concern is whether any path skips it.

**The query**

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink)
  WHERE sink.name IN ["query", "execute", "prepare", "raw_query"]
    AND NONE(n IN nodes(path) WHERE n.name IN ["validate_input", "sanitize", "escape_sql"])
  RETURN ep.name, ep.file, ep.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY ep.file, ep.line
' --repo ./
```

Note: `entrypoint_class` and `sink_class` are not supported node properties at v0.3 (plan error, exit 2). Identify database sinks by name pattern or module prefix instead of `sink_class`.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(ep {kind:"entrypoint"})` | Start at any entrypoint. At v0.3, `entrypoint_class` is not a supported node property; filter HTTP-specific handlers by name or module path if needed. |
| `-[:CALLS*]->` | Follow call edges transitively. |
| `sink.name IN [...]` | End at known database query functions by name. At v0.3, `sink_class` is not a supported node property; enumerate sink names explicitly. |
| `WHERE NONE(n IN nodes(path) WHERE n.name IN [...])` | Negative path constraint (Q-14): discard any path that passes through one of the named validation functions. Only paths that bypass them entirely remain. |

**Reading the result** — A non-empty result set is evidence of a SQL injection risk surface: paths from user-facing endpoints to the database layer that never visit a validation function. Zero results means every such path passes through at least one of the listed names — but note this is a name-match check; it does not verify that the validation function actually validates correctly.

---

### Q3 — Is the CVE'd function `libfoo::deserialize()` actually reachable from any of our entrypoints in production code?

**Personas:** PSE · **Status:** needs-schema-room-feature (Q-25 dependency edges — GM-14.3)

A vulnerability scanner reports that a dependency contains a known-vulnerable function. The real question is whether that function is actually called by this application. Most CVE advisories report "package reachable," not "this specific function is in the call path." `cgx` answers at function granularity.

**The query**

```cgx
-- illustrative: requires Q-25 (schema-room) for dependency-edge package/version attributes
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::deserialize"})
  RETURN ep.name, ep.file, ep.line,
         length(path) AS hops,
         [e IN relationships(path) | e.condition] AS conditions,
         MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
  ORDER BY hops, ep.name
  LIMIT 20
' --repo ./
```

This query also has a Layer 1 form (specified in docs/05 Q-25 Worked Example 5):

`cgx reaches 'libfoo::deserialize' --confidence probable --repo ./`

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(ep {kind:"entrypoint"})` | Start from any declared entrypoint — HTTP, CLI, event queue, test. |
| `-[:CALLS*]->` | Follow call edges transitively across any number of hops, including across crate boundaries once dependency edges are populated. |
| `(vuln {name:"libfoo::deserialize"})` | End at the specific vulnerable function named in the advisory. |
| `[e IN relationships(path) \| e.condition] AS conditions` | Collect the edge condition label on every hop — if the vulnerable function is only reached through exception paths, exploitation may be harder. |
| `MIN([e IN relationships(path) \| e.confidence]) AS weakest_confidence` | The weakest-link confidence on the path. A path where the weakest edge is `possible` may be a false positive from over-approximate dispatch. |

**Reading the result** — Rows are confirmed reachable paths at the named confidence level. The `conditions` array tells you whether the path goes through exception-only hops. The `weakest_confidence` field helps triage: `certain` = definitely reachable; `possible` = may be an over-approximation artifact. See docs/05 Q-25 for the full CVE triage workflow.

---

### Q4 — Which paths to the crypto key derivation function come ONLY through exception handlers?

**Personas:** PSE · **Status:** answerable-today

Calls to a key derivation function that are only reachable through error-handling code represent a different risk profile than calls on the normal execution path — they may indicate dead code, a misplaced crypto operation, or a secret being generated in an error recovery context.

**The query**

The Layer 1 `cgx paths` subcommand does not have an `--only-edge-condition` flag (exit 2 if attempted). Use the CQL query form to filter on edge conditions:

```cgx
cgx query '
  MATCH path = (src)-[:CALLS*]->(kdf {name:"crypto::derive_key"})
  WHERE ALL(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN src.name, src.file, src.line,
         length(path) AS hops
' --repo ./
```

`ALL(r IN relationships(path) WHERE r.condition IN [...])` keeps only paths where every edge is in the exceptional class — the "ONLY through exception handlers" case. The looser form below returns paths with *at least one* exception edge:

```cgx
cgx query '
  MATCH path = (src {kind:"entrypoint"})-[:CALLS*]->(kdf {name:"crypto::derive_key"})
  WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
    AND ALL(r2 IN relationships(path) WHERE r2.condition IN ["exception","panic"])
  RETURN src.name, src.file, src.line, length(path) AS hops
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ALL(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Every edge on the path is in the exceptional class — the KDF is only ever reached via error-handling code. Use `ALL` instead of `NONE(... NOT IN ...)`, which is a parse error at v0.3. |
| `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Path-relative transience check (Q-12): the path must traverse at least one exceptional-class edge. |

**Reading the result** — Results show call chains that reach the KDF exclusively through exception handlers. These warrant review: is this intentional fallback key generation, or is a cryptographic primitive being invoked only when something has already gone wrong?

---

### Q5 — What is the minimal set of entrypoints from which a user-supplied value could reach the template rendering engine?

**Personas:** PSE · **Status:** needs-schema-room-feature (DF-11 typed taint labels and class-matched sanitization — core-extension Phase 3)

This question combines taint propagation with entrypoint enumeration: find every entry point that is the origin of a user-controlled value that could reach the template renderer. A single tainted rendering call can be a server-side template injection.

**The query**

```cgx
-- illustrative: requires DF-11 typed taint labels (core-extension, Phase 3)
-- source_class, sanitizer_class are unsupported node properties at v0.3 (plan error, exit 2)
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink {name:"template::render"})
  WHERE src.source_class IN ["network","cli","user-input"]
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "template")
  RETURN src.name, src.file, src.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY src.file, src.line
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src)-[:DATA_FLOW*]->` | Follow data-flow (derives-from) edges rather than call edges — tracing the value itself, not just the call chain. |
| `src.source_class IN ["network","cli","user-input"]` | The value must originate from a user-controlled input class (DF-12 source classes). Deferred: `source_class` is not a supported node property at v0.3 and causes a plan error. |
| `(sink {name:"template::render"})` | The value must reach the template rendering function. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "template")` | No sanitizer of class `template` appears on the data-flow path. Deferred: `sanitizer_class` is not a supported node property at v0.3 and causes a plan error. |

**Reading the result** — Each row names the source symbol (the point where user data enters the application) and the rendering call it can reach. The set of distinct `src.file` values identifies which files need review for template injection. For the Layer 1 equivalent, see docs/05 Q-23.

---

### Q6 — Do any paths from public API endpoints reach internal admin functions that should only be called from the scheduler?

**Personas:** PSE · **Status:** answerable-today

This is an authorization topology question: the admin functions have no authentication guard on their call-graph path from public entry points, because they were designed to only be called by the scheduler. Any path that bypasses that design assumption is a privilege escalation.

**The query**

Note: `--from-class`, `--avoiding` are not flags on `cgx paths` (exit 2). `entrypoint_class` is not a supported node property at v0.3 (plan error). Use the CQL query form:

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(admin)
  WHERE admin.name IN ["admin::create_user","admin::delete_user",
                       "admin::reset_password","admin::grant_role"]
    AND NONE(n IN nodes(path) WHERE n.name IN ["scheduler::dispatch",
                                                "scheduler::run_job",
                                                "scheduler::enqueue"])
    AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN ep.name, ep.file, ep.line,
         admin.name, admin.file, admin.line,
         length(path) AS hops
  ORDER BY ep.file, ep.line
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep {kind:"entrypoint"}` | Start at any entrypoint. At v0.3, `entrypoint_class` is not a supported node property; filter to HTTP-specific handlers by name or module prefix if needed. |
| `admin.name IN ["admin::create_user", ...]` | End at named functions in the `admin` module. At v0.3, `STARTS WITH` is not a supported CQL operator (parse error); enumerate admin function names explicitly. |
| `NONE(n IN nodes(path) WHERE n.name IN ["scheduler::..."])` | The scheduler is not on the path — the call reaches admin code without going through the intended gating code. |
| Second `NONE` | Restrict to non-exception paths, focusing on the happy-path access pattern first. |

**Reading the result** — Any non-empty result is a path from a public endpoint to internal admin code that bypasses the scheduler. These are privilege escalation candidates.

---

### Q7 — Which dependencies' functions are transitively reachable from our highest-traffic endpoints?

**Personas:** PSE · **Status:** needs-schema-room-feature (Q-25 dependency edges — GM-14.3)

This question maps the attack surface of the dependency tree: if a vulnerability is discovered in a dependency, which of our endpoints are exposed? The answer also shows the blast radius if a dependency is compromised.

**The query**

```cgx
-- illustrative: requires Q-25 (schema-room) for dependency-edge package/version attributes
-- entrypoint_class, package, version are not supported node properties at v0.3 (plan error, exit 2)
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(dep_fn)
  WHERE dep_fn.package IS NOT NULL
    AND dep_fn.package <> "this-crate"
  RETURN ep.name,
         dep_fn.package, dep_fn.version,
         dep_fn.name, dep_fn.file,
         length(path) AS hops
  ORDER BY dep_fn.package, ep.name
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep {kind:"entrypoint"}` | Start from any entrypoint. At v0.3, `entrypoint_class` is not a supported node property; also, `package` and `version` node properties require Q-25 schema-room dependency edges. |
| `-[:CALLS*]->` | Follow the full transitive call graph. |
| `dep_fn.package IS NOT NULL` | The target node has a package attribute — it lives in a dependency, not in the application code. |
| `dep_fn.package <> "this-crate"` | Exclude nodes from the application's own crate. |
| `RETURN … dep_fn.package, dep_fn.version` | Report the dependency package name and version alongside the function, so results can be correlated against SBOM advisories. |

**Reading the result** — The result set maps each endpoint to the dependency functions it can reach. Cross-reference against `cargo audit` or SBOM scan output to identify which endpoints are exposed to which advisories.

---

### Q8 — Are there paths from unauthenticated entrypoints to functions that read environment variables?

**Personas:** PSE · **Status:** answerable-today

Environment variables often hold secrets: database passwords, API keys, tokens. A path from an unauthenticated endpoint to an `env::var` call is a potential credential exposure — the caller might return the value to the user, or log it.

**The query**

Note: `entrypoint_class` and `authenticated` are not supported node properties at v0.3 (plan error, exit 2). Identify entrypoints by `kind:"entrypoint"` and, if needed, by name prefix. The query is answerable without the `authenticated` filter — review all entrypoints and triage by name:

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(env_read)
  WHERE env_read.name IN ["std::env::var",
                          "std::env::var_os",
                          "env::var",
                          "os::getenv"]
    AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN ep.name, ep.file, ep.line,
         env_read.name, env_read.file, env_read.line,
         length(path) AS hops
  ORDER BY ep.file, ep.line
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep {kind:"entrypoint"}` | Any declared entrypoint. At v0.3, `authenticated` and `entrypoint_class` are not supported node properties; filter by entrypoint name or module if needed. |
| `env_read.name IN [...]` | The target is any standard library function that reads environment variables. |
| `NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Restrict to non-exception paths — the happy path from the entrypoint. |

**Reading the result** — Each row names an entrypoint and the env-read call it can reach. The next question to ask is: does the return value from that env read flow back to the response? Use the `cgx flows-to` subcommand on the env-read function's return value node for that follow-up.

---

### Q9 — List all functions reachable from public HTTP handlers that perform file I/O without going through path sanitization.

**Personas:** ASA · **Status:** answerable-today

An automated security agent scanning for path traversal vulnerabilities wants to find every file I/O operation that an HTTP handler can reach, filtered to those that have no sanitization on the path. These are the candidates for directory traversal or arbitrary file read vulnerabilities.

**The query**

Note: `entrypoint_class`, `sink_class`, and `sanitizer_class` are not supported node properties at v0.3 (plan error, exit 2). Enumerate file I/O sinks by name and identify sanitizers by name. Status is correct as answerable-today when queries use supported properties:

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(file_op)
  WHERE file_op.name IN ["std::fs::File::open", "std::fs::read_to_string",
                         "std::fs::File::create", "std::fs::remove_file"]
    AND NONE(n IN nodes(path) WHERE n.name IN ["canonicalize", "sanitize_path",
                                               "validate_path"])
    AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN ep.name, ep.file, ep.line,
         file_op.name, file_op.file, file_op.line,
         length(path) AS hops
  ORDER BY ep.file, ep.line
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `file_op.name IN [...]` | The target is a file I/O function identified by name. At v0.3, `sink_class` is not a supported node property; enumerate specific function names instead. |
| `NONE(n IN nodes(path) WHERE n.name IN [...])` | No named path-sanitizing function appears on the path. At v0.3, `sanitizer_class` is not a supported node property; enumerate sanitizer function names explicitly. |
| Second `NONE` | Restrict to non-exception paths for the initial scan. |

**Reading the result** — Each row is a potential path traversal surface. The file operation name and location give the analyst the exact call site. Run `cgx flows-from` on the value node feeding that call site to determine whether user input flows into it.

---

### Q10 — For each CVE in our SBOM, determine reachability from each entrypoint class (HTTP, CLI, event-queue) and return a structured JSON report.

**Personas:** ASA · **Status:** needs-schema-room-feature (Q-25 dependency edges — GM-14.3)

An automated security agent processing a SBOM-derived CVE list needs function-level reachability data for each advisory, broken down by entrypoint class, in machine-readable form. This drives automated triage: advisories where no entrypoint can reach the vulnerable function can be deprioritized.

**The query**

Note: `entrypoint_class` is not a supported node property at v0.3 (plan error, exit 2). Query parameters (`$name`) are also not supported at v0.3 (plan error, exit 2); substitute the literal function name. The `--param` flag does not exist:

```cgx
-- illustrative: requires Q-25 (schema-room) for dependency-edge package/version attributes
-- entrypoint_class unsupported at v0.3; substitute literal vulnerable_fn name
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln)
  WHERE vuln.name = "libfoo::parse_header"
  RETURN ep.name, ep.file, ep.line,
         vuln.name, vuln.file,
         length(path) AS hops,
         [e IN relationships(path) | e.condition] AS conditions,
         MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
  ORDER BY hops
' --repo ./ --format json
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `vuln.name = "libfoo::parse_header"` | Literal function name. At v0.3, query parameters (`$vulnerable_fn`) are not supported; the calling script must interpolate the value directly into the query string. |
| `ep {kind:"entrypoint"}` | Any declared entrypoint. At v0.3, `entrypoint_class` is not a supported node property; filter by entrypoint name/module if needed. |
| `[e IN relationships(path) \| e.condition] AS conditions` | The list of edge condition labels on the path — useful for determining whether the vulnerable function is only reached via error paths. |
| `MIN([…\| e.confidence]) AS weakest_confidence` | The weakest edge confidence on the path; `possible` may be a false positive from dynamic dispatch over-approximation. |
| `--format json` | Return structured JSON; the agent processes each row programmatically. |

**Reading the result** — The agent receives one row per reachable path. An advisory with zero rows at `--confidence probable` can be marked "not reachable at functional confidence." See docs/05 Q-25 Worked Example 5 for the full triage query.

---

### Q11 — If I add a new public route handler, what existing call paths does it share with authenticated handlers?

**Personas:** SSE · **Status:** answerable-today

Before adding a new public (unauthenticated) route, an engineer wants to understand which existing call chains the new handler would share with authenticated handlers. Shared call paths mean shared state, shared sinks, and potentially shared attack surface.

**The query**

Note: `authenticated` is not a supported node property at v0.3 (plan error, exit 2). `collect(distinct ...)` is also a parse error at v0.3; use `collect(...)` without `distinct`:

```cgx
cgx query '
  MATCH (new_handler {name:"new_route::handler"})-[:CALLS*]->(shared)
  MATCH (auth_handler {kind:"entrypoint"})-[:CALLS*]->(shared)
  WHERE new_handler <> auth_handler
  RETURN shared.name, shared.file, shared.line,
         collect(auth_handler.name) AS auth_handlers_that_also_reach_it
  ORDER BY shared.name
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| First `MATCH` | Find all functions reachable from the new route handler. |
| Second `MATCH` | Find all functions reachable from any entrypoint. At v0.3, `authenticated` is not a supported node property; filter authenticated handlers by name prefix if needed. |
| `(shared)` | The node appears in both traversals — it is reachable from both the new public handler and at least one other entrypoint. |
| `collect(auth_handler.name)` | Group the entrypoints that also reach the shared node. At v0.3, `collect(distinct ...)` is a parse error; use `collect(...)` without the `distinct` keyword. |

**Reading the result** — Each row is a shared function — a function the new public handler would call that is also called by authenticated handlers. Pay particular attention to shared functions that access sensitive resources or global state.

---

### Q12 — Which sinks (file write, network send, process spawn) are reachable only during exception handling paths?

**Personas:** PSE · **Status:** answerable-today

Sinks that are only reachable during exception handling represent a specific risk: they may expose data in error conditions, or they may execute dangerous operations (like network calls) in contexts where the application is in a partially-failed state.

Note: `sink_class` is not a supported node property at v0.3 (plan error, exit 2). `NOT EXISTS { MATCH ... }` subquery syntax is also not supported at v0.3 (parse error, exit 2). Enumerate sink names explicitly and split the two-path condition into separate queries:

```cgx
-- Step 1: find paths with at least one exception-conditioned edge
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink)
  WHERE sink.name IN ["std::fs::File::open", "std::fs::write",
                      "std::net::TcpStream::connect",
                      "std::process::Command::spawn"]
    AND ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN ep.name, ep.file, ep.line,
         sink.name, sink.file, sink.line
  ORDER BY sink.file
' --repo ./
```

Cross-reference step 1 results against a separate query (same sink list, `NONE` on exception edges) to identify sinks reachable exclusively via exception paths. `NOT EXISTS { MATCH ... }` subqueries are not supported at v0.3.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `sink.name IN [...]` | Target dangerous file-I/O, network, and process-spawn functions by name. At v0.3, `sink_class` is not a supported node property; enumerate names explicitly. |
| `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | This path contains at least one exception-conditioned edge. |
| Two-query cross-reference | At v0.3, `NOT EXISTS { MATCH ... }` subqueries are a parse error. Identify exclusively-exception-path sinks by diffing the exception-path result set against a non-exception-path result set in the calling script. |

**Reading the result** — Each row is a sink that is accessible only when the application is in an error state. File writes and network sends from error handlers are common sources of sensitive data leakage.

---

### Q13 — Show me all paths from user-controlled input functions to any function that generates or validates JWT tokens.

**Personas:** PSE · **Status:** needs-schema-room-feature (DF-11 typed taint labels — core-extension Phase 3)

User-controlled input reaching JWT generation or validation is a high-severity finding: it may allow signature bypass, algorithm confusion, or forged tokens. This is a taint query tracing from network-class sources to JWT-related sinks.

**The query**

```cgx
-- illustrative: requires DF-11 typed taint labels (core-extension, Phase 3)
-- source_class, sanitizer_class, transformation_kind are unsupported at v0.3 (plan error, exit 2)
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(jwt_fn)
  WHERE src.source_class IN ["network","cli","user-input"]
    AND jwt_fn.name IN ["jwt::encode", "jwt::decode",
                        "jsonwebtoken::encode", "jsonwebtoken::decode",
                        "jwt::verify", "jwt::sign"]
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "jwt")
  RETURN src.name, src.file, src.line,
         jwt_fn.name, jwt_fn.file, jwt_fn.line,
         [e IN relationships(path) | e.transformation_kind] AS transforms,
         length(path) AS hops
  ORDER BY src.file, src.line
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src)-[:DATA_FLOW*]->(jwt_fn)` | Follow data-flow edges (not just call edges) — tracing the value from its origin, through any transformations, to the JWT function. |
| `src.source_class IN ["network","cli","user-input"]` | The value must originate from a user-controlled source class (DF-12). Deferred: `source_class` is not a supported node property at v0.3 and causes a plan error. |
| `jwt_fn.name IN [...]` | The target is a JWT generation or validation function. Extend this list to match the JWT library used in the project. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "jwt")` | No JWT-class sanitizer on the data-flow path. Deferred: `sanitizer_class` is not a supported node property at v0.3 and causes a plan error. |
| `[e IN relationships(path) \| e.transformation_kind] AS transforms` | The transformation sequence. Deferred: `transformation_kind` is not a supported edge property at v0.3 and causes a plan error. |

**Reading the result** — Each row traces user-controlled data to a JWT operation. A path that reaches `jwt::encode` with the user value flowing to the `secret` or `algorithm` argument is the highest severity finding.

---

### Q87 — Does every path from a public handler to a protected resource traverse the authorization check — or can any path bypass it?

**Personas:** PSE · **Status:** needs-schema-room-feature (Q-20 must-pass-through — core-extension Phase 3)

This is the canonical authorization bypass question. The key word is "every": it is not enough to know that some paths pass through the auth check — a single bypass path is a vulnerability. This requires the ∀-path (must-analysis) predicate from Q-20, specified in docs/05 Q-20.

**The query**

Note: `--avoiding` is not a flag on `cgx paths` at v0.3 (exit 2 if attempted). The bypass-detection question is answerable today using the `NONE`-based CQL form. The full ∀-path `MATCH ALL ... MUST PASS THROUGH` form is deferred (plan error, exit 2):

```cgx
cgx query '
  MATCH path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
  WHERE NONE(n IN nodes(path) WHERE n.name = "require_admin")
    AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN h.name, h.file, h.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY h.file, h.line
' --repo ./ --format sarif > authz-bypass.sarif

cgx query '
  MATCH path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
  WHERE NONE(n IN nodes(path) WHERE n.name = "require_admin")
    AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN h.name, h.file, h.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY h.file, h.line
' --repo ./ --assert-empty
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `NONE(n IN nodes(path) WHERE n.name = "require_admin")` | Negative path constraint (Q-14): keep only paths where the authorization check is absent. Any result is a bypass. |
| Second `NONE` on `relationships` | Restrict to non-exception paths first; exception-path bypasses should be evaluated separately. |
| `--assert-empty` | CI gate: exit with code 1 if any bypass path is found. Zero results means the ∀ guarantee holds. |

**Reading the result** — Zero results means every non-exception path from a handler to `db::write` passes through `require_admin` — the ∀-path authorization guarantee holds on the call graph. Non-empty results are bypass candidates. Note: this is a name-match check; Q-31 adds metadata-guard-aware authorization checks for frameworks that use annotations like `@PreAuthorize`.

---

### Q88 — Is the authorization check on the same execution path as the resource access it guards, or only on a sibling branch?

**Personas:** PSE · **Status:** needs-schema-room-feature (Q-20 must-pass-through — core-extension Phase 3)

A common subtle bug: the authorization check and the resource access are both present in the function, but the check is on one branch of a conditional and the access is on a different branch. The check appears in code review but does not actually guard the access on every path.

**The query**

The `MATCH ALL ... MUST PASS THROUGH` form is deferred at v0.3 (plan error, exit 2). Use the `NONE`-based form, which is answerable today and has identical semantics — it returns paths where the check is absent:

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(resource {name:"protected_resource::access"})
  WHERE NONE(n IN nodes(path) WHERE n.name = "authz::require_permission")
  RETURN path
' --repo ./
-- Zero results = every path to the resource passes through the check.
-- Non-empty results = at least one path reaches the resource without the check.
```

The following form is illustrative only — it causes a plan error at v0.3:

```cgx
-- deferred: MATCH ALL ... MUST PASS THROUGH → plan error, exit 2 at v0.3
cgx query '
  MATCH ALL path = (ep {kind:"entrypoint"})-[:CALLS*]->(resource {name:"protected_resource::access"})
  MUST PASS THROUGH (check {name:"authz::require_permission"})
  RETURN path
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `NONE(n IN nodes(path) WHERE n.name = "authz::require_permission")` | Return paths where the authorization check is missing — a sibling-branch bypass. This is the correct v0.3 form for the ∀-path check. |
| `MATCH ALL path … MUST PASS THROUGH` | The explicit ∀-path form (Q-20). Deferred at v0.3: causes a plan error ("not supported in this release"). Use the `NONE`-based form above. |

**Reading the result** — The `MATCH ALL … MUST PASS THROUGH` form returns paths that violate the must-pass-through property. The `NONE` form returns the same paths. Either form: non-empty means a sibling-branch bug exists — the check and the access are on different conditional branches of the call graph.

---

### Q92 — Does tainted data from any network source reach a URL-fetch, file-open, or redirect sink without a canonicalization sanitizer on every path?

**Personas:** PSE · **Status:** needs-schema-room-feature (Q-23 typed taint + Q-20 must-pass-through — core-extension Phase 3)

This question targets SSRF (server-side request forgery) and open redirect vulnerabilities. The critical requirement is "on every path" — a sanitizer present on only some paths does not prevent exploitation. This is a combination of typed taint (Q-23) and ∀-path guarantees (Q-20).

**The query**

```cgx
-- illustrative: requires Q-23 typed taint (core-extension, Phase 3)
-- source_class, sink_class, sanitizer_class are unsupported at v0.3 (plan error, exit 2)
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink)
  WHERE src.source_class = "network"
    AND sink.sink_class IN ["path","redirect-url","net-request"]
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class IN ["path","redirect-url","net-request"])
  RETURN src.name, src.file, src.line,
         sink.name, sink.sink_class, sink.file, sink.line,
         length(path) AS hops
  ORDER BY sink.sink_class, src.file
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class = "network"` | The taint origin is a network-read source (DF-12 source class). Deferred: `source_class` is not a supported node property at v0.3 and causes a plan error. |
| `sink.sink_class IN ["path","redirect-url","net-request"]` | The sink is one of the three dangerous classes for SSRF and path traversal. Deferred: `sink_class` is not a supported node property at v0.3 and causes a plan error. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class IN [...])` | No class-matched sanitizer on the data-flow path. Deferred: `sanitizer_class` is not a supported node property at v0.3 and causes a plan error. |

**Reading the result** — Each row is a data-flow path from a network input to a dangerous sink without a matching sanitizer. The typed-taint properties (`source_class`, `sink_class`, `sanitizer_class`) needed by this query are deferred to Phase 3. See docs/05 Q-23 for the typed taint query reference.

---

### Q93 — What types are constructed from untrusted deserialized bytes, and what do their constructors and `Drop` implementations reach?

**Personas:** PSE · **Status:** needs-schema-room-feature (Q-23 typed taint + GM-14 code-trust boundaries — schema-room features Q-23 core-extension Phase 3; GM-14 dependency edges schema-room)

Deserialization gadget chains are a serious attack vector: if an attacker controls the bytes fed to a deserializer, they control which objects are constructed and potentially which destructor code runs. This question maps the reachability footprint of types built from deserialized input.

**The query**

```cgx
-- illustrative: requires Q-23 typed taint (core-extension, Phase 3) and
--               GM-14 dependency edges (schema-room) for cross-crate types
-- source_class, sink_class are unsupported node properties at v0.3 (plan error, exit 2)
cgx query '
  MATCH path = (src {source_class:"deserialization"})-[:DATA_FLOW*]->(constructor)
  WHERE constructor.kind IN ["method","function"]
    AND constructor.name IN ["MyType::new","AnotherType::new"]
  WITH src, constructor
  MATCH reach_path = (constructor)-[:CALLS*]->(dangerous)
  WHERE dangerous.sink_class IN ["shell","path","net-request","sql"]
  RETURN src.name, src.file,
         constructor.name, constructor.file, constructor.line,
         dangerous.name, dangerous.sink_class,
         length(reach_path) AS hops_from_constructor
  ORDER BY dangerous.sink_class, constructor.name
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src {source_class:"deserialization"}` | The taint origin is a deserialization source (DF-12 source class). Deferred: `source_class` is not a supported node property at v0.3 and causes a plan error. |
| `constructor.name IN ["MyType::new", ...]` | The data reaches a constructor — an object is being built from the deserialized bytes. At v0.3, `ENDS WITH` is not a supported CQL operator (parse error); enumerate constructor names explicitly or use `cgx search "::new"` to discover them first. |
| Second `MATCH` | From the constructor, follow call edges to find what dangerous operations the constructed object's methods can reach. |
| `dangerous.sink_class IN [...]` | The downstream dangerous sinks — shell commands, file operations, network requests, SQL. Deferred: `sink_class` is not a supported node property at v0.3 and causes a plan error. |

**Reading the result** — Each row names a constructor that can be invoked via deserialized data, and the dangerous sink that constructor's implementation (or the methods of the type it creates) can reach. The `hops_from_constructor` field shows how many call hops separate the constructor from the sink.

---

### Q100 — Do deserialized object fields flow to model or database writes without an allow-list check on every path?

**Personas:** PSE · **Status:** needs-schema-room-feature (Q-23 typed taint + Q-20 must-pass-through — core-extension Phase 3)

Mass assignment vulnerabilities occur when deserialized fields are written directly to a model or database without an explicit allow-list validation. This is a ∀-path taint question: the allow-list check must appear on every data-flow path from deserialized input to the database write, not just on some paths.

**The query**

```cgx
-- illustrative: requires Q-23 typed taint (core-extension, Phase 3)
-- source_class, sink_class, sanitizer_class, transformation_kind unsupported at v0.3 (plan error, exit 2)
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"sql"})
  WHERE src.source_class = "deserialization"
    AND NONE(n IN nodes(path)
             WHERE n.sanitizer_class = "sql"
                OR n.name IN ["allow_list_check",
                               "permit_fields",
                               "validate_fields"])
  RETURN src.name, src.file, src.line,
         sink.name, sink.file, sink.line,
         [e IN relationships(path) | e.transformation_kind] AS transforms,
         length(path) AS hops
  ORDER BY src.file, src.line
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class = "deserialization"` | The data originates from a deserialization source (DF-12 source class). Deferred: `source_class` is not a supported node property at v0.3 and causes a plan error. |
| `sink {sink_class:"sql"}` | The data reaches a SQL query builder or ORM write call. Deferred: `sink_class` is not a supported node property at v0.3 and causes a plan error. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql" OR n.name IN [...])` | No SQL-class sanitizer and no named allow-list function appears on the data-flow path. Deferred: `sanitizer_class` is not a supported node property at v0.3 and causes a plan error; the `n.name IN [...]` portion is supported and can be used standalone. |
| `[e … e.transformation_kind] AS transforms` | The transformation sequence. Deferred: `transformation_kind` is not a supported edge property at v0.3 and causes a plan error. |

**Reading the result** — Each row is a mass-assignment path from deserialized data to a database write that bypasses all allow-list validation. The typed-taint properties needed by the full query are deferred to Phase 3; the `n.name IN [...]` allow-list check is usable today once the deferred properties are removed.
