# 12 — Language Primitives and Frameworks

**Status:** Feature specification (pre-implementation)
**Audience:** Engineers extending `cgx` with framework support; security engineers writing framework-aware queries; contributors implementing the metadata-fact layer
**Working name:** `cgx` (placeholder — see docs/README.md)
**Cross-references:** docs/03-code-graph-model.md (GM-15, GM-16, GM-17, GM-18, GM-19) · docs/04-dataflow-and-provenance.md (DF-14, DF-20) · docs/05-queries.md (Q-31) · docs/08-language-support.md (LS-8)

---

## Overview

Language-specific constructs — annotations, attributes, decorators, struct tags,
implicit calls, build flags, DI wiring — all collapse into a small set of
cross-language graph concepts. The structural call graph handles function bodies
well. The gap that most tools leave open is **metadata whose semantics come from
a framework, not the language**. A Java annotation is syntactically uniform;
whether `@PreAuthorize` expresses a security guard or `@Transactional` expresses
an interception proxy is determined entirely by the Spring framework, not by the
Java specification.

This document specifies:

- FW-1: The framework-pack design
- FW-2: The seven metadata semantic classes
- FW-3: Per-framework worked mappings (Spring, Flask/Django, ASP.NET, NestJS, tokio/actix)
- FW-4: Mediated edges — DI, events, and registries
- FW-5: Build-configuration variance
- FW-6: Reflection packs

The graph primitives that these features lower to are defined in
docs/03-code-graph-model.md (GM-15 through GM-19). This document maps framework
constructs onto those primitives; it does not redefine them.

---

## FW-1: Framework Pack Design

**Status:** `schema-room` — the pack schema and the metadata-fact representation
(GM-15) must be reserved before the schema stabilises; pack evaluation against
live indices follows once GM-15 is implemented.

### FW-1.1 — Motivation

`cgx` cannot ship knowledge of every framework. Even for supported frameworks,
the set of "security-relevant annotations" changes with library versions and
in-house convention. The solution is the same pattern already used for taint
sources, sinks, and sanitizers: a **declarative config** that the tool evaluates
at index time, supplemented by built-in packs that cover major frameworks out of
the box.

A framework pack is a collection of **pack entries**. Each entry maps a
(language, metadata pattern) pair to one or more graph facts with a confidence
label. Users write packs to teach `cgx` about in-house frameworks; contributors
write packs to extend built-in coverage; `cgx` ships packs for the frameworks in
FW-3.

### FW-1.2 — Pack entry schema

Each entry in a framework pack contains:

```toml
[[framework_packs.entries]]
language    = "java"                        # language tag matching LS-1 language names
matcher     = { annotation = "org.springframework.web.bind.annotation.GetMapping" }
facts       = [
  { class = "entrypoint", kind = "http-handler" },
]
confidence  = "certain"                     # certain | probable | possible
evidence    = "annotation-present"          # what the rule cites as justification
```

The `matcher` field selects which metadata on a symbol triggers this entry.
Matcher kinds:

| Matcher kind | Description | Example |
|---|---|---|
| `annotation` | Fully-qualified annotation / attribute / decorator / struct-tag name | `"org.springframework.security.access.prepost.PreAuthorize"` |
| `annotation_prefix` | Any annotation whose FQN starts with the given prefix | `"jakarta.persistence."` |
| `struct_tag_key` | A Go struct-tag key | `"json"` |
| `derive_trait` | A Rust `#[derive(...)]` argument | `"serde::Serialize"` |
| `attribute_name` | A Rust outer attribute name | `"tokio::main"` |
| `registration_site` | An imperative register/handler call rather than a metadata carrier; names the registration function and the argument position of the handler — see FW-4.3 | `{ function = "net/http.HandleFunc", handler_arg = 1 }` |

The `facts` array declares the metadata semantic class (or classes) this entry
asserts. One annotation may lower to multiple classes: for example,
`@Async` in Spring lowers to both `interception` (the proxy intercepts the call
and dispatches asynchronously) and modifies the outbound `calls` edge to a
`spawns` edge (GM-9) — both facts are declared in the same entry.

The `confidence` field is the confidence attached to the resulting graph facts.
Compile-time annotations with a single unambiguous semantic (e.g. `@GetMapping`)
are `certain`. Container-managed annotations whose resolution depends on runtime
state (e.g. `@ConditionalOnProperty`) are `probable`.

The `evidence` field names what the producing rule cites. Values: `annotation-present`,
`attribute-present`, `struct-tag-present`, `config-file`, `registration-site`.
This value is stored in the GM-6 provenance record so that `--evidence` output
can name the specific annotation that justified the fact.

### FW-1.3 — Built-in packs

`cgx` ships built-in packs for the frameworks enumerated in FW-3. Built-in packs
are compiled into the binary and active by default for any indexed project that
imports the relevant framework.

Framework detection is heuristic: `cgx` inspects the dependency manifest
(Cargo.toml, pom.xml, build.gradle, requirements.txt, package.json, go.mod) for
known framework package identifiers. When a framework dependency is detected,
its pack is activated. Users can force-enable or disable packs in `cgx.toml`:

```toml
[framework_packs]
force_enabled  = ["spring-security", "flask-login"]
force_disabled = ["actix-web"]
```

### FW-1.4 — User-extensible packs

Users declare custom packs in `cgx.toml` using the same schema as built-in packs.
Custom entries are merged with the built-in pack for the same language; they do not
replace it. Conflicts (same matcher, different facts) are flagged as a warning at
index time.

```toml
[[framework_packs.entries]]
language   = "java"
matcher    = { annotation = "com.myapp.security.RequiresAdmin" }
facts      = [{ class = "guard", kind = "role-check" }]
confidence = "certain"
evidence   = "annotation-present"
```

### FW-1.5 — Prior art: CodeQL Models as Data

The framework pack design draws on the same principle as CodeQL's "Models as Data"
(MaD) approach, which separates the specification of library summaries, sources,
sinks, and sanitizers from the analysis engine, allowing community-contributed models
to extend coverage without modifying analysis code. `cgx`'s pack schema applies this
principle specifically to metadata-semantic lowering (annotation → graph fact).
The R1 research findings should be consulted to confirm the precise MaD framing
before this paragraph is finalised; the claim made here is conservative — structural
similarity, not identity.

---

## FW-2: The Seven Metadata Semantic Classes

**Status:** `schema-room` — the semantic class vocabulary is reserved in the
schema (GM-15) now; the full query surface for metadata-aware queries (Q-31)
follows once GM-15 is implemented.

Metadata on a symbol — an annotation, attribute, decorator, or struct tag — is
a carrier. The carrier has no inherent graph semantics; the framework pack entry
assigns the semantics by declaring which class the metadata belongs to.

The seven classes are:

| Class | Graph effect | Cross-reference |
|---|---|---|
| `entrypoint` | The annotated symbol is added to the entrypoint set | GM-7, GM-15 |
| `guard` | An authz/authc check governs access to the annotated symbol; modelled as a must-pass-through fact | GM-15, Q-31 (Q-20 pattern) |
| `negative-guard` | A protection is explicitly disabled for the annotated symbol | GM-15, Q-31 |
| `interception` | Call edges to/from the annotated symbol pass through a proxy or advice layer; the actual execution target is not the syntactic callee | GM-15, GM-17 |
| `generated-member` | The annotated symbol has members synthesised by a code generator; those members exist without source | GM-15, GM-16 |
| `keep-alive` | The annotated member is accessed reflectively or via serialization even if no explicit call to it exists in the graph | GM-15, GM-18 |
| `contract` | The annotation carries a contract on values (nullable, non-null, range constraint, etc.) | GM-15, DF-14 |

### FW-2.1 — `entrypoint`

An entrypoint annotation declares that the framework will invoke the annotated
symbol from outside the statically visible call graph — it is a root for
reachability. Without entrypoint metadata, reachability queries report false
dead-code results on every framework codebase.

Concrete examples across ecosystems:

| Ecosystem | Annotation / attribute | Lowering |
|---|---|---|
| Spring (Java) | `@GetMapping`, `@PostMapping`, `@RequestMapping`, `@Scheduled`, `@KafkaListener` | Symbol added to entrypoint set with `kind = "http-handler"` or `kind = "scheduled"` or `kind = "kafka-consumer"` |
| Flask (Python) | `@app.route("/path")` | Symbol added to entrypoint set with `kind = "http-handler"` |
| Django (Python) | URL-mapped view function (config-file match, not annotation) | Symbol added to entrypoint set via config-file pack entry |
| ASP.NET (C#) | `[HttpGet]`, `[HttpPost]`, `[Route]` | Symbol added to entrypoint set with `kind = "http-handler"` |
| Express/NestJS (TypeScript) | `@Get()`, `@Post()`, `@Controller()` | Symbol added to entrypoint set |
| tokio (Rust) | `#[tokio::main]` | Symbol added to entrypoint set with `kind = "async-main"` |
| actix-web (Rust) | `#[get("/path")]`, `#[post("/path")]` | Symbol added to entrypoint set with `kind = "http-handler"` |
| JUnit / pytest / Jest | `@Test`, `test_*` prefix, `it()`/`test()` | Symbol added to entrypoint set with `kind = "test"` (already in GM-7.1 auto-detection; pack supplements it for annotation-driven cases) |

### FW-2.2 — `guard`

A guard annotation means: the framework evaluates an authz or authc condition
before invoking the annotated symbol. There is no call in the source code; the
check is the annotation.

This is the highest-stakes class. A must-pass-through query (Q-20) that does not
account for guard annotations will report a false authz bypass on every codebase
that uses annotation-driven security — which is the majority of Spring, ASP.NET,
and Flask-Login codebases.

When a symbol carries a `guard` fact, `cgx` inserts a synthetic `must-pass-through`
fact for that symbol in the entrypoint-reachability context, equivalent to Q-20's
semantics: any path from an entrypoint to the symbol passes through the guard.

Concrete examples:

| Ecosystem | Annotation / attribute | Guard semantics |
|---|---|---|
| Spring Security (Java) | `@PreAuthorize("hasRole('ADMIN')")` | Method is only invoked if the SpEL expression evaluates true at runtime |
| Spring Security (Java) | `@Secured("ROLE_ADMIN")` | Role membership required |
| ASP.NET (C#) | `[Authorize]`, `[Authorize(Roles = "Admin")]` | ASP.NET authorization filter must pass before action executes |
| Flask-Login (Python) | `@login_required` | User must be authenticated |
| Django (Python) | `@permission_required("myapp.can_edit")` | Named permission required |
| NestJS (TypeScript) | `@UseGuards(AuthGuard)` | NestJS guard must return `true` |
| Spring Method Security (Java) | `@PostAuthorize("returnObject.owner == authentication.name")` | Post-invocation check (the method runs, then the return value is checked) |

Note: `@PostAuthorize` is still modelled as a `guard`, but its timing is
post-invocation. The pack entry can carry a `timing = "post"` attribute to
distinguish it from pre-invocation guards; queries that ask "which paths bypass
the guard before reaching the symbol" treat post-guards differently.

### FW-2.3 — `negative-guard`

A negative guard annotation explicitly disables a protection that would otherwise
apply. This inverts the guard model: instead of requiring a condition to be true,
it asserts that the protection is absent.

"Which endpoints have authentication disabled?" is a pure metadata query, and
often a high-signal security finding.

Concrete examples:

| Ecosystem | Annotation / attribute | Semantics |
|---|---|---|
| ASP.NET (C#) | `[AllowAnonymous]` | Disables the `[Authorize]` filter applied by controller-level or global policy |
| Flask (Python) | `@csrf_exempt` (Django) / no `@login_required` | Absence of the guard annotation; also explicit opt-out |
| Django (Python) | `@csrf_exempt` | CSRF protection disabled for this view |
| Spring Security (Java) | `@PermitAll` (Jakarta EE) | Unconditionally permits access regardless of role |
| Rust `#[allow(...)]` | `#[allow(clippy::...)]` | Linter protection disabled (not authz, but same negative-guard pattern at the tooling level) |

### FW-2.4 — `interception`

An interception annotation means the framework interposes a proxy between the
caller and the annotated symbol. The syntactic call expression reaches a proxy
that may: start a transaction, apply caching, execute the call on a different
thread, run security checks, or measure latency. The proxy calls the real method
only after its own logic completes — or not at all (cache hit).

This changes the call graph structure: the `calls` edge from the caller does not
go directly to the annotated method; it goes to the proxy. `cgx` models this as
a mediated edge (GM-17) with `established-by: annotation`.

When the interception converts a synchronous call to an asynchronous one (e.g.
`@Async` in Spring), the pack entry additionally asserts that the outbound edge
is a `spawns` edge (GM-9), not a `calls` edge.

Concrete examples:

| Ecosystem | Annotation | Interception semantics |
|---|---|---|
| Spring (Java) | `@Transactional` | Proxy opens/commits/rolls back a transaction around the method body |
| Spring (Java) | `@Async` | Proxy dispatches the method body to a thread pool; caller gets a `Future`; edge becomes `spawns` |
| Spring (Java) | `@Cacheable` | Proxy checks the cache; may not invoke the method body at all on cache hit |
| AspectJ (Java) | Pointcut + advice | AOP advice executes before, after, or around any matching join point |
| CDI (Java/Jakarta) | `@Interceptor` / `@AroundInvoke` | CDI interceptor chain executes around the target method |
| Castle DynamicProxy (C#) | `[Intercept]` / `IInterceptor` | Dynamic proxy intercepts method call |

### FW-2.5 — `generated-member`

A generated-member annotation means: a code generator adds members to the
annotated type at compile time. Those members exist as real call targets but have
no source. Call edges to generated symbols are valid; `cgx` must know the members
exist.

Pack entries for generated-member annotations trigger `cgx` to synthesise
symbol nodes for the generated members, following the generation rules declared
in the pack (e.g. Lombok `@Getter` on field `String name` → method `getName()`).
These synthetic nodes carry `macro_origin` = the generator name (GM-14.5).

Concrete examples:

| Ecosystem | Annotation / attribute | Generated members |
|---|---|---|
| Lombok (Java) | `@Getter` | `getX()` for each field |
| Lombok (Java) | `@Setter` | `setX(T)` for each non-final field |
| Lombok (Java) | `@Builder` | Builder class + `builder()` factory + `build()` method |
| Lombok (Java) | `@EqualsAndHashCode` | `equals(Object)`, `hashCode()` |
| Lombok (Java) | `@ToString` | `toString()` |
| Rust `#[derive(...)]` | `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]` | `fmt()`, `clone()`, `eq()`, `serialize()`, `deserialize()` implementations |
| C# source generators | `[GenerateSerializer]`, Roslyn source generator | Generated partial class members |
| MapStruct (Java) | `@Mapper` | Interface implementation with field-mapping methods |

### FW-2.6 — `keep-alive`

A keep-alive annotation means: the annotated member is accessed by the framework
through reflection or serialization, even though no explicit call to it appears
in the static call graph. Without this class, dead-member analysis (DF-7) falsely
reports these members as unused.

Concrete examples:

| Ecosystem | Annotation / struct tag | Semantics |
|---|---|---|
| Jackson (Java) | `@JsonProperty("field_name")` | Jackson accesses this field during serialization/deserialization |
| JPA/Hibernate (Java) | `@Column`, `@Entity`, `@Id` | ORM accesses these fields and classes via reflection |
| Spring (Java) | `@Value("${config.key}")` | Spring injects a value into this field via reflection |
| serde (Rust) | `#[serde(rename = "x")]`, `#[serde(skip)]` | serde accesses these fields via the generated `Serialize`/`Deserialize` impl |
| Go | `json:"field_name"`, `yaml:"field_name"`, `db:"field_name"` | encoding/json, yaml.v3, sqlx access these fields by struct-tag key |
| .NET (C#) | `[JsonPropertyName("x")]` | System.Text.Json accesses this property |
| Pydantic (Python) | Field with `model_fields` or `Field(alias="x")` | Pydantic accesses this field for validation and serialization |

### FW-2.7 — `contract`

A contract annotation carries a constraint on the values of the annotated symbol
— typically nullability, range, or format. These annotations lower directly to
DF-14 nullability flow facts (for `@Nullable` / `@NotNull`) or to user-defined
validate constraints.

Concrete examples:

| Ecosystem | Annotation | Contract |
|---|---|---|
| JSR-305 (Java) | `@Nullable`, `@NotNull` | The annotated parameter or return value may / must not be null; lowers to DF-14 |
| Jakarta Bean Validation | `@NotNull`, `@Min(0)`, `@Email` | Validation constraint on the value; `@NotNull` lowers to DF-14; others lower to `validate(class)` (DF-10) |
| Kotlin | `T?` nullable type | Structural nullability — already tracked by LS-3; pack entry not needed but consistent |
| Rust | Type system enforces non-null via `Option<T>` | Structural; pack entry not needed |
| C# | `[NotNull]`, nullable reference types (`T?`) | Lowers to DF-14 |

---

## FW-3: Per-Framework Worked Mappings

This section works through five representative frameworks — one for each major
ecosystem — showing concretely how pack entries lower framework metadata to graph
facts. Each example names the annotation or attribute, the semantic class it
belongs to, the confidence, and the resulting graph fact or edge modification.

### FW-3.1 — Spring (Java)

Spring is the canonical annotation-heavy framework: security, transactions, async,
caching, scheduling, and messaging are all annotation-driven. This is the most
complete worked mapping.

**Entrypoints:**

| Annotation | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@GetMapping("/path")` | `entrypoint` | `certain` | Symbol added to entrypoint set; `kind = "http-handler"`; HTTP method and path in provenance |
| `@PostMapping`, `@PutMapping`, `@DeleteMapping`, `@PatchMapping` | `entrypoint` | `certain` | As above with appropriate HTTP method |
| `@RequestMapping` | `entrypoint` | `certain` | Supports all HTTP methods unless `method` attribute is set |
| `@Scheduled(fixedRate = 5000)` | `entrypoint` | `certain` | `kind = "scheduled"`; schedule expression in provenance |
| `@KafkaListener(topics = "orders")` | `entrypoint` | `probable` | `kind = "kafka-consumer"`; topic in provenance; `probable` because broker presence is a runtime condition |
| `@EventListener` | `entrypoint` | `probable` | `kind = "spring-event-handler"`; event type in provenance |
| `@RabbitListener` | `entrypoint` | `probable` | `kind = "rabbitmq-consumer"` |

**Guards:**

| Annotation | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@PreAuthorize("hasRole('ADMIN')")` | `guard` | `certain` | Guard fact on symbol; expression in provenance |
| `@Secured("ROLE_ADMIN")` | `guard` | `certain` | Role in provenance |
| `@RolesAllowed("admin")` | `guard` | `certain` | Jakarta EE equivalent |
| `@PostAuthorize(...)` | `guard` | `certain` | `timing = "post"` in provenance |

**Negative guards:**

| Annotation | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@PermitAll` | `negative-guard` | `certain` | Disables any role restriction on this symbol |

**Interception:**

| Annotation | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@Transactional` | `interception` | `certain` | `established-by: annotation`; proxy pattern; resource pair (begin/commit or begin/rollback) implied — see GM-13 |
| `@Async` | `interception` | `certain` | Outbound call edge becomes `spawns` (GM-9); return type `Future<T>` or `CompletableFuture<T>` |
| `@Cacheable` | `interception` | `certain` | Proxy may skip method body on cache hit; method body reachability is `conditional`, not `always` |
| `@CacheEvict`, `@CachePut` | `interception` | `certain` | Cache mutation; method body is always invoked |
| `@Retryable` | `interception` | `certain` | Method body may execute multiple times; loop-like edge condition |

**Generated members:**

| Annotation | Semantic class | Confidence | Generated symbols |
|---|---|---|---|
| `@Getter` (Lombok) | `generated-member` | `certain` | `getX()` for each annotated field |
| `@Builder` (Lombok) | `generated-member` | `certain` | Builder class + `builder()` + `build()` |
| `@Data` (Lombok) | `generated-member` | `certain` | Getters, setters, `equals`, `hashCode`, `toString` |

**Keep-alive:**

| Annotation | Semantic class | Confidence | Semantics |
|---|---|---|---|
| `@JsonProperty` | `keep-alive` | `certain` | Jackson accesses field; suppress dead-member finding |
| `@Value("${key}")` | `keep-alive` | `certain` | Spring injects value via reflection |
| `@Autowired` (field injection) | `keep-alive` | `certain` | Spring injects dependency via reflection; field appears unused statically |

### FW-3.2 — Flask and Django (Python)

Python web frameworks use decorators and URL-config for entrypoints; security is
decorator-driven.

**Flask:**

| Decorator | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@app.route("/path")` | `entrypoint` | `certain` | Symbol added to entrypoint set; `kind = "http-handler"`; path and methods in provenance |
| `@app.route("/path", methods=["POST"])` | `entrypoint` | `certain` | HTTP method constraint in provenance |
| `@login_required` (Flask-Login) | `guard` | `certain` | Guard fact; requires authenticated session |
| `@roles_required("admin")` (Flask-Principal) | `guard` | `certain` | Role guard |
| `@limiter.limit("10/minute")` (Flask-Limiter) | `interception` | `probable` | Rate-limit interception; `probable` because rate-limiter middleware presence is a runtime condition |

**Django:**

| Decorator / mechanism | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@login_required` | `guard` | `certain` | Guard fact |
| `@permission_required("myapp.can_edit")` | `guard` | `certain` | Named permission guard |
| `@staff_member_required` | `guard` | `certain` | Staff-member guard |
| `@csrf_exempt` | `negative-guard` | `certain` | CSRF protection disabled for this view |
| `@cache_page(60 * 15)` | `interception` | `certain` | Response cached; view body may not execute on cache hit |
| URL-to-view mapping in `urls.py` | `entrypoint` | `probable` | Config-file pack entry; `probable` because URL routing is dynamic at load time |

### FW-3.3 — ASP.NET Core (C#)

ASP.NET attributes decorate controller actions. The security model uses attribute-based
authorization filters evaluated before the action body executes.

| Attribute | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `[HttpGet]`, `[HttpPost]`, `[HttpPut]`, `[HttpDelete]` | `entrypoint` | `certain` | HTTP method and route in provenance |
| `[Route("[controller]/[action]")]` | `entrypoint` | `certain` | Route template in provenance |
| `[Authorize]` | `guard` | `certain` | Default authorization policy must pass |
| `[Authorize(Roles = "Admin")]` | `guard` | `certain` | Role constraint in provenance |
| `[Authorize(Policy = "RequireAdminRole")]` | `guard` | `certain` | Named policy in provenance |
| `[AllowAnonymous]` | `negative-guard` | `certain` | Overrides `[Authorize]` from controller level or global filter |
| `[ValidateAntiForgeryToken]` | `guard` | `certain` | CSRF token validation guard |
| `[IgnoreAntiforgeryToken]` | `negative-guard` | `certain` | Disables CSRF validation |
| `[OutputCache]` | `interception` | `certain` | Response caching; action may not execute on cache hit |
| `[ServiceFilter(typeof(MyFilter))]` | `interception` | `probable` | Action filter interception; `probable` because filter type resolved at startup |
| `[NotNull]`, `[Required]` | `contract` | `certain` | Nullability and presence constraints; lower to DF-14 |
| `[JsonPropertyName("x")]` | `keep-alive` | `certain` | System.Text.Json accesses property via reflection |

**Controller-level propagation.** When `[Authorize]` appears on a controller class
rather than a method, the pack entry propagates the guard fact to all action methods
on that controller. Pack entries can declare `propagation = "all-methods"` to
express this.

### FW-3.4 — NestJS (TypeScript/Node.js)

NestJS uses TypeScript decorators for DI, routing, and guards. The DI system wires
constructor parameters at module startup — the canonical mediated-edge scenario
(FW-4).

**Entrypoints and routing:**

| Decorator | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@Controller("prefix")` | (metadata on class; enables route registration) | `certain` | Methods on the class are candidates for entrypoint registration |
| `@Get("path")`, `@Post("path")`, etc. | `entrypoint` | `certain` | HTTP method + path in provenance; composed with controller prefix |
| `@MessagePattern("pattern")` (Microservices) | `entrypoint` | `probable` | Microservice message handler; pattern in provenance |
| `@EventPattern("pattern")` | `entrypoint` | `probable` | Event handler |

**Guards:**

| Decorator | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@UseGuards(AuthGuard)` | `guard` | `probable` | Guard class resolved at runtime; `probable` |
| `@UseGuards(RolesGuard)` | `guard` | `probable` | Role guard |
| `@Roles("admin")` | `guard` | `certain` | Metadata for `RolesGuard`; provides the role constraint |

**DI wiring** (see also FW-4):

| Decorator | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `@Injectable()` | (makes the class available for injection) | `certain` | Class is registered as a provider |
| `@Inject(TOKEN)` | `interception` via DI | `probable` | Constructor dependency resolved by the NestJS IoC container; mediated edge (GM-17) |

### FW-3.5 — tokio and actix-web (Rust)

Rust async frameworks use procedural macro attributes. Most Rust metadata is
compile-time and fully statically verifiable — pack entries for Rust carry
`confidence = "certain"` in almost all cases.

**tokio:**

| Attribute | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `#[tokio::main]` | `entrypoint` | `certain` | Transforms `main` into an async runtime entry; added to entrypoint set with `kind = "async-main"` |
| `#[tokio::test]` | `entrypoint` | `certain` | Async test entrypoint; `kind = "test"` |

**actix-web:**

| Attribute | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `#[get("/path")]`, `#[post("/path")]`, `#[put("/path")]`, `#[delete("/path")]` | `entrypoint` | `certain` | HTTP handler; path and method in provenance |
| `#[route("/path", method = "GET", method = "POST")]` | `entrypoint` | `certain` | Multi-method handler |

**serde (keep-alive):**

| Attribute | Semantic class | Confidence | Resulting fact |
|---|---|---|---|
| `#[derive(Serialize, Deserialize)]` | `generated-member` | `certain` | Generated `serialize` and `deserialize` methods; these are valid call targets |
| `#[serde(rename = "x")]`, `#[serde(skip)]`, `#[serde(default)]` | `keep-alive` | `certain` | serde accesses this field; suppress dead-member finding |
| `#[serde(skip_serializing_if = "Option::is_none")]` | `keep-alive` | `certain` | Field accessed by generated serializer |

**Confidence note for Rust.** Rust procedural macros are evaluated at compile time by the compiler. Their effects are fully visible to `cgx` via the expanded AST (or via SCIP, which reflects the post-expansion graph). For this reason, pack entries for Rust attributes carry `certain` confidence except where the semantics depend on runtime registration (e.g. actix's `App::service` registration).

---

## FW-4: Mediated Edges — DI, Events, and Registries

**Status:** `schema-room` — the `established-by` provenance attribute and the
`via-DI` cut-marker are reserved in GM-17 (and GM-5.3); full DI-container
wiring resolution follows once the metadata schema (GM-15) is stable.

The constructs in this section produce **mediated call edges** (GM-17): `calls`
or `constructs` edges where the caller and callee are connected through a
container or registry, not a call expression. The edge carries an
`established-by` provenance field naming the mechanism that wired it.

### FW-4.1 — Dependency injection

DI frameworks resolve constructor or field injection at startup. The caller does
not write a call expression; the container calls the constructor or injects the
field.

| DI framework | Wiring mechanism | `cgx` model | Confidence |
|---|---|---|---|
| Spring (Java) | `@Autowired` on constructor / field / setter; `@Bean` factory method | `constructs` edge from the injection site to the registered implementation; `established-by: annotation` | `certain` for constructor injection (Dagger-style single registration); `probable` for field injection with multiple registered implementations |
| Guice (Java) | `@Inject` on constructor; `Module.bind(Interface.class).to(Impl.class)` | `constructs` edge; `established-by: config-file` for module bindings | `certain` for single binding; `probable` for conditional bindings |
| Dagger 2 (Java/Android) | `@Inject` + `@Component` + `@Module`/`@Provides` | Compile-time code generation; mediated edge at `certain` confidence (Dagger generates the wiring code statically) | `certain` |
| CDI (Jakarta EE) | `@Inject` | `constructs` edge; `established-by: annotation`; `probable` for qualifiers with multiple candidates | `probable` |
| NestJS (TypeScript) | `@Injectable()` + `@Module` providers array | `constructs` edge; `established-by: annotation`; module registration site in provenance | `probable` (runtime container resolution) |
| Angular (TypeScript) | `@Injectable()` + `providers` array or `providedIn: 'root'` | `constructs` edge; `established-by: annotation` | `probable` |

**Confidence rationale.** Compile-time DI (Dagger 2) generates concrete wiring
code; the injected implementation is statically fixed and the edge is `certain`.
Runtime containers (Spring, CDI, NestJS) resolve bindings at application startup
based on classpath scanning and configuration; the registration evidence is
present (annotation + classpath), making the edge `probable`. Where multiple
implementations are registered for an interface, the edge degrades to `possible`
and a `via-DI` cut-marker is emitted.

### FW-4.2 — Event systems

Event-emitter patterns decouple publisher from subscriber through a named event
type or topic. The publisher fires an event; the subscriber handles it. There is
no call from publisher to subscriber in the source code.

| Event system | Registration mechanism | `cgx` model | Confidence |
|---|---|---|---|
| Spring `ApplicationEvent` (Java) | `@EventListener` on handler method; `ApplicationEventPublisher.publishEvent(event)` | `calls` edge from `publishEvent` site to all `@EventListener` methods whose parameter type matches the event class | `probable` (parameter type match is certain; registration depends on component scanning) |
| Guava EventBus (Java) | `@Subscribe` on handler; `eventBus.post(event)` | `calls` edge from `post` site to `@Subscribe` handlers whose parameter type matches | `probable` |
| Node.js `EventEmitter` | `emitter.on("event", handler)` | `calls` edge from `emit("event")` site to registered handler; resolved when the string literal is a constant | `probable` for literal event names; `possible` for computed names |
| Go `init()` registration | `func init()` registers a handler in a package-level map | Config-file or registration-site pack entry | `probable` |
| Rust `tokio::sync::broadcast` channels | `tx.send(value)` / `rx.recv()` | Non-call dataflow edge (DF-20); send and receive are connected via the channel value's pedigree | `probable` |

### FW-4.3 — Route registries

Some frameworks register handlers in imperative code rather than via annotations.
Route registration sites are call expressions; the pack entry for the registry
function identifies the registration and extracts the handler argument.

| Framework | Registration | `cgx` model |
|---|---|---|
| Express (Node.js) | `app.get("/path", handler)` | Handler argument extracted as entrypoint with `kind = "http-handler"`; path in provenance |
| Gin (Go) | `router.GET("/path", handler)` | Handler argument extracted as entrypoint |
| Axum (Rust) | `.route("/path", get(handler))` | Handler function extracted as entrypoint; `certain` confidence because handler is a static Rust type |
| Go `http.HandleFunc` | `http.HandleFunc("/path", handler)` | Handler argument extracted as entrypoint |

Pack entries for registration patterns use a `registration_site` matcher rather
than an annotation matcher:

```toml
[[framework_packs.entries]]
language  = "go"
matcher   = { registration_site = { function = "net/http.HandleFunc", handler_arg = 1 } }
facts     = [{ class = "entrypoint", kind = "http-handler" }]
confidence = "certain"
evidence  = "registration-site"
```

---

## FW-5: Build-Configuration Variance

**Status:** `schema-room` — the `cfg-condition` attribute is reserved in GM-19
now; per-configuration graph indexing is `roadmap`; the attribute is available
for schema-reservation purposes so that queries can refer to it without a
migration once indexing follows.

Build configuration flags alter which symbols and edges are compiled into a
given build target. A function guarded by a Rust `#[cfg(feature = "legacy-auth")]`
attribute exists in some builds and not others. Ignoring this produces a call
graph that mixes edges from incompatible configurations.

### FW-5.1 — Per-language mechanisms

| Language | Mechanism | `cgx` attribute |
|---|---|---|
| Rust | `#[cfg(feature = "x")]`, `#[cfg(target_os = "linux")]`, `#[cfg(debug_assertions)]` | `cfg-condition: "feature = \"x\""` on the node or edge; sourced from `Cargo.toml` feature flags |
| C / C++ | `#ifdef FOO`, `#if defined(BAR)` | `cfg-condition: "ifdef FOO"` on nodes/edges in the conditionally compiled block |
| Go | Build tags: `//go:build linux && amd64` | `cfg-condition: "linux && amd64"` |
| Python | `if sys.version_info >= (3, 10):` / `if TYPE_CHECKING:` | `cfg-condition: "sys.version_info >= (3, 10)"` (heuristic; confidence `probable`) |
| Java | Maven profiles; Gradle build variants | `cfg-condition: "profile:production"` (sourced from build manifest) |
| C# | `#if DEBUG` / `#if NETCOREAPP3_1` | `cfg-condition: "DEBUG"` |

### FW-5.2 — Querying across configurations

A node or edge with a `cfg-condition` attribute exists only when that condition
is satisfied. Queries can:

- **Ignore configurations** (default): treat all cfg-conditioned nodes and edges
  as present. This is an over-approximation — it includes paths that exist only
  in specific builds.
- **Filter to a named configuration**: pass `--cfg "feature=legacy-auth"` to
  include only edges whose `cfg-condition` is satisfied by that configuration
  (or have no condition). This produces a per-configuration sub-graph.

Per-configuration indexing — building a separate graph for each combination of
flags — is `roadmap`. The `cfg-condition` attribute is reserved now so that
this query interface can be expressed without a schema migration.

### FW-5.3 — Security relevance

Some security-relevant paths exist only under specific build configurations:

- A Rust feature flag `cfg(feature = "debug-endpoints")` gates an unauthenticated
  admin endpoint. The endpoint is unreachable in production builds but present in
  development builds — the kind of configuration-specific vulnerability that static
  analysis tools miss when they ignore build flags.
- A C `#ifdef LEGACY_AUTH` block contains a weak authentication fallback.

The `cfg-condition` attribute surfaces these cases: a query for "all entrypoints
reachable without any guard" can report `cfg-condition` annotations alongside the
finding to flag "this path exists only in the `debug-endpoints` feature build."

---

## FW-6: Reflection Packs

**Status:** `schema-room` — string-literal-pedigree resolution (GM-18) is the
foundation; reflection packs are a user-facing extension of that mechanism.

Reflection and string-mediated dispatch — `Method.invoke`, `getattr`, Go
`reflect`, `Class.forName`, Ruby `send` — are structurally unresolvable in
general. GM-18 specifies two resolution strategies:

1. **Literal-pedigree resolution**: when the string reaching a reflection call
   site has a `certain`-confidence literal pedigree (DF-1), `cgx` resolves the
   call at `probable` confidence.
2. **Tainted-string dispatch**: when the string is tainted, the reflection call
   is itself a top-tier security finding — arbitrary method invocation via
   attacker-controlled string.

Reflection packs extend this by letting users declare the expected resolution
for reflection patterns that are structurally known in the codebase:

```toml
[[reflection_packs.entries]]
language = "java"
call_site = { function = "java.lang.reflect.Method.invoke", receiver_type = "java.lang.reflect.Method" }
resolution = { strategy = "literal-pedigree", confidence = "probable" }

[[reflection_packs.entries]]
language = "python"
call_site = { function = "getattr", arg_position = 1 }
resolution = { strategy = "literal-pedigree", confidence = "probable" }
```

When a reflection pack entry matches a call site and the string argument has a
literal pedigree, `cgx` emits a `calls` edge to the resolved symbol at the
declared confidence. The `reflective` cut-marker edge is still emitted alongside
it so that the unresolved case is not silently discarded.

Framework-specific reflection patterns — e.g. Spring's `BeanFactory.getBean("beanName")`,
JPA's `entityManager.find(MyEntity.class, id)`, Go's `reflect.TypeOf(x).Method(i)` —
can be declared as reflection pack entries. Built-in packs for Spring, JPA, and Go
`reflect` cover the common cases.

---

## Precision and Confidence: Honest Accounting

**Status:** applies to all sections above; no separate feature ID.

The confidence labels on pack-derived facts reflect what the resolution mechanism
can actually guarantee:

| Mechanism | Confidence | Rationale |
|---|---|---|
| Compile-time annotation with single unambiguous semantic (Rust `#[get("/")]`, Dagger 2 `@Component`, Spring `@GetMapping`) | `certain` | The annotation's meaning is fixed at compile time; the framework cannot change it at runtime |
| Spring / CDI / NestJS runtime container | `probable` | Registration evidence is present (annotation + classpath scan), but the container wires bindings at startup; a conditional `@Bean` may not be registered in all profiles |
| Event-listener type match | `probable` | The event type match is statically verifiable, but emission depends on runtime control flow at the `publishEvent` site |
| String-name-based registration (`EventBus`, Express `app.get("name", ...)` with computed path) | `possible` | The name must match at runtime; if it is not a literal, resolution falls back to `possible` |
| Reflection with tainted string | `possible` + security finding | Attacker-controlled method dispatch; emits both a `possible` edge and a tainted-reflection finding |
| Compile-time DI (Dagger 2) | `certain` | The wiring code is generated statically; the resolution is as reliable as a direct call |

The `probable` label on runtime-container edges is not a weakness; it is the
accurate characterisation. Security queries that ask "which paths can reach a
sensitive sink?" should include `probable` edges (the default). Dead-code queries
that ask "which symbols are never reached?" should treat `probable` edges as
reachable. The distinction matters only for questions where false positives carry
a high cost — at which point the user can elevate to `--min-confidence certain`
and accept the reduced coverage explicitly.
