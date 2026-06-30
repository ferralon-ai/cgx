//! # cgx-lang-python
//!
//! The **Python language adapter**. It implements the frozen
//! [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over
//! `tree-sitter-python`, emitting per-file [`FileFacts`](cgx_frontend::FileFacts).
//!
//! This is the Cycle-1 slice: **parse + symbol-extraction (defs)** only. It emits
//! defs for module-level functions, methods (functions nested in a class body),
//! classes, and module-level bindings (constants/variables), unwrapping
//! `decorated_definition` to its inner def. Call refs, imports, inheritance,
//! effects, and intraprocedural dataflow are later cycles.
//!
//! Per the architecture's split of labor, a frontend emits facts about **one
//! file with zero cross-file knowledge**. FQN resolution, confidence, and
//! cross-file relations are the shared resolver's job.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod extract;
mod module;

pub use extract::PythonFrontend;
pub use module::module_path_for;
