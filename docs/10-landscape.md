# Code Intelligence Landscape

**Status:** Draft  
**Audience:** Software engineers, security engineers, engineering leadership  
**Working name:** `cgx` (placeholder; see `README.md`)  
**Cross-references:** `docs/01-vision-and-principles.md` (vision, non-goals), `docs/03-code-graph-model.md` (GM- features), `docs/05-queries.md` (Q- query capabilities), `docs/07-interfaces.md` (IF- interface features), `docs/09-architecture.md` (architecture decisions)

---

## Overview

This document surveys the best-in-class code intelligence and call-graph tools as of 2026, and maps their capabilities against the ten gaps that `cgx` addresses. Tools are grouped by tier: rich graph stores, symbol/reference indexes, syntactic pattern matchers, and the 2025–2026 MCP wave. Per-tool paragraphs cover what each models, its query interface, incrementality approach, precision, and deployment model.

---

## Comparison Table

| Tool | Graph model | Query interface | Incrementality | Precision | Exception-path edges | Branch-diff queries | Rust CLI + STDIO MCP | Deployment |
|------|------------|-----------------|---------------|-----------|---------------------|--------------------|--------------------|------------|
| CodeQL | AST+CFG+DFG+CG | QL (Datalog-like) | Snapshot/partial | Semantic, compiler-derived | No | No | No | JVM batch |
| Joern | CPG (AST+CFG+PDG+CG) | Scala REPL / REST | Batch-first | Semantic, build-free | CFG exception nodes only | No | No | JVM daemon |
| Meta Infer (RacerD) | Compositional method summaries | None (reports only) | Per-run | Semantic; Java/C/C#/ObjC | No | No | No | CLI batch |
| Meta Infer (Pulse) | Interprocedural memory/resource | None (reports only) | Per-run | Semantic; Java/C/C++/ObjC/Hack | No | No | No | CLI batch |
| govulncheck | Call graph (VTA) over Go modules | CLI | Per-run | Semantic; Go only | No | No | No | CLI batch |
| Sourcegraph/SCIP | Defs+refs (no transitive CG) | GraphQL/search | Incremental uploads | Compiler-derived | No | No | No | Server daemon |
| Meta Glean | Fact DB (calls, xrefs, types) | Angle (Datalog) | Fact-append | Compiler-derived | No | No | No | Haskell daemon |
| Google Kythe | Graph (VNames + typed edges) | gRPC API (hop-by-hop) | Fact-append | Compiler-derived | No | No | No | Server daemon |
| Semgrep | AST patterns + Pro taint | YAML rules | Diff-aware scan | Syntactic/semantic | No | No | No | CLI batch |
| rust-analyzer | Type-checked semantic index | LSP only | Salsa demand-driven | Full Rust types | No (panic not modeled) | No | No | LSP daemon |
| stack-graphs (archived) | Scope/name-binding graph | None (GitHub UI) | File-incremental | Name resolution only | No | No | No | GitHub server |
| tree-sitter | Concrete syntax tree | S-expr patterns | Byte-level re-parse | Syntactic only | No | No | No | Library |
| ast-grep | AST structural patterns | CLI / YAML rules | Per-run scan | Syntactic only | No | No | No | CLI batch |
| SciTools Understand | Call+dependency graph | Python/Perl API | Selective rebuild | Semantic (own parsers) | No | No | No | Desktop/server |
| Rupta (academic) | MIR-based CG + pointer analysis | None | None | Highest Rust CG precision | No | No | No | Research tool |
| Serena MCP | LSP bridge (live round-trips) | MCP STDIO | None (live LSP) | LSP-dependent | No | No | Partial (no CG) | STDIO MCP |
| CodeGraph variants | Syntactic CG (tree-sitter) | MCP STDIO / CLI | Re-index on change | Syntactic only | No | No | Partial (Rust/syntactic) | CLI+MCP |
| SocratiCode | Semantic search + dep graph | MCP STDIO | Per-branch collection | Syntactic+embeddings | No | No (collection diff only) | No | VSCode+MCP |
| CIE | Structural CG (Tree-sitter + CozoDB) | MCP STDIO (25+ tools) | None (re-index on change) | Syntactic only | No | No | No (Go/Py/JS/TS only) | STDIO MCP |
| Axon | CG + git coupling history | MCP STDIO / CLI | Incremental (claimed) | Unclear (likely syntactic) | No | No | No | CLI+MCP |
| **cgx** | **Call graph + data flow + provenance** | **Subcommands + Cypher-subset + MCP STDIO** | **Blob-OID content-addressed** | **Syntactic default; SCIP upgrade** | **Yes — first-class edge condition labels** | **Yes — graph-level diff queries** | **Yes** | **Rust CLI, no daemon** |

---

## Tier 1: Rich Graph Stores

These tools model AST, CFG, DFG, and call graph together and support transitive queries.

### CodeQL

**What it models.** Code property graph with AST, CFG, DFG, and call graph. Models data flow (taint tracking), control flow, type hierarchies, and call edges including static and virtual dispatch. Context-sensitive call graph using CHA, RTA, and SPARK-like analysis for Java.

**Query interface.** QL — a declarative Datalog-like language. Queries run via CodeQL CLI or GitHub Actions. Well-suited for interprocedural path queries and taint flows. The barrier to entry is high: QL requires understanding its type system, predicates, library imports, and the concept of a CodeQL database.

**Incrementality.** Snapshot-based. Each analysis requires a CodeQL database built from the source. Partial incremental re-extraction exists for changed files, but schema changes require a full rebuild. No streaming incremental model.

**Precision.** Industry gold standard for security analysis. Compiler-derived, interprocedural data flow. SARIF output built-in. Wide language support.

**Deployment.** JVM-based batch tool. Not embeddable. GitHub-owned, proprietary. Rust support is experimental and community-maintained as of 2026.

**Dominance and ∀-path.** `Dominance.qll` provides `dominates`, `strictlyDominates`, `postDominates`, and basic-block variants. Scope is **intra-procedural only**: dominance is computed per-function CFG, not across call-graph boundaries. There is no built-in predicate asserting "every call-graph path from any entrypoint to sink S passes through node N." `BarrierGuard` (and the 2026 `barrierModel` extensible predicate) blocks taint propagation when a conditional guard holds in a CFG branch — this is ∃-path-negation at the CFG level, not a ∀-path guarantee across the interprocedural call graph. Expressing "all paths from any entry to sink S are blocked" requires hand-rolling the complement as a data-flow configuration; it is achievable but not a first-class primitive.

**Flow states and sanitizer classes.** `DataFlow::StateConfigSig` / `TaintTracking::GlobalWithState` support per-query flow state labels: sources emit a label, `isBarrier(node, state)` clears a named label, `isSink(node, state)` fires for a named label. This is functionally equivalent to class-matched sanitizer/sink pairs but is defined **per query** — there is no shared cross-query sanitizer-class schema. Each query redefines its own flow-state class; standard library queries rarely use this advanced mechanism. Models-as-data `barrierModel` and `barrierGuardModel` allow external YAML/CSV barrier declarations per query category, not a global registry.

**Gaps relative to cgx.** No exception-path edge classification in the call graph. No branch-diff graph queries (queries "what edges exist in branch A not B"). No worktree awareness. Not embeddable as a Rust library. No STDIO MCP interface. No inter-procedural ∀-path dominance primitive. No global sanitizer-class registry across queries.

---

### Joern (Code Property Graph)

**What it models.** Code Property Graph (CPG) — a unified representation of AST, CFG, PDG (program dependence graph), call graph, and type graph. The CPG 1.1 spec includes `THROW`, `TRY`, and `CATCH` control-structure types. However, CFG edges in the CPG carry **no property distinguishing exceptional from normal control flow**. The `CALL`→`METHOD` call edges are similarly unannotated. Joern has exception nodes in the CFG but not exception-labeled call edges.

**Query interface.** Joern REPL (Scala-based) using Gremlin-style graph traversals over the CPG. Also exposes a REST API and has `cpgql` Python bindings. Supports export to Neo4j. The traversal DSL is expressive but requires Scala familiarity and a long-lived REPL session.

**Incrementality.** Some overlay support for incremental re-analysis of changed files, but fundamentally batch-first. CPG stored in embedded TinkerGraph or Neo4j.

**Precision.** Semantic, interprocedural. Context-insensitive by default; context-sensitive mode available. Does not require a project build (unlike CodeQL) — a significant practical advantage for Rust projects with complex build setups.

**Deployment.** JVM daemon. Scala dependency. High startup latency. Open source; widely used by security tools as a foundation.

**Dominance and ∀-path.** Joern CPGQL provides `dominates`, `dominatedBy`, `postDominates`, `postDominatedBy`, `controls`, and `controlledBy` traversal steps (exact names from docs.joern.io/cpgql/control-flow-steps). These are per-method dominator trees computed from each function's CFG — **intra-procedural only**. There are no `passes` or `passesNot` steps in Joern CPGQL; any documentation that cites them is incorrect. Interprocedural analysis in Joern is data-flow only (symbol tracking across call boundaries), not dominance. Like CodeQL, there is no call-graph-level ∀-path primitive.

**Gaps relative to cgx.** No exception-path classification at the call-edge level (confirmed from CPG 1.1 spec). No branch-diff graph queries. No git worktree awareness. No STDIO MCP interface. JVM overhead rules out fast CLI startup. No inter-procedural ∀-path dominance primitive.

---

### Meta Glean

**What it models.** Fact database with arbitrary facts about code: symbols, definitions, references, inheritance, calls, types, and cross-references. Schema-driven (Thrift-like). Call graph is a first-class queryable relation. Ships with indexers for C++, Hack, Python, Java, Erlang, Haskell, JavaScript.

**Query interface.** Angle — a pattern-matching Datalog query language. Concise for graph queries. `glean` CLI with JSON output. The Angle language is readable; the deployment is complex.

**Incrementality.** Fact-append: facts are content-addressed; only changed-file facts are re-derived. Incremental ingestion scales to large codebases.

**Precision.** Depends on the indexer. C++/Hack indexers use compiler output — high precision. Proven at Meta scale.

**Deployment.** Haskell server daemon. Thrift RPC. Significant infrastructure to deploy. Open-source but heavy.

**Gaps relative to cgx.** No exception-path classification. No worktree awareness. No branch-diff queries. Not Rust-native. No MCP interface. Complex deployment rules out CLI use.

---

### Google Kythe

**What it models.** Kythe graph — nodes (VNames: facts about code entities) and typed edges (ref, defines, childof, typed, calls). Call graph is first-class via the `calls` edge. Stored in LevelDB or Bigtable. Cross-language and cross-repo.

**Query interface.** KytheService gRPC API (XRefs, CrossReferences, Decorations). A `kythe` CLI tool exists but is awkward. There is no SQL or Datalog query language: navigation is API-driven, hop-by-hop only.

**Incrementality.** Incremental fact ingestion. Changed files re-emit facts; old facts are versioned out.

**Precision.** Compiler-derived (javac plugin for Java, Clang AST for C++). High precision, type-aware. Proven at Google scale.

**Deployment.** Server daemon (kythe_serving). Extraction is batch with compiler integration per language.

**Gaps relative to cgx.** No exception-path edges. No branch-diff queries. No worktree awareness. No query language (hop-by-hop API only). Not embeddable in a Rust CLI.

---

### Meta Infer (RacerD and Pulse)

Meta Infer is a compositional static analysis framework with multiple checkers. Two checkers are relevant to the security/bug-hunting question bank: RacerD (data races) and Pulse (interprocedural memory and resource safety).

**RacerD.**
RacerD detects unsynchronized concurrent accesses to class member variables using compositional method summaries — each method is analyzed independently of its call context. It reports when "two concurrent accesses to a class member variable are not separated by mutual exclusion, and at least one is a write."

The critical limitation is the **boolean lock abstraction**: RacerD tracks whether *some* lock is held at an access, not *which* lock. From official docs (fbinfer.com/docs/checker-racerd/): it "misses races where two accesses are mistakenly protected by different locks." The classic inconsistent-lock-set pattern — field F guarded by lock A in thread 1 and lock B in thread 2 — is precisely what RacerD cannot detect. Languages: Java, C/C++/ObjC, C#.

**Pulse.**
Pulse is Infer's interprocedural memory safety checker (successor to biabduction). It detects null dereferences, memory leaks, use-after-free, and resource leaks interprocedurally. It reasons about all paths to function exit, including exceptional ones, and handles known API pairs (OS file handles, `malloc`/`free`) for well-known resource types. Supports taint flow as well.

The gap relative to cgx's resource lifecycle design (GM-13): Pulse handles known built-in API pairs but does not expose a **user-declarable acquire/release pair** specification. There is no query-language interface for declaring custom pairs — Pulse is an analysis that emits reports, not a composable query primitive. Languages: Java, C/C++/ObjC, Hack, Rust (experimental), Erlang (experimental).

**Gaps relative to cgx.** RacerD's boolean lock abstraction cannot answer Q97 (inconsistent lock sets). Pulse's fixed built-in pairs cannot answer Q90 (user-declared resource lifecycle pairing). Neither tool provides a query language. No Rust support at production tier (Pulse Rust is experimental). No STDIO MCP interface.

---

### govulncheck

Official Go vulnerability scanner from the Go team. Loads application packages, identifies entrypoints (`main.main`, test functions), builds a call graph, and reports vulnerabilities only when the affected symbol is transitively callable from an entrypoint.

**Algorithm.** Uses **VTA (Variable Type Analysis)** from `golang.org/x/tools/go/callgraph/vta`. VTA builds a global type propagation graph to resolve virtual dispatch. Fallback chain on timeout: VTA → RTA → CHA. Granularity: function-level. Reports which vulnerable function is reachable and shows the call stack.

**Limitations (from official docs).** Conservative on function pointers and interface calls (may produce false positives). Calls via `reflect` are not visible — false negatives. `unsafe` use may produce false negatives. Binary mode cannot build a call graph and may report unreachable-but-compiled functions.

**Gap relative to cgx.** govulncheck answers "is the vulnerable function reachable?" (∃-path, function-level). It does not answer: what data reaches the vulnerable function (no taint), was a trust boundary crossed, was an authorization check on the path. It is Go-only. The cgx CVE-reachability feature (Q-25, Q3, Q10) adds data-flow context, trust-boundary attribution, and confidence tiers to this picture — and covers languages beyond Go.

---

## Tier 2: Symbol / Reference Indexes

These tools model definitions and references but do not provide a first-class transitive call graph.

### Sourcegraph / SCIP

**What it models.** Symbol index encoding precise definitions, references, documentation, and implementation relationships. SCIP (Sourcegraph Code Intelligence Protocol) is an open binary format (protobuf) for this index. Call graph is implicit through reference chains — it is not a first-class object, and transitive reachability queries are not supported.

**Query interface.** GraphQL API, Sourcegraph search syntax (structural, regex, diff). No native graph query language. Call graph navigation is UI-driven or API-driven hop-by-hop.

**Incrementality.** Incremental SCIP uploads per commit. The backend indexes only changed files since the previous commit.

**Precision.** Compiler-derived: indexers (scip-go, scip-java, scip-typescript, scip-python, rust-analyzer SCIP output, scip-clang) use real typecheckers. High precision for defs/refs.

**Deployment.** Server daemon (Sourcegraph instance). Indexers run as CI jobs.

**Gaps relative to cgx.** No transitive call graph queries. No exception-path awareness. No value provenance. No dead-code analysis. No branch-diff graph queries. Heavyweight server dependency.

**Note on SCIP as input format.** `cgx` can consume SCIP indexes as a resolution enrichment layer to upgrade edge confidence from `possible` to `certain`/`probable`. This is an input to `cgx`, not a query interface. See `docs/08-language-support.md`.

---

### rust-analyzer

**What it models.** Type-checked semantic index for Rust. Provides call hierarchy, find references, inlay hints. Salsa-based incremental computation. Models HIR (high-level IR) and MIR for type analysis. Highest-precision Rust symbol resolution available.

**Query interface.** LSP server only. Internal Salsa query system. No graph query language, no scripting interface beyond LSP protocol.

**Incrementality.** Salsa demand-driven — only recomputes affected queries on change. Sub-millisecond response for most queries after initial index build.

**Precision.** Full Rust type resolution, trait dispatch, macro expansion. Handles `dyn Trait` via the trait implementation hierarchy.

**Deployment.** LSP daemon. Not easily deployed as a standalone call-graph extractor or query engine.

**Gaps relative to cgx.** Not a standalone call-graph extractor. No cross-function data flow graph. Panic paths not modeled. No dead-code-from-instance analysis. LSP-only interface — not queryable as a graph store. No MCP interface.

---

### GitHub stack-graphs (archived prior art)

**What it models.** A stack graph (pushdown automaton over symbol table scopes) for name binding. Models scoping rules and name resolution for "find definition" / "find references." Call graph is implicit through resolved references, not general.

**Query interface.** None — embedded in GitHub's code navigation UI and API only.

**Incrementality.** Designed for incremental file-level recomputation: each file produces an independent partial subgraph; graphs merge by construction; changing one file only invalidates that file's subgraph.

**Precision.** Context-free language reachability for name resolution. Does not resolve dynamic dispatch or type inference. No data flow, no exception paths.

**Status.** The `stack-graphs` Rust crate was **archived by GitHub on September 9, 2025** and is now read-only. The last crates.io update was December 13, 2024. No new language definitions or bug fixes will be released upstream. The crate should not be taken on as a live dependency.

**Instructive prior art.** Despite its archived status, the stack-graphs architecture remains instructive. Its core insight — each file produces an independent partial subgraph, keyed by content hash; cross-file resolution is path-finding across subgraph boundaries at query time — directly informs `cgx`'s blob-OID content-addressed index design (see `docs/06-indexing-and-vcs.md`). The `tree-sitter-graph` DSL (which stack-graphs used to author scope graph construction rules) is still actively maintained by the tree-sitter organization and is the extraction substrate chosen for `cgx`'s architecture (see `docs/09-architecture.md`).

---

## Tier 3: Pattern Matchers (No Persistent Graph)

These tools match patterns in source code but do not build or persist a graph.

### Semgrep

**What it models.** Pattern-based AST matching (OSS). The Pro engine adds interprocedural taint/dataflow analysis and call-graph-aware matching, tracking sources→sinks across call boundaries.

**Query interface.** YAML rule files with `pattern`, `metavariable`, `taint`, and `focus-metavariable` directives. `semgrep` CLI. Best learnability of any tool surveyed (YAML, code-like patterns).

**Incrementality.** Diff-aware scanning. Caches prior results per run. Not a persistent graph — re-scans changed files each run.

**Precision.** OSS: syntactic (fast, some false positives). Pro: semantic interprocedural dataflow. Competitive with CodeQL for taint analysis. Good Rust support via OSS patterns.

**Taint labels (experimental).** Sources can carry a `label` key; sinks can declare a `requires` boolean expression over labels; propagators can `replace-labels`. This comes closest to class-matched sanitizer/sink pairs: a rule can declare `sink.requires: SQL_TAINT` and a sanitizer that clears `SQL_TAINT` but not `HTML_TAINT`. However, the feature is marked **experimental** in Semgrep docs. Labels are per-rule with no shared cross-rule schema — consistent label names across rules are a convention, not a toolchain enforcement. Like CodeQL's BarrierGuard, the sanitizer model is ∃-path-negation: it reports when a tainted path exists without a matching sanitizer, not when all paths are covered by one.

**Gaps relative to cgx.** Not a graph store — ephemeral per run. No exception-path tracking. No dead-code analysis. No worktree awareness. No STDIO MCP interface. No branch-diff graph queries. No stable cross-query sanitizer-class registry. No ∀-path guarantee for sanitizer coverage.

---

### tree-sitter

**What it models.** Concrete syntax tree (CST) with error recovery. Incremental, byte-level re-parsing. No semantics — purely syntactic.

**Query interface.** S-expression pattern queries (tree-sitter queries). Used programmatically via C/Rust/Node/Python/Wasm bindings.

**Precision.** Syntactic only. No type info, no scope resolution, no call resolution.

**Deployment.** Library (embedded). No daemon. Used as a foundational parsing layer by Neovim, Helix, VS Code, GitHub, stack-graphs, and ast-grep.

**Role in cgx.** `tree-sitter` is the parsing substrate for `cgx`. It is not a comparison tool for end users but the library underlying `cgx`'s syntactic extraction tier. See `docs/09-architecture.md`.

---

### ast-grep

**What it models.** AST-level structural patterns. Similar to Semgrep but Rust-native and faster. Supports rewriting and linting via YAML rules. Based on tree-sitter grammars.

**Query interface.** CLI with YAML rules or inline patterns (`sg run --pattern '...'`). Designed for progressive use: one-liner first, YAML rule files for complex cases. Excellent CLI ergonomics.

**Precision.** Syntactic/structural. No type resolution, no interprocedural analysis.

**Gaps relative to cgx.** No semantic analysis, no call graph, no data flow, no exception paths. Per-run scan, no persistence.

---

### SciTools Understand

**What it models.** Code metrics plus dependency and call graph. Tracks calls, data uses, inheritance, includes/imports, and complexity metrics. Graph stored in a proprietary database.

**Query interface.** Understand API (Python/Perl/C), Understand Perl scripts, visual graph UI, some Lua scripting.

**Precision.** Semantic for most supported languages. Own parsers with some compiler integration.

**Deployment.** Desktop/server GUI application with batch API. Mature (20+ years). Comprehensive for legacy languages (Ada, COBOL, FORTRAN).

**Gaps relative to cgx.** No Rust support. Proprietary and expensive. No exception-path classification. No branch-diff queries. No worktree awareness. Not embeddable. No MCP interface.

---

### Rupta (academic)

**What it models.** Context-sensitive call graph for Rust via MIR (Mid-level IR) and pointer analysis. The highest-precision Rust call graph available in open-source, using callsite-based context-sensitivity. Published at ACM CC'24.

**Query interface.** None — research tool with no query interface, no CLI for end users.

**Precision.** Highest available for Rust: operates on MIR with on-the-fly call graph construction during pointer analysis. Handles trait objects with precision not achievable from AST analysis.

**Deployment.** Academic research tool. No MCP, no incremental indexing, no branch awareness, not production-ready.

**Role relative to cgx.** Rupta is the academic state of the art for Rust call graph precision. Its MIR-based approach represents the top of the precision ladder that `cgx` aspires toward in future semantic tiers. For v1, `cgx` uses tree-sitter + optional SCIP enrichment and targets the `certain`/`probable`/`possible` confidence tiers rather than requiring MIR analysis.

---

## Tier 4: 2025–2026 MCP Wave

A wave of MCP-native code intelligence tools emerged in 2025–2026. All use tree-sitter or similar syntactic analysis. None provides a persistent semantic call graph with exception-path labels or branch-diff queries.

### Serena (Oraios)

24,000+ GitHub stars as of mid-2026. STDIO MCP transport. Bridges MCP calls to LSP (language server protocol) round-trips. Tools: `find_symbol`, `find_referencing_symbols`, `get_symbols_overview`, `get_callers` (via LSP call hierarchy).

**Strengths.** Wide language coverage (40+) via LSP backends. Fast symbol lookup (~100ms per call for indexed projects). Compact symbol IDs. Pagination via `context_lines` param.

**Gaps.** No persistent graph index — every call is a live LSP round-trip. No call graph persistence. No exception-path edges. No data flow. No dead-code analysis. No branch-diff queries.

---

### CodeGraph variants (colbymchenry, codegraph-ai, suatkocar)

Several distinct projects use the `codegraph` name. All share characteristics: tree-sitter based parsing, SQLite or graph DB storage, callers/callees as first-class tools, STDIO MCP interface.

- **colbymchenry/CodeGraph** — 19+ languages, SQLite, callers/callees queryable, v0.9.5.
- **codegraph-ai/CodeGraph** — 42 MCP tools, 38 languages, SQLite + full-text search.
- **suatkocar/codegraph** — Rust-native, 44 MCP tools, "sub-second indexing."

**Strengths.** suatkocar variant is the closest to `cgx`'s implementation approach: Rust CLI + MCP. Sub-second indexing claim (syntactic — no type resolution required).

**Gaps (all variants).** Syntactic precision only — no type resolution, no virtual dispatch resolution, no trait implementation lookup. No exception-path edge labels. No branch-diff queries. No git worktree awareness. No incremental indexing (re-indexes on change).

---

### CodeGraphContext

MCP + CLI with real graph database backends (FalkorDB, KuzuDB, Neo4j). 22 languages. Supports callers, callees, class hierarchies, and call chains. Live file watching for incremental updates.

**Gaps.** Vector embeddings + inheritance-aware call edge resolution are on the roadmap but not shipped. 135 open issues as of mid-2026. Syntactic precision in the shipped version.

---

### SocratiCode

VSCode extension + MCP. Branch-aware indexing: appends the git branch name to the project ID, creating a separate Qdrant vector collection per branch. Hybrid semantic search (embeddings + BM25/RRF). AST-aware chunking.

**Branch "awareness" qualification.** SocratiCode's branch awareness is collection-level separation, not graph-diff queries. The tool stores a separate embedding collection per branch but cannot answer "what call edges exist in branch A but not branch B." Its own documentation notes that git worktrees all map to the same project configuration as a workaround — not true worktree-native indexing.

**Gaps.** Not a true call graph — it is a dependency graph plus semantic search. No exception-path edges. No branch-diff graph queries. No worktree-aware shared-object-store indexing.

---

### CIE (Code Intelligence Engine)

MCP STDIO server with 25+ tools. Tree-sitter based parsing; CozoDB (embedded graph/relational hybrid) as the storage backend. Supports Go, Python, JavaScript, and TypeScript. Benchmarked 34 tool calls for a call-chain lookup via grep-based workflows reduced to 3 tool calls via CIE — the source of the 34→3 benchmark cited in doc 02 (Q79) and in `docs/02-personas-and-questions.md` (ACA persona).

**Strengths.** Deep MCP tooling surface (25+ tools). Efficient structured output for AI agent workflows. CozoDB provides native graph query support. Used as the efficiency baseline for the ACA persona question set.

**Gaps.** No Rust support. No exception-path edge labels. No branch-diff graph queries. No worktree-native indexing. Syntactic precision only — no type resolution or virtual dispatch narrowing. No taint analysis with provenance.

---

### Axon

Graph-powered impact analysis tool with a 12-phase pipeline. Incremental indexing. Combines call graph traversal with git coupling history (co-change frequency as edge weights). MCP + CLI. Exposed via `axon_impact` and related tools.

**Notable.** Git coupling history as an edge weight is a distinct capability: it surfaces which functions change together historically, complementing static call edges.

**Gaps.** Precision level is undocumented and likely syntactic. No exception-path classification. No worktree-native awareness. No branch-diff graph queries.

---

## Gaps We Fill

The gaps below are confirmed across all surveyed tools. Each represents a question class that no existing tool answers for a Rust codebase via a fast CLI and STDIO MCP interface.

### Gap 1: Exception-path call edge classification

**Evidence.** Joern has CFG exception nodes (`TRY`, `THROW`, `CATCH`), but CFG edges carry no property distinguishing exceptional from normal control flow, and `CALL`→`METHOD` edges in the CPG are unannotated. No other tool annotates call graph edges as exception-conditioned. OWASP Top 10 2025 includes A10: Mishandling of Exceptional Conditions, validating the security relevance of this gap.

**What cgx provides.** Every call edge carries an `edge condition` label (`always`, `conditional`, `exception`, `loop`, `panic`) as a first-class property. The labels `exception` and `panic` form the exceptional class; queries can filter on this to find non-exceptional paths, exception-only paths, or paths where a function is exception-transient relative to a given route. See `docs/03-code-graph-model.md` (GM-3) and `docs/05-queries.md` (Q-3, Q-11, Q-12).

---

### Gap 2: Branch-diff graph queries

**Evidence.** No tool supports queries of the form "what call edges exist in branch A but not branch B" or "which symbols became newly reachable after this PR." Tools do diff-aware scanning (Semgrep, CodeQL in CI), but not graph-level diff queries over the semantic graph. SocratiCode separates branches into collections but cannot diff them as graphs.

**What cgx provides.** The `cgx diff` subcommand and the `--at <ref>` flag on all query commands enable: "show new edges between main and feature/foo," "which paths to `vulnerable::bar` appeared in the exception path since last release," and "which functions became unreachable after merging." See `docs/05-queries.md` (Q-7, Q-16) and `docs/06-indexing-and-vcs.md`.

---

### Gap 3: Git worktree awareness

**Evidence.** No tool is aware of git worktrees as distinct indexing contexts sharing a git object store. All tools treat each checkout as an independent codebase. SocratiCode's docs acknowledge worktrees map to the same project configuration as a workaround.

**What cgx provides.** Index shards are keyed by blob OID (git content hash). Two worktrees on different branches share all shard data for files with identical content — indexing cost is proportional to the diff between branches, not to the total codebase size per worktree. See `docs/06-indexing-and-vcs.md`.

---

### Gap 4: Value provenance / pedigree

**Evidence.** CodeQL path queries come closest to general provenance but require security-specific rule framing. No tool provides first-class general provenance queries: "where did the value at this variable site come from, transitively, through transformations including `map`/`filter`/collection operations?"

**What cgx provides.** The `pedigree` subcommand (Q-5) and `DATA_FLOW` edges in the query language trace value origins through function boundaries, with transformation tags (`identity`, `mapped`, `aggregated`, `parsed`) on each step. See `docs/04-dataflow-and-provenance.md` (DF-) for the full data flow model.

---

### Gap 5: Dead code from a specific entrypoint (instance-level)

**Evidence.** `rustc` reports `dead_code` lint warnings for private items with no callers, but does not scope this analysis to a specific entrypoint set or support "dead from this instance." The Knight Capital incident ($440M loss from dormant reactivated code) illustrates the security significance of dead code analysis. No tool does instance-specific dead code: "given this binary is built with these entrypoints, which functions are unreachable?"

**What cgx provides.** The `unused` subcommand (Q-4) scopes dead-code analysis to a declared entrypoint set. `--entrypoint-class http` restricts to HTTP handler roots; `--entrypoint main::start` restricts to a specific root. Results include confidence tiers: `certainly-unused` requires that the item has no incoming edges from any entrypoint-reachable symbol at `certain` or `probable` confidence.

---

### Gap 6: Inter-procedural ∀-path / must-pass-through

**Evidence.** CodeQL `Dominance.qll` and Joern `dominates`/`postDominates` steps both operate intra-procedurally: they compute dominance within a single function's CFG only. Neither provides a call-graph-level assertion that every path from any entrypoint to a sink passes through a given check node. `BarrierGuard` (CodeQL) and sanitizer blocks (Semgrep) both express ∃-path-negation — "if any path from source to sink has no matching guard, report it" — not the ∀-positive complement. Expressing "all paths are guarded" requires hand-rolling the complement; it is not a first-class query primitive in any of the surveyed tools.

**What cgx provides.** Q-20 (Path quantifiers and must-pass-through) surfaces `--must-pass-through <symbol>` as a subcommand flag and a query-language predicate. The inter-procedural dominance check uses the call graph to assert that a given node is on every path from the specified source to the specified sink, not just within a single function's CFG. This directly answers the authorization-bypass question class (Q87, Q88): "no path from handler to protected resource avoids the auth check."

---

### Gap 7: Inter-procedural lock-set and await-holding-lock analysis

**Evidence.** The strongest available tool for lock-set analysis is Infer RacerD, which uses a boolean lock abstraction — it tracks whether *some* lock is held, not which lock. From official docs: it "misses races where two accesses are mistakenly protected by different locks." Clippy `await_holding_lock` detects mutex guards live at an `await` point within a single async function but does not cross async call boundaries. Rust lockbud analyzes deadlock and lock-order — not inconsistent lock sets for shared fields. No tool answers Q97 (fields accessed under inconsistent lock sets across spawn contexts) as a static query.

**What cgx provides.** GM-11 (Synchronization context and lock sets) attributes which lock identities are held at each call site. GM-9 (Spawn edges) identifies spawn-distinct execution contexts. Q-24 (Concurrency queries) composes these: "field F is written from context X under lock A and from context Y under lock B — are A and B the same?" Clippy `await_holding_lock` covers the intra-function case; cgx extends the same analysis across async call boundaries using GM-10 (Suspension points) and GM-11.

---

### Gap 8: User-declarable resource lifecycle pairs as composable query primitives

**Evidence.** Infer Pulse handles known built-in resource pairs (OS file handles, `malloc`/`free`) interprocedurally but does not expose a user-declarable acquire/release pair specification. CodeQL resource-leak queries are fixed-pattern, intra-method, and limited to known Java resource types. Rust RAII handles scoped resources at the compiler level but misses `mem::forget`, `ManuallyDrop`, `Rc` cycles, and resources inside detached `tokio` tasks. No tool expresses "user-declared pair (acquire, release): is release reached on all exception-class paths from acquire?" as a composable query primitive.

**What cgx provides.** GM-13 (Resource lifecycle pairs) stores user-declared `(acquire, release)` pairs with built-in per-language defaults and user config. Q-22 (Ordering and pairing predicates) provides the "A-then-B on all paths" primitive. Combined with `edge-condition-filter`, this answers Q90 (leak-on-error across all exception-class edges, including task-cancellation paths) and Q103 (exactly one of commit/rollback on every path).

---

### Gap 9: Global sanitizer-class schema for class-matched taint clearing

**Evidence.** CodeQL `DataFlow::StateConfigSig` allows per-query flow state labels that functionally implement class-matched sanitizer/sink pairs, but the schema is defined per query — there is no global registry that says "sanitizer of class `sql` clears taint only at sinks of class `sql`" across the standard library. Semgrep taint labels (experimental) come closest with per-rule `label`/`requires` keys, but labels are a per-rule convention with no cross-rule enforcement. No tool provides a global sanitizer-class registry where declaring `sanitizer(class=sql)` automatically governs taint clearing for all queries/rules that use `sink(class=sql)`.

**What cgx provides.** DF-11 (Typed taint labels and class-matched sanitization) is a genuine novelty: the built-in sink classes (`sql`, `shell`, `path`, `html`, `header`, `redirect-url`, `format-string`, `regex`, `deserialize`, `eval`, `log`, `net-request`) have corresponding sanitizer classes that clear taint only at matching sinks. The class lists are user-extensible via config. This directly answers Q86 (injection without class-matched sanitizer) and Q92 (SSRF/path traversal with class-specific canonicalization), and makes any cross-query taint assertion consistent by construction.

---

### Gap 10: CVE reachability with path, taint, and trust-boundary context

**Evidence.** govulncheck (Go only, VTA, function-level), Snyk Reachability, and Endor Labs all answer "is the vulnerable function reachable from an entrypoint?" (∃-path). None answer: what data reaches the vulnerable function, was the call path through a trust boundary (network-sourced input), or was an authorization check on the path. All perform ∃-path reachability only; none provide data-flow context.

**What cgx provides.** Q-25 (Dependency and CVE reachability queries) extends reachability with: (a) taint source class on the path — did user-controlled data flow to the vulnerable function? (b) trust boundary crossing — did the call path cross a source of class `network`, `deserialization`, etc.? (c) cgx confidence tiers on call edges, surfacing which edges are `certain` vs `possible`. This answers Q3 (CVE'd function reachability with internal chain), Q10 (SBOM CVE structured report), and Q75 (cargo audit × path × sanitizer) in ways govulncheck and SCA tools do not.

---

## Summary: Capability Matrix by Gap

| Gap | Best existing tool | cgx approach |
|-----|-------------------|-------------|
| Exception-path edge classification | Joern (CFG nodes only) | First-class `condition` label on every call edge |
| Branch-diff graph queries | None | `cgx diff` + `--at <ref>` on all queries |
| Git worktree awareness | None | Blob-OID shard sharing across worktrees |
| Value provenance / pedigree | CodeQL (security-specific) | `pedigree` subcommand + `DATA_FLOW` edges with transformation tags |
| Dead code from entrypoint | rustc lint (private only) | `unused` with entrypoint scoping + confidence tiers |
| Rust CLI + persistent graph + STDIO MCP | suatkocar/codegraph (syntactic) | Semantic call graph (SCIP upgrade path) + STDIO MCP |
| Inter-procedural ∀-path (must-pass-through) | CodeQL/Joern (intra-procedural only) | Q-20 call-graph-level must-pass-through predicate |
| Inconsistent lock-set / await-holding-lock | Infer RacerD (boolean lock abstraction; Java/C) | GM-11 lock-set attribution + GM-9 spawn edges + Q-24 |
| User-declarable resource pairs as query primitives | Infer Pulse (built-in pairs only; no query language) | GM-13 declared pairs + Q-22 pairing predicates |
| Global sanitizer-class schema | None (per-query CodeQL flow states; experimental Semgrep labels) | DF-11 typed taint labels with built-in + user-extensible class lists |
| CVE reachability with taint and trust-boundary context | govulncheck (∃-path, function-level, Go only) | Q-25 + DF-12 source/sink classes + confidence tiers |
