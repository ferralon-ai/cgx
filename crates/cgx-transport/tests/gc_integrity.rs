//! Criterion 5 — gc-pinned + integrity: objects reachable from `refs/cgx/index`
//! survive `git gc --prune=now`, and a corrupted/tampered object is detected by
//! OID mismatch on pull, never silently served.

mod common;

use std::path::Path;
use std::process::Command;

use common::{bare_remote, committable_set, fixture_repo, git, index};
use cgx_transport::TransportError;

#[test]
fn gc_prune_on_remote_keeps_refs_cgx_index_and_pull_still_works() {
    let (_tmp_a, repo_a) = fixture_repo();
    index(&repo_a);

    let (_tmp_remote, remote) = bare_remote();
    git(&repo_a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    cgx_transport::push(&repo_a, "origin").expect("push");

    // gc --prune=now on the bare remote itself: the object closure under
    // refs/cgx/index must be reachable enough to survive.
    git(&remote, &["gc", "--prune=now"]);
    let fsck = git(&remote, &["fsck", "--full"]);
    assert!(fsck.trim().is_empty(), "git fsck must be clean after gc: {fsck}");

    let expected = committable_set(&repo_a);
    let (_tmp_b, repo_b) = clone_bare(&remote);
    cgx_transport::configure_remote(&repo_b, "origin").expect("configure-remote");
    cgx_transport::pull(&repo_b, "origin").expect("pull must still succeed after remote gc");
    assert_eq!(
        expected,
        committable_set(&repo_b),
        "pull after gc must still materialize the full committable set"
    );
}

#[test]
fn gc_prune_on_local_repo_keeps_refs_cgx_index() {
    // The local pusher's own repo also holds refs/cgx/index (advance_ref writes it
    // before pushing) — a local gc must not prune it either.
    let (_tmp, repo) = fixture_repo();
    index(&repo);

    let (_tmp_remote, remote) = bare_remote();
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let outcome = cgx_transport::push(&repo, "origin").expect("push");

    git(&repo, &["gc", "--prune=now"]);
    let rev = git(&repo, &["rev-parse", "refs/cgx/index"]);
    assert_eq!(rev, outcome.commit_oid, "refs/cgx/index must survive a local git gc");
}

#[test]
fn pull_errors_on_tampered_object_bytes_never_serves_silently() {
    let (_tmp_a, repo_a) = fixture_repo();
    index(&repo_a);

    let (_tmp_remote, remote) = bare_remote();
    git(&repo_a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    cgx_transport::push(&repo_a, "origin").expect("push");

    // Tamper one blob's bytes IN THE REMOTE's object DB directly: rewrite the
    // git blob content at the git-blob-oid content-address so `cat-file blob`
    // returns different bytes than what was promoted, while the tree entry
    // (which names the ORIGINAL cgx OID) is untouched — this is exactly the
    // "corrupted/missing object" scenario criterion 5 requires pull to reject.
    let blob_oid = first_blob_oid_under_objects(&remote, "refs/cgx/index");
    corrupt_loose_object(&remote, &blob_oid);

    let (_tmp_b, repo_b) = clone_bare(&remote);
    cgx_transport::configure_remote(&repo_b, "origin").expect("configure-remote");
    let err = cgx_transport::pull(&repo_b, "origin")
        .expect_err("pull must reject a tampered object instead of serving it");
    match err {
        TransportError::Git { .. } => {
            // git's own hash-consistency check on `cat-file`/`fetch` can also catch
            // this (git blobs are themselves content-addressed) — either failure
            // mode satisfies "never served silently".
        }
        TransportError::Integrity { .. } => {}
        other => panic!("expected Git or Integrity error on tampered object, got {other:?}"),
    }
}

/// The git blob OID of the first entry under `objects/` in `refname`'s tree.
fn first_blob_oid_under_objects(remote: &Path, refname: &str) -> String {
    let listing = git(remote, &["ls-tree", "-r", refname]);
    for line in listing.lines() {
        // "<mode> blob <oid>\t<path>"
        if let Some((meta, path)) = line.split_once('\t') {
            if path.starts_with("objects/") {
                return meta.split_whitespace().nth(2).unwrap().to_owned();
            }
        }
    }
    panic!("no objects/ blob found under {refname}");
}

/// Directly overwrite a loose git object's compressed bytes with garbage, at its
/// own on-disk shard path in a **bare** repo — corrupting the object without
/// touching the tree that still names its (now-stale) OID.
fn corrupt_loose_object(remote: &Path, blob_oid: &str) {
    let path = remote.join("objects").join(&blob_oid[..2]).join(&blob_oid[2..]);
    assert!(path.exists(), "loose object must exist on disk at {path:?}");
    // git writes loose objects read-only (mode 0444); regain write permission
    // before overwriting the bytes.
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o644);
    }
    std::fs::set_permissions(&path, perms).expect("chmod loose object writable");
    std::fs::write(&path, b"not a valid git object").expect("overwrite loose object");
}

fn clone_bare(remote: &Path) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dst = tmp.path().join("clone");
    let status = Command::new("git")
        .args(["clone", "-q"])
        .arg(remote)
        .arg(&dst)
        .status()
        .expect("git clone");
    assert!(status.success(), "git clone failed");
    (tmp, dst)
}
