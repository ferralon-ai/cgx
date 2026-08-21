//! # cgx-select
//!
//! The language-agnostic **node selector engine** (residual-selector design,
//! cycle `2026-08-17_1033_cgx-pack-cards`). It turns a raw argv selector string —
//! written in one language's surface syntax — into a predicate over
//! [`NodeRecord`](cgx_core::node::NodeRecord)s, usable exactly like the existing
//! `SymbolPattern::matches` in the `resolve_symbol` scan (regime (b): a drop-in
//! linear-scan predicate, no store change).
//!
//! ## What this is, and is not
//!
//! This is **package pattern matching** — not glob, not regex. Borrow neither
//! mental model. In particular `(a|b)*` means *starts with a or b*, never
//! "zero-or-more of `(a|b)`". Patterns are **anchored at both ends**: bare
//! `Service` means *equals* `Service` (so `com.foo` ≠ `com.foobar`); `*`/`**` are
//! the only opt-in to looseness. Unlike the coexisting
//! [`SymbolPattern`](cgx_core::pattern::SymbolPattern) glob it drops `?`.
//!
//! ## Pipeline
//!
//! 1. **Tokenizer registry** ([`tokenizer`]) — every registered per-language
//!    tokenizer optimistically attempts the string; each is the sole authority on
//!    its own separator + identifier grammar. No central separator→language table.
//! 2. **Grammar** ([`grammar`]) — each accepting tokenizer lexes into the uniform
//!    `::`-segmented [`Pattern`](grammar::Pattern): `*` (intra-segment glob), `**`
//!    (globstar over depth), `(a|b)` alternation, and `!` affix-negation.
//! 3. **Thompson NFA** ([`nfa`]) — each pattern compiles to a flat, segment-
//!    consuming automaton (O(|pattern|), no regex, no alternation×depth expansion).
//! 4. **Driver** ([`engine`]) — carries the union of clean parses (each keeping
//!    its family provenance); **zero clean parses is a labeled error, never a
//!    silent empty match**. [`Selector::evaluate`] returns the match plus the
//!    honesty signals: provenance, truncation, and agnostic cross-language.
//!
//! ## Grammar reference
//!
//! | form | meaning |
//! |------|---------|
//! | `Service` | segment equals `Service` |
//! | `*Service` | segment ends with `Service` (matches `Service` too) |
//! | `Svc*` | segment starts with `Svc` |
//! | `*Svc*` | segment contains `Svc` |
//! | `*` (whole segment) | any one segment |
//! | `**` (whole segment) | zero-or-more whole segments |
//! | `(foo\|bar)::Svc` | two segments: `foo` or `bar`, then `Svc` |
//! | `(foo\|bar)Svc` | one segment: `fooSvc` or `barSvc` |
//! | `!seg` | segment not-equals `seg` |
//! | `!seg*` | segment not-starts-with `seg` |
//! | `*!Test` | segment not-ends-with `Test` |
//! | `!seg*test` | not-starts-with `seg` AND ends-with `test` |

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod diagnostics;
pub mod engine;
pub mod family;
pub mod grammar;
pub mod nfa;
pub mod tokenizer;

#[cfg(test)]
mod tests;

pub use diagnostics::{Diagnostic, NodeMatch, SelectorError};
pub use engine::{compile, MatchOptions, Selector, DEFAULT_ACTIVE_STATE_CAP};
pub use family::Family;
pub use tokenizer::{Tokenizer, TokenizerRegistration};
