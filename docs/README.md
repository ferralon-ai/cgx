# cgx Documentation

## Product Statement

`cgx` is a static call-graph CLI for Rust, TypeScript/JavaScript, Go, Python, and Java
that answers structural and dataflow questions about source code — for software
engineers, security engineers, and AI coding/security agents. Every answer carries
file:line evidence and labeled edge confidence (`certain`, `probable`, `possible`), and
every traversal answer additionally carries an **approximation contract** stating which
direction it can be wrong and, for a negative answer, the scope it was searched under.
The tool runs without a daemon, starts fast, and exposes a large subset of the same
capability set over a STDIO MCP server so AI agents can query it with minimal tool calls.
The index is auto-managed; `cgx index` gives explicit control. No LLM is in the answer
path: answers are deterministic and byte-reproducible.

**Shipped differentiators**, all runnable today: exception-path edge labeling (every call
edge annotated `always`, `conditional`, `exception`, `loop`, or `panic`); graph diff
across commits and refs, including a structural PR gate on newly-introduced reachability
paths; blob-OID content addressing that lets branches and worktrees share per-file facts;
file-level co-change coupling over an explicit commit range; and the per-answer
approximation contract and index-freshness envelope.

**Designed and not yet built** — described throughout these docs, and labeled where they
appear: inter-procedural ∀-path / must-pass-through queries, typed taint labels with
class-matched sanitization, lock-set modeling for concurrency safety, framework-pack
modeling of annotation-driven guards and entrypoints, closure capture-edge attributes,
parameter-mutation effect summaries, and lineage type reconstruction. See
[14 — Implementation Status Matrix](14-implementation-status-matrix.md) for the
per-capability, per-language ledger, and the status markers in
[10 — Landscape](10-landscape.md) for which competitive gaps are closed today.

## Reading Order

Start with docs 01 and 02 to understand the product commitments and the question space.
Docs 03–09 cover implementation concerns in any order; **09 describes the system as
built**, and is the fastest way to see how far the design and the implementation have
diverged. Docs 10–12 cover market positioning, planning, and framework-pack design. Docs
13–15 are reference: glossary, status matrix, and the forward capability ledger.

**These docs are design-forward.** The tree describes intent as well as shipped surface,
and it is meant to: the design leads the implementation on purpose. What is not
acceptable is a reader being unable to tell which is which — so where a doc names a flag,
subcommand, or query form that does not exist, it says so at the point of the claim. For
what a command actually accepts today, `docs/commands/` and `docs/mcp-tools/` are the
per-surface references. Treat `--help` as a summary rather than a specification: it is
generated from the flag definitions, so it is authoritative about which flags exist and
less reliable about what one of them *means*.

`--depth 0` is the standing example, and it means three different things:

- On **`paths`**, it lifts the depth limit — but the walk is still bounded by a shared
  work budget, and when that budget fires the result is truncated at exit 0 with
  `truncated` set. Lifting the depth bound lets the search spend its budget on breadth, so
  on a large graph `--depth 0` can return **fewer** paths than a bounded `--depth 6`, and
  **zero is a realistic outcome**. An empty result under `--depth 0` does not mean no path
  exists.
- On **`callers` / `callees` / `reaches`**, it is taken literally: depth zero, so you get
  the seed node and nothing else.
- Over **MCP**, `max_depth: 0` on `paths` returns nothing at all.

In every case the answer's own approximation contract reports what happened — `under`,
with a `depth-limit` or truncation reason — which is the field to read rather than the
help text.

| # | File | Description |
|---|------|-------------|
| 01 | [01-vision-and-principles.md](01-vision-and-principles.md) | Product vision; hard constraints (single-binary CLI, no daemon, no LLM in answer path, Rust, STDIO MCP); design principles (file:line evidence on every answer, labeled confidence, soundiness honesty, stable output); non-goals (no TUI in v1, not a SAST scanner, no dynamic analysis) |
| 02 | [02-personas-and-questions.md](02-personas-and-questions.md) | Persona-driven question inventory: 138 questions across 13 themes, 110 NOVEL; four personas (PSE, SSE, ACA, ASA); all four canonical prompt examples; Theme 10 covers concurrency and resource safety; Theme 11 covers types, mutability, and closures; Theme 12 covers framework semantics and metadata; Theme 13 covers object model and inheritance |
| 03 | [03-code-graph-model.md](03-code-graph-model.md) | Node/edge taxonomy; edge condition labels (`always`, `conditional`, `exception`, `loop`, `panic`) with exceptional-class definition; path-relative transience semantics; confidence levels (`certain`, `probable`, `possible`); file:line evidence on every fact |
| 04 | [04-dataflow-and-provenance.md](04-dataflow-and-provenance.md) | Value pedigree, def-use chains, scope entry/exit, transformation see-through (map/filter/closures), taint sources/sinks/sanitizers, slicing, instance-level dead-member analysis |
| 05 | [05-queries.md](05-queries.md) | Query capabilities as a taxonomy; subcommands and full query language design; worked examples for every question class including the four canonical prompts |
| 06 | [06-indexing-and-vcs.md](06-indexing-and-vcs.md) | Incremental indexing; blob-OID content addressing (branches and worktrees share unchanged-file shards); branch-aware graph diffs; auto and explicit `cgx index` / `cgx prune`; locking and staleness |
| 07 | [07-interfaces.md](07-interfaces.md) | CLI UX; output formats (text / JSON / JSONL / DOT / Mermaid / SARIF / CSV); exit codes and CI assertion mode; MCP STDIO server tool surface; token-efficiency design for AI agents; `--explain` and `--evidence` flags |
| 08 | [08-language-support.md](08-language-support.md) | Tiered language strategy; per-language semantics (exceptions vs `Result` vs error returns; dynamic dispatch; closures); resolution enrichment via SCIP/LSP |
| 09 | [09-architecture.md](09-architecture.md) | **The system as built.** Rust rationale (including which of its original reasons did not survive); the crate layering; parse → resolve → graph refinement → store → query pipeline; the five-step resolution ladder and its known false-exact; SQLite storage and the SQL view contract; determinism; and AR-13, the per-answer approximation contract and freshness envelope |
| 10 | [10-landscape.md](10-landscape.md) | Comparison with CodeQL, Joern, Infer (RacerD/Pulse), govulncheck, Sourcegraph/SCIP, Glean, Kythe, Semgrep, rust-analyzer, stack-graphs, Rupta, Serena, codegraph variants, and specialized analyzers (CodeQL MaD, Go VTA, loop-capture lints, SpotBugs, TypeScript compiler API, Pytype, TamiFlex, DroidRA, CodeQL Reflection.qll, Dagger); 16 gaps `cgx` fills including ∀-path analysis, lock-set queries, resource pairing, sanitizer-class schema, CVE reachability with taint context, type reconstruction as a query, framework-guard graph facts, capture-edge attributes, parameter-mutation summaries, established-by DI provenance, and string-pedigree reflection edges |
| 11 | [11-roadmap.md](11-roadmap.md) | Phasing: MVP → semantic precision → dataflow/concurrency/security → branch/VCS features → MCP polish; feature-ID status rollup (GM-9..20, DF-9..20, Q-20..31, IX-9, LS-7..8, docs/12 FW-1..6); risks including alias analysis cost, lock-set false positives, framework-pack maintenance burden, and type-reconstruction precision |
| 12 | [12-language-primitives-and-frameworks.md](12-language-primitives-and-frameworks.md) | Framework-pack design and semantic-class catalog: seven metadata semantic classes (`entrypoint`, `guard`, `negative-guard`, `interception`, `generated-member`, `keep-alive`, `contract`); per-framework worked mappings (Spring, Flask/Django, ASP.NET, NestJS, tokio/actix); mediated edges with `established-by` provenance; build-configuration variance; reflection packs. Maps language/framework constructs onto the primitives defined in docs/03 (GM-15..19) and docs/04 (DF-20) |
| 13 | [13-glossary.md](13-glossary.md) | Alphabetical dictionary of all terms of art used across the `cgx` documentation: graph vocabulary (node, edge, edge conditions, exceptional class, transience), path concepts (reachability, dominance, must-pass-through), provenance and taint vocabulary (pedigree, source, sink, sanitizer, typed taint, union/join pedigree), confidence ladder, status tags (core-extension/schema-room/roadmap), query-language terms (Layer 1/Layer 2, MATCH, predicate, procedure, COMPATIBLE_WITH, IS EMPTY), cycle-3 vocabulary (metadata fact, seven semantic classes, framework pack, established-by, capture edge, mutability, function value, coerce, lineage type reconstruction), and indexing terms |
| 14 | [14-implementation-status-matrix.md](14-implementation-status-matrix.md) | Feature×language implementation status matrix, and the tree's ledger of what is actually built. Section 1: adapter capabilities (parse, defs, refs, imports, inheritance, entrypoints, cut hints, effects, dataflow, concurrency, implicit call sites, SCIP, framework packs). Section 2: the query surface — every shipped subcommand, including `symbols` and `coupling` — by language (Rust · TS/JS · Go · Python · Java · C#). Section 3: which surfaces carry the approximation contract and the freshness envelope. **Read the matrix itself rather than this description for the row list; it grows.** |

| — | [questions/README.md](questions/README.md) | Index of the 13 question-cookbook theme files (138 questions total): file names, themes, question ID ranges, and counts; plus an explanation of the entry template (how to read personas/status/query/breakdown/result sections) and pointers to docs/13 and docs/05 |
| — | [commands/](commands/) | Per-subcommand reference for the CLI surface: flags, formats, exit codes, worked examples, and stated limits. The authority on what a command accepts today; docs 01–15 are the authority on what it is *for* |
| — | [mcp-tools/](mcp-tools/) | Per-tool reference for the MCP surface: input schemas, response shapes, and which envelope fields each tool carries |
| — | [research/](research/) | Dated research snapshots feeding the capability ledger (doc 15). Historical, not living spec — read for provenance, not for current state |


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

[13 — Glossary](13-glossary.md) is the alphabetical dictionary and the place to look
first; [03-code-graph-model.md](03-code-graph-model.md) is where the graph-model terms
are *specified*. Key terms used across all docs: `symbol`, `edge condition`,
`path-relative transience`, `confidence`, `pedigree`, `provenance`, `entrypoint`,
`approximation contract`, `freshness envelope`, `modeled boundary`, `capture edge`,
`framework pack`, `mediated edge`, `established-by`, `mutation fan-out`,
`type-confidence boundary`. Do not introduce synonyms; add the term to doc 13, and
specify it in doc 03 or doc 04, rather than coining a new one.

Glossary entries whose surface is not built are tagged **(deferred)** in place, so the
glossary can define the vocabulary of the design without implying the binary answers to
it.

## Doc Status

All fifteen numbered docs plus the question cookbook are written. They are living
documents, not a delivered artifact: docs 03–05, 08, and 12 in particular specify a
model well ahead of the implementation, and doc 14 is the ledger that says how far
ahead. Docs under `research/` and `reviews/` are dated snapshots and are deliberately
not refreshed — a review is evidence of what was believed on its date, and editing it
destroys that.
