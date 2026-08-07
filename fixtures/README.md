# cgx Fixture Corpus

## Purpose

Small, hand-auditable source trees, checked in so tests read real files instead
of string literals. Three groups, with three different consumers:

1. **`rust-sample/`, `ts-sample/` and `goldens/`** — the annotated corpus. The
   goldens are hand-written expected facts; `xtask eval` validates them and
   `crates/cgx-lang-rust/tests/` and `crates/cgx-lang-ts/tests/` assert extracted
   facts against them.
2. **`go/`, `java/`, `python/`, `ts/`** — unannotated per-adapter corpora, read
   directly by that adapter's tests (`crates/cgx-lang-*/tests/`) and, for Go,
   Python and TypeScript, by the CLI end-to-end tests
   (`crates/cgx-cli/tests/*_e2e.rs`). `go/` is additionally the class-hierarchy
   fixture for `crates/cgx-resolve/tests/cha.rs`. No goldens; the assertions live
   in the tests.
3. **`scip-relabel/`** — a tiny crate backing the SCIP re-label integration and
   determinism tests (`crates/cgx-index/tests/scip_relabel_integration.rs`). See
   its own [`REGEN.md`](scip-relabel/REGEN.md).

`python-app/` has no consumer anywhere in the workspace.

## Layout

```
fixtures/
  rust-sample/          # Rust fixture crate — the annotated corpus
    Cargo.toml
    src/
      main.rs           # entrypoints: fn main, #[test] fns, #[tokio::main]
      direct.rs         # direct calls (calls, certain)
      virtual_dispatch.rs  # dyn Trait (calls:virtual, probable)
      closures.rs       # closure/iterator (calls:closure, calls:callback)
      async_calls.rs    # async/await (calls:async)
      conditions.rs     # conditional / loop edges
      errors.rs         # ? operator, Err arm -> exception label
      panics.rs         # panic!, unwrap, expect -> panic label
      imports.rs        # mod + use imports, re-exports
      inheritance.rs    # impl Trait for T, overrides
      dead_code.rs      # symbols unreachable from main
      recursion.rs      # direct and mutual recursion
      dataflow.rs       # derives-from edges over local values
      cfg_feature.rs    # #[cfg(feature="...")] cfg-condition
      unsafe_ffi.rs     # extern "C" -> via-FFI cut marker
      proc_macro_fixture.rs  # #[derive(...)] proc-macro blind-spot
      spawn.rs          # tokio::spawn -> spawns edge
  ts-sample/            # TypeScript fixture repo — the annotated corpus
    package.json
    tsconfig.json
    src/
      index.ts          # entrypoints: exports, jest test()
      direct.ts         # direct function calls (calls, probable/certain w/ SCIP)
      virtual_dispatch.ts  # duck-typed method calls (calls:virtual, possible)
      closures.ts       # arrow functions, .map/.filter callbacks
      async_calls.ts    # async/await (calls:async)
      conditions.ts     # if/switch/ternary conditional edges
      errors.ts         # try/catch/finally -> exception, always labels
      throws.ts         # throw new Error() paths
      imports.ts        # ESM import/re-export chains
      inheritance.ts    # extends / implements override chains
      dead_code.ts      # unreachable exports
      dynamic_import.ts # dynamic import() -> dynamic cut marker
      reflection.ts     # eval() -> reflective cut marker
      spawn.ts          # Promise + async spawn patterns
  goldens/              # hand-written expected facts, one YAML per annotated file
    rust-sample/        # closures, direct, errors, imports, panics, recursion,
                        #   spawn, virtual_dispatch
    ts-sample/          # dynamic_import, errors, imports, virtual_dispatch
  go/                   # interfaces, imports, concurrency, dataflow + go.mod
  java/                 # Shapes, Lattice, Effects, Dataflow
  python/               # shapes, lattice, dataflow
  ts/src/               # dataflow, effects
  python-app/           # app, storage
  scip-relabel/         # tiny crate for the SCIP re-label tests (see REGEN.md)
```

**Not every fixture file has a golden.** The corpus is deliberately wider than
the annotated subset: the goldens cover the phenomena listed in the matrix below,
and the remaining fixture files are exercised by crate tests that assert on the
extracted graph directly.

## Annotation Format

Each golden is a YAML file. The schema is the `Golden`/`DefRecord`/`RefRecord`/
`ImportRecord` structs in [`xtask/src/eval.rs`](../xtask/src/eval.rs) — that
loader is the authority, so a field added there belongs here too:

```yaml
# goldens/<lang>-sample/<file>.yaml
file: "src/<file>.<ext>"
defs:
  - fqn: "crate_or_module::path::symbol_name"
    kind: function|method|type|field|variable|module|constant|macro|lambda|entrypoint
    line: <int>
    visibility: public|private
    is_abstract: false            # optional; a trait/interface, or a member with no body
    entrypoint_kind: null         # optional; a kebab-cased cgx-core EntrypointKind
                                  #   (main, test, async-main, http-handler, declared)
refs:
  - caller: "crate::path::fn"
    callee: "crate::path::fn"
    kind: calls|calls:virtual|calls:closure|calls:callback|calls:async|calls:indirect|spawns
    edge_condition: always|conditional|loop|exception|panic
    confidence: certain|probable|possible
    line: <int>
    cut_marker: null|dynamic|…    # optional; a kebab-cased cgx-core CutMarker
    implicit: null|iterator|…     # optional; the implicit-call shape, when any
imports:
  - from: "other_module_or_crate"
    name: "imported_name"
    re_export: false
    alias: null                   # optional
```

Absent optional fields default to null/false. `kind`, `edge_condition` and
`confidence` are the only fields `xtask eval` validates against a closed set;
`cut_marker` and `implicit` are free-form strings there and are checked by the
crate tests that consume them.

Lists are **not** sorted by the loader and the goldens are not consistently
sorted: `defs` are ordered by `fqn`, `refs` roughly by line. Nothing enforces
either, so re-order a golden only when the diff is the point.

## Coverage Matrix

The phenomena the annotated `rust-sample`/`ts-sample` corpus is built to cover.
A `—` means that language's annotated corpus does not cover it — either the
phenomenon has no analogue there, or it is exercised elsewhere (local dataflow,
for instance, is covered for TypeScript by `ts/src/dataflow.ts`).

| Phenomenon | Rust | TypeScript |
|---|---|---|
| Direct call (always) | direct.rs | direct.ts |
| Virtual/trait dispatch | virtual_dispatch.rs | virtual_dispatch.ts |
| Closure invocation | closures.rs | closures.ts |
| Callback via fn-value | closures.rs | closures.ts |
| Async call (calls:async) | async_calls.rs | async_calls.ts |
| Spawns edge | spawn.rs | spawn.ts |
| Conditional edge | conditions.rs | conditions.ts |
| Loop edge | conditions.rs | conditions.ts |
| Exception edge (? / catch) | errors.rs | errors.ts |
| Panic edge (panic!/unwrap) | panics.rs | throws.ts |
| Import / re-export chain | imports.rs | imports.ts |
| Inheritance / override | inheritance.rs | inheritance.ts |
| Dead code from instance | dead_code.rs | dead_code.ts |
| Recursion (direct + mutual) | recursion.rs | — |
| Local dataflow (derives-from) | dataflow.rs | — |
| Entrypoints (main/test) | main.rs | index.ts |
| cfg-condition (build flags) | cfg_feature.rs | — |
| FFI cut marker | unsafe_ffi.rs | — |
| Proc-macro blind spot | proc_macro_fixture.rs | — |
| Dynamic import cut marker | — | dynamic_import.ts |
| Reflective cut marker | — | reflection.ts |

## Running the harness

```sh
cargo run -p xtask -- eval                 # both languages
cargo run -p xtask -- eval --lang rust     # rust | ts | all (the default)
```

There is no `cargo xtask` alias in this repository (no `.cargo/config.toml`), so
the `cargo xtask eval …` form in `xtask/src/eval.rs`'s own header comment does not
run; `cargo run -p xtask -- …` is the working invocation.

With no `--actual-facts`, `xtask eval` **validates golden structure only** — it
parses every YAML, checks each `kind`/`edge_condition`/`confidence` against its
closed set, and prints a per-golden def/ref/import count. It computes no FP/FN
rates in this mode, so a green run says the goldens are well-formed, not that the
extractors agree with them. That comparison runs when a directory of extracted
facts is supplied:

```sh
cargo run -p xtask -- eval --actual-facts <dir> --strict
```

`<dir>` must hold `rust-sample/` and `ts-sample/` subdirectories of YAML in the
same schema; a `<dir>` without them silently falls back to the structure-only
check. Nothing in the repository emits that directory today, so the FP/FN path is
unexercised in CI — the extractor-vs-golden assertions that do run live in
`crates/cgx-lang-rust/tests/` and `crates/cgx-lang-ts/tests/`. When facts are
supplied, missing refs count as FN and extra refs as FP, precision/recall print
per edge-condition and per ref-kind, and `--strict` makes an FP fail the run.

Note that `xtask eval`'s own report iterates a hash map, so the per-golden lines
come out in a different order on each run.
