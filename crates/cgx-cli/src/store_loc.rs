//! Store location policy (IX-8) and the current-index pointer, with the CLI's
//! exit codes attached.
//!
//! The layout itself — `.cgx/`, its self-ignoring `.gitignore`, and the
//! `.cgx/HEAD.json` pointer — is [`cgx_session::store_loc`], shared with the
//! resident session so every writer produces the same bytes. This module adds the
//! CLI's cleanup of files an older cgx left behind and maps failures onto the IF-4
//! exit-code contract.

use std::path::Path;

pub use cgx_session::store_loc::{cgx_dir, IndexPointer};

use crate::exit::ExitCode;
use crate::CliError;

/// Files an older cgx wrote beside the object store: a full SQLite copy of every
/// graph, kept only for parity checking. Nothing reads them any more.
const RETIRED_FILES: [&str; 4] = ["index.db", "index.db-wal", "index.db-shm", "index.lock"];

/// Ensure `.cgx/` exists with its self-ignoring `.gitignore`
/// ([`cgx_session::store_loc::ensure_cgx_dir`]).
pub fn ensure_cgx_dir(repo_root: &Path) -> Result<(), CliError> {
    cgx_session::store_loc::ensure_cgx_dir(repo_root).map_err(CliError::graph)
}

/// Delete the [`RETIRED_FILES`] an older cgx left in `.cgx/`, if present.
pub fn remove_retired_files(repo_root: &Path) -> Result<(), CliError> {
    for name in RETIRED_FILES {
        let path = cgx_dir(repo_root).join(name);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(CliError::graph(format!("removing {path:?}: {e}")))
            }
            _ => {}
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_retired_files_deletes_the_old_sqlite_copy_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        ensure_cgx_dir(root).unwrap();
        let dir = cgx_dir(root);
        for name in ["index.db", "index.db-wal", "cache.db"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        remove_retired_files(root).unwrap();
        assert!(!dir.join("index.db").exists());
        assert!(!dir.join("index.db-wal").exists());
        assert!(dir.join("cache.db").exists(), "dataflow cache is kept");
        // Idempotent when nothing is left to remove.
        remove_retired_files(root).unwrap();
    }
}
