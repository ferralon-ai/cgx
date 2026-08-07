# cgx mental model

**Audience:** AI agents and engineers interpreting cgx output. Read this before trusting any result.
**Cross-refs:** `reference/cli.md` (flag syntax) · `reference/versions.md` (capability ladder) · `reference/query-language.md` (CQL)

---

## Before you read results

Run `cgx --version`. Parse `cgx 0.<MINOR>.<PATCH>`. A feature tagged `Since: v0.N` is available iff `MINOR ≥ N`.
The current shipped binary is **v0.3.0**. See `reference/versions.md` for the full ladder.

---

## 1. Nodes — symbol kinds

Every program entity is a **symbol** (a node). Symbols are identified by their **fully-qualified name (FQN)**, composed from enclosing scopes separated by `::`.

| Kind | What it is |
|---|---|
| `function` | Named, freestanding callable (`fn parse`, `def process`) |
| `method` | Callable bound to a type (`impl Foo { fn bar() }`) |
| `type` | Struct, class, interface, trait, enum, union |
| `field` | Named member of a type |
| `variable` | Local, parameter, or module-level binding |
| `module` | File-level or package-level namespace |
| `constant` | Compile-time named value |
| `macro` | Hygienic macro or preprocessor definition |
| `lambda` | Anonymous callable with a stable allocation site (closures, arrow functions) |
| `entrypoint` | Declared root for reachability (`main`, HTTP handlers, test functions) |

**`call-site` nodes** are a subordinate kind: they identify a specific call expression by `(caller, file, line, col)`. They never appear in `nodes(path)` and are never returned by symbol queries. (The `r.site` edge property is not selectable in CQL in v0.3; the site location appears in `cgx explain --format json` output.)

Every symbol carries: `kind`, `fqn`, `location` (file + start/end line), `language`, `visibility`, `confidence`.

**v0.3 population:** all node kinds are schema-present and populated. `lambda` captures are indexed. `call-site` attributes `cfg_block`/`dominating_sites`/`lock_set` remain schema-room.

**Source:** `docs/03-code-graph-model.md` GM-1.

---

## 2. Edges — edge kinds

Edges are directed, typed, attributed relations between symbols.

### 2.1 Call-edge family (populated)

| Edge type | Meaning |
|---|---|
| `CALLS` | Direct invocation of a callable |
| `CALLS:virtual` | Dispatch through a virtual/interface/trait method |
| `CALLS:closure` | Invocation of a closure or lambda |
| `CALLS:callback` | Invocation via a function-value argument |
| `CALLS:async` | Logical call across an async suspension boundary (`.await`) |
| `CALLS:indirect` | Call via function pointer without resolved target |
| `CALLS:super` | Statically-bound delegation to a named ancestor's body |

In CQL queries, use plain `[:CALLS]` to match the entire call-edge family. The `CALLS:<subtype>` qualifier syntax (e.g. `[:CALLS:super]`, `[:CALLS:virtual]`) is **recognised but not supported in v0.3** — it fails with exit 2 ("deferred, Theme-13"). Match the family with `[:CALLS]` and filter on `r.condition` in the WHERE clause instead. (The subtypes above describe how cgx *labels* edges internally; they are not yet selectable in a query.)

### 2.2 Structural and data-flow edges

**"Registered" and "populated" are two different claims here.** Every edge type below parses in
CQL and exits 0; only some are ever *emitted* by a language frontend or the resolver. A query
over a registered-but-unemitted type returns zero rows at exit 0 with `approximation.direction:
"exact"` — an empty answer that reads like a finding and is a modeling gap. Check this column
before concluding anything from an empty result.

| Edge type | Meaning | Status |
|---|---|---|
| `overrides` | A method overrides a method in a supertype | **emitted** (resolver, `trait-override`) |
| `implements` | A type implements an interface or trait | **emitted** (resolver, `impl-relation`) |
| `inherits` | A class inherits from a superclass | **emitted** (resolver, `supertrait`) |
| `instantiates` | Call site allocates an instance of a type | **emitted** (resolver; also drives RTA pruning) |
| `derives-from` | Value derivation (backbone of taint/pedigree). Edge orientation: **derived → source** (the derived node carries the outgoing edge to its source). Since v0.3. | **emitted (v0.3)** |
| `spawns` | Detached async launch (caller does not await result). Emitted as an *edge* by the Rust, Go, and Python frontends only; Java and TypeScript record a `spawns` **effect** on the enclosing symbol instead. | **emitted** |
| `contains` | Lexical containment (module→type, type→method) | **registered, never emitted** |
| `imports` | One module imports a symbol from another | **registered, never emitted** |
| `references` | Symbol uses another without calling it | **registered, never emitted** |
| `throws` / `catches` | Callable may propagate / handle an exception type | **registered, never emitted** |
| `reads-field` / `writes-field` | Method reads or writes a specific field | **registered, never emitted** |

**Seven** edge types exist in the edge enum, in the CQL edge-type table, and in `cgx diff --kind`'s
filter vocabulary, and are constructed by no `cgx-lang-*` frontend and no resolver pass. Measured on
one index (18,431 nodes; recipe below), the whole vocabulary:

```
CALLS 32607   DATA_FLOW 2165   INSTANTIATES 1371   OVERRIDES 52   IMPLEMENTS 22   INHERITS 3   SPAWNS 6
CONTAINS 0    IMPORTS 0        REFERENCES 0        THROWS 0       CATCHES 0       READS_FIELD 0   WRITES_FIELD 0
```

The zeros are not a property of that corpus: none of those seven has a construction site anywhere
outside tests. `references` is the one to watch, because it *looks* emitted — `link.rs:1193` maps
`RefKind::Reference => EdgeKind::References`, a fully wired consuming arm. But `grep -rn
'RefKind::Reference' crates/` returns **that one line and nothing else**: no frontend ever produces
the ref kind the arm consumes.

> **How this table got `references` wrong, and what it costs you.** An earlier revision listed it as
> emitted on the strength of that match arm, and explained the observed 0 rows away as a property of
> the fixture. A consuming match arm is not a producer, and a zero that disagrees with your reading
> of the code is evidence about the code. Where the two conflict here, the query wins — the same
> corpus returns 52 `OVERRIDES` and 1,371 `INSTANTIATES`, so a 0 is the graph speaking, not the
> query failing.

Do not write a query on one of the seven expecting rows, and do not read a zero-row answer from one
as evidence about the code.

<a id="the-self-index-corpus"></a>
**The corpus behind every count in this skill.** Absolute counts are meaningless without the exact
tree they were taken on — "an index of the cgx repo" is not a corpus, because which directories you
include changes every number. Every figure in `mental-model.md`, `cli.md`, `output-and-exit.md` and
`languages.md` comes from this one, called **the self-index corpus** below:

```bash
d=$(mktemp -d) && cp -R crates "$d/crates" && cp -R fixtures "$d/fixtures"
cd "$d" && git init -q . && git add -A && git -c user.email=a@b -c user.name=t commit -qm base
cgx index .     # → graph: 18431 nodes, 36226 edges, 11889 unresolved
```

Run from a checkout of the cgx repo. If your `cgx index` line does not read 18,431 nodes, you have a
different tree and the counts below will differ — the *directions* they illustrate still hold.

Field-level reads and writes are reachable today only through the *data-flow* layer
(`derives-from` / `[:DATA_FLOW]`, `flows-to`/`flows-from`), not through `reads-field`/
`writes-field`.

In CQL, `derives-from` edges are queryable as `[:DATA_FLOW]`. `DATA_FLOW` returns rows (exit 0). `PROVIDES_BODY` / `RESOLVES_TO` / `SHADOWS_FIELD` / `FULFILLS` edge types are still deferred — CQL plans using them return exit 2.

**DerivesFrom orientation.** The `derives-from` / `DATA_FLOW` edge points from the **derived** value to its **source**: if `b` is computed from `a`, the edge is `b → a`. Commands: `cgx flows-to` answers "what does this value propagate into?" (traverses `DATA_FLOW` edges in reverse, finding nodes that derive from the given node); `cgx flows-from` answers "what is this value derived from?" (traverses `DATA_FLOW` edges forward, following the derived→source direction).

**Source:** `docs/03-code-graph-model.md` GM-2.

---

## 3. Edge-condition labels (the guard lattice)

Every call edge carries exactly **one** edge-condition label. Compound labels are banned.

| Label | Meaning | Typical constructs |
|---|---|---|
| `always` | Edge taken on every execution of the call site | Unconditional call; also calls inside `finally`/`defer` (once that block is entered) |
| `conditional` | Taken only when an explicit boolean branch is true | `if`, `match` arm, ternary |
| `loop` | Taken zero or more times inside a loop body | `for`, `while`, iterator body |
| `exception` | Taken only during exceptional control flow | Java `catch`, Rust `Err(e)` arm / `?` on `Err`, Go `if err != nil`, C sentinel check |
| `panic` | Taken on an unwinding/aborting path that cannot be recovered | **Schema-only — no edge in any language carries it.** See below |

**⚠ `panic` is the one label with no observable instances.** It is a real value in the schema, in
the precedence rule below, and in `r.condition`'s accepted vocabulary — and a query filtered on it
returns **zero rows on every repo in every language**, at exit 0 with `direction: "exact"`.

Non-Rust adapters simply never assign it: Go's `panic()`/`recover()`, Java's `Error`/`System.exit`,
and the Python and TypeScript equivalents are not modeled. The Rust adapter does assign it, and
still nothing survives, for two independent reasons: the label is attached only to references
produced by ten **macros** (`panic!`, `assert!`, `todo!`, … — **not** `unwrap` or `expect`, which
are ordinary method calls the frontend never labels), and that reference targets the **external**
symbol `core::panicking::panic_fmt`, which resolves to nothing in-tree, so the edge is dropped
before the graph is written. Verified live on a fixture using all five idioms: 5 unresolved
references, and every surviving edge `always`.

The practical consequences: the exceptional class effectively means `exception` alone; the
precedence rule's top tier never fires; and **an empty `panic`-filtered result is a modelling gap,
not a safety finding**. To reason about Rust abort paths, query the callee by name (`unwrap`,
`expect`, a panicking helper) rather than the edge condition. Details in `reference/languages.md`.

**Exceptional class.** `exception` and `panic` together form the **exceptional class**. Query predicates for "non-exception paths" exclude both. `ANY`/`NONE` quantifiers in CQL let you filter on this at the path level.

**Precedence rule.** When a call site is nested inside multiple constructs, the label with the highest precedence wins: `panic > exception > loop > conditional > always`. Exception: calls inside `finally`/`defer` are always labeled `always` regardless of enclosing context; their exception-relativity is computed at query time (see §4).

All five labels are present and queryable as `r.condition` in CQL (e.g. `WHERE r.condition = "exception"`).

**Source:** `docs/03-code-graph-model.md` GM-3; `docs/13-glossary.md` "edge condition".

---

## 4. Path-relative transience

Transience is **not stored on edges** — it is computed per query during graph traversal.

An edge `C → D` is *exception-transient relative to path P* if and only if path P crosses at least one edge in the exceptional class (`exception` or `panic`) before reaching `C`. The same edge is not exception-transient relative to a different path `P'` that reaches `C` with no exceptional edge.

**Algorithm.** `cgx` maintains a per-walk boolean `seen_exceptional`. It initialises to `false` at the walk root. Crossing any `exception` or `panic` edge sets it to `true` irrevocably for that walk branch (backtracking resets to the fork-point value). A node `N` is exception-transient on a walk branch iff `seen_exceptional = true` when `N` is reached.

**Consequence for reading results.** The `always`-labeled edge `C → D` in a path output can be exception-transient if the path earlier crossed an `exception` edge. The edge label alone does not answer "is this call on an error-only path?" — the full path context does.

Fully computed. Use `ANY`/`NONE` quantifiers over a bounded path variable in CQL to filter on it.

**Source:** `docs/03-code-graph-model.md` GM-4; `docs/13-glossary.md` "transience".

---

## 5. Confidence ladder

Confidence is a **deterministic label** derived from the resolution algorithm. It is not a probability score.

| Value | Meaning | Typical resolution source |
|---|---|---|
| `certain` | Direct static binding; no ambiguity | Non-virtual call; monomorphized Rust generic; SCIP def/ref with exactly one target |
| `probable` | Heuristic with a small, credible candidate set | CHA/RTA virtual dispatch with few overrides; unique name match in scope; SCIP ref narrowed by type inference |
| `possible` | Over-approximated; candidate set large or unverified | Name-based match across all declarations; `dyn Trait` dispatch; function pointer without points-to; duck-typed name match |

**The resolution ladder (tiers 0–4):**

| Tier | Algorithm | Confidence emitted |
|---|---|---|
| 0 — Name/syntactic | Callee short name (+ arity) matched across every declared callable | **`possible`, always** — a lone surviving hit is a name *collision* with one survivor, not a resolution, so the band is clamped even for a singleton |
| 1 — Scope-graph | Scope-aware def/ref (tree-sitter-graph) | `certain` for a same-file bare name resolved in an enclosing scope (`scope-ref`); `probable` for a uniquely-resolved import binding (`import-ref`); for a candidate set, `probable` when one candidate survives and `possible` when several do |
| 2 — SCIP-enriched | Compiler-accurate def/ref from a supplied SCIP index | `probable` to `certain` |
| 3 — CHA/RTA | Class Hierarchy / Rapid Type Analysis over typed graph | `possible` for a multi-candidate set; `probable` where RTA narrows it |
| 4 — Points-to | Andersen/Steensgaard inclusion-based analysis | `certain` for monomorphic; `probable` for aliased |

**v0.3 ships tiers 0–3** (syntactic, scope-graph, SCIP enrichment where available, CHA/RTA). Tier 4 (points-to) is **not implemented** — no code stamps that tier.

**The resolver tries these in order, and stops at the first that hits:** same-file lexical
(`certain`) → import binding (`probable`) → same-name-method candidate set for a virtual or
duck-typed receiver (`name-method`) → the global short-name(+arity) fallback (`name-arity`,
`possible`) → unresolved.

Two consequences that change how you read results:

- **Step 4 is on by default**, so a reference is reported "unresolved" only when *no* symbol
  anywhere in the index shares its short name. Unresolved-call counts are therefore much
  narrower than the phrase suggests — they are not a measure of how much cgx failed to resolve.
- **A `probable` from step 3 can be uncorroborated.** The candidate set for a virtual or
  duck-typed call is demoted to `possible` only when it has more than one member; a singleton
  keeps `probable` even when the receiver's type was never resolved in-repo. `probable` at the
  scope-graph tier is also exactly the value that lets the approximation contract report
  `direction: "exact"`, so a single same-name method elsewhere in the tree can produce an answer
  that presents as exact. Read `rule` and `tier` in `cgx explain --format json` before trusting
  an `exact` on a method call. (Tracked as a known resolver defect; `resolve` is a frozen seam.)

**What this means for v0.3 results:** `certain` edges come from same-file scope resolution (or from SCIP, where an index was supplied); `probable` from a uniquely-resolved import binding, a singleton dispatch candidate set, or RTA narrowing; `possible` from over-approximated name-based matching and from any multi-candidate set. CHA/RTA raises edges to `probable`, not to `certain`. The majority of edges are `possible` on a real multi-crate project without a SCIP index — see the two runs below.

**Two real `cgx doctor` runs, and the reason to look at both.** Corpus size changes the
distribution more than anything else does, so a ratio quoted without its corpus is not a fact
you can carry. Left: the 5-file `fixtures/rust-sample` tree. Right: the self-index corpus (§2.2),
no SCIP.

```
# fixtures/rust-sample                  # the self-index corpus (18,431 nodes)
confidence distribution (call edges):   confidence distribution (call edges):
  certain:       75  (70.1%)              certain:     2968   (9.1%)
  probable:      13  (12.1%)              probable:    3662  (11.2%)
  possible:      19  (17.8%)              possible:   25983  (79.7%)
```

A toy fixture resolves almost everything intra-file at `certain`; a real multi-crate tree
without SCIP is ~80% `possible`. Read your own `cgx doctor` before assuming either shape.

Using `--confidence certain` as a CI gate can still pass vacuously — the vacuity guard (exit 4) exists to catch this. Use `--allow-vacuous` to suppress it when the vacuity is intentional.

**Weakest-link confidence.** A path is only as trustworthy as its weakest edge. `cgx` computes `MIN([e IN relationships(path) | e.confidence])` under `certain > probable > possible` for path-level confidence.

**Source:** `docs/03-code-graph-model.md` GM-5; `docs/13-glossary.md` "confidence", "certain", "probable", "possible", "weakest-link confidence".

---

## 6. File:line evidence (provenance)

Every node and edge carries a **provenance** record. This is a hard design constraint — no fact is emitted without it.

Provenance fields appear on each edge in `cgx explain --format json` output:

| Field | Content |
|---|---|
| `file` | Repo-relative path of the call site |
| `line` | 1-based line of the call site |
| `rule` | Name of the producing rule (e.g. `scope-ref`, `sig-compat`) |
| `tier` | Resolution tier as a string label: `name_syntactic`, `scope_graph`, `scip`, `cha_rta`, or `points_to` |
| `resolution_source` | Additional source detail (may be null) |

The `site` object on each `explain` edge contains `file` and `line`. Column information is not emitted in v0.3. There is no `--evidence` flag; use `cgx explain --format json` to inspect provenance fields.

**Source:** `docs/03-code-graph-model.md` GM-6; `docs/13-glossary.md` "file:line evidence", "provenance".

---

## 7. What is and is not in the v0.3 graph

### Populated in v0.3

- All symbol node kinds with `kind`, `fqn`, `location`, `language`, `visibility`, `confidence` attributes.
- Full CALLS-family edges with `condition`, `confidence`, `rule`, `tier`, `site`.
- Structural edges: `overrides`, `implements`, `inherits`, `instantiates`. (`contains`, `imports`, `references`, `throws`, `catches`, `reads-field` and `writes-field` are registered in the schema and queryable in CQL but **never emitted** — see § "Structural and data-flow edges".)
- Data-flow edges: `derives-from` (queryable as `[:DATA_FLOW]` in CQL). Populated by default; skipped when index is built with `--no-dataflow`.
- Detached-async edges: `spawns`.
- Syntactic edge-condition labels on every call edge: `always`, `conditional`, `exception`, `loop` from every shipped frontend. **`panic` is schema-only** — assigned by no adapter but Rust, and dropped before the graph even there (§ "Edge-condition labels").
- Cut markers: `reflective`, `dynamic`, `via-DI`, `via-FFI`, `unresolved`, `unexpanded-macro`.

### Still deferred in v0.3 (CQL returns exit 2)

| Feature | Status |
|---|---|
| `MUST PASS THROUGH` / `AVOIDING` path algebra | **plan** error — exit 2, intercepted by name and reported as `(deferred)` |
| Taint **node** properties (`entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class`) | plan error — exit 2, `(no backing field on a symbol node)` |
| Taint **edge** property (`taint_label`) — an edge property, not a node property | plan error — exit 2, `(no backing field on an edge)` |
| `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` edge types | plan error — exit 2 |
| `CALLS:<subtype>` qualifier syntax in CQL | plan error — exit 2 |
| `NOT IN [...]` in CQL WHERE | parse error — exit 2 |
| Lock sets, dominance facts on call-site nodes | schema-room |
| Dependency edges (package/version) | not yet |
| `r.site` and `r.via` edge properties in CQL SELECT/WHERE | plan error — exit 2, `(no backing field on an edge)` |

**Trust `reference/versions.md` for the authoritative capability ladder.**

---

## 8. Reading a result — checklist

1. **What confidence floor did you use?** Most edges are `possible` on typical projects without a SCIP index. A `possible` path may be a false positive from over-approximated dispatch. Raise to `--confidence probable` to reduce noise, understanding you will miss some real paths.
2. **What edge conditions are on the path?** An `exception`-conditioned edge means the callee is only reached during error handling. Use `ANY`/`NONE` quantifiers to filter. You will never see a `panic`-conditioned edge — the label is schema-only (§3), so do not build a filter that depends on one appearing.
3. **Is the path transient?** If the path includes an exceptional-class edge earlier, every subsequent `always` edge on the same path is exception-transient — the call only happens during error handling even though its own label says `always`.
4. **Cut markers.** A `reflective` or `via-DI` edge means `cgx` could not statically resolve the target. The edge is still emitted; decide whether to include or exclude it for your question.
5. **Empty result ≠ safe.** Exit 0 with no results means "no path found under these constraints" — not "the path is proven absent". Over-approximation (`possible` edges) means some real paths may be missing; under-approximation from filters means others are excluded.
6. **Read the approximation contract — and know which answers have one.** `callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `unused`, and `query` state which direction they can be wrong: a trailing `approximation:` line in human output, an `approximation` object in JSON. An answer that asserts an **absence** additionally carries `scope` — the edge kinds, confidence floor, and depth actually searched. Gate on the negative *with* its scope, never on the bare "no". The predicate is "is this an absence claim?", not "is the list empty?". The two come apart in both directions: `unused` carries `scope` on **every** answer, because "these symbols are not reached from any entrypoint" is an absence claim even when it returns thousands of results; `query` and `coupling` carry it on **no** answer, empty or not. Key on the field, not on the count. `exact` is always relative to `modeled_graph` (external/unindexed callees, undescended closure bodies, and unexpanded macros sit outside it).
7. **Check what the answer was computed *over*, not just what it says.** Every graph-backed answer carries a `freshness` envelope — a trailing `freshness: <current|stale|unknown> | indexed tree <oid>, working tree <…>` line in human output, a top-level `freshness` object in JSON, a `freshness` key in MCP `structuredContent`. `stale: true` means the answer describes code you are no longer looking at — either the index is behind `HEAD` *or* the working tree is dirty, so `stale` is **not** the negation of `matches_head`; an index that matches `HEAD` under a dirty tree is `stale` with `matches_head: true`. `matches_head` itself is **three-valued** — `true`, `false`, or `null`/`unknown` — and `null` is the ordinary result once a repo has been indexed, so branch on three values rather than treating it as a boolean. Four commands carry no envelope at all: `index`, `doctor`, `diff`, and `coupling` — the last because it answers from committed git history and never opens the index, so freshness would describe a store it never read.
8. **Where there is no contract, you supply the scope.** `index`, `explain`, `search`, `symbols`, `doctor`, and `diff` carry no `approximation` and no `scope`. A missing contract is not a claim of exactness. This bites hardest on `cgx diff --path-added`, whose clean exit *is* an absence claim on a PR gate: rule 6's "gate on the negative with its scope" is not something the output can do for you there. Record the flags you gated with — `--from`/`--to`, `--kind`, `--edge-condition` — because they *are* the scope, and nothing in the answer restates them. (`--confidence` is not among them: `cgx diff` does not accept it, and passing it is exit 2, `unexpected argument`. A `--path-added` gate is run at the graph's own confidence floor, which you cannot raise.) See `reference/output-and-exit.md` §1.2.

---

## See also

- `reference/cli.md` — exact subcommands, flags, and defaults
- `reference/versions.md` — full version ladder and capability dates
- `reference/query-language.md` — CQL syntax, supported/unsupported clauses
- `reference/output-and-exit.md` — output formats, exit codes, CI assertion mode
