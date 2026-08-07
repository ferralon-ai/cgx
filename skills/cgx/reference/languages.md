---
title: Language Support
audience: agents and engineers using cgx
Last Updated: 2026-08-06
---

# Language Support

**Step 0:** run `cgx --version` and confirm `MINOR` before relying on any capability here.
Features tagged `Since: v0.N` are available iff `MINOR ≥ N`. See `reference/versions.md` for the full
version ladder.

Edge-condition labels (`always`, `conditional`, `exception`, `loop`, `panic`) and the confidence ladder
(`certain`, `probable`, `possible`) are defined in `reference/mental-model.md`. The sections below explain
how each language's semantics map to those labels.

**⚠ `panic` is schema-only: no shipped frontend produces an observable `panic` edge.** Every
frontend populates `always`, `conditional`, `loop`, and `exception`. The Rust adapter is the only
one that *labels* anything `panic`, and none of those labels survives into the graph — see
"The `panic` label never reaches the graph" under Rust below. A query filtered on
`r.condition = "panic"` returns **zero rows on every repo in every language**, and does so at exit 0
with `direction: "exact"`. Read that as "not modeled", never as "no abort paths exist".

**One command is language-independent.** `cgx coupling` answers from committed git history and
never parses source, so it returns a full answer on a repository written entirely in languages
cgx has no adapter for — and on one that has never been indexed. Everything else in this file
gates on the adapter; `coupling` does not.

---

## Tiered language strategy

cgx assigns every edge a confidence label. That label's ceiling is determined by the language's support
tier, which controls how deeply cgx resolves call targets.

| Tier | Description | Confidence ceiling (with SCIP) | Confidence ceiling (no SCIP) |
|------|-------------|-------------------------------|------------------------------|
| **1 — Deep semantics** | Full parse + language-specific scope graph; closure and higher-order tracking; error-path annotation | `certain` (intra-file + SCIP direct), `probable` (virtual/interface), `possible` (unresolvable dynamic) | `certain` (intra-file scope-ref), `probable` (**singleton** import binding or dispatch candidate set), `possible` (multi-candidate set, or the bare-name fallback) |
| **2 — Graph + heuristic** | tree-sitter parse + heuristic cross-file name matching; no type-based dispatch narrowing | `possible` (all cross-file edges — see below) | `possible` (all cross-file edges — see below) |
| **3 — Syntactic only** | tree-sitter parse; intra-file symbol inventory; no cross-file edges | `possible` (all edges) | `possible` (all edges) |

**Without SCIP, a Tier-1 edge is not automatically `possible` — the candidate-set size decides.**
The resolver bands a candidate set by how many definitions survive, and only the *bare-name*
fallback is clamped regardless of size. Read off the rule, not off the tier:

| Resolution rule | Tier | Confidence, no SCIP |
|---|---|---|
| `scope-ref` — same-file lexical resolution of a direct call | scope-graph | `certain` |
| `import-ref` — import binding resolving to **one** exported target | scope-graph | **`probable`** |
| `import-ref` — import binding, several candidates | scope-graph | `possible` |
| `name-method` — virtual/duck-typed receiver, **one** same-name method | scope-graph | **`probable`** |
| `name-method` — virtual/duck-typed receiver, several same-name methods | scope-graph | `possible` |
| `name-arity` — global short-name(+arity) fallback, **any** number of hits | name-syntactic | `possible` |
| no candidate | — | edge dropped; `unresolved` cut marker recorded |

The singleton rows are the common case on a real tree, so **`probable` cross-file edges are
ordinary without SCIP**, not a sign SCIP ran. What SCIP adds is promotion of the *multi-candidate*
rows and of cross-file direct calls to `certain`.

The `name-arity` clamp is the one place size does not matter: a lone surviving bare-name hit is a
name collision with one survivor, not a resolution, so it stays `possible`. That asymmetry is also
the reason for the caveat in the Python section below — a singleton `name-method` keeps `probable`
with no in-repo receiver type.

**Tier 2's ceiling is `possible`, unique name or not.** A "heuristic cross-file name match" with no
scope, import, or type corroboration is exactly the resolver's Tier-0 name(+arity) fallback, and
that band is **clamped to `possible` even when a single definition survives** — a lone hit there is
a name collision with one survivor, not a resolution. A Tier-2 language therefore has no route to
`probable`: `probable` is reserved for a uniquely-resolved *import binding* or a singleton dispatch
candidate set, both of which need the scope graph a Tier-2 adapter does not build. No Tier-2
adapter ships today, so this describes the band a Tier-2 language would land in, not observed
output.

---

## Languages runnable today (v0.3)

### Rust — Tier 1 (primary, fully exercised)

The language frontend is `cgx-lang-rust`. Rust is the primary language for all tested examples and real
output shapes in this skill.

**Error model — `Result` and `panic`**

Rust has no exceptions. Failure takes two forms:

- **`Result<T,E>` / `?` operator / `Err(e) =>` match arm** — ordinary data-conditioned branches.
  cgx labels these `exception` on the call edge. This is intentional: it makes cross-language queries for
  "failure-path-only edges" work uniformly (see `reference/mental-model.md` for the `exception` label
  definition).
- **`panic!` and friends** — non-recoverable abort. The extractor labels a *reference* `panic` here,
  but no such edge reaches the graph. See below.

Reading the result: an edge labeled `exception` in a Rust graph means "this call only occurs when a
`?`-propagated error or an explicit `Err` arm is taken." It does **not** mean stack-unwinding or a
thrown object.

**The `panic` label never reaches the graph — two independent reasons**

Do not query for it. `panic` is a real value in the edge-condition schema and in the precedence
rule, and the Rust extractor is the only place that ever assigns it, but nothing survives to be
queried, for two reasons that each suffice on their own:

1. **The label is macro-only.** `is_panic_macro` matches exactly ten macro names — `panic`,
   `unreachable`, `todo`, `unimplemented`, `assert`, `assert_eq`, `assert_ne`, `debug_assert`,
   `debug_assert_eq`, `debug_assert_ne`. **`unwrap` and `expect` are not in it**, and appear nowhere
   in the Rust frontend: they are ordinary method calls and are never labelled `panic` at all. The
   most common Rust abort idioms are therefore invisible to this label even in principle.
2. **The labelled reference points out of the tree.** For the ten macros it does match, the
   extractor emits a reference to `core::panicking::panic_fmt` — an **external** symbol. Resolution
   finds no in-tree definition, so the reference is recorded as unresolved and the edge is dropped
   before the graph is written.

Verified by running, on a fresh six-function fixture using `panic!`, `assert!`, `todo!`, `.unwrap()`
and `.expect()`: the index reports `12 nodes, 5 edges, 5 unresolved` — the five unresolved refs are
the macro references to `core::panicking::panic_fmt`, and **all five surviving edges are
`always`**. `MATCH (a)-[r:CALLS]->(b) WHERE r.condition = "panic" RETURN a.name, b.name` returns
**0 rows at exit 0**.

This is why a recipe that filters on the `panic` condition returns a clean empty result: the empty
answer is a modelling gap wearing the appearance of a safety finding, not evidence that no abort
paths exist. To reason about Rust abort paths today, query the *callee* (`unwrap`, `expect`, a
panicking helper) by name rather than the edge condition.

Path-relative transience applies: a call that is only reachable via an upstream `exception` or `panic`
edge is exception-transient relative to that path, even if its own edge condition is `always`. This is
computed at query time, not stored per edge — see `reference/mental-model.md`.

**Dynamic dispatch — `dyn Trait`**

Rust direct intra-module calls resolve at `certain` (scope-ref rule). Cross-module calls resolve by
candidate-set size, not by being cross-module: an `import-ref` binding with a single exported target
and a `name-method` set with a single same-name method both band **`probable`** without any SCIP
enrichment; either with several surviving candidates bands `possible`. A call that reaches the
global bare-name (`name-arity`) fallback bands `possible` however many hits it has. `dyn Trait`
object calls produce a candidate set of known implementations — `possible` in the Phase-1 syntactic
graph whenever more than one implementation is in scope. A closure stored in an unknown variable or
passed as an untyped callback also resolves at `possible`.

Live on the self-index corpus (18,431 nodes, `reference/mental-model.md` §2.2) with **no SCIP**, `cgx explain --format json` returns
edges banded `('import-ref', 'probable', 'scope_graph')`, `('name-method', 'probable',
'scope_graph')`, `('name-method', 'possible', 'scope_graph')`, `('name-arity', 'possible',
'name_syntactic')` and `('scope-ref', 'certain', 'scope_graph')` — five distinct bands, three of
them above `possible`.

Since: v0.2 — SCIP enrichment promotes `dyn Trait` resolution from `possible` (heuristic scope-graph
rule) to `probable` (CHA/RTA-backed candidate set). Cross-module direct calls are promoted to `certain`
where SCIP provides a single precise target.

**Closures and captures**

cgx emits a call edge from the call site to the closure body. Captures are tracked for data-flow pedigree
(Since: v0.3). The call edge is present in all versions; confidence follows the resolution path above.

---

### TypeScript — Tier 1 (present, same crate family as JavaScript)

The language frontend is `cgx-lang-ts`. TypeScript and JavaScript share one adapter. TypeScript is
exercised alongside Rust; the primary validated examples are Rust.

**Error model — `throw` / `catch` and `Promise.reject`**

TypeScript uses exceptions for synchronous errors and rejected promises for async errors. cgx labels:

- Edges inside `catch` blocks: `exception`
- Edges inside `finally` blocks: unlabeled (always — `finally` runs on every path)
- `.catch()` callback edges: `exception`

**Dynamic dispatch — structural typing and interface dispatch**

Direct intra-file calls (scope-ref rule): `certain`. Import-resolved references band by candidate-set
size like every other language — **`probable`** when the binding resolves to one exported target,
`possible` when several survive — and a cross-file call that falls through to the bare-name
(`name-arity`) fallback is `possible` regardless. Interface implementations resolved via structural
name match follow the same singleton/multi split. Generic type parameter calls and Proxy traps:
`possible`.

Since: v0.2 — SCIP enrichment via `scip-typescript` promotes structural-match edges from `possible`
(heuristic) to `probable` (type-resolved), and cross-file direct calls to `certain` where SCIP gives a
single precise target.

**Own-effects and dataflow**

The TS/JS adapter emits GM-12 own-effects and intraprocedural SSA dataflow, at Go-column parity. Effects
are a name-based syntactic heuristic (`possible`-grade, no import/type resolution): `fetch`/`axios`/`http`
→ `io.net`; `fs.*` → `io.file`; `child_process.*` → `io.proc`; `Math.random`/`Date.now`/`crypto.random*`
→ `nondeterministic`; `eval`/`new Function` → `dynamic-code`; `Atomics.wait` → `blocking`. Concurrency
launchers (`setTimeout`/`setInterval`/`queueMicrotask` and `new Worker`) emit a `spawns` effect at the
launch site (mirroring Go's `go` statement).

Intraprocedural dataflow lowers each production site (declaration, assignment, `+=`, projection, call,
return) into a `DerivesFrom` fact (`derived → source`), so `flows-to` / `flows-from` resolve end-to-end on
TS/JS. TS type-coercions (`x as T`, `x!`, `x satisfies T`, `await x`) are transparent (copy). A call result
is opaque (an `opaque-call` cut plus callee + per-arg access-paths for the interprocedural pass); a
depth-2+ member access (`a.b.c`) truncates to the base local `a` with a `truncated-access-path` cut.

**Under-approximations (honest scope):** dataflow is only extracted for block-bodied functions, methods,
and named block-body arrows (`const f = () => { … }`). Expression-body arrows (`a => a + 1`), anonymous
inline callbacks, and cross-closure captures are NOT tracked — closure-capture dataflow is deferred, the
same posture Go (func-literal) and Python (lambda) chose. Effects are attributed to the syntactically
enclosing named definition only; effects performed inside an inline callback attribute to that callback's
owner, not the callback.

**Closures and captures**

Closure call edges are present; without SCIP enrichment they resolve at `possible` (sig-compat or cha_rta
rule). Unhandled async IIFEs and detached promises are not modeled as `spawns` (only the explicit
schedulers/`Worker` above are).

---

### Go — Tier 1 (shipped, PR #24)

The language frontend is `cgx-lang-go`. Go reached Go-column parity as the first of the three
post-launch Tier-1 adapters.

**Error model — `error` return and `panic`**

Go has no exceptions. Failure takes two forms:

- **`if err != nil` branch** — ordinary data-conditioned failure branch. cgx labels these `exception` on
  the call edge, for the same cross-language-consistency reason as Rust's `?` operator (see
  `reference/mental-model.md` for the `exception` label definition).
- **`panic()`** — non-recoverable abort unless an enclosing `defer` calls `recover()`. **cgx does not
  model this.** The Go adapter emits only `always`, `conditional`, `loop`, and `exception`; it recognises
  neither `panic()` nor `recover()`, so a call on a panic path carries whatever condition its enclosing
  syntax gives it (usually `always` or `conditional`). A `r.condition = "panic"` filter on a Go repo
  returns empty because the label is never produced, not because no abort paths exist — the same
  end state as Rust, reached without the Rust adapter's two-step loss.

**Dynamic dispatch — interface satisfaction**

Direct calls resolve at `certain`. Interface-satisfied calls produce a candidate set of all types
satisfying the interface: `probable` when the set resolves to a single candidate, `possible` when
multiple candidates remain (same single/multi banding as Java and Python). SCIP enrichment for Go is
**not yet available** (`planned`, not shipped — see `docs/14-implementation-status-matrix.md`).

**Closures and goroutines**

`go func() { … }` produces a `spawns` edge to the goroutine body. Channel send/receive (`ch <-` / `<- ch`)
are tracked as synchronization points. Concurrency/async hints are partial (`~`) — full lock-set modeling
is not yet complete.

---

### Java — Tier 1 (shipped, PR #25)

The language frontend is `cgx-lang-java`.

**Error model — checked and unchecked exceptions**

`throw` / `catch`; checked-exception declarations (`throws`) are indexed as metadata on callee edges.
cgx labels edges reachable only via throw/catch `exception`; `always` otherwise.

**Dynamic dispatch — virtual and interface dispatch**

Static calls resolve at `certain`. Virtual dispatch produces a class-hierarchy-derived (CHA-style)
candidate set labeled `probable` when the override set is a single implementation; reflection
(`Class.forName`, `Method.invoke`) resolves at `possible`. SCIP enrichment for Java is **not yet
available** (`planned`).

**Closures and concurrency**

Lambda and method-reference call edges are tracked as ordinary call edges. `Thread.start`,
`ExecutorService.submit`/`execute`, and `CompletableFuture.supplyAsync`/`runAsync` record a `spawns`
**effect** on the enclosing symbol — an `own_effects` node attribute, not a `spawns` edge. (Only the
Rust, Go, and Python frontends emit `spawns` *edges*; TypeScript, like Java, records the effect.)
Concurrency/async hints are partial (`~`).

---

### Python — Tier 1 (shipped, PR #26)

The language frontend is `cgx-lang-python`. Python is the newest adapter and carries the highest test
count of the three post-launch languages.

**Error model — `raise` / `except`**

cgx labels edges inside `except` blocks (or reachable only via a `raise`) `exception`. Duck typing means
the callee may not be statically resolvable — see dispatch below.

**Dynamic dispatch — duck typing and attribute lookup**

A plain call whose short name matches exactly one definition anywhere in the index resolves at
**`possible`**, not `probable`: with no scope, import, or receiver corroboration it reaches the
resolver's Tier-0 name(+arity) fallback, whose band is clamped to `possible` even for a lone
surviving hit. Attribute calls on a type-annotated variable resolve at `probable`; untyped
attribute calls, `__call__`, and metaclass-mediated dispatch resolve at `possible` when several
candidates survive. SCIP enrichment for Python is **not yet available** (`planned`).

**Closures and concurrency**

Decorator chains are tracked as call sequences. `asyncio.create_task`, `threading.Thread.start`, and
`concurrent.futures.submit` produce `spawns` edges. Concurrency/async hints are partial (`~`).

**Known cross-language caveat — read this before trusting a `probable` on a method call.** The
resolver demotes a same-name candidate set to `possible` only when it has **more than one**
member. A method call on a receiver whose type was never resolved in-repo, where exactly one
same-named method exists anywhere in the index, therefore keeps `probable` — and `probable` at
the scope-graph tier is precisely the value that lets the approximation contract report
`direction: "exact"`. A single coincidental name match elsewhere in the tree can produce an
answer that presents as exact. Duck typing makes Python meet this most often, but it affects all
five shipped languages and is a tracked resolver backlog item, not a Python-specific defect.
Check `rule` and `tier` in `cgx explain --format json` before relying on such an edge.

The *other* half of this — the bare global name(+arity) fallback over-reporting `probable` for a
singleton — was fixed and is no longer live: that band is clamped to `possible`.

---

## Roadmap languages (not runnable today)

The sections below describe the design, not the current implementation. cgx docs/08 defines these tiers
and semantics ahead of implementation. Tag: not available until the language's Tier-1 adapter ships.

### Tier 1 planned: C#

C# has full LS-2/LS-3/LS-4/LS-7/LS-8 specifications in docs/08-language-support.md but no
language-specific crate yet. It is the last Tier-1 language on the roadmap — Rust, TypeScript, Go, Java,
and Python have all shipped.

Key reading caveats for when it ships:

| Language | Error model → cgx label | Dynamic dispatch caveat |
|----------|------------------------|------------------------|
| **C#** | `throw`/`catch` → `exception`; `Task` faulted state → `exception` on fault continuations | Properties are implicit method calls; GC-scheduled finalizers → `possible` |

### Tier 2: C, C++, C#, Kotlin, Swift, Ruby, PHP

Heuristic resolution; no exception-path annotation. Calls labeled `always` or left unlabeled. No
type-based dispatch narrowing.

### Tier 3: Bash, HTML, CSS, SQL, YAML, Dockerfile, and others with a tree-sitter grammar

Intra-file symbol inventory only. All edges `possible`. No cross-file edges.

---

## Confidence degradation table

The table below shows how the confidence ceiling decreases with tier and dispatch complexity.
`--confidence` filtering (Since: v0.1) applies this floor at query time.

**Without SCIP enrichment** (the default for a plain `cgx index`), the Phase-1 graph applies:
- Intra-file scope-ref calls (same file, unambiguous target): `certain`
- Cross-file `import-ref` and dispatch `name-method` sets: `probable` when **one** candidate
  survives, `possible` when several do
- The global bare-name (`name-arity`) fallback: `possible`, however many hits

**With SCIP enrichment** (`cgx index --scip <file>`, Since: v0.2):
- `dyn Trait` / interface dispatch candidate sets: promoted to `probable`
- Cross-file direct calls with a single precise SCIP target: promoted to `certain`

| Call pattern | Tier 1 (with SCIP) | Tier 1 (no SCIP) | Tier 2 | Tier 3 |
|---|---|---|---|---|
| Direct intra-file call (scope-ref) | `certain` | `certain` | `possible` | `possible` |
| Direct cross-file call (import-resolved), **single** target | `certain` | **`probable`** | `possible` | — |
| Direct cross-file call (import-resolved), several candidates | `certain` | `possible` | `possible` | — |
| Virtual / interface dispatch, **single** surviving candidate | `probable` | **`probable`** | `possible` | — |
| Virtual / interface dispatch, several candidates | `probable` | `possible` | `possible` | — |
| Global bare-name (`name-arity`) fallback, any number of hits | see note | `possible` | `possible` | `possible` |
| Dynamic dispatch / reflection / `eval` | `possible` | `possible` | `possible` | — |
| Async callback / closure via unknown capture | `probable` | `possible` | `possible` | — |

**Note on the SCIP column.** SCIP relabeling is **upgrade-only and per-occurrence**: it keys on the
SCIP symbol class and definition count at the call site, not on which rule produced the edge, and it
rewrites `confidence`/`tier`/`rule` only when the SCIP-derived band is *higher* than the edge's
current one. A `name-arity` edge therefore reaches `certain` where SCIP maps its site to a single
definition and stays `possible` where SCIP has nothing to say. Read `rule` in `cgx explain --format
json`: `scip-occurrence` means SCIP set the band; anything else means it did not.

Every edge carries an explicit confidence label — cgx never silently drops uncertain edges. It labels
uncertain ones `possible` or `probable` and lets the caller decide whether to include them
(`--confidence possible|probable|certain`).

---

## SCIP enrichment (Since: v0.2)

SCIP (Stack-based Index of Positions and Calls) is optional pre-computed resolution that upgrades
heuristic (`possible`) edges to type-resolved (`probable` for dispatch candidate sets, `certain` for
single-target direct calls). Supported generators: `scip-typescript` and `rust-analyzer --emit-scip`
(`scip-rust`). **`scip-python` is not supported** — Python SCIP enrichment is planned, matching the
Python section above.

The `--scip` flag itself is language-agnostic: it accepts any `.scip` path and applies no
language-based gating. What is *validated* is narrower — the symbol-mapping grammar is written and
tested against `rust-analyzer`'s scheme and TypeScript's npm scheme only. Whether a `scip-go`,
`scip-java`, or `scip-python` index would in fact upgrade any edge has not been established either
way; "not yet available" in the per-language sections above means "unshipped and unvalidated", not
"the flag rejects it".

SCIP ingestion is not required; cgx falls back to heuristic resolution without it. The `--scip` flag is
available on `cgx index` since v0.2.

LSP-based extraction (shelling out to a running language server) is out of scope for v1 — incompatible
with the <100ms warm startup target.

---

## Summary: what to check when reading results

1. **Which tier is this language?** — sets the confidence ceiling.
2. **Is the edge condition `exception` in a Rust or Go codebase?** — means `Result`-branch or
   `if err != nil`, not an exception object. See `reference/mental-model.md` for the `exception` label.
3. **Is the edge `possible`?** — the callee was not narrowed to one candidate. In Tier 1 this is a
   multi-candidate `dyn Trait` set, closure-via-capture, an untyped callback, or the bare-name
   fallback. Treat as a candidate, not a confirmed call.
4. **Is the edge `probable`, and which rule produced it?** — `probable` without SCIP means a
   *singleton* candidate set (`import-ref` or `name-method`), which is the ordinary cross-file case
   and not a sign SCIP ran. A singleton `name-method` carries no in-repo receiver type, so read
   `rule` and `tier` in `cgx explain --format json` before trusting it (see the Python caveat above).
5. **Was the index built with `--scip`?** — SCIP is upgrade-only: it promotes multi-candidate
   dispatch sets to `probable` and single-target direct calls to `certain`, and leaves everything it
   cannot map alone. `rule: "scip-occurrence"` is the tell. The `--scip` flag has been available
   since v0.2.
6. **Is the call exception-transient on this path?** — computed at query time. Use edge-condition filters
   or the `ANY`/`NONE` CQL quantifiers. See `reference/mental-model.md`.
