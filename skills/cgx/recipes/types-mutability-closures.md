# Recipe: Types, Mutability, and Closures

**Theme 11 of 13** — Type reconstruction, mutation fan-out, capture edges, coercion.

**Not answerable in v0.1; requires cgx >= 0.3.**

Run `cgx --version` before using this recipe. If `MINOR < 3`, none of these
queries run. See `reference/versions.md` for the full version ladder.

The enabling features (DF-17 mutability model, DF-18 function values and closures,
DF-19 lineage type reconstruction, DF-10 coercion transformation kind, GM-14.6
type-confidence boundaries) are all `schema-room` work scheduled for v0.3. The
CALLS-graph CQL subset available in v0.1 does not model DATA_FLOW edges, capture
edges, effect summaries, or type-confidence attributes. Attempting any query below
on a v0.1 binary will exit 2 with a plan error or return empty.

**Step 0 — find the exact symbol name first.**
cgx has no search, glob, or fuzzy match. An unknown symbol exits 2 with
`no symbol matched '<x>'`. Grep/ripgrep the source for the fully-qualified name
before running any cgx command.

---

## Documented v0.3 forms (not runnable today)

All entries below are `Since: v0.3`. Gate every invocation on `cgx --version`:

```bash
cgx --version   # look at MINOR; proceed only if MINOR >= 3
```

The canonical CQL patterns use double-quoted string literals inside a
single-quoted shell argument. Wrap all cgx query invocations as shown.

---

### What concrete types can this `interface{}`-typed value actually hold? (Q109)

**Status:** spec-only (DF-19 type reconstruction)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: cgx.type_reconstruct procedure (DF-19)
MATCH (v {name: "v", scope: "process"})
CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
RETURN candidate_type, confidence, evidence
ORDER BY confidence DESC
```

**Why this works:** `cgx.type_reconstruct` gathers constraints from three
directions: upstream constructors (pedigree), downstream parameter sinks (usage),
and sideways join arms or channel pairs. The constraint intersection yields the
candidate type set.

**Reading the result:** `certain` means a constructor anchors the type upstream.
`probable` means 2–3 types satisfy the usage footprint. An empty candidate set
means the value is used in contradictory ways (see Q110 below). `possible`
candidates are structural footprint matches — treat them as hypotheses. The
`evidence` column names the specific constraints that drove each candidate.

---

### Which values are used in two incompatible ways — a type contradiction? (Q110)

**Status:** spec-only (DF-19 type reconstruction, `IS EMPTY` and `COMPATIBLE_WITH` predicates)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: cgx.type_reconstruct procedure (DF-19)
MATCH (v)
WHERE v.declared_type IS NOT NULL
CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
WHERE candidate_type IS EMPTY
   OR (confidence IN ["certain", "probable"]
       AND NOT candidate_type COMPATIBLE_WITH v.declared_type)
RETURN v.name, v.file, v.line,
       v.declared_type,
       candidate_type,
       evidence
ORDER BY v.file, v.line
```

**Why this works:** `candidate_type IS EMPTY` fires when no single type satisfies
all usage constraints simultaneously. `NOT candidate_type COMPATIBLE_WITH v.declared_type`
catches cases where reconstruction and declaration diverge without an empty set.

**Reading the result:** Each row is a value used in incompatible ways. The
`evidence` column identifies the conflicting constraints (which specific
constructor or call site drives each constraint). A contradiction against
`declared_type` is likely a bug; an empty candidate set with no declared type
means two call sites treat the same value as different types entirely.

---

### Which callees can mutate this validated value before it reaches the auth check? (Q111)

**Status:** spec-only (DF-17 mutability model, `writes-param` / `writes-receiver` effect summaries)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: cgx.mutation_fanout procedure (DF-17)
MATCH (v {name: "user_record", scope: "AuthHandler::validate"})
CALL cgx.mutation_fanout(v) YIELD mutator, effect, confidence
RETURN mutator.name, mutator.file, mutator.line,
       effect,
       confidence
ORDER BY confidence DESC, mutator.file
```

**Why this works:** `cgx.mutation_fanout` is the outbound dual of pedigree: from
a given program point it traverses alias edges (DF-16) and effect summaries (DF-17)
to find every function that can write to the value between that point and a
downstream check.

**Reading the result:** Focus on `certain` or `probable` mutators with effect
`writes-param(0)` or `writes-receiver` — those directly overwrite the value through
a parameter or receiver reference. A long list of potential mutators signals the
validated value is passed to too many callees before the authorization check.

---

### Which getters return a mutable reference to an internal field? (Q112)

**Status:** spec-only (DF-17 mutability model, `return_is_internal_field` and `return_mutability` node attributes)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: DF-17 attributes: return_mutability, return_is_internal_field
MATCH (getter:method)-[:MEMBER_OF]->(t)
WHERE getter.name STARTS WITH "get"
  AND getter.return_mutability = "mutable"
  AND getter.return_is_internal_field = true
RETURN getter.name, getter.file, getter.line,
       t.name AS containing_type
ORDER BY t.name, getter.name
```

**Why this works:** DF-17 analysis sets `return_is_internal_field = true` when
the returned value's pedigree traces to a field of `self`/`this` without a
defensive copy. `return_mutability = "mutable"` restricts to cases where the
caller receives write access.

**Reading the result:** Each row names a getter that exposes object internals to
callers. The `containing_type` column identifies the type with the exposure.
To find callers that actually mutate through the returned reference, extend with
a `MATCH (caller)-[:CALLS]->(getter)` arm and `cgx.mutation_fanout(getter)`.

---

### Can a callee mutate a sanitized value through an alias, re-introducing taint? (Q113)

**Status:** spec-only (DF-17 sanitization-invalidation via alias, DF-11 taint labels)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: DATA_FLOW edges (DF-11), taint_label.re_applied attribute (DF-17.4)
MATCH path = (sanitizer)-[:DATA_FLOW*2]->(use {sink_class: "sql"})
WHERE sanitizer.sanitizer_class = "sql"
  AND ANY(edge IN relationships(path)
          WHERE edge.taint_label.re_applied = true
            AND edge.taint_label.reason = "mutation-after-sanitization")
RETURN sanitizer.name, sanitizer.file, sanitizer.line,
       use.name, use.file, use.line
```

**Why this works:** Sanitizing a value clears its taint label, but if a callee
holds an alias to the same memory and overwrites it after sanitization, the taint
re-enters the path. DF-17 detects this and sets `taint_label.re_applied = true`
with `reason = "mutation-after-sanitization"` on the relevant DATA_FLOW edge.

**Reading the result:** Each row is a sanitize-then-mutate-via-alias pattern that
reaches a SQL sink. `sanitizer.file`/`line` is where sanitization occurred;
`use.file`/`line` is the SQL sink. Examine the intermediate call chain to locate
the alias-holding callee that performed the mutation.

---

### Which closures capture a loop variable by reference — late-binding bug? (Q114)

**Status:** spec-only (DF-18 capture edges, `by-ref` × binding mutability attributes)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: CAPTURE edges with by-ref and binding mutability (DF-18)
MATCH (loop_var)-[cap:CAPTURE {capture: "by-ref"}]->(closure)
WHERE loop_var.binding_mutability = "mutable"
  AND loop_var.is_loop_variable = true
RETURN loop_var.name, loop_var.file, loop_var.line,
       closure.name, closure.file, closure.line,
       cap.capture AS capture_mode
ORDER BY loop_var.file, loop_var.line
```

**Why this works:** DF-18 records a typed `CAPTURE` edge from each captured
variable to its closure. Filtering on `capture:"by-ref"` and
`binding_mutability:"mutable"` and `is_loop_variable:true` isolates the specific
pattern that produces the classic late-binding bug: the closure sees the final
loop value, not the value at capture time.

**Reading the result:** Each row is a closure with a by-ref capture of a mutable
loop variable. `loop_var.file`/`line` locates the loop variable;
`closure.file`/`line` locates the closure definition. The fix is a local copy
of the loop variable inside the loop body captured by value. This pattern covers
Go pre-1.22 goroutine closures, JavaScript `var`-in-loop callbacks, and Python
late-binding lambdas through the same graph shape.

---

### Which closures capture a file handle, lock guard, or db connection — resource lifetime extended? (Q115)

**Status:** spec-only (DF-18 capture edges, GM-13 resource lifecycle pairs)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: CAPTURE edges (DF-18), resource_class attribute (GM-13)
MATCH (resource)-[cap:CAPTURE]->(closure)
WHERE resource.resource_class IN ["lock", "file-handle", "db-connection"]
RETURN resource.name, resource.file, resource.line,
       closure.name, closure.file, closure.line,
       cap.capture AS capture_mode
ORDER BY resource.resource_class, resource.file, resource.line
```

**Why this works:** GM-13 annotates nodes whose types carry acquire/release
lifecycle semantics with a `resource_class` label. DF-18 then records a `CAPTURE`
edge when such a resource is captured inside a closure, extending its lifetime
to match the closure's execution context.

**Reading the result:** `capture_mode:"by-ref"` means the closure holds a
reference; the resource must outlive the closure. `capture_mode:"by-value"` means
ownership was moved into the closure and will be released when the closure drops.
Focus on `by-ref` captures of lock guards — those extend lock hold time to the
closure's lifetime rather than the enclosing scope's exit.

---

### What concrete functions can flow to this indirect call site? (Q116)

**Status:** spec-only (DF-18 function-value pedigree, indirect-call resolution)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: CALLS:indirect edges from function-value pedigree (DF-18)
MATCH (callsite)-[:CALLS:indirect]->(target)
WHERE callsite.scope = "process"
  AND callsite.arg_position = 0
RETURN target.name, target.file, target.line,
       callsite.confidence AS resolution_confidence
ORDER BY callsite.confidence DESC, target.name
```

**Why this works:** When a call site invokes a function value rather than a named
function, the CALLS graph has a gap. DF-18 fills it by tracing the pedigree of the
function value to its constructors and recording `CALLS:indirect` edges with per-candidate
confidence.

**Reading the result:** `resolution_confidence:"certain"` means only one concrete
function can ever flow to this call site. `"probable"` means a branch merge or
container store introduces ambiguity. `"possible"` reflects an opaque boundary
or FFI — these may include false positives. No v0.1 mechanism surfaces this;
v0.1 CALLS edges only represent syntactically-direct calls.

---

### Does a tainted value reach an auth check through a type coercion — type-juggling bypass? (Q117)

**Status:** spec-only (DF-10 `coerce` transformation kind, DATA_FLOW edges)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: DATA_FLOW edges (DF-11), coerce transformation_kind (DF-10)
MATCH path = (src)-[:DATA_FLOW*2]->(coerce_site)-[:DATA_FLOW {transformation_kind: "coerce"}]
             ->(sink {sink_class: "auth-check"})
WHERE src.source_class IN ["network", "user-input"]
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "auth-check")
RETURN src.name, src.file, src.line,
       coerce_site.name, coerce_site.file, coerce_site.line,
       [e IN relationships(path) | e.transformation_kind] AS transformations,
       sink.name, sink.file, sink.line
ORDER BY src.file, src.line
```

**Why this works:** DF-10 labels DATA_FLOW edges with a `transformation_kind`
attribute, including `"coerce"` for type conversions. Filtering to paths where
at least one edge carries `"coerce"` and the path ends at an `auth-check` sink
isolates the type-juggling-in-auth pattern (e.g., PHP `"0e123" == "0"`).

**Reading the result:** `coerce_site.file`/`line` is where the coercion occurs —
the location to harden with strict equality or explicit type casting. The
`transformations` list shows every type change on the path, not just the coercion,
which helps triage whether coercion is incidental or structural.

Note: the cookbook shows a Layer-1 shorthand using `--from-class` and
`--require-transformation` flags. Those flags do not exist in any version of the
binary (see `reference/cli.md`). Use the CQL form above.

---

### Where does the `any`/`interface{}`/unannotated-parameter frontier begin? (Q118)

**Status:** spec-only (GM-14.6 type-confidence boundary attribute)   **Since:** v0.3

```cypher
-- Gate: cgx --version MINOR >= 3
-- Requires: type_confidence_boundary attribute (GM-14.6)
MATCH (typed_src)-[r:DATA_FLOW]->(boundary)
WHERE typed_src.declared_type IS NOT NULL
  AND typed_src.declared_type <> "any"
  AND typed_src.declared_type <> "interface{}"
  AND boundary.type_confidence_boundary = true
RETURN typed_src.name, typed_src.file, typed_src.line,
       boundary.name, boundary.file, boundary.line,
       r.transformation_kind AS crossing_kind
ORDER BY typed_src.file, typed_src.line
```

**Why this works:** GM-14.6 marks nodes as `type_confidence_boundary = true`
at the point where static type guarantees lose precision — the `any`-frontier.
The query finds every crossing from fully-typed territory into untyped territory.

**Reading the result:** Each row is a frontier crossing. `boundary.file`/`line`
is the first location where the static type system stops enforcing guarantees.
`crossing_kind:"identity"` means a bare type assertion, `"coerce"` an explicit
cast, `"parsed"` a deserialization boundary. A high concentration of crossings
in security-critical paths (auth decisions, DB writes) signals the codebase
relies on runtime type correctness rather than compile-time enforcement.

---

## Summary: all questions in this theme are Since: v0.3

| Q | Question | Feature | Since |
|---|----------|---------|-------|
| Q109 | Candidate type set for `interface{}`-typed value | DF-19 type reconstruction | v0.3 |
| Q110 | Values used in two incompatible ways (type contradiction) | DF-19 + `IS EMPTY` / `COMPATIBLE_WITH` | v0.3 |
| Q111 | Callees that can mutate a validated value via alias | DF-17 mutation fan-out | v0.3 |
| Q112 | Getters returning mutable internal field reference | DF-17 `return_is_internal_field` | v0.3 |
| Q113 | Sanitize-then-mutate alias re-introducing taint | DF-17 + DF-11 taint | v0.3 |
| Q114 | Closures capturing loop variable by reference (late-binding) | DF-18 capture edges | v0.3 |
| Q115 | Closures capturing resource — extended lifetime | DF-18 + GM-13 | v0.3 |
| Q116 | Candidate callees for indirect call site | DF-18 function-value pedigree | v0.3 |
| Q117 | Tainted value through coercion to auth check | DF-10 + DF-11 DATA_FLOW | v0.3 |
| Q118 | `any`/`interface{}`-frontier boundary crossings | GM-14.6 type-confidence | v0.3 |

---

## Cross-references

- Version ladder and detection: `reference/versions.md`
- CQL syntax, supported/unsupported clauses: `reference/query-language.md`
- Edge-condition labels, confidence ladder, transience: `reference/mental-model.md`
- Exact subcommand flags: `reference/cli.md`
- Taint source/sink/sanitizer classes (v0.3 context): `recipes/taint.md`
- Object-model edges (MEMBER_OF, method nodes): `recipes/object-model.md`
