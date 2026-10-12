//! Module-path (dotted-import prefix) derivation from a repo-relative `.py` path.
//!
//! Unlike Go (a package per directory), Python maps each `.py` file to its own
//! module: `pkg/sub/mod.py` is importable as `pkg.sub.mod`, so the file **stem
//! contributes a segment** (the cgx prefix uses `::`, not `.`). The package
//! marker file `__init__.py` is the directory's package object, so its stem is
//! dropped and the prefix is the directory path.
//!
//! ## Import root (canonical root = importable dotted path)
//!
//! A module path is only canonical when segmented below the correct **import
//! root** — the directory Python treats as the top of the import namespace. A
//! `src/`-layout package `src/pkg/mod.py` is importable as `pkg.mod`, not
//! `src.pkg.mod`, so rooting at the literal `src` segment both mis-names the
//! module and collides every `src/`-layout package at the shared `src` root
//! (`findings/01-adapter-root-audit.md:90-94`).
//!
//! The import root is resolved pipeline-side (it needs sibling files — the
//! pyproject/setup.py location and whether a `src/` subdir exists) and handed in
//! as [`ManifestInfo::root_dir`](cgx_frontend::ManifestInfo::root_dir). A Python
//! pyproject declares no importable identity string (the *project* name is the
//! distribution name, not part of the import path), so the manifest gives only
//! an **anchor**: the path is segmented *below* it.
//!
//! When no pyproject/setup.py is an ancestor (`import_root` is `None`) we fall
//! back to repo-root-relative segmentation with a **leading-`src/` strip**
//! heuristic: today's behaviour minus the `src`-as-root bug. Only a single
//! leading `src/` is stripped — an inner `a/src/b.py` keeps its `src` segment,
//! because without a manifest we cannot know it is an import root.

/// Default module name when the path is at the import root with no usable stem.
const DEFAULT_MODULE: &str = "py_sample";

/// `::`-joined module prefix for a repo-relative Python file, segmented from the
/// repo root (no import-root anchor). Thin wrapper over
/// [`module_path_for_root`] with `import_root = None`: applies the leading-`src/`
/// strip heuristic. Retained for callers/tests that need the no-manifest path.
pub fn module_path_for(rel_path: &str) -> String {
    module_path_for_root(rel_path, None)
}

/// `::`-joined module prefix for a repo-relative Python file, segmented **below
/// the import root**.
///
/// `import_root` is the src-aware import-root anchor resolved from the
/// nearest-ancestor pyproject/setup.py ([`ManifestInfo::root_dir`]): a directory
/// path (`src`, `pkg/src`) or the empty string (pyproject at the repo root, flat
/// layout). When `Some`, the anchor is stripped as a path prefix and the file is
/// segmented below it — trusting the manifest, applying no further heuristic.
/// When `None` (no Python manifest is an ancestor), falls back to repo-root
/// segmentation with a single leading-`src/` strip.
///
/// Directory segments plus the file stem are `::`-joined; `__init__.py` drops its
/// stem (the package is the directory). When no usable segment remains (the file
/// *is* the import root's `__init__.py`), falls back to [`DEFAULT_MODULE`].
///
/// [`ManifestInfo::root_dir`]: cgx_frontend::ManifestInfo::root_dir
pub fn module_path_for_root(rel_path: &str, import_root: Option<&str>) -> String {
    let norm = rel_path.replace('\\', "/");
    let below = match import_root {
        Some(root) => strip_import_root(&norm, root),
        None => strip_leading_src(&norm),
    };
    segment(below)
}

/// Strip the import-root anchor from `path`, returning the path below it. An
/// empty anchor (pyproject at the repo root) is a no-op. The anchor must match on
/// a path-segment boundary (`src` strips `src/pkg` but not `srcpkg`); a
/// non-ancestor anchor leaves `path` untouched (defensive — the pipeline only
/// hands an ancestor).
fn strip_import_root<'a>(path: &'a str, root: &str) -> &'a str {
    if root.is_empty() {
        return path;
    }
    path.strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path)
}

/// The no-manifest fallback: strip a single leading `src/` segment. An inner
/// `src/` (`a/src/b.py`) is preserved — without a manifest we cannot prove it is
/// an import root.
fn strip_leading_src(path: &str) -> &str {
    path.strip_prefix("src/").unwrap_or(path)
}

/// Segment a `/`-path into the `::`-joined module prefix: directory segments plus
/// the file stem, with `__init__.py` dropping its stem. Empty result ⇒
/// [`DEFAULT_MODULE`].
fn segment(path: &str) -> String {
    let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
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

    // --- no-manifest fallback (import_root = None): flat layout unchanged ---

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

    // --- src-layout collision resolves below the import-root anchor ---

    #[test]
    fn src_layout_roots_below_src_not_at_it() {
        // The collision fix: `src/pkg/mod.py` is importable as `pkg.mod`, so it
        // roots at `pkg`, never the literal `src`.
        assert_eq!(module_path_for_root("src/pkg/mod.py", Some("src")), "pkg::mod");
    }

    #[test]
    fn two_src_layout_packages_do_not_collide() {
        // Both used to root at the shared `src`; now each roots at its own top
        // package — distinct, neither rooted at `src`.
        let alpha = module_path_for_root("src/alpha/x.py", Some("src"));
        let beta = module_path_for_root("src/beta/y.py", Some("src"));
        assert_eq!(alpha, "alpha::x");
        assert_eq!(beta, "beta::y");
        assert_ne!(alpha, beta);
        assert!(!alpha.starts_with("src"));
        assert!(!beta.starts_with("src"));
    }

    #[test]
    fn namespace_package_no_init_segments_normally() {
        // PEP 420 namespace packages have no `__init__.py`; directories still
        // become segments with no `__init__` requirement.
        assert_eq!(
            module_path_for_root("src/ns/leaf/mod.py", Some("src")),
            "ns::leaf::mod"
        );
    }

    #[test]
    fn init_stem_drop_preserved_below_anchor() {
        assert_eq!(
            module_path_for_root("src/pkg/sub/__init__.py", Some("src")),
            "pkg::sub"
        );
    }

    // --- pinned anchor-determination edge cases (the riskiest part) ---

    #[test]
    fn empty_anchor_is_flat_layout_no_strip() {
        // pyproject at the repo root with no `src/` subdir: PR-A resolves
        // root_dir = "". The anchor is authoritative — no leading-`src/`
        // heuristic is applied on top of a present manifest.
        assert_eq!(module_path_for_root("acme/__init__.py", Some("")), "acme");
        assert_eq!(module_path_for_root("acme/mod.py", Some("")), "acme::mod");
    }

    #[test]
    fn nested_manifest_anchor_strips_full_prefix() {
        // A nested pyproject/setup.py src-layout: PR-A resolves root_dir =
        // "pkg/src"; the whole anchor is stripped, not just a leading `src`.
        assert_eq!(
            module_path_for_root("pkg/src/mod/thing.py", Some("pkg/src")),
            "mod::thing"
        );
    }

    #[test]
    fn inner_src_is_not_an_import_root_with_anchor() {
        // Multiple `src`-like dirs: only the declared anchor is the import root.
        // An inner `src` below it stays an ordinary segment.
        assert_eq!(
            module_path_for_root("src/a/src/b.py", Some("src")),
            "a::src::b"
        );
    }

    #[test]
    fn inner_src_is_not_an_import_root_no_manifest() {
        // No manifest: only a single *leading* `src/` is stripped; an inner
        // `src` is preserved (we cannot prove it is an import root).
        assert_eq!(module_path_for_root("src/a/src/b.py", None), "a::src::b");
        assert_eq!(module_path_for("pkg/sub/mod.py"), "pkg::sub::mod");
    }

    #[test]
    fn leading_src_strip_heuristic_no_manifest() {
        // The no-pyproject fallback: strip the `src`-as-root bug but keep
        // segmenting from the repo root otherwise.
        assert_eq!(module_path_for_root("src/pkg/mod.py", None), "pkg::mod");
    }

    #[test]
    fn non_ancestor_anchor_leaves_path_untouched() {
        // Defensive: the pipeline only hands an ancestor anchor, but a prefix
        // that is not a path-segment ancestor must not partially strip.
        assert_eq!(module_path_for_root("other/x.py", Some("src")), "other::x");
        assert_eq!(module_path_for_root("srcfoo/x.py", Some("src")), "srcfoo::x");
    }

    #[test]
    fn init_at_the_import_root_falls_back_to_default() {
        // `src/__init__.py` with anchor `src` leaves no usable segment.
        assert_eq!(module_path_for_root("src/__init__.py", Some("src")), "py_sample");
    }
}
