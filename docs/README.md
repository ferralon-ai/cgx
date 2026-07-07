# cgx Documentation

## Product Statement

`cgx` is a static call-graph CLI for Rust (Go fallback) that answers structural and
dataflow questions about source code — for software engineers, security engineers, and
AI coding/security agents. Every answer carries file:line evidence and labeled edge
confidence (`certain`, `probable`, `possible`). The tool runs without a daemon, starts
fast, and exposes the same capability set over a STDIO MCP server so AI agents can
query it with minimal tool calls. The index is fully auto-managed; `cgx index` and
`cgx prune` are available for explicit control. No LLM is in the answer path: answers
are deterministic and reproducible. The core differentiating capabilities are
exception-path edge labeling (every call edge annotated `always`, `conditional`,
`exception`, `loop`, or `panic`), inter-procedural ∀-path / must-pass-through queries,
typed taint labels with class-matched sanitization, spawn-edge and lock-set modeling for
concurrency safety, graph diff across commits and branches, framework-pack modeling of
annotation-driven guards and entrypoints as composable graph facts, closure capture-edge
attributes, parameter-mutation effect summaries, and lineage type reconstruction.

## Reading Order

Start with docs 01 and 02 to understand the product commitments and the question space.
Docs 03–09 cover implementation concerns in any order. Docs 10–12 cover market
positioning, planning, and framework-pack design.

| # | File | Description |
|---|------|-------------|
| 01 | [01-vision-and-principles.md](01-vision-and-principles.md) | Product vision; hard constraints (single-binary CLI, no daemon, no LLM in answer path, Rust, STDIO MCP); design principles (file:line evidence on every answer, labeled confidence, soundiness honesty, stable output); non-goals (no TUI in v1, not a SAST scanner, no dynamic analysis) |
| 02 | [02-personas-and-questions.md](02-personas-and-questions.md) | Persona-driven question inventory: 138 questions across 13 themes, 112 NOVEL; four personas (PSE, SSE, ACA, ASA); all four canonical prompt examples; Theme 10 covers concurrency and resource safety; Theme 11 covers types, mutability, and closures; Theme 12 covers framework semantics and metadata; Theme 13 covers object model and inheritance |
| 03 | [03-code-graph-model.md](03-code-graph-model.md) | Node/edge taxonomy; edge condition labels (`always`, `conditional`, `exception`, `loop`, `panic`) with exceptional-class definition; path-relative transience semantics; confidence levels (`certain`, `probable`, `possible`); file:line evidence on every fact |
| 04 | [04-dataflow-and-provenance.md](04-dataflow-and-provenance.md) | Value pedigree, def-use chains, scope entry/exit, transformation see-through (map/filter/closures), taint sources/sinks/sanitizers, slicing, instance-level dead-member analysis |
| 05 | [05-queries.md](05-queries.md) | Query capabilities as a taxonomy; subcommands and full query language design; worked examples for every question class including the four canonical prompts |
| 06 | [06-indexing-and-vcs.md](06-indexing-and-vcs.md) | Incremental indexing; blob-OID content addressing (branches and worktrees share unchanged-file shards); branch-aware graph diffs; auto and explicit `cgx index` / `cgx prune`; locking and staleness |
| 07 | [07-interfaces.md](07-interfaces.md) | CLI UX; output formats (text / JSON / JSONL / DOT / Mermaid / SARIF / CSV); exit codes and CI assertion mode; MCP STDIO server tool surface; token-efficiency design for AI agents; `--explain` and `--evidence` flags |
| 08 | [08-language-support.md](08-language-support.md) | Tiered language strategy; per-language semantics (exceptions vs `Result` vs error returns; dynamic dispatch; closures); resolution enrichment via SCIP/LSP |
| 09 | [09-architecture.md](09-architecture.md) | Rust implementation rationale; parse → resolve → graph → store → query pipeline; key crates; storage format and startup budget; determinism guarantees |
| 10 | [10-landscape.md](10-landscape.md) | Comparison with CodeQL, Joern, Infer (RacerD/Pulse), govulncheck, Sourcegraph/SCIP, Glean, Kythe, Semgrep, rust-analyzer, stack-graphs, Rupta, Serena, codegraph variants, and specialized analyzers (CodeQL MaD, Go VTA, loop-capture lints, SpotBugs, TypeScript compiler API, Pytype, TamiFlex, DroidRA, CodeQL Reflection.qll, Dagger); 16 gaps `cgx` fills including ∀-path analysis, lock-set queries, resource pairing, sanitizer-class schema, CVE reachability with taint context, type reconstruction as a query, framework-guard graph facts, capture-edge attributes, parameter-mutation summaries, established-by DI provenance, and string-pedigree reflection edges |
| 11 | [11-roadmap.md](11-roadmap.md) | Phasing: MVP → semantic precision → dataflow/concurrency/security → branch/VCS features → MCP polish; feature-ID status rollup (GM-9..20, DF-9..20, Q-20..31, IX-9, LS-7..8, docs/12 FW-1..6); risks including alias analysis cost, lock-set false positives, framework-pack maintenance burden, and type-reconstruction precision |
| 12 | [12-language-primitives-and-frameworks.md](12-language-primitives-and-frameworks.md) | Framework-pack design and semantic-class catalog: seven metadata semantic classes (`entrypoint`, `guard`, `negative-guard`, `interception`, `generated-member`, `keep-alive`, `contract`); per-framework worked mappings (Spring, Flask/Django, ASP.NET, NestJS, tokio/actix); mediated edges with `established-by` provenance; build-configuration variance; reflection packs. Maps language/framework constructs onto the primitives defined in docs/03 (GM-15..19) and docs/04 (DF-20) |
| 13 | [13-glossary.md](13-glossary.md) | Alphabetical dictionary of all terms of art used across the `cgx` documentation: graph vocabulary (node, edge, edge conditions, exceptional class, transience), path concepts (reachability, dominance, must-pass-through), provenance and taint vocabulary (pedigree, source, sink, sanitizer, typed taint, union/join pedigree), confidence ladder, status tags (core-extension/schema-room/roadmap), query-language terms (Layer 1/Layer 2, MATCH, predicate, procedure, COMPATIBLE_WITH, IS EMPTY), cycle-3 vocabulary (metadata fact, seven semantic classes, framework pack, established-by, capture edge, mutability, function value, coerce, lineage type reconstruction), and indexing terms |
| 14 | [14-implementation-status-matrix.md](14-implementation-status-matrix.md) | Feature×language implementation status matrix: adapter capabilities (parse, defs, refs, imports, inheritance, entrypoints, cut hints, effects, dataflow, concurrency, SCIP, framework packs) and query surface (callers, callees, reaches, paths, unused, search, explain, flows-to, effects, diff) by language (Rust · TS/JS · Go · Python · Java · C#) |

| — | [questions/README.md](questions/README.md) | Index of the 13 question-cookbook theme files (138 questions total): file names, themes, question ID ranges, and counts; plus an explanation of the entry template (how to read personas/status/query/breakdown/result sections) and pointers to docs/13 and docs/05 |


## Who Should Read What

**Software engineer:** Start with 01 for product commitments, skim 02 for the SSE
question examples, then read 05 (queries) and 07 (CLI/MCP interfaces) for day-to-day
use.

**Security engineer:** Read 01, then 02 (all PSE questions and themes 1, 3, 4, 6, 9, 10, 11, 12).
Docs 04 (taint, mutability, type reconstruction), 07 (SARIF output, CI assertion mode), and 12 (framework guards and annotation-driven entrypoints) cover the toolchain integration path.

**AI coding agent / agent developer:** Read 02 (ACA questions, Q16, Q20, Q60, Q68–Q81),
then 07 (MCP STDIO server tool surface and token-efficiency design).

**AI security agent / agent developer:** Read 02 (ASA questions, Q10, Q32, Q71–Q75,
Q83), then 07 (SARIF output, structured JSON) and 04 (taint chain semantics).

**Implementer:** Read 01 for constraints, then 03 → 04 → 06 → 09 in order. Docs 05,
07, 08 can be read in parallel once 03–04 are understood.

## Glossary Pointers

Canonical terms are defined in
[03-code-graph-model.md](03-code-graph-model.md). Key terms used across all docs:
`symbol`, `edge condition`, `path-relative transience`, `confidence`, `pedigree`,
`provenance`, `entrypoint`, `capture edge`, `framework pack`, `mediated edge`,
`established-by`, `mutation fan-out`, `type-confidence boundary`. Do not introduce
synonyms; update doc 03 if a term is missing.

## Doc Status

Docs 01 and 02 are complete. Docs 03–12 are being authored in parallel and will
appear in this directory as they are finalized. All 12 docs are planned; none are
cancelled.
