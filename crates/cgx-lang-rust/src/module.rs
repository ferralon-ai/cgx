//! Module-path derivation from a repo-relative file path.
//!
//! A Rust frontend can only build *local* FQNs (the resolver may rewrite them),
//! but the crate-and-module prefix is determined by the file's location in the
//! source tree, which is information the frontend *does* have via [`FileCtx`].
//! This mirrors `rustc`'s file→module mapping closely enough for the fixtures:
//!
//! - `src/main.rs`, `src/lib.rs`, and any `mod.rs` map to their directory's
//!   module (no extra segment from the file stem).
//! - `src/errors.rs` → `<crate>::errors`; `src/a/b.rs` → `<crate>::a::b`.
//! - The crate name is the segment before `src/`, or a default when the path is
//!   already rooted at `src/` (the fixtures live at `src/…` with an implicit
//!   crate root).
//!
//! [`FileCtx`]: cgx_frontend::FileCtx

/// Default crate name when the path is rooted at `src/` with no crate segment in
/// front of it. Matches the WP-02 Rust fixture crate (`rust-sample` → the Rust
/// identifier `rust_sample`).
const DEFAULT_CRATE: &str = "rust_sample";

/// Derive the `::`-separated module path prefix for a repo-relative Rust file.
///
/// The returned string never has a trailing `::`; a top-level `src/main.rs`
/// yields just the crate name. Path separators are normalized to `/`.
pub fn module_path_for(rel_path: &str) -> String {
    let norm = rel_path.replace('\\', "/");
    let segments: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();

    // Find the `src` boundary. Everything before it (if anything) is the crate
    // directory; everything after it forms the module path.
    let src_idx = segments.iter().position(|s| *s == "src");

    let (crate_name, mod_segments): (String, &[&str]) = match src_idx {
        Some(i) => {
            let crate_name = if i == 0 {
                DEFAULT_CRATE.to_string()
            } else {
                // The directory immediately before `src` is the crate name.
                to_crate_ident(segments[i - 1])
            };
            (crate_name, &segments[i + 1..])
        }
        // No `src/` in the path: treat the whole thing as module segments under
        // the default crate.
        None => (DEFAULT_CRATE.to_string(), &segments[..]),
    };

    let mut out = crate_name;
    let n = mod_segments.len();
    for (i, seg) in mod_segments.iter().enumerate() {
        let is_last = i + 1 == n;
        let stem = seg.strip_suffix(".rs").unwrap_or(seg);
        // The file stem only contributes a module segment when it is not one of
        // the "root" file names.
        if is_last && matches!(stem, "main" | "lib" | "mod") {
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

/// Normalize a crate *directory* name into the Rust identifier crates use
/// (`my-crate` → `my_crate`).
fn to_crate_ident(dir: &str) -> String {
    dir.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_module_file_gets_stem_segment() {
        assert_eq!(module_path_for("src/errors.rs"), "rust_sample::errors");
    }

    #[test]
    fn root_files_contribute_no_segment() {
        assert_eq!(module_path_for("src/main.rs"), "rust_sample");
        assert_eq!(module_path_for("src/lib.rs"), "rust_sample");
        assert_eq!(module_path_for("src/a/mod.rs"), "rust_sample::a");
    }

    #[test]
    fn nested_directories_become_segments() {
        assert_eq!(module_path_for("src/a/b.rs"), "rust_sample::a::b");
    }

    #[test]
    fn crate_directory_before_src_is_the_crate_name() {
        assert_eq!(module_path_for("my-crate/src/x.rs"), "my_crate::x");
    }
}
