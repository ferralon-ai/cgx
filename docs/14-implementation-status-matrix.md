# 14 — Implementation Status Matrix

> **Keep this matrix in sync** when an adapter gains a capability or a new language is added. Features are rows; languages are columns. See [08 — Language Support](08-language-support.md) for the tier model and the per-language specification.

**Legend:** `✓` implemented · `~` partial (see the note) · `planned` applicable to the language but not implemented · `—` not applicable to the language.

Every cell below was checked against `crates/` at the commit this file ships on. A cell is `✓` only where code constructs the fact, not where the schema has room for it.

## Section 1 — Adapter capabilities

| Capability | Rust | TS/JS | Go | Python | Java | C# |
|---|---|---|---|---|---|---|
| Parse (tree-sitter) | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Symbol extraction (defs) | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Call refs | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Imports / exports | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Inheritance / overrides [^inh] | ✓ | planned | ~ | ✓ | ✓ | planned |
| Entrypoint hints [^ep] | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Cut hints | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Own-effects [^eff] | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Intraprocedural dataflow (SSA) | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Concurrency / async hints [^conc] | ~ | ~ | ~ | ~ | ~ | planned |
| Implicit call sites (`ImplicitKind`) [^impl] | ~ | planned | ~ | planned | planned | planned |
| SCIP enrichment [^scip] | ✓ | planned | planned | planned | planned | planned |
| Framework packs [^fw] | planned | planned | planned | planned | planned | planned |

[^inh]: Rust emits `Inherits` (supertrait bounds), `Implements`, and per-member `Overrides`; Java emits all three; Python emits `Inherits` and `Overrides`. Go is `~` because it has no inheritance to model and emits `Implements` only — the correct full coverage for the language. **TS/JS is `planned`, not `—`:** ECMAScript `extends`/`implements` is squarely applicable, and `cgx-lang-ts` emits no `Inherits`, `Implements`, or `Overrides` relation at all today.
[^ep]: Every adapter detects `Main` and `Test`; Rust adds `AsyncMain` (`#[tokio::main]`, `#[async_std::main]`). No adapter emits `HttpHandler`, and nothing anywhere constructs `Declared` — both variants exist in `EntrypointKind` (`crates/cgx-core/src/node.rs:84`) and are never populated. Route-handler detection is what the framework-pack row would deliver.
[^eff]: All five adapters ship an `effects.rs` emitting `IoFile`, `IoNet`, `IoProc`, `Blocking`, `DynamicCode`, `Nondeterministic`; Rust and Java additionally emit `Spawns`.
[^conc]: `~` across the board: every adapter emits the `Spawns` effect at thread/executor/async launch sites, but no adapter emits lock sets, suspension points, or channel dataflow (GM-9..11 remain schema-room).
[^impl]: Only Go (`Defer`) and Rust (one `Iterator` construction) build any `ImplicitKind` variant. The other ten variants — drop, deref, coercion, operator, context-enter/exit, property, static-init, conversion — are constructed by no adapter in any language, so the per-language tables in docs/08 LS-8 read ahead of the code for Rust as well as for Python and Java.
[^scip]: The relabel pass itself (`crates/cgx-index/src/pipeline/scip_relabel.rs`) is language-agnostic and upgrade-only. What is language-specific is whether a SCIP producer's symbol scheme maps onto that adapter's FQN convention: `crates/cgx-scip/src/symbol.rs` joins descriptors with `::` and prepends the package, and only rust-analyzer-shaped symbols are exercised by tests. TS/JS was previously listed `~`; nothing in `crates/` supports that, so it is `planned` with the rest.
[^fw]: **No framework-pack code exists in any adapter.** Rust and TS/JS were previously listed `~`; there is no annotation-to-semantic-class lowering, no guard/interception/keep-alive fact construction, and no `HttpHandler` entrypoint anywhere in `crates/`. docs/12 specifies this capability; nothing implements it yet.

## Section 2 — Query surface

| Query | Rust | TS/JS | Go | Python | Java | C# |
|---|---|---|---|---|---|---|
| `callers` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `callees` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `reaches` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `paths` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `unused` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `search` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `symbols` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `explain` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `flows-to` / `flows-from` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `query` (CQL) | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `diff` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `coupling` [^coup] | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| effects queries [^effq] | ~ | ~ | ~ | ~ | ~ | planned |

[^coup]: `cgx coupling` is the one query with no language column that can be `planned`. It reads committed git history and never parses a file or opens the index, so it works on any file in any language — including C#, and including languages with no adapter at all. The trade is stated in its own approximation contract: co-change is attributed at **file** granularity, so a reported pair may have had entirely unrelated symbols edited in the same commit. Symbol-level coupling would require indexing every historic tree and is a different capability.
[^effq]: `~` for every shipped language, and the limit is the *surface*, not the adapter — the facts are computed for every language in this matrix, and no command exposes them.

    **The facts.** `own_effects` is persisted as its own column and selected by the `v_symbols` view (`crates/cgx-store/src/schema.rs:98`, `:175`). `transitive_effects` is computed at index time by the effect-closure pass — `apply_effects` runs unconditionally on both index entry points, `crates/cgx-index/src/lib.rs:109` and `:142` — but is stored **only inside the node's postcard blob**: no column, no view.

    **The routes, all closed.** No CQL node property: `lower.rs:111-120` rejects `transitive_effects` explicitly. No CLI flag. And **there is no shipped `--sql` route to `v_symbols` either** — the view is real, the way to reach it is not. `--sql` is a hidden flag (`main.rs:510`, `hide = true`) whose own doc comment claims "passing this flag always exits 2." It does not:

    - `cgx query '…' --sql` → **exit 2**, `cgx: the --sql recursive-CTE interface is not implemented in this release` (`main.rs:1149-1154`, which rejects before doing any work).
    - `cgx callers <sym> --sql` → **exit 0**, ordinary output, the flag silently ignored. `args.sql` is read at exactly one site in the whole CLI, so every other subcommand sharing the flag accepts and discards it.

    Both verified by running. That asymmetry is the `--sql` defect in its loud form: the same flag is a hard usage error on one subcommand and a no-op on the rest, and the flag's own help text describes neither behaviour correctly. Reading `v_symbols` today means opening `.cgx/`'s SQLite file directly, outside `cgx`.

A query works for a language iff the facts it needs exist (Section 1). Update both sections together when an adapter changes.

## Section 3 — Answer-envelope coverage

Cross-cutting and language-independent: which surfaces attach the approximation contract (A3/A4) and the index-freshness envelope. Neither is universal, and the pairing differs per command — verified at each render site in `crates/cgx-cli/src/output.rs` and `crates/cgx-mcp/src/tools.rs`.

| Surface | Approximation contract | Freshness envelope | Why |
|---|---|---|---|
| `callers`, `callees`, `reaches`, `paths`, `unused`, `query` | ✓ | ✓ | Traversal answers; both questions apply. |
| `explain`, `search`, `symbols` | — | ✓ | They report facts about symbols rather than the result of a walk, so there is no approximation direction to state. |
| `coupling` | ✓ | — | It never opens the index, so an index-freshness envelope would describe a graph the answer never consulted. Its contract uses a history-specific modeled boundary, not the call-graph one. |
| `diff`, `doctor`, `index` | — | — | Not yet wired to either. |
| `--format dot` / `mermaid` / `d2` | — | — | Raw graph source; the emitters carry no prose lines, and freshness is deliberately not even computed for them. |
| MCP errors | — | — | An error is not an answer. |

Over MCP, `matches_head` is **three-valued**, and `null` is the ordinary case rather than an edge case: with the default `include_dirty: true`, any indexed repository reaches the indeterminate state, and the verdict renders as `unknown`. Treating it as a boolean is wrong in the common case. See [09 — Architecture](09-architecture.md) AR-13.
