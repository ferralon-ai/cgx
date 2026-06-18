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

pub mod error;
pub mod lock;
pub mod schema;
pub mod store;
pub mod types;

mod token;

pub use error::{Result, StoreError};
pub use lock::WriteLock;
pub use schema::{SCHEMA_VERSION, VIEW_SCHEMA_VERSION, VIEW_SET};
pub use store::{FactStore, FragmentInput, SqliteStore};
pub use types::{BlobOid, BlobSet, CachedFragment, GraphId, LinkedGraph, PruneStats, TreeOid};
