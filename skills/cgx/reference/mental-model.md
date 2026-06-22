# cgx mental model

**Audience:** AI agents and engineers interpreting cgx output. Read this before trusting any result.
**Cross-refs:** `reference/cli.md` (flag syntax) · `reference/versions.md` (capability ladder) · `reference/query-language.md` (CQL)

---

## Before you read results

Run `cgx --version`. Parse `cgx 0.<MINOR>.<PATCH>`. A feature tagged `Since: v0.N` is available iff `MINOR ≥ N`.
The current shipped binary is **v0.1**. See `reference/versions.md` for the full ladder.

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

**`call-site` nodes** are a subordinate kind: they identify a specific call expression by `(caller, file, line, col)`. They never appear in `nodes(path)` and are never returned by symbol queries — access them via `r.site` in a CQL edge join.

Every symbol carries: `kind`, `fqn`, `location` (file + start/end line), `language`, `visibility`, `confidence`.

**v0.1 population:** all node kinds are schema-present. `lambda` captures, `call-site` attributes `cfg_block`/`dominating_sites`/`lock_set` are `schema-room` — reserved but not populated until v0.3.

**Source:** `docs/03-code-graph-model.md` GM-1.

---

## 2. Edges — edge kinds

Edges are directed, typed, attributed relations between symbols.

### 2.1 Call-edge family (v0.1, populated)

| Edge type | Meaning |
|---|---|
| `CALLS` | Direct invocation of a callable |
| `CALLS:virtual` | Dispatch through a virtual/interface/trait method |
| `CALLS:closure` | Invocation of a closure or lambda |
| `CALLS:callback` | Invocation via a function-value argument |
| `CALLS:async` | Logical call across an async suspension boundary (`.await`) |
| `CALLS:indirect` | Call via function pointer without resolved target |
| `CALLS:super` | Statically-bound delegation to a named ancestor's body |

In CQL queries, use plain `[:CALLS]` to match the entire call-edge family. The `CALLS:<subtype>` qualifier syntax (e.g. `[:CALLS:super]`, `[:CALLS:virtual]`) is **recognised but not supported in v0.1** — it fails with exit 2 ("deferred, Theme-13"). Match the family with `[:CALLS]` and read each edge's attributes from the result instead. (The subtypes above describe how cgx *labels* edges internally; they are not yet selectable in a query.)

### 2.2 Structural and data-flow edges

| Edge type | Meaning | v0.1 status |
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
| `derives-from` | Value derivation (backbone of taint/pedigree) | **schema-room — empty until v0.3** |
| `spawns` | Detached async launch (caller does not await result) | **schema-room — empty until v0.3** |

`DATA_FLOW` edges and `PROVIDES_BODY` / `RESOLVES_TO` / `SHADOWS_FIELD` / `FULFILLS` edge types are deferred: CQL plans using them return exit 2 today.

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
| `panic` | Taken on an unwinding/aborting path that cannot be recovered | Rust `panic!`/`unwrap`, Go `panic`, C `abort` path |

**Exceptional class.** `exception` and `panic` together form the **exceptional class**. Query predicates for "non-exception paths" exclude both. `ANY`/`NONE` quantifiers in CQL let you filter on this at the path level.

**Precedence rule.** When a call site is nested inside multiple constructs, the label with the highest precedence wins: `panic > exception > loop > conditional > always`. Exception: calls inside `finally`/`defer` are always labeled `always` regardless of enclosing context; their exception-relativity is computed at query time (see §4).

**v0.1 status:** all five labels are present and queryable as `r.condition` in CQL.

**Source:** `docs/03-code-graph-model.md` GM-3; `docs/13-glossary.md` "edge condition".

---

## 4. Path-relative transience

Transience is **not stored on edges** — it is computed per query during graph traversal.

An edge `C → D` is *exception-transient relative to path P* if and only if path P crosses at least one edge in the exceptional class (`exception` or `panic`) before reaching `C`. The same edge is not exception-transient relative to a different path `P'` that reaches `C` with no exceptional edge.

**Algorithm.** `cgx` maintains a per-walk boolean `seen_exceptional`. It initialises to `false` at the walk root. Crossing any `exception` or `panic` edge sets it to `true` irrevocably for that walk branch (backtracking resets to the fork-point value). A node `N` is exception-transient on a walk branch iff `seen_exceptional = true` when `N` is reached.

**Consequence for reading results.** The `always`-labeled edge `C → D` in a path output can be exception-transient if the path earlier crossed an `exception` edge. The edge label alone does not answer "is this call on an error-only path?" — the full path context does.

**v0.1 status:** fully computed. Use `ANY`/`NONE` quantifiers over a bounded path variable in CQL to filter on it.

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

**v0.1 ships tiers 0–1** (syntactic call graph). **Tiers 2–3 (SCIP enrichment, CHA/RTA) ship in v0.2.** Tier 4 is planned.

**What this means for v0.1 results:** `certain` and `probable` are technically present but produced only by the tier-0/1 rules. A unique name match that tier 1 resolves unambiguously may emit `probable`; everything else is `possible`. **Meaningful discrimination between `certain` and `probable` requires v0.2 (SCIP + CHA/RTA).**

**Real v0.1 distribution (from `cgx doctor` on the cgx codebase itself):**

```
confidence distribution (call edges):
  certain:     1369  (10.4%)
  probable:    2823  (21.4%)
  possible:    8979  (68.2%)
```

The majority of edges today are `possible`. Using `--confidence certain` as a CI gate is **vacuous until v0.2** — the vacuity guard (exit 4) exists to catch this. Use `--confidence possible` (the default) for v0.1 queries and treat results as structural over-approximation.

**Weakest-link confidence.** A path is only as trustworthy as its weakest edge. `cgx` computes `MIN([e IN relationships(path) | e.confidence])` under `certain > probable > possible` for path-level confidence.

**Source:** `docs/03-code-graph-model.md` GM-5; `docs/13-glossary.md` "confidence", "certain", "probable", "possible", "weakest-link confidence".

---

## 6. File:line evidence (provenance)

Every node and edge carries a **provenance** record. This is a hard design constraint — no fact is emitted without it.

| Field | Content |
|---|---|
| `file` | Repo-relative path |
| `line` | 1-based line of the call site or definition |
| `col` | Column (available when SCIP provides it; optional) |
| `rule` | Name of the producing rule (e.g. `scope-graph-ref`, `scip-occurrence`, `heuristic-sentinel`) |
| `tier` | Resolution tier (0–4) used |
| `index_id` | Content-addressed blob OID of the source file at index time |

Use `--format json` or `cgx explain <SYMBOL>` to inspect provenance. (There is no `--evidence` flag in
v0.1 — it exits 2; `--format json` carries the provenance fields instead.)

**Source:** `docs/03-code-graph-model.md` GM-6; `docs/13-glossary.md` "file:line evidence", "provenance".

---

## 7. What is and is not in the v0.1 graph

### Populated in v0.1

- All symbol node kinds with `kind`, `fqn`, `location`, `language`, `visibility`, `confidence` attributes.
- Full CALLS-family edges with `condition`, `confidence`, `provenance`, `site_id`.
- Structural edges: `contains`, `imports`, `overrides`, `implements`, `inherits`, `references`, `instantiates`, `throws`, `catches`, `reads-field`, `writes-field`.
- Syntactic edge-condition labels (`always`, `conditional`, `exception`, `loop`, `panic`) on all call edges.
- Cut markers: `reflective`, `dynamic`, `via-DI`, `via-FFI`, `unresolved`, `unexpanded-macro`.

### Reserved (schema present, not populated) in v0.1

| Feature | Status | Ships |
|---|---|---|
| `derives-from` edges (pedigree / data-flow) | schema-room | v0.3 |
| `spawns` edges (detached async) | schema-room | v0.3 |
| `DATA_FLOW` edge type in CQL | deferred — exit 2 | v0.3 |
| `MUST PASS THROUGH` / `AVOIDING` path algebra | deferred — exit 2 | v0.3 |
| Taint node/edge properties (`source_class`, `sink_class`, `taint_label`, etc.) | not in schema | v0.3 |
| `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` edge types | deferred — exit 2 | v0.3 |
| `NOT IN [...]` in CQL WHERE | parse error — exit 2 | v0.3 |
| Lock sets, dominance facts on call-site nodes | schema-room | v0.3 |
| SCIP enrichment (`certain`/`probable` discrimination) | not yet | v0.2 |
| CHA/RTA virtual dispatch narrowing | not yet | v0.2 |
| Dependency edges (package/version) | not yet | v0.2 |

**The cookbook (`docs/questions/`) tags several themes as "answerable-today" that rely on features above.** The version tags in this skill override the cookbook. Trust `reference/versions.md`.

---

## 8. Reading a result — checklist

1. **What confidence floor did you use?** In v0.1, most edges are `possible`. A `possible` path may be a false positive from over-approximated dispatch. Raise to `--confidence probable` to reduce noise, understanding you will miss some real paths.
2. **What edge conditions are on the path?** An `exception`-conditioned edge means the callee is only reached during error handling. A `panic`-conditioned edge is on an abort path. Use `ANY`/`NONE` quantifiers to filter.
3. **Is the path transient?** If the path includes an exceptional-class edge earlier, every subsequent `always` edge on the same path is exception-transient — the call only happens during error handling even though its own label says `always`.
4. **Cut markers.** A `reflective` or `via-DI` edge means `cgx` could not statically resolve the target. The edge is still emitted; decide whether to include or exclude it for your question.
5. **Empty result ≠ safe.** Exit 0 with no results means "no path found under these constraints" — not "the path is proven absent". Over-approximation (`possible` edges) means some real paths may be missing; under-approximation from filters means others are excluded.

---

## See also

- `reference/cli.md` — exact subcommands, flags, and defaults
- `reference/versions.md` — full version ladder and capability dates
- `reference/query-language.md` — CQL syntax, supported/unsupported clauses (Since: v0.1)
- `reference/output-and-exit.md` — output formats, exit codes, CI assertion mode
