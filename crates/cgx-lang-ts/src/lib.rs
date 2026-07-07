//! # cgx-lang-ts
//!
//! The **TypeScript/JavaScript language adapter** (WP-05). It implements the
//! frozen [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over
//! `tree-sitter-typescript`, emitting per-file
//! [`FileFacts`](cgx_frontend::FileFacts) for TypeScript and JavaScript sources.
//!
//! ## What this frontend extracts
//!
//! - **Definitions:** functions (including arrow functions stored in `const`),
//!   classes, interfaces, methods, fields, module-level `const`/`let`/`var` at
//!   the right [`SymbolKind`](cgx_core::node::SymbolKind).
//!
//! - **References / call edges:** direct function calls (`f()`), method calls on
//!   receivers (`x.m()` → [`RefKind::CallVirtualReceiver`] so the resolver can
//!   decide `calls` vs `calls:virtual`), closure invocations (`f()` where `f` is
//!   a local lambda binding), callback invocations, `await expr` →
//!   [`RefKind::CallAsync`], `.then`/`.catch` → async/exception edges.
//!
//! - **Edge conditions** (GM-3.1, ADR-03 precedence):
//!   - `if`/`switch`/`?:` → [`EdgeCondition::Conditional`]
//!   - `for`/`while`/`.map`/`.forEach` loop bodies → [`EdgeCondition::Loop`]
//!   - `catch` blocks → [`EdgeCondition::Exception`]
//!   - `finally` blocks → [`EdgeCondition::Always`] (GM-3.1 carve-out)
//!   - `try` bodies → [`EdgeCondition::Always`]
//!
//! - **Import/export facts:** `import { a } from "m"`, `export { x } from "m"`,
//!   `export * from "m"`, aliased imports, re-exports.
//!
//! - **Cut hints:**
//!   - `eval(expr)` and `new Function(...)` → [`CutMarker::Reflective`]
//!   - `import(expr)` and `require(expr)` with non-literal argument →
//!     [`CutMarker::Dynamic`]
//!
//! - **Entrypoint hints:** Jest `test()` / `it()` / `describe()` top-level calls,
//!   named `main()` function.
//!
//! - **Own-effects (GM-12 Phase 1):** a name-based syntactic heuristic
//!   ([`effects`](crate::effects)) attributes `io.net`/`io.file`/`io.proc`,
//!   `nondeterministic`, `dynamic-code`, `blocking`, and `spawns` effects to the
//!   enclosing named definition (`possible`-grade — no import/type resolution).
//!
//! - **Intraprocedural SSA dataflow (v0.3 DATA_FLOW):** each production site
//!   (declaration / assignment / `+=` / projection / call / return) lowers into a
//!   [`DataFlowFact`](cgx_frontend::DataFlowFact) with the `DerivesFrom`
//!   orientation `derived → source`, powering `flows-to` / `flows-from`. Only
//!   block-bodied functions/methods/named arrows are covered; closure-capture and
//!   expression-body-arrow dataflow are deferred (matching Go/Python).
//!
//! - **Signatures (ADR-04):** recorded where the surface declares them (parameter
//!   names + optional type annotations, return type); never inferred.
//!
//! ## What it deliberately does NOT do
//!
//! Per the architecture split-of-labor (§2): no cross-file resolution, no
//! confidence assignment, no candidate-set construction. Those are the resolver's
//! job (WP-06). This frontend is also tree-sitter-only (oxc enrichment is
//! deferred past Phase 1, per architecture §1).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod effects;
mod extract;
mod module;
mod query;

pub use extract::TypeScriptFrontend;
pub use module::module_path_for;
