# Question Cookbook — Index

This directory contains 12 theme files totalling 126 answered questions drawn from the 126-question inventory in [docs/02-personas-and-questions.md](../02-personas-and-questions.md). Each file focuses on a single question theme and provides `cgx` query recipes for every question in that theme.

---

## Theme files

| # | File | Theme | Question IDs | Count |
|---|------|-------|-------------|-------|
| 01 | [01-reachability-and-attack-surface.md](01-reachability-and-attack-surface.md) | Reachability and Attack Surface | Q1–Q13, Q87–Q88, Q92–Q93, Q100 | 18 |
| 02 | [02-impact-and-blast-radius.md](02-impact-and-blast-radius.md) | Impact and Blast Radius | Q14–Q25 | 12 |
| 03 | [03-provenance-and-taint.md](03-provenance-and-taint.md) | Provenance and Taint | Q26–Q36, Q86, Q91, Q94–Q96, Q102 | 17 |
| 04 | [04-failure-path-behavior.md](04-failure-path-behavior.md) | Failure-Path Behavior | Q37–Q45, Q76–Q78, Q98, Q101, Q103, Q106 | 16 |
| 05 | [05-dead-and-unused-code.md](05-dead-and-unused-code.md) | Dead and Unused Code | Q46–Q53 | 8 |
| 06 | [06-temporal-and-vcs-graph-diffs.md](06-temporal-and-vcs-graph-diffs.md) | Temporal and VCS Graph Diffs | Q54–Q61, Q99, Q107 | 10 |
| 07 | [07-api-surface-and-contracts.md](07-api-surface-and-contracts.md) | API Surface and Contracts | Q62–Q67 | 6 |
| 08 | [08-ai-agent-specific-queries.md](08-ai-agent-specific-queries.md) | AI-Agent-Specific Queries | Q68–Q75, Q79–Q81 | 11 |
| 09 | [09-threat-modeling-and-dfd.md](09-threat-modeling-and-dfd.md) | Threat Modeling and DFD | Q82–Q85 | 4 |
| 10 | [10-concurrency-and-resource-safety.md](10-concurrency-and-resource-safety.md) | Concurrency and Resource Safety | Q89–Q90, Q97, Q104–Q105, Q108 | 6 |
| 11 | [11-types-mutability-and-closures.md](11-types-mutability-and-closures.md) | Types, Mutability, and Closures | Q109–Q118 | 10 |
| 12 | [12-framework-semantics-and-metadata.md](12-framework-semantics-and-metadata.md) | Framework Semantics and Metadata | Q119–Q126 | 8 |

**Total: 126 questions.** Question IDs are globally unique; each question appears in exactly one file.

---

## How to read an entry

Each entry follows a consistent template defined in the cycle synthesis document. Understanding the template helps you get the most out of the recipes.

**Header and status.** Every entry opens with `### Qnn — <question text>`, then a `**Personas:**` line naming which of the four personas (PSE = Principal Security Engineer, SSE = Staff Software Engineer, ACA = AI Coding Agent, ASA = AI Security Agent) find this question most relevant, and a `**Status:**` tag. Three status values appear: `answerable-today` means the query runs against a current `cgx` index; `needs-schema-room-feature (name)` means a specific schema-room feature must be implemented first; `roadmap` means the question depends on a feature not yet scheduled. Queries for non-`answerable-today` entries are headed with a comment line `-- illustrative: requires <feature-id> (schema-room)` or similar so you can distinguish runnable from illustrative queries at a glance.

**The query block.** Most entries show a Layer 2 (`cgx query '…'`) expression in a fenced code block labelled `cgx`. Some simple lookups show the Layer 1 subcommand form (`cgx callers …`, `cgx paths …`) with a note that the Layer 2 form also exists. Query syntax follows the specification in [docs/05-queries.md](../05-queries.md) exactly: `MATCH … WHERE … RETURN`, `MATCH ALL … MUST PASS THROUGH / AVOIDING`, `CALL cgx.<proc>(…) YIELD …`, and the documented predicates (`COMPATIBLE_WITH`, `IS EMPTY`, `NONE`, `ANY`). No invented syntax is presented as runnable.

**Breaking it down.** A table (for queries with three or more distinct fragments) or a short explanatory paragraph (for simpler queries) walks through each fragment of the query in plain English. The breakdown is written for a reader who can read the query language but does not write it beyond light use — every clause is explained, and every term of art used in the breakdown is defined in [docs/13-glossary.md](../13-glossary.md).

**Reading the result.** Where the output carries important caveats — confidence-ladder implications, path-transience effects, sampling limits, or conditions under which a result might be a false positive or false negative — a short "Reading the result" paragraph after the breakdown explains what to expect and what to watch for. Trivial single-step queries that produce no surprising output omit this section.

---

## Further reading

- **Query language specification and worked examples:** [docs/05-queries.md](../05-queries.md) — the authoritative reference for all query syntax, subcommand signatures, and the full Q-1 through Q-31 feature list.
- **Term definitions:** [docs/13-glossary.md](../13-glossary.md) — plain-language definitions of every term of art used across the cookbook, including edge conditions, confidence tiers, transience, pedigree, taint vocabulary, semantic classes, and indexing terms.
- **Persona question inventory:** [docs/02-personas-and-questions.md](../02-personas-and-questions.md) — the authoritative list of all 126 questions, their themes, and the personas that ask them.
