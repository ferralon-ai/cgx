//! Store location (IX-8), the current-index pointer, and the `cgx.toml` dataflow
//! switch: the parts of the on-disk index layout every writer of `.cgx/` shares.
//!
//! The index lives under a `.cgx/` directory at the repository root, and
//! `.cgx/HEAD.json` records the graph the last index produced so a later query
//! knows which Layer-2 graph to load without re-indexing. The CLI and a resident
//! session (native or wasm) write the pointer through this one implementation, so
//! its bytes do not depend on which of them indexed.
//!
//! Errors are message strings; callers attach their own error kind (the CLI its
//! exit code, a session its error envelope).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The repo-local index directory name.
pub const CGX_DIR: &str = ".cgx";
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

/// The pointer written by an index run and read by every query: which stored
/// Layer-2 graph is "current", and what tree/workdir key it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl IndexPointer {
    /// Whether this pointer names the current graph for a repository whose `HEAD`
    /// tree is `head_tree` (the auto-index staleness rule, audit row #23): fresh
    /// iff the pointer's key is that tree. With no tree OID (not a git repository,
    /// unborn `HEAD`) no staleness check is possible and the pointer is trusted.
    pub fn is_fresh(&self, head_tree: Option<&str>) -> bool {
        head_tree.is_none_or(|tree| tree == self.graph_key)
    }
}

/// The `.cgx/` directory for a repository rooted at `repo_root`.
pub fn cgx_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(CGX_DIR)
}

/// The self-ignoring gitignore path within [`CGX_DIR`].
pub fn gitignore_path(repo_root: &Path) -> PathBuf {
    cgx_dir(repo_root).join(GITIGNORE_FILE)
}

/// The current-index pointer path.
pub fn head_path(repo_root: &Path) -> PathBuf {
    cgx_dir(repo_root).join(HEAD_FILE)
}

/// Ensure `.cgx/` exists and holds the self-ignoring `.gitignore` (sparse-storage
/// RFC §7, option b). cgx owns this file: it is purely additive, fully owned by
/// cgx, and removed when `.cgx/` is removed — the parent project never needs a
/// root-`.gitignore` change and never sees `.cgx/` in `git status`.
///
/// Idempotent: if a `.gitignore` already exists we leave it untouched (a future
/// in-tree-committable mode rewrites it selectively; we must not clobber it).
pub fn ensure_cgx_dir(repo_root: &Path) -> Result<(), String> {
    let dir = cgx_dir(repo_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    let gi = gitignore_path(repo_root);
    if !gi.exists() {
        std::fs::write(&gi, GITIGNORE_BODY).map_err(|e| format!("writing {gi:?}: {e}"))?;
    }
    Ok(())
}

/// Persist the current-index pointer (deterministic: stable field order, trailing
/// newline). Called after a successful store of a committed-tree graph.
pub fn write_pointer(repo_root: &Path, ptr: &IndexPointer) -> Result<(), String> {
    ensure_cgx_dir(repo_root)?;
    let mut json = serde_json::to_string_pretty(ptr).expect("IndexPointer serializes");
    json.push('\n');
    let path = head_path(repo_root);
    std::fs::write(&path, json).map_err(|e| format!("writing {path:?}: {e}"))?;
    Ok(())
}

/// Read the current-index pointer. `Ok(None)` when there is none to read (the
/// repository is not indexed); an error when one exists but does not parse.
pub fn read_pointer(repo_root: &Path) -> Result<Option<IndexPointer>, String> {
    let path = head_path(repo_root);
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("corrupt index pointer {path:?}: {e}"))
}

/// The effective on-by-default dataflow setting given the text of a repository's
/// `cgx.toml` (`None` when there is none) — v0.3 SC6. Dataflow ships ON; the only
/// off-switch at config level is an `[index] data_flow = false` key. Any other
/// value, or a missing key/file, leaves dataflow enabled.
pub fn dataflow_default(cgx_toml: Option<&str>) -> bool {
    !cgx_toml.is_some_and(cgx_toml_data_flow_is_false)
}

/// Whether a `cgx.toml`'s `[index]` table sets `data_flow = false`. A minimal
/// scanner — cgx carries no `toml` dependency by design (cf. cgx-index's
/// `cargo_pkg`): find the `[index]` section header, then the first `data_flow = …`
/// assignment within it, and test for a literal `false`. Section-scoped so a
/// `data_flow` key under another table is ignored.
fn cgx_toml_data_flow_is_false(text: &str) -> bool {
    let mut in_index = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_index = line == "[index]";
            continue;
        }
        if !in_index {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "data_flow" {
            return value.trim() == "false";
        }
    }
    false
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
    fn pointer_round_trips_and_a_missing_one_reads_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(read_pointer(root).unwrap(), None);
        let ptr = IndexPointer {
            graph_key: "abc".into(),
            total_files: Some(3),
            unsupported_files: Some(1),
        };
        write_pointer(root, &ptr).unwrap();
        assert_eq!(read_pointer(root).unwrap(), Some(ptr));
        std::fs::write(head_path(root), "{").unwrap();
        assert!(read_pointer(root).unwrap_err().starts_with("corrupt index pointer"));
    }

    #[test]
    fn freshness_is_key_equality_and_trusts_the_pointer_without_a_tree() {
        let ptr = IndexPointer {
            graph_key: "t1".into(),
            total_files: None,
            unsupported_files: None,
        };
        assert!(ptr.is_fresh(Some("t1")));
        assert!(!ptr.is_fresh(Some("t2")));
        assert!(ptr.is_fresh(None));
    }

    #[test]
    fn dataflow_is_on_unless_the_index_table_turns_it_off() {
        assert!(dataflow_default(None));
        assert!(dataflow_default(Some("")));
        assert!(!dataflow_default(Some("[index]\ndata_flow = false\n")));
        assert!(dataflow_default(Some("[index]\ndata_flow = true\n")));
        assert!(dataflow_default(Some("[other]\ndata_flow = false\n")));
        assert!(!dataflow_default(Some("[index] \n data_flow = false # off\n")));
    }
}
