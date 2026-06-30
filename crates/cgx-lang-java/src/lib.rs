//! # cgx-lang-java
//!
//! The **Java language adapter**. It implements the frozen
//! [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over
//! `tree-sitter-java`, emitting per-file [`FileFacts`](cgx_frontend::FileFacts).
//!
//! Per the architecture's split of labor, a frontend emits facts about **one
//! file with zero cross-file knowledge**. Global interface satisfaction, FQN
//! resolution, confidence, and candidate sets are the shared resolver's job.
//!
//! ## Build status (Cycle 1 of 6)
//! This cycle emits **defs only** — class/interface/enum/record/method/
//! constructor/field [`SymbolDef`](cgx_frontend::SymbolDef)s with the lexical
//! scope tree that nests them. Refs, imports/exports, entrypoint/cut hints,
//! effects, intraprocedural dataflow, and inheritance/override
//! [`ImplRelation`](cgx_frontend::ImplRelation)s are stubbed (empty channels) and
//! land in later cycles; see the `// Cycle N` markers in `extract.rs`.
//!
//! Unlike the Go adapter, the FQN prefix is derived from the source
//! `package_declaration` node ([`module_path_from_package`]), not the file path.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod extract;
mod module;

pub use extract::JavaFrontend;
pub use module::module_path_from_package;
