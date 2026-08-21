//! # cgx-doctor
//!
//! Index-quality / health report for a `cgx` graph (WP-12, Phase-1 exit
//! requirement). Answers the user's core question: **how much can I trust this
//! index?**
//!
//! The report is a typed [`DoctorReport`] struct computed over an already-stored
//! [`cgx_store::LinkedGraph`]. Render it as human-readable text with
//! [`render_text`] or as machine-readable JSON with [`render_json`].
//!
//! ## Library entry point
//!
//! ```no_run
//! use cgx_doctor::{report, render_text};
//! use cgx_store::{SqliteStore, TreeOid};
//!
//! let store = SqliteStore::open("index.db").unwrap();
//! let tree = TreeOid::new("<tree-oid>");
//! let rep = report(&store, &tree).unwrap();
//! println!("{}", render_text(&rep));
//! ```
//!
//! ## CLI wiring (`cgx doctor`)
//!
//! ```rust,ignore
//! // In cgx-cli, the `doctor` subcommand wires as follows:
//! let outcome = cgx_index::index_path(repo, &registry, &mut store)?;
//! let rep = cgx_doctor::report(&store, &TreeOid::new(outcome.graph_key))?;
//! if json_flag {
//!     println!("{}", cgx_doctor::render_json(&rep)?);
//! } else {
//!     println!("{}", cgx_doctor::render_text(&rep));
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod render;
pub mod report;

pub use render::{render_json, render_text};
pub use report::{AnomalyKind, ConfidenceBreakdown, CutMarkerCount, DoctorReport, TrustLevel};

use cgx_store::{FactStore, TreeOid};

/// Compute a [`DoctorReport`] over the graph stored under `tree`.
///
/// Reads the linked graph from the store and analyses every edge and node.
/// Does not modify the store.
///
/// # Errors
///
/// Returns a [`cgx_store::StoreError`] if the graph cannot be read.
pub fn report(
    store: &impl FactStore,
    tree: &TreeOid,
) -> Result<DoctorReport, cgx_store::StoreError> {
    let graph = store.read_graph(tree)?;
    Ok(report::compute(&graph))
}
