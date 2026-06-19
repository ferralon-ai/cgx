//! # cgx-lang-rust
//!
//! The **Rust language adapter** (WP-04). It implements the frozen
//! [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over
//! `tree-sitter-rust`, emitting richer per-file [`FileFacts`](cgx_frontend::FileFacts)
//! than the Tier-0 fallback: real defs (with visibility, signatures, `is_abstract`
//! trait declarations), method/free/trait-object call refs at the right
//! [`RefKind`](cgx_frontend::RefKind), edge conditions lowered from the enclosing
//! syntax (if/match → conditional, loops → loop, `?`/`Err`-arm → exception,
//! `panic!`/`unwrap`/`expect`/`assert!`/`unreachable!` → panic), `use` imports and
//! `pub use` re-exports, entrypoint hints (`main`/`#[test]`/`#[tokio::main]`), and
//! cut hints (`extern "C"` → via-FFI, `#[derive]`/attribute proc-macros →
//! unexpanded-macro).
//!
//! ## What this frontend does and does not do
//!
//! Per the architecture's split of labor (§2), a frontend emits facts about **one
//! file with zero cross-file knowledge**. Everything cross-file — resolving a
//! `name_path` to a target FQN, assigning [`Confidence`](cgx_core::confidence::Confidence),
//! following `use` chains, building candidate sets for `dyn Trait` dispatch — is
//! the shared resolver's job (WP-06). This crate emits **resolution honesty**: a
//! plain `f()` is a [`RefKind::Call`](cgx_frontend::RefKind::Call), a `recv.m()`
//! whose receiver type the frontend cannot know is a
//! [`RefKind::CallVirtualReceiver`](cgx_frontend::RefKind::CallVirtualReceiver) —
//! and the resolver decides `calls` vs `calls:virtual` and the confidence tier.
//!
//! ## Module FQNs from the file path
//!
//! The frontend derives a module prefix from the repo-relative path
//! (`src/errors.rs` → `<crate>::errors`, `src/a/b.rs` → `<crate>::a::b`,
//! `src/main.rs`/`src/lib.rs`/`mod.rs` → the crate/parent root). The crate name
//! is taken from the first path segment after `src/` is stripped, defaulting to
//! the fixtures' `rust_sample` so the WP-02 goldens line up. See
//! [`module_path_for`].

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod effects;
mod extract;
mod module;

pub use extract::RustFrontend;
pub use module::module_path_for;
