//! The [`LanguageFrontend`] trait — the pluggability seam (architecture §5).
//!
//! A language adapter (WP-04 Rust, WP-05 TypeScript, …) implements exactly this
//! trait. Indexing (WP-08) dispatches a file to a frontend via the
//! [`registry`](crate::registry), calls [`LanguageFrontend::extract`], and
//! caches the resulting [`FileFacts`] fragment keyed by blob OID. The resolver
//! (WP-06) consumes the fragments. No frontend ever sees the store or git.

use crate::facts::FileFacts;
use std::fmt;

/// A source-language tag.
///
/// The variants are the Phase-1 commitments plus the [`Lang::Fallback`] tag the
/// Tier-0 generic frontend uses. The free-form [`Lang::Other`] keeps the tag
/// open without a core-crate change when a new adapter lands.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    JavaScript,
    Go,
    Java,
    Python,
    /// A language handled only by the Tier-0 generic fallback (the `lang` string
    /// is the detected grammar name, e.g. `"python"`, `"go"`).
    Fallback(String),
    /// A named language with a dedicated adapter not in the enum above.
    Other(String),
}

impl Lang {
    /// The stable lowercase tag stored in `NodeRecord::lang` and the store's
    /// `lang` column. Deterministic and language-neutral.
    pub fn tag(&self) -> &str {
        match self {
            Lang::Rust => "rust",
            Lang::TypeScript => "typescript",
            Lang::JavaScript => "javascript",
            Lang::Go => "go",
            Lang::Java => "java",
            Lang::Python => "python",
            Lang::Fallback(name) | Lang::Other(name) => name.as_str(),
        }
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// A repo-relative source path. A thin newtype over a `String` so the trait
/// surface is explicit about *which* path flavour `handles` inspects (always
/// repo-relative, `/`-separated, never absolute).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    pub fn new(path: impl Into<String>) -> Self {
        RelPath(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The lowercased extension without the dot (`"src/a.RS"` → `Some("rs")`),
    /// or `None` if the final path segment has no extension.
    pub fn extension(&self) -> Option<String> {
        let name = self.0.rsplit(['/', '\\']).next().unwrap_or(&self.0);
        let dot = name.rfind('.')?;
        // A leading-dot file like `.gitignore` has no extension.
        if dot == 0 {
            return None;
        }
        Some(name[dot + 1..].to_ascii_lowercase())
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The kind of build manifest a [`ManifestInfo`] was resolved from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ManifestKind {
    /// Rust `Cargo.toml`.
    Cargo,
    /// Go `go.mod`.
    GoMod,
    /// Node `package.json`.
    PackageJson,
    /// Python `pyproject.toml` / `setup.py`.
    PyProject,
}

/// Manifest-declared facts about the module/package that owns a file, resolved
/// pipeline-side from the nearest-ancestor manifest (the only place that sees
/// sibling files — a per-file [`extract`](LanguageFrontend::extract) cannot read
/// its own `go.mod` / `package.json` / `pyproject.toml`).
///
/// Behaviour-neutral until an adapter reads it: carried alongside the legacy
/// [`FileCtx::package`] field, which Rust keeps consuming unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInfo {
    /// Which manifest kind this came from.
    pub kind: ManifestKind,
    /// The manifest-declared identity, **verbatim**: the go.mod module path, the
    /// package.json `name` (scope preserved, e.g. `@acme/utils`). `None` when the
    /// manifest declares no identity string (Python: the importable path is
    /// filesystem-derived, so only [`root_dir`](Self::root_dir) is meaningful).
    pub identity: Option<String>,
    /// The repo-relative directory that anchors module-path derivation: the
    /// owning manifest's directory for Cargo/Go/Node; for Python the src-aware
    /// import root (the pyproject dir plus `/src` when a `src/` subdir exists).
    pub root_dir: String,
}

/// Context passed to [`LanguageFrontend::extract`] for one file.
///
/// Carries only what extraction needs and nothing the frontend must not see
/// (no store handle, no git): the repo-relative path (drives module-path
/// construction), the blob OID (stamped into provenance as `index_id`), the
/// owning package name (the crate root for FQN derivation when it cannot be read
/// off the path — e.g. a `src/`-at-root single-crate layout), and the resolved
/// manifest facts ([`ManifestInfo`]) for adapters whose canonical root is a
/// manifest-declared identity.
#[derive(Debug, Clone)]
pub struct FileCtx {
    /// Repo-relative path of the file being extracted.
    pub path: RelPath,
    /// Git blob OID of the file at index time; recorded in provenance.
    pub blob_oid: String,
    /// The owning package name (from the nearest ancestor `Cargo.toml`'s
    /// `[package] name`), already normalized to the Rust crate identifier
    /// (hyphens → underscores). `None` ⇒ the frontend derives the crate root
    /// from the path alone (the workspace-layout case, unchanged).
    pub package: Option<String>,
    /// The nearest-ancestor build manifest's resolved facts, if any. `None` ⇒ no
    /// manifest of a resolved kind is an ancestor (a file falls through to its
    /// path-derived root). Not read by any adapter yet.
    pub manifest: Option<ManifestInfo>,
}

impl FileCtx {
    pub fn new(path: impl Into<String>, blob_oid: impl Into<String>) -> Self {
        FileCtx {
            path: RelPath::new(path),
            blob_oid: blob_oid.into(),
            package: None,
            manifest: None,
        }
    }

    /// Set the owning package name (the crate root used for FQN derivation when
    /// the path carries no crate directory before `src/`).
    pub fn with_package(mut self, package: Option<String>) -> Self {
        self.package = package;
        self
    }

    /// Set the resolved manifest facts for this file.
    pub fn with_manifest(mut self, manifest: Option<ManifestInfo>) -> Self {
        self.manifest = manifest;
        self
    }
}

/// An error raised while extracting a file.
///
/// Extraction failures are rare by design: the Tier-0 fallback degrades to
/// empty facts rather than erroring on unknown syntax. A frontend returns
/// `Err` only when it genuinely cannot proceed (grammar load failed, source is
/// not valid UTF-8 where the frontend requires it).
#[derive(Debug, thiserror::Error)]
pub enum FrontendError {
    /// The grammar/parser could not be initialized.
    #[error("failed to initialize parser for {lang}: {detail}")]
    Parser { lang: String, detail: String },

    /// The source bytes were not valid for this frontend (e.g. non-UTF-8 where
    /// UTF-8 is required).
    #[error("invalid source for {path}: {detail}")]
    InvalidSource { path: String, detail: String },

    /// A catch-all for adapter-specific extraction failures.
    #[error("extraction failed for {path}: {detail}")]
    Extraction { path: String, detail: String },
}

/// The language-frontend seam (architecture §5).
///
/// Implementors emit per-file [`FileFacts`] with **zero cross-file knowledge**.
/// The trait is object-safe so the registry can hold `Arc<dyn LanguageFrontend>`
/// and dispatch dynamically by file extension.
pub trait LanguageFrontend: Send + Sync {
    /// The language this frontend produces facts for.
    fn lang(&self) -> Lang;

    /// Whether this frontend claims `path` (by extension and, optionally,
    /// shebang/heuristics). The registry calls this to dispatch.
    fn handles(&self, path: &RelPath) -> bool;

    /// A monotonic version of this frontend's extraction rules. Bumping it
    /// invalidates every cached fragment this frontend produced (architecture
    /// §3 `frontend_version`), forcing a re-extract on the next index run.
    fn fragment_version(&self) -> u32;

    /// Extract per-file facts from `src`. The returned [`FileFacts`] is
    /// canonicalized by the caller (the registry's [`extract`] helper does it)
    /// before encoding; an implementor may return it in any order.
    ///
    /// [`extract`]: crate::registry::FrontendRegistry::extract
    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_ctx_defaults_have_no_manifest_or_package() {
        let ctx = FileCtx::new("src/a.rs", "oid");
        assert_eq!(ctx.package, None);
        assert_eq!(ctx.manifest, None);
    }

    #[test]
    fn with_manifest_round_trips_and_leaves_package_intact() {
        let info = ManifestInfo {
            kind: ManifestKind::GoMod,
            identity: Some("example.com/app".to_string()),
            root_dir: "svc".to_string(),
        };
        let ctx = FileCtx::new("svc/db.go", "oid")
            .with_package(Some("pkg".to_string()))
            .with_manifest(Some(info.clone()));
        assert_eq!(ctx.package.as_deref(), Some("pkg"));
        assert_eq!(ctx.manifest, Some(info));
    }
}
