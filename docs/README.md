# cgx Documentation

> **Naming note:** `cgx` is a placeholder. The final tool name is not yet decided.

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
concurrency safety, and graph diff across commits and branches.

## Reading Order

Start with docs 01 and 02 to understand the product commitments and the question space.
Docs 03–09 cover implementation concerns in any order. Docs 10–11 cover market
positioning and planning.

| # | File | Description |
|---|------|-------------|
| 01 | [01-vision-and-principles.md](01-vision-and-principles.md) | Product vision; hard constraints (single-binary CLI, no daemon, no LLM in answer path, Rust, STDIO MCP); design principles (file:line evidence on every answer, labeled confidence, soundiness honesty, stable output); non-goals (no TUI in v1, not a SAST scanner, no dynamic analysis) |
| 02 | [02-personas-and-questions.md](02-personas-and-questions.md) | Persona-driven question inventory: 108 questions across 10 themes, 89 NOVEL; four personas (PSE, SSE, ACA, ASA); all four canonical prompt examples; Theme 10 covers concurrency and resource safety |
| 03 | [03-code-graph-model.md](03-code-graph-model.md) | Node/edge taxonomy; edge condition labels (`always`, `conditional`, `exception`, `loop`, `panic`) with exceptional-class definition; path-relative transience semantics; confidence levels (`certain`, `probable`, `possible`); file:line evidence on every fact |
| 04 | [04-dataflow-and-provenance.md](04-dataflow-and-provenance.md) | Value pedigree, def-use chains, scope entry/exit, transformation see-through (map/filter/closures), taint sources/sinks/sanitizers, slicing, instance-level dead-member analysis |
| 05 | [05-queries.md](05-queries.md) | Query capabilities as a taxonomy; subcommands and full query language design; worked examples for every question class including the four canonical prompts |
| 06 | [06-indexing-and-vcs.md](06-indexing-and-vcs.md) | Incremental indexing; blob-OID content addressing (branches and worktrees share unchanged-file shards); branch-aware graph diffs; auto and explicit `cgx index` / `cgx prune`; locking and staleness |
| 07 | [07-interfaces.md](07-interfaces.md) | CLI UX; output formats (text / JSON / JSONL / DOT / Mermaid / SARIF / CSV); exit codes and CI assertion mode; MCP STDIO server tool surface; token-efficiency design for AI agents; `--explain` and `--evidence` flags |
| 08 | [08-language-support.md](08-language-support.md) | Tiered language strategy; per-language semantics (exceptions vs `Result` vs error returns; dynamic dispatch; closures); resolution enrichment via SCIP/LSP |
| 09 | [09-architecture.md](09-architecture.md) | Rust implementation rationale; parse → resolve → graph → store → query pipeline; key crates; storage format and startup budget; determinism guarantees |
| 10 | [10-landscape.md](10-landscape.md) | Comparison with CodeQL, Joern, Infer (RacerD/Pulse), govulncheck, Sourcegraph/SCIP, Glean, Kythe, Semgrep, rust-analyzer, stack-graphs, Rupta, Serena, codegraph variants; 10 gaps `cgx` fills including ∀-path analysis, lock-set queries, resource pairing, sanitizer-class schema, and CVE reachability with taint context |
| 11 | [11-roadmap.md](11-roadmap.md) | Phasing: MVP → semantic precision → dataflow/concurrency/security → branch/VCS features → MCP polish; feature-ID status rollup (GM-9..14, DF-9..16, Q-20..25, IX-9, LS-7); risks including alias analysis cost and lock-set false positives |

## Who Should Read What

**Software engineer:** Start with 01 for product commitments, skim 02 for the SSE
question examples, then read 05 (queries) and 07 (CLI/MCP interfaces) for day-to-day
use.

**Security engineer:** Read 01, then 02 (all PSE questions and themes 1, 3, 4, 6, 9, 10).
Docs 04 (taint) and 07 (SARIF output, CI assertion mode) cover the toolchain
integration path.

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
`provenance`, `entrypoint`. Do not introduce synonyms; update doc 03 if a term is
missing.

## Doc Status

Docs 01 and 02 are complete. Docs 03–11 are being authored in parallel and will
appear in this directory as they are finalized. All 11 docs are planned; none are
cancelled.
