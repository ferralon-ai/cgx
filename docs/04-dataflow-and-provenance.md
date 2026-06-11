# 04 — Dataflow and Provenance

**Status:** Feature specification (pre-implementation)
**Audience:** Engineers using `cgx` for security analysis, dead-code detection, or impact analysis; contributors implementing the dataflow layer
**Working name:** `cgx` (placeholder — see docs/README.md)
**Cross-references:** docs/03-code-graph-model.md · docs/05-queries.md · docs/07-interfaces.md · docs/08-language-support.md

---

## Overview

This document specifies the dataflow and provenance features of `cgx`. These
features answer questions that the structural call graph alone cannot: where
did this value come from? what does it flow into? which properties of this
object are never used?

The model is built on top of the graph described in docs/03-code-graph-model.md.
Dataflow edges (primarily `derives-from`) are a subset of the full edge
taxonomy; provenance records (file:line + producing rule) appear on every fact.

Features specified here:

- DF-1: Value pedigree (inbound provenance fan-out)
- DF-2: Scope entry/exit inventory
- DF-3: Derives-from edges and transformation tags
- DF-4: Container and collection models
- DF-5: Taint analysis
- DF-6: Backward and forward slicing
- DF-7: Instance-level analysis (never-referenced members)
- DF-8: Soundiness — what `cgx` labels `possible` vs claims as fact

---

## DF-1 — Value Pedigree

**Pedigree** is the inbound provenance fan-out of a value: the transitive set
of values, computations, and external sources that contributed to a given value,
through any number of transformations.

### DF-1.1 — What pedigree answers

Given a symbol (a return value, a field, a parameter), a pedigree query returns
a directed acyclic graph of `derives-from` edges (DF-3) rooted at that symbol
and fanning out backwards through all contributing values.

This answers questions such as:

- "What data sources populate `user.token` by the time it reaches the
  authentication check?"
- "Which input parameters can reach the SQL template string passed to
  `db.execute()`?"
- "Does the value returned by `transform()` derive, transitively, from any
  network-read symbol?"

### DF-1.2 — Pedigree through transformations

Pedigree is not restricted to direct assignments. `cgx` traces through
transformations that alter a value while preserving derivation:

```rust
// Example: pedigree through map
let doubled = my_list.map(|x| x * 2);
return doubled;
```

In this example, `doubled` derives from `my_list` through a `mapped`
transformation (see DF-3.2). The pedigree of `doubled` includes `my_list` as
an ancestor, with a `mapped` transformation tag on the intervening
`derives-from` edge. The indirect link — mediated by the closure and the
`map` combinator — is captured by the container/collection model (DF-4).

Without the collection model, a plain backward slice would stop at the
`map` call and fail to connect `doubled` to `my_list`. `cgx` resolves this
by treating `map`, `filter`, `fold`, and equivalent combinators as transparent
for pedigree purposes (DF-4.1).

### DF-1.3 — Fan-out

Pedigree is a fan-out, not a chain. A single value may derive from multiple
sources:

```python
result = combine(user_input, config_value, db_row["field"])
```

The pedigree of `result` fans out to three independent ancestors: `user_input`,
`config_value`, and `db_row["field"]`, each with its own path and
transformation history.

Pedigree queries return the full fan-out DAG. Users can request a summarised
view (direct parents only) or a full transitive closure (all ancestors to
external sources or entrypoints).

---

## DF-2 — Scope Entry/Exit Inventory

For any callable (function or method), `cgx` builds a scope inventory: the
complete set of values that cross the callable's boundary.

### DF-2.1 — Inventory categories

| Category | Description | Examples |
|---|---|---|
| `parameter` | Value passed in by the caller | `fn f(x: i32)`, `def f(user: User)` |
| `return` | Value returned to the caller | `return result`, `Ok(value)`, `(val, err)` in Go |
| `captured-closure` | Variable from an enclosing scope captured by a closure defined in this callable | Rust `move |x|`, Python `def inner(): nonlocal y` |
| `mutation-in` | Field or global that the callable reads from (inbound ambient state) | `self.config`, `GLOBAL_REGISTRY` |
| `mutation-out` | Field or global that the callable writes (outbound mutation) | `self.state = new_state` |
| `io-read` | Callable reads from an external I/O source | `stdin`, file read, network socket read |
| `io-write` | Callable writes to an external I/O sink | `stdout`, file write, HTTP response |
| `exception-out` | Exception type that may propagate to callers | Java checked `throws`, Rust `Err(E)` |

### DF-2.2 — Uses of the scope inventory

The scope inventory powers several query forms (docs/05-queries.md):

- "What values enter `process_payment()` from any call path?"
- "Which functions write to `self.session` without reading it first?"
- "What are all the exit channels of `parse_config()` — what can it return or
  throw?"

The inventory is also the foundation for taint analysis (DF-5): sources and
sinks are defined as specific inventory categories at specific symbols.

### DF-2.3 — Field-sensitivity

The scope inventory is **field-sensitive**: `self.config` and `self.state`
are tracked as distinct abstract locations, not merged under `self`. This
prevents false flow paths between unrelated fields of the same object.

Field-sensitivity is maintained at depth 1 by default (access path `obj.field`).
The depth limit is configurable; deeper paths are truncated with a `truncated`
tag on the provenance record.

---

## DF-3 — Derives-From Edges and Transformation Tags

`derives-from` is a dedicated edge type in the `cgx` graph (see also
docs/03-code-graph-model.md GM-2.2) that encodes value derivation between
symbols.

### DF-3.1 — Edge direction and semantics

A `derives-from` edge is directed from **derived** to **source**:

```
doubled  --derives-from-->  my_list
```

This reads: `doubled` is derived from `my_list`. Following `derives-from` edges
backwards from a symbol yields its pedigree; following them forwards yields
downstream usage (DF-6).

### DF-3.2 — Transformation tags

Every `derives-from` edge carries a **transformation tag** that characterises
how the derived value relates to the source:

| Tag | Meaning | Example |
|---|---|---|
| `identity` | The derived value is the source value, possibly through aliases or moves | `let b = a`, `fn f(x) { x }` |
| `mapped` | The derived value is the result of applying a function element-wise | `list.map(f)`, `iter().map(|x| x * 2)` |
| `aggregated` | The derived value is an aggregate (fold, reduce, sum, count) of the source | `list.fold(0, |acc, x| acc + x)`, `sum(items)` |
| `filtered` | The derived value is a subset of the source | `list.filter(pred)`, `[x for x in xs if p(x)]` |
| `parsed` | The derived value is the result of parsing or deserialising the source | `serde_json::from_str(s)`, `int(input_str)` |
| `formatted` | The derived value is the result of formatting or serialising the source | `format!("{}", val)`, `json.dumps(obj)` |
| `branched` | The derived value is selected from multiple sources by a condition | `if cond { a } else { b }` |
| `composed` | The derived value is assembled from multiple distinct sources | `concat(a, b)`, `struct { x: a, y: b }` |

Transformation tags are not exhaustive; an `other` tag is used when no
specific tag applies, with the producing rule recorded in provenance.

### DF-3.3 — Interprocedural propagation

`derives-from` edges cross function boundaries via **function summaries**.
A summary records, for each output (return value, mutation-out field), which
inputs (parameters, captures, mutation-in fields) it derives from, and with
which transformation tag.

Summaries are computed on demand and cached. They are the interprocedural
analogue of IFDS summary edges (Reps–Horwitz–Sagiv 1995); `cgx` uses an
IDE-style (Sagiv–Reps–Horwitz) lattice to carry transformation tags alongside
reachability.

---

## DF-4 — Container and Collection Models

Standard library container operations (`map`, `filter`, `fold`, `collect`,
`flat_map`, etc.) are treated as transparent for pedigree and taint propagation.
Without explicit models for these operations, a plain dataflow analysis would
lose track of values across container boundaries.

### DF-4.1 — Built-in container/higher-order models

`cgx` ships built-in models for the following categories of stdlib operations:

| Category | Operations modelled | Effect on derives-from |
|---|---|---|
| Element-wise transform | `map`, `collect`, `iter().map()`, Python list comprehension, JS `Array.map` | Output collection derives from input collection with tag `mapped` |
| Filter | `filter`, `retain`, Python `filter()`, JS `Array.filter` | Output collection derives from input collection with tag `filtered` |
| Fold / aggregate | `fold`, `reduce`, `sum`, `count`, `any`, `all` | Output scalar derives from input collection with tag `aggregated` |
| Flat map | `flat_map`, `chain`, `flatten` | Output collection derives from input collection(s) with tag `mapped` |
| Collect / into | `.collect::<Vec<_>>()`, `list()`, `Array.from()` | Output collection derives from iterator/source with tag `identity` |
| Field access on elements | `items.map(|x| x.field)` | Propagates field-sensitivity through the map: output derives from `element.field` of source collection |

The canonical example from the product prompt:

```rust
return my_list.map(|x| x * 2);
```

`cgx` models this as: the return value `derives-from` `my_list` with tag
`mapped`. Queries asking for the pedigree of the return value will include
`my_list` as an ancestor. Without the container model, the `map` call would
appear as an opaque function and the link would be severed.

### DF-4.2 — Container abstraction

Internally, a collection's contents are represented as a single abstract
"element" node (smashed abstraction). This is the standard approach used by
taint tools (FlowDroid's container models; IFDS access-path-aware analysis).
It is sound for reachability purposes and imprecise for index-level
distinctions (e.g. "which index of the array").

Users needing element-index precision should note this limit. The provenance
record on container-derived edges carries the `smashed-collection` tag to
indicate this.

### DF-4.3 — User-defined container models

For project-specific container types (custom wrappers, domain-specific
collections), users can declare additional models in `cgx.toml`:

```toml
[[container-models]]
type   = "myapp::BoundedQueue"
ops    = [
  { method = "push",    effect = "stores", tag = "identity" },
  { method = "pop",     effect = "yields", tag = "identity" },
  { method = "drain",   effect = "yields", tag = "identity" },
]
```

---

## DF-5 — Taint Analysis

Taint analysis tracks whether a value originating at a **source** can reach a
**sink** without passing through a **sanitizer**. Sources, sinks, and
sanitizers are user-configurable; `cgx` ships sensible defaults for common
security patterns.

### DF-5.1 — Model

Taint propagation in `cgx` follows the IFDS (Reps–Horwitz–Sagiv) framework:

1. **Seed** taint at declared sources (specific function return values, parameter
   positions, or field reads).
2. **Propagate** taint interprocedurally via `derives-from` edges and function
   summaries, respecting field-sensitivity (DF-2.3) and container models (DF-4).
3. **Intercept** taint at declared sanitizers (specific call sites whose output
   is considered clean).
4. **Report** any path from a tainted source to a declared sink that does not
   pass through a sanitizer.

Taint edges respect edge conditions (docs/03-code-graph-model.md GM-3): taint
on an `exception`-conditioned path is reported separately from taint on an
`always`-conditioned path, so users can distinguish "taint reaches sink only in
error handling" from "taint reaches sink on the happy path."

### DF-5.2 — Source, sink, and sanitizer declarations

Sources, sinks, and sanitizers are declared in `cgx.toml` or passed as query
arguments. The schema:

```toml
[[taint.sources]]
symbol   = "std::io::stdin"
category = "user-input"

[[taint.sources]]
symbol   = "myapp::db::Row::get_field"
category = "database-read"

[[taint.sinks]]
symbol   = "myapp::db::Connection::execute"
param    = 0          # first argument is the sink parameter
category = "sql-injection"

[[taint.sanitizers]]
symbol   = "myapp::validation::escape_sql"
category = "sql-injection"
```

Multiple categories can be active simultaneously. A query can restrict to
a named category or return all.

### DF-5.3 — Default source/sink sets

`cgx` ships default sets for common vulnerability classes. These are starting
points; projects should review and extend them.

| Category | Default sources | Default sinks |
|---|---|---|
| `injection` | HTTP request params, stdin, file reads | SQL query strings, shell command strings, eval calls |
| `deserialization` | `serde_json::from_*`, `pickle.loads`, `JSON.parse` | Deserialized objects used as code/config |
| `path-traversal` | HTTP request params | `File::open`, `std::fs::read`, `open()`, `readFile` |
| `secret-exposure` | Env vars, secret stores | Logging calls, HTTP responses, stdout |

Default sets are language-specific and are only activated for detected
languages.

### DF-5.4 — Field-sensitivity in taint

Taint is tracked field-sensitively (DF-2.3). Tainting `user.name` does not
taint `user.id` unless a data-flow path connects them. This reduces false
positives in object-heavy codebases.

Access paths are k-limited (default k = 3: e.g. `request.body.field`). Deeper
paths are truncated and tagged `truncated-access-path` in provenance.

### DF-5.5 — Taint and edge conditions

Taint findings are annotated with the edge conditions along the path (GM-3). A
finding where all edges are `always`-conditioned is reported as `taint:certain`.
A finding where the path includes `exception`- or `panic`-conditioned edges is
reported as `taint:exception-path`. Users can filter by this attribute.

---

## DF-6 — Backward and Forward Slicing

Program slicing computes the set of symbols that affect or are affected by a
given symbol at a given point. `cgx` exposes both directions as first-class
query operations (syntax in docs/05-queries.md).

### DF-6.1 — Backward slice (pedigree)

A **backward slice** from symbol `S` at call site `P` returns all symbols whose
values may affect `S` at `P`, interprocedurally and transitively.

This is the precise formulation of **pedigree** (DF-1): the backward slice is
the pedigree graph. It is computed via traversal of `derives-from` edges in
reverse, using function summaries (DF-3.3) to cross call boundaries correctly.

Use cases:

- "What populated `auth_token` by the time it reaches `verify_token()`?"
- "Which function arguments or I/O reads can influence the value of
  `query_string`?"
- "Did any network-sourced value transitively contribute to `config.max_retry`?"

### DF-6.2 — Forward slice (downstream usage)

A **forward slice** from symbol `S` at call site `P` returns all symbols that
may be affected by `S`, interprocedurally and transitively.

Forward slicing follows `derives-from` edges in the forward direction. It
answers impact-analysis questions:

- "If `user.role` changes, what downstream computations are affected?"
- "Which sinks can a value originating at `parse_input()` reach?"
- "What functions or fields transitively depend on `config.timeout`?"

### DF-6.3 — Slicing and edge conditions

Slices can be restricted by edge condition. Passing `--on-exception-path`
restricts the slice to paths containing at least one `exception` or `panic`
edge, equivalent to "reachable via the failure path only." This directly
supports the path-relative transience queries described in
docs/03-code-graph-model.md GM-4.

### DF-6.4 — Context-sensitivity

`cgx` computes interprocedural slices using call/return matched traversal
(System Dependence Graph summary edges). This is context-sensitive at the
function boundary: a return value is only propagated to the call site that
invoked the callee, not to all callers. This prevents spurious flow paths
across unrelated call chains (the classic "same callee, different callers"
infeasible path problem).

---

## DF-7 — Instance-Level Analysis: Never-Referenced Members

`cgx` can answer the question: "Which properties or methods of *this specific
instance* are never referenced by any code reachable downstream from the point
where this instance is created?"

This is a forward slice on the instantiation site, restricted to member-access
edges.

### DF-7.1 — Allocation-site abstraction

Static analysis identifies instances by their **allocation site** (the
expression that creates the object: `new T()`, `T { }`, `let x = T::new()`).
All runtime objects produced by one allocation site share one abstract identity.
This is the standard allocation-site abstraction used in points-to analysis.

### DF-7.2 — Computation

Given an allocation site `A` of type `T`:

1. Compute the forward slice from `A`, restricted to `reads-field`,
   `writes-field`, and `calls` edges where the receiver is derived from `A`.
2. Collect all methods and fields of `T` accessed in the slice:
   `accessed = { m | ∃ call site in slice targeting T::m }`.
3. Collect all declared members: `declared = methods(T) ∪ fields(T)`.
4. `never-referenced = declared − accessed`.

The result is the set of members declared on `T` that no code reachable from
`A` ever touches.

### DF-7.3 — Worked example

```java
class Report {
  String title;
  String body;
  String footnotes;

  void render() { ... }
  void export()  { ... }
  void archive() { ... }
}

// Allocation site A:
Report r = new Report();
r.title = "Q3";
r.body  = content;
r.render();
// r is passed to log(r) and then goes out of scope
```

Forward slice from `A` accesses: `title`, `body`, `render`.

`never-referenced = { footnotes, export, archive }`.

`cgx` reports these three as never-referenced downstream from this instance.

### DF-7.4 — Confidence and escaping objects

Precision depends on whether the instance escapes its allocating scope:

| Scenario | Confidence |
|---|---|
| Instance is local and does not escape (not returned, not stored to field, not passed out) | `certain` — all uses are within the local scope and fully enumerable |
| Instance escapes to callers but the escape path is tracked | `probable` — uses outside the tracked escape path may exist |
| Instance escapes through a field store, a collection, or an opaque call | `possible` — downstream uses may be invisible |

The provenance record on each `never-referenced` finding states which scenario
applies and at which point (file:line) the escape, if any, was detected.

Users should treat `possible`-confidence findings as hypotheses requiring
manual confirmation, not as definitive dead-code reports.

### DF-7.5 — Interaction with inheritance and traits

For a polymorphic allocation site (`Box<dyn Trait>`, `Animal a = new Dog()`),
`never-referenced` is computed over the concrete type at the allocation site
(e.g. `Dog`), not the declared type (`Animal`). If the concrete type is
ambiguous (`possible` confidence), the result is labelled accordingly.

---

## DF-8 — Soundiness: What cgx Labels `possible` vs Claims

`cgx` is soundy (see also docs/03-code-graph-model.md — Known Limits). This
section states the specific limits that affect dataflow and provenance features.

### DF-8.1 — What `cgx` claims

`cgx` claims:

- Every `derives-from` edge at confidence `certain` or `probable` represents a
  real data-flow path that exists in a statically sound analysis of the
  resolved types.
- Backward and forward slices are **over-approximations**: they may include
  paths that are not feasible at runtime, but they do not miss paths that are
  statically resolvable.
- Container/collection models are **summaries**: they capture the derivation
  relationship but not index-level precision (DF-4.2).

### DF-8.2 — What `cgx` labels `possible`

`cgx` uses the `possible` confidence label for dataflow findings where:

- The source or sink is reached through a virtual dispatch with multiple
  candidates and no type narrowing.
- The flow path passes through a closure or callback whose target is
  over-approximated.
- A container element is tracked through a smashed abstraction (DF-4.2).
- The flow passes through a field at access-path depth > k (truncated).

### DF-8.3 — Explicit blind spots

The following constructs create gaps in the dataflow graph. `cgx` does not
silently ignore them; it emits cut-marker edges (docs/03-code-graph-model.md
GM-5.3) so that users know the graph is incomplete at those points:

| Blind spot | Cut marker | Implication for dataflow |
|---|---|---|
| Reflection (`Method.invoke`, `eval`, `__import__`) | `reflective` | Values passed through reflection are not tracked |
| Dependency injection container binding | `via-DI` | Values wired by the DI container are not tracked without user-supplied models |
| FFI / native code | `via-FFI` | Data transformations inside native code are opaque |
| Runtime code generation | `dynamic` | Dynamically generated callees are not modelled |
| Pointer aliasing beyond access-path depth k | `truncated-access-path` | Flows through deeply aliased fields may be missed |

### DF-8.4 — No path-feasibility guarantee

`cgx` does not perform symbolic execution or SMT-based path-feasibility
checking. A path reported in a backward or forward slice is "reachable in the
graph" — meaning no structural proof of infeasibility exists. Some such paths
may be infeasible at runtime due to contradictory branch conditions. This is an
inherent property of static analysis without a constraint solver; it is a known,
accepted limit, not a defect.

---

*Next:* docs/05-queries.md — query language syntax, worked examples for every
question class, and the canonical reachability and pedigree query forms.
