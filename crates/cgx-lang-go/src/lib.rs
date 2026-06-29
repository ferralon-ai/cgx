//! # cgx-lang-go
//!
//! The **Go language adapter**. It implements the frozen
//! [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over
//! `tree-sitter-go`, emitting per-file [`FileFacts`](cgx_frontend::FileFacts):
//! defs (func/method/type/const/var with visibility from capitalization),
//! call refs at the right [`RefKind`](cgx_frontend::RefKind), edge conditions,
//! imports/exports, entrypoint hints (`main`/`init`/`TestXxx`), cut hints
//! (cgo `import "C"` → via-FFI, `reflect`/`plugin` → reflective/dynamic),
//! own-effects, and intraprocedural SSA dataflow.
//!
//! Per the architecture's split of labor, a frontend emits facts about **one
//! file with zero cross-file knowledge**. Global interface satisfaction, FQN
//! resolution, confidence, and `dyn`/interface candidate sets are the shared
//! resolver's job.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod extract;
mod module;

pub use extract::GoFrontend;
pub use module::{module_path_for, module_path_for_pkg};
