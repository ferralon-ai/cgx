# Theme 7: API Surface and Contracts

A codebase's API surface is the set of symbols that are callable from outside a module or crate boundary. Questions in this theme help maintainers understand what is actually public (versus nominally public), how much of that surface is exercised, and whether internal functions are leaking across module boundaries in ways that create implicit coupling. Five of the six questions here are NOVEL — the existing tool closest to this space (the Rust compiler) can flag some dead public items but cannot express the transitive footprint, cross-module coupling, or cross-version diff questions that engineers and security reviewers need.

---

### Q62 — Which public functions in our library crate are never called by any of our own binary crates or integration tests?

**Personas:** SSE · **Status:** answerable-today

A public function that no internal consumer ever calls may be dead API surface: exported for external consumers who may not exist, or left over from a refactor. Pruning it reduces attack surface and maintenance burden.

**The query**

```cgx
cgx unused ./ --kind fn --confidence certain
```

Or scoped to the library crate's public surface:

```cgx
cgx query '
  MATCH (fn {kind:"function", visibility:"pub"})
  WHERE NOT ()-[:CALLS*]->(fn)<-[:CALLS]-({kind:"entrypoint"})
  RETURN fn.name, fn.file, fn.line
  ORDER BY fn.file, fn.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `visibility:"pub"` | Restrict to functions declared `pub` (publicly exported). |
| `NOT ()-[:CALLS*]->(fn)<-[:CALLS]-({kind:"entrypoint"})` | No execution path from any declared entrypoint reaches this function. |
| `--confidence certain` | Only report functions with no caller edges at any confidence tier — excludes `possible` edges from over-approximated dynamic dispatch. |

**Reading the result** — Each returned function is public but has no internal callers. Before removing it, verify it is not part of an intentionally exported API consumed by external crates not indexed here. If the crate is a library with external consumers, this list is candidates for deprecation, not immediate deletion.

---

### Q63 — Which internal functions are called from outside their defining module, creating implicit coupling?

**Personas:** PSE · **Status:** answerable-today

Functions that are not `pub` but are still called from other modules via re-export, `pub(crate)`, or `#[doc(hidden)]` workarounds create implicit coupling: the caller depends on an implementation detail that the module owner never intended to be stable. From a security perspective, such coupling can route untrusted data through paths that were designed to be internal-only.

**The query**

```cgx
cgx query '
  MATCH (caller)-[:CALLS]->(fn)
  WHERE caller.module <> fn.module
    AND fn.visibility IN ["pub(crate)", "pub(super)", "private"]
  RETURN fn.name, fn.module AS fn_module,
         fn.file, fn.line,
         caller.name, caller.module AS caller_module,
         caller.file, caller.line
  ORDER BY fn.module, fn.name
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `caller.module <> fn.module` | The caller and callee live in different modules. |
| `fn.visibility IN ["pub(crate)", "pub(super)", "private"]` | The function is not declared `pub` — it was not designed for cross-module use. |
| `fn.module AS fn_module` / `caller.module AS caller_module` | Surfaces the module paths so you can see exactly which module boundary is being crossed. |

**Reading the result** — Each row is a cross-module call to a non-public function. Group by `fn_module` to find modules with the most coupling violations. The combination of `fn.file`/`fn.line` and `caller.file`/`caller.line` gives the exact source locations for both ends of each violation.

---

### Q64 — What is the complete call graph footprint of our public API — every function transitively reachable from each public method?

**Personas:** SSE · **Status:** answerable-today

The transitive footprint of a public method is the set of all functions that method can ever invoke, across all call paths. For a library, this footprint is the true scope of the API contract: any function in that set can be affected by a dependency change or a security vulnerability.

**The query**

```cgx
cgx callees 'my_crate::PublicApi::*' ./ --depth 20 --format json
```

Or per-method with aggregation:

```cgx
cgx query '
  MATCH (pub_fn {kind:"function", visibility:"pub"})-[:CALLS*1..20]->(callee)
  WHERE NOT callee = pub_fn
  RETURN pub_fn.name AS api_method,
         collect(distinct callee.name) AS footprint,
         count(distinct callee) AS footprint_size
  ORDER BY footprint_size DESC
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `CALLS*1..20` | Traverse call edges transitively, up to 20 hops deep. Increase the limit for deeply nested call chains; decrease for a quick overview. |
| `collect(distinct callee.name) AS footprint` | Aggregate all reachable callees into a list per public method. `distinct` avoids counting the same function multiple times. |
| `count(distinct callee) AS footprint_size` | The size of the transitive footprint gives a quick health metric: a public API method with a footprint of 500 functions has a very large blast radius. |

**Reading the result** — Methods with the largest `footprint_size` carry the most risk: a bug or vulnerability anywhere in their footprint can affect callers. This output is the starting point for a threat model or for identifying which methods to prioritize for security review.

---

### Q65 — Which exported functions call back into the caller's provided closures/callbacks, and what do those callbacks have access to?

**Personas:** SSE · **Status:** needs-schema-room-feature (DF-18 function values and closures)

When an exported function accepts a callback and invokes it, the callback runs with whatever context the exported function provides. Understanding what data and capabilities the callback receives matters for API contract design and security review of third-party callback code.

**The query**

```cgx
-- illustrative: requires DF-18 (schema-room)
cgx query '
  MATCH (exported_fn {visibility:"pub"})-[:CALLS]->(callback_site)
  WHERE callback_site.kind = "indirect-call"
  MATCH (exported_fn)-[:DATA_FLOW*]->(arg_to_callback)
  WHERE arg_to_callback.scope = callback_site.name
  RETURN exported_fn.name, exported_fn.file, exported_fn.line,
         callback_site.name, callback_site.file, callback_site.line,
         collect(distinct arg_to_callback.name) AS callback_args
  ORDER BY exported_fn.name
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `callback_site.kind = "indirect-call"` | The call site is a function-value invocation, not a direct named call. |
| `(exported_fn)-[:DATA_FLOW*]->(arg_to_callback)` | Values that flow from the exported function's scope into the callback's argument positions — what the callback receives. |
| `collect(distinct arg_to_callback.name) AS callback_args` | The set of values the callback can observe, including any sensitive data from the exported function's context. |

**Reading the result** — Each row shows an exported function that invokes a caller-provided callback, along with the data the callback receives. If `callback_args` includes security-sensitive values (tokens, PII, internal state), the exported function is providing callers more trust than intended. DF-18 capture-edge schema must be populated for this query to run.

---

### Q66 — Which functions cross trust boundaries (e.g., move data from untrusted to trusted zones) without explicit annotation?

**Personas:** PSE · **Status:** answerable-today

Trust boundaries are the lines between zones of different trust level: an HTTP handler is untrusted-input territory; the database write layer should only receive validated, trusted data. Functions that cross these boundaries without annotation are implicit trust elevations — potentially the entry points for injection attacks.

**The query**

```cgx
cgx query '
  MATCH (src)-[:CALLS*]->(fn)-[:CALLS*]->(dst)
  WHERE src.trust_zone = "untrusted"
    AND dst.trust_zone = "trusted"
    AND fn.trust_boundary_annotated = false
    AND NONE(n IN nodes(
              (src)-[:CALLS*]->(fn)
             ) WHERE n.sanitizer_class IS NOT NULL)
  RETURN fn.name, fn.file, fn.line,
         src.name AS untrusted_src,
         dst.name AS trusted_dst
  ORDER BY fn.file, fn.line
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.trust_zone = "untrusted"` | The source node is in an untrusted zone (e.g., populated from a network source class). |
| `dst.trust_zone = "trusted"` | The destination node is in a trusted zone (e.g., a database write function). |
| `fn.trust_boundary_annotated = false` | The crossing function carries no explicit trust-boundary annotation (GM-14 code-trust boundaries). |
| `NONE(n IN nodes(...) WHERE n.sanitizer_class IS NOT NULL)` | No sanitizer appears on the path from the untrusted source to the crossing function. |

**Reading the result** — Each row is a function that moves data across a trust boundary without annotation or sanitization. These are the highest-priority candidates for adding input validation. The `untrusted_src` and `trusted_dst` columns name the specific endpoints of the boundary crossing.

---

### Q67 — Which public API methods changed their transitive call footprint between v1 and v2 of the library?

**Personas:** SSE · **Status:** answerable-today

When a library is versioned, callers expect a stable API contract. A public method whose transitive footprint changes silently — calling new functions, dropping old ones — may behave differently in ways that break callers or introduce security regressions.

**The query**

```cgx
cgx diff --base v1.0.0 --head v2.0.0 ./ \
    --from 'kind:entrypoint,visibility:pub' \
    --format json > api-footprint-diff.json
```

Or as a comparison of per-method footprints across versions:

```cgx
cgx query --at v1.0.0 '
  MATCH (pub_fn {kind:"function", visibility:"pub"})-[:CALLS*1..20]->(callee)
  RETURN pub_fn.name AS method,
         collect(distinct callee.name) AS v1_footprint
' ./ > v1-footprints.json

cgx query --at v2.0.0 '
  MATCH (pub_fn {kind:"function", visibility:"pub"})-[:CALLS*1..20]->(callee)
  RETURN pub_fn.name AS method,
         collect(distinct callee.name) AS v2_footprint
' ./ > v2-footprints.json
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--base v1.0.0 --head v2.0.0` | Pin each side of the diff to a release tag for a reproducible comparison. |
| `--from 'kind:entrypoint,visibility:pub'` | Start the diff from public API symbols — ignore internal call changes that do not affect the API surface. |
| `collect(distinct callee.name) AS v1_footprint` | Build the transitive footprint list at each version for later set-difference comparison. |

**Reading the result** — New entries in `v2_footprint` that are absent from `v1_footprint` are functions newly reachable from the public API in v2. If any of those new callees are in sensitive sink classes (`sql`, `shell`, `network`), the v2 release has expanded its trust surface and warrants a security review.
