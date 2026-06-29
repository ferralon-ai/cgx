---
title: Language Support
audience: agents and engineers using cgx
Last Updated: 2026-06-25
---

# Language Support

**Step 0:** run `cgx --version` and confirm `MINOR` before relying on any capability here.
Features tagged `Since: v0.N` are available iff `MINOR ≥ N`. See `reference/versions.md` for the full
version ladder.

Edge-condition labels (`always`, `conditional`, `exception`, `loop`, `panic`) and the confidence ladder
(`certain`, `probable`, `possible`) are defined in `reference/mental-model.md`. The sections below explain
how each language's semantics map to those labels.

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

**Closures and captures**

Tagged template literal calls and unhandled async IIFEs produce `spawns`-attribute edges (schema reserved,
Since: v0.3 for full effect propagation). Closure call edges are present; without SCIP enrichment they
resolve at `possible` (sig-compat or cha_rta rule).

---

## Roadmap languages (not runnable today)

The sections below describe the design, not the current implementation. cgx docs/08 defines these tiers
and semantics ahead of implementation. Tag: not available until the language's Tier-1 adapter ships.

### Tier 1 planned (in order): Python, Go, Java, C#

These have full LS-2/LS-3/LS-4/LS-7/LS-8 specifications in docs/08-language-support.md but no
language-specific crate as of v0.3.

Key reading caveats for when these ship:

| Language | Error model → cgx label | Dynamic dispatch caveat |
|----------|------------------------|------------------------|
| **Python** | `raise`/`except` → `exception`; duck typing means callee may not resolve statically | `probable` with name/type-annotation match; `possible` for untyped attribute calls |
| **Go** | `if err != nil` → `exception` (data-conditioned; no unwinding); `panic()` → `panic` | Interface satisfaction candidate set → `probable`; direct calls → `certain` |
| **Java** | `throw`/`catch` → `exception`; checked exception declarations indexed on callee edges | Virtual dispatch (CHA-style) → `probable`; static → `certain`; reflection → `possible` |
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
single-target direct calls). Supported generators: `scip-typescript`, `rust-analyzer --emit-scip`
(`scip-rust`), `scip-python`.

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
