# Theme 11: Types, Mutability, and Closures

Most static analysis tools work within the type system: they look up declared types
and follow declared flow. This theme covers the questions that fall through that
net — values whose declared type is erased or widened, parameters whose mutation
effects propagate invisibly across function boundaries, and closures that capture
variables or resources in ways that extend lifetimes or re-introduce taint. The
enabling features are DF-17 (mutability model), DF-18 (function values and
closures), and DF-19 (lineage type reconstruction). Queries in this theme use
docs/05 Q-26 through Q-30 worked examples; those Q-spec IDs are cited below.

---

### Q109 — What is the candidate type set for the value passed to `dispatch()` — it is typed as `interface{}` at the call site but has a concrete usage footprint downstream?

**Personas:** PSE · **Status:** core-extension — uses Q-26 lineage type reconstruction (DF-19, `core-extension`)

An `interface{}`-typed (or Go `any`-typed) parameter tells you nothing about what
concrete type the caller actually passes. By gathering constraints from how the
value is constructed upstream (pedigree direction), how it is used downstream
(usage direction), and what other values it must be compatible with (sideways
constraints), `cgx` reconstructs the candidate type set. Specified in docs/05 Q-26.

**The query**

```cgx
-- illustrative: requires DF-19 (core-extension) type reconstruction procedure
-- DEFERRED: `cgx.type_reconstruct` and named-argument CALL syntax are not supported in v0.3.0;
--           `MATCH (v {...})` with no relationship also causes a plan error (exit 2)
MATCH (v {name:"v", scope:"process"})
CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
RETURN candidate_type, confidence, evidence
ORDER BY confidence DESC
```

Via the Layer 1 subcommand:

`cgx type-of` does not exist in v0.3.0. Use `cgx query` with the CQL form above once DF-19 (schema-room) is available. The flag `--at-function` does not exist.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `MATCH (v {name:"v", scope:"process"})` | Select the value node by name and the function it lives in. The `scope` attribute limits the match to this specific binding, not all variables named `v`. |
| `CALL cgx.type_reconstruct(v, direction: "all")` | Invoke the type-reconstruction procedure. Direction `"all"` gathers constraints from three directions simultaneously: up (constructors and literals — certain anchors), down (method calls and typed parameter sinks), and sideways (DF-9 join arms, aliases, channel send↔recv pairs). The constraint intersection is why this is a procedure rather than a simple pattern. |
| `YIELD candidate_type, confidence, evidence` | Receive the result columns: the candidate type, how confident the reconstruction is, and the evidence trail (which constraints drove the result). |
| `ORDER BY confidence DESC` | Most-confident candidates first. `certain` means a constructor was found upstream; `probable` means usage footprint; `possible` means insufficient constraints. |

**Reading the result** — A single `certain` candidate means the value is effectively fully typed despite the erased declaration. A narrowed set at `probable` gives 2–3 concrete types consistent with the usage. An empty result (`candidate_type IS EMPTY`) is a type contradiction — the value is used in two incompatible ways (see Q110). `possible` candidates are structural footprint matches; treat them as hypotheses, not conclusions.

---

### Q110 — Which values are used in two incompatible ways — for example, passed as an integer to one function and as a string to another — indicating a type contradiction?

**Personas:** PSE · **Status:** core-extension — uses Q-26 lineage type reconstruction, `IS EMPTY` predicate, `COMPATIBLE_WITH` predicate (DF-19, `core-extension`)

A type contradiction is the signal that a value is doing two incompatible things:
the constraint intersection is empty because no single type can satisfy all the
usage constraints simultaneously. This is a class of bug that type checkers cannot
surface (they narrow the declared type, not the actual usage) and no production
tool exposes as an on-demand graph query. Specified in docs/05 Q-26 (type-contradiction
bug query).

**The query**

```cgx
-- illustrative: requires DF-19 (core-extension) type reconstruction procedure
-- DEFERRED: `MATCH (v)` with no relationship causes a plan error (exit 2); `IS NOT NULL` /
--           `IS EMPTY` predicates are not supported in v0.3.0 (exit 2); `cgx.type_reconstruct`
--           and `COMPATIBLE_WITH` predicate are not supported in v0.3.0 (exit 2)
MATCH (v)
WHERE v.declared_type IS NOT NULL
CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
WHERE candidate_type IS EMPTY
   OR (confidence IN ["certain","probable"]
       AND NOT candidate_type COMPATIBLE_WITH v.declared_type)
RETURN v.name, v.file, v.line,
       v.declared_type,
       candidate_type,
       evidence
ORDER BY v.file, v.line
```

For checking a specific value:

```cgx
-- illustrative: requires DF-19 (core-extension) type reconstruction procedure
-- DEFERRED: named-argument CALL syntax and `cgx.type_reconstruct` are not supported in
--           v0.3.0; `MATCH (v {...})` with no relationship also causes a plan error (exit 2)
MATCH (v {name:"result", scope:"Processor::run"})
CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
RETURN v.name, v.file, v.line, candidate_type, confidence
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `v.declared_type IS NOT NULL` | Restrict to values that have a declared type — the contradiction is most actionable when it conflicts with a stated declaration. |
| `candidate_type IS EMPTY` | The `IS EMPTY` predicate returns `true` when the constraint intersection is empty — no type satisfies all directions of usage simultaneously. This is the direct contradiction signal. |
| `NOT candidate_type COMPATIBLE_WITH v.declared_type` | The `COMPATIBLE_WITH` predicate tests whether two types share a common subtype. `false` here means the reconstructed type and the declared type are structurally incompatible — a mismatch between what is declared and how the value is actually used. |
| `evidence` | The evidence trail from the reconstruction procedure — names the specific constructor, method call, or join arm that drove each constraint. This is needed to understand which use is the "wrong" one. |

**Reading the result** — Each row is a value used in incompatible ways. The `evidence` column names the conflicting constraints. A contradiction with `declared_type` is likely a bug; a contradiction within the usage footprint (empty candidate set with no declared type to compare) means two call sites treat the value as different types. No mainstream static analysis tool detects this pattern as an on-demand query.

---

### Q111 — After this value is validated and returned from `parse_user_input()`, which aliases or callees can mutate it before it reaches the authorization check?

**Personas:** PSE · **Status:** schema-room — depends on DF-17 mutability model, `writes-param(i)` / `writes-receiver` effect summaries (`schema-room`, Phase 3)

Validation does not help if a callee can overwrite the validated value before it
reaches the check that relies on it. The mutation fan-out query (Q-27) traces
every function that can write to a parameter after a given program point, using
the `writes-param(i)` and `writes-receiver` effect summaries from DF-17. Specified
in docs/05 Q-27.

**The query**

```cgx
-- illustrative: requires DF-17 (schema-room) `writes-param` / `writes-receiver` effect summaries
MATCH (v {name:"user_record", scope:"AuthHandler::validate"})
CALL cgx.mutation_fanout(v) YIELD mutator, effect, confidence
RETURN mutator.name, mutator.file, mutator.line,
       effect,
       confidence
ORDER BY confidence DESC, mutator.file
```

Via Layer 1:

`cgx mutation-fanout` does not exist in v0.3.0. Use `cgx query` with the CQL form above once DF-17 (schema-room) is available. The flags `--at-function` and the trailing-path convention do not exist; use `--repo <path>` when specifying a repository.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `MATCH (v {name:"user_record", scope:"AuthHandler::validate"})` | Select the value at the specific function scope where validation has just completed. |
| `CALL cgx.mutation_fanout(v)` | Invoke the mutation fan-out procedure. This is the outbound dual of pedigree: instead of "what populated this value" it asks "who can change this value from this point forward." The procedure traverses alias edges (DF-16) and effect summaries (DF-17). |
| `YIELD mutator, effect, confidence` | Receive the mutating callee, its mutation effect label (`writes-param(0)`, `writes-receiver`, `mutation-out`), and the confidence of the edge. |

**Reading the result** — Each row names a function that can write to the validated value before the authorization check. Focus on high-confidence mutators (`certain` or `probable`) whose `effect` is `writes-param(0)` or `writes-receiver` — those directly overwrite the value through a parameter or receiver reference. A long list of potential mutators suggests the validated value is passed to too many callees before the auth check.

---

### Q112 — Which getters on `UserRecord` return a direct reference to a mutable internal field — callers can mutate the object's state through the returned reference?

**Personas:** SSE · **Status:** schema-room — depends on DF-17 mutability model, `writes-receiver` effect, `return_is_internal_field` attribute (`schema-room`, Phase 3)

A getter that returns a `&mut` reference (Rust), a mutable Java field reference,
or a Go slice of an internal array gives callers write access to the object's
internal state without going through any method. SpotBugs `EI_EXPOSE_REP` detects
this pattern for Java locally; `cgx` exposes it as a composable graph query that
can also answer "which callers mutate the exposed field through the returned
reference." Specified in docs/05 Q-27 (getters returning mutable internal state).

**The query**

```cgx
-- illustrative: requires DF-17 (schema-room) for `return_is_internal_field` and `return_mutability` attributes
MATCH (getter:method)-[:MEMBER_OF]->(t)
WHERE getter.name STARTS WITH "get"
  AND getter.return_mutability = "mutable"
  AND getter.return_is_internal_field = true
RETURN getter.name, getter.file, getter.line,
       t.name AS containing_type
ORDER BY t.name, getter.name
```

To extend the query and find which callers mutate the exposed field through the reference:

```cgx
-- illustrative: requires DF-17 (schema-room)
MATCH (getter:method)-[:MEMBER_OF]->(t)
WHERE getter.return_is_internal_field = true
  AND getter.return_mutability = "mutable"
MATCH (caller)-[:CALLS]->(getter)
CALL cgx.mutation_fanout(getter) YIELD mutator, effect, confidence
RETURN getter.name, getter.file, caller.name, caller.file,
       mutator.name AS downstream_mutator, effect, confidence
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `getter.return_mutability = "mutable"` | The return value is a mutable reference — in Rust terms `&mut T`, in Java terms a reference to a mutable field, in Go terms a slice or pointer to an internal array. |
| `getter.return_is_internal_field = true` | The return value's pedigree traces to a field of `self`/`this` without a defensive copy. This attribute is set by DF-17 analysis when the returned value is an alias into the object's own state. |
| `cgx.mutation_fanout(getter)` | Extended form: from the getter's return value, find every function that can further mutate the exposed field — the transitive exposure chain. |

**Reading the result** — Each row names a getter that gives callers direct write access to the object's internals. The `containing_type` column shows which type has the exposure. Combine with the caller query to see which code is actually mutating the exposed state — that is the set of places that need to use a defensive copy or be explicitly documented as internal-access-only.

---

### Q113 — Is there a path where `sanitize(input)` clears a taint label and then a callee mutates the value through an alias, re-introducing the taint before the value reaches the SQL sink?

**Personas:** PSE · **Status:** schema-room — depends on DF-17 mutability model (sanitization invalidation via alias, DF-17.4), DF-11 taint labels (`schema-room` for alias-level mutation; DF-11 taint labels are `core-extension`)

Sanitizing a value removes its taint label. But if a callee holds an alias to the
same memory and overwrites it after sanitization, the sanitized value is effectively
replaced with tainted content before it reaches the sink. This is a real attack
class (mutable value passed to sanitizer, then mutated by a concurrent or sequential
callee) with no equivalent in any production taint analysis tool. Specified in
docs/05 Q-27 (sanitization invalidation).

**The query**

```cgx
-- illustrative: requires DF-17 (schema-room) for `taint_label.re_applied` and
--               `taint_label.reason = "mutation-after-sanitization"` edge attributes
-- DEFERRED: `sanitizer_class`, `sink_class`, and `taint_label` are not supported node/edge
--           properties in v0.3.0; submitting this query produces a plan error (exit 2)
MATCH path = (sanitizer)-[:DATA_FLOW*]->(use {sink_class:"sql"})
WHERE sanitizer.sanitizer_class = "sql"
  AND ANY(edge IN relationships(path)
          WHERE edge.taint_label.re_applied = true
            AND edge.taint_label.reason = "mutation-after-sanitization")
RETURN sanitizer.name, sanitizer.file, sanitizer.line,
       use.name, use.file, use.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `sanitizer.sanitizer_class = "sql"` | The path starts at a SQL-class sanitizer. **Deferred in v0.3.0:** `sanitizer_class` is not a supported node property; using it causes a plan error (exit 2). |
| `use {sink_class:"sql"}` | The path ends at a SQL sink. **Deferred in v0.3.0:** `sink_class` is not a supported node property; using it causes a plan error (exit 2). |
| `edge.taint_label.re_applied = true` | An edge on the path carries the `re_applied` attribute, meaning the taint analysis detected that a taint label was cleared and then restored because the underlying value was mutated through an alias after sanitization. **Deferred in v0.3.0:** `taint_label` is not a supported edge property; using it causes a plan error (exit 2). |
| `taint_label.reason = "mutation-after-sanitization"` | Narrows to the specific re-application reason — mutation via alias. **Deferred in v0.3.0** (same as above). |

**Reading the result** — Each row is a sanitize-then-mutate-via-alias pattern where tainted data reaches a SQL sink despite passing through a sanitizer. The `sanitizer.file`/`line` is where sanitization occurred; the `use.file`/`line` is the SQL sink. Examine the intermediate call chain (the path nodes between sanitizer and use) to find the alias-holding callee that performed the mutation.

---

### Q114 — Which closures capture a loop variable by reference — the variable's value at call time will be the final loop value, not the value at capture time?

**Personas:** PSE · **Status:** schema-room — depends on DF-18 function values and closures, capture edges with `by-ref` × binding mutability attributes (`schema-room`, Phase 3)

In JavaScript (pre-`let`), Go (pre-1.22), and Python, a closure defined inside a
loop and capturing a loop variable by reference will see the variable's final value
when called, not the value it had when the closure was created. This is a common
source of bugs in async callbacks and goroutines. Specified in docs/05 Q-28
(loop-variable capture bug family).

**The query**

```cgx
-- illustrative: requires DF-18 (schema-room) capture edges with by-ref and binding mutability
MATCH (loop_var)-[cap:CAPTURE {capture:"by-ref"}]->(closure)
WHERE loop_var.binding_mutability = "mutable"
  AND loop_var.is_loop_variable = true
RETURN loop_var.name, loop_var.file, loop_var.line,
       closure.name, closure.file, closure.line,
       cap.capture AS capture_mode
ORDER BY loop_var.file, loop_var.line
```

Via the Layer 1 shorthand:

`cgx query --capture-bug` does not exist in v0.3.0. Use `cgx query` with the CQL form above once DF-18 (schema-room) is available. The flag `--capture-bug` does not exist on `cgx query`; the trailing-path convention is also wrong — use `--repo <path>`.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(loop_var)-[cap:CAPTURE {capture:"by-ref"}]->(closure)` | A `CAPTURE` edge typed `by-ref` from a loop variable to a closure — the closure holds a reference to the variable, not a copy of its current value. |
| `loop_var.binding_mutability = "mutable"` | The loop variable is mutable — its value changes at each iteration. An immutable loop variable would not produce the late-binding bug. |
| `loop_var.is_loop_variable = true` | Restrict to variables declared in a loop header — the specific binding sites where the iteration index or range element is introduced. |
| `cap.capture AS capture_mode` | Return the capture mode attribute from the edge, confirming `by-ref`. |

**Reading the result** — Each row is a closure with a by-ref capture of a mutable loop variable. The `loop_var.file`/`line` locates the loop variable; `closure.file`/`line` locates the closure definition. The fix is to create a local copy of the loop variable inside the loop body and capture that copy by value. This query is language-agnostic: it covers Go pre-1.22 goroutine closures, JavaScript `var`-in-loop callbacks, and Python late-binding lambdas through the same graph pattern.

---

### Q115 — Which closures capture a file handle, database connection, or lock guard, and on which execution paths does the closure run — potentially extending the resource's lifetime beyond the scope where it was acquired?

**Personas:** SSE · **Status:** schema-room — depends on DF-18 function values and closures, capture edges, GM-13 resource lifecycle pairs (`schema-room`, Phase 3)

A closure that captures a resource extends the resource's lifetime to match the
closure's execution context, not the enclosing scope's exit. If the closure is
stored, passed to another thread, or executed asynchronously, the resource may be
held far longer than intended — or released in a different context than expected.
Specified in docs/05 Q-28 (captured-resource lifetime family).

**The query**

```cgx
-- illustrative: requires DF-18 (schema-room) capture edges and GM-13 (schema-room) resource lifecycle pairs
MATCH (resource)-[cap:CAPTURE]->(closure)
WHERE resource.resource_class IN ["lock", "file-handle", "db-connection"]
RETURN resource.name, resource.file, resource.line,
       closure.name, closure.file, closure.line,
       cap.capture AS capture_mode
ORDER BY resource.resource_class, resource.file, resource.line
```

To find closures that escape to an outer scope (dangling-capture risk):

```cgx
-- illustrative: requires DF-18 (schema-room) and DF-16 (schema-room) aliasing / escape
MATCH (v)-[cap:CAPTURE {capture:"by-ref"}]->(closure)
MATCH escape_path = (closure)-[:DATA_FLOW*]->(sink)
WHERE sink.scope_depth < v.scope_depth
  AND v.binding_mutability = "mutable"
RETURN v.name, v.file, v.line,
       closure.name, sink.name,
       sink.file, sink.line
```

Via Layer 1:

`cgx query --capture-bug` does not exist in v0.3.0. Use `cgx query` with the CQL form above once DF-18 and GM-13 (schema-room) are available. The flag `--capture-bug` does not exist; use `--repo <path>` instead of a trailing path argument.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `resource.resource_class IN ["lock", "file-handle", "db-connection"]` | Restrict to nodes carrying a resource class from the GM-13 vocabulary — types that have acquire/release lifecycle semantics. |
| `(resource)-[cap:CAPTURE]->(closure)` | A CAPTURE edge from the resource to the closure — the closure holds a reference to (or owns) the resource. |
| `cap.capture AS capture_mode` | Whether the capture is `by-ref` (reference to the resource) or `by-value` (ownership transferred into the closure). By-value capture is safer for resource lifetime but may still extend the lifecycle. |
| `sink.scope_depth < v.scope_depth` | In the escape query: the closure escapes to a scope that is shallower (more outer) than where the captured variable was declared — a dangling capture risk. |

**Reading the result** — Each row is a resource captured by a closure. The `capture_mode` column distinguishes a by-ref capture (the closure holds a reference; the resource must outlive the closure) from a by-value capture (the resource is moved into the closure and released when the closure is dropped). Focus on `by-ref` captures of lock guards, which extend the lock hold time to match the closure's lifetime rather than the enclosing scope's.

---

### Q116 — What are the candidate callee functions for the indirect call at this site — `handler` is a function value whose origin I need to trace?

**Personas:** SSE · **Status:** schema-room — depends on DF-18 function values and closures, pedigree-based indirect-call resolution (`schema-room`, Phase 3)

When a call site invokes a function value rather than a named function, the call
graph has a gap: the static edge does not name the target. `cgx` fills this by
running the pedigree query over the function value to find what concrete functions
could flow to it. Specified in docs/05 Q-29.

**The query**

```cgx
-- illustrative: requires DF-18 (schema-room) function-value pedigree and indirect-call resolution
MATCH (callsite)-[:CALLS:indirect]->(target)
WHERE callsite.scope = "process"
  AND callsite.arg_position = 0
RETURN target.name, target.file, target.line,
       callsite.confidence AS resolution_confidence
ORDER BY callsite.confidence DESC, target.name
```

Via Layer 1:

`cgx pedigree` does not exist in v0.3.0. Use `cgx query` with the CQL form above once DF-18 (schema-room) is available. The `--at-function` flag does not exist (the unrelated `--at` flag pins to a git ref); use `--repo <path>` instead of a trailing path argument.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(callsite)-[:CALLS:indirect]->(target)` | Follow `CALLS:indirect` edges — these are the pedigree-resolved candidate targets for an indirect call site, populated from the function value's pedigree. |
| `callsite.scope = "process"` | Restrict to a specific function scope where the indirect call lives. |
| `callsite.arg_position = 0` | The function value being resolved is in argument position 0 (the first argument). |
| `callsite.confidence AS resolution_confidence` | The confidence reflects how the resolution was derived: `certain` for a direct function literal assignment with no branch, `probable` for a branch merge or container retrieval, `possible` for an opaque boundary or FFI. |

**Reading the result** — Each row is one candidate callee for the indirect call. The `resolution_confidence` column is the confidence of the resolution path: `certain` means the only function that could ever flow to this call site is the named target; `probable` means it is likely but a branch merge or container store introduces some ambiguity. The `possible` tier indicates over-approximation from an opaque boundary; these results may include false positives. No existing production tool surfaces function-value pedigree as a per-callee confidence-labeled query result.

---

### Q117 — Does a tainted value reach a loose-equality comparison or implicit type coercion in an authorization decision path — for example, PHP `==` treating `"0e123"` and `"0"` as equal?

**Personas:** PSE · **Status:** schema-room — depends on DF-10 `coerce(from,to)` transformation kind, Q-30 coercion and type-confidence queries (`schema-room`, Phase 3)

Type coercions in authorization decision paths are a class of authentication bypass.
PHP's loose equality allows `"0e123" == "0"` to evaluate as `true`; JavaScript's
`==` coerces types similarly. If an attacker-controlled value reaches an
authorization check through a coercion site, the attacker may satisfy the check
without providing the correct value. Specified in docs/05 Q-30 (type-juggling-in-auth).

**The query**

```cgx
-- illustrative: requires DF-10 coerce(from,to) transformation kind (schema-room)
-- DEFERRED: `source_class`, `sink_class`, and `sanitizer_class` are not supported node
--           properties in v0.3.0; submitting this query produces a plan error (exit 2)
MATCH path = (src)-[:DATA_FLOW*]->(coerce_site)-[:DATA_FLOW {transformation_kind:"coerce"}]
              ->(sink {sink_class:"auth-check"})
WHERE src.source_class IN ["network","user-input"]
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "auth-check")
RETURN src.name, src.file, src.line,
       coerce_site.name, coerce_site.file, coerce_site.line,
       [e IN relationships(path) | e.transformation_kind] AS transformations,
       sink.name, sink.file, sink.line
ORDER BY src.file, src.line
```

Via Layer 1:

`cgx paths --from-class` does not exist in v0.3.0. The flags `--from-class`, `--to-class`, and `--require-transformation` are phantom; they do not exist on `cgx paths`. Use `cgx query` with the CQL form above once DF-10 taint properties (schema-room) are available. Use `--repo <path>` to specify the repository; the trailing-path convention does not exist.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class IN ["network","user-input"]` | The source is attacker-controlled — data from the network or from explicit user input. **Deferred in v0.3.0:** `source_class` is not a supported node property; using it causes a plan error (exit 2). |
| `[:DATA_FLOW {transformation_kind:"coerce"}]` | Exactly one edge in the path must carry the `coerce` transformation kind — the DF-10 label applied when a type conversion changes the value's type. This is the coercion site. |
| `sink {sink_class:"auth-check"}` | The path ends at an authorization decision point. **Deferred in v0.3.0:** `sink_class` is not a supported node property; using it causes a plan error (exit 2). |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "auth-check")` | No auth-check-class sanitizer sits between the source and the coercion. **Deferred in v0.3.0:** `sanitizer_class` is not a supported node property; using it causes a plan error (exit 2). |
| `[e IN relationships(path) | e.transformation_kind] AS transformations` | The full transformation sequence on the path — shows every type change the value underwent, not just the coercion. |

**Reading the result** — Each row names an attacker-controlled value that reaches an authorization check through at least one type coercion. The `coerce_site.file`/`line` is where the coercion occurs; that is the location to harden (by using strict equality or explicit type casting). The `transformations` list shows whether the coercion was the only transformation or one of many, which helps triage whether the coercion is incidental or structural.

---

### Q118 — Where does the `any`-typed, `interface{}`-typed, or unannotated-parameter frontier begin — which call sites are the first point where a value crosses from fully typed into untyped territory?

**Personas:** PSE · **Status:** schema-room — depends on GM-14 type-confidence boundaries (GM-14.6), Q-30 coercion and type-confidence queries (`schema-room`, Phase 3)

The `any`-frontier is the type-system analogue of a trust boundary: beyond it,
static type guarantees no longer hold. TypeScript does not expose `any`-frontier
points as a call-graph query; `cgx` models these as type-confidence boundary nodes
(GM-14.6) and makes them directly queryable. Specified in docs/05 Q-30 (`any`-frontier
query).

**The query**

```cgx
-- illustrative: requires GM-14.6 (schema-room) type-confidence boundary attribute
MATCH (boundary)-[r:DATA_FLOW]->(typed_receiver)
WHERE boundary.type_confidence_boundary = true
  AND typed_receiver.declared_type IS NOT NULL
  AND typed_receiver.declared_type <> "any"
  AND typed_receiver.declared_type <> "interface{}"
RETURN boundary.name, boundary.file, boundary.line,
       typed_receiver.name, typed_receiver.declared_type,
       r.transformation_kind
ORDER BY boundary.file, boundary.line
```

For the reverse direction — where fully-typed values first cross into untyped territory:

```cgx
-- illustrative: requires GM-14.6 (schema-room)
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

**Breaking it down**

| Fragment | What it means |
|---|---|
| `boundary.type_confidence_boundary = true` | The GM-14.6 attribute marking a node as a type-confidence boundary — a point where the static type system loses precision and downstream resolution confidence drops to at most `possible` without further assertion or reconstruction. |
| `typed_receiver.declared_type IS NOT NULL AND ... <> "any"` | The receiving side is a fully-typed binding — the value is entering typed territory from the boundary. |
| `r.transformation_kind` | How the value crossed the boundary — `identity` (no transformation, just a type assertion), `coerce` (explicit type cast), or `parsed` (deserialized from an untyped format). |

**Reading the result** — Each row is a type-confidence boundary crossing. The `boundary.file`/`line` is the frontier point — the first location where a value moves from typed to untyped (or vice versa). These are the locations where type assertions, runtime checks, or type reconstruction (Q109) are needed to restore static guarantees. A high concentration of boundary crossings in security-critical paths (authorization decisions, database writes) is a signal that the codebase relies heavily on runtime type correctness rather than compile-time enforcement.
