//! Symbol patterns (architecture §4 `resolve_symbol`).
//!
//! A pattern names a set of symbols by FQN, by glob over the FQN, or by short
//! (last-segment) name. Matching is a pure, deterministic predicate over a node
//! record; `cgx-core` owns the matcher so every consumer (query, output, MCP)
//! resolves identically.

use crate::node::NodeRecord;
use serde::{Deserialize, Serialize};

/// How a [`SymbolPattern`] is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatternKind {
    /// Exact, full FQN match.
    Fqn,
    /// Glob over the full FQN. Supports `*` (any run of non-`::` chars) and `**`
    /// (any run including `::`), and `?` (one non-`::` char).
    Glob,
    /// Match the short (last `::`-delimited segment) name exactly.
    ShortName,
}

/// A pattern resolving to a set of symbols (architecture §4).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolPattern {
    pub kind: PatternKind,
    pub text: String,
}

impl SymbolPattern {
    pub fn fqn(text: impl Into<String>) -> Self {
        SymbolPattern {
            kind: PatternKind::Fqn,
            text: text.into(),
        }
    }

    pub fn glob(text: impl Into<String>) -> Self {
        SymbolPattern {
            kind: PatternKind::Glob,
            text: text.into(),
        }
    }

    pub fn short_name(text: impl Into<String>) -> Self {
        SymbolPattern {
            kind: PatternKind::ShortName,
            text: text.into(),
        }
    }

    /// Whether `node`'s FQN matches this pattern.
    pub fn matches(&self, node: &NodeRecord) -> bool {
        match self.kind {
            PatternKind::Fqn => node.fqn == self.text,
            PatternKind::ShortName => short_name(&node.fqn) == self.text,
            PatternKind::Glob => glob_match(&self.text, &node.fqn),
        }
    }
}

/// The last `::`-delimited segment of an FQN.
fn short_name(fqn: &str) -> &str {
    fqn.rsplit("::").next().unwrap_or(fqn)
}

/// A deterministic glob matcher over an FQN.
///
/// Tokens: `**` matches any run of characters including `::`; `*` matches any run
/// of characters that contains no `::` separator; `?` matches exactly one
/// non-separator character; every other character matches literally. Pure
/// backtracking; no regex dependency.
fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_at(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_at(pat: &[u8], text: &[u8]) -> bool {
    // `**` — consume any prefix of `text` (separators allowed), then match rest.
    if pat.starts_with(b"**") {
        let rest = &pat[2..];
        // Try every possible suffix of text, shortest first for determinism.
        for split in 0..=text.len() {
            if glob_match_at(rest, &text[split..]) {
                return true;
            }
        }
        return false;
    }

    // `*` — consume any run within a single segment (no `::`).
    if pat.first() == Some(&b'*') {
        let rest = &pat[1..];
        let mut split = 0;
        loop {
            if glob_match_at(rest, &text[split..]) {
                return true;
            }
            if split >= text.len() {
                return false;
            }
            // Stop expanding `*` at a `::` separator boundary.
            if text[split] == b':' && split + 1 < text.len() && text[split + 1] == b':' {
                return false;
            }
            split += 1;
        }
    }

    match (pat.first(), text.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => false,
        (Some(b'?'), Some(&c)) => {
            // `?` matches exactly one non-separator char.
            if c == b':' {
                false
            } else {
                glob_match_at(&pat[1..], &text[1..])
            }
        }
        (Some(&p), Some(&c)) => p == c && glob_match_at(&pat[1..], &text[1..]),
    }
}
