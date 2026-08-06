---
title: Language Support
audience: agents and engineers using cgx
Last Updated: 2026-07-06
---

# Language Support

**Step 0:** run `cgx --version` and confirm `MINOR` before relying on any capability here.
Features tagged `Since: v0.N` are available iff `MINOR ≥ N`. See `reference/versions.md` for the full
version ladder.

Edge-condition labels (`always`, `conditional`, `exception`, `loop`, `panic`) and the confidence ladder
(`certain`, `probable`, `possible`) are defined in `reference/mental-model.md`. The sections below explain
how each language's semantics map to those labels.

**`panic` is Rust-only.** Every shipped frontend populates `always`, `conditional`, `loop`, and
`exception`; only the Rust adapter emits `panic`. On a Go, Java, Python, or TypeScript repo,
`--edge-condition panic` returns empty because the label is never produced — read that as "not
modeled here", never as "no abort paths exist".

---

## Tiered language strategy

cgx assigns every edge a confidence label. That label's ceiling is determined by the language's support
tier, which controls how deeply cgx resolves call targets.

| Tier | Description | Confidence ceiling (with SCIP) | Confidence ceiling (no SCIP) |
|------|-------------|-------------------------------|------------------------------|
| **1 — Deep semantics** | Full parse + language-specific scope graph; closure and higher-order tracking; error-path annotation | `certain` (intra-file + SCIP direct), `probable` (virtual/interface), `possible` (unresolvable dynamic) | `certain` (intra-file scope-ref only), `possible` (all other calls) |
| **2 — Graph + heuristic** | tree-sitter parse + heuristic cross-file name matching; no type-based dispatch narrowing | `probable` (unique name), `possible` (overloaded or ambiguous) | `probable` (unique name), `possible` (overloaded or ambiguous) |
| **3 — Syntactic only** | tree-sitter parse; intra-file symbol inventory; no cross-file edges | `possible` (all edges) | `possible` (all edges) |

Confidence above `possible` for cross-file or dispatch edges requires SCIP enrichment (Since: v0.2).
Without SCIP, Tier 1 produces `certain` only for intra-file scope-ref calls; all other Tier 1 edges are
`possible`.

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
- **`panic!` / `unwrap` / `expect`** — non-recoverable abort. cgx labels these `panic` on the call edge,
  keeping them distinct from recoverable `Result` failure.

Reading the result: an edge labeled `exception` in a Rust graph means "this call only occurs when a
`?`-propagated error or an explicit `Err` arm is taken." It does **not** mean stack-unwinding or a
thrown object. An edge labeled `panic` means the code aborts if that path is taken.

Path-relative transience applies: a call that is only reachable via an upstream `exception` or `panic`
edge is exception-transient relative to that path, even if its own edge condition is `always`. This is
computed at query time, not stored per edge — see `reference/mental-model.md`.

**Dynamic dispatch — `dyn Trait`**

Rust direct intra-module calls resolve at `certain` (scope-ref rule). Cross-module calls (import-ref or
name-method rule) resolve at `possible` without SCIP enrichment. `dyn Trait` object calls produce a
candidate set of known implementations, labeled `possible` in the Phase-1 syntactic graph (no SCIP). A
closure stored in an unknown variable or passed as an untyped callback also resolves at `possible`.

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

Direct intra-file calls (scope-ref rule): `certain`. Cross-file calls and import-resolved references:
`possible` without SCIP enrichment. Interface implementations resolved via structural name match:
`possible` in the Phase-1 syntactic graph. Generic type parameter calls and Proxy traps: `possible`.

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
  syntax gives it (usually `always` or `conditional`). Filtering `--edge-condition panic` on a Go repo
  returns empty because the label is never produced, not because no abort paths exist.

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

Unique-name-match calls resolve at `probable`. Attribute calls on a type-annotated variable resolve at
`probable`; untyped attribute calls, `__call__`, and metaclass-mediated dispatch resolve at `possible`.
SCIP enrichment for Python is **not yet available** (`planned`).

**Closures and concurrency**

Decorator chains are tracked as call sequences. `asyncio.create_task`, `threading.Thread.start`, and
`concurrent.futures.submit` produce `spawns` edges. Concurrency/async hints are partial (`~`).

**Known cross-language caveat:** the resolver's tiered name/arity fallback (`cgx-resolve/src/link.rs`)
can over-report `probable` where `possible` is more honest for ambiguous global name collisions. This
affects all five shipped languages, not just Python, and is a tracked backlog item, not a Python-specific
defect.

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
- All other calls (cross-file import-ref, name-method, dispatch): `possible`

**With SCIP enrichment** (`cgx index --scip <file>`, Since: v0.2):
- `dyn Trait` / interface dispatch candidate sets: promoted to `probable`
- Cross-file direct calls with a single precise SCIP target: promoted to `certain`

| Call pattern | Tier 1 (with SCIP) | Tier 1 (no SCIP) | Tier 2 | Tier 3 |
|---|---|---|---|---|
| Direct intra-file call (scope-ref) | `certain` | `certain` | `probable` | `possible` |
| Direct cross-file call (import-resolved) | `certain` | `possible` | `probable` | — |
| Virtual / interface dispatch (resolved candidate set) | `probable` | `possible` | `possible` | — |
| Dynamic dispatch / reflection / `eval` | `possible` | `possible` | `possible` | — |
| Async callback / closure via unknown capture | `probable` | `possible` | `possible` | — |

Every edge carries an explicit confidence label — cgx never silently drops uncertain edges. It labels them
`possible` and lets the caller decide whether to include them (`--confidence possible|probable|certain`).

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
3. **Is the edge `possible`?** — the callee was not statically resolved. In Tier 1 this is `dyn Trait`,
   closure-via-capture, or an untyped callback. Treat as a candidate, not a confirmed call.
4. **Was the index built with `--scip`?** — without SCIP enrichment, cross-file calls and dispatch
   candidate sets are `possible` regardless of call type. With SCIP, dispatch candidate sets are
   promoted to `probable` and single-target direct calls to `certain`. The `--scip` flag has been
   available since v0.2.
5. **Is the call exception-transient on this path?** — computed at query time. Use edge-condition filters
   or the `ANY`/`NONE` CQL quantifiers. See `reference/mental-model.md`.
