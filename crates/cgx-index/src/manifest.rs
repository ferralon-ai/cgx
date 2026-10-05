//! Pipeline-side resolution of non-Cargo build manifests (`go.mod`,
//! `package.json`, `pyproject.toml` / `setup.py`) into [`ManifestInfo`].
//!
//! A language adapter's [`extract`](cgx_frontend::LanguageFrontend::extract) is
//! per-file and isolated — it never sees sibling files, so it cannot read its
//! own `go.mod` / `package.json` / `pyproject.toml`. Only the pipeline sees the
//! whole source set. This module mirrors [`crate::cargo_pkg::PackageMap`]: it
//! scans every manifest blob once and answers a nearest-ancestor
//! (longest-prefix) query per file, reusing [`cargo_pkg`](crate::cargo_pkg)'s
//! `parent_dir` / `is_dir_ancestor` so the ownership rule is identical.
//!
//! Scope: this map resolves the three *new* manifest kinds that the
//! [`FileCtx::manifest`](cgx_frontend::FileCtx::manifest) carrier exists for.
//! Cargo is deliberately *not* emitted here — Rust's crate root is already
//! resolved by [`PackageMap`](crate::cargo_pkg::PackageMap) and read off the
//! legacy `package` field. [`ManifestKind::Cargo`] remains part of the type's
//! domain for the complete design model.
//!
//! ## Zero-/existing-dep parsing
//!
//! - **go.mod**: a line scan for the `module <path>` directive (cargo_pkg style).
//! - **package.json**: tree-sitter-json (already a cgx-index dependency; a line
//!   scan of JSON is fragile and `serde_json` is not a dependency) reads the
//!   top-level `"name"`, preserving a scoped `@scope/pkg` name verbatim.
//! - **pyproject/setup.py**: no identity string (the importable path is
//!   filesystem-derived); the resolved `root_dir` is the src-aware import root.

use std::collections::BTreeMap;

use cgx_frontend::{ManifestInfo, ManifestKind};
use tree_sitter::{Node, Parser};

use crate::cargo_pkg::{is_dir_ancestor, parent_dir};
use crate::git::SourceFile;

/// Maps each source file to the nearest-ancestor non-Cargo manifest's resolved
/// facts. Built once per index run.
#[derive(Debug, Default)]
pub(crate) struct ManifestMap {
    /// `manifest-dir → resolved facts`. The key is the `/`-separated
    /// repo-relative directory containing the owning manifest (the boundary used
    /// for the longest-prefix query); the repo root is the empty string.
    by_dir: BTreeMap<String, ManifestInfo>,
}

impl ManifestMap {
    /// Build the map from the full enumerated source set, parsing every resolved
    /// manifest blob. Sources are processed in a path-sorted order so a dir that
    /// holds more than one manifest kind resolves deterministically (first by
    /// sorted path wins).
    pub(crate) fn from_sources(sources: &[SourceFile]) -> ManifestMap {
        let dirs: Vec<&str> = sources.iter().map(|s| s.rel_path.as_str()).collect();

        let mut ordered: Vec<&SourceFile> = sources.iter().collect();
        ordered.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

        let mut by_dir: BTreeMap<String, ManifestInfo> = BTreeMap::new();
        for src in ordered {
            let dir = parent_dir(&src.rel_path);
            if by_dir.contains_key(dir) {
                continue;
            }
            if let Some(info) = resolve(&src.rel_path, &src.content, dir, &dirs) {
                by_dir.insert(dir.to_string(), info);
            }
        }
        ManifestMap { by_dir }
    }

    /// The nearest-ancestor manifest facts for `rel_path`, if any resolved
    /// manifest is an ancestor. Returns the *nearest* (longest matching directory
    /// prefix), so a nested module wins over an outer one — the same rule as
    /// [`PackageMap::package_for`](crate::cargo_pkg::PackageMap::package_for).
    ///
    /// The returned manifest is the nearest of *any* resolved kind; a consumer
    /// that cares about a specific language filters on
    /// [`ManifestInfo::kind`](cgx_frontend::ManifestInfo::kind).
    pub(crate) fn manifest_for(&self, rel_path: &str) -> Option<ManifestInfo> {
        let file_dir = parent_dir(rel_path);
        let mut best: Option<(&str, &ManifestInfo)> = None;
        for (dir, info) in &self.by_dir {
            if is_dir_ancestor(dir, file_dir) {
                match best {
                    Some((best_dir, _)) if best_dir.len() >= dir.len() => {}
                    _ => best = Some((dir.as_str(), info)),
                }
            }
        }
        best.map(|(_, info)| info.clone())
    }
}

/// Resolve one manifest file to its facts, or `None` if `path` is not a resolved
/// manifest or the manifest declares nothing usable. `dir` is the manifest's
/// parent dir; `all_paths` is the full source set (needed for the Python
/// src-layout probe).
fn resolve(path: &str, content: &[u8], dir: &str, all_paths: &[&str]) -> Option<ManifestInfo> {
    if is_named(path, "go.mod") {
        let text = std::str::from_utf8(content).ok()?;
        let identity = parse_go_module(text)?;
        return Some(ManifestInfo {
            kind: ManifestKind::GoMod,
            identity: Some(identity),
            root_dir: dir.to_string(),
        });
    }
    if is_named(path, "package.json") {
        let identity = parse_package_json_name(content)?;
        return Some(ManifestInfo {
            kind: ManifestKind::PackageJson,
            identity: Some(identity),
            root_dir: dir.to_string(),
        });
    }
    if is_named(path, "pyproject.toml") || is_named(path, "setup.py") {
        return Some(ManifestInfo {
            kind: ManifestKind::PyProject,
            identity: None,
            root_dir: python_import_root(dir, all_paths),
        });
    }
    None
}

/// Whether `path`'s final segment is exactly `name` (the manifest at any depth).
fn is_named(path: &str, name: &str) -> bool {
    path == name || path.ends_with(&format!("/{name}"))
}

/// The src-aware Python import root: the pyproject/setup.py dir plus `/src` when
/// a `src/` subdir exists in the source set (src-layout anchor), else the dir
/// itself. This is the accepted v1 heuristic (`[tool.setuptools] package-dir` is
/// not honoured).
fn python_import_root(dir: &str, all_paths: &[&str]) -> String {
    let src_prefix = if dir.is_empty() {
        "src/".to_string()
    } else {
        format!("{dir}/src/")
    };
    if all_paths.iter().any(|p| p.starts_with(&src_prefix)) {
        src_prefix.trim_end_matches('/').to_string()
    } else {
        dir.to_string()
    }
}

/// Extract the `module <path>` directive from a `go.mod`'s text with a minimal
/// line scan. Strips a `//` line comment and an optional surrounding quote on the
/// path. Returns `None` when there is no `module` directive.
fn parse_go_module(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.split("//").next().unwrap_or(raw).trim();
        let Some(rest) = line.strip_prefix("module") else {
            continue;
        };
        if !rest.starts_with(|c: char| c.is_whitespace()) {
            continue;
        }
        let path = rest.trim().trim_matches('"');
        if !path.is_empty() {
            return Some(path.to_string());
        }
    }
    None
}

/// Read the top-level `"name"` string from a `package.json` via tree-sitter-json,
/// preserving a scoped `@scope/pkg` name verbatim. Returns `None` when parsing
/// fails or there is no top-level `"name"`.
fn parse_package_json_name(content: &[u8]) -> Option<String> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_json::LANGUAGE.into()).ok()?;
    let tree = parser.parse(content, None)?;

    let root = tree.root_node();
    let mut root_cursor = root.walk();
    let object = root
        .named_children(&mut root_cursor)
        .find(|n| n.kind() == "object")?;

    let mut obj_cursor = object.walk();
    for pair in object.named_children(&mut obj_cursor) {
        if pair.kind() != "pair" {
            continue;
        }
        let key = pair.child_by_field_name("key")?;
        if json_string_value(key, content).as_deref() == Some("name") {
            let value = pair.child_by_field_name("value")?;
            return json_string_value(value, content);
        }
    }
    None
}

/// The decoded content of a JSON `string` node (without its surrounding quotes),
/// or `None` if `node` is not a string.
fn json_string_value(node: Node, src: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "string_content" {
            return child.utf8_text(src).ok().map(str::to_string);
        }
    }
    // A well-formed empty string `""` has no `string_content` child.
    Some(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(path: &str, content: &str) -> SourceFile {
        SourceFile {
            blob_oid: "oid".to_string(),
            rel_path: path.to_string(),
            content: content.as_bytes().to_vec(),
        }
    }

    #[test]
    fn go_module_directive_extracted() {
        assert_eq!(
            parse_go_module("module example.com/app\n\ngo 1.21\n").as_deref(),
            Some("example.com/app")
        );
    }

    #[test]
    fn go_module_strips_comment_and_quotes() {
        assert_eq!(
            parse_go_module("module \"example.com/app\" // the module\n").as_deref(),
            Some("example.com/app")
        );
    }

    #[test]
    fn go_module_absent_yields_none() {
        assert_eq!(parse_go_module("go 1.21\n\nrequire foo v1.0.0\n"), None);
    }

    #[test]
    fn go_nested_module_nearest_ancestor_wins() {
        let map = ManifestMap::from_sources(&[
            src("go.mod", "module example.com/app\n"),
            src("svc/go.mod", "module example.com/svc\n"),
            src("svc/store/db.go", ""),
            src("cmd/main.go", ""),
        ]);
        assert_eq!(
            map.manifest_for("svc/store/db.go").unwrap().identity.as_deref(),
            Some("example.com/svc")
        );
        assert_eq!(
            map.manifest_for("cmd/main.go").unwrap().identity.as_deref(),
            Some("example.com/app")
        );
    }

    #[test]
    fn package_json_name_read() {
        assert_eq!(
            parse_package_json_name(br#"{"name": "acme", "version": "1.0.0"}"#).as_deref(),
            Some("acme")
        );
    }

    #[test]
    fn package_json_scoped_name_preserved_verbatim() {
        assert_eq!(
            parse_package_json_name(br#"{"name": "@acme/utils"}"#).as_deref(),
            Some("@acme/utils")
        );
    }

    #[test]
    fn package_json_name_not_first_key() {
        assert_eq!(
            parse_package_json_name(br#"{"version": "1.0.0", "private": true, "name": "late"}"#)
                .as_deref(),
            Some("late")
        );
    }

    #[test]
    fn package_json_no_name_yields_none() {
        assert_eq!(parse_package_json_name(br#"{"version": "1.0.0"}"#), None);
    }

    #[test]
    fn package_json_nested_name_ignored() {
        // A `name` under a nested object must not be read as the package name.
        assert_eq!(
            parse_package_json_name(br#"{"repository": {"name": "nested"}, "version": "1.0.0"}"#),
            None
        );
    }

    #[test]
    fn pyproject_src_layout_roots_at_src() {
        let map = ManifestMap::from_sources(&[
            src("pyproject.toml", "[project]\nname = \"acme\"\n"),
            src("src/acme/__init__.py", ""),
        ]);
        let info = map.manifest_for("src/acme/__init__.py").unwrap();
        assert_eq!(info.kind, ManifestKind::PyProject);
        assert_eq!(info.identity, None);
        assert_eq!(info.root_dir, "src");
    }

    #[test]
    fn pyproject_flat_layout_roots_at_manifest_dir() {
        let map = ManifestMap::from_sources(&[
            src("pyproject.toml", "[project]\nname = \"acme\"\n"),
            src("acme/__init__.py", ""),
        ]);
        assert_eq!(map.manifest_for("acme/__init__.py").unwrap().root_dir, "");
    }

    #[test]
    fn setup_py_nested_src_layout() {
        let map = ManifestMap::from_sources(&[
            src("pkg/setup.py", ""),
            src("pkg/src/mod/__init__.py", ""),
        ]);
        assert_eq!(
            map.manifest_for("pkg/src/mod/__init__.py").unwrap().root_dir,
            "pkg/src"
        );
    }

    #[test]
    fn nearest_ancestor_across_monorepo_package_json() {
        let map = ManifestMap::from_sources(&[
            src("package.json", r#"{"name": "root"}"#),
            src("packages/a/package.json", r#"{"name": "@acme/a"}"#),
            src("packages/a/src/index.ts", ""),
            src("app/index.ts", ""),
        ]);
        assert_eq!(
            map.manifest_for("packages/a/src/index.ts").unwrap().identity.as_deref(),
            Some("@acme/a")
        );
        assert_eq!(
            map.manifest_for("app/index.ts").unwrap().identity.as_deref(),
            Some("root")
        );
    }

    #[test]
    fn unparseable_manifest_yields_no_entry() {
        // A go.mod with no `module` line produces no entry, so a file under it
        // falls through to no manifest (same semantics as a virtual workspace).
        let map = ManifestMap::from_sources(&[
            src("go.mod", "go 1.21\n"),
            src("main.go", ""),
        ]);
        assert!(map.manifest_for("main.go").is_none());
    }
}
