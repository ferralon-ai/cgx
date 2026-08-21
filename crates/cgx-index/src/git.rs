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
use std::collections::BTreeMap;
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

    /// The tree OID (hex) of the commit `commit_hex` — [`head_tree_oid`](Self::head_tree_oid)
    /// for an arbitrary commit, so a query pinned to a ref can name the tree it
    /// actually answered over.
    pub fn commit_tree_oid(&self, commit_hex: &str) -> Result<String> {
        let id = gix::ObjectId::from_hex(commit_hex.as_bytes())
            .map_err(|e| IndexError::Git(format!("commit oid {commit_hex:?}: {e}")))?;
        let commit = self
            .inner
            .find_commit(id)
            .map_err(|e| IndexError::Git(e.to_string()))?;
        let tree = commit.tree().map_err(|e| IndexError::Git(e.to_string()))?;
        Ok(tree.id().to_string())
    }

    /// Path → blob OID for every blob in the tree `tree_oid`, **without** reading
    /// blob content (unlike [`enumerate_tree`](Self::enumerate_tree), which
    /// materializes every file's bytes). Tree objects only: cheap enough to run on
    /// the query path.
    pub fn tree_blob_oids(&self, tree_oid: &str) -> Result<BTreeMap<String, String>> {
        let id = gix::ObjectId::from_hex(tree_oid.as_bytes())
            .map_err(|e| IndexError::Git(format!("tree oid {tree_oid:?}: {e}")))?;
        let tree = self
            .inner
            .find_tree(id)
            .map_err(|e| IndexError::Git(e.to_string()))?;
        let entries = tree
            .traverse()
            .breadthfirst
            .files()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        let mut out = BTreeMap::new();
        for entry in entries {
            if !entry.mode.is_blob() {
                continue;
            }
            out.insert(
                entry.filepath.to_str_lossy().into_owned(),
                entry.oid.to_string(),
            );
        }
        Ok(out)
    }

    /// How many working-directory files diverge from `indexed` (a path → blob-OID
    /// map, as produced by [`tree_blob_oids`](Self::tree_blob_oids)) — the
    /// divergence half of the index-freshness envelope.
    ///
    /// Divergence is content-addressed exactly as git computes it: a file whose
    /// current content hashes to the indexed blob OID is not dirty, whatever its
    /// mtime. Three things count as one dirty file each — content changed, present
    /// on disk but absent from `indexed` (added), and present in `indexed` but
    /// absent from disk (removed).
    ///
    /// **Ignored paths are excluded**, and directories holding no indexed path are
    /// pruned unread. That is a correctness rule before it is an optimization: a
    /// committed tree can never contain an ignored path, so counting build
    /// artifacts as divergence would be wrong — and it is what keeps the query path
    /// from paying for the size of `target/`. Following git, the ignore rules are
    /// consulted **only** for paths absent from `indexed`, so a tracked-but-ignored
    /// file is still compared rather than reported as removed.
    ///
    /// **Nested repository boundaries are not descended into.** A submodule (or any
    /// other checkout nested in this one) is a single gitlink entry in the committed
    /// tree, never a set of blobs, so nothing inside it is ever an indexed path and
    /// counting its files as additions would report a clean checkout as divergent.
    /// The same reasoning bounds what this count can see: the graph never contains
    /// facts from inside a submodule either, so a submodule left at a commit other
    /// than the recorded gitlink is *not* counted — it cannot have moved the answer.
    ///
    /// **Determinism, precisely.** Given the same repository state on the same
    /// machine the result is byte-stable, which is what AR-10 requires of an answer.
    /// It is *not* a pure function of (tree content, working-tree content): the
    /// exclude stack gix assembles also folds in machine-global git configuration —
    /// `$GIT_DIR/info/exclude`, `core.excludesFile`, and `$XDG_CONFIG_HOME/git/ignore`
    /// — none of which live in the tree. Two developers with byte-identical
    /// checkouts and different global ignore rules can therefore see different
    /// counts. That axis is restated on `cgx_query::FreshnessEnvelope::dirty_files`,
    /// where a reader of the envelope will find it.
    pub fn dirty_file_count(&self, indexed: &BTreeMap<String, String>) -> Result<usize> {
        let workdir = self
            .workdir()
            .ok_or_else(|| IndexError::Git("repository has no working directory".into()))?
            .to_path_buf();
        let index = self
            .inner
            .index_or_empty()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        let mut excludes = self
            .inner
            .excludes(
                &index,
                None,
                gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            )
            .map_err(|e| IndexError::Git(e.to_string()))?;

        let hash_kind = self.inner.object_hash();
        let mut walk = DirtyWalk {
            indexed,
            excludes: &mut excludes,
            hash_kind,
            dirty: 0,
            matched: 0,
        };
        walk.visit(&workdir, "")?;
        // Symmetry with the disk-side prune: a committable store commits its
        // `.cgx/objects/**` etc. into the tree, so `indexed` can carry `.cgx/`
        // paths that the walk deliberately never visits. Excluding them from the
        // indexed denominator keeps them from counting as phantom removals — the
        // store is cgx's own artifact, not source divergence.
        let store_prefix = format!("{CGX_STORE_DIRNAME}/");
        let store_paths = indexed
            .range(store_prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&store_prefix))
            .count();
        let source_indexed = indexed.len() - store_paths;
        // Every source indexed path we never saw on disk is a removal. `matched`
        // counts distinct (non-store) indexed paths found on disk, so it can never
        // exceed `source_indexed` — except through a lossy path conversion
        // collapsing two indexed paths onto one string, which would count one
        // twice. Saturating rather than wrapping: a count is a report, and the
        // honest failure of an over-count is "zero removals", not a panic in debug
        // and ~1.8e19 in release.
        debug_assert!(
            walk.matched <= source_indexed,
            "matched {} exceeds the {source_indexed} source indexed paths",
            walk.matched,
        );
        Ok(walk.dirty + source_indexed.saturating_sub(walk.matched))
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

/// The recursive state of [`Repo::dirty_file_count`]'s working-tree walk. Carries
/// the exclude stack (which is stateful — it caches the `.gitignore` chain for the
/// directory it last visited) rather than rebuilding it per directory.
struct DirtyWalk<'a> {
    indexed: &'a BTreeMap<String, String>,
    excludes: &'a mut gix::AttributeStack<'a>,
    hash_kind: gix::hash::Kind,
    /// Working-tree files that diverge from `indexed` (modified or added).
    dirty: usize,
    /// Indexed paths seen on disk — whatever their content. `indexed.len()` minus
    /// this is the removal count.
    matched: usize,
}

/// The repo-local cgx store directory name (mirrors `cgx_cli::store_loc::CGX_DIR`,
/// duplicated here to avoid a CLI dependency in the index layer). Pruned from the
/// freshness dirty-walk because the store is cgx's own artifact, not source.
const CGX_STORE_DIRNAME: &str = ".cgx";

impl DirtyWalk<'_> {
    /// Visit `dir` (absolute), whose repo-relative form is `rel` (`""` for the
    /// root, otherwise `/`-separated with no trailing slash).
    fn visit(&mut self, dir: &Path, rel: &str) -> Result<()> {
        let read = std::fs::read_dir(dir).map_err(|source| IndexError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        for entry in read {
            let entry = entry.map_err(|source| IndexError::Io {
                path: dir.display().to_string(),
                source,
            })?;
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            // cgx's own content-addressed store (`.cgx/` at the repo root) is
            // never part of the indexed source tree. Since Slice 2 its committable
            // subset (objects/, refs/, HEAD.json) is no longer git-ignored, so
            // without this prune the store's own objects would be miscounted as
            // working-tree additions, inflating the freshness divergence. It holds
            // no indexed path by construction; skip it (root level only).
            if rel.is_empty() && name == CGX_STORE_DIRNAME {
                continue;
            }
            let file_type = entry.file_type().map_err(|source| IndexError::Io {
                path: entry.path().display().to_string(),
                source,
            })?;
            let child_rel = match rel {
                "" => name.to_string_lossy().into_owned(),
                _ => format!("{rel}/{}", name.to_string_lossy()),
            };
            if file_type.is_dir() {
                let path = entry.path();
                // Prune a directory that holds no indexed path and is either ignored
                // or a nested repository. The "holds no indexed path" test is a
                // prefix range over the sorted map, so it stays O(log n) rather than
                // a scan; it also keeps the prune honest — a path that *was* blobs
                // in the indexed tree and is a submodule now is still a divergence.
                if !self.holds_indexed_path(&child_rel)
                    && (is_nested_repository(&path) || self.is_excluded(&child_rel, true)?)
                {
                    continue;
                }
                self.visit(&path, &child_rel)?;
            } else if file_type.is_file() {
                let known = self.indexed.get(&child_rel).cloned();
                // Ignore rules apply to untracked paths only (git's own rule), so a
                // path already in the indexed tree is always compared.
                if known.is_none() && self.is_excluded(&child_rel, false)? {
                    continue;
                }
                let path = entry.path();
                let content = std::fs::read(&path).map_err(|source| IndexError::Io {
                    path: path.display().to_string(),
                    source,
                })?;
                let oid = gix::objs::compute_hash(self.hash_kind, gix::object::Kind::Blob, &content)
                    .map_err(|e| IndexError::Git(e.to_string()))?
                    .to_string();
                match known {
                    Some(indexed_oid) => {
                        self.matched += 1;
                        if indexed_oid != oid {
                            self.dirty += 1;
                        }
                    }
                    None => self.dirty += 1,
                }
            }
        }
        Ok(())
    }

    /// Whether any indexed path lives under the directory `rel`.
    fn holds_indexed_path(&self, rel: &str) -> bool {
        let prefix = format!("{rel}/");
        self.indexed
            .range(prefix.clone()..)
            .next()
            .is_some_and(|(p, _)| p.starts_with(&prefix))
    }

    fn is_excluded(&mut self, rel: &str, is_dir: bool) -> Result<bool> {
        let mode = is_dir.then_some(gix::index::entry::Mode::DIR);
        let platform = self
            .excludes
            .at_entry(rel, mode)
            .map_err(|e| IndexError::Git(e.to_string()))?;
        Ok(platform.is_excluded())
    }
}

/// Whether `dir` is the root of a *different* repository — a submodule (whose
/// `.git` is a file pointing into the parent's `modules/`) or a plain nested
/// checkout (whose `.git` is a directory). Either way git records the parent's
/// view of it as one entry, never as the files inside it.
fn is_nested_repository(dir: &Path) -> bool {
    dir.join(".git").exists()
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
