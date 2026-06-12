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
- DF-9: Join (union) pedigree nodes
- DF-10: Transformation kinds on derives-from edges
- DF-11: Typed taint labels and class-matched sanitization
- DF-12: Trust boundaries: source classes and sink classes
- DF-13: Secret pedigree
- DF-14: Nullability and optionality flow
- DF-15: Numeric narrowing and widening
- DF-16: Aliasing and escape
- DF-17: Mutability model (binding/value/alias; writes-param/writes-receiver; mutation fan-out; sanitization invalidation)
- DF-18: Function values and closures (pedigree-based indirect-call resolution; capture edges; deferred execution)
- DF-19: Lineage type reconstruction (up/down/sideways constraint query; literals as anchors; contradiction detection)
- DF-20: Non-call dataflow linkages (channels; import-time edges; deferred-execution marker)

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

## DF-9 — Join (Union) Pedigree Nodes

**Status: core-extension** — the existing `derives-from` graph already
represents fan-out pedigree (DF-1.3); join nodes make multi-source assembly
points explicit and queryable rather than implicit in the fan-out DAG.

### DF-9.1 — Motivation

DF-1.3 shows that a single value may derive from multiple independent sources.
In the current model those multiple inbound `derives-from` edges meet at the
derived-value node, but the assembly point itself carries no identity. This is
adequate for reachability queries ("does any tainted source reach this value?")
but inadequate for questions that reason about the *combination*: "which
specific sources were joined here, and can they be queried as a set?"

The canonical motivating example: an object assembled from argument `id` and
the result of `lookup(id)` has both values in its pedigree. When `id` is
network-tainted and `lookup(id)` returns a database row, the assembled object
carries taint from two distinct source classes (`network` and `db`). A query
asking "does this object derive from a network source AND a db source jointly?"
requires the join to be a named, addressable node.

### DF-9.2 — Join node representation

A **join node** is a synthetic node in the pedigree graph inserted at any point
where two or more distinct `derives-from` edges converge on a single derived
value as a result of structural assembly (not conditional branching, which uses
tag `branched`).

Join nodes carry the following attributes:

| Attribute | Type | Description |
|---|---|---|
| `kind` | `join` | Fixed node kind distinguishing join nodes from value/symbol nodes |
| `arity` | integer | Number of distinct inbound pedigree branches |
| `site` | file:line | The assembly expression (struct literal, function call, concatenation, etc.) |
| `provenance` | provenance record | Producing rule and confidence (GM-6.1) |

The join node replaces the implicit convergence: instead of multiple
`derives-from` edges arriving at the derived-value node, each inbound source
connects to the join node, and a single `derives-from` edge connects the join
node to the derived value.

```
argument `id`          --derives-from(copy)-->  [join node @ line 42]
result of lookup(id)   --derives-from(copy)-->  [join node @ line 42]
                                                       |
                                              derives-from(composed)
                                                       |
                                               assembled object
```

### DF-9.3 — Relationship to existing edges

Join nodes extend DF-1 and DF-3; they do not replace or redefine them. Every
`derives-from` edge incoming to a join node follows the same transformation-tag
rules as any other `derives-from` edge (DF-3.2, and the expanded kind vocabulary
in DF-10). The edge from join node to assembled value carries tag `composed`
(existing DF-3.2 vocabulary).

For conditional selection (`if cond { a } else { b }`), the tag remains
`branched` (DF-3.2) and no join node is inserted. Join nodes are used only for
structural assembly where both branches contribute simultaneously.

### DF-9.4 — Queryability

Join nodes are queryable via the standard pedigree API (docs/05-queries.md Q-5)
with an optional `--join-nodes` flag that expands them in the returned DAG.
Query predicates can address join nodes directly:

- "Does this value have a join node whose inbound sources include both a
  `network` source class and a `db` source class?" (typed taint — see DF-11)
- "How many distinct pedigree branches converge at this join node?"
- "What is the assembly site of this join?"

These predicates are central to the typed taint queries in docs/05-queries.md
Q-23.

---

## DF-10 — Transformation Kinds on Derives-From Edges

**Status: core-extension** — this section replaces the `transformation tags`
vocabulary introduced in DF-3.2 with a precise, security-motivated taxonomy.
The old tag names remain valid aliases during any migration period; new work
should use the kind names defined here.

### DF-10.1 — Why transformation kind matters

DF-3.2 established that every `derives-from` edge carries a tag describing how
the derived value relates to the source. The tag vocabulary in DF-3.2 is
structurally oriented (mapped, aggregated, filtered, etc.). Security analysis
needs a finer and differently-oriented vocabulary: the question is not "what
container operation occurred?" but "does this transformation preserve, introduce,
or neutralise a security property?"

The clearest example is SQL injection. Consider two ways to build a query string:

```python
# concat: taint-preserving — user_input lands verbatim in the SQL string
query = "SELECT * FROM users WHERE name = '" + user_input + "'"
db.execute(query)

# parameterize: taint-neutralising for sql sinks — user_input becomes a bind parameter
db.execute("SELECT * FROM users WHERE name = ?", [user_input])
```

Both operations derive `query` (or the second argument) from `user_input`. In
the DF-3.2 model, both would carry tag `composed`. With transformation kinds,
the first carries `concat` (a taint-preserving, injection-enabling kind) and
the second carries `parameterize(sql)` (a taint-neutralising kind for `sql`
sinks). The distinction is load-bearing for the SQL-injection query.

### DF-10.2 — Canonical transformation kind vocabulary

The following kinds are built in. The vocabulary is user-extensible via config
(see DF-10.3).

| Kind | Description | Security relevance |
|---|---|---|
| `copy` | Value is passed through without structural change; includes aliases, moves, identity functions | Fully taint-preserving |
| `projection` | A field or subset of the source is extracted | Taint-preserving for the projected field; other fields are not carried |
| `aggregation` | Multiple source values are combined into a scalar (fold, reduce, sum, count) | Taint-preserving: if any input is tainted, the aggregate is tainted |
| `parse` | Source is parsed or deserialised into a typed value | Taint-preserving: attacker-controlled bytes become attacker-controlled structure |
| `serialize` | Source is serialised to a byte/string representation | Taint-preserving: attacker-controlled structure becomes attacker-controlled bytes |
| `encode(class)` | Source is encoded for a specific target context (e.g. `encode(html)`, `encode(url)`) | Taint-cleared for the named sink class; taint-preserving for all others |
| `decode(class)` | Source is decoded from a specific encoding (e.g. `decode(base64)`) | Taint-preserving; may expand attack surface |
| `truncate` | Source value is shortened or capped in length | Taint-preserving; may affect exploit feasibility |
| `narrow` | Numeric value is converted to a smaller type or range (e.g. `u64 → u32`) | Taint-preserving; may introduce overflow (see DF-15) |
| `widen` | Numeric value is converted to a larger type or range (e.g. `i32 → i64`) | Taint-preserving |
| `arith` | Arithmetic operation is applied (add, subtract, multiply, divide, shift) | Taint-preserving; attacker-influenced arithmetic feeds downstream uses |
| `concat` | Two or more string/byte values are concatenated | Taint-preserving; the classic injection-enabling operation |
| `parameterize(class)` | Source value is passed as a bind parameter for a given sink class (e.g. `parameterize(sql)`) | Taint-cleared for the named sink class; taint-preserving for all others |
| `validate(class)` | Source value is asserted to conform to a constraint class (e.g. `validate(uuid)`, `validate(integer)`) | May clear taint depending on config — see DF-11.3 |
| `coerce(from,to)` | Source value undergoes an implicit conversion that changes its type (e.g. `coerce(string,int)`, `coerce(any,User)`) | Taint-preserving; marks the edge where a value's type changes — see DF-17.5 for mutation interaction and DF-19.4 for type-reconstruction role |

The `encode(class)` and `parameterize(class)` kinds carry a class argument that
names the sink class they address. The `coerce(from,to)` kind carries two type
arguments: the source type and the destination type of the implicit conversion.
This enables queries that trace type juggling across trust boundaries — for example,
a tainted string coerced to a boolean in a PHP authentication check
(`"0e123" == "0"` evaluating as equal after numeric coercion) is visible as a
`coerce(string,int)` edge in the pedigree. A parameterisation that uses SQL bind
parameters neutralises SQL injection but has no effect on a shell-sink: taint
survives on any `shell`-class path.

### DF-10.3 — User-extensible kinds

Projects can declare additional transformation kinds in `cgx.toml`:

```toml
[[transforms]]
kind             = "html-template-escape"
taint-preserving = false
sink-classes     = ["html"]   # clears taint only for html sinks
```

Custom kinds are recorded in provenance alongside built-in kinds and are
available in all query predicates.

### DF-10.4 — Relationship to DF-3.2 tags

The DF-3.2 tags (`identity`, `mapped`, `aggregated`, `filtered`, `parsed`,
`formatted`, `branched`, `composed`) remain supported as structural tags. They
answer "what container/structural operation occurred?" The DF-10 kinds answer
"what is the security effect of this operation?" Both attributes coexist on
a `derives-from` edge when applicable. New analysis rules should prefer the
DF-10 kind vocabulary for security predicates.

---

## DF-11 — Typed Taint Labels and Class-Matched Sanitization

**Status: core-extension** — this section refines the taint model in DF-5.
It does not replace DF-5; it adds the class-matching requirement to
sanitization, the taint-survival rules through transformation kinds, and the
extensible lattice.

### DF-11.1 — Taint labels are typed

In DF-5 the `category` field on a source or sanitizer declaration is a
free-form string used for filtering. DF-11 formalises the category as a
**source class** (see DF-12 for the canonical list) and makes the class a
first-class attribute of the taint label propagated on the dataflow graph.

Every taint label propagating through the graph carries:

| Attribute | Description |
|---|---|
| `source-class` | The class of the originating source (e.g. `network`, `env`) |
| `sink-class` | The class of the target sink that this label is relevant to (e.g. `sql`, `html`) |
| `confidence` | `certain` / `probable` / `possible` (GM-5.1) |
| `path-condition` | Edge conditions along the path (GM-3) |

### DF-11.2 — Class-matched sanitization

A sanitizer clears a taint label **only if** its declared sink class matches
the sink class of the taint label. Mismatched sanitizers are transparent.

**Motivating example:**

```python
safe_for_html = html_escape(user_input)   # sanitizer class: html
db.execute("SELECT * FROM t WHERE id = " + safe_for_html)  # sink class: sql
```

`html_escape` is declared with sanitizer class `html`. The taint label
propagating toward the `sql`-class sink carries `sink-class: sql`. Because
`html ≠ sql`, the sanitizer does not clear the taint. `cgx` reports the SQL
injection finding even though the value passed through a sanitizer.

This is the fundamental error that class-unaware taint models make: they
treat any sanitizer as a clearing event regardless of context, producing
false negatives on cross-context reuse.

### DF-11.3 — Taint survival through transformation kinds

The survival of a taint label through a `derives-from` edge depends on the
edge's transformation kind (DF-10):

| Kind | Taint survival rule |
|---|---|
| `copy`, `projection`, `aggregation`, `parse`, `serialize`, `decode(class)`, `truncate`, `narrow`, `widen`, `arith`, `concat`, `coerce(from,to)` | Taint is **preserved** — the label propagates to the derived value unchanged |
| `encode(class)` | Taint is **cleared** for sink classes matching `class`; preserved for all others |
| `parameterize(class)` | Taint is **cleared** for sink classes matching `class`; preserved for all others |
| `validate(class)` | Taint survival is **config-dependent** — the project declares whether `validate(class)` clears taint for matching sink classes; default is to preserve (conservative) |

The `branched` structural tag (DF-3.2): taint propagates from any tainted
inbound branch. The label is annotated with `path-condition: conditional`.

### DF-11.4 — Extensible taint lattice

The set of source classes, sink classes, and sanitizer classes is
user-extensible. Projects define additional entries in `cgx.toml`:

```toml
[[taint.source-classes]]
name        = "payment-api"
description = "Values returned from the payment processor API"

[[taint.sink-classes]]
name        = "audit-log"
description = "Values written to the immutable audit log"

[[taint.sanitizers]]
symbol      = "myapp::redact::scrub_pii"
sink-class  = "audit-log"
```

Built-in source and sink classes are listed in DF-12. The full class lattice
(built-ins plus project additions) is emitted in the index manifest so queries
can enumerate it.

### DF-11.5 — Reconciliation with DF-5

DF-5.2 uses a free-form `category` field on source, sink, and sanitizer
declarations. That field is now interpreted as a source class or sink class per
the DF-12 vocabulary. Existing configurations that use category names matching
DF-12 built-ins require no migration. Configurations using non-standard names
should add a corresponding `[[taint.source-classes]]` or
`[[taint.sink-classes]]` entry to make the class explicit.

---

## DF-12 — Trust Boundaries: Source Classes and Sink Classes

**Status: core-extension** — sources and sinks are already present in DF-5;
this section assigns the canonical class vocabulary, defines trust boundaries,
and documents the user-extension mechanism.

### DF-12.1 — Source classes (built-in)

A **source class** identifies the trust boundary that a value crossed to enter
the program. Values from lower-trust sources require more scrutiny before
reaching sensitive sinks.

| Source class | Description | Typical symbols |
|---|---|---|
| `network` | Values received from a network socket, HTTP request, RPC call, or any remote endpoint | HTTP request body/params/headers, gRPC payloads, raw socket reads |
| `file` | Values read from the filesystem | `File::open`, `fs::read_to_string`, `open()`, `readFile()` |
| `env` | Values from environment variables or the process environment | `std::env::var`, `os.environ`, `process.env` |
| `cli` | Values from command-line arguments | `args().collect()`, `sys.argv`, `os.Args` |
| `db` | Values returned from a database query or ORM | `Row::get`, ORM `.find()`, raw SQL result rows |
| `deserialization` | Values produced by deserialising an externally-supplied byte sequence | `serde_json::from_str`, `pickle.loads`, `JSON.parse` |
| `ipc` | Values received via inter-process communication (pipes, shared memory, Unix domain sockets, message queues) | `recv()`, named-pipe reads, mmap-backed buffers |

All built-in source classes are user-extensible (DF-11.4).

### DF-12.2 — Sink classes (built-in)

A **sink class** identifies the kind of operation that a value reaches. Sink
classes determine which sanitizers are relevant (DF-11.2) and which
vulnerability class a finding maps to.

| Sink class | Vulnerability class | Description |
|---|---|---|
| `sql` | SQL injection | Values interpolated into SQL query strings |
| `shell` | Command injection | Values passed to shell execution (`exec`, `system`, subprocess) |
| `path` | Path traversal | Values used to construct filesystem paths (`File::open`, `readFile`) |
| `html` | Cross-site scripting (XSS) | Values written to HTML output without encoding |
| `header` | Header injection | Values written to HTTP response headers |
| `redirect-url` | Open redirect | Values used as HTTP redirect targets |
| `format-string` | Format-string injection | Values used as format strings (`printf`, `format!`) |
| `regex` | ReDoS | Values used to compile regular expressions at runtime |
| `deserialize` | Deserialization gadget chains | Values passed to deserialization entry points |
| `eval` | Code injection | Values passed to `eval`, `exec`, `dlopen`, or equivalent |
| `log` | Secret / PII exposure | Values written to log output (see also DF-13) |
| `net-request` | SSRF | Values used to construct outbound network request URLs or targets |

All built-in sink classes are user-extensible (DF-11.4). Sanitizer classes
mirror sink classes: a sanitizer named for class `sql` clears `sql`-destined
taint labels (DF-11.2).

### DF-12.3 — Trust boundaries

A **trust boundary** is a point in the data flow where a value transitions from
a lower-trust context (a source class) into program-controlled processing. Every
source class defines an implicit trust boundary at the point where the value is
first introduced into the graph.

Trust boundaries are surfaced as queryable attributes. Queries can ask:
- "Which values crossing the `network` trust boundary reach any `sql`-class sink?"
- "Does any path from a `deserialization` source reach an `eval` sink?"

Trust-boundary crossing is also the point at which taint labels are seeded
(DF-11.1). The boundary node is recorded in provenance so the exact entry point
is traceable to a file:line in the index.

Cross-references: docs/03-code-graph-model.md GM-14 (code-trust boundaries —
unsafe, FFI, dependency edges, reflection) addresses compile-time and
link-time trust concerns that are orthogonal to the runtime data-flow trust
boundaries defined here.

---

## DF-13 — Secret Pedigree

**Status: core-extension** — a specialisation of DF-12 (source class `secret`
is a narrowing of `env` and external secret-store reads) plus typed taint
(DF-11) targeting the `log`, `net-request`, `serialize`, and
`format-string` sink classes.

### DF-13.1 — What counts as key material

Secret pedigree tracks values that originate from:

- Environment variables conventionally used for secrets (`*_KEY`, `*_SECRET`,
  `*_TOKEN`, `*_PASSWORD`, `*_CREDENTIAL`) — detected heuristically and
  overridable in config
- Explicit secret-store reads (e.g. `aws_secretsmanager::get_secret_value`,
  `vault::read`, `k8s::secret::get`) — declared via `[[taint.sources]]` with
  source class `secret`
- Private key and certificate material (PEM reads, key-derivation function
  outputs) — declared similarly

Projects declare secret sources in `cgx.toml`:

```toml
[[taint.sources]]
symbol       = "myapp::config::get_api_key"
source-class = "secret"
```

### DF-13.2 — Sensitive sinks for secrets

Secret-labelled taint is tracked toward sinks that could expose the material:

| Sink class | Exposure risk | Example |
|---|---|---|
| `log` | Secret written to log output | `log::info!("key={}", api_key)` |
| `format-string` | Secret interpolated into a format string that may reach log/output | `format!("Authorization: Bearer {}", token)` |
| `net-request` | Secret sent as part of an outbound request beyond its intended endpoint | URL-embedded credentials, SSRF carrying auth headers |
| `serialize` | Secret included in a serialised payload that may be stored or transmitted | JSON response body carrying raw key material |

Error-message and exception-message construction is treated as equivalent to
`log` for this purpose: a value that flows into an error string, an exception
constructor, or a `Debug`/`Display` implementation that reaches a log sink is
reported as a secret-exposure finding.

### DF-13.3 — Findings

Secret-exposure findings carry:
- `source-class: secret`
- The originating symbol (file:line) and its declaration in `cgx.toml`
- The sink (file:line) and the sink class
- The full pedigree path including any transformation kinds (DF-10) — a secret
  that is `concat`-ed into a string before logging is still reported

---

## DF-14 — Nullability and Optionality Flow

**Status: core-extension** — nullability is representable in the current graph
via edge conditions and value attributes; this section specifies the standard
pedigree queries and the dominating-check requirement.

### DF-14.1 — Where nullable values arise

A value is nullable (may be `null` / `None` / zero-value / `Err`-default at
runtime) if it originates from any of:

- A function whose return type is `Option<T>`, `Result<T, E>`, a nullable
  reference type (`T?`, `*T`), or an interface type whose runtime value may be
  nil
- A field declared with a nullable type
- A map/dictionary lookup that may not find the key (e.g. `map[key]` in Go,
  `.get(key)` returning `Option` in Rust)
- A type assertion or cast that may fail at runtime (dynamic cast)
- An external source (source class `db`, `deserialization`, `network`) where
  fields may be absent or null in the incoming data

`cgx` tracks nullable values by recording the producing expression on the
`derives-from` path and annotating it with `nullable: true` in provenance.

### DF-14.2 — Dominating-check requirement

A null dereference is possible when a nullable value is used without a
null-check **dominating** every use. **Dominates** here has the standard
control-flow meaning: node A dominates node B if every path from the entry
point to B passes through A (see docs/05-queries.md Q-20, which exposes
dominance as a query predicate).

A **dominating check** is a call or branch that:
- Returns a non-nullable type or branches on the value being non-null
  (e.g. `if x != nil`, `if let Some(v) = x`, `x.ok_or(...)`)
- Appears on every path from the nullable origin to the use site

`cgx` reports a `nullable-use` finding when a nullable value is used (via a
`derives-from` or direct-use edge) without a dominating check on every
inbound path. The finding carries the nullable origin (file:line) and the
use site (file:line).

### DF-14.3 — Propagation through transformation kinds

Nullability propagates through all taint-preserving transformation kinds
(DF-11.3). A `projection` from a nullable source produces a nullable value.
A `concat` that includes a nullable string component produces a nullable string.
An `aggregation` over a collection that may contain null elements may produce
a null result depending on the aggregation function (language-specific;
see docs/08-language-support.md LS-7 for per-language rules).

---

## DF-15 — Numeric Narrowing and Widening

**Status: core-extension** — the `narrow` and `widen` transformation kinds
(DF-10) are the representation; this section specifies the overflow-to-alloc
detection pattern and the pedigree annotations.

### DF-15.1 — Narrowing and widening on pedigree paths

A pedigree path that includes one or more `narrow` edges carries a potential
integer overflow or truncation. `cgx` records the narrowing chain on the
`derives-from` path, annotating each `narrow` edge with:

| Attribute | Description |
|---|---|
| `from-type` | Source numeric type (e.g. `u64`, `usize`) |
| `to-type` | Derived numeric type (e.g. `u32`, `u16`) |
| `site` | file:line of the conversion expression |

### DF-15.2 — Overflow-to-alloc: the canonical pattern

The canonical security-relevant instance of numeric narrowing is the
overflow-to-alloc pattern:

```rust
let count: u64 = user_input.parse()?;   // source: network/cli
let size: u32  = count as u32;           // narrow: u64 → u32 (may overflow)
let buf        = vec![0u8; size as usize]; // allocation size derived from narrowed value
```

If `count` is attacker-controlled and exceeds `u32::MAX`, the narrowing wraps
to a small value. The allocation succeeds with a much smaller buffer than
intended, and subsequent writes into `buf` using the original `count` value
overflow the buffer.

`cgx` detects this pattern as a pedigree path where:
1. A `narrow` edge appears between a tainted source and a value used as an
   allocation size or buffer length.
2. The taint label carries a source class that crosses a trust boundary
   (DF-12.1).

The finding includes the full narrowing chain and the allocation site.

### DF-15.3 — Widening

Widening (`narrow` → `widen`) does not introduce overflow but may affect
sign semantics. A `widen` from a signed to an unsigned type on a tainted
path is flagged when the widened value feeds an allocation size or index
(sign-extension to a large unsigned value).

---

## DF-16 — Aliasing and Escape

**Status: schema-room** — the escape and aliasing questions are representable
in principle using existing graph elements (`derived-from`, field stores, closure
captures, the DF-7 instance analysis), but full alias analysis is the most
analysis-expensive feature in this family. The schema reserves representation;
complete inference is deferred. Be explicit about precision limits when
interpreting findings.

### DF-16.1 — Escape categories

A value **escapes** its allocating or declaring scope when any of the following
occurs:

| Escape kind | Description | Example |
|---|---|---|
| `closure-capture` | A variable from an enclosing scope is captured by a closure (by reference or by move) | Rust `move |x| { uses outer_val }`, Python `def inner(): nonlocal y` |
| `stored-in-field` | A reference or value is written into a struct field or global that outlives the current stack frame | `self.cache = &local_buf`, `GLOBAL.store(ptr)` |
| `returned` | A reference to a local value is returned to the caller | Rust borrow-checker catches this; other languages may not |
| `passed-to-opaque` | A value is passed to a call site where the callee is opaque (FFI, reflection, dynamic dispatch) | `c_function(&local)`, `reflect.ValueOf(x).Set(...)` |

Escape is already partially tracked in DF-7.4 (instance escape for
never-referenced member analysis). DF-16 generalises the escape concept to
arbitrary values and references, not just object instances.

### DF-16.2 — Mutation through alias

When a value escapes into an alias, mutations through the alias affect the
original. `cgx` represents alias relationships as `derives-from` edges with
kind `copy` (for moves/identity) or as structural edges depending on the
language model. Mutation-through-alias findings arise when:

1. A mutable reference or pointer to a value is stored or captured (`closure-capture` or `stored-in-field` escape).
2. A `mutation-out` edge (DF-2.1) is recorded on the alias.
3. The mutation is not visible at the use site of the original value.

This is the cause of use-after-invalidation bugs (e.g. iterator invalidation,
Rust `Vec` reallocation through an aliased `&mut`).

### DF-16.3 — Precision and analysis cost

Full alias analysis (Andersen-style or Steensgaard-style points-to analysis)
is quadratic to cubic in the number of abstract locations and is the most
expensive analysis in this family. `cgx` currently uses the access-path
abstraction (DF-2.3, DF-8.3) with depth limit k, which handles the common
cases (direct field aliases, single-level captures) but misses aliases through
deeply nested structures.

Findings involving aliasing carry confidence `possible` unless the alias chain
is fully resolved within the access-path depth limit, in which case `probable`
or `certain` applies (GM-5.1).

Users should treat aliasing findings as requiring manual confirmation more
often than other finding classes. The cut-marker mechanism (DF-8.3,
`truncated-access-path`) surfaces the points where alias tracking was
truncated.

### DF-16.4 — Relationship to DF-7

DF-7.4 already classifies instance escape into three confidence tiers. DF-16
generalises this: the same escape categories apply to any value, and the same
confidence rules apply. The DF-7 analysis uses the DF-16 escape categorisation
internally; results reported by DF-7 will carry the escape kind from DF-16.1.

---

## DF-17 — Mutability Model

**Status: core-extension** (binding and value mutability) / **schema-room** (alias mutability) — the
`mutation-out` inventory category (DF-2.1) and `derives-from` edges already represent mutation in
the current graph; this section formalises the three-level model, adds the `writes-param(i)` and
`writes-receiver` effect-lattice extensions, defines mutation fan-out as the outbound dual of
pedigree, and states the sanitization-invalidation rule that refines DF-11. The alias level inherits
DF-16's `schema-room` status because full alias analysis cost applies.

### DF-17.1 — Three levels of mutability

Languages conflate three distinct concepts under "mutability." `cgx` models them separately:

| Level | What it governs | Immutable examples | Mutable examples |
|---|---|---|---|
| `binding` | Whether the **name** can be reassigned to a different value | `const` (JS/TS), `final` (Java), `let` (Rust, non-`mut`) | `var` (JS/Go), `let mut` (Rust), unqualified locals |
| `value` | Whether the **object's internal state** can change | `Object.freeze()`, frozen dataclasses (Python), `record` (Java 16+), Rust `&T` (shared ref), `const` member functions (C++) | Regular classes, `&mut T` (Rust), mutable structs |
| `alias` | Whether **another reference** can mutate the value through a separate binding | Rust ownership system statically prevents this at `certain` confidence | Shared references in most other languages; closures capturing by ref; values passed to concurrent tasks |

A binding that is `binding`-immutable may still be `value`-mutable: a `final List<String>` in Java
cannot be reassigned but can have elements appended. The three levels compose independently.

Alias mutability composes with DF-16 escape and docs/03 GM-10 suspension: a value-mutable value that
escapes its allocating scope (DF-16.1) and crosses a suspension point (GM-10) can be mutated by
another task between the suspension and the next use. This is the precise three-ingredient signature
for TOCTOU vulnerabilities.

### DF-17.2 — Effect-lattice extensions: writes-param and writes-receiver

docs/03-code-graph-model.md GM-12 specifies the function effect system, which already includes
`writes-global`. DF-17 extends the effect lattice with two finer-grained kinds:

| Effect | Meaning |
|---|---|
| `writes-param(i)` | The function mutates the value bound to its i-th parameter through a mutable reference or pointer |
| `writes-receiver` | The function mutates the receiver object (`self`, `this`) |

These effects are recorded on the function node as effect-lattice attributes (GM-12) and are
propagated interprocedurally through function summaries (DF-3.3). A call site where `writes-param(0)`
is present for the callee generates a `mutation-out` edge (DF-2.1) attributed to argument 0 at the
call site.

This makes the question "does `f(myList)` mutate `myList`?" answerable interprocedurally — the same
question that the container-model worked example in DF-1.2 raises but does not address for the
mutation direction.

### DF-17.3 — Mutation fan-out

**Pedigree** (DF-1) is the inbound provenance fan-out: "what populated this value?" **Mutation
fan-out** is its outbound dual: "who can change this value after this point?"

A mutation fan-out query from value `V` at program point `P` returns the set of all aliases,
parameters, and captures through which `V` (or a value that `V` derives into) may be mutated after
`P`, together with the confidence and the `writes-param(i)` / `writes-receiver` / `mutation-out`
evidence for each entry.

The fan-out is computed by forward traversal of escape edges (DF-16.1), capture edges (DF-18.2), and
`writes-param(i)` / `writes-receiver` summaries reachable from `V`'s allocation site. The motivating
query is the **exposed-internal-state** class (analogous to SpotBugs `EI_EXPOSE_REP`): "which getter
methods return a mutable reference to an internal field, and what code mutates the field through that
reference?"

### DF-17.4 — Sanitization-invalidation rule

This rule refines DF-11 (typed taint) and is grounded in the DF-16 aliasing model.

> **Sanitization-invalidation:** A taint label that was cleared by a sanitizer at program point `P`
> is **re-applied** if the sanitized value is subsequently mutated — through any alias — before it
> reaches a sink.

The failure mode this addresses: a value is sanitized, stored in a field or captured by a closure,
then mutated through an alias before the sanitized reference is used. The sanitization clearing event
at `P` is no longer valid at the use site. A taint model without this rule reports false safety on
the mutated value.

`cgx` enforces this rule by annotating the cleared taint label with the program-point of
sanitization and the set of live aliases at that point. If any alias in the set carries a
`mutation-out` edge (DF-2.1) after `P`, the cleared label is re-applied with `confidence: probable`
(the mutation path may not be feasible on all execution paths).

### DF-17.5 — Interaction with coerce(from,to)

A `coerce(from,to)` transformation kind (DF-10) on a pedigree edge changes the value's type.
Mutability is type-level: the mutability level of the derived value after a coercion is determined
by the destination type, not the source. Queries that trace mutation fan-out must therefore
follow `coerce` edges and re-evaluate mutability at the destination type.

---

## DF-18 — Function Values and Closures

**Status: schema-room** — the `lambda` node kind (docs/03-code-graph-model.md GM-1.1) and
`calls:closure` / `calls:callback` edge types (GM-2.1) are already in the schema; this section
adds the function-value node flavor, specifies capture edges with their mutability attribution, and
defines indirect-call resolution as pedigree computed over function values. Full precision on
indirect resolution depends on the resolution ladder (GM-5) and is `possible` without deeper
inference.

### DF-18.1 — Function values as a node flavor

A **function value** is a function or method used as a data value: stored in a variable, passed as
an argument, returned from a function, or placed in a collection. The `lambda` kind in GM-1.1 covers
anonymous callables; DF-18 generalises this to named functions used as values (Rust `let f =
my_fn;`, Python `handler = process_order`, Go `var fn func(int) = compute`).

Function values are represented as symbol nodes with `kind: function-value` in the dataflow graph.
A `derives-from` edge with kind `copy` connects the original function symbol to the function-value
node, establishing pedigree linkage.

### DF-18.2 — Indirect-call resolution as pedigree

The key insight: **indirect-call resolution is pedigree computed over function values**.

At an indirect call site `f(args)` where `f` is a function value, the candidate callee set is the
set of function values that flow to `f` at that site. This is exactly the pedigree query for `f`
(DF-1), restricted to function-value nodes. Each candidate carries the confidence label from how it
was tracked through the pedigree:

| Pedigree chain for `f` | Resolution confidence |
|---|---|
| `f` directly assigned from a named function | `certain` (if no conditional branches) or `probable` (if branch) |
| `f` returned from a function whose return is resolved | inherits the return's confidence |
| `f` stored in a container and retrieved | `probable` (smashed collection model; DF-4.2) |
| `f` passed through an opaque call or FFI boundary | `possible`; cut marker `reflective` emitted |

This resolution reuses the existing pedigree machinery (DF-1), container models (DF-4), and
function summaries (DF-3.3) with no new analysis. The `calls:indirect` edge in GM-2.1 carries the
resolved `candidate_set` populated by this pedigree query.

### DF-18.3 — Capture edges and the loop-variable capture bug family

When a closure or lambda captures a variable from its enclosing scope, `cgx` records a **capture
edge** — a `derives-from` edge from the captured variable to the function-value node, attributed
with the capture mode:

| Attribute | Value | Meaning |
|---|---|---|
| `capture` | `by-value` | The closure holds a copy of the variable's value at the time of creation |
| `capture` | `by-ref` | The closure holds a reference to the variable; mutations to the variable after closure creation are visible inside the closure |

The capture mode is composed with the captured variable's mutability level (DF-17.1). The
combination `capture: by-ref` + `value: mutable` on a loop-variable produces the canonical
**loop-variable capture bug family**:

```python
# Python late-binding example
handlers = []
for i in range(5):
    handlers.append(lambda: print(i))   # captures i by ref (late binding)
# All handlers print 4 — they all reference the same binding, which ends at i=4
```

```go
// Go pre-1.22 example
for _, v := range items {
    go func() { process(v) }()   // captures v by ref; v is reassigned each iteration
}
```

```javascript
// JS var-in-loop example
var fns = [];
for (var i = 0; i < 5; i++) {
    fns.push(function() { return i; });   // var-scoped i, captured by ref
}
// All fns return 5
```

In each case, the capture edge carries `capture: by-ref` and the captured variable has
`binding: mutable` (reassigned by the loop). The query "find closures capturing a mutable
loop-variable by reference" is directly expressible over these attributes (docs/05-queries.md Q-28).

### DF-18.4 — Captured resources and deferred execution

A closure that captures a resource (a lock guard, a file handle, a database connection) extends the
resource lifecycle pair (docs/03-code-graph-model.md GM-13): the release event is now contingent
on when — and whether — the closure executes. The capture edge appears in the GM-13 acquire/release
pairing analysis.

When a closure is stored for deferred execution (event handler registration, callback argument,
`Promise` constructor), the call from storage to execution is temporally decoupled from
registration. This ties to:

- `spawns` edges (GM-9): the closure may execute on a separate task, inheriting the spawn/exception-
  termination semantics.
- Mediated call edges (docs/03 GM-17): a stored callback is an indirect call where the mediator
  (event loop, scheduler, DI container) establishes the connection; the `established-by` provenance
  attribute records the registration site.
- Suspension points (GM-10): if the closure `await`s or yields, it is subject to the same
  interleaving hazards as any suspended task.

---

## DF-19 — Lineage Type Reconstruction

**Status: core-extension** at the query-specification level — the constraint-gathering traversal is
expressed as a pedigree query (DF-1), a forward-use scan (DF-6.2), and unification over existing
graph elements (DF-9 joins, DF-16 aliases, channel send↔recv from DF-20). Precision at runtime
depends on the resolution tier (GM-5) and degrades to `possible` at type-confidence boundaries
(docs/03 GM-14). Full inference-quality reconstruction (certain for all values) is roadmap and
depends on SCIP enrichment.

### DF-19.1 — Motivation

Dynamic languages, gradual type systems, and reflection-heavy code produce values whose types are
erased or widened to `any` / `interface{}` / `Object`. Without type information at these points,
virtual dispatch resolution is imprecise and taint queries that depend on the receiver type fail.

Lineage type reconstruction recovers type evidence from the graph itself rather than from type
annotations. For a value `V` of unknown or widened type, the reconstruction query crawls three
directions from `V`'s node and intersects the resulting constraints.

### DF-19.2 — The three constraint directions

**Up — pedigree constraints (inbound)**

Follow `derives-from` edges backward from `V` to its sources. Each source contributes type evidence:

| Source kind | Evidence strength | Type contribution |
|---|---|---|
| Literal (`42`, `"foo"`, `true`, `[]`) | `certain` | The literal's language-defined type (int, string, bool, collection) |
| Constructor call (`new User(...)`, `User { }`, `User::new()`) | `certain` | The constructed type |
| Function call return | inherits the call's confidence | The declared or inferred return type of the callee |
| Parameter | inherits annotation confidence | The declared or inferred parameter type |

Literals and constructors are the **ground-truth leaves** where the reconstruction bottoms out: they
carry `certain` confidence and propagate their types forward through transformation kinds (DF-10).
Primitive operations propagate type automatically: `arith` edges produce numeric types, `concat`
edges produce string types, `coerce(from,to)` edges produce the destination type.

**Down — use constraints (forward)**

Follow forward data-flow (DF-6.2) from `V` to its uses. Each use constrains from below:

| Use kind | Type constraint |
|---|---|
| Method call `V.send()` | `V` satisfies the type of any symbol with a `send` method (structural / duck footprint) |
| Typed-parameter sink `fn process(x: Socket)` with `V` as argument | `V` satisfies `Socket` |
| Sink class declaration (DF-12.2) that requires a specific type | `V` is compatible with the sink's expected type |

Down constraints are weaker than up constraints because they express structural compatibility, not
identity. They are most valuable for values of bare interface types (`interface{}`, `any`, untyped
`Object`) where up constraints are absent.

**Sideways — unification constraints**

Values that must share a type propagate constraints laterally:

| Unification source | Lateral constraint |
|---|---|
| Two branches feeding a DF-9 join node | Both must satisfy the join's expected type |
| Aliases tracked in DF-16 | All aliases of `V` share `V`'s type |
| Channel send↔recv pair (DF-20.1) | Sent value and received value must be the same type |
| Branch merge into one variable (`if cond { a } else { b }`, both assigned to the same binding) | `a` and `b` must be subtypes of the binding's type |

Sideways unification often collapses the candidate set. Two values entering a join where one is
`certain: string` and the other is `certain: int` produce an **empty intersection** — a
contradiction.

### DF-19.3 — Result: candidate type set with confidence

The reconstruction query returns:

| Result | Meaning |
|---|---|
| Single type, `certain` | Fully resolved — the value has exactly this type at this point |
| Narrowed set, `probable` | Two or three candidate types consistent with all constraints |
| Wider set, `possible` | Constraints are insufficient to narrow below the type-confidence boundary |
| Empty set: **CONTRADICTION** | No type satisfies all constraints simultaneously — this is a bug signal |

A contradiction indicates that the same value is used in two incompatible ways: for example, passed
to a function expecting a `Socket` (down constraint) but constructed as a `File` (up constraint).
This class of bug — "find values whose reconstructed type contradicts their declared type" — is
expressible as a query (docs/05-queries.md Q-26) and has no direct equivalent in mainstream static
tools.

### DF-19.4 — Worked example: reconstructing a Go `interface{}` value

```go
func process(v interface{}) {
    // v has no up-constraint from the call site — bare interface{}
    v.(*Config).Timeout = 30   // type assertion: down-constrains v to *Config
    store(v)                   // store expects type Store; sideways-constrains v
}

func caller() {
    cfg := &Config{Host: "db"}   // constructor: up-constraint, certain *Config
    process(cfg)
}
```

Reconstruction for `v` inside `process`:

1. **Up**: `cfg` in `caller` is constructed as `*Config` (`certain`). The `derives-from` edge
   through the parameter boundary propagates `probable: *Config` (confidence degrades one step
   through the parameter boundary at `process(cfg)` because the declared parameter type is
   `interface{}`).

2. **Down**: the type assertion `v.(*Config)` asserts `*Config` — structural constraint
   `probable: *Config`. The call `store(v)` adds whatever `store`'s parameter type is; if `store`
   expects `*Config`, this reinforces.

3. **Sideways**: no join nodes or aliases involving `v` in this fragment.

**Result**: candidate set `{ *Config }` at `probable` confidence, supported by three independent
constraints (constructor pedigree, type assertion, typed-parameter use). A query asking "what type
flows into `process`?" returns `*Config / probable` with an evidence trail naming the constructor
at `caller` line N and the assertion at `process` line M.

If a second call site passes an `*AuditLog` where a `*Config` is expected, the sideways unification
at the join of both call paths produces a contradiction — `cgx` reports the inconsistency.

### DF-19.5 — Precision limits

Type reconstruction precision degrades at type-confidence boundaries (docs/03 GM-14):

- TypeScript `any`, Go `interface{}` passed across multiple boundaries without assertion
- Reflection results (`reflect.ValueOf(x).Interface()`, `Method.invoke()`)
- Values arriving through a `possible`-confidence indirect call (DF-18.2)

At these points the reconstruction result is `possible` and the evidence trail names the boundary.
Users can ask "where does `any` enter my typed codebase?" as a type-confidence-boundary query
(Q-30) independently of the full reconstruction.

---

## DF-20 — Non-Call Dataflow Linkages

**Status: core-extension** — channel and queue send↔recv edges and import-time edges are
representable as `derives-from` edges with existing attributes; the `deferred-execution` marker
extends edge provenance with no schema change. The key addition is making these linkages explicit
and queryable rather than implicit gaps (which would otherwise appear as pedigree cut points).

### DF-20.1 — Channel and queue send↔recv edges

In languages with channel or message-queue concurrency (Go channels, Rust `mpsc`, actor mailboxes,
async queues), a value sent by producer `A` and received by consumer `B` has a dataflow dependency
that has no connecting call expression.

`cgx` represents this as a `derives-from` edge from the sent value to the received variable, with:

| Attribute | Value |
|---|---|
| `kind` | `copy` (identity — channel does not transform the value) |
| `established-by` | `channel-send-recv` |
| `confidence` | `probable` for bounded channels (bounded buffer; send and recv are structurally paired); `possible` for unbounded or dynamic queues |
| `provenance` | file:line of both the send expression and the receive expression |

Without this edge, a pedigree query on the received value stops at the receive call and fails to
trace back to the producer. Channel edges feed directly into lineage type reconstruction (DF-19.2
sideways unification): the sent type and received type must be the same, collapsing the candidate
set.

### DF-20.2 — Import-time and module-load edges

In Python, JavaScript, and Go, importing a module executes code at load time. Python and JS module
bodies run on first import; Go `init()` functions run before `main`. Java `static` initialiser
blocks behave identically.

These execution paths are invisible to a call-graph analysis that starts from `main` because they
are not reachable via call edges. `cgx` models them as entrypoint-adjacent edges (docs/03 GM-7):
each module-load execution site is treated as an entrypoint, and the code it runs is reachable from
that entrypoint.

Practically, this means:
- A taint source that populates a module-level global during `init()` is reachable in the graph.
- A registered callback added to a global registry during module load is tracked as a mediated call
  edge (GM-17) sourced from the import-time entrypoint.
- "Which side-effects occur before `main` runs?" is answerable as a reachability query from
  import-time entrypoints.

### DF-20.3 — Deferred execution: code-as-data marker

Frameworks that accept code as data — LINQ expression trees, ORM query builders (SQLAlchemy,
Hibernate Criteria), Spring Data derived queries (`findByEmailAndStatus`), Spark transformations —
do not execute the submitted code as bytecode. The lambda or expression is reified and translated
by the framework (typically into SQL, a query plan, or a request).

`cgx` marks the `derives-from` edge carrying a code-as-data value with the provenance attribute
`deferred-execution: true`. The semantics:

- Taint analysis: a tainted value flowing into a deferred-execution expression may parameterise the
  resulting query; the taint label propagates with `confidence: possible` because the translation
  semantics are framework-dependent and may neutralise injection (e.g. LINQ's parameterised SQL) or
  may not (string-interpolated query builders).
- Type reconstruction (DF-19): the return type of the deferred computation is the framework's output
  type, not the closure's declared return type. The DF-19 down-constraint for the closure body uses
  the framework's known translation rules if a framework pack (docs/03 GM-15) declares them.
- Pedigree: the deferred-execution edge is a `derives-from` edge with tag `composed` at the
  expression site and `deferred-execution: true` in provenance, so pedigree traversal includes it
  but callers can filter on the attribute.

---

*Next:* docs/05-queries.md — query language syntax, worked examples for every
question class, and the canonical reachability and pedigree query forms.
Query primitives for the security patterns covered in DF-9 through DF-16
are specified in Q-20 through Q-25. Query primitives for DF-17 through DF-20
(mutation fan-out, closure capture, lineage type reconstruction, coercion,
non-call dataflow) are specified in Q-26 through Q-31.
