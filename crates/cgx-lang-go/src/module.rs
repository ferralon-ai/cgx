//! Module-path (package-prefix) derivation from a repo-relative `.go` path.
//!
//! Go packages map to directories: every file in a directory belongs to the
//! same package, so the file stem contributes no FQN segment (unlike Rust). The
//! prefix is the owning module/package name followed by the file's directory
//! segments, `::`-joined.

/// Default package name when none is supplied and the path is at the root.
const DEFAULT_PKG: &str = "go_sample";

/// `::`-joined package prefix for a repo-relative Go file, with no owning-module
/// knowledge: the leaf directory name (or [`DEFAULT_PKG`] at the root) is the
/// crate root, followed by the directory segments.
pub fn module_path_for(rel_path: &str) -> String {
    module_path_for_pkg(rel_path, None)
}

/// `::`-joined package prefix, preferring `package` (the owning go.mod module's
/// short name) as the root segment when supplied; otherwise the leaf directory
/// name (or [`DEFAULT_PKG`] at the root).
pub fn module_path_for_pkg(rel_path: &str, package: Option<&str>) -> String {
    let norm = rel_path.replace('\\', "/");
    let mut segments: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    // Drop the file name; in Go the stem never contributes a segment.
    segments.pop();
    let dirs = segments; // directory segments only

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
}
