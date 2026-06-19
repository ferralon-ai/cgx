//! # cgx-frontend
//!
//! The **language-frontend seam** of `cgx` (architecture §5). This crate owns
//! the one trait a language adapter implements, the per-file fact types it
//! emits, the registry that dispatches a file to the right adapter, and the
//! Tier-0 generic fallback that handles any tree-sitter grammar with no
//! language-specific rules.
//!
//! It is the pluggability boundary the rest of Phase 1 builds against:
//!
//! - **WP-04** (`cgx-lang-rust`) and **WP-05** (`cgx-lang-ts`) implement
//!   [`LanguageFrontend`] and emit [`FileFacts`].
//! - **WP-06** (`cgx-resolve`) consumes [`FileFacts`] fragments and links them
//!   into a graph. It can develop against the [`FallbackFrontend`] plus
//!   hand-built [`FileFacts`] before the real adapters land.
//! - **WP-08** (`cgx-index`) owns a [`FrontendRegistry`], routes every file
//!   through [`FrontendRegistry::extract`], and caches the canonical fragment
//!   per blob OID.
//!
//! ## The contract in one screen
//!
//! ```text
//! trait LanguageFrontend {
//!     fn lang(&self) -> Lang;
//!     fn handles(&self, path: &RelPath) -> bool;
//!     fn fragment_version(&self) -> u32;
//!     fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError>;
//! }
//! ```
//!
//! A frontend emits facts about **one file with zero cross-file knowledge**.
//! Resolution (scopes across files, the import graph, confidence assignment) is
//! the shared resolver's job — never a frontend's. That split is what makes
//! blob-OID fragment caching sound (architecture §3, IX-1).
//!
//! ## Built on `cgx-core`
//!
//! The fact types reuse `cgx-core`'s model directly — [`SymbolKind`],
//! [`Visibility`], [`EdgeCondition`], [`EntrypointKind`], [`CutMarker`],
//! [`Signature`], [`Span`]. Nothing is duplicated. The flat re-exports below
//! pull the most-used core types into one import for downstream ergonomics.
//!
//! ## Determinism
//!
//! No type here carries a `HashMap`/`HashSet`. [`FileFacts::canonicalize`]
//! sorts every fact vector, and the registry calls it before returning, so the
//! postcard fragment is byte-identical across runs (architecture §3; WP-12
//! asserts the property).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod facts;
pub mod fallback;
pub mod frontend;
pub mod registry;

// --- Primary surface ---

pub use facts::{
    CutHint, EntrypointHint, ExportFact, FileFacts, ImplRelation, ImportFact, ImportedName, Name,
    RawRef, RefKind, RelationKind, Scope, ScopeId, ScopeTree, SymbolDef,
};
pub use fallback::FallbackFrontend;
pub use frontend::{FileCtx, FrontendError, Lang, LanguageFrontend, RelPath};
pub use registry::FrontendRegistry;

// --- Re-exported cgx-core model types frontends emit (reuse, never duplicate) ---

pub use cgx_core::condition::EdgeCondition;
pub use cgx_core::cut::CutMarker;
pub use cgx_core::edge::ImplicitKind;
pub use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
pub use cgx_core::provenance::Span;
pub use cgx_core::signature::Signature;
