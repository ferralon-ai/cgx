# cgx Query Capabilities

**Status:** Draft  
**Audience:** Software engineers, security engineers, AI coding agents  
**Cross-references:** `docs/03-code-graph-model.md` (node/edge taxonomy, edge condition labels), `docs/04-dataflow-and-provenance.md` (DF- features), `docs/07-interfaces.md` (CLI UX, output formats)

---

## Overview

`cgx` answers graph questions about source code through a two-layer query interface:

- **Layer 1 — Focused subcommands.** Common questions expressed as dedicated commands with no query-language syntax overhead. Subcommands cover the most frequent patterns: who calls what, what paths connect two points, what is never called.
- **Layer 2 — Full query language.** An escape hatch for questions that do not fit a subcommand. Cypher-subset syntax (ISO GQL-compatible) expressed inline or from a file.

Both layers produce identical result objects: every result node and edge carries file:line provenance and an edge condition label (`always`, `conditional`, `exception`, `loop`, `panic`) from the taxonomy in `docs/03-code-graph-model.md`. The labels `exception` and `panic` together form the exceptional class; query predicates that filter "non-exception paths" exclude both.

### What an answer is

A cgx answer is never a bare result set. Two envelopes travel with it, and reading them is part of reading the answer:

- **The approximation contract** — which direction this specific answer errs in (`exact`, `under-approximate`, `over-approximate`, `over- and under-approximate`), why, and what graph it was computed over. It is computed **per answer**, not per binary: the same subcommand is `exact` on one invocation and `under-approximate` on the next because a bound fired. Negative answers ("no path exists") additionally carry a `scope:` tail naming exactly what was searched, because an absence is only as strong as its search.
- **The freshness envelope** — whether the index the answer came from still matches the working tree (`current`, `stale`, `unknown`), which tree was indexed, and how many files are dirty.

In `human` format these are the last one or two lines; in `json` they are the `approximation` and `freshness` keys; in `sarif` they are the `cgx/approximation-contract` and `cgx/index-freshness` notes. **Which of the two a command emits is a per-command fact** — see "Answer envelopes by command" below. Confidence (`certain`/`probable`/`possible`) is a third, orthogonal axis: it labels individual *edges*, and a path carries the weakest confidence along it.

---

## Feature List

Shipped status reflects v0.3.0. Features without a "Planned" note are available in the current binary. "Shipped" here means the command or clause runs; it does not mean every flag the section below sketches for it exists — where a section describes a flag that is not in the binary, the section says so inline.

| ID | Feature | Layer | v0.3.0 status |
|----|---------|-------|---------------|
| Q-1 | `callers` subcommand | 1 | Shipped |
| Q-2 | `callees` subcommand | 1 | Shipped |
| Q-2b | `reaches` subcommand (forward enumeration, or a witness path to a target) | 1 | Shipped |
| Q-3 | `paths` subcommand | 1 | Shipped — but **no edge-condition filter on `paths`**; that filter exists only on `diff` |
| Q-4 | `unused` subcommand | 1 | Shipped |
| Q-4b | `search` / `symbols` subcommands (symbol-table lookup and degree ranking) | 1 | Shipped |
| Q-5a | `flows-from` subcommand (backward data-flow pedigree from a value node) | 1 | Shipped (`Since: v0.3`) |
| Q-5b | `flows-to` subcommand (forward data-flow walk from a value node) | 1 | Shipped (`Since: v0.3`) |
| Q-5 | `pedigree` subcommand (value provenance with `--at-function`) | 1 | **Planned** — use `flows-from` today |
| Q-6 | `explain` subcommand | 1 | Shipped |
| Q-7 | `diff` subcommand (`<BASE> <HEAD>` positional) | 1 | Shipped, including `--added/--removed/--changed`, `--kind`, `--edge-condition`, `--from/--to`, `--path-added`, `--require-anchor-match` |
| Q-7b | `coupling` subcommand (file-level co-change frequency over git history) | 1 | Shipped |
| Q-8 | Full query language: `cgx query '<q>' --repo <path>` | 2 | Shipped (CALLS + DATA_FLOW) |
| Q-9 | Query from file: `cgx query @file.cql --repo <path>` | 2 | Shipped |
| Q-10 | SQL alternative: `cgx query --sql 'WITH RECURSIVE ...'` | 2 | **Planned** — the flag is parsed and **rejected with exit 2** on `query` |
| Q-11 | Edge-condition filters | 1 + 2 | Shipped in CQL (`r.condition`) and on `cgx diff --edge-condition`. **Not** on `callers`/`callees`/`paths` |
| Q-12 | Path-relative transience predicates | 2 | Shipped |
| Q-13 | Depth limits (`--depth`) | 1 + 2 | Shipped. Fan-out limits (`--max-fanout`, `--through-sentinels`) are **Planned** |
| Q-14 | Negative path constraints (`reaches X WITHOUT passing through Y`) | 2 | Shipped |
| Q-15 | Reachability matrix queries | 2 | Shipped |
| Q-16 | Graph diff queries across commit or branch ranges | 1 + 2 | Shipped; see Q-7 for the real flag set |
| Q-17 | Determinism guarantee: stable ordering, query against a specific commit (`--at`) | 1 + 2 | Shipped. `--order` is **Planned** (no such flag) |
| Q-18 | Confidence-tier filtering (`--confidence certain\|probable\|possible`) | 1 + 2 | Shipped |
| Q-19 | Entrypoint-scoped reachability | 1 + 2 | **Planned** — `cgx unused` uses the *auto-detected* entrypoint set; there is no flag to declare or scope one |
| Q-20 | Path quantifiers and must-pass-through (∀-path / must-analysis) | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-21 | Path-set algebra (complement, intersection, difference) | 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-22 | Ordering and pairing predicates (A-then-B on all paths; acquire/release) | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-23 | Typed taint queries (class-matched source → sink with sanitizer-class clearing) | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-24 | Concurrency queries (lock sets, await-holding-lock, blocking-in-async, cross-spawn races) | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-25 | Dependency and CVE reachability queries | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-26 | Lineage type reconstruction queries (type discovery; type-contradiction bug query) | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-27 | Mutation fan-out and exposed-state queries | 1 + 2 | **Partly shipped** — `CALL cgx.mutation_fanout(v) YIELD mutator, confidence` runs; the `effect`/`transform`/`evidence` columns, the `mutation-fanout` subcommand and the `writes-param(i)` lattice are Planned |
| Q-28 | Closure-capture queries (loop-variable capture; captured-resource lifetime) | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-29 | Higher-order / function-value call-resolution queries | 1 + 2 | **Partly shipped** — `calls:callback`/`calls:indirect` edges are queryable; function-value nodes and indirect-call *resolution* are Planned |
| Q-30 | Coercion and type-confidence queries (type-juggling-in-auth; `any`-frontier) | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-31 | Framework-aware queries (metadata-guard-aware must-pass-through; entrypoint reachability; tainted-reflection dispatch) | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-32 | Override-contract drift queries (exception-widening, dropped-base-guard, field-footprint drift) | 2 | **Planned (not yet shipped in v0.3.0)** |
| Q-33 | Recursion / SCC / architecture-cycle queries | 1 + 2 | **Planned (not yet shipped in v0.3.0)** |

---

## Layer 1: Focused Subcommands

### How a subcommand is addressed to a repository

**`--repo <path>`, not a trailing positional path.** Every query subcommand — `callers`, `callees`, `reaches`, `paths`, `unused`, `explain`, `query`, `search`, `symbols`, `flows-to`, `flows-from`, `diff`, `coupling`, `doctor` — takes the repository root as the **`--repo` option**, defaulting to the current directory. A trailing positional path is a usage error:

```console
$ cgx callers helper ./
error: unexpected argument './' found

Usage: cgx callers [OPTIONS] <SYMBOL>

For more information, try '--help'.
$ echo $?
2
```

**`cgx index` is the exception, in both directions.** It takes a **positional** `[PATH]` and has no `--repo` at all:

```console
$ cgx index --repo /path/to/repo
error: unexpected argument '--repo' found

  tip: to pass '--repo' as a value, use '-- --repo'

Usage: cgx index [OPTIONS] [PATH]
$ echo $?
2
```

The graph index is located from the resolved repository root (see `docs/06-indexing-and-vcs.md`). Every command in this section except `index`, `coupling` and `mcp` auto-indexes when no index is present; `--no-auto-index` turns that off, and `cgx doctor` never auto-indexes at all (it exits 3 on an unindexed repo).

### The example repository

Every run shown in this document is against this repository, which you can recreate exactly:

```rust
// src/main.rs
fn main() {
    let cfg = load_config();
    let raw = read_input(cfg);
    let out = middle(raw);
    danger(out);
}

fn load_config() -> u32 { 7 }

fn read_input(c: u32) -> u32 {
    let x = c + 1;
    x
}

fn middle(v: u32) -> u32 {
    let y = helper(v);
    y
}

fn helper(x: u32) -> u32 { x * 2 }

fn danger(v: u32) {
    if v > 3 {
        risky(v);
    }
}

fn risky(v: u32) -> u32 { v }

fn orphan() -> u32 { 42 }
```

```console
$ git init -q . && git add -A && git commit -qm initial
$ cgx index .
Indexed /tmp/fixture (7380e245ab98db2dfed12ed3ee3b005f0fbf7036)
  blobs: 1 indexed, 1 extracted, 0 cached, 0 unsupported
  graph: 22 nodes, 14 edges, 0 unresolved
  dataflow: 5 fns recomputed, 0 fns reused; ifds: 4 summaries, 3 interproc edges, 0 budget-exceeded SCCs
```

(The absolute path in the first line is the repository root; `/tmp/fixture` stands in for whatever yours is. Everything else is verbatim.)

Twenty-two nodes for eight functions: the extra fourteen are **SSA value nodes** (`rust_sample::middle::y#1` and friends), which the dataflow pass creates and which `flows-to`/`flows-from` operate on. They are ordinary graph nodes, so they show up in `search`, `symbols` and `unused` output too.

### Q-1: callers

Find all symbols that call a given symbol, up to a specified depth.

```
cgx callers <SYMBOL> [--repo <path>] [--depth N]
                     [--confidence certain|probable|possible]
                     [--tree full|spanning] [--at <ref>] [--format <fmt>]
                     [--assert-empty] [--allow-vacuous] [--no-auto-index]
```

`callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `query` and `unused` share this flag block. Note what is **not** in it: there is no `--edge-condition` filter on any traversal subcommand (that flag exists only on `cgx diff`), and no `--max-fanout` / `--through-sentinels` — fan-out bounding is Planned, specified in Q-13.

**Worked run:**

```console
$ cgx callers helper --repo .
rust_sample::helper  src/main.rs:20
└─ rust_sample::middle  src/main.rs:15
   └─ rust_sample::main  src/main.rs:1
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

Human output is an **indented forest**, not a flat list: depth is carried by indentation, and there is no `depth=` field. A node annotated `[if]` was reached over a `conditional` edge and `[exc]` over an `exception` edge; `always` is left unmarked. Confidence is annotated only when it is not `certain` (`[probable]`, `[possible]`). JSON and SARIF do carry a numeric `depth` per result.

**`--depth` on `callers` and `callees` defaults to 2, and `--depth 0` is not "unlimited":**

```console
$ cgx callers helper --repo . --depth 0
rust_sample::helper  src/main.rs:20
approximation: under-approximate — search stopped at depth 0; deeper edges were not explored | scope: call edges, confidence>=possible, depth<=0
freshness: current | indexed tree 7380e24, working tree clean
```

`--depth 0` returns the seed node and nothing else, and the contract says so. `--depth 0` lifts the depth limit on **`cgx paths` only** (Q-3) — and even there "unlimited" needs the qualification in Q-3, because lifting the depth limit hands the bound to the work budget instead. The CLI's own `--help` text for `--depth` states the unlimited reading unconditionally and is wrong for every command except `paths`; the contract line is the reliable signal.

### Q-2: callees

Find all symbols a given symbol calls, transitively to a depth limit. Same flag block as Q-1.

```
cgx callees <SYMBOL> [--repo <path>] [--depth N] [--confidence …] [--at <ref>] [--format <fmt>]
```

**Worked run:**

```console
$ cgx callees main --repo . --depth 3
rust_sample::main  src/main.rs:1
├─ rust_sample::danger  src/main.rs:22
│  └─ rust_sample::risky  src/main.rs:28  [if]
├─ rust_sample::load_config  src/main.rs:8
├─ rust_sample::middle  src/main.rs:15
│  └─ rust_sample::helper  src/main.rs:20
└─ rust_sample::read_input  src/main.rs:10
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

`risky` carries `[if]` because `danger` only calls it inside an `if`. That is the edge-condition label doing its job: the path exists, but it is not unconditional.

`--tree full` (the default) repeats a node under each of its callers; `--tree spanning` renders each node once under its shortest-path parent with a `(+N call sites)` annotation.

### Q-2b: reaches

Answer "does `FROM` reach `TO`?" with a witness path, or enumerate everything `FROM` reaches when `TO` is omitted. The two forms have **different result shapes and different depth defaults**, which matters more than the shared name suggests:

```
cgx reaches <FROM> [TO] [--repo <path>] [--depth N] [--confidence …] [--at <ref>] [--format <fmt>]
```

| Form | Result | Default depth | `dot`/`mermaid`/`d2` |
|---|---|---|---|
| `cgx reaches <FROM>` | forest enumeration | 2 | rejected, exit 2 |
| `cgx reaches <FROM> <TO>` | witness path | unlimited | accepted |

```console
$ cgx reaches main risky --repo .
path 1 (2 hops, min-confidence=certain):
     rust_sample::main  (src/main.rs:1)
  -> rust_sample::danger  (src/main.rs:22) [always]
  -> rust_sample::risky  (src/main.rs:28) [conditional]
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

**Direction matters and is easy to get backwards.** `cgx reaches <FROM>` with no `TO` walks *forward* — what `FROM` calls. To ask "is this vulnerable function reachable from anything," that is the wrong direction; use `cgx callers <SYMBOL>` or give `reaches` both anchors.

### Q-3: paths

Enumerate the call paths between a source and a sink. **Both anchors are positional**; there are no `--from`/`--to` flags on `paths` (those exist on `cgx diff`, where they are glob anchors for `--path-added`).

```
cgx paths <FROM> <TO> [--repo <path>] [--depth N] [--confidence …]
                      [--at <ref>] [--format <fmt>] [--assert-empty] [--allow-vacuous]
```

```console
$ cgx paths --from main --to risky --repo .
error: unexpected argument '--from' found

  tip: a similar argument exists: '--format'

Usage: cgx paths --format <FORMAT> <FROM> <TO>
```

**Worked run:**

```console
$ cgx paths main risky --repo .
path 1 (2 hops, min-confidence=certain):
     rust_sample::main  (src/main.rs:1)
  -> rust_sample::danger  (src/main.rs:22) [always]
  -> rust_sample::risky  (src/main.rs:28) [conditional]
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

`min-confidence=certain` on the header line is the **weakest** confidence anywhere along the path — a path is only as trustworthy as its least trustworthy hop.

`paths` defaults to depth **6**, and is the one command where `--depth 0` lifts the depth limit.

**`--depth 0` is not "get everything", and it can return *less* than a bounded search.** Lifting
the depth limit does not remove the bound — it moves it to the shared work budget, which then
truncates. On a graph with a deep subtree, the same query goes from a provably complete answer
to a truncated one purely by passing `--depth 0`:

```console
$ cgx paths main sink --repo . --depth 6 --format json   # count=1, truncated=false, approximation.direction="exact"
$ cgx paths main sink --repo . --depth 0 --format json   # count=1, truncated=true,  truncation_reason="step-budget", direction="under"
$ cgx paths main sink --repo . --depth 0
…
[truncated: search budget exhausted; narrow with --depth]
approximation: under-approximate — path enumeration hit the work budget; more paths may exist
freshness: current | indexed tree 0b6795c, working tree clean
```

(Run against a deliberately deep fixture, not the example repository above; `…` elides the path
listing.) The budget is spent depth-first, so the deep subtree can consume it before shallow
paths are enumerated — **a smaller result than the bounded query, including zero results, is a
realistic outcome.** A reader who takes an empty `--depth 0` result as "no path exists" has it
backwards.

This is why the truncation signals are load-bearing rather than cosmetic: the `[truncated: …]`
marker line, the `truncated` / `truncation_reason` JSON keys, and the contract flipping from
`exact` to `under-approximate` are how a complete answer is distinguished from an exhausted one.
Check the contract, not the row count.

`paths` is also one of the two path-shaped result sets (with `reaches <FROM> <TO>`), so it is the place `--format dot|mermaid|d2` is accepted. On a non-path-shaped result those three formats are a **hard usage error, exit 2** — not a degraded rendering.

### Q-4: unused

Find symbols not reachable from any auto-detected entrypoint.

```
cgx unused [--repo <path>] [--kind <KIND>] [--depth N] [--confidence …]
           [--at <ref>] [--format <fmt>] [--assert-empty] [--allow-vacuous]
```

`--kind` takes one of the ten symbol kinds — `function method type field variable module constant macro lambda entrypoint` — and is the same enum `search` and `symbols` use. There is no `--type <TypeName>` flag and no `--entrypoint <symbol>` flag; entrypoint scoping is Q-19, which is Planned.

**Worked run:**

```console
$ cgx unused --repo . --kind function
rust_sample::orphan  (src/main.rs:30)
approximation: exact (within modeled graph) | scope: call edges, confidence>=possible, depth=unbounded
freshness: current | indexed tree 7380e24, working tree clean
```

The `| scope:` tail is the important half here. `unused` is a **negative** answer — "nothing calls this" — and the scope names precisely what was searched, so a reader can see that a caller reaching `orphan` through a mechanism outside the modeled call graph (reflection, a dynamic registry, an FFI boundary) is *not* excluded by this result.

**`--kind function` is not decoration.** Without it, SSA value nodes dominate the output on a dataflow index, because a local that is written and never read is genuinely a node with no incoming call edge:

```console
$ cgx unused --repo .
rust_sample::main::cfg#1  (src/main.rs:2)
rust_sample::main::raw#1  (src/main.rs:3)
rust_sample::main::out#1  (src/main.rs:4)
rust_sample::read_input::c#0  (src/main.rs:10)
…
rust_sample::risky::v#0  (src/main.rs:28)
rust_sample::orphan  (src/main.rs:30)
approximation: exact (within modeled graph) | scope: call edges, confidence>=possible, depth=unbounded
freshness: current | indexed tree 7380e24, working tree clean
```

(Rows 5–13, all value nodes, elided at the `…`; the sole function is the last row.) Any `unused` example that shows only functions was produced either with `--kind function` or against an index built `--no-dataflow`.

### Q-5: pedigree — **Planned (not yet shipped in v0.3.0)**

**What ships today: `flows-from` and `flows-to`.** Both operate on **value nodes** (`rust_sample::middle::y#1`), not on function symbols — a function FQN has no `derives-from` edges and resolves to an empty walk. Discover value-node FQNs with `cgx search`:

```console
$ cgx search y --repo .
rust_sample::middle::y#1      src/main.rs:16  [variable]
rust_sample::risky            src/main.rs:28  [function]
rust_sample::risky::return#1  src/main.rs:28  [variable]
rust_sample::risky::v#0       src/main.rs:28  [variable]
freshness: current | indexed tree 7380e24, working tree clean
```

Four hits for one letter: `search` matches a **case-insensitive substring of the whole FQN** by default, so `y` also matches `risky`. Use `--regex` to anchor (`--regex '.*::y#\d+$'`), or `--kind variable` to drop the function row.

```console
$ cgx flows-from 'rust_sample::middle::y#1' --repo .
rust_sample::middle::y#1  src/main.rs:16
└─ rust_sample::middle::v#0  src/main.rs:15  [probable]
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

`y` derives from `v`, `middle`'s parameter — and the edge is `[probable]`, not `certain`. That band is the honest one: the dataflow pass reconstructed the link without a precise type resolution, and the doc-level rule is that a `probable` edge is never described as an established fact.

**The two directions are named against intuition, and the CLI's own definition is the one to trust.** `flows-from` is the *pedigree* (backward: where did this value come from). `flows-to` is the *forward slice* (what does this value flow into). Internally `flows-from` walks `derives-from` forward and `flows-to` walks it backward, because a `derives-from` edge points from the derived value to its source.

On an index built with `cgx index --no-dataflow` the value nodes do not exist and both commands exit 2 on symbol resolution.

**Planned:** a `pedigree` subcommand addressing values by their source-level name within a function scope, rather than by SSA FQN. Nothing below this line is runnable.

Trace all inbound provenance for a value at a named program point: which callers, fields, return values, and transformations populate it. This is the fan-in slice of `docs/04-dataflow-and-provenance.md` DF-* features.

```
cgx pedigree <symbol-or-var> <path>
             [--at-function <fn>]
             [--depth N]
             [--max-fanout N] [--through-sentinels]
             [--format <fmt>]
```

A pedigree traversal stops at sentinel (hub) nodes by default and reports them
as leaves with their degree (the summarised-pedigree default generalised in
docs/04-dataflow-and-provenance.md DF-1.4); `--through-sentinels` expands through
them and `--max-fanout N` caps per-node breadth. See Q-13.

**Planned examples (not yet runnable):**

```bash
# Where does the value of `user_input` come from in AuthHandler::handle?
cgx pedigree user_input --repo . --at-function AuthHandler::handle

# Full provenance tree for config object in init_database
cgx pedigree config --repo . --at-function init_database --depth 5
```

### Q-6: explain

Show the full provenance record for a single symbol: its definition location, caller and callee counts, and every direct incident edge with its condition label, confidence, resolution tier and resolution rule.

```
cgx explain <SYMBOL> [--repo <path>] [--format <fmt>] [--no-auto-index]
```

`explain` does **not** take the shared flag block: no `--at`, no `--depth`, no `--confidence`, no `--assert-empty`. It is a one-symbol, one-hop provenance dump, not a traversal.

**Worked run:**

```console
$ cgx explain helper --repo .
rust_sample::helper  (src/main.rs:20)
  kind: function
  callers: 1, callees: 0
  edges:
    <- rust_sample::middle  (src/main.rs:15)  [always]  [certain]  tier=scope_graph  rule=scope-ref  site=src/main.rs:15
freshness: current | indexed tree 7380e24, working tree clean
```

`<-` is an incoming edge, `->` outgoing. `tier=` is the resolution tier that produced the edge (`name_syntactic`, `scope_graph`, `scip`, `cha_rta`, `points_to`) and `rule=` is the specific rule that fired. This is the command to reach for when you want to know *why* cgx believes an edge exists — and it is the only place the resolution tier is surfaced on the CLI.

`explain` carries the **freshness envelope but no approximation contract**: it reports what the index holds about one symbol rather than answering a search, so there is no search whose direction of error needs stating.

### Q-7: diff

Compute the graph diff between two git refs: which edges and nodes were added, removed, or changed.

```
cgx diff <BASE> <HEAD> [--repo <path>] [--format human|json]
                       [--added] [--removed] [--changed] [--newer-than]
                       [--kind <KIND>]… [--edge-condition always|conditional|loop|exception|panic]
                       [--from <glob>] [--to <glob>]
                       [--path-added] [--require-anchor-match]
```

`<BASE>` and `<HEAD>` are **positional**; there are no `--base`/`--head` flags. With none of `--added`/`--removed`/`--changed`, all buckets print; `--newer-than` is sugar for `--added`. `--kind` is repeatable and accepts nineteen edge-kind tokens (`calls`, `calls-virtual`, … , `data-flow`, `reads-field`, `writes-field`). `--edge-condition` is real and shipped **here** — `diff` is the only command that has it.

Two commits were added to the example repository for this section: one adding `fn sneaky() { risky(1); }` and a `tests/t.rs`, then three commits touching both files.

**Full mode:**

```console
$ cgx diff HEAD~1 HEAD --repo .
+ edge  rust_sample::sneaky  ->  rust_sample::risky  (src/main.rs)
+ node  rust_sample::sneaky
```

`+` added, `-` removed, `~` changed; a clean diff prints `(no diff)`.

**`--path-added` — the CI gate.** This mode asks a narrower and much more useful question: *did this change introduce a new reachability path between these two anchors?* Both `--from` and `--to` are **required** here (a missing anchor is exit 2), and they are globs over FQNs.

```console
$ cgx diff HEAD~1 HEAD --repo . --path-added --from 'rust_sample::sneaky' --to 'rust_sample::risky'
1 new call/dataflow reachability path(s) [reachability, not a security guarantee]:
+ path  rust_sample::sneaky  ->  rust_sample::risky  [introduced by 125264854eef (t)]
cgx: path-added gate: 1 new call/dataflow reachability path(s) found
$ echo $?
1
```

**Exit 1 when a new path is found, exit 0 when clean** — that inversion is what makes it usable as a gate step without parsing output. The bracketed disclaimer in the output is not boilerplate: a new path is a *reachability* fact, not evidence of exploitability.

`--path-added` is also where git attribution surfaces. `introduced by <12-hex> (<author>)` comes from a blame walk; `--format json` adds `introducing_commit`, `introducing_author`, `introducing_email` and `author_time` per path. An unattributable path reads `introducing commit unattributed`.

**Format support differs between the two modes**, and neither accepts `sarif`:

| Mode | Accepted `--format` | Rejected (exit 2) |
|---|---|---|
| full `cgx diff BASE HEAD` | `human`, `json` | `sarif`, `dot`, `mermaid`, `d2` |
| `--path-added` | `human`, `json`, `dot`, `mermaid`, `d2` | `sarif` |

The check runs before any indexing, so a bad `--format` fails fast.

**`diff` carries neither the approximation contract nor the freshness envelope.** It computes both sides from committed trees and never consults the working-tree index, so there is no index-freshness question to answer — see "Answer envelopes by command" below.

**Planned (v0.4):** `--calls-to <symbol>` and `--taint-class <class>` security-gate filters. Neither exists today.

### Q-7b: coupling

Ask which files historically change together. `coupling` walks committed git history over an explicit rev range and reports, for every pair of files touched by the same commit, the co-change count and each file's own change count.

```
cgx coupling <BASE> <HEAD> [--repo <path>] [--format human|json]
                           [--limit N] [--min-cochanges N] [--max-files-per-commit N]
```

`<BASE>` is **exclusive** and `<HEAD>` inclusive. `--format` accepts `human` and `json`; the other four are exit 2 (`Sarif format is not available for 'coupling' (a co-change table is neither a locatable finding nor a path graph)`).

**`coupling` needs no index.** It reads git history only — no extraction, no graph, no `.cgx/`. That is why it is the one query command that emits an approximation contract but **no freshness envelope**: there is no index whose staleness could be reported.

```console
$ cgx coupling HEAD~4 HEAD --repo .
HEAD~4 (7ef5392)..HEAD (d3429b0)
4 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
cochanges  a-changes  b-changes  files
        4          4          4  src/main.rs  tests/t.rs
approximation: over- and under-approximate — co-change is attributed at file granularity; two files reported as co-changing may have had unrelated symbols edited in the same commit; only the 4 single-parent commit(s) in HEAD~4 (7ef5392421f668900a3f6f4a886ea840a39ed817)..HEAD (d3429b0e205f8b3336d19b450c221b7356580f03) were walked; co-change outside this range is not visible; rename detection is disabled so the walk cannot depend on ambient git config; a renamed file appears as an unrelated delete and add, reporting a co-change pair between its old and new path, and the new path does not inherit the old path's history; pairs with fewer than 2 co-changes are not reported
```

The contract is long because the answer really is approximate, and each reason changes how you
should read a row. **These four are the ones this example emits**; three of them
(`file-level-granularity`, `bounded-rev-range`, `renames-not-tracked`) are unconditional and
`cochange-threshold` fires whenever `--min-cochanges` is above zero:

| Reason code | Direction | What it means for the result |
|---|---|---|
| `file-level-granularity` | over | A pair may co-change because unrelated symbols in those files were edited together. This is a file-level signal, not a symbol-level one. |
| `bounded-rev-range` | under | Only single-parent commits inside `BASE..HEAD` were walked. Coupling outside the range is invisible. |
| `renames-not-tracked` | over | Rename detection is deliberately off so the walk cannot depend on ambient `diff.*` config. A rename reads as a delete plus an add. |
| `cochange-threshold` | under | Pairs below `--min-cochanges` (default 2) are not reported at all. |

**The full set of reason codes is larger and conditional.** Seven more appear only when their
condition holds: `merge-commits-excluded`, `root-commits-excluded`, `large-commit-excluded`,
`result-limit`, `shallow-repository`, `history-boundary` and `empty-rev-range`. Read the
`reasons` array in `--format json`, keyed on `code`, rather than assuming a fixed set — a new
code can appear without any existing one changing.

### The shallow-clone trap, and how to detect it

The second-line census — `4 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded` — is how you check that the range you asked for is the range that was walked. **On a shallow clone it will not be.** `coupling` paints the base commit's full ancestry eagerly and stops at the graft, so a range that appears to sit well inside `--depth N` can return an empty answer **at exit 0**. Since `fetch-depth: 1` is the default for `actions/checkout`, a CI job that gates on exit code alone will read "no coupling here" from a checkout that simply had no history to walk.

**This is detectable rather than something to catch by eye.** Human output carries a
`degraded:` line whenever the walk truncated or the clone is shallow — on a `--depth 2` clone
the whole answer is that line:

```console
$ cgx coupling HEAD~1 HEAD --repo .
HEAD~1 (ae36d86)..HEAD (d3429b0)
0 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
degraded: walk truncated at a history boundary (missing or unreadable object) · shallow clone — deepen the clone to see more history
(no co-changed pairs)
approximation: over- and under-approximate — …
$ echo $?
0
```

Zero commits, zero pairs, exit 0. For scripts, the same condition is a named
under-approximation reason:

```json
{ "direction": "under",
  "code": "shallow-repository",
  "detail": "the repository is a shallow clone; history before the graft boundary is absent and its co-changes are not counted" }
```

So the correct CI gate is a check on the reason codes, not on the exit status:

```bash
cgx coupling "$BASE" HEAD --repo . --format json > coupling.json
jq -e '.approximation.reasons | any(.code == "shallow-repository")' coupling.json \
  && { echo "refusing to trust coupling on a shallow clone"; exit 1; }
```

`history-boundary` (the walk stopped at a missing or unreadable object) and `empty-rev-range`
deserve the same treatment. `git fetch --unshallow` before running is the other fix. The
report's `shallow_repository` and `truncated_at_history_boundary` booleans carry the same
signal if you prefer a top-level field to a reason lookup.

### Answer envelopes by command

"Every cgx answer carries both envelopes" is a convenient sentence and it is not true. Which envelope a command emits follows from what the command actually does, and a script that unconditionally reads `.approximation` will `null`-deref on a third of the surface. Verified by running each command in `human` format and counting the trailing lines:

| Command | approximation | freshness | Why |
|---|---|---|---|
| `callers`, `callees`, `reaches`, `paths`, `unused`, `flows-to`, `flows-from`, `query` | yes | yes | A bounded search over an index: both the search's error direction and the index's currency are in question |
| `explain` | no | yes | Reports what the index holds about one symbol; no search to characterise |
| `search`, `symbols` | no | yes | Symbol-table lookups, not graph traversals |
| `coupling` | yes | **no** | Approximate in several named ways, but never opens the index |
| `diff` (both modes) | no | no | Both sides come from committed trees; the working-tree index is not consulted |
| `doctor` | no | no | Reports *on* the index rather than answering from it |

The same split holds in `json` (`approximation` / `freshness` keys) and in `sarif` (`cgx/approximation-contract` / `cgx/index-freshness` notes). One more asymmetry worth knowing: under `--format dot|mermaid|d2` the freshness envelope is deliberately **omitted** rather than computed — emitting a graph does not justify the working-tree walk, so an uninspected envelope is substituted.

---

## Layer 2: Full Query Language

The `cgx query` command accepts a Cypher-subset expression (ISO GQL-compatible) or a file reference. The query language is parsed and evaluated in-process against the indexed graph — no external graph engine is required.

```
cgx query '<expression>' [--repo <path>] [--at <ref>] [--format <fmt>] [--depth N] [--confidence …]
cgx query @file.cql      [--repo <path>] [--at <ref>] [--format <fmt>]
```

`query` takes the same shared flag block as `callers` (Q-1). `--sql` is Planned; passing it is **exit 2**, not a silent fallback.

**A `RETURN path` query routes through the same path renderer as `cgx paths`**, so its `human`/`json`/`sarif`/`dot`/`mermaid`/`d2` output is identical in shape to Layer 1's. A tabular `RETURN a.fqn, b.fqn` query renders aligned columns, with the header printed even for zero rows.

### Query language syntax

Pattern: `MATCH (a)-[r:EDGE_TYPE*min..max]->(b) WHERE <filter> RETURN <projection>`

**A `MATCH` pattern must contain at least one relationship.** A bare single-node `MATCH (n)` is a plan error, not an empty result — several query classes that read naturally as "find all nodes where …" have to be phrased through an edge instead.

**Node properties available:**
- `name` — **the fully qualified name**, and `fqn` is an accepted synonym for the same field. This is the single most common source of a silently empty result: `WHERE a.name = "main"` matches nothing, because the stored value is `rust_sample::main`. Use the full FQN, or a glob in the inline property map:

```console
$ cgx query 'MATCH (a)-[:CALLS]->(b) WHERE a.name = "main" RETURN a.name, b.name' --repo .
a.name  b.name
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

```console
$ cgx query 'MATCH (a {name:"*::main"})-[:CALLS]->(b) RETURN a.fqn, b.fqn' --repo .
a.fqn              b.fqn
rust_sample::main  rust_sample::danger
rust_sample::main  rust_sample::load_config
rust_sample::main  rust_sample::middle
rust_sample::main  rust_sample::read_input
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

The first query is a clean exit 0 with a header and no rows. That is the failure mode to watch for: **an empty CQL result is not evidence of absence** until you have confirmed the anchor binds. Multi-key inline predicates (`{kind:"function", name:"*::main"}`) are supported and are usually the clearer form.

- `file` — file path relative to root
- `line` — source location (start line). **`col` is not a node property** and is a plan error.
- `kind` — one of the ten symbol kinds (`function`, `method`, `type`, `field`, `variable`, `module`, `constant`, `macro`, `lambda`, `entrypoint`)

**Not node properties, and each returns a specific plan error rather than an empty result:** `confidence` (it is an *edge* attribute — use `r.confidence`), `in_degree` / `out_degree` (computed on demand by `cgx symbols`, never materialised on the node record, so they cannot be filtered or sorted in CQL), and `scope`, `col`, `type`, `is_exit`, `entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class`, `introduced_in_branch`, `suspends`, `lock_set`, `async`. To rank hub nodes today, use `cgx symbols --rank inbound|outbound|total` (Q-4b), which computes the degrees in one pass.

`own_effects` and `transitive_effects` are rejected too, but for a different reason worth keeping straight: **the data is there.** Both are populated `NodeRecord` fields — the GM-12 transitive closure runs on every index — and what is missing is only the way to ask. `transitive_effects` happens to be rejected with the shared "no backing field on a symbol node" message, which is misleading for these two.

**Edge types:**
- `CALLS` — direct or transitive call edge
- `DATA_FLOW` — value flow (from `docs/04-dataflow-and-provenance.md`)
- `MEMBER_OF` — member-of-type relationship
- `OVERRIDES`, `INHERITS`, `IMPLEMENTS` — structural object-model edges (lowercase canonical `edge_type` in GM-2.2; uppercase in MATCH patterns per the `member-of`/`MEMBER_OF` precedent)
- `CALLS:super`, `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` — Theme-13 object-model edges (GM-21..GM-25); uppercase in MATCH patterns only

**`CALLS*` transitive-closure semantics under cycles (finding 2.3):**

`CALLS*` (and `CALLS*min..max`) uses simple-path semantics: each node is visited at most once per traversal. When the call graph contains a recursive cycle — directly recursive functions, or mutually recursive groups — the traversal terminates after visiting each node in the cycle exactly once and does not loop. This guarantees finite termination regardless of cycle structure. The default upper bound when no `max` is specified is the longest simple path in the graph — **not** the `--depth` flag, which `cgx query` ignores entirely (see "Two traps in the query surface"). An unbounded `CALLS*` on a large graph is the expensive case; bound it explicitly. Users may supply an explicit bound `[:CALLS*1..10]` to cap traversal depth independently of the simple-path guarantee.

Implications for Q-15 reachability-matrix queries: `count(*) over CALLS*` counts distinct reachable nodes (each counted once under simple-path semantics), not path lengths or traversal steps. A pair of mutually recursive functions `f ↔ g` appears as two reachable nodes, not an infinite count.

**Edge properties — exactly three:**
- `condition` — `"always"`, `"conditional"`, `"exception"`, `"loop"`, `"panic"`
- `confidence` — `"certain"`, `"probable"`, `"possible"`
- `kind` — the edge-kind token (`"calls"`, `"derives-from"`, `"throws"`, …)

`r.transformation` is rejected with a specific message (the tags exist in the schema but no frontend populates them); `r.via`, `r.taint_label` and `r.site` are rejected as having no backing field. Note that the edge-condition property is `r.condition`, **not** `r.edge_condition` — the latter is an unknown-property plan error:

```console
$ cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE r.condition = "conditional" RETURN a.fqn, b.fqn, r.confidence' --repo .
a.fqn                b.fqn               r.confidence
rust_sample::danger  rust_sample::risky  certain
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

**Path functions:**
- `nodes(path)` — list of nodes on a path
- `relationships(path)` — list of edges on a path
- `length(path)` — hop count
- `position_in(node, path)` — 0-based index of `node` within `path`; used by the ordering predicates in Q-22 and the TOCTOU query in Q-24. `position_in` is defined over the symbol-node sequence of a path; call-site nodes never appear in `nodes(path)`. Site-level facts are accessed via `r.site.<attr>` on the path's relationships.
- `last_node(path)` — the final node of `path` (e.g. `last_node(path).is_exit`); used by the acquire/release query in Q-22

**Aggregates over list comprehensions:**
- `MIN([e IN relationships(path) | e.<attr>])` / `MAX(...)` — minimum/maximum over a projected list. For `confidence`, the ordering is `certain > probable > possible`, so `MIN` returns the weakest-link confidence on a path (see Q-25 Worked Example 5).

**Predicates:**
- `NONE(n IN nodes(path) WHERE <condition>)` — negative path constraint
- `ANY(r IN relationships(path) WHERE <condition>)` — path-relative transience check

**Planned predicates, and the two different ways they fail.** `<expr> IS EMPTY` (empty reconstructed candidate set, for Q-26) reaches the planner and is rejected with the deferred-feature message: ``plan error: `IS EMPTY` / `IS NULL` predicates are not supported in this release``. `<type> COMPATIBLE_WITH <type>` (Q-26) and `STARTS WITH` / `ENDS WITH` / `CONTAINS` do not parse at all, so they surface as a bare ``parse error: expected WITH or RETURN, found identifier `COMPATIBLE_WITH` `` — informative about the syntax, silent about the roadmap. Both are exit 2; only the first tells you the feature is planned.

### Two traps in the query surface

Both produce a well-formed answer rather than an error, which is what makes them worth a section of their own. Everything in `cgx` that *cannot* work tells you so with a plan error and exit 2; these two do not.

**1. A variable shared across two `MATCH` clauses is not joined — it cross-products, and the second clause re-binds it.**

```console
$ cgx query 'MATCH (a {name:"*::main"})-[:CALLS]->(b) RETURN a.fqn, b.fqn' --repo .
a.fqn              b.fqn
rust_sample::main  rust_sample::danger
rust_sample::main  rust_sample::load_config
rust_sample::main  rust_sample::middle
rust_sample::main  rust_sample::read_input
```

Four rows, `b` ranging over `main`'s four callees. Now add a second clause that reads as "…and then what does `b` call?":

```console
$ cgx query 'MATCH (a {name:"*::main"})-[:CALLS]->(b) MATCH (b)-[:CALLS]->(c) RETURN a.fqn, b.fqn, c.fqn' --repo .
a.fqn              b.fqn                c.fqn
rust_sample::main  rust_sample::danger  rust_sample::risky
rust_sample::main  rust_sample::danger  rust_sample::risky
rust_sample::main  rust_sample::danger  rust_sample::risky
rust_sample::main  rust_sample::danger  rust_sample::risky
rust_sample::main  rust_sample::main    rust_sample::danger
…
```

Two things went wrong, and neither raised an error. `b` is bound **from scratch** by the second clause — row 5 has `b = rust_sample::main`, which was never in the first clause's binding set for `b`. And every combination appears with multiplicity four, the cardinality of the first clause: this is a cross-product of the two clauses, not a join. Twenty rows where the intended query has one.

**Express a multi-hop relationship as one pattern**, which is what the language is built for:

```console
$ cgx query 'MATCH (a {name:"*::main"})-[:CALLS]->(b)-[:CALLS]->(c) RETURN a.fqn, b.fqn, c.fqn' --repo .
a.fqn              b.fqn                c.fqn
rust_sample::main  rust_sample::danger  rust_sample::risky
rust_sample::main  rust_sample::middle  rust_sample::helper
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

Two rows, which is the correct answer. Interior nodes in a chained pattern *are* joined. Where you genuinely need two separate clauses, put a `WITH` between them — `WITH` is a real scope barrier and is the only construct that carries a binding from one clause to the next.

**Note for the Planned sections below.** Several of them (Q-24, Q-25, Q-28, Q-29, Q-32, Q-33) sketch queries that share a variable across two bare `MATCH` clauses. None of them can run today — each plan-errors on an unbacked property (`spawn_context`, `source_class`, `scc_id`, `lock_set`) or a parse error before the binding semantics matter, so no example in this document is currently returning a silent cross-product. They will need rewriting into single chained patterns, or around a `WITH`, when the properties they depend on land.

**2. `cgx query` silently ignores `--depth`, `--confidence` and `--tree`.**

They are part of the shared flag block, so clap accepts them, but `run_query` never reads them: the output is byte-identical with and without each. Verified on all four of `--depth 1`, `--depth 0`, `--confidence certain`, `--tree spanning`. Bound a CQL traversal inside the pattern instead — `[:CALLS*1..3]` — and filter confidence in the `WHERE` with `r.confidence`. (`--repo`, `--at`, `--format`, `--assert-empty` and `--allow-vacuous` *are* honoured.)

**A related cost note:** a var-length CQL expansion (`[:CALLS*n..m]`) is not bounded the way the Layer-1 `paths` walk is. `cgx paths` carries a work budget and reports `[truncated]` when it fires; an unbounded or wide `*n..m` expansion in CQL can run for a very long time on a large graph where `cgx paths` answers immediately. Bound the range explicitly, and prefer the Layer-1 subcommand when it can express the question.

### Procedure calls

Most traversals express directly as `MATCH` patterns. A small number of analyses compute a result by *intersecting* constraints gathered from several directions at once — they do not reduce to a single path pattern — and are exposed as named procedures invoked with the standard openCypher `CALL … YIELD` form:

```
CALL cgx.<procedure>(<args>) YIELD <columns>
```

The yielded columns bind like node properties and may be filtered in a following `WHERE` and returned in `RETURN`.

**Exactly two procedures are registered.** The engine's own error names them:

```console
$ cgx query 'CALL cgx.type_reconstruct("x") YIELD candidate_type RETURN candidate_type' --repo .
…
cgx: plan error: unknown procedure `cgx.type_reconstruct` (known: `cgx.mutation_fanout`, `cgx.pedigree`)
```

| Procedure | Yields | Walk |
|---|---|---|
| `cgx.pedigree(value)` | `source`, `confidence` | one direction along `derives-from` |
| `cgx.mutation_fanout(value)` | `mutator`, `confidence` | the other direction along `derives-from` |

Both are real `derives-from` BFS walks — neither is a stub. `effect`, `transform` and `evidence` are *not* yieldable columns and are rejected at plan time with a message that says so; drop them from the `YIELD` list and the base procedure runs today.

> **Known defect — the two procedures' directions are swapped relative to their column names.** Verified against the example repository, whose value chain is `middle::v#0` → `middle::y#1` → `middle::return#1`:
>
> ```console
> $ cgx query 'CALL cgx.pedigree("rust_sample::middle::y#1") YIELD source, confidence RETURN source, confidence' --repo .
> source                         confidence
> rust_sample::middle::return#1  certain
> ```
>
> `return#1` is what `y#1` flows *into*, not what it derives *from* — `cgx flows-from` on the same node returns `v#0`. `cgx.mutation_fanout` is wrong in the mirror direction: called on `return#1` it yields `v#0` and `y#1`, which are its sources, not values it mutates. The two are consistent with each other and inverted relative to `flows-from`/`flows-to`, which are correct. **Until this is fixed, read `cgx.pedigree` as a forward slice and `cgx.mutation_fanout` as a pedigree**, or use the `flows-*` subcommands, whose direction is correct.

Procedures compose with the rest of Layer 2: their yielded values can drive a subsequent `MATCH`.

### Shell quoting convention

Wrap the query in single quotes; use double quotes for string values inside the query:

```bash
cgx query 'MATCH (c)-[:CALLS*1..5]->(fn {name:"*::helper"}) RETURN c.fqn, c.file, c.line' --repo .
```

### Q-9: Query files

For queries longer than one line, use `@file.cql`. A read failure on the file is exit 2.

```bash
cgx query @queries/exception-sinks.cql --repo .
```

Contents of `exception-sinks.cql`:

```cypher
MATCH path = (src)-[:CALLS* {condition: "exception"}]->(sink)
WHERE sink.name IN ["exec", "system", "popen"]
  AND src.kind = "function"
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
ORDER BY src.file, src.line
```

### Q-10: SQL alternative (unstable, view-based) — **Planned (not yet shipped in v0.3.0)**

**Status: planned.** `--sql` is a hidden flag on the shared flag block. On `cgx query` it is read and **rejected with exit 2**. On the other seven commands that inherit the flag block nothing reads it, so it is accepted and silently ignored (exit 0) — a known defect; do not write scripts that rely on either behaviour.

For v1 implementations that use SQLite storage, a recursive-CTE interface is available. Queries execute against a small set of documented, versioned SQL views; physical tables are not API and carry no stability guarantee.

**Documented views:** `v_symbols`, `v_call_edges` (includes `site_id`, `edge_condition`, `confidence`, `introducing_commit`/`introducing_author` when present), `v_call_sites`, `v_provenance`. A `cgx_meta` view exposes `view_schema_version` (integer); view-breaking changes bump it. The feature is **unstable** in v1: the view set and columns may change between minor versions; pin `view_schema_version` in scripts.

Physical tables remain readable (SQLite cannot hide them cheaply) but are documented as **not API, no stability guarantee, may vanish entirely**.

```bash
cgx query --sql '
  WITH RECURSIVE callers(caller, callee, depth) AS (
    SELECT caller, callee, 1 FROM v_call_edges WHERE callee = "crypto::hash"
    UNION ALL
    SELECT e.caller, r.callee, r.depth + 1
    FROM v_call_edges e JOIN callers r ON e.callee = r.caller
    WHERE r.depth < 5
  )
  SELECT DISTINCT caller, depth FROM callers ORDER BY depth, caller
' --repo .
```

---

## Implementation status (v1)

**Audience:** Engineers integrating `cgx query` today.

The `cgx query` command is implemented in the `cgx-cql` crate (crates/cgx-cql). This section documents what runs today versus what the spec above describes but has not yet been built.

> **v0.2 update (shipped).** As of the v0.2 "Semantic precision" release:
> - **`--confidence` now discriminates.** CHA/RTA `dyn Trait` resolution and SCIP enrichment raise
>   edge tiers, so a `probable`/`certain` floor (CLI or `{confidence: "certain"}` in CQL) excludes
>   lower-tier edges instead of accepting everything. Pre-v0.2 the flag parsed but did not narrow.
> - **`cgx index --scip <index.scip>`** is available: an upgrade-only re-label pass over a
>   **user-supplied** SCIP index (cgx does not generate one). It records `resolution_source: scip`
>   provenance and cross-crate `scip-dep:` edges.
> - **`cgx explain`** shows per-edge provenance: confidence **tier**, the resolution **rule**,
>   `resolution_source`, and the call **site**.
>
> Effect node attributes (`own_effects`/`transitive_effects`) exist on nodes from v0.2 but are **not**
> queryable as CQL node properties yet (see "Deferred in v1" — effect *queries* are a later phase).

### Supported in v1

**MATCH patterns**
- Single fixed-hop: `MATCH (a)-[:CALLS]->(b)`
- Multi-relationship chain: `MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c)` (interior nodes must be named)
- Var-length: `*`, `*n`, `*n..`, `*..m`, `*n..m`
- `path =` binding on single-relationship patterns: `MATCH p = (a)-[:CALLS*]->(b) RETURN p`
- Multi-pattern (comma): `MATCH (m:method)-[:MEMBER_OF]->(t), (other)-[:CALLS]->(m)`

**Edge types (MATCH `:TYPE` syntax)**
- `CALLS`, `SPAWNS`, `DATA_FLOW`, `MEMBER_OF`, `CONTAINS`, `OVERRIDES`, `INHERITS`, `IMPLEMENTS`, `IMPORTS`, `REFERENCES`, `INSTANTIATES`, `THROWS`, `CATCHES`, `READS_FIELD`, `WRITES_FIELD`
- Multi-type: `READS_FIELD|WRITES_FIELD`

**Node properties**
- `n.name`, `n.file`, `n.line`, `n.kind` (also as `:label` in pattern)

**Edge properties**
- `r.condition` (`always/conditional/loop/exception/panic`)
- `r.confidence` (`possible/probable/certain`; ordering `certain > probable > possible` for MIN/MAX)
- `r.kind`

**WHERE predicates**
- Comparison: `=`, `<>`, `<`, `<=`, `>`, `>=`, `IN`
- Boolean: `NOT`, `AND`, `OR`
- Quantifiers: `NONE(x IN list WHERE p)`, `ANY(x IN list WHERE p)`, `ALL(x IN list WHERE p)`
- List comprehensions: `[r IN relationships(p) | r.condition]`, `[x IN list WHERE p | expr]`
- Bare pattern predicate: `NOT (m)<-[:CALLS]-()`

**RETURN / projections**
- Projections with `AS`, `DISTINCT`, `ORDER BY … ASC|DESC`, `LIMIT`
- Aggregates: `collect(x)`, `count(*)`, `count(x)`, `MIN([…])`, `MAX([…])`
- Path functions: `nodes(p)`, `relationships(p)`, `length(p)`, `last_node(p)`, `position_in(node, p)`, `size(list)`

**WITH pipeline**
- `WITH … WHERE` (scope barrier; all pipeline stages supported)

**CALL procedures**
- `CALL cgx.mutation_fanout(value) YIELD mutator, confidence`
- `CALL cgx.pedigree(value) YIELD source, confidence`

**CLI flags and formats**
- `cgx query '<expr>'` and `cgx query @file.cql`
- `--at <ref>` — pins the query to a specific git commit (also retrofitted onto Layer-1 subcommands)
- `--format human` (aligned columns), `json`, `sarif` (tabular results)
- `--format dot`, `mermaid`, `d2` (path-returning results only, i.e. `RETURN path`)
- `--assert-empty`, `--allow-vacuous` (vacuity guard, exit codes identical to Layer-1)

### Deferred in v1

Most items below produce a clear `plan error: … (deferred)` message rather than a silent wrong answer. **Three do not, and knowing which is the difference between a five-second fix and a confused half hour:**

- **`STARTS WITH` / `ENDS WITH` / `CONTAINS`** are not in the grammar at all, so they fail as ``parse error: expected WITH or RETURN, found identifier `STARTS` `` — no mention that anything is deferred. Use a glob in the inline property map instead (`{name:"*::handler"}`).
- **`COMPATIBLE_WITH`** fails the same way, for the same reason.
- **A bare single-node `MATCH (n)`** is ``plan error: a MATCH pattern must contain at least one relationship``. This is a structural rule of the language, not a deferred feature, and it is worth internalising early: every CQL query is anchored on an edge. "Find all nodes where …" has to be phrased through a relationship, or answered with `cgx search` / `cgx symbols` instead.

| Feature | Reason deferred |
|---|---|
| Bare single-node `MATCH (n)` with no relationship | Structural: the planner lowers `MATCH` onto an edge walk. Not planned to change |
| `STARTS WITH` / `ENDS WITH` / `CONTAINS` string predicates | Not in the grammar; use a glob in the inline property map |
| Theme-13 edge types: `CALLS:super`, `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` | Object-model edge schema not yet produced by the frontend (object-model ADR in progress) |
| `COMPATIBLE_WITH` predicate | Requires type reconstruction (`cgx.type_reconstruct`) which is deferred |
| `IS EMPTY` predicate | Requires type reconstruction; deferred with type-reconstruct |
| `cgx.type_reconstruct` procedure | Type-lineage extraction from the frontend is a follow-up cycle |
| `--sql` (Q-10) | The SQLite backend *is* integrated (`rusqlite`, the store's only backend); what is missing is the documented view set and its version contract. Rejected with exit 2 on `query`; silently ignored on the other seven commands that inherit the flag |
| `MATCH ALL … MUST PASS THROUGH / AVOIDING` (Q-20 guarded cut) | Requires call-site dominator facts at index time (Phase 3); planned follow-up |
| `UNWIND` | Deferred clause |
| `OPTIONAL MATCH` | Deferred clause |
| `UNION` | Deferred clause |
| `CREATE`, `SET`, `DELETE` | cgx query is a read-only language; write clauses will never be supported |
| Query parameters `$name` | Deferred |
| Arithmetic expressions (`+`, `-`, `*`, `/`) | Deferred |
| Node properties: `col`, `scope`, `type`, `in_degree`, `out_degree`, `is_exit`, `introduced_in_branch`, `confidence` (on nodes), `entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class`, `lock_set`, `suspends`, `async` | No backing field on a symbol node record in the current frontend |
| Node properties: `own_effects`, `transitive_effects` | **Different reason — the data exists.** Both are real, populated `NodeRecord` fields (the GM-12 closure pass runs on every index); what is deferred is the *query surface*. `transitive_effects` is rejected with the shared "no backing field" message, which is inaccurate for these two; `own_effects` falls through to `unknown node property` |
| Edge properties: `transformation`, `via`, `taint_label`, `site.*` | No backing field on an edge record in the current frontend |

### Canonical-example substitutions

Two of the three canonical examples reference properties that are not yet backed:

- **Example 2** (`scope` on a node, `r.transformation` on a `DATA_FLOW` edge): the DATA_FLOW var-length pedigree *pattern* runs and returns real rows. The integration test for Example 2 projects `r.condition` instead of `r.transformation` and omits `scope` (both produce a `plan error` when used — see the reject contract in `crates/cgx-cql/tests/rejects.rs`).
- **Example 3b** (`type` property on the `alloc` node): `type` has no backing field. The integration test substitutes a `{name:"new"}` match without the `type` attribute.

---

## Worked Examples for Every Major Question Class

### Canonical Example 1: Non-exception paths from main::foo to vulnerable::bar

The prompt's first canonical question: find call paths between two points that do not traverse any exception-conditioned edge.

**Subcommand — partially.** `cgx paths` takes both anchors positionally and enumerates *all* paths; it has no edge-condition filter, so the exceptional-class exclusion has to happen in Layer 2:

```bash
cgx paths main::foo vulnerable::bar --repo .
```

**Full query — this is the form that actually answers the question:**

```bash
cgx query '
  MATCH path = (src {name:"main::foo"})-[:CALLS*]->(dst {name:"vulnerable::bar"})
  WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN path
' --repo .
```

The `NONE` predicate implements a negative path constraint (Q-14): the path is only returned when no edge on it carries a label in the exceptional class (`exception` or `panic`). This is a condition on the complete path, not on any individual edge in isolation.

`{name:"main::foo"}` is an inline property map against the **full FQN**; in a real repository it is `{name:"*::main::foo"}` or the crate-qualified form. A `RETURN path` query renders through the same path renderer as `cgx paths`, contract line and all.

---

### Canonical Example 2: Pedigree of a variable in a function

Find all inbound data sources for a named variable at a specific program point.

**Subcommand — today:**

```bash
cgx flows-from 'PaymentProcessor::charge::amount#1' --repo .
```

`flows-from` addresses the value by its SSA FQN, not by source-level name plus `--at-function`; discover the FQN with `cgx search amount`. The `pedigree` subcommand with `--at-function` is Planned (Q-5).

**Full query — the pattern runs; two of its projections do not:**

```bash
cgx query '
  MATCH flow = (src)-[r:DATA_FLOW*1..8]->(sink {name:"*::charge::amount#1"})
  RETURN src.fqn, src.file, src.line,
         [e IN relationships(flow) | e.condition] AS conditions
' --repo .
```

The var-length `DATA_FLOW` pedigree pattern is real and returns real rows. What is **not** available is the original example's two projections: `scope` is not a node property (plan error), and `r.transformation` is rejected with ``edge property `transformation` is not populated by the current frontend; DATA_FLOW transformation tags are deferred``. The `Transform` enum exists in the schema — `Copy`, `Projection`, `Arith`, `Concat`, `Composed`, `Branched`, `Aggregation`, `Parse`, `Serialize`, `Other` — but no frontend populates it and CQL does not expose it. Project `e.condition` instead, as the integration test for this example does.

---

### Canonical Example 3: Members never referenced downstream from a given instance

Find methods or fields on a specific type that have no callers reachable from a declared entrypoint.

**Subcommand:**

```bash
cgx unused --repo . --kind method
```

`--kind` is the only filter `unused` has. `--type <TypeName>` and `--entrypoint <symbol>` do not exist (Q-19 is Planned), so narrowing to one type has to happen in Layer 2 — or by post-filtering the JSON on `fqn`.

**Full query:**

```bash
cgx query '
  MATCH (m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
  WHERE NOT (m)<-[:CALLS]-()
  RETURN m.name, m.file, m.line
  ORDER BY m.name
' --repo .
```

To restrict to "never referenced downstream from a specific instance allocation site":

```bash
cgx query '
  MATCH (alloc {name:"new", type:"MyStruct"})-[:CALLS*]->(m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
  WITH collect(m.name) AS reached
  MATCH (all_m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
  WHERE NOT all_m.name IN reached
  RETURN all_m.name, all_m.file, all_m.line
' --repo .
```

---

### Canonical Example 4: Which branches introduced calls to vulnerable::bar in the exception path

This question combines graph diff (Q-16) with edge-condition filtering (Q-11).

**Subcommand — shipped, including the edge-condition filter:**

```bash
# Edges added at feature/risky-change that were absent at main
cgx diff main feature/risky-change --newer-than --repo .

# Narrowed to exception-conditioned call edges only
cgx diff main feature/risky-change --added --kind calls --edge-condition exception --repo .
```

`--edge-condition` is real on `diff` (and only on `diff`). `--kind` is repeatable across nineteen edge-kind tokens.

**Planned (v0.4 — not yet runnable):** `--calls-to <symbol>`, which would restrict the diff to edges that reach a named sink. Today the way to express that is `--path-added --from '**' --to 'vulnerable::bar'`, which answers the closely related question "did a new *path* to this sink appear," with an exit-1 gate.

**Full query (per-branch comparison):**

```bash
cgx query '
  MATCH path = (src)-[:CALLS* {condition: "exception"}]->(dst {name:"vulnerable::bar"})
  WHERE src.introduced_in_branch = "feature/risky-change"
  RETURN src.name, src.file, src.line, length(path) AS hops
' --repo .
```

To check all branches at once against main (planned v0.4):

```bash
# cgx diff --base main --head "refs/heads/*" --repo . --calls-to vulnerable::bar --edge-condition exception
```

---

### Question class: Reachability from a set of entrypoints

```bash
# Is crypto::decrypt reachable from any HTTP handler entrypoint?
cgx query '
  MATCH (ep {kind:"entrypoint", entrypoint_class:"http"})-[:CALLS*]->(target {name:"crypto::decrypt"})
  RETURN ep.name, ep.file, ep.line
  LIMIT 10
' --repo .
```

---

### Question class: Paths NOT passing through a sanitizer (negative path constraint, Q-14)

```bash
cgx query '
  MATCH path = (src {entrypoint_class:"http"})-[:CALLS*]->(sink {name:"sql_query"})
  WHERE NONE(n IN nodes(path) WHERE n.name IN ["validate_input", "sanitize", "escape_sql"])
  RETURN src.name, sink.name, length(path) AS hops
' --repo .
```

---

### Question class: Path-relative transience predicate (Q-12)

An edge `C→D` is exception-transient relative to path P if any upstream edge on P has `condition = "exception"`. The query uses `ANY` over the path's edges upstream of the node of interest:

```bash
cgx query '
  MATCH path = (root {name:"main"})-[:CALLS*]->(target {name:"vulnerable::bar"})
  WHERE ANY(r IN relationships(path) WHERE r.condition = "exception")
  RETURN path
' --repo .
```

This returns paths where `vulnerable::bar` is exception-transient relative to the path, meaning the call to it is only reached via at least one exception-conditioned edge. Compare with the canonical example 1 which returns paths where it is NOT exception-transient.

---

### Question class: Depth-limited blast radius

```bash
# Callers up to depth 2, callees up to depth 3 — minimal subgraph for editing a function
cgx callers OrderService::submit --repo . --depth 2 --format json > callers.json
cgx callees OrderService::submit --repo . --depth 3 --format json > callees.json
```

Or as a single subgraph query:

```bash
cgx query '
  MATCH (c)-[:CALLS*1..2]->(fn {name:"OrderService::submit"})-[:CALLS*1..3]->(d)
  RETURN c, fn, d
' --repo .
```

---

### Question class: Reachability matrix (Q-15)

A matrix of entrypoint classes versus sink classes shows which attack surfaces connect to which dangerous operations.

```bash
cgx query '
  MATCH (ep)-[:CALLS*]->(sink)
  WHERE ep.entrypoint_class IN ["http", "grpc", "cli", "cron"]
    AND sink.sink_class IN ["sql", "shell", "file_write", "network_send", "crypto"]
  RETURN ep.entrypoint_class, sink.sink_class, count(*) AS path_count
  ORDER BY ep.entrypoint_class, sink.sink_class
' --repo .
```

With `--format csv`, this output is directly usable in a spreadsheet.

---

### Question class: Security regression gate

Find all paths from any symbol to a known-dangerous function; assert none exist for CI:

```console
$ cgx paths '**' dangerous_sink --repo . --assert-empty --format sarif > security.sarif
cgx: assertion failed: results found when none expected
$ echo $?   # 0 = clean; 1 = paths found (CI fail); 4 = vacuously satisfied (gate tested nothing)
1
```

Both anchors are positional. The SARIF document written to `security.sarif` carries the path as
a `codeFlows`/`threadFlows` sequence under rule `cgx/paths`, plus the two envelope notes
(`cgx/approximation-contract`, `cgx/index-freshness`) — so the gate's verdict and the basis for
it upload together.

> **Vacuity guard applies.** When combined with `--confidence certain`, this gate passes vacuously before a SCIP index is present (pre-SCIP, no edges are `certain`). The vacuity guard (exit code 4) fires in that case. See `docs/07-interfaces.md` IF-5 for the full vacuity guard definition and `--allow-vacuous` opt-out.

See `docs/07-interfaces.md` for the full exit-code contract and `--assert-count` / `--assert-max` variants.

---

### Question class: Graph diff across commit range (Q-16)

```bash
# All edge changes between two commits (shipped v0.3.0 — positional refs)
cgx diff HEAD~10 HEAD

# Only edges added at HEAD that were absent at HEAD~1 (--newer-than shipped)
cgx diff HEAD~1 HEAD --newer-than

# New exception-path edges introduced in the last commit (PLANNED v0.4 — not yet runnable)
# cgx diff --base HEAD~1 --head HEAD --repo . --edge-condition exception

# Diff expressed as a query (compare graph at two commits)
cgx query --at HEAD '
  MATCH (a)-[:CALLS {condition:"exception"}]->(b {name:"vulnerable::bar"})
  RETURN a.name, a.file, a.line
' --repo . > head.json

cgx query --at HEAD~1 '
  MATCH (a)-[:CALLS {condition:"exception"}]->(b {name:"vulnerable::bar"})
  RETURN a.name, a.file, a.line
' --repo . > prior.json
```

---

## Q-13: Depth and Fan-Out Limits

Transitive traversals are bounded in two orthogonal dimensions. **Depth** is
bounded by the simple-path guarantee and the explicit `--depth` flag (and
`[:CALLS*min..max]` in the query language — see the `CALLS*` semantics above).
**Fan-out** — the per-node breadth of the traversal — is the second dimension.
The two are independent: a shallow traversal can still enumerate an unbounded
frontier if it hops into a high-degree hub, which depth limits alone do not
prevent.

**Depth ships; fan-out does not.** `--depth` is real on every traversal
subcommand. `--max-fanout` and `--through-sentinels` are **Planned** — neither
flag exists in the binary, and passing either is a clap parse error (exit 2).
What ships in their place is a **work budget**: the walk stops after 10,000
emitted rows and renders `… (truncated: N more)`, with the approximation
contract turning `under-approximate`. The rest of this section specifies the
intended fan-out design; nothing in it is runnable today.

**`--depth` has three different behaviours, and they are worth knowing before
you script against one:**

| Command | `--depth` omitted | `--depth 0` |
|---|---|---|
| `callers`, `callees`, `reaches <FROM>`, `flows-to`, `flows-from` | 2 | seed node only |
| `paths` | 6 | depth limit lifted — **but the work budget then binds**, and the answer can be truncated (`under-approximate`) where the bounded query was `exact` |
| `reaches <FROM> <TO>`, `unused` | unlimited | seed node only |

The CLI's own `--help` text for `--depth` says `0` means unlimited without
qualification. That is wrong twice over: it is true only for `paths`, and even on `paths`
an unbounded search remains work-budgeted and frequently truncates. The reliable signal is
never the flag — it is the approximation contract on the answer you actually got.

### Per-node fan-out cap

The transitive subcommands (`callers`, `callees`, `paths`, `pedigree`,
`mutation-fanout`) and the query language accept `--max-fanout N`:

- `--max-fanout N` caps the number of neighbors expanded at each visited node at
  `N`. The cap applies per node, independently of the depth bound; it does not
  interfere with the simple-path guarantee.
- `--max-fanout 0` means **unbounded** — today's behavior, expanding every
  neighbor at every node.
- The default is a high-but-finite value. The concrete default is **TODO,
  pending measurement of real degree distributions on representative repos**; a
  figure around 2,000 is illustrative only and is not a committed default. The
  same measurement sets the sentinel threshold (below) and is tracked in
  docs/09-architecture.md ADR-10.

### Sentinel (hot-node) handling

A traversal that reaches a **sentinel** — a high-degree hub node — stops at it
and reports it as a leaf, rather than expanding through it. Sentinels are
auto-seeded from `in_degree` above a per-edge-kind threshold (the threshold is
applied per the traversal's own edge kind — a `calls`-hub is a sentinel for a
`callers`/`callees` traversal but not necessarily for a `derives-from`
traversal) plus a user/config allowlist of well-known hubs (loggers, primitive
types, `panic`/`assert`). This generalises the summarised-pedigree default
(direct parents only) of docs/04-dataflow-and-provenance.md DF-1.4 to all
transitive traversals.

`--through-sentinels` overrides the default and expands through sentinels,
enumerating their full neighborhood (subject to `--max-fanout`). The
per-edge-kind sentinel threshold itself is **UNSET/TODO** pending the same
degree-distribution measurement as the `--max-fanout` default.

### Explicit truncation markers

Truncation is always **explicit and visible** — a cut frontier is never silently
presented as a complete one, consistent with the cut-marker honesty model of
GM-5.3 (docs/03-code-graph-model.md). When a node's expansion is capped (by
`--max-fanout`) or withheld (sentinel), that node carries three markers in the
result:

| Marker | Meaning |
|---|---|
| `truncated` | boolean — the node's frontier was cut; downstream consumers must treat its neighbor list as partial |
| `degree_total` | the node's full degree for the traversal's edge kind (from GM-1.3 `in_degree` / `out_degree`) |
| `degree_emitted` | the number of neighbors actually emitted (≤ `degree_total`; `degree_total − degree_emitted` were cut) |

These markers are carried by every structured output format — `json`, `sarif`,
`dot`, and `mermaid` — so that no format can mistake a cut frontier for a
complete one. (Text output renders them inline on the truncated node.)

### Ranking under truncation

When a node's frontier is cut to `--max-fanout N`, the `N` emitted neighbors are
selected **deterministically** by **confidence then relevance, with ties broken
by symbol id**:

1. **Confidence** descending (`certain` > `probable` > `possible`, per GM-5.1) —
   the most reliably-resolved neighbors are kept first.
2. **Relevance** — closest-to-query-root first (depth ascending; the same notion
   as `--order relevance` in Q-17).
3. **Symbol id** ascending as the final deterministic tie-break, so the emitted
   set is identical on every run of the same query against the same index (the
   Q-17 determinism contract). Degree is structural and plays no part in the
   ranking beyond deciding that a cut occurs.

---

## Q-17: Determinism Guarantees

`cgx` provides a determinism contract: the same query against the same commit produces identical output on every run.

- **Stable ordering.** All result sets are sorted by `(file_path, line, col)` by default. No hash-map ordering, no pointer-order traversal.
- **Reproducibility via `--at`.** Queries accept `--at <ref>` (commit SHA or branch name) to pin the query to a specific graph snapshot. `--at HEAD` is the default; `--at abc1234` re-queries a prior state.
- **No randomness.** No random tie-breaking. No timestamp in default output.
- **Audit identity comes from the freshness envelope.** Every answer that consults the index reports `indexed_tree` and `head_tree` (git tree OIDs) plus `matches_head`, `dirty_files` and `stale` — enough to reproduce the exact graph the answer came from. **No CLI output format carries a `graph_version` key**; the MCP surface does carry one (`"7380e24+dirty.0a12fb8d97d6"` — indexed tree plus an overlay digest when a working-tree overlay is in play), so a script written against MCP JSON does not port field-for-field to CLI JSON.
- **Same index, same answer.** The graph DB is append-write during indexing and read-only during query. Concurrent reads are safe; writes lock only during index updates.
- **Deterministic truncation.** When the work budget cuts a frontier, the emitted rows are selected in the traversal's own stable order, so a truncated result is identical on every run of the same query against the same index — and the contract says the answer is `under-approximate` rather than leaving the reader to notice the marker line.

Byte-identical repeat output is an identity property, not a best effort: every example in this document was run twice and byte-compared.

**Planned:** `--order file|alpha|discovery|relevance`. No such flag exists today; ordering is the stable `(file, line, col)` sort described above and is not user-selectable.

---

## Edge-Condition Filter Reference (Q-11)

Edge condition labels come from `docs/03-code-graph-model.md`. **Where they can be filtered is narrower than the label taxonomy suggests:** `cgx diff --edge-condition <label>` and the CQL `r.condition` predicate. There is no edge-condition filter on `callers`, `callees`, `paths`, `reaches`, `flows-*` or `unused`.

| Label | Meaning | Use case |
|-------|---------|----------|
| `always` | Edge traversed on all paths | Happy-path-only analysis |
| `conditional` | Edge guarded by a data or branch condition | Conditional reachability |
| `exception` | Edge traversed only during exceptional control flow | Exception-path analysis |
| `loop` | Call site lexically inside a loop body (taken zero or more times) — see GM-3 | Loop body analysis |
| `panic` | Edge on an unwinding/aborting path (exceptional class) | Panic-surface analysis |

Subcommand flag (`cgx diff` only):

- `--edge-condition always|conditional|loop|exception|panic` — restrict the diff to edges carrying exactly this label

**Planned:** `--exclude-edge-condition` and `--only-edge-condition` on the traversal subcommands. Neither exists; both are a clap parse error. Until they land, express the constraint in CQL — which is strictly more expressive anyway, because a path-level predicate is not the same thing as an edge-level filter:

Query language predicates:

```cypher
-- Exclude exception edges
WHERE NONE(r IN relationships(path) WHERE r.condition = "exception")

-- Require at least one exception edge upstream
WHERE ANY(r IN relationships(path) WHERE r.condition = "exception")

-- Inline edge filter
MATCH (a)-[:CALLS {condition: "always"}]->(b)
```

---

## Confidence-Tier Filtering (Q-18)

Every edge carries a confidence tier from `docs/03-code-graph-model.md`: `certain`, `probable`, `possible`.

```bash
# Only certain edges (statically resolved, no ambiguity)
cgx callers foo::bar --repo . --confidence certain

# Certain and probable (excludes over-approximated edges)
cgx callers foo::bar --repo . --confidence probable

# All edges including possible (maximum coverage, may include false positives)
cgx callers foo::bar --repo . --confidence possible
```

Query language:

```cypher
MATCH (c)-[r:CALLS*1..5 {confidence: "certain"}]->(fn {name:"foo"})
RETURN c.name
```

> **Vacuity warning.** Confidence filters narrow assertion gates toward false safety: an `--assert-empty --confidence certain` gate passes vacuously green when no edge in the corpus is `certain` (e.g., pre-SCIP, most edges are `probable` or `possible`). The `--assert-empty` command detects and rejects vacuous passes by default (exit code 4). See `docs/07-interfaces.md` IF-5 for the full vacuity guard definition.

---

## Entrypoint-Scoped Reachability (Q-19) — **Planned (not yet shipped in v0.3.0)**

**What ships:** `cgx unused` walks from the entrypoint set the indexer *auto-detected* — `main`, `#[tokio::main]`, and recognised test functions. That set is not inspectable and not overridable.

```bash
# Unreachable from the auto-detected entrypoint set
cgx unused --repo . --kind function
```

**What does not ship:** every mechanism for controlling that set. `cgx unused --entrypoint-class http`, `cgx unused --entrypoint main::start` and `cgx index --entrypoint '<fqn>'` are all clap parse errors (exit 2). In CQL, `entrypoint_class` is a rejected node property. Framework entrypoint detection — the `HttpHandler` entrypoint kind that would populate an `http` class — is defined in the schema but constructed nowhere, so no symbol in any indexed repository carries it today.

The consequence for reading a `cgx unused` result: "unreachable" means unreachable from *whatever the indexer happened to recognise as a root*. A handler invoked only by a web framework's dispatcher is not currently recognised as a root, so its whole call tree can be reported as unused. This is the practical reason the `unused` contract carries its `| scope:` tail.

---

## Q-20: Path Quantifiers and Must-Pass-Through (∀-Path / Must-Analysis) — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — the ∃-path default already covers "exists a path from A to B"; Q-20 adds the dual must-analysis (∀-path) that security questions almost always require.

### Motivation

The queries throughout this document return paths that *exist* in the graph — standard may-analysis (∃-path). Most security questions need the opposite guarantee:

- "Is there **any** handler that reaches `db.write` **without** passing through `require_admin`?" (authorization bypass)
- "Do **all** paths from an acquire call reach a release call?" (resource leak)
- "Does **every** path to `log_event` pass through `redact_pii`?" (secret exposure)

These are ∀-path (all-paths, must-analysis) questions. The existing `NONE` predicate (Q-14) is one tool for them, but Q-20 introduces first-class subcommand flags and query-language clauses that express must-analysis directly.

### Prior art position

CodeQL's `Dominance.qll` provides `dominates` / `postDominates` predicates and Joern's CPGQL provides `dominates` / `dominatedBy` / `postDominates` / `postDominatedBy` steps. Both operate **intra-procedurally only** — they compute dominator trees within a single function's CFG and cannot express "every call-graph path from any entrypoint to sink S passes through check node N."

CodeQL's `BarrierGuard` / `isSanitizerGuard` is ∃-path-negation at the CFG level ("block flow when a guard is known to hold"), not a ∀-path assertion over the full call graph. Semgrep's sanitizer model similarly over-approximates all paths and cannot guarantee that a sanitizer was traversed on *every* path.

cgx extends the standard may-analysis model with must-analysis primitives operating over the full call graph, filling a gap not addressed by CodeQL's intra-procedural `Dominance.qll`, Joern's per-method `dominates` steps, or Semgrep's sanitizer model.

### Layer 1 — Subcommand flags

The `paths` subcommand gains two new flags:

```
cgx paths --from <symbol> --to <symbol> <path>
          [--must-pass-through <symbol>]    # ∀-path: ALL paths must include this node
          [--avoiding <symbol>]             # ∀-path: ALL paths must NOT include this node
          [--quantifier exists|all]         # default: exists (∃-path); use all for ∀-path
```

`--must-pass-through <symbol>` returns only paths that include the named node; it then asserts (with `--assert-empty` if desired) that no path exists without it.

`--avoiding <symbol>` returns only paths that do NOT include the named node. When used with `--assert-empty`, it acts as a must-pass-through assertion: if any path avoids the node, the assertion fires.

`--quantifier all` returns paths only if **every** path from source to sink satisfies the filter predicates. Combined with `--avoiding`, it answers "no path avoids X."

**Example: authorization bypass — does any handler reach `db.write` without `require_admin`?**

```bash
# Find paths that avoid require_admin (authorization bypass candidates)
cgx paths --from '**::handle' --to db::write --repo . \
    --avoiding require_admin \
    --confidence certain \
    --format sarif > authz-bypass.sarif

# Assert there are none (CI gate — exit 1 if any bypass path found)
cgx paths --from '**::handle' --to db::write --repo . \
    --avoiding require_admin \
    --assert-empty
```

### Normative semantics: guarded-cut reachability

`MATCH ALL ... MUST PASS THROUGH (g)` and `AVOIDING (g)` compile to **guarded-cut reachability**, not path enumeration. The definition:

**Definition (guard-protection).** Given source set S, sink T, guard symbol G, over the call graph at the active confidence floor:

1. Build the **residual graph** by:
   a. **Node cut:** delete node G (and all its incident call edges).
   b. **Dominance cut:** delete every call edge `e` whose call site `s` (via `e.site_id`) satisfies: ∃ `s' ∈ dominating_sites(s)` such that a call edge from `s'` resolves to `G` at confidence ≥ the query's confidence floor. (`dominating_sites(s)` = call sites in the same function whose CFG node dominates `s`'s CFG node, from the standard intraprocedural dominator tree on the caller's CFG.)
2. **T is guard-protected from S w.r.t. G** iff T is unreachable from S in the residual graph.
3. A **bypass finding** is any S→T path in the residual graph (reported with evidence, subject to `--max-paths`); the existence test itself is reachability (linear), not enumeration.

**Stored vs. computed.** The dominator tree per function and `dominating_sites` per call site are intraprocedural, path-independent facts — stored at index time (Phase 3) on call-site nodes (GM-1.4). The cut and reachability test run at query time. Nothing path-relative is stored (transience invariant preserved).

**Confidence ladder when CFG/dominance is absent** (Tier-2/3 languages, or Phase 1–2 before dominance ships):

| `guard_analysis` | Meaning | Finding confidence |
|---|---|---|
| `"cut+dominance"` | Every caller on the path had dominance facts | `min` edge confidence on the path (so `certain` is reachable) |
| `"cut-only"` | ≥1 caller on the path lacked dominance facts | Capped at `probable`; per-finding note names unanalyzed caller(s) ("inline sibling guards in these functions were not checked") |

∀-guarantees ("guard-protected") are always sound-conservative: a guarantee is never granted because facts were missing. Phase-1/2 behavior is pure graph-cut, all findings `guard_analysis: "cut-only"`, capped at `probable`. The legacy node-membership semantics is **removed**, not kept as a fallback.

> **Warning.** Node-membership (`NONE(n IN nodes(path))`) does not detect inline sibling guards and is **not** the must-pass-through semantics; it remains available as a weaker predicate. See below.

Multiple guards (`--must-pass-through` repeated): cut applies per guard with AND semantics (each guard independently protects).

### Layer 2 — Query language

```cypher
-- ∀-path: ALL paths from handler to db::write pass through require_admin
-- (expressed as: no path from handler to db::write avoids require_admin)
MATCH path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
WHERE NONE(n IN nodes(path) WHERE n.name = "require_admin")
RETURN path

-- ∀-path with ALL quantifier (explicit form — compiles to guarded-cut reachability)
MATCH ALL path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
MUST PASS THROUGH (check {name:"require_admin"})
RETURN path
```

The `MATCH ALL path` + `MUST PASS THROUGH` clause is the explicit ∀-path form. It compiles to guarded-cut reachability (see above). The `NONE`-based complement is the weaker node-membership predicate — use `MATCH ALL ... MUST PASS THROUGH` for the normative semantics.

`AVOIDING` is the complement:

```cypher
MATCH ALL path = (src)-[:CALLS*]->(sink {name:"log_event"})
AVOIDING (redactor {name:"redact_pii"})
RETURN src.name, src.file, src.line
```

When `MATCH ALL ... AVOIDING` returns zero paths, every path passes through the named node — the ∀ guarantee holds (subject to `guard_analysis` in output).

---

## Q-21: Path-Set Algebra (Complement, Intersection, Difference) — **Planned (not yet shipped in v0.3.0)**

> **Note.** Ordering/algebra forms written with `NONE` node-membership inherit the sibling-guard caveat from Q-20: inline sibling guards are not detected by the node-membership predicate. The dominance-correct forms arrive with Q-20 Phase-3 facts.

**Status: core-extension** — path-set algebra extends Q-14 (negative path constraints) and Q-20 (quantifiers) to a composable predicate algebra over path sets.

### Overview

A **path set** is the set of all paths in the graph satisfying a `MATCH` pattern. Q-21 provides operations over path sets:

| Operation | Meaning | Query form |
|-----------|---------|-----------|
| Complement | All paths in the graph **not** in set P | `MATCH path = ... WHERE NOT <predicate>` |
| Intersection | Paths in both set P and set Q | `WHERE <predicate-P> AND <predicate-Q>` |
| Difference | Paths in P but not in Q | `WHERE <predicate-P> AND NOT <predicate-Q>` |

These are standard boolean combinations of path predicates — no new syntax is required. Q-21 names the operations explicitly so that query authors reason clearly about which set of paths they are operating on.

### Composing with edge-condition predicates

Path predicates compose with edge-condition filters (Q-11) and quantifiers (Q-20):

```cypher
-- Intersection: paths that are BOTH on the exception class AND avoid the handler
MATCH path = (src {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  AND NONE(n IN nodes(path) WHERE n.name = "error_handler")
RETURN src.name, sink.name, length(path) AS hops

-- Difference: all-paths EXCEPT those that pass through the sanitizer
-- (i.e., the unsafe paths in the complement of the sanitized set)
MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink {sink_class:"sql"})
WHERE NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
RETURN path
```

### Non-exception path complement

The most common Q-21 pattern: restrict to non-exception paths (the complement of the exceptional-class path set):

```cypher
-- Non-exception paths from entrypoint to sink
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
RETURN ep.name, ep.file, ep.line
```

This computes the difference between all paths and the exception-class path set — the "happy path" set from the entrypoint to the sink.

### Composing algebra with Q-20 quantifiers

```cypher
-- All non-exception paths must pass through require_admin
-- (no non-exception path avoids require_admin)
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  AND NONE(n IN nodes(path) WHERE n.name = "require_admin")
RETURN path
-- Zero results = the ∀ guarantee holds on non-exception paths
```

---

## Q-22: Ordering and Pairing Predicates — **Planned (not yet shipped in v0.3.0)**

> **Note.** The `A BEFORE B ON ALL PATHS` and acquire/release forms written with `NONE` node-membership inherit the sibling-guard caveat from Q-20: inline sibling guards in callers are not detected by the node-membership predicate. The dominance-correct forms arrive with Q-20 Phase-3 facts.

**Status: core-extension** (A-then-B on all paths and acquire/release pairs); the acquire/release subset is the tractable 2-state case. Full typestate ordering is `roadmap` — see GM-13.4.

### A-then-B on all paths

The ordering predicate `A BEFORE B ON ALL PATHS` asserts that every path reaching B also passed through A before reaching B. This is the ∀-path complement of "B reachable without A" (Q-20):

**Layer 2 — Query language:**

```cypher
-- Assertion: every path from any entrypoint to sensitive::write
-- passes through authz::check before reaching sensitive::write
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(write {name:"sensitive::write"})
WHERE NONE(n IN nodes(path) WHERE n.name = "authz::check"
           AND position_in(n, path) < position_in(write, path))
RETURN path
-- Zero results = ordering guarantee holds
```

The helper `position_in(node, path)` returns the 0-based index of the node in the path. The predicate fires when `authz::check` appears on the path but *after* `sensitive::write` — or does not appear at all.

**Never-B-after-C variant:**

```cypher
-- Assert that no write() call appears after any close() call on any path
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(w {name:"resource::write"})
WHERE ANY(n IN nodes(path) WHERE n.name = "resource::close"
          AND position_in(n, path) < position_in(w, path))
RETURN path
-- Non-empty results = write-after-close found
```

### Acquire/release pairing (resource pairs, GM-13)

The acquire/release predicate checks whether every path from an acquire call reaches a release call — including exception-class paths. This is the Q-22 counterpart to the GM-13 resource-pair graph representation.

**Layer 1 — Subcommand:**

```bash
# Check that every path from db::Connection::begin reaches commit or rollback
cgx paths --from db::Connection::begin \
          --to db::Connection::commit db::Connection::rollback \
          --quantifier all \
          --including-exception-paths \
          <path>

# Assert no leak (exit 1 if any path from begin lacks a commit/rollback)
cgx paths --from db::Connection::begin \
          --to db::Connection::commit db::Connection::rollback \
          --quantifier all \
          --including-exception-paths \
          --assert-all-reach-sink \
          <path>
```

`--including-exception-paths` ensures that exception-class edges (the paths that most commonly miss the release) are included. `--assert-all-reach-sink` exits with code 1 if any path from the acquire call does not reach one of the named release symbols.

**Layer 2 — Query language:**

```cypher
-- Find all paths from acquire that do NOT reach any release
-- on ANY edge condition (including exception and panic paths)
MATCH path = (acquire {name:"db::Connection::begin"})-[:CALLS*]->(any_node)
WHERE NOT ANY(n IN nodes(path)
              WHERE n.name IN ["db::Connection::commit",
                               "db::Connection::rollback"])
  AND (last_node(path).is_exit = true OR length(path) >= 1)
RETURN path
-- Non-empty results = resource leak found (no release on this path)
```

### Prior art note

Meta Infer Pulse detects resource leaks interprocedurally for known API pairs (open/close, malloc/free). CodeQL's `java/input-resource-leak` query checks intra-method resource closure on all paths within a single method. Neither exposes the check as a composable query primitive parameterizable with arbitrary user-declared acquire/release symbols. cgx's pairing predicate generalises this to user-declared pairs, composable with taint and effect queries.

### Typestate ordering (roadmap note)

Full typestate — "no `write()` after `close()`", "every `begin()` reaches exactly one of `commit()` or `rollback()`" — is `roadmap` status (GM-13.4). The acquire/release subset (a 2-state automaton: acquired, released) is the tractable case and is the scope of Q-22.

---

## Q-23: Typed Taint Queries — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — builds on DF-11 (typed taint labels and class-matched sanitization) and DF-12 (source and sink classes). Adds the query-layer surface for composing class-matched taint predicates.

### Overview

Standard taint queries (∃-path: "does any source reach any sink without a sanitizer?") are already available via DF-5. Q-23 adds class-aware taint queries: the sanitizer class must match the sink class, taint labels carry source-class and sink-class attributes, and queries can specify both.

### Prior art position

CodeQL's `DataFlow::StateConfigSig` / `TaintTracking::GlobalWithState` allows per-query flow states where `isBarrier(node, state)` clears only a named state. This is functionally equivalent to class-matched sanitizer/sink pairs, but defined per query configuration — there is no cross-query class schema. CodeQL's models-as-data (April 2026) adds extensible `barrierModel` / `barrierGuardModel` predicates for external barrier declarations, still per query category.

Semgrep taint labels (experimental feature) allow sources to carry a `label:` string and sinks to specify `requires:` (boolean over labels), with propagators that can replace labels. Each rule defines its own label strings independently; there is no shared schema across rules.

cgx promotes this to a first-class cross-query class schema: source classes, sanitizer classes, and sink classes are declared once in `cgx.toml` (DF-11.4 / DF-12) and automatically linked. HTML-escaping is a sanitizer of class `html` and clears only taint at `html` sinks, not `sql` sinks. Neither CodeQL nor Semgrep enforces this linkage globally across queries.

### Layer 1 — Subcommand

```bash
# Find all network-source → sql-sink paths without a sql-class sanitizer
cgx paths --from-class network --to-class sql \
          --require-sanitizer-class sql \
          --negate-sanitizer \
          <path>

# Secret-to-log pedigree: secret sources reaching log sinks
cgx paths --from-class secret --to-class log \
          --confidence probable \
          <path>

# Class-matched taint: deserialization sources reaching eval sinks
cgx paths --from-class deserialization --to-class eval \
          <path>
```

The `--require-sanitizer-class <class>` / `--negate-sanitizer` combination finds paths where a sanitizer of the matching class is absent. Omitting `--negate-sanitizer` finds paths where a matching sanitizer is present (useful for auditing sanitizer coverage).

### Layer 2 — Query language

New node properties for taint queries:

- `source_class` — on source nodes, the trust-boundary class (DF-12.1)
- `sink_class` — on sink nodes, the vulnerability class (DF-12.2)
- `sanitizer_class` — on sanitizer nodes, the class this sanitizer clears
- `taint_label` — on data-flow path edges, the propagating taint label (carries `source_class` and `sink_class` attributes)

```cypher
-- Injection: network → sql with no sql-class sanitizer on the path
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"sql"})
WHERE src.source_class = "network"
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class = "sql")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops

-- Class-matched sanitizer present: auditing correct sanitization
MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink {sink_class:"html"})
WHERE ANY(n IN nodes(path)
          WHERE n.sanitizer_class = "html")
RETURN src.name, sink.name
```

### Taint-label predicate on DATA_FLOW edges

```cypher
-- Find paths where the taint label's sink_class does not match
-- the sanitizer_class of any node on the path
-- (cross-context sanitizer misuse: html-escape applied to a sql-destined value)
MATCH path = (src)-[r:DATA_FLOW*]->(sink)
WHERE ANY(edge IN relationships(path) WHERE edge.taint_label.sink_class = "sql")
  AND ANY(n IN nodes(path) WHERE n.sanitizer_class = "html"
                              AND NOT n.sanitizer_class = "sql")
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
RETURN src.name, sink.name, sink.sink_class
```

---

## Q-24: Concurrency Queries — **Planned (not yet shipped in v0.3.0)**

**Status: schema-room for the queries; two of their four inputs already ship.** The `spawns`
edge kind (GM-9) is real and queryable — `MATCH (a)-[:SPAWNS]->(b)` plans and runs — and the
`blocking` effect (GM-12) is a real `Effect` variant stamped by the adapters. What is missing is
the pair the query families actually pivot on: **lock sets (GM-11) do not exist**, and
**`suspends` (GM-10), though a real `CallSite` field, is a rejected CQL node property**. Every
query below needs one or the other, so none is expressible; but "no concurrency support" would
overstate the gap.

### Query families

#### Await-holding-lock

A suspension point (`suspends = true` on the call-site node, GM-10) reached while the lock set is non-empty (GM-11):

**Layer 1:**

```bash
cgx query --concurrency await-holding-lock <path>
```

**Layer 2:**

```cypher
-- Find all call sites that suspend while holding at least one lock
MATCH (site)
WHERE site.suspends = true
  AND size(site.lock_set) > 0
RETURN site.name, site.file, site.line,
       site.lock_set AS held_locks
ORDER BY site.file, site.line
```

**Prior art note:** Clippy's `await_holding_lock` lint detects this pattern intra-procedurally within a single async function. It fires when a `MutexGuard`, `RwLockReadGuard`, or `RwLockWriteGuard` is live in the generator's interior types at an `await` point. cgx's query is inter-procedural: it fires when a lock acquired in a calling function is still in the lock set when a suspension point is reached in a callee.

#### Blocking-in-async

A function with the `blocking` effect (GM-12) reachable from an async entrypoint via `calls:async` or `spawns` edges:

**Layer 1:**

```bash
cgx query --concurrency blocking-in-async <path>
```

**Layer 2:**

```cypher
-- Find blocking functions reachable from any async entrypoint
MATCH path = (ep {kind:"entrypoint", async:true})-[:CALLS*]->(fn)
WHERE "blocking" IN fn.transitive_effects
  AND NONE(n IN nodes(path) WHERE n.name IN ["spawn_blocking",
                                              "tokio::task::spawn_blocking",
                                              "rayon::spawn"])
RETURN fn.name, fn.file, fn.line,
       length(path) AS depth
ORDER BY depth, fn.file
```

The `NONE` predicate excludes paths where the blocking call was intentionally dispatched to a thread pool (the correct pattern for calling blocking code from async).

#### Lock-set consistency (cross-thread)

A field accessed from two or more spawn-distinct contexts whose lock sets do not share a common lock:

```cypher
-- Find fields accessed from multiple spawn contexts with inconsistent lock sets
MATCH (a)-[:CALLS*]->(site_a)-[:READS_FIELD|WRITES_FIELD]->(field)
MATCH (b)-[:CALLS*]->(site_b)-[:READS_FIELD|WRITES_FIELD]->(field)
WHERE a.spawn_context <> b.spawn_context
  AND NOT ANY(lock IN site_a.lock_set WHERE lock IN site_b.lock_set)
  AND (site_a)-[:WRITES_FIELD]->(field) OR (site_b)-[:WRITES_FIELD]->(field)
RETURN field.name, field.file,
       site_a.name AS access_a, site_a.lock_set AS locks_a,
       site_b.name AS access_b, site_b.lock_set AS locks_b
```

**Prior art note:** Meta Infer RacerD uses a boolean lock abstraction — it tracks whether *some* lock is held, not *which* lock — and cannot detect the inconsistent-lock-set pattern where field F is protected by lock A in one thread and lock B in another. cgx's lock-set model (GM-11) tracks lock identity, enabling this query.

#### TOCTOU across a suspension point

A value validated at one call site used at a later call site with a suspension point between them (the `suspends` property from GM-10):

```cypher
-- TOCTOU: value validated, then suspension, then value used again
MATCH path = (validate {name:$validate_fn})-[:CALLS*]->(use {name:$use_fn})
WHERE ANY(n IN nodes(path)
          WHERE n.suspends = true
          AND position_in(n, path) > position_in(validate, path)
          AND position_in(n, path) < position_in(use, path))
RETURN path
```

#### Cross-spawn race (writes-global effect)

Functions with `writes-global` effect reachable from two or more distinct spawn sites without consistent lock protection:

```cypher
MATCH (spawn_a {edge_type:"spawns"})-[:CALLS*]->(fn_a)
MATCH (spawn_b {edge_type:"spawns"})-[:CALLS*]->(fn_b)
WHERE spawn_a <> spawn_b
  AND fn_a = fn_b
  AND "writes-global" IN fn_a.own_effects
  AND NOT ANY(lock IN spawn_a.lock_set WHERE lock IN spawn_b.lock_set)
RETURN fn_a.name, fn_a.file, fn_a.line,
       spawn_a.file AS spawn_a_site,
       spawn_b.file AS spawn_b_site
```

---

## Q-25: Dependency and CVE Reachability Queries — **Planned (not yet shipped in v0.3.0)**

**Status: schema-room** — dependency edges (GM-14.3) must be populated (package, version, ecosystem attributes) before these queries execute. The confidence labels and edge-condition context described here depend on the GM-14 schema.

### Overview

Dependency and CVE reachability queries answer: "Is the vulnerable symbol in `package@version` actually reachable from any entrypoint in this application, on what path, with what confidence, and under what conditions?"

This is function-level reachability-based SCA (Software Composition Analysis), extended with taint context and trust-boundary awareness.

### Prior art position

govulncheck (Go-specific) uses VTA (Variable Type Analysis) from `golang.org/x/tools/go/callgraph/vta` to build a call graph and traces paths from entrypoints to vulnerable symbols. It provides function-level reachability (∃-path) and shows the call stack. It does not provide data-flow taint context (what data reaches the vulnerable function), trust-boundary information, or edge-condition context. VTA falls back to RTA then CHA on timeout. govulncheck is Go-only.

Snyk Reachability and Endor Labs Reachability similarly provide function-level ∃-path reachability for Java, JavaScript/TypeScript, and Python. Neither provides data-flow taint or edge-condition context. The reachability algorithms are not publicly documented.

cgx's CVE reachability adds: multi-language support, edge-condition annotation (is the vulnerable symbol reachable only on exception paths?), taint context (does tainted data from a network source reach the vulnerable symbol?), confidence labels from the GM-5 resolution ladder, and composability with Q-20 must-analysis (does every path to the vulnerable symbol pass through a mitigation check?).

### Layer 1 — Subcommand

```bash
# Is any symbol in serde_json@1.0.x reachable from any entrypoint?
cgx paths --from '**' --to-package serde_json --confidence probable <path>

# CVE-specific: is the named symbol reachable?
cgx paths --from '**' \
          --to 'serde_json::de::from_str' \
          --to-package 'serde_json@>=1.0.0,<1.0.96' \
          --format sarif <path>

# With taint context: does tainted network data reach the vulnerable symbol?
cgx paths --from-class network \
          --to 'vulnerable_lib::parse_header' \
          --confidence probable \
          <path>

# CI gate: fail if any entrypoint reaches the CVE symbol
cgx paths --from '**' --to 'log4j_core::PatternLayout::toSerializable' \
          --assert-empty <path>
```

### Layer 2 — Query language

Dependency attributes from GM-14.3 (`package`, `version`, `ecosystem`) are available on call-edge nodes:

```cypher
-- All entrypoint paths to any symbol in package libfoo, version 2.x
MATCH path = (ep {kind:"entrypoint"})-[r:CALLS*]->(vuln)
WHERE ANY(edge IN relationships(path)
          WHERE edge.dependency.package = "libfoo"
            AND edge.dependency.version STARTS WITH "2.")
RETURN ep.name, vuln.name, vuln.file,
       [e IN relationships(path) | e.confidence] AS confidences,
       [e IN relationships(path) | e.condition] AS conditions

-- CVE triage: reachable, with what edge conditions?
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
RETURN ep.name, ep.file, ep.line,
       length(path) AS hops,
       [e IN relationships(path) | e.condition] AS conditions,
       ANY(e IN relationships(path) WHERE e.confidence = "certain") AS any_certain
ORDER BY length(path)

-- With taint context: does network-tainted data reach the vulnerable symbol?
MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(arg)-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
RETURN src.name, src.file, src.line,
       arg.name, vuln.name,
       length(path) AS hops
```

### Confidence labels on dependency paths

Confidence on dependency-crossing edges follows the GM-5 resolution ladder (GM-14.3). When the dependency has a SCIP index, edges are `probable` to `certain`. When the dependency has no SCIP index, edges fall to `probable` (unique name match) or `possible` (ambiguous). All findings include the confidence tier in the result.

```bash
# Only report findings with at least probable confidence (exclude CHA over-approximation)
cgx paths --from '**' --to 'libfoo::parse_header' \
          --confidence probable <path>
```

---

## Security Query Worked Examples

The following six worked examples show both query layers where sensible. They address the security question bank from the design; cross-references name the feature IDs that implement each capability.

---

### Worked Example 1: Authorization Bypass

**Question:** Which handlers reach `db::write` on some path that avoids `require_admin`?

This is the ∀-path authorization bypass pattern: find the complement of "all paths pass through `require_admin`."

**Subcommand:**

**`--avoiding` is Planned (Q-20); `paths` takes positional anchors.** What runs today is the
unfiltered enumeration plus the CI gate:

```console
$ cgx paths '**::handle' db_write --repo . --confidence certain
path 1 (1 hops, min-confidence=certain):
     rust_sample::handle  (src/main.rs:2)
  -> rust_sample::db_write  (src/main.rs:4) [always]
approximation: exact (within modeled graph)
freshness: current | indexed tree 57616e8, working tree clean

$ cgx paths '**::handle' db_write --repo . --assert-empty
…
cgx: assertion failed: results found when none expected
$ echo $?
1
```

The `--avoiding require_admin` half — "only the paths that bypass the guard" — has no
subcommand form. Express it in Layer 2, where it is a path-level predicate:

```bash
cgx query '
  MATCH path = (h {name:"*::handle"})-[:CALLS*]->(sink {name:"*::db_write"})
  WHERE NONE(n IN nodes(path) WHERE n.name = "*::require_admin")
  RETURN path
' --repo .
```

**Full query:**

```bash
cgx query '
  MATCH path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
  WHERE NONE(n IN nodes(path) WHERE n.name = "require_admin")
    AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN h.name, h.file, h.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY h.file, h.line
' --repo .
```

The `NONE` on `require_admin` finds bypass paths (Q-20 / Q-14). The second `NONE` restricts to non-exception paths (Q-21) — exception-path-only bypasses are reported separately to distinguish structural bypasses from error-path bypasses.

---

### Worked Example 2: Resource Leak on Exception-Class Paths

**Question:** Is there any path from `db::Connection::begin` that reaches a function exit without passing through `commit` or `rollback`?

This directly uses Q-22 acquire/release pairing. The critical paths are those in the exceptional class (Q-11, Q-21) — the paths most commonly missing release calls.

**Subcommand:**

**Planned — not runnable.** Four separate unshipped things: `--quantifier`,
`--including-exception-paths` and `--assert-all-reach-sink` are Q-20 must-analysis flags that do
not exist, `paths` takes exactly two positional anchors (not a list of sinks), and there is no
`--from`/`--to`. The ∀-path question this asks ("do *all* paths from acquire reach a release?")
is precisely what Q-20 is for, and it is a plan error today.

```bash
# Planned (Q-20) — none of these flags exist
# cgx paths db::Connection::begin db::Connection::commit \
#           --quantifier all --including-exception-paths --assert-all-reach-sink --repo .
```

**Full query:**

```bash
cgx query '
  MATCH path = (acquire {name:"db::Connection::begin"})-[:CALLS*]->(exit_fn)
  WHERE ANY(r IN relationships(path) WHERE r.site.is_return_site = true)
    AND NONE(n IN nodes(path)
             WHERE n.name IN ["db::Connection::commit",
                               "db::Connection::rollback"])
  RETURN acquire.file, acquire.line,
         exit_fn.name, exit_fn.file, exit_fn.line,
         ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
           AS on_exception_path
  ORDER BY on_exception_path DESC, acquire.file
' --repo .
```

Non-empty results indicate a resource leak. The `on_exception_path` field distinguishes leaks that only occur on error paths (the most common category) from leaks on the happy path.

---

### Worked Example 3: TOCTOU Across a Suspension Point

**Question:** Is there a path where a file or value is validated and then used again after an `await` point?

Uses GM-10 (`suspends` property), Q-20 (ordering predicates), and Q-24 (TOCTOU query family).

**Full query:**

```bash
cgx query '
  MATCH path = (validate)-[:CALLS*]->(use)
  WHERE validate.name = $validate_fn
    AND use.name = $use_fn
    AND ANY(n IN nodes(path)
            WHERE n.suspends = true
            AND position_in(n, path) > position_in(validate, path)
            AND position_in(n, path) < position_in(use, path))
  RETURN path
' --repo . --param validate_fn=fs::metadata --param use_fn=fs::read
```

> **`--param` / `$name` are Planned.** `cgx query` has no `--param` flag (clap parse error, exit 2) and query parameters are in the Deferred list below. Inline the literal value into the query text until they ship.

**Variation — any value validated by a check function and then used after an await:**

```bash
cgx query '
  MATCH path = (check {name:"validate_path"})-[:CALLS*]->(use {name:"fs::open"})
  WHERE ANY(n IN nodes(path) WHERE n.suspends = true)
  RETURN check.file, check.line, use.file, use.line
' --repo .
```

---

### Worked Example 4: Secret-to-Log Pedigree

**Question:** Does any value originating from key material or credentials reach a `log`-class sink?

Uses DF-13 (secret pedigree), DF-11/12 (typed taint), Q-23 (typed taint queries).

**Subcommand:**

**Planned — not runnable.** `--from-class` / `--to-class` are Q-23 typed-taint flags; they do
not exist, and neither does the `source_class` / `sink_class` node property they would filter on
(both are explicit CQL plan errors). There is no taint-class machinery to anchor this query.

```bash
# Planned (Q-23) — no class-based anchors exist
# cgx paths --from-class secret --to-class log --confidence probable --repo .
```

What ships in this space is the untyped pedigree walk: `cgx flows-from`/`cgx flows-to` over
`derives-from`, which answers "where did this value come from" with no notion of trust class.

**Full query:**

```bash
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"log"})
  WHERE src.source_class = "secret"
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "log")
  RETURN src.name, src.file, src.line,
         sink.name, sink.file, sink.line,
         [e IN relationships(path) | e.transformation_kind] AS transforms,
         length(path) AS hops
  ORDER BY src.file, src.line
' --repo .
```

The `transformation_kind` sequence on each path shows whether the secret passed through `encode`, `format-string`, or `copy` transformations — useful for triage. A secret that reaches a `log` sink via `concat` is a direct exposure; one via `encode(log)` may be intentionally redacted (if a redact sanitizer class is declared).

---

### Worked Example 5: CVE Reachability Triage

**Question:** Is `libfoo::parse_header` (the vulnerable symbol named in CVE-XXXX-YYYY) actually reachable from any entrypoint in this application?

Uses Q-25 (dependency and CVE reachability), GM-14.3 (dependency edges).

**Subcommand:**

```console
$ cgx paths '**' 'libfoo::parse_header' --confidence probable --repo .
cgx: no symbol matched pattern `libfoo::parse_header`
$ echo $?
2
```

That is the shipped form — positional anchors — shown against a repository that does not vendor
`libfoo`. **Exit 2, not exit 0**: an unresolvable anchor is a usage error, not "no path found."
The distinction matters for CVE triage, because a typo'd symbol name and a genuinely unreachable
CVE must not look the same. Only an exit 0 with `(no results)` and an `exact` contract is a real
negative.

The second half — restricting to exception paths — has **no subcommand form**;
`--only-edge-condition` does not exist on `paths`. Express it in Layer 2:

```bash
cgx query '
  MATCH path = (ep)-[:CALLS*]->(v {name:"*::parse_header"})
  WHERE ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
  RETURN path
' --repo .
```

**Full query:**

```bash
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
  RETURN ep.name, ep.file, ep.line,
         length(path) AS hops,
         [e IN relationships(path) | e.condition] AS conditions,
         MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
  ORDER BY hops, ep.name
  LIMIT 20
' --repo .
```

The `MIN` over edge confidences gives the weakest-link confidence for the entire path. A path where all edges are `certain` is a confirmed reachable path; one where the weakest edge is `possible` may be a false positive from dynamic dispatch over-approximation.

**With taint context — does attacker-controlled data reach the vulnerable symbol?**

```bash
cgx query '
  MATCH taint_path = (src {source_class:"network"})-[:DATA_FLOW*]->(arg)
  MATCH call_path = (arg)-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
  RETURN src.name, src.file, arg.name,
         vuln.name, vuln.file,
         length(taint_path) + length(call_path) AS total_hops
' --repo .
```

---

### Worked Example 6: Branch-Diff Security Gate

**Question:** Did this branch introduce any new source → sink path, any new edge into a sensitive sink, or any edge with a newer introducing commit?

Uses Q-16 (graph diff), IX-9 (edge age and author attribution, `docs/06-indexing-and-vcs.md`), Q-20 (quantifiers for regression checking).

**Subcommand (CI gate):**

**`cgx diff` takes positional refs, and the shipped gate is `--path-added`.** The class-based
anchors (`--from-class`/`--to-class`) and `--new-paths-only` / `--calls-to-sink-class` are Q-23
typed-taint surface and do not exist; `--from`/`--to` on `diff` are **glob anchors over FQNs**,
which is what makes the shipped gate usable without taint classes:

```console
$ cgx diff HEAD~1 HEAD --repo . --path-added --from '**::bypass' --to '**::db_write'
1 new call/dataflow reachability path(s) [reachability, not a security guarantee]:
+ path  rust_sample::bypass  ->  rust_sample::db_write  [introduced by f1fa44406965 (t)]
cgx: path-added gate: 1 new call/dataflow reachability path(s) found
$ echo $?
1
```

Exit 1 when a new path appears, exit 0 when clean — no output parsing needed in CI. Attribution
(`introduced by <12-hex> (<author>)`) comes free; `--format json` adds `introducing_commit`,
`introducing_author`, `introducing_email` and `author_time` per path. For the coarser "what
edges changed" view:

```console
$ cgx diff HEAD~1 HEAD --repo . --added --kind calls
+ edge  rust_sample::bypass  ->  rust_sample::db_write  (src/main.rs)
```

The class-typed version below is Planned:

```bash
# Planned (Q-23) — none of these flags exist
# cgx diff main HEAD --repo . \
#     --new-paths-only \
#     --from-class network \
#     --to-class sql \
#     --assert-empty

# Step 2 (Planned): new edges into sensitive sinks by taint class
# cgx diff main HEAD --repo . \
#     --calls-to-sink-class sql \
#     --calls-to-sink-class shell \
#     --calls-to-sink-class eval \
#     --format sarif > new-sink-edges.sarif
#   (`--calls-to-sink-class` does not exist, and `diff` accepts only human|json — sarif is exit 2)

# Step 3: General new-edge security gate using IX-9 edge age
cgx query --at HEAD '
  MATCH (a)-[r:CALLS]->(sink)
  WHERE sink.sink_class IN ["sql","shell","eval","log","net-request"]
    AND r.introducing_commit IN $branch_commits
  RETURN a.name, a.file, a.line,
         sink.name, sink.sink_class,
         r.introducing_commit, r.introducing_author
  ORDER BY sink.sink_class, a.file
' --repo . --param branch_commits=$(git log main..HEAD --format='%H' | jq -Rs 'split("\n")')
```

> **`--param` / `$name` are Planned.** `cgx query` has no `--param` flag (clap parse error, exit 2) and query parameters are in the Deferred list below. Inline the literal value into the query text until they ship.

**Full query (comparing edge sets between base and head):**

```bash
# At HEAD: new edges since merge-base
cgx query --at HEAD '
  MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink {sink_class:"sql"})
  WHERE NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
  RETURN src.name, sink.name, length(path) AS hops
' --repo . > head-findings.json

# At merge-base: baseline findings
cgx query --at $(git merge-base main HEAD) '
  MATCH path = (src {source_class:"network"})-[:DATA_FLOW*]->(sink {sink_class:"sql"})
  WHERE NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
  RETURN src.name, sink.name, length(path) AS hops
' --repo . > base-findings.json

# Diff the findings sets to identify regressions introduced by this branch
# (new findings in head that were not in base)
```

The `introducing_commit` and `introducing_author` fields on edges (IX-9) let CI annotate new sink-reachable edges with the exact commit and author that introduced them, enabling targeted security review assignments.

---

## Q-26: Lineage Type Reconstruction Queries — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — the query-layer surface mirrors the graph-level reconstruction defined in `docs/04-dataflow-and-provenance.md` DF-19, which is itself `core-extension`. The constraint-gathering traversal (up/down/sideways over existing pedigree, forward-flow, and unification edges) requires no new schema primitives beyond those already present in DF-1, DF-6, DF-9, DF-16, and DF-20.

### Motivation and prior art position

For values of erased or widened type (`any`, `interface{}`, untyped parameters, reflection results), standard type lookup returns the widened type — not the actual type the value carries. TypeScript's Compiler API `getTypeAtLocation` resolves the inferred or declared type at any AST node after whole-program type-checking, but returns `any` for values of type `any` — this is type **lookup**, not reconstruction from usage. Pytype infers types for unannotated Python by whole-program analysis and generates stub files, but does not expose a per-expression query API that can be driven from the call graph. No production tool answers "what are the candidate types of this value, derived from how it is used?" as an on-demand graph query. DF-19 is a genuine novelty claim.

cgx answers this question by crawling three directions from the value's node and intersecting the resulting constraints (see DF-19.2 for the full specification):

- **Up (pedigree):** constructors and literals are `certain` type anchors; return types and parameters contribute their declared/inferred types with inherited confidence.
- **Down (uses):** method calls and typed-parameter sinks constrain the value structurally (its duck footprint).
- **Sideways (unification):** values that must share a type — two arms of a DF-9 join, DF-16 aliases, DF-20 channel send↔recv pairs — propagate constraints laterally.

Primitive operations propagate type via DF-10 transformation kinds: `arith` edges yield numeric types, `concat` edges yield string types, `coerce(from,to)` edges yield the destination type.

### Result shape

| Result | Meaning |
|--------|---------|
| Single type, `certain` | Fully resolved — the value has exactly this type at this point |
| Narrowed set, `probable` | Two or three candidate types consistent with all constraints |
| Wider set, `possible` | Insufficient constraints; call is near a type-confidence boundary (GM-14.6) |
| Empty set — **CONTRADICTION** | No type satisfies all constraints simultaneously; this is a bug signal |

A contradiction indicates the value is used in two incompatible ways — for example, constructed as `File` (up, `certain`) but passed to a function requiring `Socket` (down). This query class has no direct equivalent in any mainstream static analysis tool.

### Layer 1 — Subcommand

```
cgx type-of <symbol-or-var> [--repo <path>]
             [--at-function <fn>]
             [--direction up|down|sideways|all]  # default: all
             [--confidence certain|probable|possible]
             [--format <fmt>]
```

The `--direction` flag restricts which constraint directions are gathered. `all` runs all three and intersects the results.

**Examples:**

```bash
# Discover the candidate type of 'v' at entry to process()
cgx type-of v --repo . --at-function process

# Type discovery using only pedigree (up-direction only — fastest)
cgx type-of v --repo . --at-function process --direction up --confidence probable

# All values in the codebase whose declared type is any/interface{} but
# whose reconstructed type is certain — they have a known type despite erasure
cgx type-of '*' --repo . --declared-type any --reconstructed-confidence certain
```

### Layer 2 — Query language

**Discover the type of a named value:**

```bash
cgx query '
  MATCH (v {name:"v", scope:"process"})
  CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
  RETURN candidate_type, confidence, evidence
  ORDER BY confidence DESC
' --repo .
```

**Type-contradiction bug query — values whose reconstructed type contradicts their declared type:**

```bash
cgx query '
  MATCH (v)
  WHERE v.declared_type IS NOT NULL
  CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
  WHERE candidate_type IS EMPTY
     OR (confidence IN ["certain","probable"]
         AND NOT candidate_type COMPATIBLE_WITH v.declared_type)
  RETURN v.name, v.file, v.line,
         v.declared_type,
         candidate_type,
         evidence
  ORDER BY v.file, v.line
' --repo .
```

`candidate_type IS EMPTY` returns values where the constraint intersection is empty — a contradiction. The `COMPATIBLE_WITH` predicate tests structural compatibility; it returns `false` when the reconstructed type and the declared type share no common subtype.

---

## Q-27: Mutation Fan-Out and Exposed-State Queries — **Partly shipped**

**The base procedure runs today.** `CALL cgx.mutation_fanout(<value>) YIELD mutator, confidence`
plans and executes as a real `derives-from` BFS walk, returning rows with an `exact` contract:

```console
$ cgx query 'CALL cgx.mutation_fanout("rust_sample::middle::return#1") YIELD mutator, confidence RETURN mutator, confidence' --repo .
mutator                   confidence
rust_sample::middle::v#0  probable
rust_sample::middle::y#1  certain
approximation: exact (within modeled graph)
freshness: current | indexed tree 7380e24, working tree clean
```

**What is deferred is narrower than "the feature":**

- The **`effect` / `transform` / `evidence` YIELD columns.** Ask for one and the plan error says
  exactly this — drop them and the procedure runs:
  ```
  plan error: procedure `cgx.mutation_fanout` does not yield column `effect`; it yields
  `mutator` and `confidence` (the `effect`/`transform`/`evidence` columns are not populated
  by the current frontend and are deferred)
  ```
- The **`cgx mutation-fanout` subcommand** — not in the `Command` enum; the CQL procedure is the
  only surface.
- The **`writes-param(i)` / `writes-receiver` effect-lattice extensions** (DF-17.2, GM-12) and
  alias edges (DF-16), both genuinely `schema-room`. The richer query families below rest on
  those and are not expressible yet.

**Read the direction caveat before using it.** `cgx.mutation_fanout` and `cgx.pedigree` are
currently **inverted relative to their column names** — `mutation_fanout` returns a value's
*sources* under a column called `mutator`. See DF-1.1a in
`docs/04-dataflow-and-provenance.md` for the reproduction. The rows above are `return#1`'s
sources, not values it mutates.

### Motivation and prior art position

**Pedigree** (DF-1) answers "what populated this value" — the inbound direction. **Mutation fan-out** (DF-17.3) is its outbound dual: "who can change this value after this point?" Both directions are needed to reason about a value's safety.

SpotBugs `EI_EXPOSE_REP` detects getter methods that return a reference to a mutable internal field — AST/bytecode pattern-matching, intraprocedural, Java only. It does not compute interprocedural write effects. Go escape analysis records whether a parameter escapes to the heap (for allocation optimization), not whether its contents are written. Rust's borrow checker prevents aliased mutation at compile time — this is a prevention mechanism, not a post-hoc query over existing code. No production tool computes `writes-param(i)` as a callable-effect summary that is queryable across a call graph. DF-17's effect-lattice extensions are novel as composable graph facts. (Closest prior art terminology: "parameter-effect summaries" or "write-effect summaries" in the research literature.)

### Query families

#### Who can mutate this value after this point?

```bash
# Mutation fan-out from 'user_record' after AuthHandler::validate returns
cgx query '
  MATCH (v {name:"user_record", scope:"AuthHandler::validate"})
  CALL cgx.mutation_fanout(v) YIELD mutator, effect, confidence
  RETURN mutator.name, mutator.file, mutator.line,
         effect,        -- writes-param(0), writes-receiver, mutation-out
         confidence
  ORDER BY confidence DESC, mutator.file
' --repo .
```

#### Does f(x) mutate x?

```bash
cgx query '
  MATCH (fn {name:"sort_records"})
  WHERE "writes-param(0)" IN fn.own_effects
     OR "writes-param(0)" IN fn.transitive_effects
  RETURN fn.name, fn.file, fn.line,
         fn.transitive_effects
' --repo .
```

#### Getters returning references to mutable internal state (EI_EXPOSE_REP-style)

```bash
cgx query '
  MATCH (getter:method)-[:MEMBER_OF]->(t)
  WHERE getter.name STARTS WITH "get"
    AND getter.return_mutability = "mutable"
    AND getter.return_is_internal_field = true
  RETURN getter.name, getter.file, getter.line,
         t.name AS containing_type
  ORDER BY t.name, getter.name
' --repo .
```

The `return_is_internal_field` attribute is set when the return value's pedigree traces to a field of `self`/`this` without a defensive copy. This is the composable graph-query equivalent of SpotBugs `EI_EXPOSE_REP`; unlike SpotBugs, it composes with pedigree and taint queries to answer "which code mutates the exposed field through the returned reference?"

#### Sanitization invalidation: cleared taint re-applied via mutation (DF-17.4)

```bash
cgx query '
  MATCH path = (sanitizer)-[:DATA_FLOW*]->(use {sink_class:"sql"})
  WHERE sanitizer.sanitizer_class = "sql"
    AND ANY(edge IN relationships(path)
            WHERE edge.taint_label.re_applied = true
              AND edge.taint_label.reason = "mutation-after-sanitization")
  RETURN sanitizer.name, sanitizer.file, sanitizer.line,
         use.name, use.file, use.line
' --repo .
```

### Layer 1 — Subcommand

```bash
# Mutation fan-out from a named value at a given function
cgx mutation-fanout <symbol-or-var> [--repo <path>] [--at-function <fn>] [--depth N] \
                    [--max-fanout N] [--through-sentinels] [--format <fmt>]

# Functions that mutate their first argument
cgx query --effect writes-param(0)   # Planned: no --effect flag exists
```

---

## Q-28: Closure-Capture Queries — **Planned (not yet shipped in v0.3.0)**

**Status: schema-room** — depends on capture edges (DF-18.3) with `by-value`/`by-ref` attribution and the captured variable's mutability level (DF-17.1). Both are `schema-room`. The query patterns are expressed once these edge attributes are populated.

### Motivation and prior art position

ESLint `no-loop-func` (JavaScript), Go `vet loopclosure`, and Python `flake8-bugbear B023` all detect the **symptom** of the loop-variable capture bug — a function defined inside a loop that references a mutable loop variable — as syntactic lint rules. They operate on scope analysis, not graph facts. Go 1.22 changed per-iteration variable scoping to eliminate the most common Go form of this bug, but does not model the capture. VTA (`golang.org/x/tools/go/callgraph/vta`) propagates function literals through type assignments but does not model individual capture edges with `by-value`/`by-ref` × mutability attributes.

cgx models captures as typed graph edges (DF-18.3), making the loop-capture family directly queryable and composable with escape (DF-16), mutation fan-out (DF-17.3), and resource-lifecycle (GM-13) analysis.

### Query families

#### Loop-variable capture bug family (by-ref capture of a mutable loop variable)

```bash
cgx query '
  MATCH (loop_var)-[cap:CAPTURE {capture:"by-ref"}]->(closure)
  WHERE loop_var.binding_mutability = "mutable"
    AND loop_var.is_loop_variable = true
  RETURN loop_var.name, loop_var.file, loop_var.line,
         closure.name,  closure.file, closure.line,
         cap.capture AS capture_mode
  ORDER BY loop_var.file, loop_var.line
' --repo .
```

This query is language-agnostic: it finds Go pre-1.22 goroutine closures capturing loop variables, JavaScript `var`-in-loop closures, and Python late-binding lambdas from a single pattern.

#### Captured-resource lifetime: closure captures a lock guard

```bash
cgx query '
  MATCH (resource)-[cap:CAPTURE]->(closure)
  WHERE resource.resource_class = "lock"
  RETURN resource.name, resource.file, resource.line,
         closure.name, closure.file, closure.line,
         cap.capture AS capture_mode
  ORDER BY resource.file, resource.line
' --repo .
```

A closure capturing a lock guard extends the resource lifecycle pair (GM-13): the lock is held until the closure executes, not until the enclosing scope exits. This is analogous to the `await-holding-lock` concurrency query (Q-24) but for captured (rather than actively held) locks.

#### Closures that outlive their frame (dangling capture risk)

```bash
cgx query '
  MATCH (v)-[cap:CAPTURE {capture:"by-ref"}]->(closure)
  MATCH escape_path = (closure)-[:DATA_FLOW*]->(sink)
  WHERE sink.scope_depth < v.scope_depth   -- closure escapes to outer scope
    AND v.binding_mutability = "mutable"
  RETURN v.name, v.file, v.line,
         closure.name, sink.name,
         sink.file, sink.line
' --repo .
```

### Layer 1 — Subcommand

```bash
# Find all closures with by-ref captures of mutable loop variables
cgx query --capture-bug loop-variable <path> [--format <fmt>]

# Find all closures capturing resources (lock guards, file handles, connections)
cgx query --capture-bug resource-lifetime <path> [--format <fmt>]
```

---

## Q-29: Higher-Order / Function-Value Call-Resolution Queries — **Partly shipped**

**The edge kinds ship and are queryable.** `calls:callback` and `calls:indirect` are real
`EdgeKind` variants, and the CQL `CALLS` token expands to the whole call family — `Calls`,
`CallsVirtual`, `CallsClosure`, `CallsCallback`, `CallsAsync`, `CallsIndirect` — so a
higher-order call is already traversable, and `r.kind` projects which flavour it was:

```bash
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE r.kind = "calls-indirect" RETURN a.fqn, b.fqn' --repo .
```

`lambda` is likewise a real symbol kind and matches in an inline predicate.

**What is Planned** is everything that would make those edges *resolve*: function-value nodes
(DF-18.1 — `SymbolKind` has no `FunctionValue` variant, so `:function-value` is a parse error),
pedigree-based indirect-call resolution (DF-18.2), and population of the `calls:indirect`
candidate set from a pedigree query. Today an indirect call is an edge you can find but whose
target set is not reconstructed.

### Overview

An indirect call site `f(args)` where `f` is a function value resolves by asking "what is the pedigree of `f`?" — the set of function values flowing to `f` at that call site is the candidate callee set. This is the pedigree query (DF-1) applied to function-value nodes. No new analysis is required: the existing pedigree machinery (DF-1), container models (DF-4), and function summaries (DF-3.3) answer the question directly.

Each candidate in the set carries the confidence label from how it was tracked through pedigree:

| Pedigree chain for the function value | Resolution confidence |
|--------------------------------------|-----------------------|
| Direct assignment from a named function | `certain` (no branch) or `probable` (branch merge) |
| Returned from a function whose return is resolved | Inherits the return edge's confidence |
| Stored in a container and retrieved | `probable` (smashed collection model, DF-4.2) |
| Passed through an opaque call or FFI boundary | `possible`; cut-marker `reflective` emitted |

### Layer 1 — Subcommand

```bash
# What functions can the callback argument to process() actually call?
cgx pedigree callback --repo . --at-function process --kind function-value

# Resolve indirect call sites in the auth module up to probable confidence
cgx callees '**' --repo . --indirect --confidence probable --path-filter 'auth/**'
```

### Layer 2 — Query language

**What functions can this callback actually invoke?**

```bash
cgx query '
  MATCH (callsite)-[:CALLS:indirect]->(target)
  WHERE callsite.scope = "process"
    AND callsite.arg_position = 0
  RETURN target.name, target.file, target.line,
         callsite.confidence AS resolution_confidence
  ORDER BY callsite.confidence DESC, target.name
' --repo .
```

**Unresolved indirect call sites (no candidate beyond possible):**

```bash
cgx query '
  MATCH (callsite)-[r:CALLS:indirect]->(target)
  WHERE r.confidence = "possible"
    AND size(callsite.candidate_set) = 0
  RETURN callsite.name, callsite.file, callsite.line,
         r.cut_marker
  ORDER BY callsite.file, callsite.line
' --repo .
```

**Function values that flow to multiple call sites (shared callbacks):**

```bash
cgx query '
  MATCH (fn_val:function-value)-[:DATA_FLOW*]->(site1)-[:CALLS:indirect]->()
  MATCH (fn_val)-[:DATA_FLOW*]->(site2)-[:CALLS:indirect]->()
  WHERE site1 <> site2
  RETURN fn_val.name, fn_val.file, fn_val.line,
         collect(distinct site1.name + "@" + site1.file + ":" + site1.line)
           AS call_sites
  ORDER BY fn_val.file, fn_val.line
' --repo .
```

---

## Q-30: Coercion and Type-Confidence Queries — **Planned (not yet shipped in v0.3.0)**

**Status: schema-room** — depends on `coerce(from,to)` transformation kind on DF-10 edges (`schema-room` as part of DF-10 vocabulary) and type-confidence boundaries (GM-14.6, `schema-room`). The query patterns are expressed once these attributes are populated.

### Query families

#### Type-juggling-in-auth (tainted value reaching a loose-equality comparison in an auth decision)

This query targets the class of bugs where PHP-style type coercion (`"0e123" == "0"` evaluates to `true` in loose comparison) or JavaScript loose equality (`"0" == 0`) allows an attacker-controlled value to satisfy an auth check through coercion rather than genuine equality. The `coerce(from,to)` edge on DF-10 marks the conversion point; Q-30 traces tainted values through those edges to auth-class sinks.

**Layer 2 — Full query:**

```bash
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(coerce_site)-[:DATA_FLOW {transformation_kind:"coerce"}]
                ->(sink {sink_class:"auth-check"})
  WHERE src.source_class IN ["network","user-input"]
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "auth-check")
  RETURN src.name, src.file, src.line,
         coerce_site.name, coerce_site.file, coerce_site.line,
         [e IN relationships(path) | e.transformation_kind] AS transformations,
         sink.name, sink.file, sink.line
  ORDER BY src.file, src.line
' --repo .
```

The `transformation_kind:"coerce"` filter selects only the edges where a type change occurs; the `sink_class:"auth-check"` targets comparisons or decisions that determine authentication or authorization outcomes.

**Layer 1 — Subcommand:**

```bash
cgx paths --from-class user-input --to-class auth-check \
          --require-transformation coerce \
          --confidence probable \
          <path>
```

#### The `any`-frontier: where does a dynamically typed value enter typed code?

Values of type `any` (TypeScript), `interface{}` / `any` (Go), `dynamic` (C#), or reflection results mark points where the static type system stops being evidence (GM-14.6). The `any`-frontier query finds these entry points — the trust-boundary analog for the type system.

```bash
cgx query '
  MATCH (boundary)-[r:DATA_FLOW]->(typed_receiver)
  WHERE boundary.type_confidence_boundary = true
    AND typed_receiver.declared_type IS NOT NULL
    AND typed_receiver.declared_type <> "any"
    AND typed_receiver.declared_type <> "interface{}"
  RETURN boundary.name, boundary.file, boundary.line,
         typed_receiver.name, typed_receiver.declared_type,
         r.transformation_kind
  ORDER BY boundary.file, boundary.line
' --repo .
```

When `type_confidence_boundary = true`, the node carries a GM-14.6 attribute indicating that downstream resolution confidence is at most `possible` without further assertion or reconstruction (DF-19).

#### Coercion chains — values that change type multiple times

```bash
cgx query '
  MATCH path = (src)-[:DATA_FLOW* {transformation_kind:"coerce"}]->(sink)
  WHERE length(path) >= 2
  RETURN src.name, src.file, src.line,
         [e IN relationships(path) | e.coerce_from + " → " + e.coerce_to] AS coercion_chain,
         sink.name, sink.file, sink.line
  ORDER BY length(path) DESC
  LIMIT 20
' --repo .
```

---

## Q-31: Framework-Aware Queries — **Planned (not yet shipped in v0.3.0)**

**Status: core-extension** — the query patterns here consume GM-15 `guard` and `entrypoint` facts and GM-18 `reflective` cut-marker edges, all of which are `core-extension`. The must-pass-through predicate (Q-20) and the pedigree query (DF-1) are already specified as core capabilities; Q-31 extends them by consuming the framework-pack-supplied metadata facts at query time.

### Metadata-guard-aware authorization bypass (headline framework query)

The Q-20 must-pass-through query finds handlers that reach a protected operation without passing through a named check function. On framework-heavy codebases this produces false positives: `@PreAuthorize("hasRole('ADMIN')")` is an authorization check with **no call in the source path** — the authz check is the annotation itself. Without GM-15 `guard` facts, must-pass-through queries report every annotated endpoint as a bypass.

GM-15 lowers `@PreAuthorize` (and equivalent annotations in other frameworks — Django `@login_required`, C# `[Authorize]`, Go middleware registration) to a `must-pass-through` fact on the symbol node. Q-31's metadata-guard-aware query consumes these facts:

**Layer 2 — Full query:**

```bash
cgx query '
  MATCH path = (h {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db::write"})
  WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
    AND NONE(n IN nodes(path)
             WHERE n.name = "require_admin"
                OR n.guard_class IS NOT NULL)   -- GM-15 guard fact on any node
  RETURN h.name, h.file, h.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY h.file, h.line
' --repo .
```

The `n.guard_class IS NOT NULL` predicate matches any node that carries a GM-15 `guard` semantic class, regardless of whether the guard is a function call or a metadata annotation. Without this clause, annotated endpoints produce false bypass findings.

**Layer 1 — Subcommand:**

```bash
# Authorization bypass: any handler reaches db::write without a guard
# (metadata guards satisfied by GM-15 guard facts — no false positives on
# annotated endpoints)
cgx paths --from 'kind:entrypoint' --to db::write --repo . \
    --avoiding 'name:require_admin OR guard_class:*' \
    --confidence probable \
    --format sarif > authz-bypass.sarif

# Assert no bypass exists (CI gate)
cgx paths --from 'kind:entrypoint' --to db::write --repo . \
    --avoiding 'name:require_admin OR guard_class:*' \
    --assert-empty
```

### Framework entrypoint reachability

When framework packs are active, entrypoints declared via metadata (`@GetMapping`, `#[tokio::main]`, `@Scheduled`, etc.) enter the entrypoint set automatically (GM-15.2). Reachability queries are correct for framework code without manual entrypoint declaration:

```bash
cgx query '
  MATCH (ep {kind:"entrypoint", entrypoint_class:"http"})-[:CALLS*]->(sink)
  WHERE sink.sink_class IN ["sql","shell","file_write"]
  RETURN ep.name, ep.file, ep.line,
         sink.name, sink.sink_class,
         ep.framework_pack AS established_by
  ORDER BY ep.framework_pack, ep.name
' --repo .
```

The `framework_pack` attribute records which pack promoted the symbol to `entrypoint`, enabling queries that audit coverage ("which frameworks have entrypoints in scope?").

### Negative-guard inventory: which endpoints disable a protection?

```bash
cgx query '
  MATCH (ep {kind:"entrypoint"})
  WHERE ep.negative_guard_class IS NOT NULL
  RETURN ep.name, ep.file, ep.line,
         ep.negative_guard_class AS disabled_protection,
         ep.framework_pack
  ORDER BY ep.negative_guard_class, ep.name
' --repo .
```

Negative guards (GM-15.1) are annotations that disable a protection — `@csrf_exempt` (Django), `[AllowAnonymous]` (ASP.NET), `#[allow(...)]` (Rust). This query enumerates all endpoints that carry such a fact, making the "which endpoints opt out of protection P?" question a single graph lookup.

### Tainted-string reflective dispatch

A tainted string reaching a reflective dispatch site means an attacker who controls that string controls which code executes. GM-18.2 emits a taint-flow finding when a tainted string flows to any reflective dispatch; Q-31 exposes this as a query.

The string pedigree attribute on reflective call edges (GM-18.1) is a novel framing in cgx: all mainstream tools share the same ceiling — literal-pedigree strings resolve to `probable` edges; dynamic strings are unresolved (TamiFlex via runtime instrumentation; DroidRA via COAL solver for Android; CodeQL Java Reflection for `Class.forName` literals). No production tool surfaces the string pedigree itself as a first-class graph attribute that downstream security queries can consume. cgx does.

```bash
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(dispatch {cut_marker:"reflective"})
  WHERE src.source_class IN ["network","user-input"]
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "method-name")
  RETURN src.name, src.file, src.line,
         dispatch.name, dispatch.file, dispatch.line,
         [e IN relationships(path) | e.transformation_kind] AS transforms,
         dispatch.string_pedigree
  ORDER BY src.file, src.line
' --repo .
```

The `string_pedigree` attribute on the dispatch node records whether the method-name string has a `literal`, `constant`, `joined`, or `tainted` provenance (GM-18.1). Queries can filter for `tainted` specifically to surface the highest-severity cases.

---

## Language-Primitives Worked Examples

The following three worked examples show both query layers for the headline query types introduced in Q-26, Q-30, and Q-31.

---

### Worked Example 7: Lineage Type Reconstruction and the Type-Contradiction Bug Query

**Question:** What type does the value `payload` actually carry in `RpcHandler::dispatch`, and does any value in the codebase have a reconstructed type that contradicts its declared type?

This demonstrates Q-26 (lineage type reconstruction, DF-19).

**Part A — Type discovery for a single value. Planned — `cgx type-of` is not a subcommand**
(it is absent from the 16-command `Command` enum), and `cgx.type_reconstruct` is not a
registered CQL procedure — the engine's own error names the only two that are:
`unknown procedure 'cgx.type_reconstruct' (known: 'cgx.mutation_fanout', 'cgx.pedigree')`.

```bash
# Planned (Q-26) — no `type-of` subcommand exists
# cgx type-of payload --repo . --at-function RpcHandler::dispatch
```

Illustrative output shape (not produced by any shipped command):

```json
{
  "symbol": "payload",
  "scope": "RpcHandler::dispatch",
  "declared_type": "interface{}",
  "reconstructed": [
    {
      "candidate_type": "*Config",
      "confidence": "probable",
      "evidence": [
        { "direction": "up",
          "kind": "constructor",
          "source": "caller.go:42: cfg := &Config{Host: \"db\"}",
          "confidence": "certain" },
        { "direction": "down",
          "kind": "typed-param-sink",
          "source": "rpc.go:17: store(payload) where store expects *Config",
          "confidence": "probable" }
      ]
    }
  ]
}
```

The `declared_type` is `interface{}` (a type-confidence boundary per GM-14.6); the reconstructed type is `*Config / probable` based on constructor pedigree (up, `certain`) and a typed-parameter use (down, `probable`).

**Part B — Codebase-wide type-contradiction query:**

```bash
cgx query '
  MATCH (v)
  WHERE v.declared_type IS NOT NULL
    AND v.declared_type <> "any"
    AND v.declared_type <> "interface{}"
  CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
  WHERE candidate_type IS EMPTY
  RETURN v.name, v.file, v.line,
         v.declared_type AS declared,
         evidence AS contradiction_evidence
  ORDER BY v.file, v.line
' --repo .
```

An empty candidate set means no type satisfies all three directions of constraints simultaneously — the value is used in two incompatible ways. This is a class of bug that no production static analysis tool exposes as an on-demand graph query.

**Variation — contradiction at a specific call site:**

```bash
cgx query '
  MATCH (v {name:"result", scope:"Processor::run"})
  CALL cgx.type_reconstruct(v, direction: "all") YIELD candidate_type, confidence, evidence
  RETURN v.name, v.file, v.line, candidate_type, confidence
' --repo .
```

---

### Worked Example 8: Type-Juggling-in-Auth

**Question:** Does any user-controlled value reach an authorization decision through a type coercion — a path where the value's type changes between input and auth check?

This demonstrates Q-30 (coercion and type-confidence queries, DF-10 `coerce(from,to)`).

**Subcommand:**

**Planned — not runnable.** `--from-class` / `--to-class` are Q-23 typed-taint flags and
`--require-transformation` is Q-30; none exists. Nor does the data underneath: `source_class`
and `sink_class` are rejected node properties, and `r.transformation` is rejected with
``edge property `transformation` is not populated by the current frontend``. There is no
`coerce` transformation kind in the schema at all — the shipped `Transform` enum is
`Copy, Projection, Arith, Concat, Composed, Branched, Aggregation, Parse, Serialize, Other`.

```bash
# Planned (Q-23 + Q-30) — none of these flags or classes exist
# cgx paths --from-class user-input --to-class auth-check \
#           --require-transformation coerce --confidence probable --repo .
```

**Full query:**

```bash
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(coerce_edge_target)-[:DATA_FLOW*]->(sink {sink_class:"auth-check"})
  WHERE src.source_class IN ["network","user-input"]
    AND ANY(edge IN relationships(path)
            WHERE edge.transformation_kind STARTS WITH "coerce")
    AND NONE(n IN nodes(path)
             WHERE n.sanitizer_class = "auth-check"
               AND NOT ANY(e IN relationships(path)
                           WHERE e.transformation_kind STARTS WITH "coerce"
                             AND position_in(coerce_edge_target, path)
                                 < position_in(n, path)))
  RETURN src.name, src.file, src.line,
         [e IN relationships(path) | e.transformation_kind] AS transforms,
         sink.name, sink.file, sink.line,
         length(path) AS hops
  ORDER BY src.file, src.line
' --repo .
```

The `ANY ... WHERE transformation_kind STARTS WITH "coerce"` selects paths that include at least one DF-10 `coerce(from,to)` edge. The `NONE` clause excludes paths where a sanitizer appears after the coercion. Non-empty results indicate that a user-controlled value reaches an auth decision through a type change — the value may satisfy the check through coercion rather than legitimate data.

---

### Worked Example 9: Metadata-Guard-Aware Authorization Bypass

**Question:** Which HTTP handler entrypoints reach `db::write` on a non-exception path without any authorization guard — either a function call or a metadata annotation?

This demonstrates Q-31 consuming GM-15 `guard` facts so that `@PreAuthorize`-style annotations do not generate false bypass findings.

**Subcommand:**

**Planned — not runnable, and unbuilt several layers deep.** `paths` takes two positional
anchors and has no `--from`/`--to`/`--avoiding`/`--exclude-edge-condition`;
`kind:entrypoint,entrypoint_class:http` is not an anchor syntax cgx parses; `entrypoint_class`
is a rejected node property, and the `HttpHandler` entrypoint kind that would populate it is
constructed **nowhere**, so no symbol in any indexed repository carries an http entrypoint
class; and `guard_class` / GM-15 metadata facts have zero occurrences in the codebase. Each is
independently blocking.

```bash
# Planned (Q-20 + Q-31) — anchors, guard classes and http entrypoint detection are all unbuilt
# cgx paths 'kind:entrypoint,entrypoint_class:http' db::write \
#           --avoiding 'name:require_admin OR guard_class:*' \
#           --exclude-edge-condition exception --assert-empty
```

The `--assert-empty` half *is* shipped and is the reusable part of this pattern: a gate that
exits 1 when results appear. What is missing is every anchor and filter that would make the
result mean "authorization bypass" rather than "reaches `db_write`".

**Full query:**

```bash
cgx query '
  MATCH path = (h {kind:"entrypoint", entrypoint_class:"http"})-[:CALLS*]->(sink {name:"db::write"})
  WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
    AND NONE(n IN nodes(path)
             WHERE n.name = "require_admin"
                OR n.guard_class IS NOT NULL)
  RETURN h.name, h.file, h.line,
         sink.name, sink.file, sink.line,
         h.framework_pack AS handler_established_by,
         length(path) AS hops
  ORDER BY hops, h.file, h.line
' --repo .
```

**Without** the `n.guard_class IS NOT NULL` clause, every handler annotated with `@PreAuthorize` (or equivalent) would appear as a bypass because the annotation's authz check has no call in the source path. With GM-15 `guard` facts present, `guard_class` is non-null on those methods and the `NONE` predicate correctly excludes them. This is the direct answer to the false-positive problem on Spring, Django, and ASP.NET codebases.

**Auditing guard coverage — which entrypoints have NO guard?**

```bash
cgx query '
  MATCH (ep {kind:"entrypoint", entrypoint_class:"http"})
  WHERE ep.guard_class IS NULL
    AND ep.name NOT IN ["health_check", "favicon"]
  RETURN ep.name, ep.file, ep.line,
         ep.framework_pack
  ORDER BY ep.framework_pack, ep.name
' --repo .
```

Zero results means every HTTP endpoint has at least one authz annotation or call-based guard — the ∀ coverage assertion for the authz layer.

---

### Object-model example: overrides that drop a base-class guard

**Question:** Which overrides reach the same database-write sink as their base method without passing through the authorization guard the base method enforced on all paths?

This demonstrates Q-32 (override-contract drift), specifically the dropped-guard sub-class. It applies corrected Q-20 ∀-path must-pass-through semantics: the guard set is derived from the base method's sub-graph rather than supplied by the user.

**Full query:**

```cypher
-- illustrative: requires Q-32 (core-extension; depends on corrected Q-20 ∀-path)
MATCH (sub:method)-[:OVERRIDES]->(base:method)
MATCH ALL (base)-[:CALLS*]->(sink {sink_class:"db-write"}) MUST PASS THROUGH (g {guard_class:"authz"})
MATCH ALL (sub)-[:CALLS*]->(sink {sink_class:"db-write"}) AVOIDING (g)
RETURN sub.name, sub.file, sub.line
```

**Breaking it down.** The first `MATCH ALL ... MUST PASS THROUGH` confirms that the base method routes every path to the sink through a guard with `guard_class:"authz"`. The second `MATCH ALL ... AVOIDING` finds the same sink reachable from the override on a path that avoids that guard — the dropped-guard pattern. Because Q-32 is a query-class feature that depends on the corrected Q-20 ∀-path semantics, the query is gated with the illustrative comment.

**Reading the result.** Non-empty results identify overrides that silently removed an authorization check the base method enforced. Each row names the override method and its source location. The guard set is derived from the base's sub-graph, not declared by the query author — this is the key difference from a plain Q-20 authorization-bypass query.

---

## Q-33: Recursion / SCC / Architecture-Cycle Queries — **Planned (not yet shipped in v0.3.0)**

**Status: roadmap.** Nothing in this section runs today, and the Feature List previously marked it Shipped — that was wrong. Specifically: `scc_id` and `scc_size` are not node properties (unknown-property plan error, and they are not even in the deferred list); `cgx unused --filter`, `cgx explain --show-scc` and `cgx query --scc-of` are not flags; and several of the CQL examples below additionally use `STARTS WITH`, which does not parse. Read the section as the design for SCC exposure, not as a usage guide.

Two things a reader can rely on today in place of it: `CALLS*` uses **simple-path semantics**, so a recursive cycle terminates the traversal after visiting each member once rather than looping; and a var-length traversal that hits a cycle renders `↺ name (cycle)` in the human forest.

### Motivation

A **strongly connected component** (SCC) of the call graph is a maximal set of functions that are mutually reachable — every function in the set can transitively call every other. A non-trivial SCC (size > 1) is a recursive cycle: the functions are directly or mutually recursive. Architecture-level cycles (module A depends on module B which depends on module A) are the cross-module analog.

SCC membership answers questions that simple reachability cannot: "Is function `f` part of a recursive group?", "What is the maximum depth of any non-recursive path?", "Which module pairs form an architectural cycle?"

### Schema representation

Each function node carries two derived attributes populated at index time:

- `scc_id` — an opaque identifier shared by all members of the same SCC. Functions not in any cycle have a unique `scc_id` (singleton SCC).
- `scc_size` — the number of members in the SCC. `scc_size = 1` means the function is non-recursive; `scc_size > 1` means it is part of a recursive cycle.

These attributes are derived from the call graph at index time using Tarjan's or Kosaraju's algorithm (petgraph `kosaraju_scc`). They are not stored as edges; they are node properties.

### Layer 1 — Subcommand

```bash
# List all functions in recursive cycles (scc_size > 1)
cgx unused --repo . --kind fn --filter scc_size_gt 1

# Show which SCC a given function belongs to
cgx explain crypto::hash --repo . --show-scc

# List all members of the SCC containing a named function
cgx query --scc-of crypto::hash --repo .
```

### Layer 2 — Query language

**Which functions are part of a recursive cycle?**

```cypher
MATCH (f)
WHERE f.scc_size > 1
RETURN f.name, f.file, f.line, f.scc_id, f.scc_size
ORDER BY f.scc_size DESC, f.scc_id, f.name
```

Non-empty results identify functions in mutual or direct recursion. A single large SCC may indicate an architecture smell: a cluster of tightly coupled functions with no clear call hierarchy.

**All members of the SCC containing a named function:**

```cypher
MATCH (root {name: "order_service::process"})
MATCH (member)
WHERE member.scc_id = root.scc_id
  AND member.scc_size > 1
RETURN member.name, member.file, member.line
ORDER BY member.name
```

**Architecture cycles: module pairs that mutually depend on each other:**

```cypher
-- Illustrative: file-path prefix used as module proxy
MATCH (a)-[:CALLS*]->(b)-[:CALLS*]->(a)
WHERE a.file STARTS WITH "src/auth/"
  AND b.file STARTS WITH "src/payment/"
  AND a <> b
RETURN DISTINCT a.name AS auth_fn, b.name AS payment_fn
ORDER BY auth_fn, payment_fn
LIMIT 20
```

**Maximum non-recursive path depth from an entrypoint (acyclic subgraph):**

```cypher
-- Find the longest path from any entrypoint, excluding recursive nodes
MATCH path = (ep {kind: "entrypoint"})-[:CALLS*]->(leaf)
WHERE NONE(n IN nodes(path) WHERE n.scc_size > 1)
  AND NOT (leaf)-[:CALLS]->()
RETURN ep.name, leaf.name, length(path) AS depth
ORDER BY depth DESC
LIMIT 10
```

### Reading the result

`scc_id` is an opaque integer assigned at index time; its value has no semantic meaning beyond grouping members of the same SCC. `scc_size` is the authoritative measure. A path query constrained to `scc_size = 1` nodes is guaranteed to terminate without the cycle concern (simple-path semantics still apply, but no cycle can exist among nodes all with `scc_size = 1`). Functions with `scc_size > 1` are those for which `CALLS*` traversal applies the simple-path cycle-cut.

---

## Known Limitations

The query engine reports **graph reachability**, not path feasibility. A path is returned if no edge on it is statically provable to be unreachable; the tool does not perform symbolic execution or SMT solving to rule out infeasible paths. This is an honest soundy approximation: no false negatives for graph-reachable paths, but some returned paths may be infeasible at runtime due to branch conditions not modeled at the call-graph level.

Dynamic dispatch, function pointers, closures, and `dyn Trait` objects are over-approximated (CHA-style candidate sets) and produce edges labeled `possible`. Filter with `--confidence certain` to remove them, with the understanding that some real edges may be excluded.

Reflection, runtime code generation, and foreign-function interface calls are labeled at the edge level and do not produce transitive call edges. The tool marks these as **blind spots** in the `explain` output.
