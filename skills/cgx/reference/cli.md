# cgx CLI Reference

**Audience:** Engineers and AI agents driving cgx from the command line.

Run `cgx --version` first. Parse the `0.<MINOR>.<PATCH>` after `cgx `. A capability tagged
`Since: v0.N` is available **iff MINOR ≥ N**. The current shipped binary is **v0.3** (`cgx 0.3.0`).
See `reference/versions.md` for the full ladder.

> **Phantom flags — do not emit these.** They appear in upstream cookbook examples but cause
> exit 2 in the real binary: `--max-depth`, `--base`, `--head`, `--from`, `--to`, `--from-class`,
> `--to-class`, `--avoiding`, `--only-edge-condition`, `--calls-to-sink-class`, `--kind fn`,
> and a trailing `./` positional in place of `--repo ./`. The correct names are in the
> signatures below. See ground truth §F for the complete list.

---

## Index store

The index lives at `.cgx/` in the repository root (`HEAD.json` + `index.db` SQLite).
Auto-index is **on by default**: a missing `.cgx/` triggers an automatic build before the
first query. Pass `--no-auto-index` to require an explicit index; the command exits 3 if
the index is absent. Force a rebuild at any time with `cgx index .`. There is no
`prune` or `clean` subcommand.

---

## Subcommands

### `index` — build or rebuild the graph index

Since: v0.1

```
cgx index <PATH>
```

| Argument | Default | Notes |
|---|---|---|
| `<PATH>` | required | Repository root or any subdirectory |

**Example:**
```bash
cgx index .
```

---

### `callers` — find symbols that call a given symbol

Since: v0.1

```
cgx callers [OPTIONS] <SYMBOL>
```

| Argument / Flag | Default | Notes |
|---|---|---|
| `<SYMBOL>` | required | Exact qualified symbol name |
| `--repo <PATH>` | CWD | Repository root |
| `--format <FMT>` | `human` | `human`, `json`, `sarif`, `dot`, `mermaid`, `d2` |
| `--tree <MODE>` | `full` | Human forest shape: `full` (expand every call edge) or `spanning` (each symbol once, `(+N call sites)` for extra callers). Human format only. |
| `--at <REF>` | HEAD | Pin query to git ref |
| `--depth <N>` | unlimited (forest: 2) | Traversal depth. The human forest applies a default depth of 2 when unset; an explicit value overrides it. |
| `--confidence <LEVEL>` | `possible` | Minimum floor: `possible`, `probable`, `certain` |
| `--assert-empty` | off | CI: exit 1 if results found |
| `--allow-vacuous` | off | Suppress exit 4 vacuity guard |
| `--no-auto-index` | off | Exit 3 if index missing instead of auto-building |

The default human view of `callers`/`callees` is an ASCII call **forest** (the
anchor is the root; callers/callees hang off `├─ │ └─` box-drawing prefixes). See
[output-and-exit.md](output-and-exit.md). Machine formats are unchanged.

**Example:**
```bash
cgx callers MyModule::my_fn --depth 3 --format json
cgx callees MyModule::my_fn --tree spanning
```

---

### `callees` — find symbols a given symbol calls

Since: v0.1

```
cgx callees [OPTIONS] <SYMBOL>
```

Accepts the same flags as `callers`.

**Example:**
```bash
cgx callees MyModule::my_fn --depth 2 --format sarif
```

---

### `reaches` — test whether one symbol can reach another

Since: v0.1

```
cgx reaches [OPTIONS] <FROM> [TO]
```

| Argument / Flag | Default | Notes |
|---|---|---|
| `<FROM>` | required | Source symbol (exact qualified name) |
| `[TO]` | optional | Target symbol; omit to enumerate all reachable symbols |
| `--tree <MODE>` | `full` | Forest shape for the `reaches <FROM>` (no `TO`) human view; see `callers`. Ignored for `reaches <FROM> <TO>`. |
| `--repo`, `--format`, `--at`, `--depth`, `--confidence`, `--assert-empty`, `--allow-vacuous`, `--no-auto-index` | — | See shared-flags table |

`reaches <FROM>` (no `TO`) enumerates the reachable set and renders it as the
human forest, like `callees`. `reaches <FROM> <TO>` answers a single
reachability question and renders its witness **path** (not a forest), so the
`dot`/`mermaid`/`d2` graph formats apply to it as before.

**Example:**
```bash
cgx reaches FromFn ToFn --format dot
cgx reaches FromFn --tree spanning
```

---

### `paths` — enumerate call paths between two symbols

Since: v0.1

```
cgx paths [OPTIONS] <FROM> <TO>
```

| Argument / Flag | Default | Notes |
|---|---|---|
| `<FROM>` | required | Source symbol |
| `<TO>` | required | Target symbol |
| `--depth <N>` | 6 | `0` = unlimited (work-budgeted) |
| `--repo`, `--format`, `--at`, `--confidence`, `--assert-empty`, `--allow-vacuous`, `--no-auto-index` | — | See shared-flags table |

**Example:**
```bash
cgx paths FromFn ToFn --depth 0 --format mermaid
```

---

### `explain` — show provenance for a symbol

Since: v0.1

```
cgx explain [OPTIONS] <SYMBOL>
```

| Argument / Flag | Default | Notes |
|---|---|---|
| `<SYMBOL>` | required | Exact qualified symbol name |
| `--repo <PATH>` | CWD | |
| `--format <FMT>` | `human` | `human`, `json`, `sarif`, `dot`, `mermaid`, `d2` |
| `--no-auto-index` | off | |

`explain` does **not** accept `--at`, `--assert-empty`, or `--confidence`.

**Example:**
```bash
cgx explain MyModule::my_fn --format json
```

---

### `query` — run a CQL expression against the call graph

Since: v0.1 (CALLS-graph subset only — see `reference/query-language.md`)

```
cgx query [OPTIONS] <QUERY>
```

| Argument / Flag | Default | Notes |
|---|---|---|
| `<QUERY>` | required | Inline CQL string or `@/path/to/file.cql` |
| `--repo`, `--format`, `--at`, `--depth`, `--confidence`, `--assert-empty`, `--allow-vacuous`, `--no-auto-index` | — | See shared-flags table |

**Shell quoting rule:** wrap the whole query in single quotes; use double quotes inside
for string literals. Single-quoted strings cause a parse error (`unexpected character '\''`).

**Example:**
```bash
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_fn" RETURN a.name, a.file LIMIT 10'
```

**What works today (v0.1):** `MATCH (a)-[:CALLS]->(b)`, node props `name/kind/file/line`,
edge props `condition/confidence`, `IN [..]`, `<>`, bounded `*N` hops, `ANY/NONE`
quantifiers, `@file.cql`, `--at`. See `reference/query-language.md` for the full
supported/unsupported clause list.

> **Never emit unbounded `CALLS*` — it hangs.** Always bound the hops: `CALLS*2`.

---

### `search` — find symbols by partial name

Since: v0.2

```
cgx search [OPTIONS] <PATTERN>
```

A pure node-table scan (no graph walk). Resolves a partial or half-remembered name to exact FQNs for use
with `callers`/`callees`/`reaches`. Default match is a case-insensitive substring over the whole FQN;
`--regex` switches to a full regex match.

| Argument / Flag | Default | Notes |
|---|---|---|
| `<PATTERN>` | required | Substring (default) or regex (`--regex`) matched against the full FQN |
| `--regex` | off | Treat `<PATTERN>` as a regex over the whole FQN. Invalid regex → exit 2 |
| `--kind <KIND>` | (all) | Restrict to a symbol kind: `function`, `method`, `type`, `field`, `variable`, `module`, `constant`, `macro`, `lambda`, `entrypoint`. Unknown value → exit 2 |
| `--limit <N>` | `50` | Max results to print. `0` = unlimited. Truncated output adds a footer `… (N more — raise --limit)` |
| `--repo <PATH>` | CWD | Repository root |
| `--format <FMT>` | `human` | `human` or `json`. JSON shape: `[{ "file", "fqn", "kind", "line" }]` |
| `--no-auto-index` | off | Exit 3 if index missing instead of auto-building |

**Exit codes:** empty result → exit 0 (unlike exact-symbol commands which exit 2 on `no symbol matched`).
Bad regex or unknown `--kind` value → exit 2. Bare `cgx search` (no pattern) → exit 2.

**Output columns (human):** FQN (left-aligned), `file:line`, `[kind]`. Sorted by FQN for deterministic output.

**Examples:**
```bash
cgx search Counter                              # substring match, case-insensitive
cgx search make --kind function                 # narrow to functions only
cgx search 'derive_key' --regex                 # regex over the full FQN
cgx search Counter --format json                # JSON array output
cgx search auth --limit 0                       # unlimited results
```

---

### `flows-to` — forward data-flow slice from a value node

Since: v0.3

```
cgx flows-to [OPTIONS] <VALUE-NODE>
```

Requires a `--dataflow` index (`cgx index --dataflow .`). Traverses `DerivesFrom` edges
forward from the named value node to show what values it flows into.

`<VALUE-NODE>` is a synthetic FQN of the form `<fn>::<local>#<ver>`. Use `cgx search`
to locate the exact name:

```bash
cgx search process_request --kind variable   # find value-node FQNs in scope
```

| Argument / Flag | Default | Notes |
|---|---|---|
| `<VALUE-NODE>` | required | Exact value-node FQN (`<fn>::<local>#<ver>`) |
| `--confidence <LEVEL>` | `possible` | Minimum floor: `possible`, `probable`, `certain` |
| `--at <REF>` | HEAD | Pin query to git ref |
| `--depth <N>` | unlimited | Traversal depth cap |
| `--tree <full|spanning>` | `full` | Output tree shape |
| `--repo <PATH>` | CWD | Repository root |
| `--format <FMT>` | `human` | `human`, `json`, `sarif`, `dot`, `mermaid`, `d2` |

Exit codes follow the standard contract: unknown value node → exit 2
(`no symbol matched`); empty result → exit 0. See `reference/output-and-exit.md`.

**Edge-condition rendering:** omit `always`; `conditional` renders as `if`;
`exception` renders as `exc`; `loop` and `panic` render verbatim.

**Example:**
```bash
cgx flows-from "<fn::local#1>" --confidence probable --depth 4 --tree spanning
```

---

### `flows-from` — backward data-flow slice (pedigree) from a value node

Since: v0.3

```
cgx flows-from [OPTIONS] <VALUE-NODE>
```

Requires a `--dataflow` index. Traverses `DerivesFrom` edges backward from the named
value node to show where it originates. Accepts the same flags as `flows-to`.

**Example:**
```bash
cgx flows-from 'write_log::msg#1' --confidence probable
```

---

### `unused` — find symbols not reachable from any entrypoint

Since: v0.1

```
cgx unused [OPTIONS]
```

| Flag | Default | Notes |
|---|---|---|
| `--kind <KIND>` | (all) | `function`, `method`, `type`, `field`, `variable`, `module`, `constant`, `macro`, `lambda`, `entrypoint` — use the full word, not `fn` |
| `--repo`, `--format`, `--confidence`, `--assert-empty`, `--allow-vacuous`, `--no-auto-index` | — | See shared-flags table |
| `--depth`, `--at` | — | Accepted (exit 0) but inert for the whole-graph unused computation |

`unused` has **no name or pattern filter**. To find unused symbols matching a pattern,
run `cgx unused` (optionally with `--kind`) then grep the output.

**Example:**
```bash
cgx unused --kind function --format sarif
```

---

### `doctor` — inspect index health

Since: v0.1

```
cgx doctor [OPTIONS]
```

| Flag | Default | Notes |
|---|---|---|
| `--repo <PATH>` | CWD | |
| `--format <FMT>` | `human` | `human`, `json` |

`doctor` does not accept `--at`, `--assert-empty`, `--confidence`, or `--no-auto-index`.

**Example:**
```bash
cgx doctor --format json
```

Sample output:
```
trust:  HIGH   — index looks sound
nodes:           2798
edges:          13185  (total)
call edges:     13171  (call-family)
confidence distribution (call edges):
  certain:     1369  (10.4%)
  probable:    2823  (21.4%)
  possible:    8979  (68.2%)
unresolved references: 0/13171 refs unresolved  (0.0%)
anomalies:  none
```

---

### `diff` — compute graph diff between two refs

Since: v0.1 (`--newer-than` only; full diff filters at v0.4)

```
cgx diff [OPTIONS] <BASE> <HEAD>
```

| Argument / Flag | Default | Notes |
|---|---|---|
| `<BASE>` | required | Base ref — **positional**, not `--base` |
| `<HEAD>` | required | Head ref — **positional**, not `--head` |
| `--newer-than` | off | Only new edges added at `<HEAD>` |
| `--repo <PATH>` | CWD | |
| `--format <FMT>` | `human` | `human`, `json`, `sarif`, `dot`, `mermaid`, `d2` |

`diff` does not accept `--at`, `--assert-empty`, `--confidence`, or `--no-auto-index`.

**Example:**
```bash
cgx diff HEAD~1 HEAD --format json
```

---

### `mcp` — start the MCP STDIO server

Since: v0.1

```
cgx mcp [OPTIONS]
```

| Flag | Default | Notes |
|---|---|---|
| `--root <PATH>` | CWD | Repository root served by the MCP server |

Reads from stdin, writes to stdout using MCP STDIO transport (NDJSON). Five functional
tools: `callers`, `callees`, `paths`, `unused`, `explain`. The `graph_query` tool is
registered but **always returns an unimplemented error** — use `cgx query` via CLI
instead. See `reference/mcp.md` for full tool schemas and MCP-vs-CLI differences.

**Example:**
```bash
cgx mcp --root /path/to/repo
```

---

## Shared flags — subcommand applicability

| Flag | callers | callees | reaches | paths | query | search | unused | explain | doctor | diff | mcp | flows-to | flows-from |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `--repo <PATH>` | Y | Y | Y | Y | Y | Y | Y | Y | Y | Y | — | Y | Y |
| `--format <FMT>` | Y | Y | Y | Y | Y | Y† | Y | Y | Y | Y | — | Y | Y |
| `--at <REF>` | Y | Y | Y | Y | Y | — | Y* | — | — | — | — | Y | Y |
| `--depth <N>` | Y | Y | Y | Y | Y | — | Y* | — | — | — | — | Y | Y |
| `--tree <full|spanning>` | — | — | — | — | — | — | — | — | — | — | — | Y | Y |
| `--confidence <LEVEL>` | Y | Y | Y | Y | Y | — | Y | — | — | — | — | Y | Y |
| `--assert-empty` | Y | Y | Y | Y | Y | — | Y | — | — | — | — | — | — |
| `--allow-vacuous` | Y | Y | Y | Y | Y | — | Y | — | — | — | — | — | — |
| `--no-auto-index` | Y | Y | Y | Y | Y | Y | Y | Y | — | — | — | Y | Y |
| `--newer-than` | — | — | — | — | — | — | — | — | — | Y | — | — | — |

**`--format` values:** `human` (default), `json`, `sarif`, `dot`, `mermaid`, `d2`.
`dot`, `mermaid`, and `d2` are meaningful only for path-shaped results (`reaches`,
`paths`, or `RETURN path` queries).

**`†` (on `search`):** `search` accepts only `human` and `json`. `sarif`, `dot`, `mermaid`, and `d2` are not
meaningful for a symbol-list result and are not accepted.

**`*` (on `unused`):** the binary accepts `--at`, `--depth`, and `--confidence` on
`unused` (exit 0); `--depth`/`--at` have no effect on the unused-symbol computation,
which is whole-graph. `--confidence` applies the edge-confidence floor when deciding
whether an incoming edge counts as a caller.

**`--confidence` values:** `possible` (default floor), `probable`, `certain`.
Note: `certain` and `probable` are only discriminating after SCIP enrichment (v0.2).
In v0.1 most edges are labeled `possible`; an `--assert-empty --confidence certain`
gate may pass vacuously (exit 4). See `reference/output-and-exit.md`.

---

## See also

- `reference/query-language.md` — supported and unsupported CQL clauses for `cgx query`
- `reference/output-and-exit.md` — output format details, exit-code contract, CI assertion mode
- `reference/mcp.md` — MCP tool schemas, MCP-vs-CLI defaults, pagination
- `reference/versions.md` — full version ladder and capability since-tags
