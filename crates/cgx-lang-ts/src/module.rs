//! Module-path derivation from a repo-relative TypeScript/JavaScript file path.
//!
//! TypeScript/JS use `package.json`-based module names. For the fixtures we
//! derive a conventional `ts_sample::module_stem` path from the file's location,
//! mirroring the WP-02 golden YAML naming convention.
//!
//! Mapping rules:
//! - Strip `src/` prefix; the enclosing `package.json` name becomes the root
//!   (default: `ts_sample`).
//! - Each directory segment becomes a `::` segment.
//! - `index.ts`/`index.js` map to the directory root (no extra segment).
//! - Extension `.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts` is stripped.
//!
//! [`FileCtx`]: cgx_frontend::FileCtx

/// Default package name when the file is inside `src/` with no explicit
/// package directory. Matches the WP-02 TypeScript fixture package (`ts-sample`
/// → `ts_sample`).
const DEFAULT_PACKAGE: &str = "ts_sample";

/// Derive the `::`-separated module path prefix for a repo-relative TS/JS file.
///
/// The returned string never has a trailing `::`; a top-level `src/index.ts`
/// yields just the package name. Path separators are normalized to `/`.
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
    out
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
}
