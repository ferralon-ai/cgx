//! Workspace-aware owning-package resolution for Rust source files.
//!
//! A Rust node's FQN is rooted at its crate name (e.g. `cgx_core::module::item`).
//! The frontend can read the crate name off the path *only* for the workspace
//! layout (`crates/foo/src/...` → `foo`); for a single-crate-at-root layout
//! (`<root>/src/...`) or a no-`src` layout there is no crate directory in the
//! path, and the frontend would otherwise fall back to a hardcoded default. That
//! mismatch silently breaks the `--scip` join, whose symbols carry the *real*
//! crate name (design BLOCKER-2).
//!
//! This module resolves each file's owning package from the nearest ancestor
//! `Cargo.toml`'s `[package] name`, normalized to the Rust crate identifier
//! (hyphens → underscores, matching what rust-analyzer emits in SCIP symbols).
//!
//! ## Zero-dep TOML parse
//!
//! cgx carries no `toml` dependency and adds none. The `[package] name` field is
//! extracted by a deliberately small line scan: find the `[package]` table
//! header, then the first `name = "..."` (or `'...'`) before the next table
//! header. This is sufficient for real-world manifests (cargo writes `name`
//! near the top of `[package]`) and matches cgx's zero-dep ethos. A manifest
//! with no `[package]` (a virtual workspace root) yields no package and is
//! skipped, so files under it resolve to a *nearer* member manifest.

use std::collections::BTreeMap;

use crate::git::SourceFile;

/// Maps each crate-source file to the Rust crate identifier of its owning
/// package, resolved workspace-aware from the set of `Cargo.toml` manifests in
/// the index. Built once per index run.
#[derive(Debug, Default)]
pub(crate) struct PackageMap {
    /// `manifest-dir → normalized crate identifier`. The manifest dir is the
    /// `/`-separated repo-relative directory containing a `Cargo.toml` that
    /// declares a `[package] name`. The repo root is the empty string.
    by_dir: BTreeMap<String, String>,
}

impl PackageMap {
    /// Build the map from the full enumerated source set, parsing every
    /// `Cargo.toml` blob for its `[package] name`.
    pub(crate) fn from_sources(sources: &[SourceFile]) -> PackageMap {
        let mut by_dir = BTreeMap::new();
        for src in sources {
            if !is_cargo_manifest(&src.rel_path) {
                continue;
            }
            let dir = parent_dir(&src.rel_path);
            if let Ok(text) = std::str::from_utf8(&src.content) {
                if let Some(name) = parse_package_name(text) {
                    by_dir.insert(dir.to_string(), to_crate_ident(&name));
                }
            }
        }
        PackageMap { by_dir }
    }

    /// The owning package's crate identifier for `rel_path`, if any `Cargo.toml`
    /// with a `[package] name` is an ancestor. Returns the *nearest* ancestor
    /// (longest matching directory prefix), so workspace members win over the
    /// workspace root.
    pub(crate) fn package_for(&self, rel_path: &str) -> Option<&str> {
        let file_dir = parent_dir(rel_path);
        let mut best: Option<(&str, &str)> = None;
        for (dir, name) in &self.by_dir {
            if is_dir_ancestor(dir, file_dir) {
                match best {
                    Some((best_dir, _)) if best_dir.len() >= dir.len() => {}
                    _ => best = Some((dir.as_str(), name.as_str())),
                }
            }
        }
        best.map(|(_, name)| name)
    }
}

/// Whether `path` is a Cargo manifest (`Cargo.toml`, at any depth).
fn is_cargo_manifest(path: &str) -> bool {
    path == "Cargo.toml" || path.ends_with("/Cargo.toml")
}

/// The `/`-separated directory portion of a repo-relative path (`""` for a
/// root-level file).
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// Whether `ancestor` is `descendant` or a directory prefix of it. Both are
/// `/`-separated repo-relative dirs (`""` is the repo root, an ancestor of all).
fn is_dir_ancestor(ancestor: &str, descendant: &str) -> bool {
    if ancestor.is_empty() {
        return true;
    }
    if ancestor == descendant {
        return true;
    }
    descendant
        .strip_prefix(ancestor)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Normalize a cargo package name into the Rust crate identifier (`my-crate` →
/// `my_crate`), matching the identifier rust-analyzer emits in SCIP symbols.
fn to_crate_ident(name: &str) -> String {
    name.replace('-', "_")
}

/// Extract `[package] name` from a `Cargo.toml`'s text with a minimal line scan
/// (no `toml` dependency). Returns the raw (un-normalized) name. `None` when the
/// manifest has no `[package]` table or no `name` key within it (e.g. a virtual
/// workspace root).
fn parse_package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            // A new table header. `[package]` (and only that) opens the scan;
            // any other header (incl. `[package.metadata...]`) closes it.
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(value) = key_value(line, "name") {
                return Some(value);
            }
        }
    }
    None
}

/// Strip a `#` line comment, respecting `#` inside a quoted string value.
fn strip_comment(line: &str) -> &str {
    let mut in_str: Option<char> = None;
    for (i, ch) in line.char_indices() {
        match in_str {
            Some(q) if ch == q => in_str = None,
            Some(_) => {}
            None => match ch {
                '"' | '\'' => in_str = Some(ch),
                '#' => return &line[..i],
                _ => {}
            },
        }
    }
    line
}

/// Parse `key = "value"` (or `'value'`), returning the unquoted value when
/// `line`'s key matches `key`.
fn key_value(line: &str, key: &str) -> Option<String> {
    let (lhs, rhs) = line.split_once('=')?;
    if lhs.trim() != key {
        return None;
    }
    let rhs = rhs.trim();
    let bytes = rhs.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0] as char;
        let last = bytes[bytes.len() - 1] as char;
        if (first == '"' || first == '\'') && last == first {
            return Some(rhs[1..rhs.len() - 1].to_string());
        }
    }
    None
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
    fn parses_simple_package_name() {
        let toml = "[package]\nname = \"acme\"\nversion = \"0.1.0\"\n";
        assert_eq!(parse_package_name(toml).as_deref(), Some("acme"));
    }

    #[test]
    fn ignores_name_outside_package_table() {
        let toml = "[package]\nversion = \"0.1.0\"\n\n[dependencies]\nname = \"nope\"\n";
        assert_eq!(parse_package_name(toml), None);
    }

    #[test]
    fn ignores_name_in_package_metadata_subtable() {
        let toml = "[package]\n\n[package.metadata.foo]\nname = \"nope\"\n";
        assert_eq!(parse_package_name(toml), None);
    }

    #[test]
    fn handles_single_quotes_and_trailing_comment() {
        let toml = "[package]\nname = 'acme-corp'  # the crate\n";
        assert_eq!(parse_package_name(toml).as_deref(), Some("acme-corp"));
    }

    #[test]
    fn virtual_workspace_manifest_has_no_package() {
        let toml = "[workspace]\nmembers = [\"crates/*\"]\n";
        assert_eq!(parse_package_name(toml), None);
    }

    #[test]
    fn root_manifest_resolves_src_at_root_layout() {
        let map = PackageMap::from_sources(&[
            src("Cargo.toml", "[package]\nname = \"acme\"\n"),
            src("src/lib.rs", ""),
        ]);
        assert_eq!(map.package_for("src/lib.rs"), Some("acme"));
    }

    #[test]
    fn hyphenated_name_normalized_to_crate_ident() {
        let map = PackageMap::from_sources(&[src("Cargo.toml", "[package]\nname = \"acme-corp\"\n")]);
        assert_eq!(map.package_for("src/lib.rs"), Some("acme_corp"));
    }

    #[test]
    fn nearest_member_manifest_wins_over_workspace_root() {
        let map = PackageMap::from_sources(&[
            src("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
            src("crates/foo/Cargo.toml", "[package]\nname = \"foo\"\n"),
            src("crates/foo/src/lib.rs", ""),
        ]);
        assert_eq!(map.package_for("crates/foo/src/lib.rs"), Some("foo"));
    }

    #[test]
    fn member_manifest_shadows_root_package_for_its_files() {
        let map = PackageMap::from_sources(&[
            src("Cargo.toml", "[package]\nname = \"root\"\n"),
            src("crates/foo/Cargo.toml", "[package]\nname = \"foo\"\n"),
            src("crates/foo/src/lib.rs", ""),
            src("src/lib.rs", ""),
        ]);
        assert_eq!(map.package_for("crates/foo/src/lib.rs"), Some("foo"));
        assert_eq!(map.package_for("src/lib.rs"), Some("root"));
    }

    #[test]
    fn no_manifest_yields_none() {
        let map = PackageMap::from_sources(&[src("src/lib.rs", "")]);
        assert_eq!(map.package_for("src/lib.rs"), None);
    }
}
