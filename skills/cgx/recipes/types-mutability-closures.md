# Recipe: Types, Mutability, and Closures

**Theme 11 of 13** — Type reconstruction, mutation fan-out, capture edges, coercion.

**Version posture: all questions in this theme are deferred past v0.3.**

Run `cgx --version` before using this recipe. The queries below all require CQL
constructs, edge types, or node properties that produce exit 2 plan or parse errors
in v0.3.0. Even if `MINOR >= 3`, none of these queries run today. See
`reference/versions.md` for the full version ladder and `reference/query-language.md`
for the current CQL clause matrix.

The enabling features (DF-17 mutability model, DF-18 function values and closures,
DF-19 lineage type reconstruction, DF-10 coercion transformation kind, GM-14.6
type-confidence boundaries) are all `schema-room` work scheduled after v0.3. The
CQL constructs they require include `CALL cgx.type_reconstruct(...)`
(unknown procedure, exit 2), `CAPTURE` edges (unknown edge type, exit 2),
`CALLS:indirect` edges (sub-type qualifier deferred, exit 2), `STARTS WITH`,
`IS NULL`/`IS EMPTY`, and `COMPATIBLE_WITH` predicates (parse or plan errors,
exit 2), and many node/edge properties (`scope`, `declared_type`,
`binding_mutability`, `is_loop_variable`, `resource_class`,
`return_mutability`, `return_is_internal_field`, `type_confidence_boundary`,
`transformation_kind`). `cgx.mutation_fanout` is partially registered but yields
only `mutator` and `confidence` — `effect` and `transform` are deferred — and
the Q111 recipe query also uses the deferred `scope` node property, so Q111 exits
2 today.

**Step 0 — find the exact symbol name first.**
Use `cgx search <pattern>` (Since: v0.2) to resolve a partial or half-remembered
name to an exact FQN before running any cgx command. An unknown symbol exits 2 with
`no symbol matched '<x>'`.

```bash
cgx search "make_adder" --repo /path/to/repo
cgx search "closure" --repo /path/to/repo --kind lambda
cgx search "validate" --repo /path/to/repo --kind method
```

---

## Documented deferred forms (not runnable in v0.3)

All entries below are deferred. Gate every invocation on `cgx --version`:

```bash
cgx --version   # look at MINOR; none of these run yet even at MINOR == 3
```

The canonical CQL patterns use double-quoted string literals inside a
single-quoted shell argument. Wrap all cgx query invocations as shown.

---

### What concrete types can this `interface{}`-typed value actually hold? (Q109)

**Status:** deferred (DF-19 type reconstruction — `CALL cgx.type_reconstruct(...)` procedure not implemented; exit 2)

```cypher
-- Requires: cgx.type_reconstruct procedure (DF-19) — NOT available in v0.3
MATCH (v)-[:DATA_FLOW]->(w)
WHERE v.name = "v"
CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
RETURN candidate_type, confidence, evidence
ORDER BY confidence DESC
```

**Why this will work:** `cgx.type_reconstruct` gathers constraints from three
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

**Status:** deferred (DF-19 type reconstruction; `IS NULL`/`IS EMPTY` and `COMPATIBLE_WITH` predicates — all exit 2 in v0.3)

```cypher
-- Requires: cgx.type_reconstruct procedure (DF-19),
--           IS EMPTY / IS NULL predicates, COMPATIBLE_WITH predicate
--           — NONE available in v0.3
MATCH (v)-[:DATA_FLOW]->(w)
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

**Why this will work:** `candidate_type IS EMPTY` fires when no single type satisfies
all usage constraints simultaneously. `NOT candidate_type COMPATIBLE_WITH v.declared_type`
catches cases where reconstruction and declaration diverge without an empty set.

**Reading the result:** Each row is a value used in incompatible ways. The
`evidence` column identifies the conflicting constraints (which specific
constructor or call site drives each constraint). A contradiction against
`declared_type` is likely a bug; an empty candidate set with no declared type
means two call sites treat the same value as different types entirely.

---

### Which callees can mutate this validated value before it reaches the auth check? (Q111)

**Status:** deferred (DF-17 mutability model — `cgx.mutation_fanout` is
partially registered but yields only `mutator` and `confidence`; `effect` /
`writes-param` / `writes-receiver` effect summaries are deferred. The recipe
query also uses the deferred `scope` node property and `YIELD effect` column,
both of which exit 2 today.)

```cypher
-- Requires: effect summary columns (DF-17) and scope node property
--           — YIELD effect and scope property both exit 2 in v0.3
MATCH (v {name: "user_record"})-[:DATA_FLOW]->(w)
WHERE v.scope = "AuthHandler::validate"
CALL cgx.mutation_fanout(v) YIELD mutator, effect, confidence
RETURN mutator.name, mutator.file, mutator.line,
       effect,
       confidence
ORDER BY confidence DESC, mutator.file
```

**Why this will work:** `cgx.mutation_fanout` is designed as the outbound dual of pedigree: from
a given program point it traverses alias edges (DF-16) and effect summaries (DF-17)
to find every function that can write to the value between that point and a
downstream check.

**The procedure itself is not deferred** — only the `effect` YIELD column and the `scope` node
property in the query above are (both exit 2). `CALL cgx.mutation_fanout(v) YIELD mutator` runs
today over `derives-from` edges. What it returns is not what its column name says: it yields the
values `v` **derives from** — its sources, the `cgx flows-from` answer — while `cgx.pedigree` yields
consumers. The two are swapped relative to their names. See the direction table in
`recipes/taint.md` before building on either.

**Reading the result:** Focus on `certain` or `probable` mutators with effect
`writes-param(0)` or `writes-receiver` — those directly overwrite the value through
a parameter or receiver reference. A long list of potential mutators signals the
validated value is passed to too many callees before the authorization check.

---

### Which getters return a mutable reference to an internal field? (Q112)

**Status:** deferred (DF-17 mutability model — `MEMBER_OF` edge carries no data in v0.3; `STARTS WITH` predicate is a parse error; `return_mutability` / `return_is_internal_field` node properties are unknown — all exit 2)

```cypher
-- Requires: MEMBER_OF edge data (DF-17), return_mutability and
--           return_is_internal_field attributes, STARTS WITH predicate
--           — NONE available in v0.3
MATCH (getter:method)-[:MEMBER_OF]->(t)
WHERE getter.name STARTS WITH "get"
  AND getter.return_mutability = "mutable"
  AND getter.return_is_internal_field = true
RETURN getter.name, getter.file, getter.line,
       t.name AS containing_type
ORDER BY t.name, getter.name
```

**Why this will work:** DF-17 analysis sets `return_is_internal_field = true` when
the returned value's pedigree traces to a field of `self`/`this` without a
defensive copy. `return_mutability = "mutable"` restricts to cases where the
caller receives write access.

**Reading the result:** Each row names a getter that exposes object internals to
callers. The `containing_type` column identifies the type with the exposure.
To find callers that actually mutate through the returned reference, extend with
a `MATCH (caller)-[:CALLS]->(getter)` arm and `cgx.mutation_fanout(getter)`.

---

### Can a callee mutate a sanitized value through an alias, re-introducing taint? (Q113)

**Status:** deferred — two independent deferrals: (1) taint props `sink_class` / `sanitizer_class` / `taint_label.re_applied` are not implemented in any v0.3 release (exit 2 plan error); (2) DF-17 alias-mutation detection is also deferred past v0.3.

```cypher
-- Requires: sink_class / sanitizer_class node properties (taint engine, deferred),
--           taint_label.re_applied edge attribute (DF-17.4, deferred)
--           — ALL deferred past v0.3
MATCH path = (sanitizer)-[:DATA_FLOW*2]->(use)
WHERE sanitizer.sanitizer_class = "sql"
  AND use.sink_class = "sql"
  AND ANY(edge IN relationships(path)
          WHERE edge.taint_label.re_applied = true
            AND edge.taint_label.reason = "mutation-after-sanitization")
RETURN sanitizer.name, sanitizer.file, sanitizer.line,
       use.name, use.file, use.line
```

**Why this will work:** Sanitizing a value clears its taint label, but if a callee
holds an alias to the same memory and overwrites it after sanitization, the taint
re-enters the path. DF-17 detects this and sets `taint_label.re_applied = true`
with `reason = "mutation-after-sanitization"` on the relevant DATA_FLOW edge.

**Reading the result:** Each row is a sanitize-then-mutate-via-alias pattern that
reaches a SQL sink. `sanitizer.file`/`line` is where sanitization occurred;
`use.file`/`line` is the SQL sink. Examine the intermediate call chain to locate
the alias-holding callee that performed the mutation.

---

### Which closures capture a loop variable by reference — late-binding bug? (Q114)

**Status:** deferred (DF-18 capture edges — `CAPTURE` is an unknown edge type in v0.3, exit 2; `binding_mutability` / `is_loop_variable` node properties are unknown, exit 2)

```cypher
-- Requires: CAPTURE edges with by-ref and binding mutability (DF-18),
--           binding_mutability and is_loop_variable node properties
--           — NONE available in v0.3
MATCH (loop_var)-[cap:CAPTURE {capture: "by-ref"}]->(closure)
WHERE loop_var.binding_mutability = "mutable"
  AND loop_var.is_loop_variable = true
RETURN loop_var.name, loop_var.file, loop_var.line,
       closure.name, closure.file, closure.line,
       cap.capture AS capture_mode
ORDER BY loop_var.file, loop_var.line
```

**Why this will work:** DF-18 records a typed `CAPTURE` edge from each captured
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

**Status:** deferred (DF-18 capture edges — `CAPTURE` is an unknown edge type in v0.3, exit 2; `resource_class` node property is unknown, exit 2)

```cypher
-- Requires: CAPTURE edges (DF-18), resource_class attribute (GM-13)
--           — NONE available in v0.3
MATCH (resource)-[cap:CAPTURE]->(closure)
WHERE resource.resource_class IN ["lock", "file-handle", "db-connection"]
RETURN resource.name, resource.file, resource.line,
       closure.name, closure.file, closure.line,
       cap.capture AS capture_mode
ORDER BY resource.resource_class, resource.file, resource.line
```

**Why this will work:** GM-13 annotates nodes whose types carry acquire/release
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

**Status:** deferred (DF-18 function-value pedigree — `CALLS:indirect` sub-type qualifier is recognised but deferred in v0.3, exit 2)

```cypher
-- Requires: CALLS:indirect edges from function-value pedigree (DF-18)
--           — sub-type qualifier deferred in v0.3 (exit 2)
MATCH (callsite)-[:CALLS:indirect]->(target)
WHERE callsite.name = "process"
RETURN target.name, target.file, target.line,
       callsite.confidence AS resolution_confidence
ORDER BY callsite.confidence DESC, target.name
```

**Why this will work:** When a call site invokes a function value rather than a named
function, the CALLS graph has a gap. DF-18 fills it by tracing the pedigree of the
function value to its constructors and recording `CALLS:indirect` edges with per-candidate
confidence.

**Reading the result:** `resolution_confidence:"certain"` means only one concrete
function can ever flow to this call site. `"probable"` means a branch merge or
container store introduces ambiguity. `"possible"` reflects an opaque boundary
or FFI — these may include false positives. In v0.3 today, `cgx callees` surfaces
indirect calls as `[probable]` or `[possible]` edges in the forest using
conventional CHA/RTA narrowing, without the per-candidate pedigree detail that Q116 provides.

---

### Does a tainted value reach an auth check through a type coercion — type-juggling bypass? (Q117)

**Status:** deferred — three independent deferrals: (0) `path =` binding over a multi-relationship pattern (`[…*2]…[…]`) is a plan error in v0.3, exit 2 — this fires before taint props are evaluated; (1) `source_class` / `sink_class` taint node properties exit 2 in v0.3; (2) `transformation_kind` edge property is unknown in v0.3, exit 2.

```cypher
-- Requires: multi-segment path= binding (deferred),
--           source_class / sink_class node properties (taint engine, deferred),
--           transformation_kind edge property (DF-10, deferred)
--           — ALL deferred past v0.3
MATCH path = (src)-[:DATA_FLOW*2]->(coerce_site)-[:DATA_FLOW]->(sink)
WHERE src.source_class IN ["network", "user-input"]
  AND sink.sink_class = "auth-check"
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "auth-check")
RETURN src.name, src.file, src.line,
       coerce_site.name, coerce_site.file, coerce_site.line,
       sink.name, sink.file, sink.line
ORDER BY src.file, src.line
```

**Why this will work:** DF-10 labels DATA_FLOW edges with a `transformation_kind`
attribute, including `"coerce"` for type conversions. Filtering to paths where
at least one edge carries `"coerce"` and the path ends at an `auth-check` sink
isolates the type-juggling-in-auth pattern (e.g., PHP `"0e123" == "0"`).

**Reading the result:** `coerce_site.file`/`line` is where the coercion occurs —
the location to harden with strict equality or explicit type casting. The
`transformations` list shows every type change on the path, not just the coercion,
which helps triage whether coercion is incidental or structural.

Note: the cookbook shows a Layer-1 shorthand using `--from-class` and
`--require-transformation` flags. Those flags do not exist in any version of the
binary (see `reference/cli.md`). Use the CQL form above when the taint engine ships.

---

### Where does the `any`/`interface{}`/unannotated-parameter frontier begin? (Q118)

**Status:** deferred (GM-14.6 type-confidence boundary — `type_confidence_boundary` node property is unknown in v0.3, exit 2)

```cypher
-- Requires: type_confidence_boundary attribute (GM-14.6) — NOT available in v0.3
MATCH (typed_src)-[r:DATA_FLOW]->(boundary)
WHERE typed_src.declared_type IS NOT NULL
  AND boundary.type_confidence_boundary = true
RETURN typed_src.name, typed_src.file, typed_src.line,
       boundary.name, boundary.file, boundary.line,
       r.transformation_kind AS crossing_kind
ORDER BY typed_src.file, typed_src.line
```

**Why this will work:** GM-14.6 marks nodes as `type_confidence_boundary = true`
at the point where static type guarantees lose precision — the `any`-frontier.
The query finds every crossing from fully-typed territory into untyped territory.

**Reading the result:** Each row is a frontier crossing. `boundary.file`/`line`
is the first location where the static type system stops enforcing guarantees.
`crossing_kind:"identity"` means a bare type assertion, `"coerce"` an explicit
cast, `"parsed"` a deserialization boundary. A high concentration of crossings
in security-critical paths (auth decisions, DB writes) signals the codebase
relies on runtime type correctness rather than compile-time enforcement.

---

## Summary: all questions in this theme are deferred past v0.3

| Q | Question | Feature | Blocking construct |
|---|----------|---------|-------------------|
| Q109 | Candidate type set for `interface{}`-typed value | DF-19 type reconstruction | `CALL cgx.type_reconstruct` procedure |
| Q110 | Values used in two incompatible ways (type contradiction) | DF-19 + `IS EMPTY` / `COMPATIBLE_WITH` | `IS EMPTY`/`IS NULL` + `COMPATIBLE_WITH` predicates |
| Q111 | Callees that can mutate a validated value via alias | DF-17 mutation fan-out | `CALL cgx.mutation_fanout` procedure; `scope` node property |
| Q112 | Getters returning mutable internal field reference | DF-17 `return_is_internal_field` | `STARTS WITH` predicate; `return_mutability` / `return_is_internal_field` node props |
| Q113 | Sanitize-then-mutate alias re-introducing taint | DF-17 + taint engine | `sanitizer_class`/`sink_class` node props; `taint_label` edge prop |
| Q114 | Closures capturing loop variable by reference (late-binding) | DF-18 capture edges | `CAPTURE` edge type; `binding_mutability`/`is_loop_variable` node props |
| Q115 | Closures capturing resource — extended lifetime | DF-18 + GM-13 | `CAPTURE` edge type; `resource_class` node prop |
| Q116 | Candidate callees for indirect call site | DF-18 function-value pedigree | `CALLS:indirect` sub-type qualifier |
| Q117 | Tainted value through coercion to auth check | DF-10 + taint engine | `path =` multi-segment binding; `source_class`/`sink_class` node props; `transformation_kind` edge prop |
| Q118 | `any`/`interface{}`-frontier boundary crossings | GM-14.6 type-confidence | `IS NOT NULL` predicate; `type_confidence_boundary`/`declared_type` node props |

---

## Cross-references

- Version ladder and detection: `reference/versions.md`
- CQL syntax, supported/unsupported clauses: `reference/query-language.md`
- Edge-condition labels, confidence ladder, transience: `reference/mental-model.md`
- Exact subcommand flags: `reference/cli.md`
- Taint source/sink/sanitizer classes (taint engine, deferred): `recipes/taint.md`
- Object-model edges (MEMBER_OF, method nodes): `recipes/object-model.md`
