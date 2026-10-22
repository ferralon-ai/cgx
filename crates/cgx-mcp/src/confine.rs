//! Filesystem confinement: keep every tool call inside the configured root.
//!
//! When the server is started with `cgx mcp --root <path>`, that root is not a
//! mere default — it is a *confinement boundary*. A client or agent may still
//! pass an explicit `root` per call (and omitting it still gets the configured
//! default; see [`crate::server`]), but the effective root of every call must
//! resolve to the boundary itself or a path beneath it. Anything else — an
//! absolute path elsewhere on the host, a `..` traversal, or a symlink inside
//! the boundary that points out of it — is rejected before any indexing runs.
//!
//! The boundary is canonicalized **once at startup** (see
//! [`crate::server::ServerConfig::new`]); [`confine_root`] is given that already
//! canonical path and only ever canonicalizes the *requested* path per call.
//! The descendant test is [`Path::starts_with`] on canonical forms — component
//! wise, not a string prefix — so a sibling whose name merely shares a prefix
//! (`/srv/repo-evil` against a `/srv/repo` boundary) is correctly outside.

use std::path::{Path, PathBuf};

/// Why a requested root was not admitted inside the confinement boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfineError {
    /// The requested path could not be canonicalized — it does not exist or is
    /// inaccessible. A non-existent path cannot be confined, so it is rejected.
    Unresolvable { requested: String, reason: String },
    /// The requested path canonicalized to somewhere outside the boundary.
    Escapes { boundary: String },
}

impl std::fmt::Display for ConfineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfineError::Unresolvable { requested, reason } => write!(
                f,
                "requested root `{requested}` cannot be resolved: {reason}"
            ),
            ConfineError::Escapes { boundary } => write!(
                f,
                "requested root is outside the configured --root boundary ({boundary})"
            ),
        }
    }
}

impl std::error::Error for ConfineError {}

/// Confine `requested` to `boundary`, returning the canonical requested path on
/// success.
///
/// `boundary` **must already be canonical** — the server canonicalizes it once
/// at startup. `requested` is canonicalized here (resolving symlinks and `..`),
/// which is what makes a symlink inside the boundary that points out of it an
/// escape rather than a false pass. The result is the canonical path, suitable
/// to hand straight to the indexer.
pub fn confine_root(boundary: &Path, requested: &Path) -> Result<PathBuf, ConfineError> {
    let canonical = requested
        .canonicalize()
        .map_err(|e| ConfineError::Unresolvable {
            requested: requested.display().to_string(),
            reason: e.to_string(),
        })?;
    if canonical.starts_with(boundary) {
        Ok(canonical)
    } else {
        Err(ConfineError::Escapes {
            boundary: boundary.display().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonicalize like startup does, so the test boundary matches the contract
    /// `confine_root` is given (and so macOS `/var` → `/private/var` symlinks do
    /// not spuriously fail the prefix test).
    fn canon(p: &Path) -> PathBuf {
        p.canonicalize().expect("canonicalize fixture path")
    }

    #[test]
    fn root_equal_to_boundary_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let boundary = canon(tmp.path());
        let got = confine_root(&boundary, &boundary).expect("boundary itself is inside");
        assert_eq!(got, boundary);
    }

    #[test]
    fn descendant_of_boundary_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let boundary = canon(tmp.path());
        let child = boundary.join("sub/dir");
        std::fs::create_dir_all(&child).unwrap();
        let got = confine_root(&boundary, &child).expect("descendant is inside");
        assert_eq!(got, canon(&child));
    }

    #[test]
    fn absolute_path_outside_boundary_is_rejected() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let boundary = canon(inside.path());
        let err = confine_root(&boundary, &canon(outside.path())).unwrap_err();
        assert!(matches!(err, ConfineError::Escapes { .. }), "got {err:?}");
    }

    #[test]
    fn dotdot_traversal_escaping_boundary_is_rejected() {
        // boundary = <tmp>/repo; requested = <tmp>/repo/../sibling, which
        // canonicalizes to <tmp>/sibling — outside the boundary.
        let tmp = tempfile::tempdir().unwrap();
        let boundary = tmp.path().join("repo");
        let sibling = tmp.path().join("sibling");
        std::fs::create_dir_all(&boundary).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let boundary = canon(&boundary);
        let escaping = boundary.join("..").join("sibling");
        let err = confine_root(&boundary, &escaping).unwrap_err();
        assert!(matches!(err, ConfineError::Escapes { .. }), "got {err:?}");
    }

    #[test]
    fn sibling_sharing_a_name_prefix_is_rejected() {
        // The component-wise check: `/x/repo-evil` is NOT inside `/x/repo`.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let evil = tmp.path().join("repo-evil");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&evil).unwrap();
        let boundary = canon(&repo);
        let err = confine_root(&boundary, &canon(&evil)).unwrap_err();
        assert!(
            matches!(err, ConfineError::Escapes { .. }),
            "a name-prefix sibling must not be considered inside: {err:?}"
        );
    }

    #[test]
    fn nonexistent_requested_path_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let boundary = canon(tmp.path());
        let missing = boundary.join("does/not/exist");
        let err = confine_root(&boundary, &missing).unwrap_err();
        assert!(matches!(err, ConfineError::Unresolvable { .. }), "got {err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_boundary_pointing_outside_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let boundary = tmp.path().join("repo");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&boundary).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let boundary = canon(&boundary);
        let link = boundary.join("escape");
        std::os::unix::fs::symlink(canon(&outside), &link).unwrap();
        // The link path lexically starts with the boundary, but canonicalizing
        // resolves it to `outside` — so confinement catches it.
        let err = confine_root(&boundary, &link).unwrap_err();
        assert!(matches!(err, ConfineError::Escapes { .. }), "got {err:?}");
    }
}
