//! # cgx-lang-c
//!
//! The **C language adapter** — scaffold milestone. Implements the frozen
//! [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over
//! `tree-sitter-c` by delegating to the Tier-0
//! [`FallbackFrontend`](cgx_frontend::FallbackFrontend): grammar-blind def/call
//! extraction, no cross-file edges, every ref `possible` confidence. This
//! proves the crate compiles and registers end to end; dedicated tree-walk
//! extraction (`extract.rs` doing its own node-kind dispatch) is a later phase.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod extract;

pub use extract::CFrontend;
