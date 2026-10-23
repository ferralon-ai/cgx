//! Language families the selector engine recognizes, and their mapping to the
//! [`NodeRecord.lang`](cgx_core::node::NodeRecord) string domain (the recon-verified
//! set: all lowercase, full-word tags).

use serde::{Deserialize, Serialize};

/// A language family. A family is the identity a tokenizer stamps onto every
/// parse it produces, and it carries the `NodeRecord.lang` tag(s) used for the
/// native (non-agnostic) match restriction.
///
/// The declaration order is the canonical sort order; it is load-bearing for
/// deterministic provenance output regardless of tokenizer registration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Rust,
    Go,
    TypeScript,
    Java,
    Python,
}

impl Family {
    /// The `NodeRecord.lang` tag(s) a native match against this family requires.
    /// Verified domain: `crates/cgx-lang-*/src/extract.rs` (recon A).
    pub fn lang_tags(self) -> &'static [&'static str] {
        match self {
            Family::Rust => &["rust"],
            Family::Go => &["go"],
            Family::TypeScript => &["typescript"],
            Family::Java => &["java"],
            Family::Python => &["python"],
        }
    }

    /// The family that owns a [`NodeRecord.lang`](cgx_core::node::NodeRecord) tag,
    /// or `None` for a tag no family claims (e.g. `c`, `cpp`). The inverse of
    /// [`lang_tags`](Self::lang_tags); the tag domain is disjoint across families,
    /// so the first match is unambiguous.
    pub fn from_lang_tag(tag: &str) -> Option<Family> {
        [
            Family::Rust,
            Family::Go,
            Family::TypeScript,
            Family::Java,
            Family::Python,
        ]
        .into_iter()
        .find(|fam| fam.lang_tags().contains(&tag))
    }

    /// The family that owns a source file, inferred from its extension, or `None`
    /// for an extension no family claims. This is the fallback language signal for
    /// rendering a node whose [`NodeRecord.lang`](cgx_core::node::NodeRecord) tag is
    /// empty — dataflow value nodes are synthesized without a lang tag, yet their
    /// fqn must still render in the owning language's native syntax rather than
    /// leaking the internal segment delimiter. Extensions mirror the adapters'
    /// `Frontend::extensions` (`crates/cgx-lang-*`); C/C++ are deliberately absent
    /// (no family), so their nodes keep the stored encoding.
    pub fn from_path(path: &str) -> Option<Family> {
        let ext = path.rsplit('.').next().filter(|e| *e != path)?;
        match ext {
            "rs" => Some(Family::Rust),
            "go" => Some(Family::Go),
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Some(Family::TypeScript),
            "java" => Some(Family::Java),
            "py" => Some(Family::Python),
            _ => None,
        }
    }

    /// The surface separator this family joins fqn segments with when rendering a
    /// symbol in its native syntax. This is the family's own separator — kept in
    /// lockstep with the family's tokenizer (`tokenizer::SeparatorTokenizer`): Rust
    /// `::`, Go `/`, and `.` for the dotted families (TypeScript/Java/Python). It
    /// is deliberately distinct from the opaque internal
    /// [`FQN_SEGMENT_DELIM`](cgx_core::FQN_SEGMENT_DELIM) that a stored fqn is split
    /// on — that Rust's native separator coincides with it is not relied upon here.
    pub fn native_separator(self) -> &'static str {
        match self {
            Family::Rust => "::",
            Family::Go => "/",
            Family::TypeScript | Family::Java | Family::Python => ".",
        }
    }

    /// Stable human-readable name (matches the surface syntax family, not a tag).
    pub fn name(self) -> &'static str {
        match self {
            Family::Rust => "rust",
            Family::Go => "go",
            Family::TypeScript => "typescript",
            Family::Java => "java",
            Family::Python => "python",
        }
    }
}
