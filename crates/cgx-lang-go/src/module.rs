//! FQN-root derivation from a repo-relative `.go` path.
//!
//! Go packages map to directories: every file in a directory belongs to the
//! same package, so the file stem contributes no FQN segment (unlike Rust).
//!
//! The canonical root is the **import path** = the owning `go.mod` module path
//! joined with the package subdirectory by `/`, matching govulncheck/OSV
//! `imports[].path` byte-for-byte (e.g. `example.com/app/internal/store`). It is
//! computed by [`import_path_root`] from the pipeline-resolved manifest facts.
//! The symbol is appended to that root by the caller with `::`
//! (`example.com/app/internal/store::Open`).
//!
//! When no `go.mod` is in scope, there is no module path to be canonical to, so
//! the fallback [`module_path_for_pkg`] uses the non-canonical, best-effort
//! leaf-directory basename as the root — see its note.

/// Default package name when none is supplied and the path is at the root.
const DEFAULT_PKG: &str = "go_sample";

/// The import-path root for a Go file when the owning `go.mod` module path is
/// known: `module_path` joined with the package subdirectory — the directory
/// segments between the module's `go.mod` dir (`root_dir`, the nearest-ancestor
/// boundary) and the file's own directory — by `/`. The result is the
/// contiguous import path (`example.com/app/internal/store`); the symbol is
/// appended by the caller with `::`.
///
/// `root_dir` is the repo-relative directory of the owning `go.mod` (empty at
/// the repo root) and is always an ancestor of the file, so its segments are a
/// prefix of the file's directory segments. A file sitting directly in the
/// module root yields the bare `module_path`.
pub fn import_path_root(rel_path: &str, module_path: &str, root_dir: &str) -> String {
    let file_dirs = dir_segments(rel_path);
    let root_len = root_dir
        .replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty())
        .count();

    let mut out = module_path.to_string();
    for seg in file_dirs.iter().skip(root_len) {
        out.push('/');
        out.push_str(seg);
    }
    out
}

/// `::`-joined fallback root for a repo-relative Go file with no owning-module
/// knowledge: the leaf directory name (or [`DEFAULT_PKG`] at the root),
/// followed by the directory segments.
pub fn module_path_for(rel_path: &str) -> String {
    module_path_for_pkg(rel_path, None)
}

/// Non-canonical, best-effort fallback root used only when no `go.mod` is in
/// scope. `package` is the incidental pipeline `FileCtx::package` value (the
/// resolved Cargo `[package] name`, almost always `None` in a Go repo); when
/// present it is used as the root segment, otherwise the leaf directory name (or
/// [`DEFAULT_PKG`] at the root). This is non-unique (`internal/store` and
/// `pkg/store` both collapse to `store`) and consults no manifest — the
/// canonical import-path root is [`import_path_root`], used whenever a `go.mod`
/// module path is resolved.
pub fn module_path_for_pkg(rel_path: &str, package: Option<&str>) -> String {
    let dirs = dir_segments(rel_path);

    match package {
        Some(pkg) => {
            let mut out = pkg.to_string();
            for d in &dirs {
                out.push_str("::");
                out.push_str(d);
            }
            out
        }
        None => {
            // No module known: the leaf dir is the Go package short-name (Go
            // package names match their directory), with no module root to
            // prefix its ancestors; at the repo root use the default.
            match dirs.last() {
                Some(leaf) => leaf.to_string(),
                None => DEFAULT_PKG.to_string(),
            }
        }
    }
}

/// The directory segments of a repo-relative path, file name dropped (in Go the
/// file stem never contributes an FQN segment — the package is the directory).
/// Backslashes are normalized to `/` so a Windows-style path segments the same.
fn dir_segments(rel_path: &str) -> Vec<String> {
    let norm = rel_path.replace('\\', "/");
    let mut segments: Vec<String> = norm
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    segments.pop();
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_in_package_dir_uses_dir_path_not_stem() {
        // All files in a dir share the package; the stem never adds a segment.
        assert_eq!(
            module_path_for_pkg("internal/store/db.go", Some("app")),
            "app::internal::store"
        );
        assert_eq!(
            module_path_for_pkg("internal/store/query.go", Some("app")),
            "app::internal::store"
        );
    }

    #[test]
    fn root_file_is_just_the_package() {
        assert_eq!(module_path_for_pkg("main.go", Some("app")), "app");
    }

    #[test]
    fn without_package_falls_back_to_leaf_dir() {
        assert_eq!(module_path_for("internal/store/db.go"), "store");
        assert_eq!(module_path_for("main.go"), "go_sample");
    }

    // --- import-path root (canonical, go.mod module path known) ---

    #[test]
    fn import_path_collision_resolves_to_distinct_roots() {
        // The leaf-dir fallback collapses both of these to `store`; the
        // import-path root keeps them distinct.
        let a = import_path_root("internal/store/db.go", "example.com/app", "");
        let b = import_path_root("pkg/store/db.go", "example.com/app", "");
        assert_eq!(a, "example.com/app/internal/store");
        assert_eq!(b, "example.com/app/pkg/store");
        assert_ne!(a, b);
        // The root is the contiguous import path: `/` between components, no `::`.
        assert!(a.contains('/'));
        assert!(!a.contains("::"));
    }

    #[test]
    fn import_path_nested_module_offsets_from_go_mod_dir() {
        // A nested `svc/go.mod` (module example.com/svc) roots a file at
        // `svc/store/db.go` from the `svc/` boundary, not the repo root.
        assert_eq!(
            import_path_root("svc/store/db.go", "example.com/svc", "svc"),
            "example.com/svc/store"
        );
    }

    #[test]
    fn import_path_root_file_is_bare_module_path() {
        // `main.go` directly in the module root → the bare module path.
        assert_eq!(
            import_path_root("main.go", "example.com/app", ""),
            "example.com/app"
        );
        // Same when the module itself is in a subdir and the file sits in it.
        assert_eq!(
            import_path_root("svc/main.go", "example.com/svc", "svc"),
            "example.com/svc"
        );
    }

    #[test]
    fn import_path_main_package_and_internal_need_no_special_casing() {
        assert_eq!(
            import_path_root("cmd/foo/main.go", "example.com/app", ""),
            "example.com/app/cmd/foo"
        );
        assert_eq!(
            import_path_root("internal/store/db.go", "example.com/app", ""),
            "example.com/app/internal/store"
        );
    }
}
