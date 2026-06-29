# Recipe: Object Model & Inheritance

**Theme 13 of 13** — Questions about which class provides a method body, how MRO
resolves dispatch, where `super` delegates, and whether an override drifts from
its base contract.

**Version posture: mostly deferred past v0.3.** Only two questions are runnable
today. The edge types that power the richer analysis — `RESOLVES_TO`, `PROVIDES_BODY`,
`SHADOWS_FIELD`, `FULFILLS`, and `CALLS:super` — are still deferred in v0.3 and
produce exit 2 (`plan error: … is not supported in this release (Theme-13, deferred)`).

Before running any command: `cgx --version` → `cgx 0.<MINOR>.<PATCH>`.
A capability tagged `Since: v0.N` requires `MINOR ≥ N`.
Full version ladder: see `reference/versions.md`.
CQL dialect rules: see `reference/query-language.md`.
Graph vocabulary (edge-condition labels, confidence ladder, transience):
see `reference/mental-model.md`.

**Step 0 always applies:** use `cgx search` to find the exact fully-qualified name
before passing it to any command. A wrong name exits 2 with `no symbol matched '<x>'`.

```bash
# Find the exact FQN for a method before querying
cgx search "handle" --repo /path/to/repo --kind method
cgx search "BaseHandler" --repo /path/to/repo
```

---

## Partial v0.1 — runnable today with caveats

### Q128 — Which subclasses override method X?

**Status:** partial v0.1 (OVERRIDES edge populated syntactically; candidate set
narrows at v0.2)  **Since: v0.1**  **Personas:** SSE

The `OVERRIDES` edge (GM-2.2) is present in the v0.1 schema for languages where
the override relationship is syntactically declared (`override`, `virtual`,
`@Override`, etc.). It does not yet require MRO resolution.

```bash
# Step 0: find the exact FQN for the base method
cgx search "handle" --repo /path/to/repo --kind method
# → MyPackage::BaseHandler::handle   src/handler.rs:12  [method]

# Query override set — use WHERE base.fqn for exact match; inline {name:} returns empty
cgx query 'MATCH (sub)-[:OVERRIDES]->(base) WHERE base.fqn = "MyPackage::BaseHandler::handle"
RETURN sub.name, sub.file, sub.line' \
  --repo /path/to/repo
```

**Since: v0.1**

**Why this works:** The `OVERRIDES` edge is written during syntactic analysis when
a language keyword or annotation marks the declaration as an override. No MRO
traversal is required to record the relation.

**Reading the result:** Each row is a class that supplies a non-inherited body for
this method. A short list means most subclasses use the base implementation; a
long list suggests widespread customisation.

Caveats:
- Use `WHERE base.fqn = "..."` for unambiguous exact-match filtering. Both `.name`
  and `.fqn` store the full FQN, so `WHERE base.name = "speak"` returns empty
  (the short name is not equal to the full FQN string). Inline property filters
  `{name:"speak"}` match the last FQN component as a short-name lookup and may
  return multiple symbols with the same short name across types; anchor to the full
  FQN with `WHERE base.fqn = "..."` to avoid false matches. See
  `reference/query-language.md`.
- `OVERRIDES` coverage is limited to languages with explicit override syntax.
  Implicit overrides (e.g., Python methods that shadow without a decorator) may
  not appear until a future enrichment pass.
- `candidate_set` discrimination on virtual calls requires the `CALLS:virtual`
  edge sub-type and CHA/RTA narrowing, both **deferred past v0.3** (exit 2). At v0.3,
  confidence values are predominantly `possible` (over-approximated). See Q136.
- Always bound any hop variable in follow-on queries: `CALLS*2`, never bare `CALLS*`
  (hangs). See `reference/query-language.md`.

---

### Q136 — For this polymorphic call, what is the full candidate set it could dispatch to?

**Status:** spec-only (DEMOTED — still exits 2 in v0.3)  **Since: deferred past v0.3**  **Personas:** PSE, SSE

The `CALLS:virtual` sub-type qualifier is **deferred in v0.3** (Theme-13): it errors with
`sub-type qualifier 'CALLS:…' … recognised but not supported in this release (deferred)` (exit 2).
The `candidate_set` node property is also unknown in v0.3 (exit 2).
The query below is a **future form**; do not emit it on any v0.3 binary.

```cypher
-- Deferred past v0.3 — CALLS:virtual qualifier + candidate_set both exit 2 today
MATCH (site)-[c:CALLS:virtual]->(target {name:"Codec::decode"})
RETURN site.name, site.candidate_set, site.file, site.line
```

**Why this is deferred:** `CALLS:virtual` is a deferred edge sub-type; CHA/RTA narrowing that populates and
promotes the `candidate_set` attribute is not shipped in v0.3. Neither the qualifier nor the
property exists in the current graph schema.

**v0.3 fallback:** to approximate the candidate set, run `cgx callees` from the call site's enclosing
function and inspect the `possible`-confidence call edges, or use Q128's `OVERRIDES` enumeration to list
the implementing methods of the dispatched-on type.

---

## Deferred past v0.3 — documented design, not yet runnable

The following questions require edge types (`RESOLVES_TO`, `PROVIDES_BODY`,
`SHADOWS_FIELD`, `FULFILLS`) or the `CALLS:super` edge kind that are deferred in
the v0.3 binary. Emitting them produces exit 2
(`plan error: … is not supported in this release (Theme-13, deferred)`).

Do not present these as runnable on any current binary.

| Question | Required feature | Query form (illustrative only — exits 2 today) |
|---|---|---|
| **Q127** — Which class actually implements the method a virtual call resolves to? | GM-22 (`RESOLVES_TO` edge, MRO) | `MATCH (site)-[:RESOLVES_TO]->(body:method)-[:MEMBER_OF]->(t) WHERE site.fqn = "render" RETURN t.name, body.name, body.file, body.line` |
| **Q131** — Which calls bypass an override via `super`? | GM-21 (`CALLS:super` edge) | `MATCH (caller)-[s:CALLS:super]->(ancestor:method) RETURN caller.name, ancestor.name, s.bypassed_override, caller.file, caller.line` |
| **Q132** — When a class doesn't override a trait/interface method, which default body does it resolve to? | GM-23 (`PROVIDES_BODY` edge) | `MATCH (t)-[:IMPLEMENTS]->(iface) MATCH (t)-[:PROVIDES_BODY]->(body:method)-[:MEMBER_OF]->(iface) WHERE body.name = "serialize" RETURN t.name, body.name, body.file, body.line` |
| **Q133** — Which concrete types leave an abstract method unfulfilled? | GM-25 (`unfulfilled_abstract` predicate; `IS NULL` predicates also deferred) | `MATCH (t)-[:OVERRIDES]->(base) WHERE t.is_abstract = false AND t.unfulfilled_abstract IS NOT NULL RETURN t.name, t.unfulfilled_abstract, t.file, t.line` |
| **Q134** — Which concrete method fulfills this abstract declaration, per subtype? | GM-25 (`FULFILLS` edge) | `MATCH (impl:method)-[:FULFILLS]->(decl:method) WHERE decl.is_abstract = true AND decl.name = "Storage::put" RETURN impl.name, impl.file, impl.line` |
| **Q135** — Which subclass accessor shadows a parent field? | GM-24 (`SHADOWS_FIELD` edge) | `MATCH (acc:method)-[:SHADOWS_FIELD]->(parent_field) RETURN acc.name, parent_field.name, acc.file, acc.line` |
| **Q138** — In a diamond/multiple-inheritance hierarchy, which mixin wins for method M? | GM-22 (`RESOLVES_TO` + MRO linearization) | `MATCH (t)-[:RESOLVES_TO]->(winner:method)-[:MEMBER_OF]->(provider) WHERE t.name = "Worker" AND winner.name = "run" RETURN provider.name, winner.file, winner.line` |

The following questions require **override-contract drift** analysis (Q-32),
which composes `OVERRIDES` edges with `MUST PASS THROUGH`/`AVOIDING` path-set
algebra. Both clauses are parse errors in v0.3 (exit 2, not plan errors — the
keywords are not recognised by the parser).

| Question | Required feature |
|---|---|
| **Q129** — Which overrides widen the exception contract their base method declared? | Q-32 + corrected Q-20 ∀-path |
| **Q130** — Which overrides drop a guard the base enforced on all paths to a sink? | Q-32 + `MUST PASS THROUGH`/`AVOIDING` (parse error exit 2 today) |
| **Q137** — Which overrides change the field-write footprint relative to the base? | Q-32 (`WRITES_FIELD` comparison) |

---

## What you can do today

When the deferred Theme-13 features are not available, combine the runnable
commands to partially answer the most common polymorphic questions:

**"Which classes provide an implementation?"** → Q128 (above). Enumerate
overrides directly via `OVERRIDES`. For classes with no override row, the base body applies.

**"Can a call reach a sensitive function through any implementation?"** →
Use `cgx reaches` or `cgx paths` with the base method as the start point. This
answers reachability over call paths, not dispatch resolution. See
`recipes/reachability.md`.

**"What is the blast radius of changing this method?"** → Use `cgx callers`
on the base and on each override returned by Q128. See `recipes/impact.md`.

**"Are there failure paths through any override?"** → Use Q128 to enumerate
overrides, then apply failure-path analysis to each. See
`recipes/failure-paths.md`.

Note: `cgx reaches` / `cgx paths` answer call-path reachability, not data-flow
or dispatch semantics. An answer of "reaches" does not mean "data flows" or
"this implementation is selected." See `reference/mental-model.md`.
