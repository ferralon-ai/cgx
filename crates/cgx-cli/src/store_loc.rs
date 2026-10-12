//! Store location policy (IX-8) and the current-index pointer.
//!
//! Phase-1 policy: the index lives under a `.cgx/` directory at the repository
//! root — the content-addressed object store (`objects/`, `refs/`, `fragments/`,
//! and the dataflow-cache `cache.db`), and `.cgx/HEAD.json`, which records the
//! graph the last `cgx index` produced so a subsequent query knows which
//! Layer-2 graph to load without re-indexing. (The architecture's longer-term
//! `~/.cache/cgx/<repo-id>/` location is a later refinement; a repo-local `.cgx/`
//! keeps Phase-1 self-contained and trivially inspectable.)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::exit::ExitCode;
use crate::CliError;

/// The repo-local index directory name.
pub const CGX_DIR: &str = ".cgx";
/// Files an older cgx wrote beside the object store: a full SQLite copy of every
/// graph, kept only for parity checking. Nothing reads them any more.
const RETIRED_FILES: [&str; 4] = ["index.db", "index.db-wal", "index.db-shm", "index.lock"];
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
/// (keep ignoring the rebuildable cache `cache.db*`/`*.lock`, but un-ignore the
/// committable `objects/` + manifest via `!objects/` then `!objects/**`). Do not
/// build that mode now — `*` is correct for the default posture.
const GITIGNORE_BODY: &str = "*\n";

/// The pointer written by `cgx index` and read by every query subcommand: which
/// stored Layer-2 graph is "current", and what tree/workdir key it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexPointer {
    /// The Layer-2 key the graph was stored under (tree OID or `workdir:<digest>`).
    /// This is the read key: `FactStore::read_graph` is keyed by it directly.
    pub graph_key: String,
    /// Total files seen by the pipeline (supported + unsupported), from
    /// `IndexStats`. `#[serde(default)]` so a pointer written by a pre-existing
    /// `.cgx/HEAD.json` (before this field existed) still parses; `cgx doctor`
    /// then falls back to its "pipeline stats unavailable" placeholder.
    #[serde(default)]
    pub total_files: Option<usize>,
    /// Files skipped because no registered adapter claimed them, from
    /// `IndexStats`. See `total_files` for the `#[serde(default)]` rationale.
    #[serde(default)]
    pub unsupported_files: Option<usize>,
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

/// The current-index pointer path.
pub fn head_path(repo_root: &Path) -> PathBuf {
    cgx_dir(repo_root).join(HEAD_FILE)
}

/// Persist the current-index pointer (deterministic: stable field order, trailing
/// newline). Called by `cgx index` after a successful store. Written temp → flush
/// → rename, so a crash leaves either the previous pointer or the new one.
pub fn write_pointer(repo_root: &Path, ptr: &IndexPointer) -> Result<(), CliError> {
    ensure_cgx_dir(repo_root)?;
    let mut json = serde_json::to_string_pretty(ptr).expect("IndexPointer serializes");
    json.push('\n');
    let path = head_path(repo_root);
    cgx_store::durable::write_durable(&path, json.as_bytes())
        .map_err(|e| CliError::graph(format!("writing {path:?}: {e}")))?;
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
        let selective = "cache.db\ncache.db-wal\n!objects/\n!objects/**\n";
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
                total_files: None,
                unsupported_files: None,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(gitignore_path(root)).unwrap(),
            "*\n"
        );
    }

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
