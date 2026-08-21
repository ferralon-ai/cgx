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
/// The Slice-1 body of `.cgx/.gitignore`: ignore everything (the "nothing
/// committed" posture). Retained only as the migration key — a `.gitignore`
/// byte-equal to this is a cgx-authored default and is safe to upgrade in place
/// (see [`ensure_cgx_dir`]). Any other content is user-owned and never clobbered.
const LEGACY_GITIGNORE_BODY: &str = "*\n";

/// Body of the committable `.cgx/.gitignore` (Slice 2 — the flip). A **fail-closed
/// allowlist**: `*` ignores everything by default, then only the three
/// content-addressed graph-artifact classes are re-included, so any *future*
/// scratch/lock file type is ignored automatically rather than accidentally
/// committed.
///
/// Ordering is load-bearing: `!objects/` (re-include the directory) MUST precede
/// `!objects/**` (re-include its contents), because git will not descend into a
/// directory excluded by `*` to reconsider its children; same for `refs/`. The
/// trailing `objects/**/*.tmp` re-exclude is last so last-match-wins keeps
/// transient atomic-write temp files out even inside the re-included tree.
/// Everything else — `fragments/`, `cache.db*`, `index.db*`, all `*.lock`, WAL
/// sidecars — falls under the leading `*` and is never re-included.
const GITIGNORE_BODY: &str = "\
# cgx committable object store (Slice 2). Ignore everything by default so
# rebuildable scratch and lockfiles can never be committed; re-include only
# the content-addressed graph artifacts meant to travel in the repo.
*
!.gitignore
!HEAD.json
!objects/
!objects/**
!refs/
!refs/**
# transient atomic-write temp files are never committable:
objects/**/*.tmp
";

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

/// Ensure `.cgx/` exists and holds the committable-allowlist `.gitignore`
/// ([`GITIGNORE_BODY`]). cgx owns this file: it is purely additive, fully owned
/// by cgx, and removed when `.cgx/` is removed.
///
/// Write policy (idempotent, never-clobber a user's file):
/// - absent ⇒ write the current allowlist body;
/// - present and byte-equal to the Slice-1 [`LEGACY_GITIGNORE_BODY`] (`"*\n"`) ⇒
///   a **narrow, cgx-owned upgrade** to the allowlist so already-materialized
///   objects become git-visible (the "flip"). Only the exact cgx-authored
///   default is upgraded;
/// - present with any other content ⇒ left untouched (user-customized; the
///   never-clobber contract Slice 1 shipped stays intact).
pub fn ensure_cgx_dir(repo_root: &Path) -> Result<(), CliError> {
    let dir = cgx_dir(repo_root);
    std::fs::create_dir_all(&dir).map_err(|e| CliError::graph(format!("creating {dir:?}: {e}")))?;
    let gi = gitignore_path(repo_root);
    let write = match std::fs::read(&gi) {
        Err(_) => true,
        Ok(existing) => existing == LEGACY_GITIGNORE_BODY.as_bytes(),
    };
    if write {
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

/// The synthetic `graph_key` prefix for an index of the dirty working tree
/// (`store_loc` comment on [`IndexPointer::graph_key`]). Such a key names a
/// per-machine, non-tree ephemeral graph — never committable.
const WORKDIR_KEY_PREFIX: &str = "workdir:";

/// What [`flip_to_committable`] did — so the caller can report it and callers can
/// assert idempotency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlipOutcome {
    /// The `.gitignore` was upgraded from the Slice-1 `"*\n"` default (or created)
    /// to the committable allowlist — the graph artifacts are now git-visible.
    Flipped,
    /// The `.gitignore` was already the committable allowlist (or a user-owned
    /// file) — nothing to do. A second run lands here (idempotent).
    AlreadyCommittable,
}

/// Make the current `.cgx/` graph artifacts trackable by git — the explicit
/// "flip" (`cgx store-commit`). It (a) guards against an ephemeral `workdir:`
/// `graph_key` (committing a per-machine key is meaningless-to-harmful — recon
/// §5), then (b) runs the narrow gitignore upgrade so `objects/`, `refs/`, and
/// `HEAD.json` become git-visible. It deliberately does **not** run
/// `git add`/`git commit`: it only makes the artifacts trackable; the user
/// commits. Idempotent — running it twice is a no-op.
pub fn flip_to_committable(repo_root: &Path) -> Result<FlipOutcome, CliError> {
    let ptr = read_pointer(repo_root)?;
    if ptr.graph_key.starts_with(WORKDIR_KEY_PREFIX) {
        return Err(CliError::usage(format!(
            "refusing to flip: the current index is a working-tree graph ({}), \
             which names a per-machine ephemeral graph, not a committable tree. \
             Commit or stash your changes and re-index a clean tree, then run \
             `cgx store-commit` again.",
            ptr.graph_key
        )));
    }
    // Was the flip already applied? (`.gitignore` present and not the legacy
    // default ⇒ already the allowlist or a user file.) Determine before the
    // upgrade so we can report idempotently.
    let already = match std::fs::read(gitignore_path(repo_root)) {
        Ok(existing) => existing != LEGACY_GITIGNORE_BODY.as_bytes(),
        Err(_) => false,
    };
    ensure_cgx_dir(repo_root)?;
    Ok(if already {
        FlipOutcome::AlreadyCommittable
    } else {
        FlipOutcome::Flipped
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_cgx_dir_emits_committable_allowlist_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!cgx_dir(root).exists());

        ensure_cgx_dir(root).unwrap();

        let gi = gitignore_path(root);
        assert!(gi.exists(), ".cgx/.gitignore must be created");
        assert_eq!(std::fs::read_to_string(&gi).unwrap(), GITIGNORE_BODY);
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
            GITIGNORE_BODY
        );
    }

    #[test]
    fn migration_upgrades_only_the_exact_legacy_default() {
        // A `.cgx/.gitignore` byte-equal to the Slice-1 `"*\n"` default is a
        // cgx-authored default: it is upgraded in place to the allowlist.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(cgx_dir(root)).unwrap();
        std::fs::write(gitignore_path(root), "*\n").unwrap();

        ensure_cgx_dir(root).unwrap();
        assert_eq!(
            std::fs::read_to_string(gitignore_path(root)).unwrap(),
            GITIGNORE_BODY,
            "exact legacy default must be upgraded to the allowlist"
        );

        // Anything else — even a near-miss like `"*"` without the newline — is
        // treated as user-owned and left untouched.
        let near_miss = tempfile::tempdir().unwrap();
        let root2 = near_miss.path();
        std::fs::create_dir_all(cgx_dir(root2)).unwrap();
        std::fs::write(gitignore_path(root2), "*").unwrap();
        ensure_cgx_dir(root2).unwrap();
        assert_eq!(
            std::fs::read_to_string(gitignore_path(root2)).unwrap(),
            "*",
            "a non-exact body must never be clobbered"
        );
    }

    /// Run `git check-ignore` on `path` (relative to `repo`); `true` ⇒ git would
    /// ignore it. Requires the `.gitignore` files to already be on disk.
    fn is_ignored(repo: &Path, path: &str) -> bool {
        let out = std::process::Command::new("git")
            .args(["check-ignore", "-q", path])
            .current_dir(repo)
            .status()
            .expect("run git check-ignore");
        // exit 0 = ignored, 1 = not ignored.
        out.success()
    }

    #[test]
    fn allowlist_tracks_graph_artifacts_and_ignores_scratch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .expect("git init")
            .success());
        ensure_cgx_dir(root).unwrap();

        // Tracked: the graph artifacts that must travel with the repo.
        for p in [
            ".cgx/.gitignore",
            ".cgx/HEAD.json",
            ".cgx/objects/ab/cdef0123456789",
            ".cgx/refs/deadbeefcafef00d",
        ] {
            assert!(!is_ignored(root, p), "{p} must be tracked, not ignored");
        }

        // Ignored: rebuildable scratch, lockfiles, WAL sidecars, and transient
        // atomic-write temp objects.
        for p in [
            ".cgx/cache.db",
            ".cgx/cache.db-wal",
            ".cgx/index.db",
            ".cgx/index.db-wal",
            ".cgx/objects.lock",
            ".cgx/cache.lock",
            ".cgx/fragments/xy/blobhash",
            ".cgx/objects/ab/cdef0123.tmp",
        ] {
            assert!(is_ignored(root, p), "{p} must be ignored");
        }
    }

    #[test]
    fn flip_is_idempotent_on_a_tree_key() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Simulate a Slice-1 repo: pointer at a real tree OID, legacy gitignore.
        write_pointer(
            root,
            &IndexPointer {
                graph_key: "deadbeef".into(),
                total_files: None,
                unsupported_files: None,
            },
        )
        .unwrap();
        std::fs::write(gitignore_path(root), "*\n").unwrap();

        assert_eq!(flip_to_committable(root).unwrap(), FlipOutcome::Flipped);
        let after_first = std::fs::read_to_string(gitignore_path(root)).unwrap();
        assert_eq!(after_first, GITIGNORE_BODY);

        // Second run is a no-op.
        assert_eq!(
            flip_to_committable(root).unwrap(),
            FlipOutcome::AlreadyCommittable
        );
        assert_eq!(
            std::fs::read_to_string(gitignore_path(root)).unwrap(),
            after_first
        );
    }

    #[test]
    fn flip_refuses_a_workdir_graph_key() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_pointer(
            root,
            &IndexPointer {
                graph_key: "workdir:0011223344".into(),
                total_files: None,
                unsupported_files: None,
            },
        )
        .unwrap();

        let err = flip_to_committable(root).unwrap_err();
        assert!(
            err.message.contains("working-tree graph"),
            "workdir guard must fire: {}",
            err.message
        );
    }
}
