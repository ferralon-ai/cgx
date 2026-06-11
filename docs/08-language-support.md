# 08 — Language Support

**Status:** Feature specification (pre-implementation)
**Audience:** Engineers extending `cgx` with new language support; contributors; advanced users evaluating language coverage
**Working name:** `cgx` (placeholder — see docs/README.md)
**Cross-references:** docs/03-code-graph-model.md (GM-) · docs/04-dataflow-and-provenance.md (DF-) · docs/09-architecture.md (AR-)

---

## Overview

`cgx` supports multiple programming languages through a tiered strategy: all
languages receive a syntactic baseline; selected languages receive deeper
semantic analysis. The tier determines the confidence labels (see GM-5) that
`cgx` attaches to edges and the accuracy of dispatch resolution.

This document specifies:

- LS-1: Tiered language strategy
- LS-2: Error and exception model per language
- LS-3: Dispatch model per language
- LS-4: Async model per language
- LS-5: Optional enrichment via SCIP ingestion
- LS-6: Confidence degradation across tiers

---

## LS-1: Tiered Language Strategy

### Tier 1 — Deep semantics

Full parse plus language-specific semantic enrichment. Scope graph construction
via `tree-sitter-graph` with language-specific rules. Where available, an
optional language-specific deep parser augments the tree-sitter AST.

Languages in Tier 1 at launch: **Rust, JavaScript, TypeScript, Python, Java, Go**

Characteristic capabilities at Tier 1:
- Callee resolution at `certain` or `probable` confidence for most call sites.
- Exception/error path annotation (see LS-2).
- Dynamic dispatch candidate sets with bounded size.
- Closure and higher-order call tracking.
- Async call edge annotation (see LS-4).

### Tier 2 — Graph and heuristic resolution

Parse via tree-sitter grammar; cross-file name resolution via heuristic matching
(unique name match, import-path resolution). No language-specific scope graph.
Dispatch is approximated.

Languages in Tier 2: **C, C++, C#, Kotlin, Swift, Ruby, PHP**

Characteristic capabilities at Tier 2:
- Callee resolution at `probable` confidence when the callee name is unique in
  the indexed codebase; `possible` confidence for overloaded or ambiguous names.
- No type-based dispatch narrowing.
- No exception-path annotation (calls are labeled `always` or left unlabeled).

### Tier 3 — Syntactic only

tree-sitter grammar exists; `cgx` can parse the file, extract function-like
constructs, and identify textual call patterns, but performs no name resolution
or cross-file graph linking.

Languages in Tier 3: all languages for which a tree-sitter grammar exists but
no scope graph rules or heuristic resolver has been authored. Examples:
Bash, HTML, CSS, SQL, YAML, Dockerfile.

Characteristic capabilities at Tier 3:
- Symbol inventory (function and class names).
- Intra-file reference extraction.
- All edges labeled `possible`; no cross-file edges.

### Adding a new language

Promoting a language from Tier 3 to Tier 2 requires: a tree-sitter grammar
(usually already exists), a heuristic name resolver, and import-path extraction
rules. Promoting from Tier 2 to Tier 1 additionally requires: a
`tree-sitter-graph` scope graph definition, and (optionally) a deep-parse
enrichment integration.

---

## LS-2: Error and Exception Model per Language

Edge condition labels (see docs/03-code-graph-model.md GM-3) are annotated on
call edges to distinguish control-flow paths. The following table maps each
supported language's error/exception surface to `cgx` edge conditions.

| Language | Error / exceptional control flow mechanism | `cgx` edge annotation | Notes |
|---|---|---|---|
| **Java** | `throw` / `catch`; checked exceptions declared in `throws` | `exception` on edges reachable only via throw/catch; `always` otherwise | Checked exception declarations are indexed as metadata on callee edges |
| **Python** | `raise` / `except`; `BaseException` hierarchy | `exception` on edges in except blocks or following raise | Duck typing means callee may not be statically resolved; see LS-3 |
| **JavaScript / TypeScript** | `throw` / `catch`; `Promise.reject`; unhandled rejections | `exception` on edges in catch/finally; `exception` on `.catch()` callbacks | Async throw/reject annotated via async model (see LS-4) |
| **Rust** | `Result<T, E>` / `?` operator; `panic!` / `unwrap` / `expect` | `exception` on edges gated by `?` or `Err(e) =>` match arm (data-conditioned failure branch); `panic` on edges reached only via `panic!` / `unwrap` / `expect` | Rust `?` and `Err` arms are ordinary data-conditioned branches, not exceptions in the unwinding sense; `cgx` labels them `exception` so cross-language "non-exception path" queries work uniformly. `panic` (non-recoverable) stays distinct. See GM-3 in docs/03-code-graph-model.md. |
| **Go** | `error` return value; `panic` / `recover` | `exception` on edges inside `if err != nil` blocks (data-conditioned failure branch); `panic` on `panic()` paths; `exception` on `recover()` defers | Go error returns are ordinary data-conditioned branches; `cgx` labels them `exception` for cross-language query consistency (same as Rust `?`). `panic` stays distinct. |
| **C** | Sentinel return values (NULL, negative int, errno); `longjmp` | `conditional` on edges gated by sentinel check; `exception` on longjmp paths | `cgx` identifies sentinel checks via pattern matching; confidence is `probable` |
| **C++** | `throw` / `catch`; RAII destructor chains | `exception` on edges in catch blocks; `exception` on destructor calls reached only via stack unwinding | Destructor call edges during unwinding are labeled `exception` |
| **C#** | `throw` / `catch`; `Task` faulted state | `exception` on catch block edges; `exception` on `.ContinueWith` fault continuations | Async model follows LS-4 |
| **Kotlin** | Unchecked exceptions; `Result<T>` / `runCatching` | `exception` on catch edges; `conditional` on `runCatching` success/failure paths | Coroutine cancellation is annotated as `exception` |
| **Swift** | `throw` / `catch`; `Result<T, E>`; `fatalError` | `exception` on try/catch edges; `conditional` on Result branches; `exception` on fatalError | |
| **Ruby** | `raise` / `rescue`; `ensure` | `exception` on rescue block edges | Duck typing applies; see LS-3 |

**Rust `?` / Go `if err != nil` and the `exception` label.** Rust's `?` operator and Go's `if err != nil` pattern are ordinary data-conditioned branches in their respective languages — there is no stack unwinding and no exception object. `cgx` nonetheless assigns them the `exception` label because they represent the semantically equivalent "failure branch" concept. This ensures that cross-language queries for "non-exception paths" (`--exclude-edge-condition exception`) correctly exclude both languages' failure-path call edges. The producing rule is recorded in provenance so users can distinguish Go/Rust error-branch edges from Java/Python catch-block edges. `panic` (Rust `panic!`/`unwrap`, Go `panic()`) remains a separate label because it represents non-recoverable abort-path semantics.

**Path-relative transience:** an edge that is only reachable because an upstream
edge is in the exceptional class (`exception` or `panic`) is exception-transient
relative to that path, even if the edge itself is labeled `always`. This property
is computed at query time, not stored per-edge. See docs/03-code-graph-model.md
(GM-3, GM-4) and docs/04-dataflow-and-provenance.md for full semantics.

---

## LS-3: Dispatch Model per Language

The dispatch model determines how `cgx` resolves a call expression to one or
more callee symbols and what confidence label it assigns.

| Language | Dispatch mechanisms | `cgx` resolution strategy | Confidence |
|---|---|---|---|
| **Java** | Virtual dispatch (vtable); interface dispatch; static calls; reflection | Scope graph resolves static calls at `certain`; virtual dispatch produces candidate set from class hierarchy (CHA-style); reflection calls are `possible` with no callee | `certain` (static), `probable` (virtual, unique override), `possible` (reflection / ambiguous hierarchy) |
| **Python** | Duck typing; attribute lookup; `__call__`; metaclasses; decorators | Unique name match at `probable`; attribute call on typed variable at `probable` (with type annotation); decorator chains tracked as call sequences | `probable` (name match), `possible` (untyped attribute call) |
| **JavaScript** | Prototype chain; duck typing; module imports; dynamic property access | Import-resolved calls at `probable`; prototype calls at `possible`; `require`/`import` tracked for module boundary edges | `probable` (import-resolved), `possible` (dynamic property) |
| **TypeScript** | Same as JS plus structural typing; interface dispatch | Interface implementations resolved via structural match at `probable`; generic type parameter calls at `possible` | `probable` (structural match), `certain` (direct static call with type narrowing) |
| **Rust** | Static calls; trait dispatch (static monomorphization or `dyn Trait`); closure calls | Static and monomorphized calls at `certain`; `dyn Trait` produces trait impl candidate set at `probable`; closure stored in variable tracked via data-flow | `certain` (static/monomorphized), `probable` (dyn Trait impl set), `possible` (closure via unknown capture) |
| **Go** | Direct calls; interface satisfaction; function values; goroutine launch | Interface calls produce candidate set from types satisfying the interface; goroutine launch annotated as async edge | `probable` (interface impl set), `certain` (direct call) |
| **C** | Direct calls; function pointers | Direct calls at `certain`; function pointer calls at `possible` (callee unknown without data-flow) | `certain` (direct), `possible` (function pointer) |
| **C++** | vtable (virtual); templates (monomorphized); std::function; function pointers | Virtual dispatch: CHA-style candidate set; templates: monomorphized call at `certain`; std::function: `possible` | `certain` (direct/template), `probable` (virtual, single override), `possible` (std::function / function pointer) |
| **DI frameworks** | Spring (Java), Angular (TypeScript), Guice | Constructor injection and `@Autowired` / `@Inject` annotations tracked; injected type resolves to registered implementation | `probable` (single registered impl), `possible` (multiple candidates or dynamic registration) |
| **Closures (all languages)** | Closure captures; higher-order functions passed as callbacks | Call edge from call site to closure body; capture tracking feeds DF- pedigree; higher-order call at `possible` if callee is a parameter | `probable` (closure assigned to typed variable), `possible` (callback parameter) |

---

## LS-4: Async Model per Language

Async call sites are annotated with an `async` attribute on the call edge (in
addition to the edge condition label). `cgx` tracks the logical call from caller
to the eventual async body, not just the dispatch mechanism.

| Language | Async mechanism | `cgx` representation |
|---|---|---|
| **JavaScript / TypeScript** | `async`/`await`; `Promise` chain (`.then`/`.catch`/`.finally`) | Edge from await expression to the async callee body; `.then` callback tracked as a separate call edge with `async` attribute; `.catch` callback edge labeled `exception` |
| **Python** | `asyncio` (`async def`, `await`); `concurrent.futures` | Edge from `await` expression to coroutine body; `asyncio.create_task` / `loop.run_until_complete` tracked as async dispatch |
| **Rust** | `async fn`; `.await`; `tokio::spawn` / `async_std::task::spawn` | `.await` produces a call edge to the async fn body; `spawn` produces an edge labeled with `async` + `concurrent` (not awaited in caller scope) |
| **Java** | `CompletableFuture`; `ExecutorService.submit`; virtual threads | `thenApply` / `thenCompose` lambdas tracked as call edges; `submit` targets tracked as async edges |
| **Go** | `go` statement (goroutine launch) | `go func()` produces a call edge labeled `async` + `concurrent`; channel send/receive tracked as synchronization points |
| **C#** | `async`/`await`; `Task.Run`; `Task.ContinueWith` | `await` produces edge to async method body; continuation lambdas tracked as call edges; fault continuations labeled `exception` |
| **Kotlin** | Coroutines (`suspend fun`, `launch`, `async`) | `launch` / `async` blocks tracked as call edges with `async` attribute; `await()` on `Deferred` produces a call edge |
| **Swift** | Swift Concurrency (`async`/`await`; `Task { }`) | Same pattern as Rust/Kotlin |

Async edges are traversable in query filters: `cgx query 'calls_async_to(<target>)'` returns callers that invoke `<target>` through an async mechanism. See docs/05-queries.md for query syntax.

---

## LS-5: Optional Enrichment via SCIP Ingestion

SCIP (Stack-based Index of Positions and Calls) is a protocol for representing
pre-computed symbol resolution and reference graphs. `cgx` can ingest an
externally generated SCIP index to upgrade call edges from heuristic resolution
(`probable`) to type-resolved resolution (`certain`) for Tier 1 and Tier 2
languages.

**SCIP ingestion is optional enrichment, not baseline.** `cgx` does not require
a SCIP index to function; it falls back to its own heuristic resolution when
none is provided. SCIP ingestion is an accuracy upgrade for teams whose build
tooling already generates SCIP outputs (e.g., `scip-typescript`, `scip-rust` /
`rust-analyzer --emit-scip`, `scip-python`).

**Stewardship note:** The SCIP protocol is maintained by Sourcegraph. As of the
writing of this document, there is uncertainty about Sourcegraph's long-term
stewardship of the `scip` crate following organizational changes. The SCIP
protobuf schema itself is stable and versioned; if the upstream crate becomes
unmaintained, `cgx` can regenerate bindings from the protobuf schema. For this
reason, SCIP ingestion is isolated behind an optional feature flag and is not
part of the critical indexing path.

**LSP-based extraction** (shelling out to a running language server for
type-resolved call targets) is classified as out of scope for v1. It adds
per-language server startup latency that is incompatible with the <100ms warm
startup target. It may be addressed in a later tier.

---

## LS-6: Confidence Degradation Across Tiers

Confidence labels (`certain`, `probable`, `possible`) from docs/03-code-graph-model.md
(GM-5) degrade predictably as language support tier decreases and as dispatch
complexity increases.

| Scenario | Tier 1 | Tier 2 | Tier 3 |
|---|---|---|---|
| Direct static call to named function | `certain` | `probable` | `possible` |
| Virtual / interface dispatch (resolved) | `probable` | `possible` | — |
| Dynamic dispatch / reflection / eval | `possible` | `possible` | — |
| Cross-file call (import-resolved) | `certain` or `probable` | `probable` | — |
| Intra-file call | `certain` | `probable` | `possible` |
| Async callback | `probable` | `possible` | — |

Users and agents querying `cgx` can filter by minimum confidence:
`cgx query '<q>' --min-confidence probable` excludes `possible` edges. See
docs/05-queries.md for query options.

**No confidence label is ever omitted.** Every edge in the output carries an
explicit confidence label. The system does not silently drop uncertain edges;
it labels them as `possible` and lets the caller decide whether to include
them. This is consistent with the soundiness principle stated in
docs/01-vision-and-principles.md.
