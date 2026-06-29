# 14 — Implementation Status Matrix

> **Keep this matrix in sync** when an adapter gains a capability or a new language is added. Features are rows; languages are columns. See [08 — Language Support](08-language-support.md) for the tier model and the per-language specification.

**Legend:** `✓` implemented · `~` partial · `planned` on the roadmap · `—` not applicable / not started.

## Section 1 — Adapter capabilities

| Capability | Rust | TS/JS | Go | Python | Java | C# |
|---|---|---|---|---|---|---|
| Parse (tree-sitter) | ✓ | ✓ | ✓ | planned | planned | planned |
| Symbol extraction (defs) | ✓ | ✓ | ✓ | planned | planned | planned |
| Call refs | ✓ | ✓ | ✓ | planned | planned | planned |
| Imports / exports | ✓ | ✓ | ✓ | planned | planned | planned |
| Inheritance / overrides | ✓ | — | ~ | planned | planned | planned |
| Entrypoint hints | ✓ | ✓ | ✓ | planned | planned | planned |
| Cut hints | ✓ | ✓ | ✓ | planned | planned | planned |
| Own-effects | ✓ | — | ✓ | planned | planned | planned |
| Intraprocedural dataflow (SSA) | ✓ | — | ✓ | planned | planned | planned |
| Concurrency / async hints | ~ | ~ | ~ | planned | planned | planned |
| SCIP enrichment | ✓ | ~ | planned | planned | planned | planned |
| Framework packs | ~ | ~ | planned | planned | planned | planned |

## Section 2 — Query surface

| Query | Rust | TS/JS | Go | Python | Java | C# |
|---|---|---|---|---|---|---|
| `callers` | ✓ | ✓ | ✓ | planned | planned | planned |
| `callees` | ✓ | ✓ | ✓ | planned | planned | planned |
| `reaches` | ✓ | ✓ | ✓ | planned | planned | planned |
| `paths` | ✓ | ✓ | ✓ | planned | planned | planned |
| `unused` | ✓ | ✓ | ✓ | planned | planned | planned |
| `search` | ✓ | ✓ | ✓ | planned | planned | planned |
| `explain` | ✓ | ✓ | ✓ | planned | planned | planned |
| `flows-to` / `flows-from` | ✓ | — | ✓ | planned | planned | planned |
| effects queries | ✓ | — | ✓ | planned | planned | planned |
| `diff` | ✓ | ✓ | ✓ | planned | planned | planned |

A query works for a language iff the facts it needs exist (Section 1). Update both sections together when an adapter changes.
