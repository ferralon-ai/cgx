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

| Edge type | Meaning | Status |
|---|---|---|
| `contains` | Lexical containment (module→type, type→method) | present |
| `imports` | One module imports a symbol from another | present |
| `overrides` | A method overrides a method in a supertype | present |
| `implements` | A type implements an interface or trait | present |
| `inherits` | A class inherits from a superclass | present |
| `references` | Symbol uses another without calling it | present |
| `instantiates` | Call site allocates an instance of a type | present |
| `throws` / `catches` | Callable may propagate / handle an exception type | present |
| `reads-field` / `writes-field` | Method reads or writes a specific field | present |
| `derives-from` | Value derivation (backbone of taint/pedigree). Edge orientation: **derived → source** (the derived node carries the outgoing edge to its source). Since v0.3. | **present (v0.3)** |
| `spawns` | Detached async launch (caller does not await result). Emitted as an *edge* by the Rust, Go, and Python frontends only; Java and TypeScript record a `spawns` **effect** on the enclosing symbol instead. | **present** |

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
| `panic` | Taken on an unwinding/aborting path that cannot be recovered | Rust `panic!`/`unwrap`/`expect` — **Rust only**, see below |

**`panic` is emitted by the Rust frontend alone.** The label is part of the schema and the
precedence rule below, but no other shipped adapter produces it: Go's `panic()`/`recover()`,
Java's `Error`/`System.exit`, and the Python and TypeScript equivalents are not modeled. On a
non-Rust repo, `--edge-condition panic` is empty because nothing emits the label, and the
exceptional class there means `exception` alone.

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
| 0 — Name/syntactic | Callee name + arity match across all declared functions | `possible` |
| 1 — Scope-graph | Scope-aware def/ref (tree-sitter-graph) | `probable` for resolved; `possible` for ambiguous |
| 2 — SCIP-enriched | Compiler-accurate def/ref from a SCIP index | `probable` to `certain` |
| 3 — CHA/RTA | Class Hierarchy / Rapid Type Analysis over typed graph | `probable` |
| 4 — Points-to | Andersen/Steensgaard inclusion-based analysis | `certain` for monomorphic; `probable` for aliased |

**v0.3 ships tiers 0–3** (syntactic, scope-graph, SCIP enrichment where available, CHA/RTA). Tier 4 (points-to) is planned.

**What this means for v0.3 results:** `certain` edges come from scope-graph or CHA/RTA resolution with unique targets; `probable` from heuristic narrowing; `possible` from over-approximated name-based matching. The majority of edges are still `possible` on typical projects without a SCIP index.

**Example distribution (from `cgx doctor` on the rust-sample corpus):**

```
confidence distribution (call edges):
  certain:     2051  (10.9%)
  probable:    3691  (19.6%)
  possible:   13060  (69.5%)
```

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
- Structural edges: `contains`, `imports`, `overrides`, `implements`, `inherits`, `references`, `instantiates`, `throws`, `catches`, `reads-field`, `writes-field`.
- Data-flow edges: `derives-from` (queryable as `[:DATA_FLOW]` in CQL). Populated by default; skipped when index is built with `--no-dataflow`.
- Detached-async edges: `spawns`.
- Syntactic edge-condition labels on every call edge: `always`, `conditional`, `exception`, `loop` from every shipped frontend; `panic` from the Rust frontend only (§ "Edge-condition labels").
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
2. **What edge conditions are on the path?** An `exception`-conditioned edge means the callee is only reached during error handling. A `panic`-conditioned edge is on an abort path. Use `ANY`/`NONE` quantifiers to filter.
3. **Is the path transient?** If the path includes an exceptional-class edge earlier, every subsequent `always` edge on the same path is exception-transient — the call only happens during error handling even though its own label says `always`.
4. **Cut markers.** A `reflective` or `via-DI` edge means `cgx` could not statically resolve the target. The edge is still emitted; decide whether to include or exclude it for your question.
5. **Empty result ≠ safe.** Exit 0 with no results means "no path found under these constraints" — not "the path is proven absent". Over-approximation (`possible` edges) means some real paths may be missing; under-approximation from filters means others are excluded.
6. **Read the approximation contract — and know which answers have one.** `callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `unused`, and `query` state which direction they can be wrong: a trailing `approximation:` line in human output, an `approximation` object in JSON. A negative answer from one of those additionally carries `scope` — the edge kinds, confidence floor, and depth actually searched. Gate on the negative *with* its scope, never on the bare "no". `exact` is always relative to `modeled_graph` (external/unindexed callees, undescended closure bodies, and unexpanded macros sit outside it).
7. **Where there is no contract, you supply the scope.** `index`, `explain`, `search`, `symbols`, `doctor`, and `diff` carry no `approximation` and no `scope`. This bites hardest on `cgx diff --path-added`, whose clean exit *is* an absence claim on a PR gate: rule 6's "gate on the negative with its scope" is not something the output can do for you there. Record the flags you gated with — `--from`/`--to`, `--kind`, `--edge-condition`, `--confidence` — because they *are* the scope, and nothing in the answer restates them. See `reference/output-and-exit.md` §1.2.

---

## See also

- `reference/cli.md` — exact subcommands, flags, and defaults
- `reference/versions.md` — full version ladder and capability dates
- `reference/query-language.md` — CQL syntax, supported/unsupported clauses
- `reference/output-and-exit.md` — output formats, exit codes, CI assertion mode
