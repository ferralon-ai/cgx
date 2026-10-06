//! Store location policy (IX-8) and the current-index pointer, with the CLI's
//! exit codes attached.
//!
//! The layout itself — `.cgx/`, its self-ignoring `.gitignore`, and the
//! `.cgx/HEAD.json` pointer — is [`cgx_session::store_loc`], shared with the
//! resident session so every writer produces the same bytes. This module adds the
//! CLI-only SQLite shadow path and maps failures onto the IF-4 exit-code contract.

use std::path::{Path, PathBuf};

pub use cgx_session::store_loc::{cgx_dir, IndexPointer};

use crate::exit::ExitCode;
use crate::CliError;

/// The SQLite store filename within `.cgx/`.
pub const DB_FILE: &str = "index.db";

/// The store database path for a repository rooted at `repo_root`.
pub fn db_path(repo_root: &Path) -> PathBuf {
    cgx_dir(repo_root).join(DB_FILE)
}

/// Ensure `.cgx/` exists with its self-ignoring `.gitignore`
/// ([`cgx_session::store_loc::ensure_cgx_dir`]).
pub fn ensure_cgx_dir(repo_root: &Path) -> Result<(), CliError> {
    cgx_session::store_loc::ensure_cgx_dir(repo_root).map_err(CliError::graph)
}

/// Persist the current-index pointer ([`cgx_session::store_loc::write_pointer`]).
/// Called by `cgx index` after a successful store.
pub fn write_pointer(repo_root: &Path, ptr: &IndexPointer) -> Result<(), CliError> {
    cgx_session::store_loc::write_pointer(repo_root, ptr).map_err(CliError::graph)
}

/// Read the current-index pointer. A query against an un-indexed repo is a graph
/// error (exit 3): the user must run `cgx index` first.
pub fn read_pointer(repo_root: &Path) -> Result<IndexPointer, CliError> {
    cgx_session::store_loc::read_pointer(repo_root)
        .map_err(CliError::graph)?
        .ok_or_else(|| {
            CliError::new(
                ExitCode::Graph,
                format!(
                    "no index found at {:?}; run `cgx index` first",
                    cgx_dir(repo_root)
                ),
            )
        })
}
