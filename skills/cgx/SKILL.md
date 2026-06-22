---
name: cgx
description: >-
  Use when answering structural or call-graph questions about a codebase with the cgx tool (CLI or MCP):
  who calls a function, what a function calls, whether one symbol reaches another, the call paths between
  two symbols, dead or unused code, the blast radius of a change, exception/failure paths, taint or data
  provenance, framework entrypoints, inheritance/overrides, or call-graph diffs between git refs. Also use
  when a cgx command failed unexpectedly (exit code 2, a CQL parse/plan error, a hanging query, or
  "no symbol matched").
---

# cgx — deterministic call-graph tool

cgx is a general-purpose, deterministic call-graph and code-graph tool for source code in any language,
implemented in Rust. No daemon, fast startup, usable as a **human/agent CLI** (the initial focus) and over a
**STDIO MCP server**. Every answer carries `file:line` evidence and a labeled confidence
(`certain`/`probable`/`possible`). **No LLM is in the answer path** — answers are deterministic and
reproducible. cgx tells you about *structure and reachability*; it does not run the code.

## STEP 0 — Always check the version first

cgx features are gated by version. **Run `cgx --version`** → `cgx 0.<MINOR>.<PATCH>`. A capability tagged
`Since: v0.N` is available **iff `MINOR ≥ N`**. Everything in this skill carries a `Since:` tag. If a feature is
above the running version, do not emit it — fall back to the highest available alternative or tell the user it
needs `cgx ≥ 0.N`. The current shipped binary is **v0.1**. See `reference/versions.md` for the full ladder.

## STEP 1 — You need an exact symbol name

cgx has **no search, glob, or fuzzy match**. Commands need the exact (usually fully-qualified) symbol name and
fail with `no symbol matched '<x>'` (exit 2) otherwise. **Grep/ripgrep the source for the real name first**,
then pass it to cgx. The index auto-builds on first query into `.cgx/`; force a rebuild with `cgx index .`.
There is no `prune`/`clean`.

## Mental model — read before interpreting any result

The graph vocabulary (node/edge kinds, edge-condition labels `always`/`conditional`/`exception`/`loop`/`panic`,
the confidence ladder, path-relative transience, file:line evidence) is defined once in
`reference/mental-model.md`. Read it before trusting a result — confidence and edge conditions change what an
answer means.

## Dispatch table — load the file that matches the question

| You want to… | Load |
|---|---|
| Find who calls / what calls a symbol; whether/how X reaches Y; enumerate call paths | `recipes/reachability.md` |
| Assess impact / blast radius of changing a symbol; what could break | `recipes/impact.md` |
| Find dead / unused / unreachable code | `recipes/dead-code.md` |
| Reason about exception / panic / failure paths; ∀-path "must pass through" | `recipes/failure-paths.md` |
| Trace taint / data provenance; "can input reach this sink" *(v0.3)* | `recipes/taint.md` |
| Map the public API surface / contracts | `recipes/api-contracts.md` |
| See what changed in the call graph between commits/branches | `recipes/vcs-diffs.md` |
| Drive cgx as an AI agent (MCP tools, token-efficient queries) | `recipes/ai-agent.md` + `reference/mcp.md` |
| Build a threat model / data-flow diagram *(v0.3)* | `recipes/threat-modeling.md` |
| Reason about concurrency, locks, async safety *(v0.3)* | `recipes/concurrency.md` |
| Reason about types, mutability, closures *(v0.3)* | `recipes/types-mutability-closures.md` |
| Reason about framework entrypoints/guards (Spring/Flask/…) *(v0.3)* | `recipes/frameworks.md` |
| Reason about inheritance, overrides, MRO *(v0.3)* | `recipes/object-model.md` |
| Write or debug a CQL (`cgx query`) expression | `reference/query-language.md` |
| Choose an output format; understand exit codes; assert in CI | `reference/output-and-exit.md` |
| Know the exact subcommands and flags | `reference/cli.md` |
| Know which languages are supported and their semantics | `reference/languages.md` |

## Common wrong turns (pre-empt these)

- **`cgx query` runs, but CQL is gated per-clause.** In v0.1 only the **CALLS-graph** subset works
  (`MATCH (a)-[:CALLS]->(b) … RETURN …`). `DATA_FLOW`, taint props, and `MUST PASS THROUGH`/`AVOIDING` are
  **v0.3** and error or return empty today. See `reference/query-language.md`.
- **Never emit unbounded `CALLS*` — it hangs.** Always bound the hops: `CALLS*2`.
- **CQL strings use double quotes.** Wrap the whole query in single quotes for the shell:
  `cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "foo" RETURN a.name'`.
- **Real flags, not the cookbook's:** `--max-depth` (not `--depth`); `--repo ./` (not a trailing `./`);
  `--kind function` (not `--kind fn`); `cgx diff <BASE> <HEAD>` positional (not `--base/--head`). Flags like
  `--from-class`, `--avoiding`, `--only-edge-condition` **do not exist**.
- **cgx answers reachability (call paths), not data flow.** `reaches A B` means "a call path exists", not
  "data can flow A→B". Real taint is v0.3.
- **Exit codes:** `0` = success *including empty results*; `2` = bad symbol / CQL parse-or-plan error / bad arg
  (not "empty"); `1`/`4` = `--assert-empty` outcomes. See `reference/output-and-exit.md`.

## Note on the question cookbook

`docs/questions/` (the upstream cookbook) is a rich source of phrasings, but several entries tagged
"answerable-today" use flags or analysis that the v0.1 binary does not implement. This skill is tagged against
what actually runs (`reference/versions.md`). When they disagree, trust the version tags here.
