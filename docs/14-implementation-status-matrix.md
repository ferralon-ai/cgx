# 14 — Implementation Status Matrix

> **Keep this matrix in sync** when an adapter gains a capability or a new language is added. Features are rows; languages are columns. See [08 — Language Support](08-language-support.md) for the tier model and the per-language specification.

**Legend:** `✓` implemented · `~` partial (see the note) · `planned` applicable to the language but not implemented · `—` not applicable to the language.

Every cell below was checked against `crates/` at the commit this file ships on. A cell is `✓` only where code constructs the fact, not where the schema has room for it.

## Section 1 — Adapter capabilities

| Capability | Rust | TS/JS | Go | Python | Java | C | C++ | C# |
|---|---|---|---|---|---|---|---|---|
| Parse (tree-sitter) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Symbol extraction (defs) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Call refs | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Imports / exports [^imp] | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Inheritance / overrides [^inh] | ✓ | planned | ~ | ✓ | ✓ | — | ✓ | planned |
| Entrypoint hints [^ep] | ✓ | ✓ | ✓ | ✓ | ✓ | ~ | ~ | planned |
| Cut hints | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Own-effects [^eff] | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Intraprocedural dataflow (SSA) | ✓ | ✓ | ✓ | ✓ | ✓ | planned | planned | planned |
| Receiver narrowing (opt-in) [^narrow] | planned | planned | ✓ | ✓ | planned | — | planned | planned |
| Concurrency / async hints [^conc] | ~ | ~ | ~ | ~ | ~ | ~ | ~ | planned |
| Implicit call sites (`ImplicitKind`) [^impl] | ~ | planned | ~ | planned | planned | — | planned | planned |
| SCIP enrichment [^scip] | ✓ | planned | planned | planned | planned | ✓ | ✓ | planned |
| Framework packs [^fw] | planned | planned | planned | planned | planned | planned | planned | planned |

[^imp]: For C and C++ an `#include` is recorded as an import fact; the preprocessor is grammar-only, so nothing is expanded or resolved through it (see [08 — Language Support](08-language-support.md)).
[^inh]: Rust emits `Inherits` (supertrait bounds), `Implements`, and per-member `Overrides`; Java emits all three; Python and C++ emit `Inherits` and `Overrides` (C++ has no separate interface construct, so a base class is always `Inherits`). Go is `~` because it has no inheritance to model and emits `Implements` only — the correct full coverage for the language. C has no inheritance, so `—`. **TS/JS is `planned`, not `—`:** ECMAScript `extends`/`implements` is squarely applicable, and `cgx-lang-ts` emits no `Inherits`, `Implements`, or `Overrides` relation at all today.
[^ep]: Rust, TS/JS, Go, Python and Java detect `Main` and `Test`; Rust adds `AsyncMain` (`#[tokio::main]`, `#[async_std::main]`). C and C++ detect `main` only and recognise no test framework, hence `~` — which is also why `impacted-tests` does not cover them. No adapter emits `HttpHandler`, and nothing anywhere constructs `Declared` — both variants exist in `EntrypointKind` (`crates/cgx-core/src/node.rs`) and are never populated. Route-handler detection is what the framework-pack row would deliver.
[^eff]: All seven adapters emit `IoFile`, `IoNet`, `IoProc`, `Blocking`, `DynamicCode`, `Nondeterministic`, and `Spawns`. Most of the mapping lives in each adapter's `effects.rs`; Go, Python and TS/JS emit `Spawns` from `extract.rs` at the launch site.
[^narrow]: Receiver narrowing is an opt-in resolution pass (`cgx index --receiver-narrowing`, or `CGX_RECEIVER_NARROWING=1`; `CGX_PRECISION_TRACE=1` traces each decision) that uses local type evidence to shrink a method call's candidate set. Its per-language rules exist for Python and Go only (`rules()` in `crates/cgx-resolve/src/receiver/lang.rs`); a call in any other language passes through unchanged. Off by default. C is `—`: it has no method calls.
[^conc]: `~` across the board: every adapter emits the `Spawns` effect at thread/executor/async launch sites (C: `pthread_create`/`thrd_create`; C++ adds `std::thread`/`std::async`), but no adapter emits lock sets, suspension points, or channel dataflow (GM-9..11 remain schema-room).
[^impl]: Only Go (`Defer`) and Rust (one `Iterator` construction) build any `ImplicitKind` variant. The other ten variants — drop, deref, coercion, operator, context-enter/exit, property, static-init, conversion — are constructed by no adapter in any language, so the per-language tables in docs/08 LS-8 read ahead of the code for Rust as well as for Python and Java. C++ is `planned` (destructors, operator overloads and conversions all apply; none is modelled); C has none of these constructs.
[^scip]: The relabel pass itself (`crates/cgx-index/src/pipeline/scip_relabel.rs`) is language-agnostic and upgrade-only. What is language-specific is whether a SCIP producer's symbol scheme maps onto that adapter's FQN convention. Two schemes exist, chosen from the index's tool name (`SymbolScheme::detect` in `crates/cgx-scip/src/lib.rs`): rust-analyzer (`crates/cgx-scip/src/symbol.rs`) and scip-clang (`crates/cgx-scip/src/symbol_cxx.rs`), which covers C and C++. scip-clang promotions are fail-closed on compilation-database completeness: unless every discovered C++ translation unit is covered (or `--compdb-complete` asserts it), promotion is capped at `probable`. No scheme exists for TS/JS, Go, Python or Java.
[^fw]: **No framework-pack code exists in any adapter.** There is no annotation-to-semantic-class lowering, no guard/interception/keep-alive fact construction, and no `HttpHandler` entrypoint anywhere in `crates/`. docs/12 specifies this capability; nothing implements it yet.

## Section 2 — Query surface

| Query | Rust | TS/JS | Go | Python | Java | C | C++ | C# |
|---|---|---|---|---|---|---|---|---|
| `callers` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `callees` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `reaches` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `paths` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `unused` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `search` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `symbols` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `explain` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `flows-to` / `flows-from` [^flow] | ✓ | ✓ | ✓ | ✓ | ✓ | planned | planned | planned |
| `query` (CQL) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `diff` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `impacted-tests` [^imt] | ✓ | planned | ✓ | ✓ | ✓ | planned | planned | planned |
| `pack` [^pack] | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `dump` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `coupling` [^coup] | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `push` / `pull` / `configure-remote` [^xport] | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| effects queries [^effq] | ~ | ~ | ~ | ~ | ~ | ~ | ~ | planned |

[^flow]: `flows-to`/`flows-from` walk value nodes, which only the intraprocedural dataflow pass produces (Section 1). C and C++ have no dataflow pass, so these queries return nothing for them.
[^imt]: `cgx impacted-tests` maps a change to the test entrypoints that reach it, so it needs `Test` entrypoint hints. Its supported set is `SUPPORTED_LANGS` in `crates/cgx-query/src/impacted.rs`: Rust, Go, Java and Python. TS/JS has test entrypoints but is not yet in that set; C and C++ have none. A change touching only unsupported languages exits 4 with an `impacted-language-unsupported` reason rather than an empty list.
[^pack]: `cgx pack` wraps existing primitives as JSON cards. `dependency-footprint` is a callees walk and works wherever call edges do; `interface-map` reads `Implements` edges only (`crates/cgx-cli/src/pack.rs`), so it is populated for Rust, Go and Java and empty for TS/JS, Python, C and C++, which emit no `Implements`.
[^coup]: `cgx coupling` is one of the commands with no language column that can be `planned`. It reads committed git history and never parses a file or opens the index, so it works on any file in any language — including C#, and including languages with no adapter at all. The trade is stated in its own approximation contract: co-change is attributed at **file** granularity, so a reported pair may have had entirely unrelated symbols edited in the same commit. Symbol-level coupling would require indexing every historic tree and is a different capability.
[^xport]: The transport commands move the already-built `.cgx/` object store through git under `refs/cgx/index`. They never parse source, so they are language-independent.
[^effq]: `~` for every shipped language, and the limit is the *surface*, not the adapter — the facts are computed for every language in this matrix, and no command exposes them.

    **The facts.** Every symbol node carries `own_effects` and `transitive_effects` (`crates/cgx-core/src/node.rs`). `transitive_effects` is computed at index time by the effect-closure pass — `apply_effects` (`crates/cgx-index/src/pipeline.rs`) runs unconditionally on both index entry points in `crates/cgx-index/src/lib.rs` — and both sets are stored only inside the node's object in `.cgx/objects`.

    **The routes, all closed.** No CQL node property: the node-property match in `crates/cgx-cql/src/lower.rs` rejects `transitive_effects` explicitly and does not know `own_effects`. No CLI flag, no MCP tool, and `cgx dump` prints nodes and edges without effects. The hidden `--sql` flag is not a route either: its own doc comment (`QueryArgs::sql` in `crates/cgx-cli/src/main.rs`) claims "passing this flag always exits 2." It does not:

    - `cgx query '…' --sql` → **exit 2**, `cgx: the --sql recursive-CTE interface is not implemented in this release` (`run_query`, which rejects before doing any work).
    - `cgx callers <sym> --sql` → **exit 0**, ordinary output, the flag silently ignored. `args.sql` is read at exactly one site in the whole CLI, so every other subcommand sharing the flag accepts and discards it.

    Both verified by running. That asymmetry is the `--sql` defect in its loud form: the same flag is a hard usage error on one subcommand and a no-op on the rest, and the flag's own help text describes neither behaviour correctly.

A query works for a language iff the facts it needs exist (Section 1). Update both sections together when an adapter changes.

## Section 3 — Answer-envelope coverage

Cross-cutting and language-independent: which surfaces attach the approximation contract (A3/A4) and the index-freshness envelope. Neither is universal, and the pairing differs per command — verified at each render site in `crates/cgx-cli/src/output.rs` and `crates/cgx-mcp/src/tools.rs`.

| Surface | Approximation contract | Freshness envelope | Why |
|---|---|---|---|
| `callers`, `callees`, `reaches`, `paths`, `unused`, `query` | ✓ | ✓ | Traversal answers; both questions apply. |
| `explain`, `search`, `symbols` | — | ✓ | They report facts about symbols rather than the result of a walk, so there is no approximation direction to state. |
| `coupling` | ✓ | — | It never opens the index, so an index-freshness envelope would describe a graph the answer never consulted. Its contract uses a history-specific modeled boundary, not the call-graph one. |
| `impacted-tests` | ✓ | ✓ | A selection answer with both a direction (it can miss and over-include tests) and an index to be fresh against. |
| `pack` | ✓ | — | Each card carries its own approximation contract; the pack's header reports graph-honesty counts (confidence mix, cut markers, dangling refs) rather than freshness. |
| `diff`, `doctor`, `index`, `dump` | — | — | Not yet wired to either. |
| `push`, `pull`, `configure-remote` | — | — | Transport, not answers: each prints what it moved or configured. |
| `--format dot` / `mermaid` / `d2` | — | — | Raw graph source; the emitters carry no prose lines, and freshness is deliberately not even computed for them. |
| MCP errors | — | — | An error is not an answer. |

Over MCP, `matches_head` is **three-valued**, and `null` is the ordinary case rather than an edge case: with the default `include_dirty: true`, any indexed repository reaches the indeterminate state, and the verdict renders as `unknown`. Treating it as a boolean is wrong in the common case. See [09 — Architecture](09-architecture.md) AR-13.
