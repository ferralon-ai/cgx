//! # cgx-transport
//!
//! The **ref-overlay transport** for the Slice-1 content-addressed store: it moves
//! the committable `.cgx/` set — `objects/` + `refs/` + `HEAD.json` (never
//! `fragments/`, `index.db`, or `cache.db`; D-a) — between clones through git's
//! own object database under a single custom ref, **`refs/cgx/index`**, using only
//! the `git` binary on `PATH` (no libgit2; criterion 6).
//!
//! ## Object identity — two hash spaces
//!
//! A cgx object OID is `SHA-1(canonical-postcard-bytes)` with **no** git framing
//! (`cgx-store/src/oid.rs`), whereas a git blob OID is `SHA-1("blob <len>\0" ‖
//! bytes)`. They never coincide. Because the bytes stored on disk are exactly the
//! bytes hashed, promoting a stored object file **verbatim** through
//! `git hash-object -w` preserves byte-identity: the git tree records the **cgx
//! OID as the entry name** and the git blob OID as the entry's blob. On pull the
//! bytes are read back and re-hashed; `SHA-1(bytes)` must equal the entry name
//! (criterion 5 integrity — a mismatch is a hard error, never served).
//!
//! ## Push (D-b / D-c)
//!
//! Enumerate the union of object closures reachable from **all** local
//! `.cgx/refs/*` pointers (each ref → its manifest → `referenced_oids()`, plus the
//! manifest OID itself), promote each object's bytes into git, and assemble a tree
//! that mirrors the `.cgx/` relative layout (`objects/<oid[0:2]>/<oid[2:]>`,
//! `refs/<name>`, `HEAD.json`). Wrap it in a **deterministic parentless commit**
//! (pinned identity/date/message), compare-and-swap `refs/cgx/index` to it, and
//! push `+refs/cgx/index:refs/cgx/index`. Recompute-wins: a push *replaces* the
//! ref with the pusher's current local committable state (no remote-side merge).
//!
//! ## Pull
//!
//! Fetch the ref (loud failure — [`TransportError::RefAbsent`] — if it is absent
//! afterward, never a silent empty), list every blob, verify each `objects/` blob
//! against its OID, and materialize the tree back into `.cgx/` via atomic
//! temp→rename writes.

mod error;
mod git;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::Path;

use cgx_store::manifest::Manifest;
use cgx_store::ObjectOid;

use crate::git::{Git, TreeEntry, ZERO_OID};

pub use crate::error::{TransportError, CGX_REFSPEC};

/// The custom ref carrying the whole committable index. Walked by `git log`/
/// `blame`/`refs/heads/*` diffs? No — those walk `HEAD` only, so the index stays
/// invisible to history (criterion 3). It *is* shown by `git log --all`.
pub const CGX_REF: &str = "refs/cgx/index";

/// Outcome of a successful [`push`].
#[derive(Debug, Clone)]
pub struct PushOutcome {
    /// The deterministic commit OID `refs/cgx/index` now points at.
    pub commit_oid: String,
    /// Whether the CAS moved the local ref (false = it already pointed here, an
    /// idempotent re-push of unchanged state).
    pub ref_advanced: bool,
    /// Count of content-addressed objects promoted into git (honesty: printed so a
    /// push is never a silent success on an empty store).
    pub objects_promoted: usize,
}

/// Outcome of a successful [`pull`].
#[derive(Debug, Clone)]
pub struct PullOutcome {
    /// Count of content-addressed objects written into `.cgx/objects/`.
    pub objects_materialized: usize,
    /// Count of restored per-graph pointer files (`.cgx/refs/*`).
    pub graph_keys: usize,
}

/// Promote the local `.cgx` committable set into git's object DB under
/// [`CGX_REF`] (a deterministic parentless commit), compare-and-swap-advance the
/// ref, then push it to `remote` via `+refs/cgx/index:refs/cgx/index`.
pub fn push(repo_root: &Path, remote: &str) -> Result<PushOutcome, TransportError> {
    let git = Git::new(repo_root);
    let cgx_dir = repo_root.join(".cgx");

    let built = build_committable_tree(&git, &cgx_dir)?;
    // Refuse to publish an empty index ref: an unindexed `.cgx/` has no pointers,
    // so promoting nothing would advance the ref to an empty tree and let a puller
    // materialize nothing while both sides report success (honesty spine).
    if built.objects_promoted == 0 {
        return Err(TransportError::NothingIndexed);
    }
    let commit_oid = git.commit_tree(&built.tree_oid)?;
    let ref_advanced = advance_ref(&git, &commit_oid)?;
    git.push(remote, "+refs/cgx/index:refs/cgx/index")?;

    Ok(PushOutcome {
        commit_oid,
        ref_advanced,
        objects_promoted: built.objects_promoted,
    })
}

/// Fetch [`CGX_REF`] from `remote` and materialize `.cgx/{objects,refs,HEAD.json}`
/// locally, verifying each object's bytes re-hash to its OID. Fails loudly (never
/// silently empty) if the ref is absent after the fetch.
pub fn pull(repo_root: &Path, remote: &str) -> Result<PullOutcome, TransportError> {
    let git = Git::new(repo_root);
    let cgx_dir = repo_root.join(".cgx");

    // An explicit single-ref fetch hard-errors when the remote lacks the ref
    // (rather than succeeding empty), so a genuinely-absent index would surface as
    // a raw git error. Separate the honest "not indexed on the remote yet" case
    // (loud, actionable — RefAbsent with the refspec hint) from a real transport
    // failure by asking `ls-remote` whether the ref exists at all.
    if let Err(e) = git.fetch(remote, "+refs/cgx/index:refs/cgx/index") {
        // If the remote simply doesn't advertise the ref, that's the honest,
        // actionable RefAbsent case; anything else is a real transport failure.
        return match git.ls_remote_has(remote, CGX_REF) {
            Ok(false) => Err(TransportError::ref_absent()),
            _ => Err(e),
        };
    }
    if git.rev_parse(CGX_REF)?.is_none() {
        return Err(TransportError::ref_absent());
    }

    let mut objects_materialized = 0usize;
    let mut graph_keys = 0usize;

    for (path, blob_oid) in git.ls_tree_r(CGX_REF)? {
        let bytes = git.cat_file_blob(&blob_oid)?;

        if let Some(rest) = path.strip_prefix("objects/") {
            // Path is `objects/<oid[0:2]>/<oid[2:]>`; the OID is the concatenation.
            let oid = rest.replace('/', "");
            let got = ObjectOid::of_bytes(&bytes);
            if got.as_str() != oid {
                return Err(TransportError::Integrity {
                    oid,
                    got: got.0,
                });
            }
            objects_materialized += 1;
        } else if path.starts_with("refs/") {
            graph_keys += 1;
        }

        atomic_write(&cgx_dir.join(&path), &bytes)?;
    }

    Ok(PullOutcome {
        objects_materialized,
        graph_keys,
    })
}

/// Write `remote.<remote>.fetch`/`.push = +refs/cgx/*:refs/cgx/*` via
/// `git config --add` (idempotent — a value already present is not re-added).
pub fn configure_remote(repo_root: &Path, remote: &str) -> Result<(), TransportError> {
    let git = Git::new(repo_root);
    for verb in ["fetch", "push"] {
        let key = format!("remote.{remote}.{verb}");
        if !git.config_get_all(&key)?.iter().any(|v| v == CGX_REFSPEC) {
            git.config_add(&key, CGX_REFSPEC)?;
        }
    }
    Ok(())
}

/// The tree OID mirroring the committable set, plus the count of promoted objects.
struct BuiltTree {
    tree_oid: String,
    objects_promoted: usize,
}

/// Enumerate the committable closure and assemble the mirror tree (D-b).
fn build_committable_tree(git: &Git, cgx_dir: &Path) -> Result<BuiltTree, TransportError> {
    let objects_dir = cgx_dir.join("objects");
    let refs_dir = cgx_dir.join("refs");
    let head_path = cgx_dir.join("HEAD.json");

    // Union of object closures over every local pointer, plus each pointer's own
    // bytes (a ref file is NOT reconstructable from the object closure).
    let mut closure: BTreeSet<String> = BTreeSet::new();
    let mut ref_files: Vec<(String, Vec<u8>)> = Vec::new();

    if let Some(entries) = read_dir_sorted(&refs_dir)? {
        for entry in entries {
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let bytes = fs::read(entry.path())?;
            // Ref file: `<manifest_oid>\n<graph_key>\n`.
            let manifest_oid = String::from_utf8_lossy(&bytes)
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned();
            if !manifest_oid.is_empty() {
                closure.insert(manifest_oid.clone());
                let manifest_path = ObjectOid(manifest_oid).object_path(&objects_dir);
                let manifest: Manifest = cgx_core::codec::decode(&fs::read(&manifest_path)?)
                    .map_err(TransportError::codec)?;
                for oid in manifest.referenced_oids() {
                    closure.insert(oid.0.clone());
                }
            }
            ref_files.push((name, bytes));
        }
    }

    // Promote every closure object verbatim and place it at its `.cgx/` path.
    let mut root = TreeNode::default();
    let mut objects_promoted = 0usize;
    for oid in &closure {
        let path = ObjectOid(oid.clone()).object_path(&objects_dir);
        let blob = git.hash_object_w(&fs::read(&path)?)?;
        root.insert(&format!("objects/{}/{}", &oid[..2], &oid[2..]), blob);
        objects_promoted += 1;
    }
    for (name, bytes) in &ref_files {
        let blob = git.hash_object_w(bytes)?;
        root.insert(&format!("refs/{name}"), blob);
    }
    if let Some(bytes) = read_if_exists(&head_path)? {
        let blob = git.hash_object_w(&bytes)?;
        root.insert("HEAD.json", blob);
    }

    let tree_oid = build_tree(git, &root)?;
    Ok(BuiltTree {
        tree_oid,
        objects_promoted,
    })
}

/// CAS-advance [`CGX_REF`] to `commit_oid`, retrying on contention (recompute-wins
/// means the commit is deterministic, so a retry re-uses the same commit against
/// the freshly-read expected value). Returns whether the ref actually moved.
fn advance_ref(git: &Git, commit_oid: &str) -> Result<bool, TransportError> {
    for _ in 0..8 {
        let current = git.rev_parse(CGX_REF)?;
        if current.as_deref() == Some(commit_oid) {
            return Ok(false);
        }
        let expected = current.as_deref().unwrap_or(ZERO_OID);
        match git.update_ref_cas(CGX_REF, commit_oid, expected) {
            Ok(()) => return Ok(true),
            // A lost CAS is the only expected git failure here; re-read + retry.
            Err(TransportError::Git { .. }) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(TransportError::RefContention)
}

/// An in-memory tree mirroring `.cgx/` relative paths, built bottom-up into git
/// tree objects. Blobs and subdirs are kept in `BTreeMap`s so assembly order is
/// stable (git canonicalizes the final tree regardless).
#[derive(Default)]
struct TreeNode {
    blobs: BTreeMap<String, String>,
    dirs: BTreeMap<String, TreeNode>,
}

impl TreeNode {
    fn insert(&mut self, path: &str, blob_oid: String) {
        match path.split_once('/') {
            None => {
                self.blobs.insert(path.to_owned(), blob_oid);
            }
            Some((head, rest)) => {
                self.dirs
                    .entry(head.to_owned())
                    .or_default()
                    .insert(rest, blob_oid);
            }
        }
    }
}

/// Recursively `mktree` a [`TreeNode`], returning its git tree OID.
fn build_tree(git: &Git, node: &TreeNode) -> Result<String, TransportError> {
    let mut entries: Vec<TreeEntry> = Vec::new();
    for (name, oid) in &node.blobs {
        entries.push(TreeEntry {
            mode: "100644",
            kind: "blob",
            oid: oid.clone(),
            name: name.clone(),
        });
    }
    for (name, child) in &node.dirs {
        let child_oid = build_tree(git, child)?;
        entries.push(TreeEntry {
            mode: "040000",
            kind: "tree",
            oid: child_oid,
            name: name.clone(),
        });
    }
    git.mktree(&entries)
}

/// Directory entries sorted by file name, or `None` if the directory is absent.
fn read_dir_sorted(dir: &Path) -> Result<Option<Vec<fs::DirEntry>>, TransportError> {
    match fs::read_dir(dir) {
        Ok(rd) => {
            let mut v = rd.collect::<std::io::Result<Vec<_>>>()?;
            v.sort_by_key(|e| e.file_name());
            Ok(Some(v))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn read_if_exists(path: &Path) -> Result<Option<Vec<u8>>, TransportError> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write `bytes` to `path` via temp → `fsync` → atomic `rename`, creating parent
/// directories. Mirrors the store's own write discipline so a torn object never
/// occupies its final name.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), TransportError> {
    let parent = path
        .parent()
        .expect("materialized path has a parent under .cgx");
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .expect("materialized path has a file name")
        .to_str()
        .expect("materialized file name is utf-8");
    let tmp = parent.join(format!("{file_name}.cgx-tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
