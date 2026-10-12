//! Compdb-completeness self-check (Phase F2 — Eric's binding policy, 2026-10-06).
//!
//! scip-clang resolves C++ symbols only inside the translation units its
//! `compile_commands.json` told it to compile. If that compdb omits a TU cgx
//! discovers in the source tree, scip-clang's merged index can report one
//! definition where the program has several — a *false singleton* that would
//! wrongly promote to `certain`. The FAIL-CLOSED cap in
//! [`scip_relabel`](super::scip_relabel) exists for exactly that; this module
//! produces the signal it consumes.
//!
//! **The policy (build-authoritative + TU-coverage self-check):** the compdb is
//! [`Complete`](CompdbCompleteness::Complete) iff every C++ *translation unit* cgx
//! discovers in the source tree is covered by a compdb `file` entry. Any
//! discovered TU absent ⇒ [`Partial`](CompdbCompleteness::Partial) ⇒ the relabel
//! caps promotion at `probable`. Computed locally (zero-egress).
//!
//! **TUs only.** A translation unit is a compiled source file — `.cpp/.cc/.cxx`
//! (the TU subset of the extensions [`cgx_lang_cpp::CppFrontend`] claims). Headers
//! (`.hpp/.hh/.hxx`) are not compilation units and never appear in a compdb, so
//! they never gate completeness: a header absent from the compdb does **not** make
//! it `Partial`.
//!
//! **Fail-closed on every doubt.** Absent compdb, unreadable file, malformed JSON
//! → `Partial`. `certain` is reached only via `--compdb-complete` (an explicit CI
//! assertion) or a parsed compdb that demonstrably covers every discovered TU.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::scip_relabel::CompdbCompleteness;
use crate::git::SourceFile;

/// C++ translation-unit extensions (NOT headers). The TU subset of the extensions
/// `cgx_lang_cpp::CppFrontend::handles` claims (`cpp/cc/cxx` are compiled units;
/// `hpp/hh/hxx` are headers and excluded by policy).
const CPP_TU_EXTENSIONS: &[&str] = &["cpp", "cc", "cxx"];

/// How many missing-TU paths the diagnostic sample retains. The full count is
/// reported separately; the sample is bounded so a large miss does not flood logs.
const MISSING_SAMPLE: usize = 8;

/// The computed compdb-completeness verdict plus the evidence behind it, surfaced
/// through [`IndexStats`](super::IndexStats) so the CLI can report *why* a
/// scip-clang index capped at `probable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompdbReport {
    /// The verdict fed to the relabel's fail-closed gate.
    pub completeness: CompdbCompleteness,
    /// C++ TUs cgx discovered in the source tree (headers excluded).
    pub discovered_tus: usize,
    /// Discovered C++ TUs covered by a compdb entry.
    pub covered_tus: usize,
    /// Count of discovered C++ TUs absent from the compdb (0 ⇒ complete).
    pub missing_tus: usize,
    /// Bounded sample (≤ [`MISSING_SAMPLE`]) of the missing TU paths, for the
    /// diagnostic line. Empty when complete.
    pub missing_sample: Vec<String>,
}

/// One `compile_commands.json` entry. Only `file` and `directory` matter for
/// coverage; the compile `command`/`arguments` are ignored.
#[derive(Deserialize)]
struct CompdbEntry {
    file: String,
    #[serde(default)]
    directory: Option<String>,
}

/// Whether `rel_path` names a C++ translation unit (by extension; headers excluded).
fn is_cpp_tu(rel_path: &str) -> bool {
    let name = rel_path.rsplit(['/', '\\']).next().unwrap_or(rel_path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => {
            let ext = name[dot + 1..].to_ascii_lowercase();
            CPP_TU_EXTENSIONS.contains(&ext.as_str())
        }
        _ => false,
    }
}

/// Lexically normalize `path` (joined onto `base` when relative) to an absolute,
/// `.`/`..`-resolved [`PathBuf`] **without touching the filesystem**.
///
/// Filesystem canonicalization is deliberately avoided: a committed-tree index
/// has no files on disk to canonicalize, and symlink resolution would make the
/// signal depend on the machine rather than the source tree. Both the discovered
/// TUs and the compdb entries pass through this one function so they compare in a
/// single frame.
fn clean_abs(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(comp.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Parse `compile_commands.json` into the set of absolute, cleaned TU paths it
/// covers. A relative `file` is resolved against the entry's `directory` (compdb
/// semantics) — falling back to the compdb's own parent directory when the entry
/// omits one. Returns `None` on any read/parse failure (⇒ fail-closed `Partial`).
fn parse_compdb(compdb_path: &Path) -> Option<BTreeSet<PathBuf>> {
    let bytes = std::fs::read(compdb_path).ok()?;
    let entries: Vec<CompdbEntry> = serde_json::from_slice(&bytes).ok()?;
    let compdb_dir = compdb_path.parent().unwrap_or_else(|| Path::new(""));
    let mut covered = BTreeSet::new();
    for e in entries {
        let base = match &e.directory {
            Some(d) => PathBuf::from(d),
            None => compdb_dir.to_path_buf(),
        };
        covered.insert(clean_abs(&base, Path::new(&e.file)));
    }
    Some(covered)
}

/// Compute the compdb-completeness signal for a scip-clang index.
///
/// `sources` is the index's own file-discovery output; `source_root` is the
/// directory those rel-paths are relative to (the repo workdir for a committed
/// index, the walked directory for a workdir index) and anchors discovered TUs to
/// absolute paths. `compdb` is the `--compdb` path (`None` ⇒ no compdb known).
/// `assert_complete` is the `--compdb-complete` CI override.
///
/// FAIL-CLOSED: `Complete` is returned only when `assert_complete` is set, or a
/// parsed compdb covers every discovered C++ TU. Everything else — absent,
/// unreadable, malformed, or missing even one TU — yields `Partial`.
pub(crate) fn completeness(
    sources: &[SourceFile],
    source_root: &Path,
    compdb: Option<&Path>,
    assert_complete: bool,
) -> CompdbReport {
    let discovered: Vec<&str> = sources
        .iter()
        .map(|s| s.rel_path.as_str())
        .filter(|p| is_cpp_tu(p))
        .collect();
    let discovered_tus = discovered.len();

    // --compdb-complete: CI asserts completeness explicitly; trust it.
    if assert_complete {
        return CompdbReport {
            completeness: CompdbCompleteness::Complete,
            discovered_tus,
            covered_tus: discovered_tus,
            missing_tus: 0,
            missing_sample: Vec::new(),
        };
    }

    // No compdb, or one we cannot read/parse → fail-closed Partial. Every
    // discovered TU is "missing" (none verified covered).
    let Some(compdb) = compdb else {
        return partial_all_missing(&discovered);
    };
    let Some(covered) = parse_compdb(compdb) else {
        return partial_all_missing(&discovered);
    };

    let mut missing = Vec::new();
    for rel in &discovered {
        let abs = clean_abs(source_root, Path::new(rel));
        if !covered.contains(&abs) {
            missing.push((*rel).to_string());
        }
    }
    let missing_tus = missing.len();
    let completeness = if missing_tus == 0 {
        CompdbCompleteness::Complete
    } else {
        CompdbCompleteness::Partial
    };
    let mut missing_sample = missing;
    missing_sample.truncate(MISSING_SAMPLE);
    CompdbReport {
        completeness,
        discovered_tus,
        covered_tus: discovered_tus - missing_tus,
        missing_tus,
        missing_sample,
    }
}

fn partial_all_missing(discovered: &[&str]) -> CompdbReport {
    CompdbReport {
        completeness: CompdbCompleteness::Partial,
        discovered_tus: discovered.len(),
        covered_tus: 0,
        missing_tus: discovered.len(),
        missing_sample: discovered
            .iter()
            .take(MISSING_SAMPLE)
            .map(|p| (*p).to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(rel: &str) -> SourceFile {
        SourceFile {
            blob_oid: "0".repeat(40),
            rel_path: rel.to_string(),
            content: Vec::new(),
        }
    }

    #[test]
    fn tu_extensions_exclude_headers() {
        for tu in ["a.cpp", "src/b.cc", "deep/c.cxx", "d.CPP"] {
            assert!(is_cpp_tu(tu), "{tu} should be a TU");
        }
        for hdr in ["a.hpp", "b.hh", "c.hxx", "d.h", "e.rs", "noext", ".hidden"] {
            assert!(!is_cpp_tu(hdr), "{hdr} should NOT be a TU");
        }
    }

    #[test]
    fn clean_abs_resolves_dot_segments() {
        let base = Path::new("/repo");
        assert_eq!(clean_abs(base, Path::new("src/./f.cpp")), PathBuf::from("/repo/src/f.cpp"));
        assert_eq!(clean_abs(base, Path::new("build/../src/f.cpp")), PathBuf::from("/repo/src/f.cpp"));
        assert_eq!(clean_abs(base, Path::new("/abs/g.cpp")), PathBuf::from("/abs/g.cpp"));
    }

    #[test]
    fn assert_complete_overrides_without_reading() {
        let sources = [src("src/f.cpp")];
        let r = completeness(&sources, Path::new("/repo"), None, true);
        assert_eq!(r.completeness, CompdbCompleteness::Complete);
        assert_eq!(r.missing_tus, 0);
    }

    #[test]
    fn absent_compdb_is_partial_fail_closed() {
        let sources = [src("src/f.cpp")];
        let r = completeness(&sources, Path::new("/repo"), None, false);
        assert_eq!(r.completeness, CompdbCompleteness::Partial);
        assert_eq!(r.missing_tus, 1);
    }
}
