//! The git-facing source-enumeration layer (docs/06 IX-1/IX-6).
//!
//! Two enumeration modes feed the same pipeline:
//!
//! - **Committed tree** ([`enumerate_tree`]): walk a commit's tree, yielding one
//!   [`SourceFile`] per blob with its real git blob OID. Unchanged blobs across
//!   branches/worktrees share an OID, which is what makes the Layer-1 cache a hit
//!   (IX-1).
//! - **Working directory** ([`enumerate_workdir`]): walk a checkout on disk,
//!   computing each file's git blob OID with the same SHA git would use
//!   (`blob <len>\0<content>`), so worktree/uncommitted files are indexed too
//!   (IX-6, IX-3). A worktree file whose content matches a committed blob yields
//!   the identical OID and is therefore a cache hit against committed facts.
//!
//! All paths are repo-relative, `/`-separated. Enumeration is deterministic:
//! results are sorted by path.

use crate::error::{IndexError, Result};
use gix::bstr::ByteSlice;
use std::path::Path;

/// One source file to index: its content-addressed identity plus its bytes.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Git blob OID (hex). For working-dir files this is the synthetic OID git
    /// *would* assign — identical to the committed OID when content matches.
    pub blob_oid: String,
    /// Repo-relative, `/`-separated path.
    pub rel_path: String,
    /// Raw file bytes.
    pub content: Vec<u8>,
}

/// A repository handle plus the means to enumerate sources from it.
#[derive(Debug)]
pub struct Repo {
    inner: gix::Repository,
}

impl Repo {
    /// Discover the repository containing `path` (the `.git` dir may be a parent).
    pub fn discover(path: impl AsRef<Path>) -> Result<Self> {
        let inner = gix::discover(path.as_ref()).map_err(|e| IndexError::Git(e.to_string()))?;
        Ok(Repo { inner })
    }

    /// The tree OID (hex) of `HEAD`'s commit — the Layer-2 key for the committed
    /// state (IX-2 staleness signal).
    pub fn head_tree_oid(&self) -> Result<String> {
        let commit = self
            .inner
            .head_commit()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        let tree = commit.tree().map_err(|e| IndexError::Git(e.to_string()))?;
        Ok(tree.id().to_string())
    }

    /// The absolute working directory of the main checkout, if any.
    pub fn workdir(&self) -> Option<&Path> {
        self.inner.workdir()
    }

    /// Enumerate every blob in `HEAD`'s tree as a [`SourceFile`], sorted by path.
    pub fn enumerate_tree(&self) -> Result<Vec<SourceFile>> {
        let commit = self
            .inner
            .head_commit()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        let tree = commit.tree().map_err(|e| IndexError::Git(e.to_string()))?;

        let mut out = Vec::new();
        let entries = tree
            .traverse()
            .breadthfirst
            .files()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        for entry in entries {
            if !entry.mode.is_blob() {
                continue;
            }
            let rel_path = entry.filepath.to_str_lossy().into_owned();
            let obj = self
                .inner
                .find_object(entry.oid)
                .map_err(|e| IndexError::Git(e.to_string()))?;
            out.push(SourceFile {
                blob_oid: entry.oid.to_string(),
                rel_path,
                content: obj.data.clone(),
            });
        }
        out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(out)
    }

    /// Enumerate source files from `root` on disk (a working directory or linked
    /// worktree), computing each file's git blob OID from its current content
    /// (IX-6 worktree awareness, IX-3 dirty handling). `.git` is skipped. Results
    /// are sorted by repo-relative path.
    pub fn enumerate_workdir(&self, root: impl AsRef<Path>) -> Result<Vec<SourceFile>> {
        let root = root.as_ref();
        let hash_kind = self.inner.object_hash();
        let mut out = Vec::new();
        walk_dir(root, root, hash_kind, &mut out)?;
        out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(out)
    }

    /// The absolute base directories of all linked worktrees (IX-6). The main
    /// checkout is [`workdir`](Self::workdir); these are the *additional* ones.
    pub fn linked_worktree_dirs(&self) -> Result<Vec<std::path::PathBuf>> {
        let mut dirs = Vec::new();
        for wt in self
            .inner
            .worktrees()
            .map_err(|e| IndexError::Git(e.to_string()))?
        {
            let base = wt.base().map_err(|e| IndexError::Git(e.to_string()))?;
            dirs.push(base);
        }
        dirs.sort();
        Ok(dirs)
    }
}

/// Compute the git blob OID (hex) a file with `content` would have, without
/// writing it (IX-3: synthetic OID for dirty/worktree files). Uses SHA-1 by
/// default to match git's object hash.
pub fn compute_blob_oid(content: &[u8]) -> String {
    gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::object::Kind::Blob, content)
        .expect("blob hashing is infallible for in-memory content")
        .to_string()
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    hash_kind: gix::hash::Kind,
    out: &mut Vec<SourceFile>,
) -> Result<()> {
    let read = std::fs::read_dir(dir).map_err(|source| IndexError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for entry in read {
        let entry = entry.map_err(|source| IndexError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        let file_name = entry.file_name();
        if file_name == ".git" {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| IndexError::Io {
            path: path.display().to_string(),
            source,
        })?;
        if file_type.is_dir() {
            walk_dir(root, &path, hash_kind, out)?;
        } else if file_type.is_file() {
            let content = std::fs::read(&path).map_err(|source| IndexError::Io {
                path: path.display().to_string(),
                source,
            })?;
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let blob_oid = gix::objs::compute_hash(hash_kind, gix::object::Kind::Blob, &content)
                .map_err(|e| IndexError::Git(e.to_string()))?
                .to_string();
            out.push(SourceFile {
                blob_oid,
                rel_path: rel,
                content,
            });
        }
    }
    Ok(())
}
