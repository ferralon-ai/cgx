# Theme 5: Dead and Unused Code

Dead code is source that exists in the repository but can never be reached during execution. It is a security risk beyond a tidiness issue: dormant functions accumulate technical debt, are rarely patched when vulnerabilities are found in their dependencies, and can be reactivated accidentally — the Knight Capital $440M incident (2012) is the canonical case. `cgx` answers dead-code questions at a finer grain than the Rust compiler's `dead_code` lint, which only reports items that are syntactically private. The queries here scope reachability to declared entrypoints, so a public method that no entrypoint ever reaches is correctly reported as unreachable.

---

### Q46 — Which public methods on `UserService` are never called from any entrypoint in our own codebase?

**Personas:** SSE · **Status:** answerable-today

A public method that no entrypoint ever calls is dead to the application even if it is technically exported. This question finds those methods on a specific type so a maintainer can decide whether to remove them or verify they are called by external consumers not visible in this repository.

**The query**

```cgx
cgx unused ./ --kind method --type UserService
```

The Layer-2 form also exists:

```cgx
cgx query '
  MATCH (m:method)-[:MEMBER_OF]->(t {name:"UserService"})
  WHERE NOT (m)<-[:CALLS]-()
  RETURN m.name, m.file, m.line
  ORDER BY m.name
' ./
```

Specified in docs/05 Canonical Example 3.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--kind method` | Restrict the unused search to methods (not standalone functions or fields). |
| `--type UserService` | Scope to methods that belong to the type `UserService`. |
| `NOT (m)<-[:CALLS]-()` | In the query form, exclude any method that has at least one incoming call edge from anywhere in the graph. |

**Reading the result** — Each row is a method on `UserService` with no caller anywhere in the indexed codebase. Because the unreachability check is relative to the full indexed graph (not just declared entrypoints by default), add `--entrypoint main` to the subcommand if you want to restrict to "unreachable from a specific root." Results are `certain` confidence when no call edge exists; `probable` when dynamic dispatch is possible.

---

### Q47 — Which authentication-related functions exist in the codebase but are never reachable from any live entrypoint?

**Personas:** PSE · **Status:** answerable-today

Authentication functions that are not reachable from any declared entrypoint are dormant security code. They may have been superseded by a newer auth path, may contain older vulnerabilities, and — if reactivated — could bypass current security controls.

**The query**

```cgx
cgx unused ./ --kind fn --path-filter '**/auth*'
```

Or with a name-pattern filter in the query language:

```cgx
cgx query '
  MATCH (fn {kind:"function"})
  WHERE (fn.name CONTAINS "auth" OR fn.name CONTAINS "login"
         OR fn.name CONTAINS "authenticate" OR fn.name CONTAINS "verify_token")
    AND NOT (fn)<-[:CALLS]-({kind:"entrypoint"})
    AND NOT ()-[:CALLS*]->(fn)<-[:CALLS]-({kind:"entrypoint"})
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.name CONTAINS "auth"` (etc.) | Match functions whose qualified name includes common authentication terms. Adjust the list to your naming conventions. |
| `NOT (fn)<-[:CALLS]-({kind:"entrypoint"})` | The function is not directly called by an entrypoint. |
| `NOT ()-[:CALLS*]->(fn)<-[:CALLS]-({kind:"entrypoint"})` | The function is not transitively reachable from any entrypoint, regardless of depth. `CALLS*` means zero-or-more hops. |

**Reading the result** — Each returned function is authentication-related by name but unreachable from any declared entrypoint. Before deleting, verify the function is not called via dynamic dispatch (`possible`-confidence edges) and is not part of a public library API consumed by external callers.

---

### Q48 — Which feature-flag branches are permanently dead given that flag `LEGACY_AUTH` is always false?

**Personas:** SSE · **Status:** needs-schema-room-feature (GM-19 build-configuration variance)

When a feature flag is statically known to be always false, all code guarded exclusively by that flag is unreachable. This question finds those dead branches before they accumulate over time.

**The query**

```cgx
-- illustrative: requires GM-19 (schema-room)
cgx query '
  MATCH (fn)
  WHERE fn.cfg_condition = "LEGACY_AUTH"
    AND fn.cfg_value = false
  RETURN fn.name, fn.file, fn.line,
         fn.cfg_condition AS dead_flag
  ORDER BY fn.file, fn.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.cfg_condition = "LEGACY_AUTH"` | The node is guarded by the named compile-time flag (GM-19 `cfg-condition` attribute). |
| `fn.cfg_value = false` | The flag evaluates to false in the default configuration, making this branch permanently dead. |

**Reading the result** — Returned nodes are only compiled in when `LEGACY_AUTH` is true. Because the flag is always false in the production build, these are effectively dead. GM-19 must be populated before this query runs.

---

### Q49 — Which database migration functions have already been applied and are now unreachable dead code?

**Personas:** SSE · **Status:** answerable-today

Migration functions are typically written to run once, applied to production, and then left in the codebase. Over time they accumulate as dead code. This question finds them so they can be removed or archived.

**The query**

```cgx
cgx unused ./ --kind fn --path-filter '**/migrations/**'
```

Or with a name-pattern approach:

```cgx
cgx query '
  MATCH (fn {kind:"function"})
  WHERE (fn.name STARTS WITH "migrate_" OR fn.name STARTS WITH "migration_"
         OR fn.file CONTAINS "/migrations/")
    AND NOT ()-[:CALLS*]->(fn)
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.file CONTAINS "/migrations/"` | Target functions in the migrations directory by path convention. |
| `NOT ()-[:CALLS*]->(fn)` | No caller anywhere in the graph reaches this function, at any depth. |

**Reading the result** — Each returned function has zero callers. For migrations, that is expected once they have been applied. The list gives you a candidate set to archive or delete — cross-check against your migration runner's applied-migrations log before removing.

---

### Q50 — List all functions that reference crypto primitives but are not reachable from any current entrypoint.

**Personas:** PSE · **Status:** answerable-today

Unreachable functions that touch cryptography are a special concern: they may contain older, weaker crypto that was replaced, and their dormancy means they are not included in ongoing security reviews. If an attacker finds a way to reactivate them, the codebase is exposed to a downgrade attack.

**The query**

```cgx
cgx query '
  MATCH (fn {kind:"function"})-[:CALLS*]->(crypto {sink_class:"crypto"})
  WHERE NOT ()-[:CALLS*]->(fn)<-[:CALLS]-({kind:"entrypoint"})
  RETURN fn.name, fn.file, fn.line,
         collect(distinct crypto.name) AS crypto_callees
  ORDER BY fn.file, fn.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(fn {kind:"function"})-[:CALLS*]->(crypto {sink_class:"crypto"})` | Find functions that transitively call into at least one node classified as a `crypto` sink. |
| `NOT ()-[:CALLS*]->(fn)<-[:CALLS]-({kind:"entrypoint"})` | The function itself is not reachable from any entrypoint. |
| `collect(distinct crypto.name) AS crypto_callees` | Aggregate all crypto functions the dead code touches into one result row per dead function. |

**Reading the result** — Each row is a function that uses crypto but sits outside any live execution path. `crypto_callees` lists the specific crypto functions it calls. Prioritize reviewing functions that use deprecated algorithms (`md5`, `sha1`, `des`, `rc4`) — these are the highest-risk dormant code.

---

### Q51 — Before I delete function `F`, confirm it has zero callers from any entrypoint and is not referenced by any test entrypoint either.

**Personas:** ACA · **Status:** answerable-today

An AI coding agent about to delete a function needs a safety check: is the function truly unreachable from all execution roots, including test entrypoints? A false positive here causes a test suite failure.

**The query**

```cgx
cgx unused ./ --kind fn --entrypoint '**'
```

To check a specific function by name:

```cgx
cgx callers my_module::F ./ --depth 20 --confidence probable
```

A zero-result response confirms no callers exist. The Layer-2 form:

```cgx
cgx query '
  MATCH (caller)-[:CALLS*]->(fn {name:"my_module::F"})
  RETURN caller.name, caller.file, caller.line,
         caller.kind
  ORDER BY caller.file, caller.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx callers my_module::F ./ --depth 20` | Walk up to 20 hops backward from `F`; any result means a caller exists. |
| `--confidence probable` | Include probable-confidence edges (dynamic dispatch candidates) to avoid false assurance. |
| In the query: `MATCH (caller)-[:CALLS*]->(fn {name:"my_module::F"})` | Find any node that reaches `F` transitively. If this returns zero rows, `F` is safe to delete. |

**Reading the result** — Zero rows means no path to `F` exists in the indexed graph. Check `caller.kind` in non-empty results to distinguish production callers from test callers — if only test callers remain, the decision changes. Always run at `--confidence probable` or looser before deletion to catch dynamically-dispatched callers.

---

### Q52 — Which branches of `switch`/`match` statements on enum type `OrderStatus` are unreachable given actual call sites?

**Personas:** SSE · **Status:** needs-schema-room-feature (GM-19 build-configuration variance)

A `match` arm that no actual call site ever exercises is dead code at the branch level. This is finer-grained than function-level dead code: the function is reachable, but specific variant arms within it never fire.

**The query**

```cgx
-- illustrative: requires GM-19 (schema-room)
cgx query '
  MATCH (branch {kind:"match-arm", enum_type:"OrderStatus"})
  WHERE NOT ()-[:CALLS*]->(branch)
  RETURN branch.name, branch.variant, branch.file, branch.line
  ORDER BY branch.file, branch.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `kind:"match-arm"` | Targets individual arms of a `match`/`switch` statement, not the whole function. |
| `enum_type:"OrderStatus"` | Scope to arms that match on the `OrderStatus` enum specifically. |
| `NOT ()-[:CALLS*]->(branch)` | No execution path reaches this arm. |

**Reading the result** — Each returned row is a `match` arm that is structurally present but never triggered. This may indicate a removed feature (the variant was deleted from call sites but the arm was forgotten) or an impossible variant in the current data model. GM-19 schema population is needed for branch-level reachability attribution.

---

### Q53 — Which API endpoints defined in the router are never called by any integration test?

**Personas:** PSE · **Status:** answerable-today

An API endpoint with no integration test coverage is a blind spot: regressions in its behavior go undetected. From a security perspective, an untested endpoint may harbor vulnerabilities that would have been caught by a test exercising its auth and input-validation paths.

**The query**

```cgx
cgx query '
  MATCH (ep {kind:"entrypoint", entrypoint_class:"http"})
  WHERE NOT ()-[:CALLS*]->(ep)<-[:CALLS]-({kind:"entrypoint", entrypoint_class:"test"})
  RETURN ep.name, ep.file, ep.line,
         ep.framework_pack AS established_by
  ORDER BY ep.file, ep.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep {kind:"entrypoint", entrypoint_class:"http"}` | Find all HTTP entrypoints declared in the router (populated by framework packs or explicit `--entrypoint` declarations). |
| `NOT ()-[:CALLS*]->(ep)<-[:CALLS]-({kind:"entrypoint", entrypoint_class:"test"})` | Exclude endpoints that are reachable from a test entrypoint — i.e., keep only those with no test coverage. |
| `ep.framework_pack AS established_by` | Show which framework pack promoted this symbol to entrypoint, so you know whether the route is a Spring `@GetMapping`, an Axum handler, or something else. |

**Reading the result** — Each row is an HTTP endpoint that no integration test exercises. The `established_by` column identifies the framework that registered the endpoint, useful for tracing where the route is defined if the router is annotation-driven.
