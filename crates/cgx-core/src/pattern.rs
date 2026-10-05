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
    /// Glob over the full FQN. Supports `*` (any run within one segment — stops at
    /// a segment separator, `::` or `/`) and `**` (any run, crossing separators),
    /// and `?` (one non-separator char).
    Glob,
    /// Match the short (last-segment) name exactly, where a segment is delimited by
    /// `::` or `/`.
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
        self.matches_fqn(&node.fqn)
    }

    /// Whether a bare FQN string matches this pattern. The FQN-only form of
    /// [`matches`](Self::matches) — every pattern kind is a pure predicate over the
    /// FQN, so callers that hold an FQN (e.g. the diff post-filters, which match a
    /// `DiffEdge`'s endpoint FQNs without a `NodeRecord`) can match without
    /// constructing a throwaway record.
    pub fn matches_fqn(&self, fqn: &str) -> bool {
        match self.kind {
            PatternKind::Fqn => fqn == self.text,
            PatternKind::ShortName => short_name(fqn) == self.text,
            PatternKind::Glob => glob_match(&self.text, fqn),
        }
    }
}

/// Length in bytes of a segment separator starting at `bytes[i]`, or `0` if none
/// starts there. The separators are the two-byte pair `::` and the single byte
/// `/`. This is the one shared boundary predicate: short-name splitting, glob
/// `*`/`?` expansion, and the CQL FQN-vs-short-name classifier all route through
/// it so the grammar stays consistent across every consumer.
///
/// `/` is language-blind here — it is a real boundary in Go import paths
/// (`example.com/app/store::Open`) and TS scoped names (`@acme/utils::Button`).
/// `.` is deliberately **not** a separator: it is common inside identifiers and
/// would shatter legitimate FQNs. (`.`↔`::` receiver rendering is a per-language
/// concern handled elsewhere.)
pub fn segment_separator_len(bytes: &[u8], i: usize) -> usize {
    match bytes.get(i) {
        Some(b'/') => 1,
        Some(b':') if bytes.get(i + 1) == Some(&b':') => 2,
        _ => 0,
    }
}

/// Whether `s` contains any segment separator (`::` or `/`). Used by the CQL
/// classifier to route a separator-bearing string to an FQN rather than a
/// short-name match.
pub fn contains_segment_separator(s: &str) -> bool {
    let bytes = s.as_bytes();
    (0..bytes.len()).any(|i| segment_separator_len(bytes, i) > 0)
}

/// The last segment of an FQN, where segments are delimited by `::` or `/`.
fn short_name(fqn: &str) -> &str {
    let bytes = fqn.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let sep = segment_separator_len(bytes, i);
        if sep > 0 {
            i += sep;
            start = i;
        } else {
            i += 1;
        }
    }
    // `start` always falls just after an ASCII separator (or at 0), so it is a
    // valid UTF-8 boundary.
    &fqn[start..]
}

/// A deterministic glob matcher over an FQN.
///
/// Tokens: `**` matches any run of characters, crossing segment separators (`::`
/// and `/`); `*` matches any run within a single segment (stops at `::` or `/`);
/// `?` matches exactly one non-separator character; every other character matches
/// literally. Pure backtracking; no regex dependency.
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
            // Stop expanding `*` at a segment separator boundary (`::` or `/`).
            if segment_separator_len(text, split) > 0 {
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
            // `?` matches exactly one non-separator char — stops at `:` (either
            // half of `::`) and at `/`.
            if c == b':' || c == b'/' {
                false
            } else {
                glob_match_at(&pat[1..], &text[1..])
            }
        }
        (Some(&p), Some(&c)) => p == c && glob_match_at(&pat[1..], &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_separator_len_recognizes_double_colon_and_slash() {
        assert_eq!(segment_separator_len(b"a::b", 1), 2);
        assert_eq!(segment_separator_len(b"a/b", 1), 1);
        assert_eq!(segment_separator_len(b"a:b", 1), 0); // lone colon is not a boundary
        assert_eq!(segment_separator_len(b"a.b", 1), 0); // `.` is deliberately not a boundary
    }

    #[test]
    fn contains_segment_separator_covers_both_and_excludes_dot() {
        assert!(contains_segment_separator("a::b"));
        assert!(contains_segment_separator("example.com/app/store"));
        assert!(contains_segment_separator("@acme/utils"));
        assert!(!contains_segment_separator("Open"));
        assert!(!contains_segment_separator("example.com")); // `.` alone is not a separator
    }

    #[test]
    fn short_name_takes_last_segment_after_slash_or_colons() {
        let m = |text: &str, fqn: &str| SymbolPattern::short_name(text).matches_fqn(fqn);
        assert!(m("Open", "example.com/app/store::Open"));
        assert!(m("store", "example.com/app/store"));
        assert!(m("Button", "@acme/utils::Button"));
        // Pure `::` behavior unchanged.
        assert!(m("c", "a::b::c"));
        assert!(!m("b", "a::b::c"));
    }

    #[test]
    fn single_star_stops_at_slash_but_double_star_crosses() {
        // `*` matches exactly one path component, not across `/`.
        assert!(SymbolPattern::glob("example.com/*::Open").matches_fqn("example.com/app::Open"));
        assert!(!SymbolPattern::glob("example.com/*::Open")
            .matches_fqn("example.com/app/store::Open"));
        // `**` crosses `/` (and `::`).
        assert!(SymbolPattern::glob("example.com/**::Open")
            .matches_fqn("example.com/app/store::Open"));
        // `*` stops at `/` in a scoped name.
        assert!(SymbolPattern::glob("@acme/*").matches_fqn("@acme/utils"));
        assert!(!SymbolPattern::glob("@acme/*").matches_fqn("@acme/utils/sub"));
    }

    #[test]
    fn single_star_still_stops_at_double_colon() {
        // Pre-existing `::` boundary behavior must be unchanged.
        assert!(SymbolPattern::glob("a::*").matches_fqn("a::b"));
        assert!(!SymbolPattern::glob("a::*").matches_fqn("a::b::c"));
        assert!(SymbolPattern::glob("a::**").matches_fqn("a::b::c"));
    }

    #[test]
    fn question_mark_stops_at_separators() {
        assert!(SymbolPattern::glob("a?c").matches_fqn("abc"));
        assert!(!SymbolPattern::glob("a?b").matches_fqn("a/b")); // `?` won't match `/`
        assert!(!SymbolPattern::glob("a?b").matches_fqn("a:b")); // nor a colon
    }
}
