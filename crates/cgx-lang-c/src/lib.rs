//! # cgx-lang-c
//!
//! The **C language adapter**. Implements the frozen
//! [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over `tree-sitter-c`
//! with a dedicated tree-walk [`extract`] that emits defs (functions, structs,
//! unions, enums, typedefs, file-scope globals, macro names), call refs attributed
//! to their **enclosing function's scope** (so intra-file calls resolve `certain`
//! and cross-file calls band to `possible` through the shared resolver), edge
//! conditions, the `main` entrypoint, `#include` import facts, and grammar-only
//! preprocessor handling (ADR B1: expand nothing, fabricate nothing).
//!
//! C has a flat global namespace, so a symbol's local FQN is its bare source name;
//! `static` yields file-local [`Visibility::Internal`](cgx_core::node::Visibility).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod effects;
mod extract;

pub use extract::CFrontend;
