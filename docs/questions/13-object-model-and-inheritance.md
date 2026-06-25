# Theme 13: Object Model and Inheritance

Object-model questions ask which class actually provides the body a polymorphic call lands on, how multiple-inheritance linearization determines that, how `super`/base-delegation calls bypass the normal override, and how an override can silently drift from the contract its base method established. `cgx` answers the structurally simpler forms today (which subclasses override a method; which virtual call has which candidate set) and reserves schema space for the richer forms — MRO-dependent resolution, default-body provenance, accessor shadowing, and override-contract drift detection — that require new graph entities.

---

### Q127 — What class actually implements the method this call resolves to?

**Personas:** SSE, ACA · **Status:** needs-schema-room-feature (GM-22)

When a virtual call site is bound at runtime to a concrete implementation, knowing which class provides the body is the first step in any polymorphic code review. For single-inheritance hierarchies this is derivable today from `inherits` and `overrides` edges, but a deterministic answer across multiple inheritance, mixins, and default methods requires the MRO stored by GM-22.

**The query**

```cgx
-- illustrative: requires GM-22 (schema-room)
MATCH (site {name:"render"})-[:RESOLVES_TO]->(body:method)-[:MEMBER_OF]->(t)
RETURN t.name, body.name, body.file, body.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(site {name:"render"})` | The call site whose resolution you are investigating — here by the name of the called symbol. |
| `-[:RESOLVES_TO]->` | The GM-22 derived edge from a call site to the method body the MRO selects for each candidate concrete type. Absent until GM-22 is populated. |
| `(body:method)-[:MEMBER_OF]->(t)` | Walk from the resolved body to the type that contains it. `t` is the class that actually provides the method. |
| `RETURN t.name, body.name, body.file, body.line` | Return the providing class, the resolved method name, and its source location. |

**Reading the result** — Each row names the concrete class whose method body this call site resolves to. Until GM-22 is populated, use Q128 to enumerate which subclasses have overrides and combine with `calls:virtual` `candidate_set` (Q136) to narrow manually.

---

### Q128 — Which subclasses override method (or property) X?

**Personas:** SSE · **Status:** answerable-today

Knowing which concrete classes override a base method is the prerequisite for any polymorphic impact analysis: which classes customise the behaviour, which rely on the inherited body. The `overrides` edge from GM-2.2 records this relation directly.

**The query**

```cgx
MATCH (sub:method)-[:OVERRIDES]->(base:method {name:"BaseHandler::handle"})
RETURN sub.name, sub.file, sub.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(sub:method)` | Any method node in the graph — the potential override. |
| `-[:OVERRIDES]->` | The structural override edge (GM-2.2): present when `sub` is declared to override `base` in the language's override mechanism. |
| `(base:method {name:"BaseHandler::handle"})` | The base method being overridden, matched by fully-qualified name. Replace with the method you are investigating. |
| `RETURN sub.name, sub.file, sub.line` | Return the overriding method's qualified name and source location. |

**Reading the result** — Each row is a class that supplies a non-inherited body for this method. A short list means most subclasses use the base implementation; a long list suggests widespread customisation that may need coordinated review.

---

### Q129 — Which overrides widen the exception contract their base method declared?

**Personas:** PSE, SSE · **Status:** needs-schema-room-feature (Q-32)

A base method's callers were written against the exception surface the base declared. An override that introduces new `throws`, `panic`, or `unwrap()` paths exposes those callers to failures they never anticipated. This is the exception-contract widening form of override-contract drift (Q-32), which composes `overrides` edges with `throws`/`panic` edge conditions and corrected Q-20 ∀-path semantics.

**The query**

```cgx
-- illustrative: requires Q-32 (core-extension; depends on corrected Q-20)
MATCH (sub:method)-[:OVERRIDES]->(base:method)
MATCH p_sub = (sub)-[:CALLS*]->(x)
WHERE ANY(r IN relationships(p_sub) WHERE r.condition IN ["exception","panic"])
  AND NONE(r IN relationships((base)-[:CALLS*]->(x)) WHERE r.condition IN ["exception","panic"])
RETURN sub.name, sub.file, sub.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(sub:method)-[:OVERRIDES]->(base:method)` | Bind each override/base pair in the `overrides` relation. |
| `MATCH p_sub = (sub)-[:CALLS*]->(x)` | Find any node `x` reachable from the override's body, binding the path. |
| `ANY(r IN relationships(p_sub) WHERE r.condition IN ["exception","panic"])` | At least one edge on that path is in the exceptional class — the override reaches `x` via an exception or panic path. |
| `NONE(r IN relationships((base)-[:CALLS*]->(x)) WHERE r.condition IN ["exception","panic"])` | No such exceptional edge exists on any path from the base to the same `x` — the base never exposed this exceptional surface. |
| `RETURN sub.name, sub.file, sub.line` | Return the override whose exceptional surface is a strict superset of the base's. |

**Reading the result** — Each returned method is an override that introduces exception or panic paths the base method did not have. Priority candidates are overrides that call `unwrap()`/`expect()` (panic edges) where the base returned a `Result`, or that throw checked exceptions the base never declared. Callers of the base may not handle these new failure modes.

---

### Q130 — Which overrides drop a guard the base class enforced on all paths to a sink?

**Personas:** PSE · **Status:** needs-schema-room-feature (Q-32)

If the base method routes every path to a sensitive sink through an authorisation or validation guard, that guard is part of the contract all callers rely on. An override that reaches the same sink on a path that avoids the guard has silently removed a protection. This is the dropped-guard form of override-contract drift (Q-32), applied as a per-override instance of corrected Q-20 ∀-path must-pass-through.

**The query**

```cgx
-- illustrative: requires Q-32 (core-extension; depends on corrected Q-20 ∀-path)
MATCH (sub:method)-[:OVERRIDES]->(base:method)
MATCH ALL (base)-[:CALLS*]->(sink {sink_class:"db-write"}) MUST PASS THROUGH (g {guard_class:"authz"})
MATCH ALL (sub)-[:CALLS*]->(sink {sink_class:"db-write"}) AVOIDING (g)
RETURN sub.name, sub.file, sub.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(sub:method)-[:OVERRIDES]->(base:method)` | Bind each override/base pair. |
| `MATCH ALL (base)-[:CALLS*]->(sink {sink_class:"db-write"}) MUST PASS THROUGH (g {guard_class:"authz"})` | Assert that every path from the base to a database-write sink passes through an authorisation guard node. The guard set is derived from the base method, not supplied by the user. |
| `MATCH ALL (sub)-[:CALLS*]->(sink {sink_class:"db-write"}) AVOIDING (g)` | Find cases where the override reaches the same sink class on a path that avoids that guard. An `AVOIDING` match produces results only when such a bypass path exists. |
| `RETURN sub.name, sub.file, sub.line` | Return overrides where a bypass path was found. |

**Reading the result** — This query is illustrative only; it exits 2 today. `MUST PASS THROUGH` and `AVOIDING` are parse errors in v0.3 (not recognised keywords), and `sink_class` / `guard_class` are plan errors (unsupported node properties). When Q-32 is implemented each returned row will be an override that has silently removed an authorisation check the base enforced.

---

### Q131 — Which calls bypass an override via `super` (delegate to an ancestor body)?

**Personas:** SSE, ACA · **Status:** needs-schema-room-feature (GM-21)

A `super` call statically and non-virtually delegates to a named ancestor's method body, deliberately bypassing the override that virtual dispatch would otherwise select. Without a distinct edge kind, these calls are indistinguishable from ordinary virtual calls and pollute candidate sets. GM-21 adds the `calls:super` edge kind with a single resolved target and a `bypassed_override` attribute.

**The query**

```cgx
-- illustrative: requires GM-21 (core-extension)
MATCH (caller)-[s:CALLS:super]->(ancestor:method)
RETURN caller.name, ancestor.name, s.bypassed_override, caller.file, caller.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(caller)` | Any symbol that contains a `super` call. |
| `[s:CALLS:super]` | The GM-21 edge kind for statically-bound ancestor delegation. Unlike `CALLS:virtual`, this edge has a single resolved target. |
| `(ancestor:method)` | The ancestor method body the call resolves to. |
| `s.bypassed_override` | The FQN of the override that virtual dispatch would have selected for the receiver's static type — the method being bypassed. |
| `RETURN caller.name, ancestor.name, s.bypassed_override, caller.file, caller.line` | Return the calling context, the ancestor body invoked, and the bypassed override. |

**Reading the result** — Each row is a call site where a class explicitly delegates to an ancestor, skipping its own override or a sibling's. The `bypassed_override` column identifies what the caller chose not to invoke — useful for auditing whether the bypass is intentional and safe.

---

### Q132 — When a class doesn't override an interface/trait method, which default body does a call resolve to?

**Personas:** SSE, ACA · **Status:** needs-schema-room-feature (GM-23)

When a concrete class implements an interface or trait but does not override a method that has a default body, calls on instances of that class resolve to the interface or trait's default implementation. Today `implements` records the relation but there is no fact identifying the body provider for the not-overridden case. GM-23 adds a `provides-body` derived edge for exactly this mapping.

**The query**

```cgx
-- illustrative: requires GM-23 (schema-room)
MATCH (t)-[:IMPLEMENTS]->(iface)
MATCH (t)-[:PROVIDES_BODY {method:"serialize"}]->(body:method)-[:MEMBER_OF]->(iface)
RETURN t.name, body.name, body.file, body.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(t)-[:IMPLEMENTS]->(iface)` | Find each concrete type and the interface it implements. |
| `(t)-[:PROVIDES_BODY {method:"serialize"}]->` | The GM-23 derived edge recording the body provider for the named method when `t` does not override it. |
| `(body:method)-[:MEMBER_OF]->(iface)` | The body lives in the interface or trait — confirming it is a default, not a class-level override. |
| `RETURN t.name, body.name, body.file, body.line` | Return the concrete type, the default body it inherits, and where that body is defined. |

**Reading the result** — Each row identifies a concrete type that silently inherits a default method body from an interface or trait. This matters when the default body contains business logic or security checks that a future override might need to preserve or delegate to.

---

### Q133 — Which concrete types leave an abstract method unfulfilled (would fail to compile / instantiate)?

**Personas:** SSE · **Status:** needs-schema-room-feature (GM-25)

A concrete type that does not supply an implementation for every abstract method declared in its supertypes is a compile error (Java, C#, Kotlin, Rust) or a runtime instantiation failure (Python ABC). GM-25 exposes the `unfulfilled_abstract` derived predicate on type nodes, making these violations directly queryable rather than requiring a manual hierarchy walk.

**The query**

```cgx
-- illustrative: requires GM-25 (core-extension)
MATCH (t {kind:"type"})
WHERE t.is_abstract = false AND t.unfulfilled_abstract IS NOT NULL
RETURN t.name, t.unfulfilled_abstract, t.file, t.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(t {kind:"type"})` | Any type node in the graph. |
| `t.is_abstract = false` | Restrict to concrete (non-abstract) types — abstract types are not expected to fulfill all abstract members. |
| `t.unfulfilled_abstract IS NOT NULL` | The GM-25 derived predicate: the type has at least one abstract member with no fulfilling override reachable through its MRO. |
| `RETURN t.name, t.unfulfilled_abstract, t.file, t.line` | Return the type and the list of unfulfilled abstract members. |

**Reading the result** — Each returned type has abstract methods that no concrete body satisfies. In statically-typed languages these indicate code that cannot currently compile; in Python they indicate types that will raise `TypeError` on instantiation. `unfulfilled_abstract` lists the specific abstract declarations missing implementations.

---

### Q134 — Which concrete method fulfills this abstract declaration, per concrete subtype?

**Personas:** SSE, ACA · **Status:** needs-schema-room-feature (GM-25)

Given an abstract method declaration, finding which concrete method in each subclass actually satisfies it maps the design contract to its implementations. GM-25 adds the `fulfills` edge from each concrete method to the abstract declaration it satisfies, making this a direct edge lookup rather than a hierarchy-traversal inference.

**The query**

```cgx
-- illustrative: requires GM-25 (core-extension)
MATCH (impl:method)-[:FULFILLS]->(decl:method {is_abstract:true, name:"Storage::put"})
RETURN impl.name, impl.file, impl.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(impl:method)` | Any concrete method in the graph — the candidate fulfillment. |
| `-[:FULFILLS]->` | The GM-25 derived edge from a concrete method to the abstract declaration it satisfies. Restricted to an `is_abstract = true` target. |
| `(decl:method {is_abstract:true, name:"Storage::put"})` | The abstract method declaration being investigated, matched by name. |
| `RETURN impl.name, impl.file, impl.line` | Return each concrete fulfillment and its source location. |

**Reading the result** — Each row is a class that supplies a concrete body for the named abstract declaration. A missing row for a class that claims to implement the interface indicates an unfulfilled abstract (see Q133). Compare fulfillments to detect divergence in behaviour across implementations.

---

### Q135 — Which subclass property/accessor shadows a parent field or accessor (changing read/write semantics)?

**Personas:** SSE · **Status:** needs-schema-room-feature (GM-24)

When a subclass defines a getter, setter, or property that shadows a parent field or accessor, reads and writes to that attribute no longer go directly to the parent field — they are intercepted by the subclass accessor. This can change semantics silently. GM-24 extends `overrides` to accessor symbols and adds a `shadows-field` edge from the subclass accessor to the shadowed parent field.

**The query**

```cgx
-- illustrative: requires GM-24 (schema-room)
MATCH (acc:method)-[:SHADOWS_FIELD]->(parent_field)
RETURN acc.name, parent_field.name, acc.file, acc.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(acc:method)` | A subclass getter, setter, or property method. |
| `-[:SHADOWS_FIELD]->` | The GM-24 derived edge from the accessor to the parent field or accessor it shadows. |
| `(parent_field)` | The field or accessor in the parent class that is being shadowed. |
| `RETURN acc.name, parent_field.name, acc.file, acc.line` | Return the shadowing accessor and the parent field it intercepts. |

**Reading the result** — Each row is a subclass accessor that interposes on field access. The parent's `reads-field` / `writes-field` edges no longer fully describe the read/write footprint for instances of the subclass. Review whether the subclass accessor preserves the parent's invariants.

---

### Q136 — For this polymorphic call, what is the full set of bodies it could resolve to across instantiated subtypes?

**Personas:** PSE, SSE · **Status:** needs-schema-room-feature (GM-2.1)

Before a virtual call is analysed for security or correctness, the reviewer needs the complete set of possible runtime targets. The `CALLS:virtual` subtype qualifier and the `candidate_set` edge attribute that would carry the CHA/RTA-narrowed target set are recognised by the parser but deferred to v0.3 — using them today produces a plan error (exit 2). The query below is illustrative of the intended form; run it as-is and cgx returns exit 2.

**The query**

```cgx
-- illustrative: requires GM-2.1 (virtual dispatch edge kind + candidate_set attribute, deferred)
MATCH (site)-[c:CALLS:virtual]->(target {name:"Codec::decode"})
RETURN site.name, site.candidate_set, site.file, site.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(site)` | The call site where the virtual call originates. |
| `[c:CALLS:virtual]` | The virtual dispatch edge kind (GM-2.1). Unlike `CALLS`, this edge represents dispatch through a virtual, interface, or trait method. Deferred: exits 2 in v0.3. |
| `(target {name:"Codec::decode"})` | The declared target of the virtual call, matched by its declared type and method name. |
| `site.candidate_set` | The GM-2.1 attribute on the edge carrying all possible runtime targets, ordered by probability. Absent until GM-2.1 is populated. |
| `RETURN site.name, site.candidate_set, site.file, site.line` | Return the call site and the full set of bodies it may invoke. |

**Reading the result** — Until GM-2.1 is populated, use Q128 to enumerate which subclasses override the method, then use `cgx callers` to find call sites that reach those overrides. `candidate_set` will list possible runtime implementations in probability order once the virtual dispatch edge kind is active; entries with `certain` or `probable` confidence from CHA/RTA narrowing are high-priority candidates.

---

### Q137 — Which overrides change the receiver's field-write footprint relative to the base (write a field the base did not, or stop writing one it did)?

**Personas:** SSE, ACA · **Status:** needs-schema-room-feature (Q-32)

An override that mutates fields the base method never touched, or stops mutating fields the base always wrote, changes the object's state contract. This is a field-footprint-drift form of override-contract drift (Q-32): it compares the `writes-field` sets of an override and its base method, using `writes-field` edges from GM-2.2.

**The query**

```cgx
-- illustrative: requires Q-32 (core-extension)
MATCH (sub:method)-[:OVERRIDES]->(base:method)
MATCH (sub)-[:WRITES_FIELD]->(f)
WHERE NONE(bf IN [x IN (base)-[:WRITES_FIELD]->() | x] WHERE bf = f)
RETURN sub.name, f.name, sub.file, sub.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(sub:method)-[:OVERRIDES]->(base:method)` | Bind each override/base pair. |
| `MATCH (sub)-[:WRITES_FIELD]->(f)` | Find each field that the override writes directly. |
| `NONE(bf IN [x IN (base)-[:WRITES_FIELD]->() | x] WHERE bf = f)` | The base method does not write field `f` — so this is a new field write the override introduced. |
| `RETURN sub.name, f.name, sub.file, sub.line` | Return the override and the newly-written field. |

**Reading the result** — Each row is a field that an override writes but the base method did not. This may indicate the override is taking on additional state responsibilities, or may reveal an unintended mutation. The inverse (fields the base writes but the override drops) requires a symmetric query with `sub` and `base` swapped.

---

### Q138 — In a diamond / multiple-inheritance hierarchy, which mixin or trait actually wins for method M, and does that differ from the naive nearest-base guess?

**Personas:** PSE · **Status:** needs-schema-room-feature (GM-22)

In a diamond inheritance or mixin/trait hierarchy, the naive "nearest base" rule gives the wrong answer — two bases are equidistant, and the language's linearization algorithm determines the winner. Without the MRO stored by GM-22, the resolved body is indeterminate from the graph alone.

**The query**

```cgx
-- illustrative: requires GM-22 (schema-room)
MATCH (t {name:"Worker"})-[:RESOLVES_TO {method:"run"}]->(winner:method)-[:MEMBER_OF]->(provider)
RETURN provider.name, winner.file, winner.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(t {name:"Worker"})` | The concrete type whose diamond/mixin hierarchy you are investigating. |
| `-[:RESOLVES_TO {method:"run"}]->` | The GM-22 derived edge from the type to the body the MRO selects for the named method. The `method` property on the edge identifies which method's resolution is being retrieved. |
| `(winner:method)-[:MEMBER_OF]->(provider)` | Walk from the winning body to the type that provides it — the linearization winner. |
| `RETURN provider.name, winner.file, winner.line` | Return the winning provider class and where its method body is defined. |

**Reading the result** — The `provider.name` column identifies which mixin or trait the language's linearization algorithm selected. Compare this against your expectation from the inheritance diagram to confirm the right implementation is active. A mismatch between `provider` and the "nearest base" in the source hierarchy indicates linearization ordering matters here and the naive reading is wrong.
