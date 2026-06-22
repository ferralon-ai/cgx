//! # cgx-core
//!
//! The foundational graph data model for `cgx`. Every other crate depends on
//! this one; it has no I/O and no dependencies beyond `serde`, `postcard`, and
//! `smallvec`.
//!
//! ## What lives here
//!
//! - Node taxonomy ([`node`]): [`SymbolKind`], [`NodeFlavor`] (symbol +
//!   reserved call-site, ADR-01), [`Visibility`], [`EntrypointKind`],
//!   [`NodeRecord`], [`CallSite`].
//! - Edge taxonomy ([`edge`]): [`EdgeKind`] (call/structural/`spawns`),
//!   [`ImplicitKind`], [`EstablishedBy`], [`EdgeRecord`], [`Candidate`].
//! - Edge conditions ([`condition`]): the five-value [`EdgeCondition`] set with
//!   the ADR-03 precedence rule.
//! - Effects ([`effect`]): the GM-12 syntactic effect labels ([`Effect`]) and the
//!   `u16`-bitset [`EffectSet`] carried as a node's own/transitive effects.
//! - Confidence ([`confidence`]): the [`Confidence`] ladder and resolution
//!   [`Tier`]s.
//! - Cut markers ([`cut`]): [`CutMarker`] / [`CutMarkers`] (GM-5.3, ADR-07).
//! - Identity ([`id`]): deterministic [`NodeId`], [`EdgeId`], [`SiteId`].
//! - Signatures ([`signature`]): the structured [`Signature`] record (ADR-04).
//! - Provenance ([`provenance`]): [`Provenance`] and [`Span`].
//! - Patterns ([`pattern`]): [`SymbolPattern`] resolution.
//! - Sort rules ([`sort`]): the canonical orders that keep output deterministic.
//! - Codec ([`codec`]): the canonical `postcard` encode/decode seam.
//!
//! ## Determinism
//!
//! No type in this crate carries a `HashMap`/`HashSet`; sets are sorted
//! `SmallVec`s ([`CutMarkers`]). No system time or randomness enters any value.
//! Identical inputs always produce byte-identical [`codec::encode`] output.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod codec;
pub mod condition;
pub mod confidence;
pub mod cut;
pub mod edge;
pub mod effect;
pub mod id;
pub mod node;
pub mod pattern;
pub mod provenance;
pub mod signature;
pub mod sort;

// --- Curated flat re-exports for downstream ergonomics ---

pub use condition::EdgeCondition;
pub use confidence::{Confidence, Tier};
pub use cut::{CutMarker, CutMarkers};
pub use edge::{Candidate, EdgeKind, EdgeRecord, EdgeWithProvenance, EstablishedBy, ImplicitKind};
pub use effect::{Effect, EffectSet};
pub use id::{EdgeId, NodeId, NodeSortKey, SiteId};
pub use node::{
    CallSite, EntrypointKind, NodeFlavor, NodeRecord, NodeWithProvenance, SymbolKind, Visibility,
};
pub use pattern::{PatternKind, SymbolPattern};
pub use provenance::{Provenance, Span};
pub use signature::{Param, Signature};
pub use sort::EdgeIdentity;
