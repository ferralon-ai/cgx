# cgx Fixture Corpus

## Purpose

Small, hand-auditable source trees used to:
1. Drive the `xtask eval` harness (FP/FN rates per label)
2. Serve as golden-test inputs for WP-04 (Rust frontend) and WP-05 (TS frontend)
3. Validate WP-06 (resolver) cross-file resolution

## Layout

```
fixtures/
  rust-sample/          # Rust fixture repo
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
      cfg_feature.rs    # #[cfg(feature="...")] cfg-condition
      unsafe_ffi.rs     # extern "C" -> via-FFI cut marker
      proc_macro.rs     # #[derive(...)] proc-macro blind-spot
      spawn.rs          # tokio::spawn -> spawns edge
      Cargo.toml
  ts-sample/            # TypeScript fixture repo
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
      package.json
      tsconfig.json
  goldens/
    rust-sample/        # one YAML file per source file
      main.yaml
      direct.yaml
      ...
    ts-sample/
      index.yaml
      direct.yaml
      ...
```

## Annotation Format

Each golden is a YAML file with this schema:

```yaml
# goldens/<lang>-sample/<file>.yaml
file: "src/<file>.<ext>"
defs:
  - fqn: "crate_or_module::path::symbol_name"
    kind: function|method|type|lambda|constant|module
    line: <int>
    visibility: public|private
    entrypoint_kind: main|test|async-main|null
refs:
  - caller: "crate::path::fn"
    callee: "crate::path::fn"
    kind: calls|calls:virtual|calls:closure|calls:async|calls:callback|spawns
    edge_condition: always|conditional|loop|exception|panic
    confidence: certain|probable|possible
    line: <int>
    cut_marker: null|via-FFI|reflective|dynamic|unresolved   # omit if null
imports:
  - from: "other_module_or_crate"
    name: "imported_name"
    re_export: false
```

Absent fields default to null. The format is **deterministic**: all lists are sorted by (line, fqn/name) so diffs are stable.

## Coverage Matrix

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
| Entrypoints (main/test) | main.rs | index.ts |
| cfg-condition (build flags) | cfg_feature.rs | — |
| FFI cut marker | unsafe_ffi.rs | — |
| Proc-macro blind spot | proc_macro.rs | — |
| Dynamic import cut marker | — | dynamic_import.ts |
| Reflective cut marker | — | reflection.ts |

## Convergence Criterion (WP-02)

`xtask eval` runs green when the hand-written goldens are fully covered with
zero FP/FN on the phenomena listed above. FP/FN rates are printed per label.
