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
/// The self-ignoring gitignore filename within [`CGX_DIR`].
pub const GITIGNORE_FILE: &str = ".gitignore";
/// Body of the self-ignoring `.cgx/.gitignore` (default "nothing committed"
/// posture). `*` ignores everything in this directory — including the
/// `.gitignore` itself — so the whole `.cgx/` is invisible to the parent repo's
/// `git status` with **zero** change to any project-owned file.
///
/// Forward-compat (sparse-storage RFC §7, option c): when a user opts into the
/// in-tree committable loose store, cgx will *rewrite* this file selectively
/// (keep ignoring the rebuildable cache `index.db*`/`*.lock`, but un-ignore the
/// committable `objects/` + manifest via `!objects/` then `!objects/**`). Do not
/// build that mode now — `*` is correct for the default posture.
const GITIGNORE_BODY: &str = "*\n";

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

/// The self-ignoring gitignore path within [`CGX_DIR`].
pub fn gitignore_path(repo_root: &Path) -> PathBuf {
    cgx_dir(repo_root).join(GITIGNORE_FILE)
}

/// Ensure `.cgx/` exists and holds the self-ignoring `.gitignore` (sparse-storage
/// RFC §7, option b). cgx owns this file: it is purely additive, fully owned by
/// cgx, and removed when `.cgx/` is removed — the parent project never needs a
/// root-`.gitignore` change and never sees `.cgx/` in `git status`.
///
/// Idempotent: if a `.gitignore` already exists we leave it untouched (a future
/// in-tree-committable mode rewrites it selectively; we must not clobber it).
pub fn ensure_cgx_dir(repo_root: &Path) -> Result<(), CliError> {
    let dir = cgx_dir(repo_root);
    std::fs::create_dir_all(&dir).map_err(|e| CliError::graph(format!("creating {dir:?}: {e}")))?;
    let gi = gitignore_path(repo_root);
    if !gi.exists() {
        std::fs::write(&gi, GITIGNORE_BODY)
            .map_err(|e| CliError::graph(format!("writing {gi:?}: {e}")))?;
    }
    Ok(())
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
    ensure_cgx_dir(repo_root)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_cgx_dir_emits_self_ignoring_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!cgx_dir(root).exists());

        ensure_cgx_dir(root).unwrap();

        let gi = gitignore_path(root);
        assert!(gi.exists(), ".cgx/.gitignore must be created");
        assert_eq!(std::fs::read_to_string(&gi).unwrap(), "*\n");
    }

    #[test]
    fn ensure_cgx_dir_is_idempotent_and_never_clobbers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // First index creates the default self-ignoring file.
        ensure_cgx_dir(root).unwrap();
        // Simulate a future selective (in-tree-committable) form the user/cgx put there.
        let gi = gitignore_path(root);
        let selective = "index.db\nindex.db-wal\n!objects/\n!objects/**\n";
        std::fs::write(&gi, selective).unwrap();

        // A subsequent index must not clobber an existing .gitignore.
        ensure_cgx_dir(root).unwrap();
        assert_eq!(std::fs::read_to_string(&gi).unwrap(), selective);
    }

    #[test]
    fn write_pointer_also_emits_gitignore_on_first_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_pointer(
            root,
            &IndexPointer {
                graph_key: "deadbeef".into(),
                graph_id: 1,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(gitignore_path(root)).unwrap(),
            "*\n"
        );
    }
}
