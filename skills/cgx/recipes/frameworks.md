# Recipe: Framework Semantics and Metadata

**Theme 12 of 13** — Annotation-declared entrypoints, guards, DI-wired edges,
reflective dispatch, and mediated call sites.

**Not answerable in v0.1; requires cgx >= 0.3.**

Run `cgx --version` before using this recipe. The entire theme is gated on v0.3
framework packs (GM-15/GM-17/GM-18). A capability marked `Since: v0.N` requires
`MINOR >= N`. See `reference/versions.md`.

**Step 0 — find the exact symbol name first.**
cgx has no search, glob, or fuzzy match. An unknown symbol exits 2 with
`no symbol matched '<x>'`. Grep/ripgrep the source for the fully-qualified name
before running any cgx command.

---

## Why this theme requires v0.3

Frameworks (Spring, Flask, Django, ASP.NET Core, NestJS, tokio) do much of their
work through annotations, decorators, and registration calls that produce no call
expression in the source code. A `@PreAuthorize` annotation enforces a guard
without appearing in any call path; a `@KafkaListener` creates an entrypoint that
no static reachability traversal finds on its own.

Framework packs lower annotation semantics into graph facts across seven semantic
classes (GM-15: `entrypoint`, `guard`, `negative-guard`, `interception`,
`generated-member`, `keep-alive`, `contract`). The graph then answers framework-aware
queries using those facts. This capability ships at v0.3.

The one capability that partially arrives at v0.2 is literal-reflection resolution
(Q123): when the string passed to a reflection call is a compile-time constant,
the graph can resolve the call to `probable` confidence. The reflection edge schema
(`cut_marker:"reflective"`, `string_pedigree`) is reserved in v0.1 but the
resolution data is only present from v0.2 onward.

**In v0.1, none of the queries in this recipe run or return non-empty results.**
The node properties they depend on (`entrypoint_class`, `guard_class`,
`negative_guard_class`, `framework_pack`, `cut_marker`, `string_pedigree`,
`established_by`, `cfg_condition`) are schema-reserved but not populated.
Queries against these properties either plan-error (exit 2) or return zero rows.

---

## Spec-only forms — documented, not yet runnable

All eight questions in Theme 12 are `Since: v0.3` (one partially at v0.2). They
are shown here in their documented v0.3 form. Do not emit them as runnable in a
v0.1 environment.

Before each query, check:

```bash
cgx --version   # must be 0.3.x or higher for any query below
```

---

### Does every HTTP handler path traverse the authorization guard?

**Status:** spec-only   **Since:** v0.3   **Personas:** PSE

Without framework packs, a must-pass-through query on an annotated codebase
produces false positives for every `@PreAuthorize`-annotated endpoint — the
annotation's check has no call expression in the source path. GM-15 lowers
`@PreAuthorize` (and Django `@login_required`, ASP.NET `[Authorize]`, etc.) to a
`guard` fact on the symbol node. The query below consumes that fact.

```bash
# Verify version first
cgx --version   # must be >= 0.3

cgx query 'MATCH path = (h {kind:"entrypoint", entrypoint_class:"http"})-[:CALLS*6]->(sink)
WHERE sink.name = "db::write"
  AND NONE(r IN relationships(path)
           WHERE r.condition IN ["exception","panic"])
  AND NONE(n IN nodes(path)
           WHERE n.name = "require_admin"
              OR n.guard_class IS NOT NULL)
RETURN h.name, h.file, h.line,
       sink.name, sink.file, sink.line,
       h.framework_pack AS handler_established_by,
       length(path) AS hops
ORDER BY hops, h.file, h.line'
```

CI gate (exits 1 if any bypass path found):

```bash
# Since: v0.3 — --avoiding does not exist in v0.1 (exits 2)
# Use the CQL form above with --assert-empty instead
cgx query '...' --assert-empty
```

**Why this works:** `h {kind:"entrypoint", entrypoint_class:"http"}` matches
HTTP handler entrypoints populated from annotation packs (Spring `@GetMapping`,
actix-web `#[get(...)]`). The `NONE(... n.guard_class IS NOT NULL)` clause
eliminates paths that pass through any annotation-guard node, resolving false
positives for `@PreAuthorize`-annotated endpoints.

**Reading the result:** Non-empty results are confirmed authorization bypass
paths — handlers that reach the sink on a non-exception path without a named guard
or annotation guard in between. Zero results means every handler is covered.
Use `--format sarif` for GitHub Advanced Security inline annotations. `handler_established_by`
narrows the finding to a specific framework's handler type.

Note: `--avoiding` is listed in the upstream cookbook but does not exist in the
v0.1 binary (exits 2). The `NONE(... guard_class IS NOT NULL)` CQL clause is the
correct equivalent once v0.3 ships.

---

### Which endpoints have a negative-guard annotation disabling a default protection?

**Status:** spec-only   **Since:** v0.3   **Personas:** PSE

Annotations like `@csrf_exempt`, `[AllowAnonymous]`, and `@PermitAll` explicitly
disable a protection the framework applies by default. GM-15 populates
`negative_guard_class` facts from framework packs. Enumerating them is a single
graph lookup once those facts are present.

```bash
# Since: v0.3
cgx query 'MATCH (ep {kind:"entrypoint"})
WHERE ep.negative_guard_class IS NOT NULL
RETURN ep.name, ep.file, ep.line,
       ep.negative_guard_class AS disabled_protection,
       ep.framework_pack
ORDER BY ep.negative_guard_class, ep.name'
```

**Why this works:** `ep.negative_guard_class IS NOT NULL` selects every
entrypoint carrying a GM-15 `negative-guard` fact. The `negative_guard_class`
value names the protection disabled: `"csrf"`, `"authz"`, `"role-check"`, etc.
`ep.framework_pack` identifies which framework recognized the annotation.

**Reading the result:** Each row is an endpoint where a protection is explicitly
disabled. `disabled_protection = "authz"` rows warrant highest scrutiny — they
disable the authorization layer entirely for that endpoint. A `@csrf_exempt` on
a public read-only endpoint is expected; one on a state-changing POST handler is
a vulnerability.

---

### Which annotation-declared entrypoints lack authentication coverage?

**Status:** spec-only   **Since:** v0.3   **Personas:** PSE

Framework-registered entrypoints are invisible to plain reachability analysis:
no call expression invokes `@GetMapping` handlers — the framework does. Once
GM-15 and framework packs populate the entrypoint set, the query below finds
which annotation-declared entrypoints are not covered by authentication
middleware.

```bash
# Since: v0.3
cgx query 'MATCH (ep {kind:"entrypoint"})
WHERE ep.guard_class IS NULL
  AND ep.name NOT IN ["health_check", "favicon", "metrics"]
RETURN ep.name, ep.file, ep.line,
       ep.entrypoint_class,
       ep.framework_pack AS established_by
ORDER BY ep.framework_pack, ep.entrypoint_class, ep.name'
```

Extended form — entrypoints that also lack an auth check on paths to protected
resources:

```bash
# Since: v0.3
cgx query 'MATCH path = (ep {kind:"entrypoint"})-[:CALLS*6]->(resource)
WHERE resource.is_protected = true
  AND ep.guard_class IS NULL
  AND NONE(n IN nodes(path)
           WHERE n.name IN ["authenticate", "check_auth", "require_login"]
              OR n.guard_class IS NOT NULL)
RETURN ep.name, ep.file, ep.line,
       ep.framework_pack AS established_by,
       resource.name, length(path) AS hops
ORDER BY hops, ep.name'
```

**Why this works:** `ep.guard_class IS NULL` selects entrypoints with no
annotation guard on themselves. The `NOT IN` exclusion list removes
known-intentional unguarded endpoints (adjust to match the codebase's
conventions). The extended form also checks whether any guard appears anywhere
on the path to a protected resource.

**Reading the result:** Each row is a framework-declared entrypoint not covered
by any annotation guard or named auth function. `established_by` tells you which
framework registered it — relevant for determining whether the endpoint type
(Kafka consumer, scheduled task) needs HTTP authentication at all.

---

### Which reflective dispatch calls receive user-controlled strings?

**Status:** spec-only   **Since:** v0.3   **Personas:** PSE

A reflective dispatch call (`Method.invoke`, `getattr`, `Class.forName`)
invoked with an attacker-controlled string means an attacker controls which code
executes. GM-18 models `string_pedigree` on reflection call edges; the query
below finds DATA_FLOW paths from network/user-input sources to reflection
dispatch sites without a method-name sanitizer.

```bash
# Since: v0.3 — DATA_FLOW edge type and taint properties are not in v0.1
cgx query 'MATCH path = (src)-[:DATA_FLOW*6]->(dispatch {cut_marker:"reflective"})
WHERE src.source_class IN ["network","user-input"]
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "method-name")
RETURN src.name, src.file, src.line,
       dispatch.name, dispatch.file, dispatch.line,
       [e IN relationships(path) | e.transformation_kind] AS transforms,
       dispatch.string_pedigree
ORDER BY src.file, src.line'
```

**Why this works:** `dispatch {cut_marker:"reflective"}` is a node with the
GM-18 reflective cut-marker — a call that cannot be resolved to a static target.
`NONE(... sanitizer_class = "method-name")` confirms no whitelist sanitizer
sits on the path. `dispatch.string_pedigree` reports the pedigree classification
of the string reaching the dispatch.

**Reading the result:** Focus on rows where `dispatch.string_pedigree = "tainted"` —
attacker input directly controls method dispatch. The `transforms` column shows
whether the string was encoded or passed through unchanged. `dispatch.file`/`line`
is the reflection call site to fix.

---

### Which reflective calls have a literal-pedigree string and what are the probable targets?

**Status:** spec-only   **Since:** v0.2 (reflection edge schema); full resolution at v0.3   **Personas:** SSE

When the string passed to a reflection call is a compile-time constant, cgx can
resolve the call to `probable` confidence. This turns an unresolvable reflection
call into a navigable edge — useful for understanding framework internals, ORM
mappings, and plugin systems. The `string_pedigree` attribute and reflection
edge schema are reserved in v0.1 but resolved edge data is only present from v0.2.

```bash
# Since: v0.2 for literal-pedigree resolution
cgx query 'MATCH (callsite)-[r:CALLS]->(target)
WHERE callsite.cut_marker = "reflective"
  AND r.confidence IN ["certain","probable"]
RETURN callsite.name, callsite.file, callsite.line,
       target.name, target.file, target.line,
       r.confidence AS resolution_confidence,
       callsite.string_pedigree
ORDER BY r.confidence DESC, callsite.file, callsite.line'
```

For unresolved reflection sites (no `probable` or better target found):

```bash
# Since: v0.2
cgx query 'MATCH (callsite)-[r:CALLS]->(target)
WHERE callsite.cut_marker = "reflective"
  AND r.confidence = "possible"
RETURN callsite.name, callsite.file, callsite.line
ORDER BY callsite.file, callsite.line'
```

**Why this works:** `r.confidence IN ["certain","probable"]` returns only
candidates where GM-18 resolved the string to a literal or constant pedigree.
The `possible`-only form surfaces the unresolvable call sites — the blind spots
where the call graph is incomplete.

**Reading the result:** Resolved rows (`certain`/`probable`) make reflection
sites navigable for impact analysis and dead-code detection. Unresolved rows
(`possible`) are the blind spots where a security review must examine the string
argument manually at runtime.

Note: `r.confidence` is schema-present from v0.1 for CALLS edges. It begins to
discriminate (`certain` vs `probable` vs `possible`) at v0.2, where CHA/RTA
`dyn Trait` resolution and SCIP enrichment raise edge tiers (see
`reference/versions.md`).

---

### Which call edges are established by dependency injection, not a direct call?

**Status:** spec-only   **Since:** v0.3   **Personas:** SSE

DI-wired edges are invisible in source code: the container calls the constructor
and injects the dependency; no call expression links the injection site to the
implementation. GM-17 synthesizes these as mediated call edges with
`established-by` provenance.

```bash
# Since: v0.3
cgx query 'MATCH (caller)-[r:CALLS]->(callee)
WHERE r.established_by IS NOT NULL
RETURN caller.name, caller.file, caller.line,
       callee.name, callee.file, callee.line,
       r.established_by AS wiring_mechanism,
       r.confidence AS edge_confidence,
       r.wiring_annotation AS annotation_used
ORDER BY r.confidence DESC, caller.file, caller.name'
```

For auditing which frameworks are doing the wiring:

```bash
# Since: v0.3
cgx query 'MATCH (caller)-[r:CALLS]->(impl)
WHERE r.established_by IS NOT NULL
RETURN caller.name, impl.name,
       r.established_by AS wired_by,
       r.confidence
ORDER BY wired_by, caller.name'
```

**Why this works:** `r.established_by IS NOT NULL` selects edges carrying a
GM-17 provenance attribute — edges created by DI, event dispatch, or a route
registry rather than a direct call expression. `r.wiring_annotation` names the
specific annotation or config entry that established the edge (`@Autowired`,
`@Inject`, `@Bean`).

**Reading the result:** Each row is a DI-wired call edge invisible in the source
code. The `wired_by` and `annotation_used` columns explain what created the
edge. Confidence reflects DI wiring certainty: `certain` for compile-time DI
(Dagger 2), `probable` for runtime containers (Spring, NestJS) with clear
annotation evidence, `possible` for string-name-based registries.

---

### Does tainted data flowing through a channel reach a sensitive sink?

**Status:** spec-only   **Since:** v0.3   **Personas:** PSE

A channel send↔recv pair is a non-call dataflow edge: the sender puts a value
into the channel; the receiver takes it out; no function call links sender to
receiver. DF-20 models these as explicit `derives-from` linkages so taint
propagates through the channel. This query requires DATA_FLOW edges, which are
not in v0.1.

```bash
# Since: v0.3 — DATA_FLOW edges are not in v0.1
cgx query 'MATCH path = (src {source_class:"network"})-[:DATA_FLOW*6]->(sink)
WHERE sink.sink_class IN ["sql", "shell", "file_write"]
  AND ANY(edge IN relationships(path)
          WHERE edge.linkage_kind = "channel-send-recv")
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = sink.sink_class)
RETURN src.name, src.file, src.line,
       sink.name, sink.sink_class, sink.file, sink.line,
       [e IN relationships(path) | e.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY sink.sink_class, hops'
```

**Why this works:** `ANY(edge IN relationships(path) WHERE edge.linkage_kind = "channel-send-recv")`
requires at least one DF-20 channel linkage on the path — taint crossed a channel
boundary. `NONE(... sanitizer_class = sink.sink_class)` confirms no class-matched
sanitizer sits between source and sink.

**Reading the result:** Each row is a taint path where attacker-controlled data
crossed a channel boundary and reached a dangerous sink without sanitization.
The `linkage_kind = "channel-send-recv"` edge is the channel crossing point;
`transforms` shows how the tainted value was shaped in transit.

---

### Which call paths to a symbol exist only when a build flag is enabled?

**Status:** spec-only   **Since:** unscheduled (GM-19 per-configuration indexing)   **Personas:** SSE

A path that exists only in a specific build configuration is invisible to
standard static analysis. GM-19 reserves the `cfg-condition` attribute on edges;
the query below finds paths whose edges are gated by a named feature flag.
Per-configuration indexing is roadmap-only and has no scheduled version.

```bash
# Since: unscheduled — GM-19 cfg-condition attribute; do not promise a version
# Illustrative form only
cgx query '-- illustrative: requires GM-19 per-configuration indexing
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*6]->(target)
WHERE target.name = "legacy_auth"
  AND ALL(r IN relationships(path)
          WHERE r.cfg_condition = "feature = \"LEGACY_AUTH\""
             OR r.cfg_condition IS NULL)
  AND ANY(r IN relationships(path)
          WHERE r.cfg_condition = "feature = \"LEGACY_AUTH\"")
RETURN ep.name, ep.file, ep.line,
       [r IN relationships(path) | r.cfg_condition] AS cfg_conditions,
       length(path) AS hops
ORDER BY hops, ep.name'
```

**Why this works (when GM-19 ships):** `ALL(r IN relationships(path) WHERE ...)`
requires every edge to be either unconditional or gated by the named flag —
filtering to paths that exist only in that build configuration.
`ANY(... cfg_condition = ...)` confirms at least one edge is actually gated,
ruling out always-present paths.

**Reading the result:** Each row is a call path absent from the default
production build. The `cfg_conditions` column shows exactly which edges are
gated. These are dormant paths — present in the binary when the flag is enabled
but never active in production. Do not emit this query as runnable until `cgx --version` reports GM-19 support; there is no `MINOR >= N` gate to check yet.

---

## Summary — version gates for this theme

| Question | Since | Key properties |
|----------|-------|----------------|
| Authorization bypass (guard check) | v0.3 | `entrypoint_class`, `guard_class`, `framework_pack` |
| Negative-guard inventory | v0.3 | `negative_guard_class`, `framework_pack` |
| Unauthenticated annotation entrypoints | v0.3 | `entrypoint_class`, `guard_class`, `framework_pack` |
| Tainted reflective dispatch | v0.3 | `DATA_FLOW`, `cut_marker`, `source_class`, `sanitizer_class`, `string_pedigree` |
| Literal-pedigree reflection resolution | v0.2 (schema) / v0.3 (full) | `cut_marker`, `string_pedigree`, `confidence` |
| DI-wired edges (`established-by`) | v0.3 | `established_by`, `wiring_annotation` |
| Channel-crossing taint | v0.3 | `DATA_FLOW`, `linkage_kind`, `source_class`, `sink_class` |
| Build-flag-gated paths | unscheduled | `cfg_condition` (GM-19) |

---

## Common pitfalls for this theme

| Mistake | Correct behaviour |
|---------|------------------|
| Emitting `--avoiding SYM` | `--avoiding` does not exist in any version; use `NONE(... guard_class IS NOT NULL)` in CQL at v0.3 |
| Querying `entrypoint_class` in v0.1 | Plan error, exit 2; gate on `cgx --version >= 0.3` |
| Unbounded `CALLS*` | Always bound hops: `CALLS*6`; unbounded hangs |
| `DATA_FLOW` edges in v0.1 | Parses but returns empty; not an error — just no data |
| Using `NOT IN [...]` in WHERE | Parse error in v0.1; use `NONE(... WHERE ...)` instead |
| `MATCH (n:Label)` with no relationship | Plan error; every MATCH must contain at least one relationship |

See `reference/query-language.md` for the full CQL dialect reference and
`reference/cli.md` for all real flag names.
