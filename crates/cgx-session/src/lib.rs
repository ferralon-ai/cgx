//! # cgx-session
//!
//! A **resident session** over one repository's index: warm-open the persisted
//! `.cgx/` graph (never indexing), or index and keep the graph in memory, then
//! answer every MCP tool and the session ops ([`ops`]) against it without
//! re-indexing per call. It is the single implementation behind both embedding
//! transports — the wasm module (`cgx-wasm`) and the native `cgx session` verb —
//! which differ only in who drives extraction: a host's instance pool feeding
//! [`Session::index_begin`]…[`Session::index_finish`], or the in-process native
//! composition ([`Session::index_native`]).
//!
//! Both write the store through `cgx-store`'s `ObjectStore` code and the pointer
//! through [`store_loc`], so `.cgx/{objects,fragments,refs}` and `HEAD.json` are
//! byte-identical whichever transport indexed.
//!
//! ## Versions
//!
//! - [`ABI_VERSION`]: the wasm exports' signatures, frame layouts, op JSON shapes
//!   and error envelope. The native handshake reports it too.
//! - [`PROTOCOL_VERSION`]: the native NDJSON protocol.
//! - [`schema_hash`]: SHA-1 of the tool schema document (`cgx mcp
//!   --print-schemas`), so a host's generated types can detect skew.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod error;
#[cfg(not(target_family = "wasm"))]
pub mod native;
pub mod ops;
pub mod session;
pub mod store_loc;

pub use error::{ErrorKind, Result, SessionError};
pub use ops::{EXPORT_EDGES, RESOLVE, SESSION_OPS};
pub use session::{
    extract, BeginOpts, BeginResponse, ExtractMeta, IndexReport, Mode, OpenRequest, OpenResponse,
    OpenState, PlanMiss, PlanSummary, ReportStats, Resident, Session, StoreOpener, Submitted,
};

/// The wasm ABI version (see the crate docs).
pub const ABI_VERSION: u32 = 1;
/// The native `cgx session` protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// The tool schema document exactly as `cgx mcp --print-schemas` prints it,
/// trailing newline included. Embedded rather than generated because the wasm
/// build has no `coupling` output type to generate it from; a test pins it to
/// the generated document.
pub const SCHEMA_DOCUMENT: &[u8] = include_bytes!("../../../schemas/mcp-tools.schema.json");

/// SHA-1 (lowercase hex) of [`SCHEMA_DOCUMENT`].
pub fn schema_hash() -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(SCHEMA_DOCUMENT)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
