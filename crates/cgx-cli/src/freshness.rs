//! The CLI's side of the index-freshness envelope: recover the graph key the
//! answer was computed over, peel `HEAD` to a tree, and count the working-tree
//! divergence.
//!
//! The envelope type itself lives in `cgx-query` next to the approximation
//! contract, and carries no store or git dependency — identity is threaded in
//! from here, the layer that already holds it. `cgx index` writes the current
//! graph key into `.cgx/HEAD.json`
//! ([`IndexPointer`](crate::store_loc::IndexPointer)); until now every query
//! discarded it before building the `GraphView`.
//!
//! ## Never fails
//!
//! [`envelope`] answers with what it could establish and reports the rest as
//! `null`. A freshness signal must not be able to turn a successful answer into
//! an error, and a `null` is exactly the honest report for "could not establish"
//! (see the type's docs — `null` never means zero).

use std::path::Path;

use cgx_diff::BlameRepo;
use cgx_index::Repo;
use cgx_query::FreshnessEnvelope;

use crate::store_loc::read_pointer;

/// The freshness envelope for an answer produced from `repo_root`'s index.
///
/// `at` is the `--at <ref>` pin: when present the answer describes that ref's
/// tree, not the pointer's, so the divergence is measured from there.
pub fn envelope(repo_root: Option<&Path>, at: Option<&str>) -> FreshnessEnvelope {
    let Some(root) = repo_root else {
        return FreshnessEnvelope::new(None, None, None);
    };
    let repo = Repo::discover(root).ok();
    let head_tree = repo.as_ref().and_then(|r| r.head_tree_oid().ok());
    let indexed_tree = match at {
        Some(rev) => pinned_tree(root, rev, repo.as_ref()),
        None => read_pointer(root).ok().map(|p| p.graph_key),
    };

    // A synthetic `workdir:<digest>` key names no git tree, so there is nothing to
    // diff against: `tree_blob_oids` fails and the count stays `null` rather than
    // becoming a fabricated zero.
    let dirty_files = match (&repo, &indexed_tree) {
        (Some(r), Some(key)) => r
            .tree_blob_oids(key)
            .and_then(|indexed| r.dirty_file_count(&indexed))
            .ok(),
        _ => None,
    };

    FreshnessEnvelope::new(indexed_tree, head_tree, dirty_files)
}

/// The tree OID behind a `--at <ref>` pin. Resolving the ref a second time (the
/// query path already did it) is two object lookups and no walk — cheaper than
/// threading the value through an emission signature that two sibling cycles are
/// adding call sites to.
fn pinned_tree(root: &Path, rev: &str, repo: Option<&Repo>) -> Option<String> {
    let commit = BlameRepo::discover(root).ok()?.resolve_commit(rev).ok()?;
    repo?.commit_tree_oid(&commit).ok()
}
