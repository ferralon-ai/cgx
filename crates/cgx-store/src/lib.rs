//! # cgx-store
//!
//! The Phase-1 persistence layer for `cgx`: a SQLite store (rusqlite, **bundled**,
//! WAL) for the two-layer graph model of architecture §3.
//!
//! - **Layer 1 — per-blob fragments.** A file's facts are stored as canonical
//!   postcard bytes keyed by its git blob OID ([`BlobOid`]). Re-putting an
//!   unchanged blob is a no-op ([`FactStore::put_fragments`]): content-addressed
//!   keying is what makes re-indexing an untouched file free (IX-1).
//! - **Layer 2 — per-tree linked graphs.** A [`LinkedGraph`] (nodes + edges +
//!   candidate sets) is materialized per tree OID into the `nodes`/`edges`/
//!   `candidates` tables and read back byte-identically.
//!
//! ## Stability surfaces
//!
//! Physical tables are **not** a public API. The only SQL-visible contract
//! (ADR-05) is the versioned view set — [`schema::VIEW_SET`]:
//! `v_symbols`, `v_call_edges`, `v_call_sites`, `v_provenance`, and `cgx_meta`
//! (which exposes `view_schema_version`). `--sql` runs against these views alone.
//! View-breaking changes bump [`schema::VIEW_SCHEMA_VERSION`]; an incompatible
//! physical layout is rejected by [`schema::SCHEMA_VERSION`] at open time.
//!
//! ## Determinism
//!
//! The store preserves the order it is given and round-trips records from
//! canonical postcard `data` columns, so two `put_graph` calls of the same
//! [`LinkedGraph`] produce identical rows and bytes (architecture §3, §4).
//!
//! ## Concurrency
//!
//! WAL gives readers MVCC snapshots; writers take an advisory [`lock::WriteLock`]
//! (IX-7) and use the check → lock → re-check pattern.

//!
//! ## Targets
//!
//! On `wasm32` the SQLite backend (rusqlite) and its `fd-lock` advisory locking
//! are not built. [`ObjectStore`] is: its [`WriteLock`] is a no-op there (the host
//! holds the exclusive lock on `.cgx/objects.lock`), and its three dataflow caches,
//! which natively live in a scoped SQLite DB, are not persisted (every
//! `--dataflow` link is a cold build; the graph is unaffected).

pub mod error;
pub mod fact_store;
#[cfg(not(target_family = "wasm"))]
pub mod lock;
#[cfg(target_family = "wasm")]
#[path = "lock_wasm.rs"]
pub mod lock;
pub mod manifest;
pub mod object_store;
pub mod oid;
pub mod schema;
#[cfg(not(target_family = "wasm"))]
pub mod store;
pub mod types;

#[cfg(not(target_family = "wasm"))]
mod token;

pub use error::{Result, StoreError};
pub use fact_store::{FactStore, FragmentInput};
pub use lock::WriteLock;
pub use object_store::ObjectStore;
pub use oid::ObjectOid;
pub use schema::{SCHEMA_VERSION, VIEW_SCHEMA_VERSION, VIEW_SET};
#[cfg(not(target_family = "wasm"))]
pub use store::SqliteStore;
pub use types::{BlobOid, BlobSet, CachedFragment, LinkedGraph, PruneStats, TreeOid};
