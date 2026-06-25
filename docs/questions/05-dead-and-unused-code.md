# Theme 5: Dead and Unused Code

Dead code is source that exists in the repository but can never be reached during execution. It is a security risk beyond a tidiness issue: dormant functions accumulate technical debt, are rarely patched when vulnerabilities are found in their dependencies, and can be reactivated accidentally — the Knight Capital $440M incident (2012) is the canonical case. `cgx` answers dead-code questions at a finer grain than the Rust compiler's `dead_code` lint, which only reports items that are syntactically private. The queries here scope reachability to declared entrypoints, so a public method that no entrypoint ever reaches is correctly reported as unreachable.

---

### Q46 — Which public methods on `UserService` are never called from any entrypoint in our own codebase?

**Personas:** SSE · **Status:** answerable-today

A public method that no entrypoint ever calls is dead to the application even if it is technically exported. This question finds those methods on a specific type so a maintainer can decide whether to remove them or verify they are called by external consumers not visible in this repository.

**The query**

```cgx
cgx unused --repo PATH --kind method
```

`cgx unused` has no `--type` filter flag; use `--kind method` to restrict to methods. There is no Layer-2 CQL equivalent: CQL v0.3 requires a relationship in every `MATCH` clause, so a standalone "find nodes with no callers" pattern is not expressible without `NOT EXISTS` (deferred). Use `cgx unused --kind method` for the bulk scan.

Specified in docs/05 Canonical Example 3.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--kind method` | Restrict the unused search to methods (not standalone functions or fields). |

**Reading the result** — Each row is a method with no caller anywhere in the indexed codebase. `cgx unused` does not accept an `--entrypoint` filter flag; it reports all symbols not reachable from any declared entrypoint in the graph. Results are `certain` confidence when no call edge exists; `probable` when dynamic dispatch is possible.

---

### Q47 — Which authentication-related functions exist in the codebase but are never reachable from any live entrypoint?

**Personas:** PSE · **Status:** answerable-today

Authentication functions that are not reachable from any declared entrypoint are dormant security code. They may have been superseded by a newer auth path, may contain older vulnerabilities, and — if reactivated — could bypass current security controls.

**The query**

```cgx
cgx unused --repo PATH --kind function
```

`cgx unused` has no `--path-filter` flag. To filter by name pattern, use `cgx search` to identify auth-related function FQNs, then check reachability with `cgx callers`:

```cgx
cgx search auth --repo PATH
cgx callers my_module::authenticate --repo PATH --confidence probable
```

A zero-result `cgx callers` response confirms the function is unreachable. CQL v0.3 cannot express "find functions with zero callers" in a single query (standalone `MATCH (fn)` without a relationship is a plan error; `NOT EXISTS` is deferred). Use `cgx unused --kind function` for the bulk scan, then `cgx callers` to verify specific symbols.

Note: CQL does not support `CONTAINS`, `STARTS WITH`, or multi-condition node-property inline predicates (e.g. `{kind:"function", name:...}`). Name filtering requires exact FQN from `cgx search`.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--kind function` | Restrict to function symbols. `--kind fn` is not a valid value; the accepted value is `function`. |
| `cgx search auth --repo PATH` | Discover exact FQNs of auth-related functions before checking reachability. |
| `cgx callers ... --confidence probable` | Include probable-confidence edges (dynamic dispatch candidates) to avoid false assurance. |

**Reading the result** — A zero-result `cgx callers` confirms the function is unreachable from any declared entrypoint. Before deleting, verify the function is not part of a public library API consumed by external callers.

---

### Q48 — Which feature-flag branches are permanently dead given that flag `LEGACY_AUTH` is always false?

**Personas:** SSE · **Status:** not answerable as specced — requires branch-predicate modeling (no feature; see docs/05 'graph reachability, not path feasibility')

Determining which branches are permanently dead because a runtime flag is always false requires reasoning about runtime values flowing into branch conditions. `cgx` reports graph reachability, not path feasibility; branch predicates are not modelled at the call-graph level (docs/01 non-goals, docs/05 feasibility boundary). The adjacent answerable question — which nodes exist only under a *build-time* configuration flag — is Q126 / GM-19, which stays answerable.

---

### Q49 — Which database migration functions have already been applied and are now unreachable dead code?

**Personas:** SSE · **Status:** answerable-today

Migration functions are typically written to run once, applied to production, and then left in the codebase. Over time they accumulate as dead code. This question finds them so they can be removed or archived.

**The query**

```cgx
cgx unused --repo PATH --kind function
```

`cgx unused` has no `--path-filter` flag. To narrow to migration functions, use `cgx search` to find FQNs matching your naming convention, then verify each with `cgx callers`:

```cgx
cgx unused --repo PATH --kind function
cgx search migrate --repo PATH
cgx callers my_crate::migrations::v1_add_users --repo PATH --confidence probable
```

Note: CQL v0.3 cannot express "find functions with zero callers" in a standalone query (standalone `MATCH (fn)` without a relationship is a plan error; `NOT EXISTS` is deferred). CQL does not support `CONTAINS`, `STARTS WITH`, or file-path substring matching; use `cgx search --regex` to discover exact FQNs first.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--kind function` | Restrict to function symbols. `--kind fn` is not a valid value; the accepted value is `function`. |
| `cgx search migrate --repo PATH` | Discover exact FQNs of migration functions before checking reachability. |
| `cgx callers ... --confidence probable` | Include probable-confidence edges to avoid false assurance. Zero results confirms no callers. |

**Reading the result** — Each zero-result `cgx callers` confirms the function is unreachable. For migrations, that is expected once they have been applied. The list gives you a candidate set to archive or delete — cross-check against your migration runner's applied-migrations log before removing.

---

### Q50 — List all functions that reference crypto primitives but are not reachable from any current entrypoint.

**Personas:** PSE · **Status:** not answerable as specced — requires `sink_class` node property (deferred; plan error exit 2 in v0.3.0)

Unreachable functions that touch cryptography are a special concern: they may contain older, weaker crypto that was replaced, and their dormancy means they are not included in ongoing security reviews. If an attacker finds a way to reactivate them, the codebase is exposed to a downgrade attack.

The security-typed sink classification (`sink_class`, `source_class`, `sanitizer_class`, `taint_label`) is not yet backed by a node property in v0.3.0. Any query using these properties exits with a plan error (exit 2). The adjacent answerable form — finding unreachable functions whose names suggest crypto — requires first discovering the relevant FQNs via `cgx search`.

**The query (v0.3.0 — name-based approximation)**

```cgx
cgx unused --repo PATH --kind function
```

Then cross-reference the results against known crypto function names from `cgx search` (e.g. `cgx search sha1 --repo PATH`, `cgx search md5 --repo PATH`). The Layer-2 form filtering by known crypto callee name:

```cgx
cgx query '
  MATCH (fn)-[:CALLS*]->(crypto)
  WHERE crypto.name = "my_crate::crypto::sha1_hash"
    AND NOT ()-[:CALLS]->(fn)
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' --repo PATH
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(fn)-[:CALLS*]->(crypto)` | Find functions that transitively call the named crypto function. Replace `crypto.name` with the exact FQN from `cgx search`. |
| `NOT ()-[:CALLS]->(fn)` | No direct callers exist for `fn` in the indexed graph. |
| `sink_class:"crypto"` | **Not supported in v0.3.0** — exits with plan error (exit 2). Deferred to a future security-taint release. |

---

### Q51 — Before I delete function `F`, confirm it has zero callers from any entrypoint and is not referenced by any test entrypoint either.

**Personas:** ACA · **Status:** answerable-today

An AI coding agent about to delete a function needs a safety check: is the function truly unreachable from all execution roots, including test entrypoints? A false positive here causes a test suite failure.

**The query**

```cgx
cgx unused --repo PATH --kind function
```

`cgx unused` has no `--entrypoint` flag. It scans all symbols not reachable from any declared entrypoint in the indexed graph. To check a specific function by name:

```cgx
cgx callers my_module::F --repo PATH --depth 20 --confidence probable
```

A zero-result response confirms no callers exist. The Layer-2 form:

```cgx
cgx query '
  MATCH (caller)-[:CALLS*]->(fn {name:"my_module::F"})
  RETURN caller.name, caller.file, caller.line,
         caller.kind
  ORDER BY caller.file, caller.line
' --repo PATH
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx callers my_module::F --repo PATH --depth 20` | Walk up to 20 hops backward from `F`; any result means a caller exists. The repository path is `--repo PATH`, not a trailing positional argument. |
| `--confidence probable` | Include probable-confidence edges (dynamic dispatch candidates) to avoid false assurance. |
| In the query: `MATCH (caller)-[:CALLS*]->(fn {name:"my_module::F"})` | Find any node that reaches `F` transitively. If this returns zero rows, `F` is safe to delete. |

**Reading the result** — Zero rows means no path to `F` exists in the indexed graph. Check `caller.kind` in non-empty results to distinguish production callers from test callers — if only test callers remain, the decision changes. Always run at `--confidence probable` or looser before deletion to catch dynamically-dispatched callers.

---

### Q52 — Which branches of `switch`/`match` statements on enum type `OrderStatus` are unreachable given actual call sites?

**Personas:** SSE · **Status:** not answerable as specced — requires branch-predicate modeling (no feature; see docs/05 'graph reachability, not path feasibility')

Determining which `match` arms are unreachable given the actual values supplied at call sites requires reasoning about argument values flowing into the scrutinee — that is, path feasibility under argument-value constraints. `cgx` reports graph reachability, not path feasibility; branch predicates and runtime values are not modelled at the call-graph level (docs/01 non-goals, docs/05 feasibility boundary).

---

### Q53 — Which API endpoints defined in the router are never called by any integration test?

**Personas:** PSE · **Status:** not answerable as specced — requires `entrypoint_class` node property (deferred; plan error exit 2 in v0.3.0)

An API endpoint with no integration test coverage is a blind spot: regressions in its behavior go undetected. From a security perspective, an untested endpoint may harbor vulnerabilities that would have been caught by a test exercising its auth and input-validation paths.

The `entrypoint_class` node property (e.g. `"http"`, `"test"`) is not backed in v0.3.0. Any query filtering on `entrypoint_class` exits with a plan error (exit 2). The adjacent answerable form — finding all symbols with kind `entrypoint` that have no callers — uses only supported properties:

**The query (v0.3.0 — kind-only filter)**

```cgx
cgx unused --repo PATH --kind entrypoint
```

There is no working Layer-2 CQL equivalent: CQL v0.3 requires a relationship in every `MATCH` clause, so a standalone "find nodes with no callers" pattern cannot be expressed without `NOT EXISTS` (deferred). Use `cgx unused --kind entrypoint` for the bulk scan.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--kind entrypoint` | Restrict to symbols promoted to entrypoints. Filtering to `"http"` or `"test"` subtypes via `entrypoint_class` is **not supported in v0.3.0** — exits with plan error (exit 2). |
| `ep.framework_pack AS established_by` | **Not supported in v0.3.0** — `framework_pack` is not a known node property in this release. |

**Reading the result** — Each row is a symbol declared as an entrypoint with no callers in the indexed graph. HTTP-vs-test subtype discrimination and framework-pack attribution are deferred to a future release.
