//! # cgx-resolve
//!
//! The **shared, language-agnostic resolution engine** (architecture §2, WP-06).
//! It consumes the per-file [`FileFacts`](cgx_frontend::FileFacts) every language
//! frontend emits and links them into a single graph of resolved nodes and
//! call/structural edges carrying honest confidence labels.
//!
//! ## What it does
//!
//! Frontends emit facts about *one file with zero cross-file knowledge*. This
//! crate owns everything cross-file (architecture §2):
//!
//! - a **global definition index** (FQN → node, short-name → nodes), built from
//!   the union of all files' defs;
//! - an **import graph** built from import/export facts, with re-export chasing;
//! - **lexical resolution** within a file via the [`ScopeTree`](cgx_frontend::ScopeTree);
//! - **confidence assignment** along the GM-5 ladder, mapping resolution tiers to
//!   confidence bands; and
//! - **candidate sets** for virtual / duck-typed dispatch, rather than guessing a
//!   single target.
//!
//! The split is what makes blob-OID fragment caching sound (architecture §3,
//! IX-1): the per-file fragment is a pure function of the file's bytes; this
//! crate is the recomputable cross-file join (Layer 2).
//!
//! ## Tier → confidence mapping (GM-5, architecture §2)
//!
//! | Resolution rule | Tier | Confidence |
//! |---|---|---|
//! | same-file lexical resolution of a direct call | [`Tier::ScopeGraph`] | [`Confidence::Certain`] |
//! | import binding → unique exported target | [`Tier::ScopeGraph`] | [`Confidence::Probable`] |
//! | import binding → several candidates | [`Tier::ScopeGraph`] | [`Confidence::Possible`] |
//! | global name(+arity) match, unique | [`Tier::NameSyntactic`] | [`Confidence::Probable`] |
//! | global name(+arity) match, several | [`Tier::NameSyntactic`] | [`Confidence::Possible`] |
//! | virtual receiver, several same-name methods | [`Tier::ScopeGraph`] | [`Confidence::Possible`] |
//! | virtual receiver, single same-name method | [`Tier::ScopeGraph`] | [`Confidence::Probable`] |
//! | no candidate found | n/a | edge dropped → dangling, [`CutMarker::Unresolved`] cut hint recorded |
//!
//! Nothing is ever silently dropped (LS-6): an unresolvable call leaves no edge
//! but is recorded in [`ResolvedGraph::unresolved`] with a
//! [`CutMarker::Unresolved`] marker so the index-quality report can surface it.
//!
//! ## The WP-08-facing API
//!
//! ```ignore
//! use cgx_resolve::{link, FileInput, LinkOpts};
//!
//! let inputs: Vec<FileInput> = /* one per file, from the indexer */;
//! let graph = link(&inputs, &LinkOpts::default());
//! // hand graph.nodes / graph.edges / graph.candidates to cgx-store:
//! let (nodes, edges, candidates) = graph.into_linked();
//! ```
//!
//! [`link`] returns a [`ResolvedGraph`] built from [`cgx_core`] records only —
//! this crate never depends on `cgx-store`. The indexer (WP-08) depends on both
//! and wraps the lowered triple in `cgx_store::LinkedGraph`.
//!
//! ## Determinism
//!
//! Output is a pure function of the input fact set, independent of the order the
//! files are supplied (architecture §3; WP-06 convergence criterion). All
//! internal maps are `BTreeMap`/sorted vectors; node and edge ids are assigned by
//! the `cgx-core` canonical sort, never by insertion order.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod cha;
mod effects;
mod graph;
mod input;
mod link;
mod rta;
mod sig;
mod symtab;

pub use cha::{run_cha, ChaStats, CHA_SUPERNODE_CAP};
pub use effects::{run_effect_closure, EffectStats};
pub use graph::{ResolvedGraph, UnresolvedRef};
pub use input::{FileInput, LinkOpts};
pub use link::{canonicalize, link};
pub use rta::{run_rta, RtaStats};
pub use sig::{run_sig, SigStats};

// Re-export the core types a caller needs to read the result without importing
// cgx-core directly.
pub use cgx_core::{
    Candidate, Confidence, EdgeRecord, EdgeWithProvenance, NodeRecord, NodeWithProvenance, Tier,
};
