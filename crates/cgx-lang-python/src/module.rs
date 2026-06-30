//! Module-path (dotted-import prefix) derivation from a repo-relative `.py` path.
//!
//! Unlike Go (a package per directory), Python maps each `.py` file to its own
//! module: `pkg/sub/mod.py` is importable as `pkg.sub.mod`, so the file **stem
//! contributes a segment** (the cgx prefix uses `::`, not `.`). The package
//! marker file `__init__.py` is the directory's package object, so its stem is
//! dropped and the prefix is the directory path.

/// Default module name when the path is at the repo root with no stem.
const DEFAULT_MODULE: &str = "py_sample";

/// `::`-joined module prefix for a repo-relative Python file. Directory segments
/// plus the file stem are joined; `__init__.py` drops the stem (the package is
/// the directory). At the root with no usable stem, falls back to
/// [`DEFAULT_MODULE`].
pub fn module_path_for(rel_path: &str) -> String {
    let norm = rel_path.replace('\\', "/");
    let mut segments: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    let Some(file) = segments.pop() else {
        return DEFAULT_MODULE.to_string();
    };
    let stem = file.strip_suffix(".py").unwrap_or(file);

    // `__init__.py` is the package object for its directory: no stem segment.
    if stem != "__init__" {
        segments.push(stem);
    }

    if segments.is_empty() {
        DEFAULT_MODULE.to_string()
    } else {
        segments.join("::")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stem_contributes_a_segment() {
        assert_eq!(module_path_for("pkg/sub/mod.py"), "pkg::sub::mod");
        assert_eq!(module_path_for("app.py"), "app");
    }

    #[test]
    fn init_drops_the_stem_to_the_package_dir() {
        assert_eq!(module_path_for("pkg/sub/__init__.py"), "pkg::sub");
        assert_eq!(module_path_for("pkg/__init__.py"), "pkg");
    }

    #[test]
    fn root_init_falls_back_to_default() {
        assert_eq!(module_path_for("__init__.py"), "py_sample");
    }
}
