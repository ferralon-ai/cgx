# 14 — Implementation Status Matrix

> **Keep this matrix in sync** when an adapter gains a capability or a new language is added. Features are rows; languages are columns. See [08 — Language Support](08-language-support.md) for the tier model and the per-language specification.

**Legend:** `✓` implemented · `~` partial · `planned` on the roadmap · `—` not applicable / not started.

## Section 1 — Adapter capabilities

| Capability | Rust | TS/JS | Go | Python | Java | C# |
|---|---|---|---|---|---|---|
| Parse (tree-sitter) | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Symbol extraction (defs) | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Call refs | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Imports / exports | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Inheritance / overrides | ✓ | — | ~ | ✓ | ✓ | planned |
| Entrypoint hints | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Cut hints | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| Own-effects | ✓ | — | ✓ | ✓ | ✓ | planned |
| Intraprocedural dataflow (SSA) | ✓ | — | ✓ | ✓ | ✓ | planned |
| Concurrency / async hints | ~ | ~ | ~ | ~ | ~ | planned |
| SCIP enrichment | ✓ | ~ | planned | planned | planned | planned |
| Framework packs | ~ | ~ | planned | planned | planned | planned |

## Section 2 — Query surface

| Query | Rust | TS/JS | Go | Python | Java | C# |
|---|---|---|---|---|---|---|
| `callers` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `callees` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `reaches` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `paths` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `unused` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `search` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `explain` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |
| `flows-to` / `flows-from` | ✓ | — | ✓ | ✓ | ✓ | planned |
| effects queries | ✓ | — | ✓ | ✓ | ✓ | planned |
| `diff` | ✓ | ✓ | ✓ | ✓ | ✓ | planned |

A query works for a language iff the facts it needs exist (Section 1). Update both sections together when an adapter changes.
