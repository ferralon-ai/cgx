//! # cgx-lang-java
//!
//! The **Java language adapter**. It implements the frozen
//! [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over
//! `tree-sitter-java`, emitting per-file [`FileFacts`](cgx_frontend::FileFacts):
//! defs (class/interface/enum/record/annotation-type/method/constructor/field
//! [`SymbolDef`](cgx_frontend::SymbolDef)s with visibility from access modifiers
//! and a lexical scope tree that nests them), call refs at the right
//! [`RefKind`](cgx_frontend::RefKind) with edge conditions, imports/exports,
//! entrypoint hints (`main`, JUnit tests), cut hints (`native` methods →
//! via-FFI, `reflect`/dynamic-load markers), inheritance and override
//! [`ImplRelation`](cgx_frontend::ImplRelation)s (`extends`/`implements`,
//! `@Override` grounding), own-effects including concurrency
//! (`synchronized` → `Blocking`), and intraprocedural SSA dataflow.
//!
//! Per the architecture's split of labor, a frontend emits facts about **one
//! file with zero cross-file knowledge**. Global interface satisfaction, FQN
//! resolution, confidence, and candidate sets are the shared resolver's job.
//!
//! Unlike the Go adapter, the FQN prefix is derived from the source
//! `package_declaration` node ([`module_path_from_package`]), not the file path.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod effects;
mod extract;
mod module;

pub use extract::JavaFrontend;
pub use module::module_path_from_package;
