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
