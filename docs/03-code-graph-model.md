# 03 — Code Graph Model

**Status:** Mixed. The core taxonomy — symbol kinds, edge kinds, edge-condition labels,
confidence values and tiers, provenance records, cut markers — is **implemented and
authoritative**; the enums in this document match `crates/cgx-core/` exactly. The extension
sections (GM-9 onward) are largely schema reservation or design, and each carries its own
`Status:` line. Where a `Status:` line and the body of a section disagree about whether
something exists, the section now says so inline.
**Audience:** Engineers integrating with or extending `cgx`; contributors; advanced users building queries
**Cross-references:** docs/04-dataflow-and-provenance.md · docs/05-queries.md · docs/08-language-support.md · docs/10-landscape.md · docs/12-language-primitives-and-frameworks.md

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
- GM-9: Spawn edges (`spawns` edge kind; detached error domain)
- GM-10: Suspension points (await/yield)
- GM-11: Synchronization context and lock sets
- GM-12: Function effect system
- GM-13: Resource lifecycle pairs (acquire/release)
- GM-14: Code-trust boundaries (unsafe, FFI, dependency edges, reflection, macro provenance, type-confidence boundaries)
- GM-15: Metadata and annotation facts (semantic classes; lowering to graph facts)
- GM-16: Implicit call sites (`implicit:<kind>` marker; resource-pair harvest)
- GM-17: Mediated call edges (DI, events, registries; `established-by` provenance + confidence)
- GM-18: Reflection and string-mediated dispatch (literal-pedigree resolution; tainted-string dispatch)
- GM-19: Build-configuration variance (`cfg-condition` attribute; per-configuration graph)
- GM-20: Error-model conversion points (recover/catch_unwind exit the exceptional class; checked-exception precision)

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
| `call-site` | A call expression site within a caller's body; subordinate node kind — never returned by symbol queries, joined via edge `site_id` | `handler::process` call to `db::write` at file.rs:42:8 |

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
- `confidence`: **Planned — not a node attribute.** Confidence is an **edge** attribute. The
  weakest-confidence-along-a-path value that queries actually use is computed at query time and
  never stored per node; CQL rejects `n.confidence` with a plan error that says so and points at
  `r.confidence`.
- `in_degree` / `out_degree`: **Planned — not stored on the node record, and not queryable.**
  `NodeRecord` has no such fields; degrees are computed on demand as plain totals inside
  `cgx symbols`' ranking pass, and CQL rejects `n.in_degree` / `n.out_degree` explicitly (the
  reject test cites this very section). There is also no `coerce` edge kind — `Coercion` is an
  `ImplicitKind`, not an `EdgeKind`. Use `cgx symbols --rank inbound|outbound|total` to rank hub
  nodes today. The design follows: per-edge-kind inbound and outbound edge counts, computed at index time. Each is a map keyed by edge kind — at minimum `calls`, `derives-from`, and `coerce` — recording how many edges of that kind enter (`in_degree`) or leave (`out_degree`) the node. The count is a cheap grouped count over the edge tables already present (no extra traversal) and is recomputed when the node's edges change. These attributes are **structural only**: they record graph shape and are explicitly **not** coupled to `confidence` (a high-degree node is not more or less confidently resolved for being a hub). They are queryable and filterable like any other node attribute (e.g. `MATCH (n) WHERE n.in_degree.calls > 5000`; the `derives-from` edge kind is accessed as `n.in_degree.derives_from`, since a hyphen is not a valid property identifier), and they seed the sentinel/hot-node detection that bounds traversal fan-out (see docs/04-dataflow-and-provenance.md DF-1.4 and docs/05-queries.md Q-3/Q-13). **Status: core-extension** (computed from Phase 1; the grouped count is available as soon as the edge tables exist).
- `signature`: nullable structured record on `function`/`method`/`lambda` kinds (see below):
  - `params`: ordered list of `{name, type_text, has_default: bool, variadic: bool}` — `type_text` is the declared type as written in source (Tier 1) or the SCIP-resolved type (Tier 2+), with the source recorded in provenance.
  - `return_type_text`: string or null.
  - `type_params`: list of strings (generics, as written).
  - `receiver`: string or null (self/this type for methods).
  - `canonical`: deterministic rendered string derived from the above (single normalization rule, language-tagged).
  - Populated where the parser provides it; null where it cannot. No inference is performed to fill it at Tier 1. **Status: core-extension** (schema present and populated-where-parseable from Phase 1).

### GM-1.4 — Call-site nodes

**Identity.** A call-site node's identity is `(caller symbol id, file, line, col)` → stable `site_id`. The `site_id` is an FNV-1a hash of `(caller_fqn, file, line, col)` — not of a blob OID and byte offset, and not of insertion order. The property the original wording was reaching for holds: it is deterministic and stable across re-indexes of an unchanged blob.

**Subordinate node kind.** Call-site nodes are subordinate: they never appear in `nodes(path)` and are not matched by default symbol patterns. Call-graph paths remain sequences of symbol nodes. Site facts are reached through the edge: `r.site.lock_set`, `r.site.suspends`. `position_in(n, path)` is defined over the symbol-node sequence of a path; call-site nodes never appear in `nodes(path)`. Site-level ordering within one caller uses `site.ordinal` (lexical) or, when populated, CFG dominance — never path position.

**Attributes** (with population phase):

| Attribute | Type | Status | Phase |
|---|---|---|---|
| `caller` | symbol ref | core-extension | 1 |
| `file` | string | core-extension | 1 |
| `line` | int | core-extension | 1 |
| `col` | int | core-extension | 1 |
| `ordinal` | int (lexical index within caller) | core-extension | 1 |
| `suspends` | bool (GM-10) | core-extension | 1 |
| `cfg_block` | opaque id (intraprocedural CFG block) | **Status: schema-room** — consumed by Q-20 guarded-cut semantics | 3 |
| `dominating_sites` | Set<site_id> (intraprocedural dominator facts) | **Status: schema-room** — consumed by Q-20 guarded-cut semantics | 3 |
| `lock_set` | Set<symbol> (GM-11) | **Status: schema-room** | 3 |
| `is_return_site` | bool | **Status: schema-room** | 3 |
| arg position bindings (Q-29) | per-site | **Status: schema-room** | 3 |

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
| `calls:super` *(Planned — see GM-21)* | Statically-bound delegation to a named ancestor's body, bypassing virtual dispatch | `super().m()`, `Base::m()` |

**Six of these seven are shipped `EdgeKind` variants; `calls:super` is not.** It is Theme-13
syntax the CQL parser recognises and deliberately rejects (`CALLS:super` → plan error), so a
query written against it fails loudly rather than returning an empty result. See GM-21.

Virtual and indirect call edges carry a `candidate_set` attribute listing all
symbols the edge may resolve to at runtime, ordered by estimated probability. A `calls:super` edge would have a single resolved target and a `bypassed_override` attribute naming the override virtual dispatch would otherwise select.

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
- `site_id`: reference to the call-site node (GM-1.4) this edge originates from; present on all call-edge types.

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

### GM-3.2 — Precedence rule

When a call site is lexically enclosed by multiple constructs that each apply a different label (e.g., a call inside an `if` inside a `for` inside a `catch`), exactly one label wins: the **maximum by the following total precedence order**:

```
panic  >  exception  >  loop  >  conditional  >  always
```

**Determination rule.** Collect the set of applicable labels from the chain of enclosing constructs between the call expression and the caller's body root (per the GM-3.1 per-language mapping); emit the maximum by precedence.

**finally/defer carve-out.** The existing rule that calls inside `finally`/`defer`/cleanup blocks are labeled `always` applies **before** precedence — a `finally` block contributes `always`, not `exception`, to the applicable set. Its exception-relativity remains path-relative transience per GM-4, computed at query time. An `if` inside a `finally` yields `conditional`.

**Worked example.** A call inside an `if` inside a `for` inside a `catch` block: applicable labels = {`exception` (catch), `loop` (for), `conditional` (if)}. Maximum by precedence: `exception` > `loop` > `conditional` → the edge is labeled `exception`.

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
| `unexpanded-macro` | Call edges may exist inside code generated by an unexpanded proc macro at this site; expansion was not performed. **Status: core-extension** (emitted from Phase 1). |
| `opaque-call` | The callee's body is not available to the analysis, so its effects and flows are unknown |
| `truncated-access-path` | A field-access path was cut at the modelled depth (field sensitivity is depth-1) |
| `summary-budget-exceeded` | An IFDS summary computation hit its budget and stopped short |

**Nine markers, not six.** The last three landed with the v0.3 dataflow/IFDS work and are
emitted by it. Note also that the serialised forms are kebab-case and lowercase throughout —
`via-di` and `via-ffi`, not `via-DI`/`via-FFI` — so a query or a `jq` filter must match the
lowercase token. `via-di` in particular is **never emitted today**: it is a real marker, but
nothing detects DI containers, so no edge carries it (see GM-17).

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
  rule:     string    # name of the producing rule. Shipped values include
                      #   "scope-ref", "import-ref", "indirect", "derives-from"
                      #   (cgx-resolve/src/link.rs, ifds.rs), "cha-trait-set"
                      #   (cha.rs), "rta-pruned" (rta.rs), "sig-compat" (sig.rs),
                      #   "interprocedural", "name-method" and "name-arity".
                      #   "scip-occurrence" ships on the SCIP path.
  tier:     int       # resolution tier (0–4) used
  index_id: string    # content-addressed blob OID of the source file at index time
}
```

The `index_id` field links the fact to the exact version of the source file that
produced it, enabling staleness detection when the file changes.

**`cha-override` and `heuristic-sentinel` are not rule names cgx emits** — but not because CHA
is unbuilt. **CHA ships and runs on every index**: `crates/cgx-resolve/src/cha.rs` narrows
name-method dispatch candidate sets against the trait/class hierarchy, stamps the surviving
edges `rule = "cha-trait-set"` at `Tier::ChaRta`, and is invoked as `apply_cha()` on both
indexing paths — the same pipeline position GM-12.3 describes the effect closure running after.
RTA follows it and re-stamps pruned edges `rule = "rta-pruned"`. What does not exist is the
*spelling* `cha-override`, and the sentinel heuristic behind `heuristic-sentinel` (see Q-13 in
`docs/05-queries.md`, where the fan-out/sentinel flags are Planned).

To see the rule and tier on a real edge, use `cgx explain <SYMBOL>` (GM-6.2), which prints
`tier=` and `rule=` per incident edge.

### GM-6.2 — Querying provenance

*(Runs shown here are against the shared example repository defined in [`docs/05-queries.md` → "The example repository"](05-queries.md#the-example-repository); its recipe reproduces tree OID `7380e245ab98db2dfed12ed3ee3b005f0fbf7036` exactly.)*

**There is no `--evidence` flag.** Provenance is surfaced by a dedicated subcommand,
`cgx explain <SYMBOL>`, which prints every direct incident edge of one symbol with its
condition, confidence, resolution tier, producing rule and call site:

```console
$ cgx explain helper --repo .
rust_sample::helper  (src/main.rs:20)
  kind: function
  callers: 1, callees: 0
  edges:
    <- rust_sample::middle  (src/main.rs:15)  [always]  [certain]  tier=scope_graph  rule=scope-ref  site=src/main.rs:15
freshness: current | indexed tree 7380e24, working tree clean
```

That is narrower than a `--evidence` modifier on every query would be — it is one symbol and
one hop, not a provenance record attached to each row of an arbitrary result — but it is the
shipped mechanism, and it is the only place the resolution tier is exposed on the CLI.
`tier=` takes one of `name_syntactic`, `scope_graph`, `scip`, `cha_rta`, `points_to`.

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
| Java | `public static void main(String[])`, `@Test` methods |
| Python | `if __name__ == "__main__"` block, pytest functions named `test_*` |
| Go | `func main()` in package `main`, `func Test*(*testing.T)` |
| JavaScript / TypeScript | Top-level `export default`, Jest `test()`/`it()` calls |

**Every framework/HTTP-handler row is Planned and has been removed from the table above** —
Spring `@Controller`/`@RestController`, Django/Flask routes, Express/Fastify handlers. The
`EntrypointKind::HttpHandler` variant is defined in `crates/cgx-core/src/node.rs` and is
constructed **nowhere**: it has zero producers across all five language adapters, so no symbol
in any indexed repository carries it. `EntrypointKind::Declared` is likewise never constructed
(see GM-7.2).

The practical consequence lands on `cgx unused`: a handler invoked only by a web framework's
dispatcher is not recognised as a root, so its entire call tree can be reported as unreachable.
That is the reason `unused` answers carry a `| scope:` tail naming what was actually searched.

### GM-7.2 — Explicit entrypoint declaration — **Planned**

Neither half of this exists. `cgx.toml` parsing is a hand-rolled single-key line scanner that
understands only `[index] data_flow = <bool>` — cgx carries no `toml` dependency by design — so
an `[[entrypoints]]` table is silently ignored rather than rejected. There is no
`--entrypoint` flag on `cgx index` or on any query subcommand; passing one is a clap parse
error, exit 2. Until this ships, the auto-detected set in GM-7.1 is the whole entrypoint set
and it cannot be extended.

The design: users declare entrypoints via a configuration file or CLI flag. This is
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
| `macro_rules!` (declarative macro) | Call sites textually visible inside invocation arguments are extracted best-effort as `possible` edges, provenance rule `macro-textual` |
| Proc-macro invocation / annotation | Code generated by the proc macro is a labeled blind spot: each site emits a cut-marker `unexpanded-macro` (GM-5.3) so the gap is queryable and countable; expansion-based resolution only under `cgx index --rust-expand` (Status: roadmap) |

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

### GM-8.5 — C / C++ — **Planned; no C or C++ adapter exists**

Unlike GM-8.1 through GM-8.4, this section describes a language cgx **cannot index at all**.
There is no `cgx-lang-c` or `cgx-lang-cpp` crate and no C grammar dependency in the workspace;
the heuristics below (sentinel checks, FFI, macro expansion, vtable dispatch) are the design
for an adapter that has not been written. The same C sentinel-check row appears unhedged in
GM-3.1 and should be read the same way.

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

## GM-9 — Spawn Edges

**Status:** `schema-room` — the edge kind and its domain interaction with
path-relative transience must be reserved in the schema now; the full
concurrency-query surface (Q-24) is a later implementation milestone.

`spawns` is an **edge kind**, orthogonal to the five-value edge-condition label
set. A `spawns` edge from symbol A to symbol B means: A initiates asynchronous,
detached execution of B without waiting for its completion. The spawner does not
hold a reference to the spawned task's return value at the call site; errors
that occur inside the spawned task do not propagate back to A's call stack.

### GM-9.1 — Spawn edge kind vs edge condition

The `spawns` kind and the edge-condition label are independent attributes on the
same edge record:

| Field | Values | Orthogonality |
|---|---|---|
| `edge_type` | `spawns` (or `calls`, `calls:async`, etc.) | Classifies the structural relationship |
| `edge_condition` | `always` / `conditional` / `loop` / `exception` / `panic` | Classifies the guard at the call site |

A spawn inside a `catch` block is a `spawns` edge with `edge_condition =
exception`. A spawn in a loop is `spawns` with `edge_condition = loop`. The
condition describes when the spawn is initiated, not what happens inside the
spawned task.

### GM-9.2 — Detached error domain

**Exceptional propagation terminates across `spawns` edges.**

A path that crosses a `spawns` edge enters a **detached error domain**: the
spawned task's error-handling machinery is independent of the spawner's. This
has two consequences for path-relative transience (GM-4):

1. **The `seen_exceptional` flag does not cross `spawns` edges.** A walk that
   enters a spawned task via a `spawns` edge reinitialises `seen_exceptional =
   false` for paths inside that task. Exception-class edges inside the spawner
   do not make callees inside the spawned task exception-transient.

2. **Exception edges inside the spawned task do not propagate back to the
   spawner.** There is no call-stack unwind from task to spawner; the spawned
   task's panics, unhandled rejections, or task-completion errors surface only
   through the task's own completion handle (if one exists — see LS-7 for
   per-language behavior).

Queries that ask "which paths from entrypoint E carry an exception-class edge
before reaching sink S" correctly exclude paths whose only exception-class edge
is inside a spawned task.

### GM-9.3 — Per-language lowering

Per-language spawn constructs are specified in docs/08-language-support.md
(LS-7). The table there maps each language's spawn surface to `spawns` edges
with appropriate edge conditions.

### GM-9.4 — Relationship to `calls:async`

`calls:async` (GM-2.1) models an `await` point: the caller suspends but retains
ownership of the result — control returns when the callee completes. `spawns`
models detached launch: the caller does not wait. The distinction is queryable
and is the basis for "await while holding a lock" vs "spawn under a lock"
queries (see Q-24).

---

## GM-10 — Suspension Points

**Status:** `schema-room` — the `suspends` property must be reserved now so
that TOCTOU-class queries (Q-24) can be expressed without a schema migration; full
query support follows.

A **suspension point** is a call site or expression where the current execution
context yields control to a scheduler or event loop before resuming. Between the
suspension and resumption, any shared mutable state may have changed.

### GM-10.1 — The `suspends` node property

`cgx` attaches a boolean `suspends = true` attribute to any call-site node
(GM-1.4) that is a known suspension point. This property is node-level: it annotates the
call site in the caller, not the callee.

`suspends` is the **TOCTOU primitive** in the graph model. A query of the form
"is there an `await` between the validation of X and the use of X?" is
expressed as a path predicate: "does any node on the path from validation to use
have `suspends = true`?" The `async-toctou` query pattern in Q-24 is built on
this property.

### GM-10.2 — Per-language lowering

| Language | Suspension construct | `suspends` emitted on |
|---|---|---|
| Rust | `.await` expression | The `calls:async` edge's source call-site node |
| JavaScript / TypeScript | `await` expression | The `calls:async` edge's source call-site node |
| Python | `await` expression | The `calls:async` edge's source call-site node |
| Go | Channel send/receive (`<-`), `select` | The channel-operation node |
| Java | `CompletableFuture.get()`, `Future.get()` blocking join; not `thenApply` (which is non-blocking continuation) | The blocking-join call-site node |
| Kotlin | `suspend fun` call site; `await()` on `Deferred` | The suspend-call-site node |

Full per-language detail is in docs/08-language-support.md (LS-7).

### GM-10.3 — Interaction with lock sets

A suspension point while holding a lock is the canonical "await-under-lock"
bug. The combination of `suspends = true` and a non-empty lock set at the call
site (GM-11) is queryable as a first-class predicate. See Q-24 for the query
surface.

---

## GM-11 — Synchronization Context and Lock Sets

**Status:** `schema-room` — the lock-set attribute must be reserved in the
schema alongside GM-9 and GM-10; lock-set queries (Q-24) depend on it.

The **synchronization context** at a call site is the set of
synchronization objects that are **held** (acquired but not yet released) when
that call site executes. `cgx` models this as a **lock set** attribute on
call-site nodes (GM-1.4).

### GM-11.1 — Lock set attribute

Each call-site node (GM-1.4) in the graph carries:

```
lock_set: Set<symbol>   # symbols of synchronization objects held at this site
```

A symbol is in `lock_set` if a path from an acquire call for that symbol to
this call site exists with no intervening release call for the same symbol on
any path reaching this site. Lock-set computation is a path-sensitive property;
`cgx` computes a **conservative over-approximation** (the set of locks that
_may_ be held, not a proof that all are held) using the acquire/release pair
model from GM-13.

### GM-11.2 — Uses of the lock set

Lock-set attributes enable three classes of queries (see Q-24):

| Query | Predicate |
|---|---|
| Await-under-lock | Call site has `suspends = true` AND `lock_set` is non-empty |
| Inconsistent lock set | Field `F` is accessed from ≥ 2 call sites whose `lock_set` attributes do not share a common lock |
| Blocking-call-in-async | Call site has a `blocking` effect (GM-12) AND is reachable from an async entrypoint |
| Double-acquire | Acquire path for lock L reaches another acquire site for L with no intervening release |

### GM-11.3 — Lock set and `spawns` edges

Lock sets do not cross `spawns` edges. The spawned task starts with an empty
lock set regardless of what the spawner holds. This reflects the runtime
reality: the spawner's mutex guards do not transfer to a detached task.

---

## GM-12 — Function Effect System

**Status:** `core-extension` — effect sets are high-leverage metadata
derivable from the existing call graph structure and edge taxonomy; they are
core to the security and correctness query families planned from the outset.

Every function node in the graph carries an **effect set**: the union of
effects that function may exhibit, computed transitively over the call graph.

### GM-12.1 — Effect lattice

**The shipped `Effect` enum has seven variants, not eleven.** `pure` is the absence of any
variant rather than a variant of its own; `reads-global`, `writes-global` and `io.db` do not
exist anywhere in the workspace and are Planned.

| Effect | Status | Meaning |
|---|---|---|
| `blocking` | shipped | May block the calling thread (sync I/O, `thread::sleep`, `std::mutex::lock`) |
| `spawns` | shipped | Initiates a detached concurrent task |
| `io.file` | shipped | File system read or write |
| `io.net` | shipped | Network socket read or write |
| `io.proc` | shipped | Subprocess execution or inter-process communication |
| `dynamic-code` | shipped | Executes dynamically specified code (`eval`, `exec`, `dlopen`, `Class.forName`) |
| `nondeterministic` | shipped | Depends on time, randomness, or environment variables |
| `pure` | — | Not a variant: an empty effect set *is* purity |
| `reads-global` | **Planned** | Reads module-level or process-global mutable state |
| `writes-global` | **Planned** | Writes module-level or process-global mutable state |
| `io.db` | **Planned** | Database read or write |

The two `-global` values matter beyond this table: `docs/04-dataflow-and-provenance.md` DF-17
builds its mutability model on `writes-global` as though GM-12 already had it. It does not.

`pure` is the bottom element: a function declared `pure` may not have any other
effect. The remaining values are not totally ordered; a function may carry any
subset of the non-`pure` values simultaneously.

### GM-12.2 — Transitive computation rule

A function's effect set is computed as follows:

1. **Own effects**: assigned from the function's body by the producing rule
   (e.g. a call to `File::open` contributes `io.file`; a call to `tokio::spawn`
   contributes `spawns`).
2. **Called-function union**: the function's effect set includes the union of
   the effect sets of all functions it reaches via `calls` (and `calls:*`)
   edges, recursively.
3. **Spawned-function attribution**: effects of functions reachable via `spawns`
   edges are **not** unioned into the spawner's own effect set by default. They
   are attributed to the spawner via the `spawns` edge and are queryable
   separately (e.g. "which effects does this spawner's task graph reach?").
   This keeps the spawner's direct effect set accurate: a function that only
   spawns work and does nothing else is `spawns`-effect only, not
   transitively `io.file` through its spawned tasks.

The spawns-attribution rule is the explicit design choice: tools that naively
union spawned effects into spawners produce false positives ("this async
handler has `io.file` effect") that obscure the real question ("is this async
handler's spawned task doing file I/O independently?").

### GM-12.3 — Effect set storage

Effect sets are stored as a bit vector on the function node, and both attributes are real and
populated.

- **The transitive closure ships and runs on every index.** `crates/cgx-resolve/src/effects.rs`
  computes it as a fixpoint over strongly-connected components — an iterative Tarjan, so mutual
  recursion terminates and every member of a cycle shares one transitive set — and
  `cgx-index/src/pipeline.rs` wraps it as `apply_effects()`, which `cgx-index/src/lib.rs` invokes
  on both indexing paths. It runs
  **last**, after the SCIP/CHA/RTA/signature confidence passes have settled, so the closure rides
  the improved edge precision; it mutates only node effect attributes, so no re-canonicalization
  is needed. `crates/cgx-index/tests/effects_closure.rs` asserts population end-to-end through
  the pipeline.

  The propagation rule is GM-12.2's, enforced: effects flow over the call family but **not** over
  `Spawns`, so a function that only launches work carries `spawns` without inheriting its task's
  `io.file`.

  *(The doc comment on the `transitive_effects` field in `cgx-core/src/node.rs` still says
  "Unpopulated in Phase 1 — always empty until the P8b closure pass fills it." That comment is
  stale: P8b landed. Do not take it as the status of the field.)*

- **What is missing is the query surface, not the computation.** Neither attribute is reachable
  from CQL: `transitive_effects` is rejected with an explicit not-supported plan error, and
  `own_effects` falls through to the generic unknown-node-property error. The effects are
  computed and stored on every index; there is currently no way to ask about them. Effect
  *queries* are a later phase than effect *extraction*.

### GM-12.4 — Cross-language effect lowering

Per-language lowering of construct → effect value is specified in
docs/08-language-support.md (LS-7).

---

## GM-13 — Resource Lifecycle Pairs (Acquire/Release)

**Status:** `schema-room` — the pair representation and release-on-all-paths
query must be reserved now; full query semantics live in Q-22 (forward citation).

A **resource lifecycle pair** is a declared `(acquire, release)` symbol pair
asserting that every execution path from an acquire call must reach a
corresponding release call — including paths in the exceptional class.

### GM-13.1 — Pair declaration

Pairs are declared in `cgx.toml` or built into the per-language defaults:

```toml
# cgx.toml — user-defined resource pair
[[resource_pairs]]
acquire = "myapp::db::Connection::begin"
release = ["myapp::db::Connection::commit", "myapp::db::Connection::rollback"]
```

The `release` field accepts a list: any one of the listed symbols satisfies the
release obligation. This models transactions (either `commit` or `rollback`
closes the resource) and multiple close paths.

### GM-13.2 — Built-in per-language defaults — **Planned**

Despite the present tense below, **no resource-pair machinery exists**: there is no
`ResourcePair` type in `cgx-core`, no acquire/release harvesting in any language adapter, and
no `cgx.toml` table to configure one (the parser reads a single key, `[index] data_flow`). The
table is the intended default set, not a shipped one. This section's own `Status: schema-room`
header is the accurate signal; the present-tense body was not.

`cgx` would ship with built-in defaults for common resource pairs, active
without configuration:

| Language | Acquire | Release |
|---|---|---|
| Java | `InputStream.open`, `Connection.getConnection`, `Lock.lock` | `close()`, `Connection.close()`, `Lock.unlock()` |
| Python | `open()` | `close()`, `__exit__` |
| Python | `threading.Lock.acquire` | `threading.Lock.release`, `__exit__` |
| Rust | `Mutex::lock` | `MutexGuard` drop (implicit at scope end; modelled as release on the scope-exit edge) |
| Go | `sync.Mutex.Lock` | `sync.Mutex.Unlock` |
| Go | `os.Open` | `(*os.File).Close` |
| JavaScript | `fs.open` | `fs.close` |
| Java | `synchronized` block enter | `synchronized` block exit (implicit; modelled on scope-exit edge) |

Users can add pairs for any framework pattern (database sessions, HTTP client
builders, file streams, custom transaction abstractions) via `cgx.toml`.

### GM-13.3 — The release-on-all-paths question

The motivating query for resource pairs is:

> Is release reached on ALL paths from acquire, including exception-class paths?

This is not a reachability query (∃-path). It is a **must-reach** query
(∀-path): every execution path from the acquire call site must pass through a
release call. Paths in the exceptional class (`exception` or `panic` edge
condition) are explicitly included; a resource leaked only on the exception path
is still a leak.

This query pattern is specified in Q-22 (forward citation). Its foundation is
path-relative transience (GM-4): the exceptional-class path machinery that GM-4
defines for queries like "which callees are only reached through error handling"
generalises directly to "does every exception-class path from acquire reach
release."

### GM-13.4 — Typestate (roadmap)

**Status (this subsection only):** `roadmap` — full typestate is design-room
only; no schema commitment is made here.

Typestate generalises resource pairs to arbitrary protocol sequences: "methods
valid only after `init()`", "no `write()` after `close()`", "every `begin()`
reaches exactly one of `commit()` or `rollback()`." The acquire/release pair
model in GM-13.1–13.3 captures roughly 80% of the real bugs in this family at
a fraction of the cost. Full typestate — tracking arbitrary state machines over
method call sequences — is reserved for a later design milestone.

The graph model does not preclude typestate: the pair-declaration schema in
GM-13.1 is a degenerate two-state automaton, and the schema can be extended to
n-state automata without restructuring the node/edge model.

---

## GM-14 — Code-Trust Boundaries

**Status:** roadmap — and, unusually for a `schema-room` section, **the room has not been
reserved either.** `unsafe_region` and `ffi_language` do not exist on `NodeRecord` or
`EdgeRecord`, not even as placeholder fields; the `dependency{package, version, ecosystem}`
attributes are likewise absent from `cgx-core`. The `via-ffi` cut marker (GM-5.3) is real and is
the only part of this section with a shipped counterpart. Read "must be reserved now" as a
statement of intent that has not been acted on, and the present-tense phrasings below ("are
annotated", "is queryable") as design.

The attributes described here would extend the existing
cut-marker and confidence machinery (GM-5) with queryable metadata; they must
be reserved so that CVE-reachability and dependency-graph queries (Q-25)
resolve correctly.

Code-trust boundaries are regions or transitions in the call graph where the
confidence of static analysis degrades and where external trust assumptions
enter. `cgx` makes each such boundary explicit and queryable.

### GM-14.1 — Unsafe regions (Rust)

Rust `unsafe` blocks and `unsafe fn` declarations are annotated on the
enclosing function node:

```
unsafe_region: true | false
```

Any function node with `unsafe_region = true` — or that transitively calls a
function with `unsafe_region = true` — is queryable as a trust boundary.
Confidence does not automatically degrade at unsafe boundaries (the static
call graph is still resolved), but unsafe regions are surfaced as a filter so
that security queries can include or exclude them explicitly.

### GM-14.2 — FFI boundaries

Calls crossing a foreign-function boundary carry the existing `via-FFI`
cut-marker (GM-5.3). At GM-14 level, the additional attribute is:

```
ffi_language: string    # e.g. "C", "C++", "JNI", "Python C extension"
```

Confidence is `possible` for the resolved target (the foreign symbol name is
known but behaviour inside it is not modelled). The cut-marker edge is still
emitted; query authors filter it in or out according to their question.

### GM-14.3 — Dependency edges (package and version)

Every `calls` or `spawns` edge that crosses a module boundary into a
third-party dependency carries additional attributes:

```
dependency {
  package:   string    # package name (e.g. "serde", "log4j-core", "requests")
  version:   string    # version string as resolved by the package manager
  ecosystem: string    # "crates.io", "maven", "pypi", "npm", "go"
}
```

These attributes enable CVE-reachability queries (Q-25): given a CVE that
identifies a vulnerable symbol in `package@version`, determine whether any
path from a declared entrypoint reaches that symbol, with what confidence, and
on what edge condition.

Confidence degrades for dependency edges when the resolved target is inside a
dependency that has no SCIP index: edges fall to `probable` (unique name) or
`possible` (ambiguous name) per the standard confidence ladder (GM-5.2).

### GM-14.4 — Reflection and dynamic dispatch uncertainty

Reflection sites and dynamic dispatch with unresolved candidate sets are
already marked with cut-marker values `reflective` and `dynamic` (GM-5.3). At
GM-14 level, these are additionally surfaced as explicit trust-boundary
transitions: any path that traverses a `reflective` or `dynamic` cut-marker
edge has reduced analytical confidence for all downstream nodes.

Confidence degradation rule: any node reachable only via at least one
`reflective` or `dynamic` cut-marker edge carries at most `possible` confidence,
regardless of the resolution tier that produced the edge.

### GM-14.5 — Macro and codegen provenance

Edges produced from macro-expanded or code-generated source carry a provenance
`rule` tag identifying the expansion source (e.g. `macro_rules!`,
`proc-macro`, `annotation-processor`, `codegen-template`). This is already
supported in GM-6; GM-14 adds the queryable attribute:

```
macro_origin: string | null    # name of the macro or generator, if applicable
```

**Declarative macros (`macro_rules!`):** call sites textually visible inside invocation arguments are extracted with provenance `rule: macro-textual`. The `macro_origin` attribute names the macro.

**Proc-macro provenance:** expansion-side provenance fields (span mapping from generated to source, `proc-macro` rule tag) have **Status: schema-room** — populated only under `cgx index --rust-expand` (roadmap). Until then, proc-macro invocation sites emit an `unexpanded-macro` cut marker (GM-5.3) rather than expansion-derived edges.

Edges with a non-null `macro_origin` have reduced human auditability: the
developer did not write the call site directly. Security queries can filter for
"all paths that include at least one macro-generated call edge" to identify
code whose control flow may not match developer intent.

Confidence is not automatically degraded for macro-expanded edges (the
expansion is static and fully visible to `cgx`), but `macro_origin` is queryable
so that query authors can apply their own confidence policy.

### GM-14.6 — Type-confidence boundaries

A **type-confidence boundary** is a program point where the static type of a
value stops being evidence for call-target resolution or taint classification.
Beyond this point, the static type carries no constraint on which methods are
dispatched or which data flows through.

Type-confidence boundaries arise from language escape hatches for the static
type system:

| Language | Construct | Effect |
|---|---|---|
| TypeScript | `any` | Type checker stops constraining; all method calls are `possible` resolution |
| Go | `interface{}` / `any` | Concrete type erased; interface method calls require type-switch or runtime assertion |
| C# | `dynamic` | Dispatch deferred to the DLR at runtime; `cgx` cannot resolve the callee statically |
| Any | Reflection result | Return type is `Object` or equivalent; concrete type erased (see GM-14.4, GM-18) |
| Any | Type-assertion / cast from broad interface | Confidence partially restored if the assertion target is a concrete type visible to `cgx` |

The confidence-degradation rule at type-confidence boundaries matches the
`reflective`/`dynamic` cut-marker rule in GM-14.4: any node reachable only via
at least one type-confidence-boundary transition carries at most `possible`
confidence.

Type-confidence boundaries are in the same uncertainty family as unsafe regions
(GM-14.1), FFI boundaries (GM-14.2), and reflection sites (GM-14.4). The
`any`-frontier query — "where does a dynamically typed value enter my
statically typed code?" — is specified in DF-19 (lineage type reconstruction)
and Q-30 (coercion and type-confidence queries).

---

## GM-15 — Metadata and Annotation Facts

**Status:** `core-extension` — without metadata facts, entrypoint sets, guard
checks, and dead-member analysis are incorrect on framework-heavy code, which
describes the majority of real codebases.

Annotations (Java), attributes (C#, Rust), decorators (Python, TypeScript),
and struct tags (Go) are **metadata facts on symbols**. `cgx` models them as
graph facts, not as a new node type: metadata is an attribute of the symbol node
it annotates. The semantic interpretation of metadata — what it means for
reachability, call routing, or analysis — is supplied by **framework packs**
(see docs/12-language-primitives-and-frameworks.md for the pack configuration
format and per-framework catalogs).

### GM-15.1 — Metadata semantic classes

Every piece of metadata that `cgx` processes is lowered to at least one of the
following **semantic classes**. A single annotation may lower to more than one
class.

| Class | Meaning | Machinery it feeds |
|---|---|---|
| `entrypoint` | The annotated symbol is a reachability root | Populates the entrypoint set (GM-7); reachability is wrong without this on framework code |
| `guard` | The annotated symbol carries an authz/authn check that is the annotation itself — there is no call in the source path | Must-pass-through predicate (Q-20); false authz-bypass findings result if this class is absent |
| `negative-guard` | The annotated symbol disables a protection that would otherwise apply | "Which endpoints disable X?" security query family |
| `interception` | The call to the annotated symbol is rewritten at runtime to pass through a proxy, advice, or wrapper | Call-graph rewriter; adds a mediated `calls` edge (see GM-17); `@Async` additionally converts the call to a `spawns` edge (GM-9) |
| `generated-member` | The annotated symbol, or symbols derived from it, exist with no source the developer wrote | Produced symbols receive `macro_origin` provenance (GM-14.5); call targets resolve against generated symbols |
| `keep-alive` | The annotated field or method is used reflectively or by serialization, even if no static call site exists | Suppresses DF-8 dead-member false positives on annotated members |
| `contract` | The annotation carries a nullability, precondition, or type contract | Feeds DF-14 (nullability and optionality flow) |

### GM-15.2 — Lowering to graph facts — **Planned; no lowering exists**

**GM-15 as a whole is unimplemented, and its `Status: core-extension` header overstated it more
than any other section in this document.** A repo-wide grep of `crates/**/*.rs` finds zero
occurrences of `semantic_class`, `FrameworkPack`, `negative-guard`, `keep-alive`,
`generated-member`, `PreAuthorize`, `Spring`, `Guice`, `NestJS`, `FastAPI` or `Django`. There
is no annotation-to-semantic-class mapping, no metadata-fact record, and no framework-pack
mechanism to host one. The `@PreAuthorize` worked example below does not run.

What this costs the reader is worth stating plainly, because the missing facts are load-bearing
elsewhere: `keep-alive` is what would suppress `cgx unused` false positives on
reflectively-used members, and `negative-guard` is what the "which endpoints disable
authentication" query family would filter on. Neither suppression happens today.

The design follows. The lowering would be performed at index time by the active framework packs. Each
class maps to a concrete graph-fact change:

- `entrypoint`: the symbol node's `kind` is promoted to (or additionally tagged
  as) `entrypoint`; the symbol is added to the entrypoint set used by GM-7.
- `guard`: a `must-pass-through` fact is recorded on the symbol node,
  queryable as a Q-20 predicate.
- `negative-guard`: a `protection-disabled` attribute is set; queryable as a
  filter in authz-bypass queries.
- `interception`: a `calls` edge is inserted from the annotated symbol's call
  site to the intercepting proxy or advice symbol, with `established-by:annotation`
  (see GM-17). When the annotation is `@Async` (or equivalent), the inserted
  edge is `spawns` (GM-9) rather than `calls`.
- `generated-member`: generated symbols are indexed with `macro_origin` set to
  the generator name and `implicit_source: true`; call edges to them are valid
  targets.
- `keep-alive`: the symbol node receives `keep_alive: true`; DF-8 dead-member
  analysis treats it as referenced.
- `contract`: the nullability or contract fact is stored as a DF-14 annotation
  on the symbol node.

### GM-15.3 — Motivating example: Spring @PreAuthorize

```java
@GetMapping("/admin/delete")
@PreAuthorize("hasRole('ADMIN')")
public void deleteUser(String id) { ... }
```

Without metadata facts:

- `deleteUser` is not in the entrypoint set — reachability analysis is blind to
  it.
- The authz check `hasRole('ADMIN')` has no call in the source path — a
  must-pass-through query (Q-20) asking "is there an authz check before
  `deleteUser`?" finds none and reports a false authz-bypass.

With the Spring pack active:

- `@GetMapping` lowers to `entrypoint` — `deleteUser` enters the reachability
  set.
- `@PreAuthorize` lowers to `guard` — the must-pass-through fact is recorded;
  Q-20 queries resolve correctly.
- A query for "endpoints whose only authz mechanism is a `negative-guard`"
  (`@PermitAll`, `@AllowAnonymous`) finds real bypass candidates, not noise.

### GM-15.4 — Framework pack scope

The pack configuration format, the built-in pack catalog (Spring, Guice,
NestJS, FastAPI, Django, Axum, and others), and the user-extension mechanism
are specified in docs/12-language-primitives-and-frameworks.md. This section
defines only the graph-fact semantics that packs produce.

---

## GM-16 — Implicit Call Sites

**Status:** `core-extension` — implicit calls are real control-flow edges;
omitting them produces incorrect effect sets (GM-12), broken resource-pair
tracking (GM-13), and missed reachability.

An **implicit call site** is a location in source code where a function is
invoked without explicit call syntax. `cgx` models these as `calls` edges with
an `implicit:<kind>` attribute that identifies the language mechanism.

### GM-16.1 — Implicit kind enum

The `implicit` attribute on a `calls` edge takes exactly one of the following
values:

| Kind | Description | Language examples |
|---|---|---|
| `drop` | Destructor / finalizer called at scope exit | Rust `Drop::drop`, C++ destructor |
| `destructor` | Named destructor syntax (distinct from drop in C++) | C++ `~MyClass()` explicit call in derived class |
| `deref` | Implicit dereference coercion triggers a method call | Rust `Deref` / `DerefMut` coercion |
| `coercion` | Implicit type conversion invokes a conversion function | Rust `From`/`Into` via `?`; C++ conversion operators |
| `operator` | Arithmetic, comparison, or index operator desugars to a method call | Rust `Add::add`, Python `__add__`, C++ `operator+` |
| `iterator` | A `for`/`foreach` loop desugars to iterator protocol calls | Rust `Iterator::next`, Python `__iter__`/`__next__`, C++ range `begin`/`end` |
| `context-enter` | A context-management construct calls the entry function | Python `with` (`__enter__`); C# `using` enter; Java `try-with-resources` open |
| `context-exit` | A context-management construct calls the exit / cleanup function | Python `with` (`__exit__`); C# `using` dispose; Java `try-with-resources` close |
| `property` | A field-access expression desugars to a getter/setter call | Python `@property`, C# `get`/`set` accessors, Kotlin `get`/`set` |
| `defer` | A deferred call is registered at one site and executes at scope exit | Go `defer`; runs on panic path too — see GM-3 and GM-13 |
| `static-init` | A static or class-level initializer runs before first use | Java `static {}` block; Go `init()`; Python module-level code |
| `conversion` | An explicit conversion syntax desugars to a conversion function | Python `int(x)`, `str(x)`; Swift `Int(x)` |

Per-language mapping of each construct to an `implicit:<kind>` edge is
specified in docs/08-language-support.md (LS-8).

### GM-16.2 — Relationship to resource lifecycle pairs — **Planned**

`ImplicitKind` is a real twelve-variant enum, but **only two variants are ever constructed
anywhere in the workspace**: Go's `Defer`, and one `Iterator` site in the Rust adapter. No
adapter harvests `drop`, `context-enter`/`context-exit`, `deref`, `coercion`, `operator`,
`property`, `static-init` or `conversion`. The harvest described below is therefore doubly
unbuilt: the kinds are not produced, and GM-13's resource-pair mechanism does not exist to
receive them.

**Nor is there any query surface for them, and that is the good news.** `implicit` is not part
of the CQL vocabulary at all, so a query that tries to filter on one **fails loudly** rather
than returning a misleading empty result:

```console
$ cgx query 'MATCH (a)-[r:IMPLICIT]->(b) RETURN a.fqn' --repo .
cgx: plan error: unknown edge type `IMPLICIT`
$ cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE r.implicit = "defer" RETURN a.fqn' --repo .
cgx: plan error: unknown edge property `implicit`
```

Both are exit 2. This is the failure mode the language is supposed to have for an unbuilt
feature — the reader is told, rather than handed zero rows to misread.

**Rust's one `Iterator` construction produces no edge either, for a reason worth knowing.** It
fires on every `for` loop, but its target is `core::iter::Iterator::next` — a stdlib symbol that
is never in the indexed repository. The reference therefore resolves to nothing and is counted
as a dangling ref in `unresolved_calls`, not materialised as an edge. So Go's `defer` is the
only implicit-call construct that reaches the graph at all.

The design: the constructs `context-enter` / `context-exit`, `defer`, and `drop` are the
**language-native declarations of GM-13 resource pairs**. Framework packs and
per-language rules would harvest these syntax forms automatically:

- Python `with` blocks desugar to `(__enter__, __exit__)` pairs — `cgx` records
  them as resource pair instances without user configuration.
- Java `try-with-resources` emits `(open, close)` pairs via the `context-enter`
  / `context-exit` implicit edges.
- Rust `Drop` creates a `(acquire, drop)` pair at the scope boundary; the
  release edge is the implicit `drop` call.
- Go `defer` registers the release; `cgx` pairs it with the corresponding
  acquire call site and records the `defer` kind on the release edge.

The must-reach-on-all-paths query (GM-13.3, Q-22) applies to these implicit
pairs identically to user-declared pairs.

### GM-16.3 — Effect set impact

Implicit call edges participate fully in transitive effect computation (GM-12).
A `defer` call to a function with `io.file` effect contributes `io.file` to the
enclosing function's transitive effect set; an implicit `Drop` that closes a
socket contributes `io.net`.

---

## GM-17 — Mediated Call Edges

**Status:** `core-extension` — dependency injection, event dispatch, and
registry patterns are the dominant architectural patterns in enterprise
codebases; modelling them closes the largest gap in call-graph completeness for
those codebases.

A **mediated call edge** connects a caller and a callee that are not syntactically
linked — the binding is established by a container, event bus, or registry at
runtime. `cgx` models these as `calls` or `constructs` edges carrying an
`established-by` attribute that records the binding evidence.

### GM-17.1 — Edge attributes for mediated calls

```
established-by: annotation | config-file | registration-site | convention
confidence:     certain | probable | possible
```

The `established-by` value records the evidence class:

| Value | Meaning | Example |
|---|---|---|
| `annotation` | The binding is declared by a metadata annotation on one or both symbols | Spring `@Autowired`, Guice `@Inject`, NestJS `@Injectable` |
| `config-file` | The binding is declared in an external configuration file | Spring XML, Guice Module class, CDI beans.xml |
| `registration-site` | The binding is established by an explicit register/subscribe call in source | `eventBus.subscribe(MyHandler.class)`, `router.register("/path", handler)` |
| `convention` | The binding is inferred from a naming or structural convention | Spring `@Component` name-based injection, Go `init()` side effects |

### GM-17.2 — Confidence rules

| Evidence class | Confidence | Rationale |
|---|---|---|
| Compile-time DI (Dagger, Wire) | `certain` | Binding is resolved and type-checked at compile time; the generated code is present in the graph |
| Runtime container with annotation evidence (Spring, Guice, NestJS) | `probable` | Container resolves the binding at startup; the wiring annotation is direct evidence even though resolution is deferred |
| Convention-based or config-file binding | `probable` | Convention evidence is strong but requires the convention to hold exactly |
| Event bus or signal/slot without type constraint | `possible` | The subscriber set cannot be fully determined without runtime type information |

### GM-17.3 — Relationship to cut markers

**GM-17 is Planned in its entirety.** `EstablishedBy` is a real four-variant enum on
`EdgeRecord` (`established_by: Option<EstablishedBy>`), but every construction site in the
workspace sets it to `None`; the only place a non-`None` value appears is a round-trip
serialization test. No DI, annotation, event-bus or registry detection logic exists to set it.
The `via-di` cut marker is in the same position: defined, never emitted.

In the design, mediated call edges that cannot be resolved to a unique callee retain the
`via-DI` cut marker (GM-5.3) alongside the `established-by` attribute. The cut
marker signals that the edge required external evidence; the `established-by`
attribute records what evidence was used. Both are independently queryable.

### GM-17.4 — Examples

```
// Spring @Autowired — annotation-mediated construction
// established-by: annotation, confidence: probable
UserService → UserRepository  (constructs, established-by:annotation)

// Dagger 2 — compile-time DI
// established-by: annotation, confidence: certain (generated code visible)
AppComponent → DatabaseModule  (constructs, established-by:annotation)

// Event bus — registration-site evidence
// established-by: registration-site, confidence: possible
OrderService → ShippingEventHandler  (calls, established-by:registration-site)
```

---

## GM-18 — Reflection and String-Mediated Dispatch

**Status:** `core-extension` — reflection is present in most large codebases;
string-literal resolution recovers a meaningful fraction of otherwise invisible
edges, and tainted-string dispatch is a high-severity security pattern.

Reflection and string-mediated dispatch include `Method.invoke`, `getattr`,
`Class.forName`, `dynamic import()`, Go `reflect`, Ruby `send`, and analogous
constructs that resolve a call target from a string value at runtime.

### GM-18.1 — Resolution by string pedigree

When the string value reaching a reflective dispatch site has a **literal
pedigree** — that is, the `derives-from` chain (DF-1) traces back to a string
constant with no taint and no join with a non-literal source — `cgx` resolves
the edge to the named symbol:

| String pedigree | Confidence assigned | Example |
|---|---|---|
| Direct string literal | `probable` | `getattr(obj, "process_payment")` |
| Constant derived from a literal only | `probable` | `method = "process_" + "payment"; getattr(obj, method)` |
| Joined with a non-literal source | `possible` | Value from a join node (DF-9) where one arm is non-literal |
| No literal ancestry; fully dynamic | Unresolved; cut-marker `reflective` emitted | Method name from user input, config, or database |

The resolved edge carries the standard `reflective` cut marker alongside its
confidence label; the cut marker is never removed even for literal-pedigree
resolutions, because the dispatch is still dynamic at runtime.

### GM-18.2 — Tainted string reaching reflective dispatch

When the string reaching a reflective dispatch site is tainted (derived from
an untrusted source per DF-5), the dispatch is a **security-critical reflection
site**: an attacker who controls the method name controls which code executes.

`cgx` emits a taint-flow finding when a tainted string flows to any reflective
dispatch. This is a top-tier security query (Q-31) in the framework-aware query
family: "find paths where a tainted value reaches a reflective dispatch site."

The candidate-callee set in this case is explicitly marked `possible` with the
full set of methods matching the name pattern (if determinable from the taint
source's value set); otherwise the candidate set is recorded as empty with
`unresolved` status.

### GM-18.3 — Unresolvable dynamic dispatch

When the string is fully dynamic and the source of the method name cannot be
determined by pedigree analysis, `cgx` is honest:

- The edge is emitted with cut-marker `reflective` and confidence `possible`.
- The `candidate_set` attribute is populated if a bounded set of method names
  is inferrable (e.g. from an enum or a set of string constants that feeds the
  dispatch); otherwise it is empty.
- Queries that ask for "all paths through reflective dispatch" find these edges
  via the `reflective` cut marker regardless of whether the callee is resolved.

The soundiness statement applies: `cgx` does not claim to enumerate all dynamic
dispatch targets. Known unresolved dispatch sites are surfaced, not silently
dropped.

---

## GM-19 — Build-Configuration Variance

**Status:** `schema-room` — per-configuration graph indexing is roadmap; the
`cfg-condition` attribute is reserved now so that the schema does not need a
breaking change when that feature ships.

Build-configuration variance covers Rust `#[cfg(feature = "...")]`,
C/C++ `#ifdef`, Go build tags (`//go:build`), Python version guards
(`sys.version_info`), and analogous mechanisms that gate code inclusion on
compile-time or build-time conditions.

The call graph is **per build configuration**: a symbol or edge that exists
only when `feature = "legacy-auth"` is enabled is not present in graphs built
without that feature. A second dimension of branch-awareness beyond runtime
edge conditions (GM-3).

### GM-19.1 — The `cfg-condition` attribute

`cgx` records build-configuration conditions as an attribute on affected nodes
and edges:

```
cfg-condition: string | null
```

A non-null `cfg-condition` is the textual representation of the condition that
must hold for this node or edge to be present in the graph. Examples:

```
cfg-condition: "feature = \"legacy-auth\""
cfg-condition: "target_os = \"linux\""
cfg-condition: "go:build linux && amd64"
cfg-condition: "#ifdef ENABLE_TLS"
```

A null `cfg-condition` means the node or edge is unconditionally present.

### GM-19.2 — Query behavior under `cfg-condition`

Queries that traverse a node or edge with a non-null `cfg-condition` surface
the condition alongside the result:

```
Path: entrypoint → auth::validate → legacy::token_check
  Note: edge (auth::validate → legacy::token_check) exists only with
        cfg-condition: "feature = \"legacy-auth\""
```

Queries can filter to include or exclude cfg-conditioned nodes/edges:
`--exclude-cfg` omits any path segment that requires a non-default build
condition; `--cfg feature=legacy-auth` includes it.

### GM-19.3 — Per-configuration indexing (roadmap)

**Status (this subsection only):** `roadmap` — full per-configuration indexing
is not committed to the initial schema.

Full per-configuration analysis would index the graph once per distinct feature
combination, producing separate sub-graphs. Queries would then ask "does this
path exist in ANY configuration?" or "in ALL configurations?" or "only in
configuration X?"

The `cfg-condition` attribute is the schema-room for this: it is the
per-edge/per-node annotation that a per-configuration query engine would consume.
No restructuring of the node/edge model is required when this feature is
implemented.

---

## GM-20 — Error-Model Conversion Points

**Status:** `core-extension` — Go `recover` and Rust `catch_unwind` are common
idioms that convert panic-path flow back to value-path flow; stating the
conversion semantics explicitly prevents false positive exception-class tagging
downstream of these sites. Java checked exceptions give free precision on
exception-edge targets.

### GM-20.1 — Panic-to-value conversion

Go's `recover()` and Rust's `std::panic::catch_unwind` convert a panic (which
`cgx` models as a `panic`-conditioned path) back into an ordinary return value.
The conversion is a **point** in the graph, not a new edge-condition label.

The five-value edge-condition model (GM-3) is unchanged. The conversion is
expressed within the existing model as follows:

1. **Before the conversion point**: edges on the panic path carry
   `edge_condition = panic` as defined in GM-3.
2. **At the conversion point**: the `recover()` call or `catch_unwind` call
   site is the boundary node. The call site itself receives `edge_condition =
   panic` (it is only reached on the panic path).
3. **After the conversion point**: downstream edges from the conversion call's
   return value carry `edge_condition = always` or `edge_condition = conditional`
   (depending on whether the caller checks whether recovery succeeded) — the
   exceptional class does not propagate past this point.

This means `recover` and `catch_unwind` act analogously to a `catch` block in
Java or Python: they terminate the exceptional path and resume value-path
semantics. The distinction from `catch` is that they absorb `panic`-class edges,
not `exception`-class edges.

```
// Go example
go_panic_site          (edge_condition: panic)
    → defer_func       (edge_condition: panic)
        → recover()    (edge_condition: panic)   ← conversion point
recover_return_value   (edge_condition: conditional)  ← downstream: conditional on whether recover() returned non-nil
    → handle_error     (edge_condition: conditional)
```

### GM-20.2 — Interaction with GM-9 (detached error domains)

A `recover` inside a `defer` inside a goroutine (the canonical Go pattern)
operates within the goroutine's own detached error domain (GM-9.2). The panic
does not propagate to the spawner regardless of whether `recover` is present.
`recover` determines whether the goroutine itself exits normally or panics;
the spawner is unaffected either way.

`catch_unwind` in a Rust `tokio::spawn` closure similarly constrains the panic
within the task. Downstream edges from `catch_unwind`'s return value resume
always/conditional semantics inside the task; the `spawns` edge to the outer
context does not carry the panic class.

### GM-20.3 — Java checked exceptions

Java checked exceptions give `certain` confidence on exception-edge targets,
because the throws clause in a method signature is a static declaration that
`cgx` can read directly from the SCIP index or AST.

For a method declared `throws IOException`:

- The `throws` edge from the method to `IOException` carries `confidence:
  certain`.
- Call edges to that method on paths where `IOException` is not caught carry
  `edge_condition = exception` with `confidence: certain`.
- Callers that handle the exception in a `catch (IOException e)` block have
  calls inside the handler body labeled `exception` (they are only reached if
  the exception is thrown).

This contrasts with unchecked exceptions (`RuntimeException` and subclasses),
which are not declared in signatures and produce `exception`-conditioned edges
at `probable` or `possible` confidence only when the throw site is statically
visible.

The precision benefit is queryable: a query for "all certain exception-class
paths from `readConfig` to its callers" returns the declared-checked-exception
propagation chain, not an over-approximated set.

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

---

## GM-21 — `calls:super` Edge Kind — **Planned (not yet shipped)**

**Status:** roadmap. The premise is sound — `inherits`/`overrides` are real `EdgeKind` variants
and the call-site lowering exists, so this really is a re-classification rather than a new
entity kind — but the re-classification has not been done. `EdgeKind` has no `CallsSuper`
variant, and `CALLS:super` is in the CQL parser's recognised-but-deliberately-unbuilt Theme-13
set alongside `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD` and `FULFILLS`: it parses, then
plan-errors. That is the intended failure mode for this whole group — a deferred feature
announces itself rather than returning zero rows.

A `calls:super` edge denotes a statically-bound delegation to a *named ancestor's* method body that deliberately bypasses virtual dispatch. The edge's target is the resolved ancestor body (single target, not a candidate set), and it carries a `bypassed_override` attribute = the FQN of the override that virtual dispatch *would* have selected for the receiver's static type (when one exists).

**Per-language lowering:**

| Language | Construct |
|---|---|
| Python | `super().m()` |
| Java / C# / Kotlin | `super.m()` |
| Scala | `super.m` / `super[Trait].m` |
| Rust | `Trait::method(self)` / `BaseStruct::method(&self)` UFCS to a default |
| C++ | `Base::m()` |
| Ruby | `super` |

---

## GM-22 — Method-Resolution Order (Linearization)

**Status:** schema-room — raw `inherits`/`implements` edges exist, but the *ordered linearization* is a new stored entity the graph lacks today. C3/trait-linearization is language-specific computation that must be reserved now so MRO-dependent queries do not force a later schema migration.

A per-type ordered ancestor sequence `mro` attached to type nodes: the linearization the language uses to resolve a method or `super` call. For single-inheritance languages the MRO is the `inherits` chain; the value is the language's own algorithm result (Python C3, Scala trait linearization, Ruby ancestry, C++ with virtual-base de-duplication). A derived edge kind `resolves-to` (call-site → method body) records, for a `calls:virtual` or `calls:super` site, the body the MRO selects per candidate concrete type.

**Per-language scope:** Python (C3), Scala (trait linearization), Ruby (module ancestry), C++ (virtual inheritance), Groovy/Kotlin mixins. Single-inheritance languages (Java classes, C#) populate it trivially from `inherits`.

---

## GM-23 — Default-Method Body Provenance

**Status:** schema-room — `implements`/`overrides` give the relation but not the *body-provider* resolution for the not-overridden case; this is a new derived fact requiring default-method detection per language.

A `provides-body` derived edge from a (concrete-type, abstract-or-interface-method) pair to the symbol that actually supplies the executed body when the concrete type does **not** override it — i.e. an interface/trait default method or an inherited base implementation. Makes the body provider a first-class candidate-set member for `calls:virtual` over default-method calls.

**Per-language scope:** Java `default` methods; Scala/Rust trait default methods; C# default interface methods; Kotlin interface method bodies; Ruby module-included methods.

---

## GM-24 — Accessor / Property Override

**Status:** schema-room — today `overrides` is method-to-method and `reads-field`/`writes-field` are method-to-field; representing an accessor that shadows a field is a new entity relationship the model lacks.

Extends `overrides` to range over **accessor** symbols (getter/setter/property), and adds a `shadows-field` derived edge from a subclass accessor to the parent field or accessor it shadows. Lets field-read/field-write reasoning (GM `reads-field`/`writes-field`) account for an accessor interposed by a subclass.

**Per-language scope:** C# `override` properties; Kotlin `val`/`var` with custom `get()`/`set()` override; Python `@property` override; Swift computed-property override; Java getter/setter override (by convention).

---

## GM-25 — Abstract-Method Fulfillment Map

**Status:** core-extension — `is_abstract` and `overrides` exist; `fulfills` is the `overrides` relation restricted to an abstract target, and `unfulfilled_abstract` is a derived set over existing edges. No new stored entity required (uses GM-22's MRO when present, falls back to `inherits` chain).

A `fulfills` derived edge from a concrete method to the abstract method (`is_abstract = true`) it satisfies, plus a derived per-(concrete-type) predicate `unfulfilled_abstract` listing abstract members with no fulfilling override reachable through the type's MRO. Exposes the mapping CHA/RTA already computes internally.

**Per-language scope:** All languages with abstract methods/interfaces/traits: Java/C#/Kotlin/Scala abstract + interface; Rust trait required methods; Python `abc.abstractmethod`; C++ pure-virtual.

---

## GM-26 — Transitive `may-panic` Effect

**Status:** `schema-room` — `panic` already exists as an edge condition label (GM-3); this feature reserves one new effect-lattice value (`may-panic`) on the GM-12 effect system and a corresponding derived node attribute. Full transitive computation is deferred to the same phase as the GM-12 transitive-closure upgrade (Phase 2/3).

### Motivation

The `panic` edge condition (GM-3) labels individual call edges taken only on an unwinding or aborting path. It answers "does this specific call site execute on a panic path?" — a per-edge property. It does not answer "can this function, transitively, cause a panic?" — a per-function property. For Rust code (the primary launch language), the latter is the operationally important question: library authors annotate functions as `#[must_not_panic]`; callers want to know whether a transitive dependency exposes a panic path.

The `may-panic` effect fills this gap: it is one effect-lattice value away from the existing `panic` edge condition, propagated transitively over `calls` edges rather than stored on individual edges.

### Schema representation

The GM-12 effect lattice (GM-12.1) is extended with one value:

| Effect | Meaning |
|---|---|
| `may-panic` | The function, or a function it reaches transitively, may execute a `panic!`, `unwrap`, `expect`, `index out of bounds`, or equivalent abort path |

`may-panic` follows the same transitive computation rule as all other effect values (GM-12.2): a function's `may-panic` effect is set if any reachable function via `calls` or `calls:*` edges carries a `panic` edge condition on at least one outgoing edge.

`may-panic` is **not** set for functions reachable only via `spawns` edges (consistent with GM-12.2's spawns-attribution rule: spawned work's effects are attributed separately, not unioned into the spawner).

### Attributes

- `own_effects`: extended to include `may-panic` when the function's own body contains at least one `panic`-conditioned call site.
- `transitive_effects`: extended to include `may-panic` when any reachable callee carries `may-panic` in its own or transitive effects.

Both attributes are schema-reserved in Phase 1 (null/absent); populated in Phase 2 once edge confidence improves and transitive effect computation runs.

### Illustrative query

```cypher
-- illustrative: requires GM-26 (schema-room)
-- Which public API functions transitively may panic?
MATCH (f {visibility: "public"})
WHERE "may-panic" IN f.transitive_effects
RETURN f.name, f.file, f.line
ORDER BY f.file, f.line
```

See also: Q-33 (docs/05), which exposes the recursion/SCC and architecture-cycle query class; `may-panic` on recursive functions is a natural companion query.

---

*Next:* docs/04-dataflow-and-provenance.md — value pedigree, scope entry/exit
inventory, taint analysis, and instance-level dead-member analysis.
