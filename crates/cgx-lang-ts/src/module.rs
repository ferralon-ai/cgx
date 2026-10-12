//! Module-path derivation from a repo-relative TypeScript/JavaScript file path.
//!
//! The canonical TS/JS module root is the **`package.json` `name`**, taken
//! **verbatim** (a scoped `@scope/pkg` keeps its `@` and `/`; a hyphenated name
//! like `ts-sample` keeps its hyphen — no identifier mangling). The nearest
//! ancestor `package.json` is resolved pipeline-side (a per-file `extract`
//! cannot read sibling files) and reaches us as `FileCtx::manifest`; when it is
//! present we derive the prefix with [`module_path_from_manifest`].
//!
//! When no `package.json` is resolved (`FileCtx::manifest == None`) we fall back
//! to [`module_path_for`], a **non-canonical best-effort** derivation that roots
//! at the directory before `src/` (mangled to an identifier). A repo with no
//! `package.json` has no declared name to be canonical to.
//!
//! Subpath mapping (both paths, below the package root / `src/` boundary):
//! - Each directory segment becomes a `::` segment.
//! - A trailing `index.ts`/`index.js` contributes no segment (it is the
//!   directory's module root).
//! - Extension `.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts`, `.mjs`, `.cjs` is
//!   stripped.
//!
//! [`FileCtx`]: cgx_frontend::FileCtx

/// Default package name used by the non-canonical fallback when the file is
/// inside `src/` with no explicit package directory. Matches the WP-02
/// TypeScript fixture package (`ts-sample` → `ts_sample`).
const DEFAULT_PACKAGE: &str = "ts_sample";

/// Derive the canonical `::`-separated module prefix from the resolved
/// `package.json` `name` and the file's subpath below the package directory.
///
/// `name` is used **verbatim** as the root segment — it is never run through
/// [`to_pkg_ident`], so `@acme/utils` keeps its `@` and `/`, and `ts-sample`
/// keeps its hyphen. `root_dir` is the repo-relative directory of the owning
/// `package.json` (the boundary); the file's segments below it form the
/// remaining `::` segments, with a leading `src/` boundary and a trailing
/// `index` contributing nothing. The returned string never has a trailing `::`.
pub fn module_path_from_manifest(rel_path: &str, name: &str, root_dir: &str) -> String {
    let norm = rel_path.replace('\\', "/");
    let segments: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();

    let root_norm = root_dir.replace('\\', "/");
    let root_segments: Vec<&str> = root_norm.split('/').filter(|s| !s.is_empty()).collect();

    // Strip the owning package.json's directory prefix. The resolver always
    // picks an ancestor, so this normally matches; if it somehow does not, fall
    // back to the whole path rather than panicking or mis-slicing.
    let below: &[&str] = if segments.len() >= root_segments.len()
        && segments[..root_segments.len()] == root_segments[..]
    {
        &segments[root_segments.len()..]
    } else {
        &segments[..]
    };

    // Drop a leading `src` boundary segment, mirroring the dir-before-`src/`
    // convention of the non-canonical fallback (`src/foo.ts` → `::foo`).
    let below = if below.first() == Some(&"src") {
        &below[1..]
    } else {
        below
    };

    let mut out = name.to_string();
    append_subpath(&mut out, below);
    out
}

/// Non-canonical fallback: derive the `::`-separated module prefix for a
/// repo-relative TS/JS file when no `package.json` is resolved.
///
/// Roots at the directory immediately before `src/` (mangled to an identifier
/// via [`to_pkg_ident`]), defaulting to [`DEFAULT_PACKAGE`]. The returned string
/// never has a trailing `::`; a top-level `src/index.ts` yields just the package
/// name. Path separators are normalized to `/`.
pub fn module_path_for(rel_path: &str) -> String {
    let norm = rel_path.replace('\\', "/");
    let segments: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();

    // Find the `src` boundary.
    let src_idx = segments.iter().position(|s| *s == "src");

    let (pkg_name, mod_segments): (String, &[&str]) = match src_idx {
        Some(i) => {
            let pkg_name = if i == 0 {
                DEFAULT_PACKAGE.to_string()
            } else {
                // The directory immediately before `src` is the package name.
                to_pkg_ident(segments[i - 1])
            };
            (pkg_name, &segments[i + 1..])
        }
        // No `src/` in the path: treat the whole path as segments under the
        // default package.
        None => (DEFAULT_PACKAGE.to_string(), &segments[..]),
    };

    let mut out = pkg_name;
    append_subpath(&mut out, mod_segments);
    out
}

/// Append the `::`-joined subpath segments to `out`, stripping TS/JS extensions,
/// skipping a trailing `index` stem, and skipping empty stems.
fn append_subpath(out: &mut String, mod_segments: &[&str]) {
    let n = mod_segments.len();
    for (i, seg) in mod_segments.iter().enumerate() {
        let is_last = i + 1 == n;
        let stem = strip_ts_ext(seg);
        // `index` at the end of the path contributes no segment (it's the
        // directory's module root).
        if is_last && stem == "index" {
            continue;
        }
        if stem.is_empty() {
            continue;
        }
        out.push_str("::");
        out.push_str(stem);
    }
}

/// Strip a TypeScript/JavaScript file extension from a segment.
fn strip_ts_ext(seg: &str) -> &str {
    for ext in &[".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"] {
        if let Some(stem) = seg.strip_suffix(ext) {
            return stem;
        }
    }
    seg
}

/// Normalize a package *directory* name into the identifier form used in FQNs
/// (`ts-sample` → `ts_sample`).
fn to_pkg_ident(dir: &str) -> String {
    dir.chars()
        .map(|c| if c == '-' || c == '.' { '_' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_module_file_gets_stem_segment() {
        assert_eq!(module_path_for("src/errors.ts"), "ts_sample::errors");
    }

    #[test]
    fn index_contributes_no_segment() {
        assert_eq!(module_path_for("src/index.ts"), "ts_sample");
    }

    #[test]
    fn nested_directory() {
        assert_eq!(module_path_for("src/a/b.ts"), "ts_sample::a::b");
    }

    #[test]
    fn package_directory_before_src() {
        assert_eq!(module_path_for("ts-sample/src/x.ts"), "ts_sample::x");
    }

    #[test]
    fn js_extension_stripped() {
        assert_eq!(module_path_for("src/utils.js"), "ts_sample::utils");
    }

    #[test]
    fn tsx_extension_stripped() {
        assert_eq!(module_path_for("src/App.tsx"), "ts_sample::App");
    }

    // -- canonical package.json-name root (module_path_from_manifest) --

    #[test]
    fn scoped_name_preserved_verbatim() {
        // `@` and `/` of a scoped name survive; no `-`/`.`→`_` mangling.
        assert_eq!(
            module_path_from_manifest(
                "packages/utils/src/components/Button.tsx",
                "@acme/utils",
                "packages/utils",
            ),
            "@acme/utils::components::Button"
        );
    }

    #[test]
    fn scoped_index_contributes_no_segment() {
        assert_eq!(
            module_path_from_manifest(
                "packages/web/utils/src/index.ts",
                "@acme/web-utils",
                "packages/web/utils",
            ),
            "@acme/web-utils"
        );
    }

    #[test]
    fn monorepo_same_leaf_resolves_to_distinct_roots() {
        // Before this PR both rooted at `utils` (dir-before-`src/`); the
        // package.json name now disambiguates them.
        let web = module_path_from_manifest(
            "packages/web/utils/src/service.ts",
            "@acme/web-utils",
            "packages/web/utils",
        );
        let api = module_path_from_manifest(
            "services/api/utils/src/service.ts",
            "@acme/api-utils",
            "services/api/utils",
        );
        assert_eq!(web, "@acme/web-utils::service");
        assert_eq!(api, "@acme/api-utils::service");
        assert_ne!(web, api);
    }

    #[test]
    fn hyphenated_name_kept_verbatim() {
        // Unscoped names are also verbatim — no `to_pkg_ident` on the root.
        assert_eq!(
            module_path_from_manifest("src/errors.ts", "ts-sample", ""),
            "ts-sample::errors"
        );
    }

    #[test]
    fn manifest_extension_stripped() {
        assert_eq!(
            module_path_from_manifest("pkg/src/util.mts", "@acme/utils", "pkg"),
            "@acme/utils::util"
        );
    }

    #[test]
    fn no_package_json_fallback_unchanged() {
        // The non-canonical dir-before-`src/` fallback is untouched.
        assert_eq!(module_path_for("src/errors.ts"), "ts_sample::errors");
        assert_eq!(module_path_for("ts-sample/src/x.ts"), "ts_sample::x");
    }
}
