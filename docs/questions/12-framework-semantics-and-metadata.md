# Theme 12: Framework Semantics and Metadata

Modern frameworks do much of their work through annotations, decorators, and
registration calls that have no corresponding call expression in the source code.
A Spring `@PreAuthorize` annotation enforces access control without appearing in
any call path; a `@KafkaListener` creates an entrypoint that no static reachability
analysis will find on its own. These eight questions use framework packs (docs/12)
to lower annotation semantics into graph facts — seven semantic classes, enumerated
in FW-2 — so that must-pass-through, taint, and reachability queries work correctly
on framework-heavy codebases. All queries use docs/05 Q-31. Readers with passing
query-language familiarity will find each clause explained in the breakdown tables.

---

### Q119 — Does every path from an HTTP handler to a protected resource traverse the `@PreAuthorize` annotation guard — or can any path reach the resource without the guard being on the call path?

**Personas:** PSE · **Status:** core-extension — uses Q-31 framework-aware queries, Q-20 must-pass-through, GM-15 `guard` semantic class (`core-extension`)

Without framework packs, a must-pass-through query on an annotated codebase reports
every `@PreAuthorize`-annotated endpoint as an authorization bypass — because the
annotation's check has no call expression in the source path. GM-15 lowers
`@PreAuthorize` (and equivalents in other frameworks) to a `guard` fact on the
symbol node, and Q-31's metadata-guard-aware query consumes that fact to eliminate
false positives. Specified in docs/05 Q-31 (metadata-guard-aware authorization bypass).

**The query**

```cgx
MATCH path = (h {kind:"entrypoint", entrypoint_class:"http"})-[:CALLS*]->(sink {name:"db::write"})
WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  AND NONE(n IN nodes(path)
           WHERE n.name = "require_admin"
              OR n.guard_class IS NOT NULL)
RETURN h.name, h.file, h.line,
       sink.name, sink.file, sink.line,
       h.framework_pack AS handler_established_by,
       length(path) AS hops
ORDER BY hops, h.file, h.line
```

CI gate (exits 1 if any bypass path exists):

```bash
cgx paths --from 'kind:entrypoint,entrypoint_class:http' \
          --to db::write \
          --avoiding 'name:require_admin OR guard_class:*' \
          --exclude-edge-condition exception \
          --assert-empty
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `h {kind:"entrypoint", entrypoint_class:"http"}` | HTTP handler entrypoints. These include both explicitly declared entrypoints and those populated from annotation packs (e.g. Spring `@GetMapping`, actix-web `#[get(...)]`). |
| `NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Restrict to non-exception paths — structural bypasses on the happy path. Exception-class bypass paths are a separate concern. |
| `n.name = "require_admin" OR n.guard_class IS NOT NULL` | The combined guard check: either a named guard function is on the path, or any node carries a non-null `guard_class` attribute from GM-15. The second clause is what eliminates false positives for `@PreAuthorize`-annotated endpoints. |
| `h.framework_pack AS handler_established_by` | Which framework pack registered this handler as an entrypoint — useful for triaging findings by framework. |

**Reading the result** — Non-empty results are confirmed authorization bypass paths: HTTP handlers that reach the protected sink on a non-exception path without any named guard function or annotation guard in between. Zero results means every handler is covered by at least one guard. Use `--format sarif` for inline annotations in GitHub Advanced Security.

---

### Q120 — Which endpoints carry a negative-guard annotation — `@csrf_exempt`, `[AllowAnonymous]`, or `@PermitAll` — disabling a protection that is on by default?

**Personas:** PSE · **Status:** core-extension — uses Q-31 framework-aware queries, GM-15 `negative-guard` semantic class (`core-extension`)

A negative-guard annotation explicitly disables a protection that the framework
applies by default to all endpoints. These annotations are often legitimate
(public health-check endpoints, CORS preflight handlers) but are also a common
misconfiguration source. Enumerating them is a single graph lookup once GM-15
has populated `negative_guard_class` facts from framework packs. Specified in
docs/05 Q-31 (negative-guard inventory).

**The query**

```cgx
MATCH (ep {kind:"entrypoint"})
WHERE ep.negative_guard_class IS NOT NULL
RETURN ep.name, ep.file, ep.line,
       ep.negative_guard_class AS disabled_protection,
       ep.framework_pack
ORDER BY ep.negative_guard_class, ep.name
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep {kind:"entrypoint"}` | All entrypoints — HTTP handlers, scheduled tasks, message consumers — regardless of class. |
| `ep.negative_guard_class IS NOT NULL` | The endpoint carries a GM-15 `negative-guard` fact with a named protection class. The `negative_guard_class` attribute is set by the framework pack for the matching annotation: `@csrf_exempt` → `"csrf"`, `[AllowAnonymous]` → `"authz"`, `@PermitAll` → `"role-check"`. |
| `ep.negative_guard_class AS disabled_protection` | Returns the protection that has been explicitly disabled — the human-readable name of what is turned off. |
| `ep.framework_pack` | Which framework pack recognized the annotation — Spring Security, Django, ASP.NET Core, etc. |

**Reading the result** — Each row is an endpoint where a protection is explicitly disabled. Review each finding against the intent: a `@csrf_exempt` on a public read-only API endpoint is expected; one on a state-changing POST handler is a vulnerability. Entries with `disabled_protection = "authz"` warrant the highest scrutiny — they disable the authorization layer entirely for that endpoint.

---

### Q121 — Which annotation-declared entrypoints (`@GetMapping`, `@app.route`, `#[tokio::main]`, `@KafkaListener`) are reachable without passing through the authentication middleware?

**Personas:** PSE · **Status:** core-extension — uses Q-31 framework-aware queries, GM-15 `entrypoint` semantic class, GM-16 implicit call sites, Q-20 must-pass-through (`core-extension`)

Framework-registered entrypoints are invisible to plain reachability analysis: no
call expression in the source code invokes `@GetMapping` handlers — the framework
does. Once GM-15 and framework packs have populated the entrypoint set, Q-31's
framework-entrypoint reachability query finds which of these annotation-declared
entry points are not covered by the authentication middleware. Specified in
docs/05 Q-31 (framework entrypoint reachability).

**The query**

```cgx
MATCH (ep {kind:"entrypoint"})
WHERE ep.guard_class IS NULL
  AND ep.name NOT IN ["health_check", "favicon", "metrics"]
RETURN ep.name, ep.file, ep.line,
       ep.entrypoint_class,
       ep.framework_pack AS established_by
ORDER BY ep.framework_pack, ep.entrypoint_class, ep.name
```

To find annotation-declared entrypoints that specifically lack auth middleware on every path to a protected resource:

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(resource)
WHERE resource.is_protected = true
  AND ep.guard_class IS NULL
  AND NONE(n IN nodes(path)
           WHERE n.name IN ["authenticate", "check_auth", "require_login"]
              OR n.guard_class IS NOT NULL)
RETURN ep.name, ep.file, ep.line,
       ep.framework_pack AS established_by,
       resource.name, length(path) AS hops
ORDER BY hops, ep.name
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep.guard_class IS NULL` | The entrypoint carries no GM-15 guard fact — no `@PreAuthorize`, `@login_required`, or equivalent annotation guard on the entry itself. |
| `ep.name NOT IN ["health_check", "favicon", "metrics"]` | Exclude known-intentional unguarded endpoints — public health checks and static assets that do not need authentication. Adjust this list to match the codebase's conventions. |
| `ep.framework_pack AS established_by` | Which framework pack declared this as an entrypoint — narrows the finding to a specific framework's annotation type. |
| `n.guard_class IS NOT NULL` | In the extended query: any node on the path to the resource carries a guard annotation — the authentication check may be on a downstream service or inner call rather than the entrypoint itself. |

**Reading the result** — Each row is a framework-declared entrypoint that has neither an annotation guard on itself nor any named authentication function on the path to a protected resource. `established_by` tells you which framework created this entrypoint — important for triaging whether this is a Spring handler, a Kafka consumer, or an actix route. Entrypoint classes like `kafka-consumer` or `scheduled` may not need HTTP authentication but may need authorization for the operations they perform.

---

### Q122 — Which reflective dispatch calls — `Method.invoke`, `getattr`, `Class.forName` — receive a string whose pedigree includes user-controlled input?

**Personas:** PSE · **Status:** core-extension — uses Q-31 framework-aware queries, GM-18 reflection and string-mediated dispatch, taint propagation (`core-extension`)

A reflective dispatch call invoked with an attacker-controlled string means an
attacker controls which code executes. GM-18 models the `string_pedigree` attribute
on reflection call edges, and framework packs provide reflection pack entries for
common reflection sites. Specified in docs/05 Q-31 (tainted-string reflective dispatch).

**The query**

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(dispatch {cut_marker:"reflective"})
WHERE src.source_class IN ["network","user-input"]
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "method-name")
RETURN src.name, src.file, src.line,
       dispatch.name, dispatch.file, dispatch.line,
       [e IN relationships(path) | e.transformation_kind] AS transforms,
       dispatch.string_pedigree
ORDER BY src.file, src.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `dispatch {cut_marker:"reflective"}` | The reflection call site — a node with the GM-18 `reflective` cut-marker, set when the call cannot be resolved to a static target because it goes through `Method.invoke`, `getattr`, `Class.forName`, or a similar reflection API. |
| `src.source_class IN ["network","user-input"]` | The string originates from attacker-controlled input — the network or direct user input. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "method-name")` | No function on the path sanitizes the string against a whitelist of allowed method names — the string reaches the reflection call raw. |
| `dispatch.string_pedigree` | The pedigree classification of the string reaching the dispatch: `literal` (a string constant — safe, resolvable), `constant` (a compile-time expression), `joined` (concatenated from constants), or `tainted` (includes attacker data — the highest-severity case). |
| `[e IN relationships(path) | e.transformation_kind] AS transforms` | The transformation sequence from source to dispatch — shows whether the string was encoded, formatted, or passed through unchanged. |

**Reading the result** — Each row is a tainted-string-to-reflection path. Focus on rows where `dispatch.string_pedigree = "tainted"` — these are the cases where attacker input directly controls method dispatch. The `dispatch.file`/`line` is the reflection call site to fix. The `transforms` column shows what processing (if any) the string underwent before reaching the call.

---

### Q123 — Which reflective calls have a string with a literal pedigree — the class or method name comes from a string constant — and what are the probable call targets?

**Personas:** SSE · **Status:** core-extension — uses Q-31 framework-aware queries, GM-18 literal-pedigree reflection resolution (`core-extension` in Phase 2 for literal resolution; base reflection schema is Phase 1)

When the string passed to a reflection call is a compile-time constant, `cgx` can
resolve the call to `probable` confidence. This turns an unresolvable reflection
call into a navigable edge in the call graph — useful for understanding framework
internals, ORM mappings, and plugin systems. Specified in docs/05 Q-31.

**The query**

```cgx
MATCH (callsite)-[r:CALLS:indirect]->(target)
WHERE callsite.cut_marker = "reflective"
  AND r.confidence IN ["certain","probable"]
RETURN callsite.name, callsite.file, callsite.line,
       target.name, target.file, target.line,
       r.confidence AS resolution_confidence,
       callsite.string_pedigree
ORDER BY r.confidence DESC, callsite.file, callsite.line
```

For unresolved reflection sites (no `probable` or better target found):

```cgx
MATCH (callsite)-[r:CALLS:indirect]->(target)
WHERE callsite.cut_marker = "reflective"
  AND r.confidence = "possible"
  AND size(callsite.candidate_set) = 0
RETURN callsite.name, callsite.file, callsite.line,
       r.cut_marker
ORDER BY callsite.file, callsite.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `callsite.cut_marker = "reflective"` | This call site is a reflection dispatch point — the actual target is not visible from a direct call expression. |
| `r.confidence IN ["certain","probable"]` | Only return candidates where the resolution has at least `probable` confidence — meaning the string argument had a literal or constant pedigree that GM-18 could resolve. |
| `callsite.string_pedigree` | Confirms the pedigree classification that drove the resolution (`literal` or `constant` for the resolvable cases). |
| `size(callsite.candidate_set) = 0` | In the unresolved query: the candidate set is empty — the string was dynamic or the pedigree traversal found no constant anchor. These are the blind spots in the call graph. |

**Reading the result** — Resolved rows (`certain`/`probable`) provide the actual call targets for reflection sites, making them navigable in the call graph for impact analysis and dead-code detection. Unresolved rows (`possible`, empty candidate set) are the blind spots — locations where the call graph is incomplete and a security review may need to examine the string argument manually at runtime. DroidRA and CodeQL Java Reflection handle subsets of these cases for specific platforms; `cgx` extends coverage to any language where the string has a traceable literal pedigree.

---

### Q124 — Which call edges in this Spring or NestJS application are established by dependency injection rather than a direct call expression — and what annotation or config entry established each edge?

**Personas:** SSE · **Status:** core-extension — uses Q-31 framework-aware queries, GM-17 mediated call edges, `established-by` provenance (`core-extension`)

DI-wired edges are invisible in the source code: the container calls the constructor
and injects the dependency; no call expression links the injection site to the
implementation. GM-17 synthesizes these as mediated call edges with `established-by`
provenance. Specified in docs/05 Q-31 (framework-aware queries via GM-15/GM-17).

**The query**

```cgx
MATCH (caller)-[r:CALLS {established_by:"annotation"}]->(callee)
WHERE r.established_by IS NOT NULL
RETURN caller.name, caller.file, caller.line,
       callee.name, callee.file, callee.line,
       r.established_by AS wiring_mechanism,
       r.confidence AS edge_confidence,
       r.wiring_annotation AS annotation_used
ORDER BY r.confidence DESC, caller.file, caller.name
```

For auditing which frameworks are doing the wiring:

```cgx
MATCH (ep {kind:"entrypoint"})-[:CALLS*]->(sink)
WHERE ep.entrypoint_class = "http"
MATCH (caller)-[r:CALLS]->(impl)
WHERE r.established_by IS NOT NULL
RETURN ep.name, ep.framework_pack,
       caller.name, impl.name,
       r.established_by AS wired_by,
       r.confidence
ORDER BY ep.framework_pack, caller.name
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `r.established_by IS NOT NULL` | The call edge carries an `established-by` provenance attribute from GM-17 — it was created by DI, event dispatch, or a route registry, not a direct call expression. |
| `r.established_by = "annotation"` | Narrows to edges established by annotation-driven DI (Spring `@Autowired`, NestJS `@Injectable`, CDI `@Inject`). |
| `r.wiring_annotation AS annotation_used` | The specific annotation or config entry that established the edge — `@Autowired`, `@Inject`, `@Bean`, etc. Stored in the provenance record. |
| `r.confidence AS edge_confidence` | DI wiring confidence: `certain` for compile-time DI (Dagger 2), `probable` for runtime containers (Spring, NestJS) with clear annotation evidence, `possible` for string-name-based registries. |

**Reading the result** — Each row is a DI-wired call edge invisible in the source code. The `wired_by` and `annotation_used` columns explain what created the edge. This is the answer to "why does this class appear to be called when nothing in the source code calls it?" — and it enables full reachability analysis on framework-heavy codebases without false dead-code reports. No production tool exposes `established-by` provenance on DI edges as a queryable graph attribute; this is a genuine novelty claim in `cgx`.

---

### Q125 — Does tainted data sent on a Go channel or Rust `mpsc` channel reach a sensitive sink on the receiving side — tracing the dataflow through the send↔recv pair?

**Personas:** PSE · **Status:** core-extension — uses Q-31 framework-aware queries, DF-20 non-call dataflow linkages, taint propagation through channel edges (`core-extension`)

A channel send↔recv pair is a non-call dataflow edge: the sender puts a value into
the channel; the receiver takes it out; there is no function call from sender to
receiver. DF-20 models these as explicit `derives-from` linkages, so taint
propagates through the channel and appears in the pedigree of the received value.
Specified in docs/05 Q-31 (via DF-20 and taint propagation).

**The query**

```cgx
MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink)
WHERE sink.sink_class IN ["sql", "shell", "file_write"]
  AND ANY(edge IN relationships(path)
          WHERE edge.linkage_kind = "channel-send-recv")
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = sink.sink_class)
RETURN src.name, src.file, src.line,
       sink.name, sink.sink_class, sink.file, sink.line,
       [e IN relationships(path) | e.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY sink.sink_class, hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src {source_class:"network"}` | The source of tainted data is network input — attacker-controlled. |
| `sink.sink_class IN ["sql", "shell", "file_write"]` | The sink is a high-severity vulnerability class: SQL injection, shell command injection, or arbitrary file write. |
| `ANY(edge IN relationships(path) WHERE edge.linkage_kind = "channel-send-recv")` | At least one edge on the path is a DF-20 channel linkage — the taint crossed a channel boundary. Without this filter, the query would also return direct taint paths (covered by Q86 and Q92). |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = sink.sink_class)` | No class-matched sanitizer sits between source and sink on the path — the taint reaches the sink without being cleaned. |
| `[e IN relationships(path) | e.transformation_kind] AS transforms` | The transformation sequence — shows how the tainted value was shaped as it passed through the channel and any intermediate functions. |

**Reading the result** — Each row is a taint path where attacker-controlled data crossed a channel boundary and reached a dangerous sink without sanitization. The `linkage_kind = "channel-send-recv"` edge is the channel crossing point. In all existing tools, the channel send breaks the pedigree: the receiver side has no edge back to the sender and taint is lost. `cgx` models the channel as a first-class `derives-from` linkage, so taint propagates through it. This is a direct answer to the class of "taint crossed a Go channel" or "tainted data sent on mpsc" vulnerabilities that SAST tools miss.

---

### Q126 — Which call paths to the legacy authentication function exist only when build flag `LEGACY_AUTH` is enabled — and are they absent in the default production build?

**Personas:** SSE · **Status:** schema-room — depends on GM-19 build-configuration variance, `cfg-condition` attribute (`schema-room`; per-configuration indexing is `roadmap`)

A path that exists only in a specific build configuration is invisible to standard
static analysis — tools scan one configuration at a time without recording which
edges are configuration-conditional. GM-19 reserves the `cfg-condition` attribute
on edges; Q-126 queries for paths whose entire set of edges is gated by the named
feature flag, meaning the path is absent in the default production build.
Specified in docs/05 Q-31 (via GM-19).

**The query**

```cgx
-- illustrative: requires GM-19 (schema-room) `cfg-condition` attribute on nodes and edges
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(target {name:"legacy_auth"})
WHERE ALL(r IN relationships(path)
          WHERE r.cfg_condition = "feature = \"LEGACY_AUTH\""
             OR r.cfg_condition IS NULL)
  AND ANY(r IN relationships(path)
          WHERE r.cfg_condition = "feature = \"LEGACY_AUTH\"")
RETURN ep.name, ep.file, ep.line,
       [r IN relationships(path) | r.cfg_condition] AS cfg_conditions,
       length(path) AS hops
ORDER BY hops, ep.name
```

To find all configuration-gated paths to any sink class (broader security gate):

```cgx
-- illustrative: requires GM-19 (schema-room)
MATCH (ep {kind:"entrypoint"})-[r:CALLS]->(sink)
WHERE sink.sink_class IN ["sql","shell","eval"]
  AND r.cfg_condition IS NOT NULL
RETURN ep.name, ep.file, sink.name, sink.sink_class,
       r.cfg_condition AS gating_condition
ORDER BY r.cfg_condition, sink.sink_class
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `r.cfg_condition = "feature = \"LEGACY_AUTH\""` | Each edge on the path is either unconditional (`cfg_condition IS NULL`, present in all builds) or gated by the named feature flag — meaning the path only exists when `LEGACY_AUTH` is enabled. |
| `ALL(r IN relationships(path) WHERE ...)` | All edges on the path satisfy the condition — a path with even one edge gated by a different flag would not appear in this result set. |
| `ANY(r IN relationships(path) WHERE r.cfg_condition = "feature = \"LEGACY_AUTH\"")` | At least one edge is actually gated — confirming the path is feature-flag-conditional, not always-present. |
| `[r IN relationships(path) | r.cfg_condition] AS cfg_conditions` | The full list of `cfg-condition` values on the path edges — most will be `null` (unconditional) with one or more being the named flag. |

**Reading the result** — Each row is a call path that exists only in `LEGACY_AUTH`-enabled builds and is absent from the default production build. The `cfg_conditions` column shows exactly which edges are gated. Findings here indicate security-relevant code paths that are never active in production — they are dormant but present in the binary and could be reactivated by enabling the feature flag. No existing static analysis tool represents build-flag-gated paths as a queryable edge attribute; `cgx`'s `cfg-condition` attribute makes this a first-class graph query.
