# cgx CLI — Quick Reference

**Audience:** Engineers and AI agents driving cgx from the command line.

**Authoritative, comprehensive reference:** [`docs/commands/`](../../../docs/commands/)
and its [`README`](../../../docs/commands/README.md) (per-command pages with full flag tables,
output samples, and exit-code details).

This file is a **cheat-sheet only** — one line per subcommand and the flags you reach for
most. Look up edge cases in `docs/commands/`.

---

## All 22 subcommands

| Subcommand | One-job summary | Most-used flags |
|---|---|---|
| `index` | Analyze source and write the call+dataflow graph to `.cgx/` | `--no-dataflow`, `--scip`, `--compdb`, `--receiver-narrowing` |
| `callers` | Symbols that (transitively) call a named symbol | `--depth`, `--format`, `--confidence` |
| `callees` | Symbols that a named symbol (transitively) calls | `--depth`, `--format`, `--confidence` |
| `flows-to` | Forward data-flow slice from a value node (v0.3) | `--depth`, `--confidence`, `--repo` |
| `flows-from` | Backward data-flow pedigree from a value node (v0.3) | `--depth`, `--confidence`, `--repo` |
| `reaches` | Boolean reachability check with witness path, or full reachable-symbol enumeration | `--depth`, `--format`, `--confidence` |
| `paths` | Enumerate every distinct call path from one symbol to another | `--depth` (default 6), `--format` |
| `explain` | Full provenance for one symbol: definition, edge counts, all incident edges | `--format`, `--repo` |
| `query` | Run a CQL (Cypher-subset) expression against the call or dataflow graph | `--at`, `--format`, `--repo` (`--depth` is inert here — bound the walk in the query) |
| `search` | Find symbols by FQN substring, regex, or selector (or `--all`); no graph walk | `--all`, `--kind`, `--regex`, `--agnostic`, `--limit` |
| `symbols` | Rank symbols by reference count, with inbound/outbound edge breakdown (v0.3) | `--rank total\|inbound\|outbound`, `--top`, `--kind` |
| `unused` | Symbols not reachable from any indexed entrypoint | `--kind`, `--format` |
| `doctor` | Report on the quality and trust level of the current on-disk index | `--format` |
| `diff` | Diff the call graph between two git refs; `--path-added` turns it into a structural PR gate | `--added`, `--kind`, `--edge-condition`, `--from`/`--to`, `--path-added` |
| `coupling` | File pairs that historically change together, over a commit range. Reads git history, not the index | `--min-cochanges`, `--limit`, `--max-files-per-commit`, `--format` |
| `impacted-tests` | Test entrypoints whose call graph reaches a symbol changed between two refs, or in the working tree | `[BASE] [HEAD]`, `--uncommitted`, `--no-merge-base`, `--format` |
| `pack` | Agent-context pack: JSON cards (`interface-map`, `dependency-footprint`) plus a graph-honesty manifest | `--interface`/`--type`, `--depth`, `--onedoc`, `-o` |
| `dump` | Print the stored graph (nodes, edges, candidates), whole or filtered to one symbol substring | `--format text\|json` |
| `push` | Publish the built index to a git remote under `refs/cgx/index` | `[REMOTE]` (default `origin`), `--repo` |
| `pull` | Fetch `refs/cgx/index` from a remote and materialize it into `.cgx/` | `[REMOTE]` (default `origin`), `--repo` |
| `configure-remote` | Add the `+refs/cgx/*:refs/cgx/*` refspec to a remote so plain `git fetch`/`push` carry the index | `[REMOTE]` (default `origin`) |
| `mcp` | Start the MCP STDIO server for AI agents and IDE extensions | `--root` |

Seventeen of the twenty-two read the code graph. `coupling` reads committed git history and never
opens `.cgx/`; `push`, `pull` and `configure-remote` move or configure the stored index but answer
no question; `mcp` produces no answer of its own. That split decides which trailing envelope
lines an answer carries — see "What rides on an answer" below.

---

## Global gotchas

### What rides on an answer

Two trailing lines can follow a human answer, and the pair is **per-command**, not universal.
Do not expect both, and do not parse for one the command never emits.

| Subcommand | `approximation:` | `freshness:` |
|---|---|---|
| `callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `unused`, `query` | yes | yes |
| `explain`, `search`, `symbols` | no | yes |
| `impacted-tests` | yes | yes |
| `coupling` | yes | **no** — it never opens the index |
| `pack` | per card, inside the JSON (a `manifest` header rides alongside) | no |
| `index`, `doctor`, `diff`, `dump`, `push`, `pull`, `configure-remote` | no | no |

In `--format json` the same split holds as top-level `approximation` and `freshness` keys.
`--format dot|mermaid|d2` emits raw graph source and carries neither.

`freshness` reports how far the graph the answer was computed over sits from your working tree.
Human form is a verdict plus only the qualifiers that apply:

Three real runs of the same `cgx callers helper --no-auto-index`, differing only in what
happened to the repo between them:

```
freshness: current | indexed tree a6a64d0, working tree clean
freshness: stale | indexed tree a6a64d0, 1 dirty file
freshness: stale | indexed tree a6a64d0 (behind HEAD eb34133), 1 dirty file
```

The second followed an uncommitted edit; the third followed committing it. The `(behind HEAD …)`
qualifier appears only when the *committed* tree moved, so "stale" alone does not tell you which
of the two happened — read the qualifier.

JSON carries `{indexed_tree, head_tree, matches_head, dirty_files_base, dirty_files, stale}`.
**`matches_head` is three-valued** — `true`, `false`, or `null` (rendered `(HEAD unknown)`,
verdict `unknown`) — so branch on three values rather than treating it as a boolean. Over MCP
with the default `include_dirty: true`, `null` is the *ordinary* result for any repo that has
been indexed. `current` is reserved for the case where both halves were established *and* both
came back clean; anything the command did not look at yields `unknown`, never the reassuring
word.

A command with no `approximation:` line is not claiming its answer is exact; it is saying
nothing about the question.

### Dataflow is ON by default

A plain `cgx index .` builds the full dataflow layer (SSA value nodes + `derives-from`
edges). Pass `--no-dataflow` to produce a lighter, CALLS-only index. There is **no**
`--dataflow` flag — the flag to know is `--no-dataflow`.

### Depth flag is always `--depth`

Where a CLI subcommand takes a depth limit at all, the flag is `--depth`. Nine take it:
`callers`, `callees`, `flows-to`, `flows-from`, `reaches`, `paths`, `query`, `unused`,
`impacted-tests` (plus `pack dependency-footprint`, whose card-local `--depth` defaults to 2).
`--max-depth` does **not** exist on the CLI (the MCP `paths` tool uses the JSON property
`max_depth`, but that is MCP-only).

Default depth by command group:
- `callers`, `callees`, `flows-to`, `flows-from`, `reaches <from>` (enumerate-all form): **2**
- `paths`: **6**
- `reaches <from> <to>` (witness-path form) and `unused`: **unbounded** when `--depth` is unset —
  they pass `--depth` through verbatim with no default, so only the walker's internal step budget
  applies.
- `query`: **`--depth` is accepted but has no effect.** A CQL traversal is bounded entirely by the
  query's own hop syntax (`[:CALLS*1..8]`) or, for a bare `*`, by the engine's default cap. The same
  is true of `--confidence` and `--tree` on `query`: they parse and are then discarded.

### ⚠ `--depth 0` means *unlimited* only on `paths`

`paths` translates `--depth 0` to "no depth limit". No other command does. Everywhere else `0` is
taken literally as **zero hops** — and what a zero-hop walk *produces* differs per command:

| Command | `--depth 0` |
|---|---|
| `paths` | depth bound removed — but the walker's **step budget** still applies, and often bites first (see below) |
| `callers`, `callees`, `flows-to`, `flows-from`, `reaches <from>` | **zero results** — the walk expands no neighbors. Contract reports `direction: "under"` with a `depth-limit` reason |
| `reaches <from> <to>` | **not reachable** — zero hops, so the answer is a false negative, not an empty result. Same `under` / `depth-limit` contract |
| `unused` | **inflated dead-code report** — zero hops makes nothing reachable through the walk, so far more symbols fall into the complement. Never pass `--depth 0` here |
| `query` | no effect (see above) |
| `impacted-tests` | **zero results** — `(no results)` at exit 0, contract reports `depth<=0`. Omit `--depth` for unbounded |

The `unused` row is the dangerous one. `--depth 0` is the natural thing to reach for when you want
"no limit", and on `unused` it does not fail loudly — it returns a dead-code report inflated with
false positives that looks like a rich, successful answer. On the **self-index corpus** (18,431
nodes — the exact build recipe is in `reference/mental-model.md` §2.2, and a different tree gives
different absolutes), `cgx unused` reports **15,649** symbols and `cgx unused --depth 0` reports
**17,157**: 1,508 symbols that are reachable in the honest graph, presented identically to the real
ones.

**`paths --depth 0` is not the escape hatch it looks like either.** It removes the *depth* bound
only; the step budget stays, and on a large graph it can cut the search before any path is found.
On the same self-index corpus, `cgx paths cgx_cli::run cgx_cli::assertions::evaluate` returns
**17 paths at `direction: "exact"`** with the default `--depth 6`, and **0 paths** with `--depth 0`
— flagged `"truncated": true`, `"truncation_reason": "step-budget"`, `direction: "under"`. Removing
the bound made the answer strictly worse *and* empty. Read `truncated` before reading an empty
`paths` result as absence.

To walk unbounded, omit `--depth` on `reaches <from> <to>`/`unused` (they are unbounded by default),
or pass an explicitly large `--depth N` elsewhere. Never reach for `--depth 0` expecting "no limit".

> **`--help` will tell you otherwise — believe this table.** `--depth` is a shared flag, so its
> help text is the *same string on all nine commands that take it* (`callers`, `callees`,
> `flows-to`, `flows-from`, `reaches`, `paths`, `query`, `unused`, `impacted-tests`), and it reads "pass
> `--depth 0` for unlimited depth, which stays protected by an internal work budget". That is
> true of `paths` and false of every other command; `cgx callers --help` and `cgx unused --help`
> both print it. Verifying this table against `--help` will faithfully restore the bug.
>
> The **MCP `paths` tool inverts the sentinel outright**: `max_depth: 0` is passed through as a
> literal bound, so it returns *nothing* where the CLI's `--depth 0` returns everything the budget
> allows. Never port a `0` between the two surfaces. See `reference/mcp.md`.

`reaches` also differs across surfaces in what it *returns*: the **MCP** tool answers with an
explicit `"reachable": <bool>` plus a `witness`. The **CLI** returns the same path-result shape as
`paths` — `count`/`results` — and emits no `reachable` key at all. An empty `results` is the CLI's
"not reachable". Do not write a CLI JSON parser looking for `reachable`.

### Phantom flags — never emit these

These look plausible but cause exit 2 in the real binary:

`--max-depth`, `--base`, `--head`, `--from-class`, `--to-class`,
`--avoiding`, `--only-edge-condition`, `--dataflow`, `--calls-to-sink-class`,
`--kind fn` (correct: `--kind function`), a trailing `./` positional in place of
`--repo ./`.

**`--from` / `--to` are real — but only on `cgx diff`**, where they are FQN glob post-filters and
the anchors for `--path-added`. They are phantom on all 21 other subcommands. (`cgx diff --kind`
also takes a *different* enum from `unused`/`search`/`symbols`: `calls`, `data-flow`, `overrides`,
… — 19 edge-kind tokens, none of them `fn` or `function`.)

### `--sql` is accepted almost everywhere and read in one place

`--sql` is part of the shared query-flag group, so `callers`, `callees`, `flows-to`, `flows-from`,
`reaches`, `paths`, `unused`, `impacted-tests` and `query` all accept it. Only `query` reads it — and there it exits
2 with `the --sql recursive-CTE interface is not implemented in this release`. On the other eight
it is **silently ignored at exit 0**: `cgx callers <sym> --sql` returns an ordinary answer with no
warning. The flag is hidden from every `--help`, so there is nothing to check it against. Do not
read a clean exit as confirmation that the flag did anything.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success (including empty results) |
| 1 | A CI gate fired — `--assert-empty` found results, **or** `diff --path-added` found a new path (no assertion flag involved) |
| 2 | Bad symbol / CQL parse or plan error / bad flag value / a `--format` the command does not implement / an unresolvable git revspec |
| 3 | Index missing, corrupt, or unbuildable (most commonly `--no-auto-index` against a repo with no `.cgx/`, but a corrupt `.cgx/` store also exits 3 regardless of that flag). **`doctor` hits it without any flag** — see below. Also the "not a git repository" case: `index` and `coupling` both exit 3 when the target is not inside a git working tree. `push`/`pull` also exit 3 when the underlying git command fails (no such remote; a remote with no `refs/cgx/index`) |
| 4 | `--assert-empty` vacuous: confidence filter excluded all results (suppress with `--allow-vacuous`) |
| 4 | **Also** `impacted-tests` on a changed set in no supported language (TS/JS, C/C++): a degenerate answer, with an `impacted-language-unsupported` reason and a stderr note — never a silent empty list |

**`doctor` is the one read command that never auto-indexes.** Every other read command builds the
index on demand when `.cgx/` is absent; `doctor` does not, so against an unindexed repo it exits
`3` with `no index found at "…/.cgx"; run \`cgx index\` first` — with no `--no-auto-index` passed.
Run `cgx index .` first, or treat a `doctor` exit 3 as "not indexed yet", not as "index corrupt".

### CQL deferred clauses (exit 2 today)

Taint properties (`source_class`, `sink_class`, `sanitizer_class`, `taint_label`) and
the keywords `MUST PASS THROUGH` / `AVOIDING` are not implemented — they produce a plan
or parse error (exit 2). Note `taint_label` is gated as an **edge** property, the other three as
node properties — the exit code and plan error are the same either way. `:CALLS` and `:DATA_FLOW`
edges work. Node properties `name` (alias `fqn`), `kind`, `file`, `line` and edge properties
`condition`, `confidence`, `kind` work.

---

## Selectors (`cgx search` and CQL `select()`)

A `cgx search` pattern containing `**`, `(` or `!` (and no `--regex`) goes to the selector engine
instead of the substring matcher. Any other pattern is a plain case-insensitive substring — so a
lone `*` is a **literal character**: `cgx search 'rust_sample::*::dispatch'` returns
`(no results)` at exit 0, while `'rust_sample::**::dispatch'` finds `rust_sample::conditions::dispatch`.

| Token | Meaning |
|---|---|
| `**` | zero or more whole path segments |
| `*` | any run of characters inside one segment |
| `(a\|b)*` | a segment starting with `a` or `b` |
| `!(…)` | invert that segment's constraint |

There is no `?`. Verified on `fixtures/rust-sample`:

```bash
cgx search 'rust_sample::**::dispatch'                  # one hit, any depth
cgx search 'rust_sample::(conditions|direct)*::*'       # everything in those two modules
cgx search 'rust_sample::virtual_dispatch::!(make)*'    # that module, minus make_*
```

**Selectors are family-gated by source language.** Families exist for Rust, Go, TypeScript, Java and
Python; a symbol matches only through a family that owns its language. **C and C++ have no family**,
so a selector matches no C/C++ symbol unless you pass `--agnostic`, which drops the gate and prints a
stderr `selector note:` counting the matches that came from another language. `--agnostic` has no
effect on substring or `--regex` patterns. In CQL the same engine is `select(node, "<selector>")`,
with an optional third `agnostic` argument:
`cgx query 'MATCH (a)-[:CALLS]->(b) WHERE select(a, "rust_sample::**") RETURN a.name, b.name LIMIT 3'`.

The MCP `search` tool has no selector grammar and no `agnostic` parameter — it is substring or
regex only. Over MCP, reach selectors through `graph_query` with `select()`.

## `impacted-tests` — which tests does this change reach?

```bash
cgx impacted-tests --uncommitted          # inner loop: working tree vs HEAD
cgx impacted-tests main HEAD              # PR gate: what this branch adds (merge-base by default)
cgx impacted-tests HEAD~1 HEAD --format json
```

The changed set is every symbol the graph diff names **plus** every symbol defined in a changed file
— the file-granular half is what catches a body-only edit, and the contract discloses it as an
over-approximation (`impacted-changed-set-file-granular`). Each result is a test entrypoint with the
changed symbol it reaches. `--no-merge-base` compares against the base tip instead (note `cgx diff`
compares tips, so the two disagree on the same pair of refs). JSON keys: `results`, `count`,
`vacuous`, `approximation`, `freshness`. The query flags `--depth`, `--confidence` and
`--max-candidates` apply.

**Languages: Rust, Go, Java and Python.** A changed set in no supported language (TypeScript/JavaScript,
C, C++) exits **4** with `impacted-language-unsupported` on the contract and a stderr line
(`no supported language in the changed set (c); the empty result is vacuous, not a clean bill of
health`). Python test recognition is incomplete — unittest camelCase `testFoo` methods and
non-default pytest discovery config are missed (`impacted-test-recognition-incomplete-python`).

Verified on a copy of `fixtures/python-app` after a body edit to `app.build_record`:

```
app::build_record  app.py:11
└─ test_app::test_build_record  test_app.py:10  [probable]
```

An edit to a function no test reaches returns `(no results)` at exit 0 — that one *is* an answer.

## `pack` — agent-context cards

```bash
cgx pack                                                     # full pack: interface-map + manifest
cgx pack interface-map --interface rust_sample::virtual_dispatch::Speak  # Dog, Cat
cgx pack dependency-footprint rust_sample::virtual_dispatch::make_speak
```

Output is JSON only (`--json` is accepted for symmetry). The full pack is `{cards, manifest}`;
each card carries its own `approximation`. `dependency-footprint` takes an **exact** FQN — no
substring or short-name matching — and walks forward to `--depth` (default 2). A wrong FQN is not an
empty footprint: it returns `"resolved": false` with an `error`, at exit 0, so check `resolved`.
More than one card (`interface-map,dependency-footprint`) requires `--onedoc`, else exit 2.

## `dump` — inspect the stored graph

`cgx dump [SYMBOL] [--format text|json]` prints `graph_key`, the node/edge/candidate counts, then
nodes (`id kind fqn file:line`) and edges (`src -> dst kind condition confidence`). `SYMBOL` is a
substring filter, so `cgx dump make_speak` also returns `make_speak_boxed`. It is read-only: it never
indexes and carries no envelope. It is a debugging view of raw storage, not a query surface.

## Transport — `push`, `pull`, `configure-remote`

Share a built index through git instead of re-indexing on every machine:

```bash
cgx index . && cgx push            # pushed 217 object(s) to origin: refs/cgx/index advanced to <oid>
cgx pull                           # (in a clone) pulled from origin: materialized 217 object(s) across 1 graph(s)
cgx configure-remote               # adds +refs/cgx/*:refs/cgx/* so plain git fetch/push carry it
```

The index lives under the non-branch ref `refs/cgx/index`; no branch moves and no commit appears in
`git log`. `pull` re-hashes every object against its OID. A remote with no `refs/cgx/index`, or no
such remote at all, exits **3** — never an empty success. `configure-remote` edits only this
repository's config; a CI runner needs the same refspec in its own git config.

## Precision dials

- **`--max-candidates N`** (on `callers`, `callees`, `reaches`, `paths`, `impacted-tests`; MCP
  `max_candidates` on the first four): drop call edges whose candidate set (name collision, dynamic
  dispatch) has more than `N` targets. A singly-resolved call is never dropped. It is orthogonal to
  `--confidence`, and the contract turns `under` and counts what it cut:
  `cgx callees make_speak --max-candidates 1` on `fixtures/rust-sample` drops both `Cat::speak` and
  `Dog::speak` and reports `2 edge(s) in a candidate set larger than 1 were excluded`.
- **`cgx index --receiver-narrowing`** (or `CGX_RECEIVER_NARROWING=1`): opt-in narrowing of virtual
  call sites by receiver, for **Python and Go** only — `self`/`cls` to the enclosing class cone,
  `super()` and class-headed calls to the class lookup, literal and out-of-repo receivers to no edge.
  The `index` summary gains `receiver …` stat lines; answers that lost candidates say so in their
  contract. `CGX_PRECISION_TRACE=1` prints one stderr line per narrowed site. (`--help` says
  "(Python)"; Go is narrowed too.)
- **`cgx index --scip <file> --compdb compile_commands.json`**: a `scip-clang` index promotes C++
  overload/ADL/template edges only as far as `probable` unless the compdb is verified to cover every
  C++ translation unit (`.cpp/.cc/.cxx`; headers excluded). A missing TU fails closed at `probable`
  with a diagnostic. `--compdb-complete` asserts completeness instead (for CI, where the build system
  guarantees it). Both are ignored for a rust-analyzer index.

---

## Verified one-liners (corpus: `fixtures/rust-sample`)

```bash
# Index — full dataflow (default)
cgx index fixtures/rust-sample

# Callers — depth 2, human forest
cgx callers rust_sample::conditions::dispatch --repo fixtures/rust-sample

# Callees
cgx callees rust_sample::conditions::dispatch --repo fixtures/rust-sample

# Paths between two symbols
cgx paths rust_sample::conditions::dispatch rust_sample::conditions::log_error \
  --repo fixtures/rust-sample

# Reaches — witness path
cgx reaches rust_sample::conditions::dispatch rust_sample::conditions::log_error \
  --repo fixtures/rust-sample

# Explain a symbol
cgx explain rust_sample::conditions::dispatch --repo fixtures/rust-sample

# Search by substring
cgx search dispatch --repo fixtures/rust-sample --limit 5

# Search — list every symbol (no pattern)
cgx search --all --repo fixtures/rust-sample --limit 0

# Symbols — top-N by reference count (in+out)
cgx symbols --rank inbound --top 10 --repo fixtures/rust-sample

# Unused functions
cgx unused --kind function --repo fixtures/rust-sample

# CQL query
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.fqn = "rust_sample::conditions::dispatch" RETURN a.name, a.file LIMIT 5' \
  --repo fixtures/rust-sample

# Dataflow — forward slice from a value node
# (requires an index built with dataflow: cgx index <path>)
cgx flows-to "rust_sample::dataflow::flow_example::b#1" --repo <path-indexed-with-dataflow>

# Dataflow — backward pedigree
cgx flows-from "rust_sample::dataflow::flow_example::b#1" --repo <path-indexed-with-dataflow>

# Index health
# (`doctor` never auto-indexes: run `cgx index` first or it exits 3)
cgx doctor --repo fixtures/rust-sample

# Co-change coupling over a commit range — reads git history, not the index
# (BASE is exclusive, HEAD inclusive; needs at least two commits in the range)
cgx coupling HEAD~1 HEAD --min-cochanges 1 --limit 5
```

Real `coupling` output from the last of those, against a fixture whose most recent commit
touched two files (run twice, byte-identical; the `approximation:` line is elided at `…`):

```
HEAD~1 (f027a3e)..HEAD (9268906)
1 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
cochanges  a-changes  b-changes  files
        1          1          1  fixtures/rust-sample/src/conditions.rs  fixtures/rust-sample/src/dataflow.rs
approximation: over- and under-approximate — co-change is attributed at file granularity; …
```

`--min-cochanges 1` is what makes that pair visible: the **default is 2**, so a pair that
co-changed exactly once is filtered out and the same command prints `(no co-changed pairs)`. An
empty coupling answer usually means the threshold, the range, or a shallow clone — not that the
files are independent. Note also the absent `freshness:` line: `coupling` never opens the index.

> **`flows-to` / `flows-from` require value-node FQNs** (form: `<fn>::<local>#<ver>`),
> not function symbols. Use `cgx search <name> --kind variable` to find the exact FQN.
> The index must have been built without `--no-dataflow` (the default); on a `--no-dataflow`
> index, value nodes are absent and the symbol does not resolve (exit 2).

> **`search --all`** lists every symbol with no pattern — the explicit, discoverable form of
> a match-everything search (use it instead of `cgx search '.' --regex`). `--all` is mutually
> exclusive with a `<PATTERN>` (passing both → exit 2) and composes with `--kind`/`--limit`/`--format`.

---

## See also

- [`docs/commands/`](../../../docs/commands/) — per-command pages with full flag tables
- `reference/query-language.md` — supported and unsupported CQL clauses
- `reference/output-and-exit.md` — format options, exit-code contract, CI assertion mode
- `reference/mcp.md` — MCP tool schemas, MCP-vs-CLI defaults, pagination
- `reference/versions.md` — version ladder and capability since-tags
