//! Store location policy (IX-8) and the current-index pointer.
//!
//! Phase-1 policy: the index lives under a `.cgx/` directory at the repository
//! root — `.cgx/index.db` holds the SQLite store, and `.cgx/HEAD.json` records
//! the graph the last `cgx index` produced so a subsequent query knows which
//! Layer-2 graph to load without re-indexing. (The architecture's longer-term
//! `~/.cache/cgx/<repo-id>/` location is a later refinement; a repo-local `.cgx/`
//! keeps Phase-1 self-contained and trivially inspectable.)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::exit::ExitCode;
use crate::CliError;

/// The repo-local index directory name.
pub const CGX_DIR: &str = ".cgx";
/// The SQLite store filename within [`CGX_DIR`].
pub const DB_FILE: &str = "index.db";
/// The current-index pointer filename within [`CGX_DIR`].
pub const HEAD_FILE: &str = "HEAD.json";

/// The pointer written by `cgx index` and read by every query subcommand: which
/// stored Layer-2 graph is "current", and what tree/workdir key it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexPointer {
    /// The Layer-2 key the graph was stored under (tree OID or `workdir:<digest>`).
    pub graph_key: String,
    /// The stored graph's id; passed to `FactStore::read_graph`.
    pub graph_id: i64,
}

/// The `.cgx/` directory for a repository rooted at `repo_root`.
pub fn cgx_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(CGX_DIR)
}

/// The store database path for a repository rooted at `repo_root`.
pub fn db_path(repo_root: &Path) -> PathBuf {
    cgx_dir(repo_root).join(DB_FILE)
}

/// The current-index pointer path.
pub fn head_path(repo_root: &Path) -> PathBuf {
    cgx_dir(repo_root).join(HEAD_FILE)
}

/// Persist the current-index pointer (deterministic: stable field order, trailing
/// newline). Called by `cgx index` after a successful store.
pub fn write_pointer(repo_root: &Path, ptr: &IndexPointer) -> Result<(), CliError> {
    let dir = cgx_dir(repo_root);
    std::fs::create_dir_all(&dir).map_err(|e| CliError::graph(format!("creating {dir:?}: {e}")))?;
    let mut json = serde_json::to_string_pretty(ptr).expect("IndexPointer serializes");
    json.push('\n');
    let path = head_path(repo_root);
    std::fs::write(&path, json).map_err(|e| CliError::graph(format!("writing {path:?}: {e}")))?;
    Ok(())
}

/// Read the current-index pointer. A query against an un-indexed repo is a graph
/// error (exit 3): the user must run `cgx index` first.
pub fn read_pointer(repo_root: &Path) -> Result<IndexPointer, CliError> {
    let path = head_path(repo_root);
    let bytes = std::fs::read(&path).map_err(|_| {
        CliError::new(
            ExitCode::Graph,
            format!(
                "no index found at {:?}; run `cgx index` first",
                cgx_dir(repo_root)
            ),
        )
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|e| CliError::graph(format!("corrupt index pointer {path:?}: {e}")))
}
