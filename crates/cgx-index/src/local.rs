//! The repo-local persistent index: where it lives, the current-index pointer,
//! and the auto-index policy every query surface (CLI and MCP) shares.
//!
//! The index lives under a `.cgx/` directory at the repository root — the
//! content-addressed object store (`objects/`, `refs/`, `fragments/`, and the
//! dataflow-cache `cache.db`), and `.cgx/HEAD.json`, which records the graph the
//! last index run produced so a subsequent query knows which Layer-2 graph to
//! load without re-indexing. (The architecture's longer-term
//! `~/.cache/cgx/<repo-id>/` location is a later refinement; a repo-local `.cgx/`
//! keeps the index self-contained and trivially inspectable.)
//!
//! ## Concurrency
//!
//! Any number of processes may run [`ensure_indexed`] and [`load_current`] on one
//! `.cgx/` at once. Store writes are serialized by the store's advisory write
//! lock and are write-once per content address, so two runs over the same tree
//! write the same objects. The pointer is replaced by an atomic rename. A graph
//! that a concurrent run pruned or tore reads as missing or corrupt, and
//! [`load_current`] answers that by re-indexing rather than by serving it.

use std::path::{Path, PathBuf};

use cgx_store::{FactStore, LinkedGraph, ObjectStore, StoreError, TreeOid};
use serde::{Deserialize, Serialize};

use crate::error::{IndexError, Result};
use crate::{default_registry, index_path, IndexOpts, IndexOutcome, Repo};

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

/// The pointer written by an index run and read by every query: which stored
/// Layer-2 graph is "current", and what tree/workdir key it came from.
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
pub fn ensure_cgx_dir(repo_root: &Path) -> Result<()> {
    let dir = cgx_dir(repo_root);
    std::fs::create_dir_all(&dir)
        .map_err(|e| IndexError::Local(format!("creating {dir:?}: {e}")))?;
    let gi = gitignore_path(repo_root);
    if !gi.exists() {
        std::fs::write(&gi, GITIGNORE_BODY)
            .map_err(|e| IndexError::Local(format!("writing {gi:?}: {e}")))?;
    }
    Ok(())
}

/// Delete the [`RETIRED_FILES`] an older cgx left in `.cgx/`, if present.
pub fn remove_retired_files(repo_root: &Path) -> Result<()> {
    for name in RETIRED_FILES {
        let path = cgx_dir(repo_root).join(name);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(IndexError::Local(format!("removing {path:?}: {e}")))
            }
            _ => {}
        }
    }
    Ok(())
}

/// Persist the current-index pointer (deterministic: stable field order, trailing
/// newline). Written temp → flush → rename, so a crash leaves either the previous
/// pointer or the new one.
pub fn write_pointer(repo_root: &Path, ptr: &IndexPointer) -> Result<()> {
    ensure_cgx_dir(repo_root)?;
    let mut json = serde_json::to_string_pretty(ptr).expect("IndexPointer serializes");
    json.push('\n');
    let path = head_path(repo_root);
    cgx_store::durable::write_durable(&path, json.as_bytes())
        .map_err(|e| IndexError::Local(format!("writing {path:?}: {e}")))?;
    Ok(())
}

/// Read the current-index pointer. A repo with none is [`IndexError::NoIndex`].
pub fn read_pointer(repo_root: &Path) -> Result<IndexPointer> {
    let path = head_path(repo_root);
    let bytes = std::fs::read(&path).map_err(|_| IndexError::NoIndex(cgx_dir(repo_root)))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| IndexError::Local(format!("corrupt index pointer {path:?}: {e}")))
}

/// Open the repo's persistent object store.
pub fn open_store(repo_root: &Path) -> Result<ObjectStore> {
    ObjectStore::open(cgx_dir(repo_root))
        .map_err(|e| IndexError::Local(format!("opening store at {repo_root:?}: {e}")))
}

/// Index the committed `HEAD` tree of `repo_root` to `.cgx/` and persist the
/// current-index pointer — the shared core of `cgx index` and the auto-index
/// trigger ([`ensure_indexed`]). Deterministic and idempotent: a re-index of an
/// unchanged tree extracts 0 blobs (IX-1) and rewrites an identical pointer.
pub fn index_repo(repo_root: &Path, opts: &IndexOpts) -> Result<IndexOutcome> {
    let registry = default_registry();
    let mut store = open_store(repo_root)?;
    let outcome = index_path(repo_root, &registry, &mut store, opts)
        .map_err(|e| IndexError::Build(Box::new(e)))?;
    // Blast-radius fix (sparse-storage RFC P1): GC every superseded Layer-2 graph,
    // keeping only the one we just wrote. No read path needs historical graphs on
    // the normal index path; the `diff` path uses its own session and never lands
    // here. Bounded (retain exactly 1) and deterministic (delete-by-key).
    let keep = TreeOid::new(outcome.graph_key.clone());
    store
        .prune_graphs_except(&[&keep])
        .map_err(|e| IndexError::Local(format!("pruning stale graphs: {e}")))?;
    remove_retired_files(repo_root)?;
    write_pointer(
        repo_root,
        &IndexPointer {
            graph_key: outcome.graph_key.clone(),
            total_files: Some(outcome.stats.blobs_indexed + outcome.stats.blobs_unsupported),
            unsupported_files: Some(outcome.stats.blobs_unsupported),
        },
    )?;
    Ok(outcome)
}

/// Ensure `repo_root` has a current `.cgx/` index before a query reads it.
/// Auto-indexes the committed `HEAD` tree when no pointer exists, or when the
/// pointer's tree OID differs from the current `HEAD` tree OID (a cheap staleness
/// check). A no-op when the index is already current.
///
/// Idempotent: the underlying pipeline performs zero extraction on an unchanged
/// tree (IX-1), so a re-index over the same committed state re-links cached facts
/// without re-parsing.
pub fn ensure_indexed(repo_root: &Path) -> Result<()> {
    // Not a git repo / no HEAD → `None`: we then fall back to "index iff the
    // pointer is missing" (no staleness check is possible without a tree OID).
    let current_tree = Repo::discover(repo_root)
        .and_then(|r| r.head_tree_oid())
        .ok();

    let opts = auto_index_opts(repo_root);
    match read_pointer(repo_root) {
        Ok(ptr) => match &current_tree {
            // Pointer's tree OID differs from the live HEAD tree: stale, re-index.
            Some(tree) if *tree != ptr.graph_key => index_repo(repo_root, &opts).map(|_| ()),
            // Fresh, or HEAD tree undeterminable (non-git): trust the pointer.
            _ => Ok(()),
        },
        // No index yet: build one.
        Err(_) => index_repo(repo_root, &opts).map(|_| ()),
    }
}

/// Read the current Layer-2 graph named by the pointer, with its key.
///
/// With `rebuild_if_corrupt`, a graph whose objects fail verification — or whose
/// ref is gone although the pointer names it, which is what a concurrent index of
/// another tree leaves behind once it prunes — is treated as not indexed: it is
/// re-indexed once and read again. Without it, either case is an error; an empty
/// graph is never served in place of a missing one.
pub fn load_current(repo_root: &Path, rebuild_if_corrupt: bool) -> Result<(String, LinkedGraph)> {
    let ptr = read_pointer(repo_root)?;
    let store = open_store(repo_root)?;
    let tree = TreeOid::new(ptr.graph_key.clone());
    let read = match store.graph_for(&tree) {
        Ok(true) => store.read_graph(&tree),
        Ok(false) => Err(StoreError::Corrupt {
            oid: ptr.graph_key.clone(),
        }),
        Err(e) => Err(e),
    };
    match read {
        Err(StoreError::Corrupt { .. }) if rebuild_if_corrupt => {
            drop(store);
            index_repo(repo_root, &auto_index_opts(repo_root))?;
            load_current(repo_root, false)
        }
        Ok(graph) => Ok((ptr.graph_key, graph)),
        Err(e) => Err(IndexError::Local(format!(
            "reading graph {}: {e}",
            ptr.graph_key
        ))),
    }
}

/// The options an automatic (query-triggered) index runs with. v0.3 SC6: an
/// auto-built index is a STANDARD index — dataflow ON by default, so a fresh
/// `flows-*`/`:DATA_FLOW` query works with no prior explicit `index`. The
/// `[index] data_flow = false` cgx.toml key opts back out.
pub fn auto_index_opts(repo_root: &Path) -> IndexOpts {
    IndexOpts {
        dataflow: dataflow_default(repo_root),
        ..IndexOpts::default()
    }
}

/// The effective on-by-default dataflow setting for `repo_root` (v0.3 SC6).
/// Dataflow ships ON; the only off-switch at config level is the
/// `[index] data_flow = false` key in a `cgx.toml` at the repo root. Any other
/// value (or a missing key/file) leaves dataflow enabled.
pub fn dataflow_default(repo_root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(repo_root.join("cgx.toml")) else {
        return true;
    };
    !cgx_toml_data_flow_is_false(&text)
}

/// Whether a `cgx.toml`'s `[index]` table sets `data_flow = false`. A minimal
/// section-scoped line scan — cgx carries no `toml` dependency by design (cf.
/// `cargo_pkg.rs`) — so a `data_flow` key under another table is ignored.
pub fn cgx_toml_data_flow_is_false(text: &str) -> bool {
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

    #[test]
    fn a_pointer_to_a_missing_graph_is_never_served_empty() {
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
        let err = load_current(root, false).unwrap_err();
        assert!(
            err.to_string().starts_with("reading graph deadbeef"),
            "{err}"
        );
    }
}
