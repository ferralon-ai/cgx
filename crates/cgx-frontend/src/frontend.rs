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

/// Context passed to [`LanguageFrontend::extract`] for one file.
///
/// Carries only what extraction needs and nothing the frontend must not see
/// (no store handle, no git): the repo-relative path (drives module-path
/// construction), the blob OID (stamped into provenance as `index_id`), and the
/// owning package name (the crate root for FQN derivation when it cannot be read
/// off the path — e.g. a `src/`-at-root single-crate layout).
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
}

impl FileCtx {
    pub fn new(path: impl Into<String>, blob_oid: impl Into<String>) -> Self {
        FileCtx {
            path: RelPath::new(path),
            blob_oid: blob_oid.into(),
            package: None,
        }
    }

    /// Set the owning package name (the crate root used for FQN derivation when
    /// the path carries no crate directory before `src/`).
    pub fn with_package(mut self, package: Option<String>) -> Self {
        self.package = package;
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
