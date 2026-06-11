# 03 — Code Graph Model

**Status:** Feature specification (pre-implementation)
**Audience:** Engineers integrating with or extending `cgx`; contributors; advanced users building queries
**Working name:** `cgx` (placeholder — see docs/README.md)
**Cross-references:** docs/04-dataflow-and-provenance.md · docs/05-queries.md · docs/08-language-support.md · docs/10-landscape.md

---

## Overview

`cgx` represents source code as a directed, attributed graph. Every node is a
symbol; every edge is a typed relation between symbols. Facts are not inferred
silently: each edge carries a confidence label, a provenance record (file:line
plus the rule that produced it), and — for call edges — an edge condition label
that distinguishes happy-path from exceptional control flow.

This document specifies:

- GM-1: Node taxonomy
- GM-2: Edge taxonomy
- GM-3: Edge condition labels (the guard lattice)
- GM-4: Path-relative transience semantics
- GM-5: Confidence labels and the resolution ladder
- GM-6: Provenance attached to every fact
- GM-7: Entrypoint modelling
- GM-8: Per-language semantics mapping

---

## GM-1 — Node Taxonomy

Every named program entity is a **symbol** in the graph. Symbols are the nodes.

### GM-1.1 — Symbol kinds

| Kind | Description | Examples |
|---|---|---|
| `function` | Named, freestanding callable | `fn parse_input`, `def process`, `func handle` |
| `method` | Callable bound to a type | `MyClass.save()`, `impl Foo { fn bar() }` |
| `type` | Struct, class, interface, trait, enum, union | `class User`, `struct Config`, `trait Serde` |
| `field` | Named member of a type | `User.id`, `Config.timeout` |
| `variable` | Local, parameter, or module-level binding | `let count`, `err` in Go, `x: int` param |
| `module` | File-level or package-level namespace | Python module, Rust `mod`, Java package |
| `constant` | Compile-time named value | `MAX_RETRIES`, `const PI` |
| `macro` | Hygienic macro or preprocessor definition | Rust `macro_rules!`, C `#define` |
| `lambda` | Anonymous callable with a stable allocation site | closures, arrow functions, lambdas |
| `entrypoint` | Declared root for reachability (see GM-7) | `main`, HTTP handler, test function |

### GM-1.2 — Symbol identity

Each symbol is identified by a **fully-qualified name** (FQN) composed from its
enclosing scopes separated by `::` (language-agnostic normalisation). Where a
SCIP index is available, the SCIP symbol descriptor is used as a stable,
canonical identifier and the FQN is derived from it.

### GM-1.3 — Symbol attributes

Every symbol node carries:

- `kind`: one of the kinds in GM-1.1
- `fqn`: fully-qualified name string
- `location`: file path, start line, end line (from the index)
- `language`: source language tag
- `visibility`: `public` / `private` / `protected` / `internal` / `package` (language-mapped)
- `is_abstract`: boolean (interfaces, abstract methods, trait declarations)
- `confidence`: the weakest confidence of any fact contributing to this node (see GM-5)

---

## GM-2 — Edge Taxonomy

Edges express typed relations between symbols. Every edge is directed
(from-symbol → to-symbol) and carries the attributes described in GM-5 and GM-6.

### GM-2.1 — Call edges

| Edge type | Meaning | Example |
|---|---|---|
| `calls` | Direct invocation of a callable | `main()` calls `parse_args()` |
| `calls:virtual` | Dispatch through a virtual/interface/trait method | `animal.speak()` where `animal: &dyn Animal` |
| `calls:closure` | Invocation of a closure or lambda captured from scope | `list.map(|x| x * 2)` |
| `calls:callback` | Invocation via a function-value argument | `sorted(key=fn)` |
| `calls:async` | Logical call across an async suspension boundary | `.await` on an `async fn` |
| `calls:indirect` | Call via function pointer without resolved target | `(*fp)(args)` |

Virtual and indirect call edges carry a `candidate_set` attribute listing all
symbols the edge may resolve to at runtime, ordered by estimated probability.

### GM-2.2 — Data-flow and structural edges

| Edge type | Meaning |
|---|---|
| `contains` | Lexical containment: a module contains a type; a type contains a method or field |
| `imports` | One module imports a symbol from another |
| `derives-from` | A value is derived from another value (see docs/04-dataflow-and-provenance.md) |
| `overrides` | A method overrides a method in a supertype |
| `implements` | A type implements an interface or trait |
| `inherits` | A class inherits from a superclass |
| `references` | A symbol uses another symbol without calling it (e.g. a type annotation, a constant use) |
| `instantiates` | A call site allocates an instance of a type (`new T`, `T { }`, allocation expression) |
| `throws` | A callable may propagate an exception of a given type to its callers |
| `catches` | A callable handles an exception of a given type |
| `reads-field` | A method reads a specific field from an instance in scope |
| `writes-field` | A method writes a specific field on an instance in scope |

### GM-2.3 — Edge attributes (common to all edges)

Every edge carries:

- `edge_type`: one of the types above
- `edge_condition`: one of the guard labels defined in GM-3
- `confidence`: `certain` / `probable` / `possible` (see GM-5)
- `provenance`: file:line + producing rule (see GM-6)
- `cut_markers`: zero or more of `reflective`, `dynamic`, `via-DI`, `via-FFI`,
  `unresolved` (see GM-5.3)

---

## GM-3 — Edge Condition Labels (the Guard Lattice)

Every call edge carries an **edge condition** label that encodes the runtime
condition under which control takes that edge. This is a first-class, queryable
attribute; it is not inferred per-path at query time (path-relative transience
is, however — see GM-4).

The canonical label set has exactly **five values**:

| Label | Meaning | Languages / constructs |
|---|---|---|
| `always` | Edge is taken on every execution of the call site; no runtime condition guards it | Unconditional function call; also used for calls inside `finally`/`defer`/cleanup blocks (the call is always taken once that block is entered — its exception-relativity is path-relative transience, not a stored label) |
| `conditional` | Edge is taken only when an explicit boolean branch is true | `if`, `when`, `match` arm, ternary |
| `loop` | Edge is taken zero or more times inside a loop body | `for`, `while`, `loop`, iterator body |
| `exception` | Edge is taken only during exceptional control flow — the call site is inside an exception handler, a Rust `?`/`Err` branch, a Go `err != nil` block, or a C sentinel check | Java `catch`, Rust `Err(e) =>` arm or `?` on `Err`, Go `if err != nil`, C `if (ret < 0)` |
| `panic` | Edge is taken only on an unwinding/aborting path that cannot be recovered from in normal control flow (Rust `panic!`/`unwrap`, Go `panic`, Java unchecked throw reaching exit, C abort path) | Rust `panic!()`, `unwrap()`, `expect()`, `assert!()`; Go `panic()`; C `abort()` |

**Compound labels are banned.** Every edge carries exactly one label from this five-value set; there is no `always,exception` or similar concatenation. A plain call inside a `catch` or `finally` block is labeled `always`; its exception-relativity is determined at query time from path-relative transience (GM-4), not from the edge's own label.

**Exceptional class.** The labels `exception` and `panic` together form the **exceptional class**. Query predicates expressed as "non-exception path" or `exceptional()` in query language filter against this class (i.e., both `exception` and `panic` edges are treated as exceptional for path-relative transience purposes). See GM-4.

`cgx` is the first tool in this class to expose these as queryable edge attributes; Joern's Code Property Graph, for reference, carries `THROW` and `TRY` control structures but does not label CFG edges with exceptional vs normal distinction.

### GM-3.1 — Per-language mapping

| Language | Mechanism | cgx label |
|---|---|---|
| Java / C# / Python | `catch` / `except` handler body | `exception` |
| Java / C# / Python | `finally` block | `always` (the call is unconditional once the block is entered; exception-relativity is path-relative transience computed at query time — see GM-4) |
| Rust | `Err(e) =>` match arm; `?` early return on `Err` | `exception` (data-conditioned error branch) |
| Rust | `panic!` / `unwrap` on `None` or `Err` | `panic` |
| Go | `if err != nil` block | `exception` (data-conditioned error branch) |
| Go | `recover()` inside `defer` | `exception` |
| C | `if (ret < 0)` / `if (!ptr)` sentinel check (heuristic) | `exception` (confidence: `probable`) |
| JavaScript / Python | `catch` / `except` | `exception` |
| Any | Non-exceptional conditional call | `conditional` |

Rust's `?` operator and Go's `err != nil` check are ordinary data-conditioned
branches in those languages, not exceptions in the traditional sense. `cgx`
models them under the `exception` guard label because they represent the
semantically equivalent "failure branch" concept; the producing rule is recorded
in provenance so users can distinguish them.

---

## GM-4 — Path-Relative Transience Semantics

**Definition.** An edge `C → D` is *exception-transient relative to path P* if
and only if P traverses at least one edge whose edge condition is in the
**exceptional class** (`exception` or `panic`) before reaching `C`. The same
edge `C → D` is not exception-transient relative to a path P′ that reaches `C`
with no exceptional-class edge.

Path-relative transience is computed per query, not stored as a fixed edge
attribute.

### GM-4.1 — Canonical example

```
A → B          (edge condition: always)
B → C          (edge condition: exception)
C → D          (edge condition: always)
```

- Path P = A → B → C → D: the edge B → C is `exception`-conditioned. After
  crossing it, a `seen_exceptional` flag is set for this path walk. The edge
  C → D is therefore *exception-transient relative to P*.
- Path P′ = X → C → D (reached via some other path with no exceptional edge):
  the edge C → D is *not* exception-transient relative to P′.

`D` is not globally "exception-transient"; it is transient only relative to
paths that pass through B's exception edge. This is the intended semantics for
questions of the form "which callees are only reached through error handling?"

### GM-4.2 — Computation algorithm

For each path walk, `cgx` maintains a per-walk boolean flag `seen_exceptional`.

1. Initialise `seen_exceptional = false` at the walk root (entrypoint).
2. On crossing any edge: if `edge.edge_condition ∈ {exception, panic}`, set
   `seen_exceptional = true` for this walk branch (irrevocably — backtracking
   to a fork resets to the flag value at the fork point).
3. At any node N: N is exception-transient on this walk branch iff
   `seen_exceptional = true`.

This is a purely structural graph walk; it requires no SMT solver, no symbolic
execution, and no call-site context. It is deterministic and fast.

### GM-4.3 — Query examples (prose; syntax in docs/05-queries.md)

- "Non-exception paths from `main::foo` to `security::bar`": walk paths,
  reject any path where any edge has `edge_condition ∈ {exception, panic}`.
- "Paths where `security::bar` is reachable only through exception handling":
  paths where `seen_exceptional = true` at `security::bar` AND no non-exception
  path to `security::bar` exists.
- "Which callees of `C` are exception-transient via the path through `B`":
  enumerate paths through the `B → C` exception edge; collect all nodes reached
  with `seen_exceptional = true`.

---

## GM-5 — Confidence Labels and the Resolution Ladder

Every edge in `cgx` carries a **confidence** label. Confidence is a
deterministic label derived from the resolution method, not a probabilistic
score.

### GM-5.1 — Confidence values

| Value | Meaning | Typical resolution source |
|---|---|---|
| `certain` | Resolved by direct static binding; no ambiguity remains | Non-virtual call; monomorphized Rust generic; SCIP def/ref with exactly one target |
| `probable` | Resolved heuristically with a small, credible candidate set | CHA/RTA virtual dispatch with few overrides; unique name match in scope; SCIP ref narrowed by type inference |
| `possible` | Over-approximated; candidate set is large or unverified | Name-based match with multiple candidates; `dyn Trait` candidate set; function pointer without points-to; duck-typed name match |

### GM-5.2 — The resolution ladder

Confidence correlates directly with the resolution tier used to produce the
edge. The ladder is strictly narrowing (each tier is a subset of the previous):

| Tier | Algorithm | Confidence band emitted | Requires |
|---|---|---|---|
| 0 — Name/syntactic | Callee name + arity string match across all declared functions | `possible` | tree-sitter only; zero config |
| 1 — Scope-graph resolution | Scope-aware def/ref via per-file scope graphs (algorithm from the stack-graphs paper, Creager et al. 2022 — prior art only; the `stack-graphs` crate was archived Sep 2025, so the substrate is `tree-sitter-graph`; see docs/09-architecture.md AR-4) | `probable` for resolved refs, `possible` for ambiguous | tree-sitter + scope-graph rules; zero config |
| 2 — SCIP-enriched | Compiler-accurate def/ref from a SCIP index (rust-analyzer, scip-java, scip-typescript, scip-clang) | `probable` to `certain` depending on target uniqueness | SCIP index pre-built |
| 3 — CHA/RTA | Class Hierarchy Analysis / Rapid Type Analysis over the SCIP-typed graph | `probable` (candidate set narrowed to instantiated types) | SCIP + type hierarchy |
| 4 — Points-to | Andersen/Steensgaard inclusion-based points-to analysis | `certain` for monomorphic sites; `probable` for aliased | Full program required |

`cgx` ships tiers 0–2 in the initial release. Tiers 3–4 are planned extensions
(see docs/11-roadmap.md). The active tier is recorded in each edge's provenance.

The default tier at startup is the highest tier for which a SCIP index is
available; otherwise tier 1 (scope-graph resolution).

### GM-5.3 — Cut markers (orthogonal to confidence)

Some call targets are structurally unreachable by any static resolution. `cgx`
does not silently drop these edges; it emits a cut-marker edge instead:

| Marker | Meaning |
|---|---|
| `reflective` | Call target is a string computed at runtime (reflection, `Class.forName`, `eval`) |
| `dynamic` | Call site dispatches through a runtime plugin, script, or dynamic loader |
| `via-DI` | Binding is managed by a dependency injection container (Spring, Guice, Dagger, etc.) |
| `via-FFI` | Call crosses a foreign-function boundary (JNI, Python C extensions, Rust `extern "C"`) |
| `unresolved` | Resolution was attempted and produced no candidates (the symbol is genuinely unknown) |

A cut-marker edge is still a queryable fact. Security engineers searching for
all paths to a sink will filter *in* `reflective` edges; software engineers
doing dead-code analysis will filter them *out*. The choice is left to the
query.

---

## GM-6 — Provenance Attached to Every Fact

Every node and edge carries a **provenance** record: the evidence that
justifies the fact.

### GM-6.1 — Provenance record schema

```
provenance {
  file:     string    # repo-relative path
  line:     int       # 1-based line of the call site or definition
  col:      int?      # optional column (available when SCIP provides it)
  rule:     string    # name of the producing rule, e.g. "scope-graph-ref",
                      #   "scip-occurrence", "cha-override", "heuristic-sentinel"
  tier:     int       # resolution tier (0–4) used
  index_id: string    # content-addressed blob OID of the source file at index time
}
```

The `index_id` field links the fact to the exact version of the source file that
produced it, enabling staleness detection when the file changes.

### GM-6.2 — Querying provenance

Every query result can be returned with `--evidence` to show the provenance
record alongside each fact. This is the basis for the "explain" capability
described in docs/07-interfaces.md.

---

## GM-7 — Entrypoint Modelling

Reachability queries require **roots**: the set of symbols from which graph
traversal begins. `cgx` calls these **entrypoints**.

### GM-7.1 — Automatic entrypoint detection

`cgx` applies per-language heuristics to detect entrypoints without
configuration:

| Language | Auto-detected entrypoints |
|---|---|
| Rust | `fn main()`, `#[test]` functions, `#[tokio::main]` / async entry |
| Java | `public static void main(String[])`, `@Test` methods, Spring `@Controller`/`@RestController` request handlers |
| Python | `if __name__ == "__main__"` block, pytest functions named `test_*`, Django/Flask route functions |
| Go | `func main()` in package `main`, `func Test*(*testing.T)` |
| JavaScript / TypeScript | Top-level `export default`, Express/Fastify route handlers, Jest `test()`/`it()` calls |
| Any | Any symbol explicitly declared as an entrypoint (see GM-7.2) |

### GM-7.2 — Explicit entrypoint declaration

Users can declare entrypoints via a configuration file or CLI flag. This is
necessary when the framework is not auto-detected (e.g. a custom IoC container,
an embedded callback system, a generated main).

```toml
# cgx.toml
[[entrypoints]]
symbol = "myapp::server::HttpServer::dispatch"
kind   = "http-handler"
```

Declared entrypoints are stored in the graph as `entrypoint` nodes (GM-1.1)
and are queried like any other node.

### GM-7.3 — Reachability scope

All reachability queries are evaluated relative to the active entrypoint set.
The default scope is the full auto-detected set. Queries can restrict scope to
a named subset (e.g. "reachable from test entrypoints only"). Symbols not
reachable from any entrypoint are labelled `unreachable` in query output.

---

## GM-8 — Per-Language Semantics Mapping

The graph model is language-agnostic, but the mapping from source constructs to
graph elements is language-specific. This section states the canonical mapping
for each supported language. Full detail is in docs/08-language-support.md;
this section covers the constructs that affect graph topology.

### GM-8.1 — Java

| Construct | Graph representation |
|---|---|
| Virtual method call (`object.method()`) | `calls:virtual` edge; candidate set from CHA/RTA over class hierarchy; confidence `probable` |
| Interface call | `calls:virtual` edge; candidate set = all implementing types |
| `throw new SomeException()` | `throws` edge from current method to exception type; call edges on the throw-site path are labeled `exception` (thrown into a handler) or `panic` (unchecked throw reaching program exit) |
| `catch (SomeException e)` handler body | Calls inside: `edge_condition = exception` |
| `finally` block | Calls inside: `always` (unconditional once the block is entered); exception-relativity is path-relative transience from GM-4 |
| Checked exception in signature (`throws E`) | `throws` edge; confidence `certain` |
| Reflection (`Class.forName`, `Method.invoke`) | Cut-marker `reflective` |

### GM-8.2 — Rust

| Construct | Graph representation |
|---|---|
| Trait method on `dyn Trait` | `calls:virtual` edge; candidate set = all trait implementations in scope; confidence `probable` |
| Monomorphized generic call | `calls` edge; confidence `certain` |
| `?` operator on `Result` | `exception` edge condition on the implicit early-return call path |
| `panic!` / `unwrap()` / `expect()` | `panic` edge condition on the panic path |
| `async fn` / `.await` | `calls:async` edge to the awaited function; runtime mediates execution; edge tagged `async` |
| `extern "C"` / FFI call | Cut-marker `via-FFI` |
| `macro_rules!` expansion | Edges resolved at the macro call site after expansion |

### GM-8.3 — Go

| Construct | Graph representation |
|---|---|
| Interface method call | `calls:virtual`; candidate set from concrete implementations; confidence `probable` |
| `if err != nil` block | Calls inside: `edge_condition = exception` |
| `panic` / `recover` | `panic` edge condition; `recover()` in `defer` models `exception` handler |
| First-class function / closure | `calls:closure` or `calls:callback`; confidence `possible` without points-to |
| CGO call | Cut-marker `via-FFI` |

### GM-8.4 — JavaScript / TypeScript (and Python duck typing)

Dynamic dispatch is pervasive in these languages; the gap between syntactic and
type-aware resolution is wide.

| Construct | Graph representation |
|---|---|
| Method call on untyped variable | `calls:virtual`; candidate set = all methods named the same across all types; confidence `possible` |
| TypeScript with SCIP index | `calls:virtual`; candidate set narrowed by type-checker; confidence `probable` to `certain` |
| `catch` block | Calls inside: `edge_condition = exception` |
| `eval` / `Function()` | Cut-marker `reflective` |
| `require()` / `import()` dynamic | Cut-marker `dynamic` if the argument is not a string literal |
| Decorator / framework route | Auto-detected as entrypoint where the framework is known; else requires explicit declaration (GM-7.2) |

Python follows the same pattern as JavaScript; SCIP (`scip-python` via
pyright) provides type narrowing where available.

### GM-8.5 — C / C++

C has no language-level exceptions; failure paths are signalled by return-value
convention. `cgx` applies heuristic rules:

| Construct | Graph representation |
|---|---|
| Function pointer call | `calls:indirect`; confidence `possible` without points-to |
| `if (ret < 0)` / `if (!ptr)` after call (heuristic) | `exception` condition on edges inside the guarded block; confidence `probable` |
| `#define`-expanded macro call | Resolved after expansion with `heuristic` rule tag |
| `setjmp` / `longjmp` | Modelled as `exception`-conditioned edges to the `longjmp` target |
| C++ `throw` / `catch` | Same as Java: `throws`/`catches` edges, `exception` condition |
| Virtual method call (C++ vtable) | `calls:virtual`; CHA/RTA candidate set |

---

## Known Limits and Soundiness Statement

`cgx` is **soundy**, not sound, in the terminology of the Soundiness Manifesto
(Livshits, Sridharan, Smaragdakis et al., CACM 2015). It is sound for the
core language but deliberately under-approximates the following features. These
are not bugs; they are explicit, documented trade-offs:

| Feature | Treatment | Effect |
|---|---|---|
| Reflection / `eval` / `Class.forName` | Cut-marker `reflective`; not followed | Calls through reflection are invisible to the graph |
| Dynamic dispatch without type information | Over-approximated candidate set at `possible` confidence | Some edges in the graph may be infeasible at runtime |
| Dependency injection containers (Spring, Guice, Dagger, CDI) | Cut-marker `via-DI`; explicit model required | Bindings injected by the container are not resolved without user-supplied configuration |
| FFI / native code | Cut-marker `via-FFI`; not followed | Behaviour inside native code is not modelled |
| Runtime code generation / class loading | Cut-marker `dynamic` | Dynamically generated classes are not present in the graph |

**Reachability semantics.** "Reachable" in `cgx` means "not proven unreachable
by structural analysis." It does not mean "proven feasible." The tool performs
graph reachability, not constraint-based path-feasibility (which would require
SMT solving). Guard-conditioned reachability (filtering edges by `edge_condition`)
is cheap and exact for the structural condition; it does not prove that no
path-infeasibility exists on other grounds.

---

*Next:* docs/04-dataflow-and-provenance.md — value pedigree, scope entry/exit
inventory, taint analysis, and instance-level dead-member analysis.
