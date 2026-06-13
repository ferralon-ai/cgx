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
- LS-7: Per-language concurrency semantics (spawn/suspend/lock constructs + detached-error behavior)
- LS-8: Per-language primitive harvest table (implicit call kinds, resource-pair syntax, metadata carriers, build-config mechanism, non-call dataflow)

---

## LS-1: Tiered Language Strategy

### Tier 1 — Deep semantics

Full parse plus language-specific semantic enrichment. Scope graph construction
via `tree-sitter-graph` with language-specific rules. Where available, an
optional language-specific deep parser augments the tree-sitter AST.

**Tier 1 at launch: Rust, TypeScript, JavaScript** (TypeScript and JavaScript
share one adapter family).

**Tier 1 planned (in order):** Python, Go, Java, C#. Rows for planned languages
in LS-7 and LS-8 are specification-ahead-of-implementation (`Status: roadmap` for
those rows); only Rust and TypeScript/JavaScript rows are Phase-1 commitments.

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

**Detached-task error behavior.** Errors that occur inside a spawned task do not propagate back to the spawner via the call stack (see GM-9 in docs/03-code-graph-model.md). Each language surfaces the spawned task's error through its own completion handle; the table below covers the four primary Tier 1 cases:

| Language | Spawn construct | Detached error surface |
|---|---|---|
| **Go** | `go func()` / `go goroutine` | Goroutine panics terminate the program by default (no recovery at spawner); recovery is only possible inside the goroutine itself via `defer recover()`. Panics are not delivered to the spawner. |
| **Rust** | `tokio::spawn` / `async_std::task::spawn` | Returns a `JoinHandle<T>`; calling `.await` on it yields `Err(JoinError)` if the task panicked. The error is surfaced on the `JoinHandle` path, not propagated to the spawner automatically. Dropping the `JoinHandle` detaches the task; panics in detached tasks do not affect the spawner. |
| **JavaScript / TypeScript** | `Promise` without `.catch`; `async` IIFE without `await` | An unhandled rejection fires the global `unhandledRejection` event (Node.js) or is surfaced as a console warning (browser). It is not delivered to the spawner's call frame. |
| **Python** | `asyncio.create_task` | An exception in a task that is never awaited fires `Task exception was never retrieved` via the event loop's exception handler. Awaiting the task raises the exception at the `await` site; if the task is discarded, the exception is lost. |

In `cgx`, the exceptional-class edges inside a spawned task have `seen_exceptional` scoped to the task's own path walk; they do not make call sites in the spawner exception-transient. See GM-9.2 in docs/03-code-graph-model.md for the full semantics.

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

---

## LS-7: Per-Language Concurrency Semantics

**Status:** `schema-room` — spawn, suspension, and lock constructs must be
catalogued per-language now so that GM-9, GM-10, GM-11, and GM-12 schema
reservations are grounded in concrete language surface; the full concurrency
query surface (Q-24) follows.

**Scope note:** rows for Rust and TypeScript/JavaScript are Phase-1 implementation
commitments. Rows for Python, Go, Java, C/C++, Kotlin, Swift, C# are
specification-ahead-of-implementation (`Status: roadmap`) — they ground the schema
reservation but are not implemented until their language's Tier-1 adapter ships.

This section specifies the per-language mapping of concurrency constructs to
the graph attributes defined in docs/03-code-graph-model.md: `spawns` edge kind
(GM-9), `suspends` node property (GM-10), lock-set entries (GM-11), and effect
values (GM-12). Languages covered match the Tier 1 set from LS-1 plus C/C++ for
completeness.

### LS-7.1 — Spawn constructs

| Language | Spawn construct | `cgx` representation | Notes |
|---|---|---|---|
| **Rust** | `tokio::spawn(async { … })`, `async_std::task::spawn`, `std::thread::spawn` | `spawns` edge from call site to the spawned closure or async block; `edge_condition` reflects the guard at the call site | `JoinHandle` drop (detach) is annotated; `tokio::task::spawn_blocking` adds `blocking` effect to the spawned fn |
| **Go** | `go f()`, `go func() { … }()` | `spawns` edge from `go` statement to the goroutine body | Channel operations on the spawned goroutine are tracked as synchronization points for lock-set analysis |
| **Python** | `asyncio.create_task(coro)`, `asyncio.ensure_future`, `loop.run_in_executor` | `spawns` edge to the coroutine or executor function | `concurrent.futures.Future` submit tracked as `spawns` with `blocking` effect in executor |
| **JavaScript / TypeScript** | `new Promise(fn)`, unhandled `async` IIFE (`(async () => { … })()`), `setTimeout`/`setImmediate` callbacks | `spawns` edge to the callback or async body; `.then` / `.catch` / `.finally` continuations are tracked as `calls:async` edges (`edge_condition: conditional`/`exception`), not spawn — see LS-7 scope note and LS-4 async model | Promise constructor callback is a spawned async context if not explicitly awaited |
| **Java** | `ExecutorService.submit`, `CompletableFuture.runAsync`, `new Thread(r).start`, virtual thread `Thread.ofVirtual().start` | `spawns` edge to the `Runnable`/`Callable` body | `CompletableFuture.thenApplyAsync` is a continuation chain and uses `calls:async`; only `.runAsync` with no handle is a detached spawn |
| **C / C++** | `pthread_create`, `std::thread` constructor, `std::async(std::launch::async, …)` | `spawns` edge to the thread function | `std::async` with `std::launch::deferred` is modelled as `calls:async`, not spawn |

### LS-7.2 — Suspension constructs

| Language | Suspension construct | `suspends = true` emitted on | Notes |
|---|---|---|---|
| **Rust** | `.await` expression | The `calls:async` edge source node | Every `.await` is a suspension; the scheduler may schedule other tasks before resumption |
| **JavaScript / TypeScript** | `await` expression | The `calls:async` edge source node | Includes `await` inside loops and `await` in `Promise.all` arguments |
| **Python** | `await` expression | The `calls:async` edge source node | `asyncio.sleep(0)` is a well-known yield point; `cgx` marks it `suspends = true` |
| **Go** | Channel send (`ch <-`) / receive (`<- ch`), `select` statement, `runtime.Gosched` | The channel-operation node | Go does not have `await`; cooperative yield happens at channel operations and GC preemption points |
| **Java** | `Future.get()` (blocking join), `CompletableFuture.join()`, `Object.wait()` | The blocking-join call-site node | Non-blocking continuations (`thenApply`) do not set `suspends` |
| **Kotlin** | `suspend fun` call site; `Deferred.await()` | The suspend-call-site node | `delay()` and other suspend library functions are pre-tagged in `cgx`'s built-in Kotlin model |
| **C / C++** | No language-level suspension primitive | Not emitted | Platform-level blocking (POSIX `select`, `epoll_wait`) tagged with `blocking` effect only |

### LS-7.3 — Lock constructs

| Language | Acquire construct | Release construct | `cgx` modelling |
|---|---|---|---|
| **Rust** | `Mutex::lock()`, `RwLock::read()`, `RwLock::write()` | `MutexGuard` / `RwLockGuard` drop (scope-exit edge) | RAII; lock held from acquire call site to end-of-scope; scope-exit edge models release; `lock_set` populated accordingly |
| **Go** | `sync.Mutex.Lock()`, `sync.RWMutex.Lock()`, `sync.RWMutex.RLock()` | `Unlock()`, `RUnlock()` | Explicit acquire/release; `defer mu.Unlock()` is modelled as release on the `always`-condition scope-exit edge |
| **Python** | `threading.Lock.acquire()`, `with lock:` (context manager) | `release()`, `__exit__` | `with` block modelled as acquire at entry, release on scope-exit (including exception paths via `__exit__` which is `always`-conditioned) |
| **JavaScript / TypeScript** | No language-level mutex; `Atomics.wait` (SharedArrayBuffer); library mutexes (e.g. `async-mutex`) | Library-specific release | `Atomics.wait` annotated with `blocking` effect; library mutexes tracked if declared in `cgx.toml` |
| **Java** | `synchronized` block / method enter; `Lock.lock()`, `ReadWriteLock` | `synchronized` block exit (implicit); `Lock.unlock()` | `synchronized` block exit is modelled as an `always`-conditioned scope-exit edge; `Lock.unlock()` in `finally` is `always`-conditioned inside the finally block |
| **C / C++** | `pthread_mutex_lock`, `std::mutex::lock`, `std::unique_lock` constructor | `pthread_mutex_unlock`, `std::mutex::unlock`, `std::unique_lock` destructor | RAII (`unique_lock`) modelled same as Rust; C explicit acquire/release tracked as pair; `cgx` applies heuristic matching for `pthread_*` pairs |

### LS-7.4 — Effect values per language

The following table maps each language's concrete constructs to the GM-12
effect lattice values. Entries marked `(built-in)` are pre-populated in
`cgx`'s per-language model; all others follow from the transitive computation
rule (GM-12.2).

| Language | Construct | Effect value |
|---|---|---|
| **Rust** | `tokio::spawn`, `std::thread::spawn`, `task::spawn_blocking` | `spawns` |
| **Rust** | `async fn` / `.await` | (transitively computes `io.*` from async body) |
| **Rust** | `tokio::time::sleep`, `thread::sleep`, `std::sync::Mutex::lock` | `blocking` |
| **Rust** | `rand::random`, `std::time::SystemTime::now` | `nondeterministic` |
| **Go** | `go` statement | `spawns` |
| **Go** | `time.Sleep`, blocking channel receive, `sync.Mutex.Lock` | `blocking` |
| **Go** | `time.Now`, `rand.Int`, `os.Getenv` | `nondeterministic` |
| **Python** | `asyncio.create_task`, `threading.Thread.start`, `concurrent.futures.submit` | `spawns` |
| **Python** | `time.sleep`, `threading.Lock.acquire` (blocking) | `blocking` |
| **Python** | `eval`, `exec`, `importlib.import_module` | `dynamic-code` |
| **Python** | `time.time`, `random.random`, `os.environ` | `nondeterministic` |
| **JavaScript / TypeScript** | Unhandled async IIFE, `new Worker(…)` | `spawns` |
| **JavaScript / TypeScript** | `eval`, `new Function(…)` | `dynamic-code` |
| **JavaScript / TypeScript** | `Date.now`, `Math.random` | `nondeterministic` |
| **Java** | `ExecutorService.submit`, `new Thread(…).start`, `CompletableFuture.runAsync` | `spawns` |
| **Java** | `Thread.sleep`, `Object.wait`, `Future.get` (blocking) | `blocking` |
| **Java** | `Class.forName`, `Method.invoke` | `dynamic-code` |
| **Java** | `System.currentTimeMillis`, `new Random()` | `nondeterministic` |
| **C / C++** | `pthread_create`, `std::thread` constructor | `spawns` |
| **C / C++** | `sleep`, `usleep`, `pthread_mutex_lock` (blocking) | `blocking` |
| **C / C++** | `dlopen`, `dlsym` | `dynamic-code` |
| **C / C++** | `time(NULL)`, `rand()` | `nondeterministic` |

All `io.*` effects (`io.file`, `io.net`, `io.db`, `io.proc`) are assigned by the
producing rule when a call site resolves to a known system-call or standard-library
I/O function. The full built-in list is managed in the language model files and is
user-extensible via `cgx.toml`.

---

## LS-8: Per-Language Primitive Harvest Table

**Status:** `schema-room` — the primitive kinds in this table must be reserved
in the schema (GM-15, GM-16, GM-19, DF-20) before the schema stabilises; the
mechanical harvest from language syntax into these primitives is the prerequisite
for framework-pack evaluation (see docs/12-language-primitives-and-frameworks.md
FW-1). Per-language packs for resource-pair syntax (column 3) harvest into GM-13
without requiring user configuration.

**Scope note:** rows for Rust (LS-8.1) and TypeScript/JavaScript (LS-8.4) are
Phase-1 implementation commitments. Rows for Go, Python, Java, C# are
specification-ahead-of-implementation (`Status: roadmap`) — they ground the
schema reservation but are not implemented until the language's Tier-1 adapter
ships.

This table maps the concrete surface syntax of each Tier 1 language (plus C#,
which is covered in LS-2 and LS-3) to the `cgx` primitive types defined in
docs/03-code-graph-model.md and docs/04-dataflow-and-provenance.md. The
`implicit:<kind>` enum is defined in GM-16; it is reproduced here for reference
only — the authoritative definition is GM-16.

`implicit:<kind>` enum values: `drop`, `destructor`, `deref`, `coercion`,
`operator`, `iterator`, `context-enter`, `context-exit`, `property`, `defer`,
`static-init`, `conversion`.

For the framework-pack layer that sits on top of the metadata-carrier column, see
docs/12-language-primitives-and-frameworks.md.

### LS-8.1 — Rust

| Construct category | Concrete syntax | `cgx` primitive | Notes |
|---|---|---|---|
| **Implicit calls — drop** | Scope-exit of a value implementing `Drop` | `calls` with `implicit:drop`; also GM-13 resource-pair release | The release half of the resource pair declared by `Drop`; harvested automatically without user `[[resource_pairs]]` entry |
| **Implicit calls — deref** | `*x` where `x: impl Deref`, auto-deref in method call | `calls` with `implicit:deref`; confidence `certain` for monomorphized, `probable` for `dyn` | Deref coercion chains are followed transitively |
| **Implicit calls — coercion** | `T as U` where `From<T>` / `Into<U>` / `?` operator | `calls` with `implicit:coercion`; `coerce(T,U)` transformation kind on the pedigree edge (DF-10) | The `?` operator invokes `From::from` on the error type; this is an implicit call |
| **Implicit calls — operator** | `a + b`, `a == b`, `a < b`, `*a`, `a[i]` | `calls` with `implicit:operator` to the `Add::add` / `PartialEq::eq` / `Index::index` etc. implementation | Monomorphized: `certain`; `dyn Trait`: `probable` |
| **Implicit calls — iterator** | `for x in iter` | `calls` with `implicit:iterator` to `IntoIterator::into_iter` and `Iterator::next` | Harvested at the `for` loop desugaring; `next` call has `edge_condition = loop` |
| **Native resource-pair syntax** | `MutexGuard` drop, `File` drop, `impl Drop` release | GM-13 resource pair (acquire = constructor / `lock()`, release = `implicit:drop` scope-exit) | Built-in pairs cover `Mutex::lock` / `MutexGuard::drop`, `File::open` / `File::drop` |
| **Metadata carrier** | `#[attr]`, `#[derive(Trait)]`, `#[cfg(...)]` | GM-15 metadata fact on the annotated symbol | `#[cfg(...)]` → GM-19 `cfg-condition` attribute; `#[derive(...)]` → GM-16 `generated-member` class |
| **Build-config mechanism** | `#[cfg(feature = "x")]`, `#[cfg(target_os = "linux")]` | GM-19 `cfg-condition` attribute; feature flags sourced from `Cargo.toml` | |
| **Channel / non-call dataflow** | `tx.send(value)` / `rx.recv()` (tokio, std::sync) | DF-20 non-call dataflow edge; `derives-from` between send and receive sites | Channel value pedigree preserved across the send/receive boundary |

### LS-8.2 — Go

| Construct category | Concrete syntax | `cgx` primitive | Notes |
|---|---|---|---|
| **Implicit calls — defer** | `defer f()` | `calls` with `implicit:defer`; `edge_condition = always`; also GM-13 resource-pair release when paired with a resource open | `defer` runs on both normal and panic exit; the `always` condition captures this |
| **Implicit calls — static-init** | `func init()` | `calls` with `implicit:static-init`; `entrypoint` adjacent | Each package's `init()` functions are invoked at package load time in declaration order |
| **Implicit calls — iterator** | `for k, v := range collection` | `calls` with `implicit:iterator` to the range iteration protocol | Range over maps, slices, channels; channel range blocks until close |
| **Native resource-pair syntax** | `defer f.Close()`, `defer mu.Unlock()` | GM-13 resource pair; acquire = `os.Open` / `mu.Lock()`, release = `implicit:defer` scope-exit edge | Built-in pairs cover `os.Open` / `(*os.File).Close`, `sync.Mutex.Lock` / `Unlock` |
| **Metadata carrier** | Go struct tags: `` `json:"field"` ``, `` `yaml:"field"` ``, `` `db:"field"` `` | GM-15 metadata fact on the struct field; `keep-alive` class for serialization tags | Struct tags are the primary metadata mechanism; no annotation syntax |
| **Build-config mechanism** | `//go:build linux && amd64`, `// +build linux` (legacy) | GM-19 `cfg-condition` attribute; sourced from file-level build constraint comments | |
| **Channel / non-call dataflow** | `ch <- value` (send), `<-ch` (receive), `select` | DF-20 non-call dataflow edge; also GM-10 suspension point on channel send/receive in goroutines | Channel direction (send-only, receive-only) preserved in the edge attributes |

### LS-8.3 — Python

| Construct category | Concrete syntax | `cgx` primitive | Notes |
|---|---|---|---|
| **Implicit calls — context-enter/exit** | `with resource as r:` | `calls` with `implicit:context-enter` to `__enter__`; `calls` with `implicit:context-exit` to `__exit__`; also GM-13 resource pair | `__exit__` has `edge_condition = always` (called on both normal and exception exit) |
| **Implicit calls — iterator** | `for x in iterable:`, list comprehension | `calls` with `implicit:iterator` to `__iter__` and `__next__` | Generator protocol (`__iter__` returns `self`; `__next__` advances) |
| **Implicit calls — property** | `obj.attr` where `attr` is a `@property` | `calls` with `implicit:property` to the getter/setter/deleter | Descriptor protocol generalises this; `@property` is the common case |
| **Implicit calls — operator** | `a + b`, `a == b`, `a[i]`, `a[i] = v` | `calls` with `implicit:operator` to `__add__`, `__eq__`, `__getitem__`, `__setitem__` | Duck-typed; confidence `probable` with name-match, `possible` without type annotation |
| **Implicit calls — static-init** | Module-level code executed on import | `calls` with `implicit:static-init`; `entrypoint` adjacent; `edge_condition = always` on module load | Python module execution on import is a first-class effect; relevant for DF-20 import-time side effects |
| **Native resource-pair syntax** | `with open("f") as fh:`, `with lock:` | GM-13 resource pair; acquire = `__enter__`, release = `implicit:context-exit` | Built-in pairs cover `open()` / `__exit__`, `threading.Lock` / `__exit__` |
| **Metadata carrier** | `@decorator`, `@app.route(...)`, `@dataclass` | GM-15 metadata fact on the decorated symbol | Decorators are the primary metadata mechanism; framework packs extend (see docs/12 FW-3.2) |
| **Build-config mechanism** | `if sys.version_info >= (3, 10):`, `if TYPE_CHECKING:` | GM-19 `cfg-condition` attribute (heuristic; confidence `probable`) | |
| **Channel / non-call dataflow** | `asyncio.Queue.put` / `asyncio.Queue.get`, `queue.Queue` | DF-20 non-call dataflow edge | Async queue send/receive treated as non-call dataflow; pedigree preserved |

### LS-8.4 — TypeScript / JavaScript

| Construct category | Concrete syntax | `cgx` primitive | Notes |
|---|---|---|---|
| **Implicit calls — iterator** | `for (const x of iterable)`, spread `[...iter]` | `calls` with `implicit:iterator` to `Symbol.iterator` / `next()` | Generator protocol; async iterators add `Symbol.asyncIterator` |
| **Implicit calls — property** | Proxy traps (`get`, `set`); getter/setter definitions | `calls` with `implicit:property` to the getter/setter | Proxy interception is modelled as `possible`; static getter/setter is `probable` with SCIP |
| **Implicit calls — operator** | Tagged template literals `` tag`...` `` | `calls` with `implicit:operator` to the tag function | Tag function receives the template parts and interpolated values |
| **Implicit calls — coercion** | Implicit `toString()` in string concatenation, `valueOf()` in arithmetic | `calls` with `implicit:coercion`; `coerce(any,string)` or `coerce(any,number)` transformation kind (DF-10) | High-value for type-juggling auth-bypass queries (DF-10) |
| **Implicit calls — static-init** | Top-level module code, `import` side effects | `calls` with `implicit:static-init`; DF-20 import-time edge | `import "side-effect-module"` executes module code; entrypoint-adjacent |
| **Native resource-pair syntax** | No language-level syntax; library patterns (`using` in TS 5.2+, `Symbol.dispose`) | GM-13 resource pair when `Symbol.dispose` is implemented; `using` keyword → `implicit:context-exit` | TS 5.2 `using` keyword is an explicit resource-pair declaration |
| **Metadata carrier** | TypeScript decorators (`@Controller()`, `@Injectable()`) | GM-15 metadata fact on the decorated symbol | Requires `experimentalDecorators` or TS 5.0+ native decorators; framework packs extend (docs/12 FW-3.4) |
| **Build-config mechanism** | `process.env.NODE_ENV === "production"`, `typeof window !== "undefined"` | GM-19 `cfg-condition` attribute (heuristic; confidence `probable`) | Bundler-level dead-code elimination (Rollup/esbuild `tree-shaking`) is not modelled |
| **Channel / non-call dataflow** | `postMessage` / `message` event (Workers), `EventEmitter.emit` / `on` | DF-20 non-call dataflow edge; Worker channel treated as DF-20 with `possible` confidence | EventEmitter string-named events: literal names resolve at `probable`; computed names at `possible` |

### LS-8.5 — Java

| Construct category | Concrete syntax | `cgx` primitive | Notes |
|---|---|---|---|
| **Implicit calls — static-init** | `static { ... }` block | `calls` with `implicit:static-init` to the static initializer block | Executed once on class loading; entrypoint-adjacent |
| **Implicit calls — coercion** | Implicit `toString()` in string concatenation (`"x" + obj`) | `calls` with `implicit:coercion` to `Object.toString()`; `coerce(Object,String)` transformation kind (DF-10) | |
| **Implicit calls — iterator** | Enhanced `for (T x : collection)` | `calls` with `implicit:iterator` to `Iterable.iterator()` and `Iterator.next()` | |
| **Implicit calls — operator** | No operator overloading in Java; string `+` is `concat` kind (DF-10) | `concat` transformation kind on `derives-from` edge | |
| **Native resource-pair syntax** | `try (Resource r = new Resource()) { }` (try-with-resources) | GM-13 resource pair; acquire = constructor / `open()`, release = `implicit:context-exit` mapped to `AutoCloseable.close()` | try-with-resources is the native Java resource-pair syntax; harvested without user configuration |
| **Metadata carrier** | `@Annotation`, `@interface` declarations | GM-15 metadata fact on the annotated symbol | Spring, JPA, Jakarta annotations extend via framework packs (docs/12 FW-3.1, FW-3.3) |
| **Build-config mechanism** | Maven profiles, Gradle build variants, `@ConditionalOnProperty` | GM-19 `cfg-condition` attribute; `probable` confidence for annotation-driven conditions | |
| **Channel / non-call dataflow** | `BlockingQueue.put` / `take`, `CompletableFuture` completion | DF-20 non-call dataflow edge; CompletableFuture completion chain tracked as async edges (LS-4) | |

### LS-8.6 — C#

| Construct category | Concrete syntax | `cgx` primitive | Notes |
|---|---|---|---|
| **Implicit calls — property** | `obj.Property` (C# properties are methods) | `calls` with `implicit:property` to `get_Property()` / `set_Property()` | Properties are syntactic sugar over getter/setter methods; the implicit call is always present |
| **Implicit calls — iterator** | `foreach (var x in collection)`, `yield return` | `calls` with `implicit:iterator` to `IEnumerable.GetEnumerator()` and `IEnumerator.MoveNext()` | `yield return` desugars to a state-machine class; the state-machine type is a `generated-member` |
| **Implicit calls — coercion** | Implicit conversion operators (`operator T(U u)`) | `calls` with `implicit:coercion`; `coerce(U,T)` transformation kind (DF-10) | Explicit `operator` keyword defines the conversion; invoked implicitly at assignment |
| **Implicit calls — operator** | `a + b`, `a == b`, `a[i]` | `calls` with `implicit:operator` to the defined `operator +`, `operator ==`, indexer | |
| **Implicit calls — destructor** | Class finalizer `~MyClass()` | `calls` with `implicit:destructor`; confidence `possible` (GC-scheduled, not deterministic) | GC-scheduled; not a reliable resource-release mechanism; modelled for completeness |
| **Native resource-pair syntax** | `using (var r = new Resource()) { }`, `using var r = ...;` | GM-13 resource pair; acquire = constructor, release = `implicit:context-exit` mapped to `IDisposable.Dispose()` | Both `using` statement and `using` declaration (C# 8+) are harvested |
| **Metadata carrier** | `[Attribute]`, custom `Attribute` subclasses | GM-15 metadata fact on the decorated symbol | ASP.NET attributes extend via framework packs (docs/12 FW-3.3) |
| **Build-config mechanism** | `#if DEBUG`, `#if NETCOREAPP3_1`, `[Conditional("DEBUG")]` | GM-19 `cfg-condition` attribute; `[Conditional]` attribute harvested as a cfg-conditioned call site | `[Conditional("DEBUG")]` methods are only called when the named symbol is defined; this is a call-site cfg condition |
| **Channel / non-call dataflow** | `Channel<T>` (`WriteAsync` / `ReadAsync`), `BlockingCollection` | DF-20 non-call dataflow edge; async channel send/receive treated as non-call dataflow | |
