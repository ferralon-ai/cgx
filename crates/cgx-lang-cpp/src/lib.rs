//! # cgx-lang-cpp
//!
//! The **C++ language adapter**. Implements the frozen
//! [`LanguageFrontend`](cgx_frontend::LanguageFrontend) seam over `tree-sitter-cpp`
//! with a dedicated tree-walk [`extract`] that reuses the C kit (grammar-only
//! preprocessor, declarator descent, scope-attribution contract, callback honesty)
//! and layers the C++ surface on top: namespaces and `::`-FQN construction,
//! classes/structs/unions with methods and constructors/destructors, the
//! inheritance lattice (`Inherits`/`Overrides`), CHA virtual dispatch via
//! [`RefKind::CallVirtualReceiver`](cgx_frontend::RefKind) (the shared size-only
//! band applies by construction), `throw`/`catch` exception edges, templates, and
//! overload candidate sets.
//!
//! Tier-1 for syntactic/hierarchy facts; Tier-2 `possible` for type-dependent
//! facts (overloads, ADL, template-dependent targets) which Phase F's scip-clang
//! wiring later promotes. No fabricated RAII/destructor edges; no confidence a
//! mechanism cannot support.
//!
//! `.h` is intentionally NOT claimed here (it goes to `cgx-lang-c`); C++ headers
//! conventionally use `.hpp`/`.hh`/`.hxx`.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod effects;
mod extract;

pub use extract::CppFrontend;
