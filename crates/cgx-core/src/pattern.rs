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

/// The last `::`-delimited segment of an FQN.
fn short_name(fqn: &str) -> &str {
    fqn.rsplit("::").next().unwrap_or(fqn)
}

/// Render a canonical `::`-joined FQN in its source language's native symbol
/// grammar, keyed on the symbol's **own** `lang`.
///
/// - `rust` (and any unknown language) → returned unchanged (`::` is Rust's own
///   grammar, and identity is the safe default for a tag we don't model).
/// - `go` | `java` | `python` | `typescript` → each joins segments with `.`, so
///   the transform is a `::` → `.` separator remap. The language-specific segment
///   shape (e.g. Go's embedded receiver token `(*Type)` / `Type`) already lives in
///   the FQN segments, so `go::(*Chain)::chain` → `go.(*Chain).chain` needs no
///   special-casing.
///
/// Pure display transform: the canonical stored FQN is never altered. The inverse
/// (native → canonical) for queries is [`normalize_pattern_text`], which is
/// language-blind on purpose.
pub fn render_fqn(fqn: &str, lang: &str) -> String {
    match lang {
        // "ts" accepted as an alias though the adapter stamps "typescript".
        "go" | "java" | "python" | "typescript" | "ts" => fqn.replace("::", "."),
        _ => fqn.to_string(),
    }
}

/// Normalize a user-supplied symbol pattern to canonical separator form.
///
/// Language-blind: treat both `.` and `::` (and any run mixing them) as one
/// segment separator, collapsing each run to a single canonical `::`. Glob
/// metacharacters (`*`, `?`) and every other character — including a Go receiver
/// token's `(`, `*`, `)` — are preserved verbatim. `a.b.c` → `a::b::c`;
/// `auth.*` → `auth::*`; `a::b` unchanged; `go.(*Chain).chain` →
/// `go::(*Chain)::chain`.
///
/// Applied to the raw query string *before* [`SymbolPattern`] construction, so a
/// native query in any language's separator resolves against the canonical FQN
/// (matching is over `::`). Purely additive: an existing `::` input is unchanged.
pub fn normalize_pattern_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == '.' || c == ':' {
            // Collapse the maximal run of separator characters to one "::".
            while matches!(chars.peek(), Some('.') | Some(':')) {
                chars.next();
            }
            out.push_str("::");
        } else {
            out.push(c);
            chars.next();
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_fqn_per_language() {
        struct Case {
            lang: &'static str,
            canonical: &'static str,
            native: &'static str,
        }
        let cases = [
            // Rust keeps `::` (identity).
            Case {
                lang: "rust",
                canonical: "rust_sample::direct::Counter::add_one",
                native: "rust_sample::direct::Counter::add_one",
            },
            // Go: `::` → `.`; pointer receiver token survives in-segment.
            Case {
                lang: "go",
                canonical: "go::(*Chain)::chain",
                native: "go.(*Chain).chain",
            },
            // Go value receiver.
            Case {
                lang: "go",
                canonical: "go::Chain::chain",
                native: "go.Chain.chain",
            },
            // Go bare type.
            Case {
                lang: "go",
                canonical: "go::Chain",
                native: "go.Chain",
            },
            Case {
                lang: "java",
                canonical: "com::example::direct::Direct::chain",
                native: "com.example.direct.Direct.chain",
            },
            Case {
                lang: "python",
                canonical: "fixtures::python::direct_chain::step_a",
                native: "fixtures.python.direct_chain.step_a",
            },
            Case {
                lang: "typescript",
                canonical: "mod::Class::method",
                native: "mod.Class.method",
            },
            // Unknown language → identity (safe default).
            Case {
                lang: "cobol",
                canonical: "a::b::c",
                native: "a::b::c",
            },
        ];
        for c in cases {
            assert_eq!(
                render_fqn(c.canonical, c.lang),
                c.native,
                "render_fqn({:?}, {:?})",
                c.canonical,
                c.lang
            );
        }
    }

    #[test]
    fn normalize_pattern_text_cases() {
        let cases = [
            ("a.b.c", "a::b::c"),
            ("a::b", "a::b"),
            ("a::b::c", "a::b::c"),
            ("auth.*", "auth::*"),
            ("auth::*", "auth::*"),
            // Glob metachars survive, including globstar.
            ("**::sink", "**::sink"),
            ("Parser*", "Parser*"),
            ("valid?te", "valid?te"),
            // Mixed / repeated separators collapse to one `::`.
            ("a.::b", "a::b"),
            ("a...b", "a::b"),
            // Bare short name unchanged.
            ("validate", "validate"),
            // Go native receiver form → canonical; receiver token preserved.
            ("go.(*Chain).chain", "go::(*Chain)::chain"),
            ("go.Chain.chain", "go::Chain::chain"),
        ];
        for (input, want) in cases {
            assert_eq!(
                normalize_pattern_text(input),
                want,
                "normalize_pattern_text({input:?})"
            );
        }
    }

    /// The load-bearing invariant: a native string cgx renders must re-parse
    /// (via normalize) back to the exact canonical FQN it came from.
    #[test]
    fn render_then_normalize_round_trips_to_canonical() {
        let fqns = [
            ("go", "go::(*Chain)::chain"),
            ("go", "go::Chain::chain"),
            ("java", "com::example::Direct::chain"),
            ("python", "fixtures::python::direct_chain::step_a"),
            ("typescript", "mod::Class::method"),
            ("rust", "rust_sample::direct::Counter::add_one"),
        ];
        for (lang, canonical) in fqns {
            let native = render_fqn(canonical, lang);
            assert_eq!(
                normalize_pattern_text(&native),
                canonical,
                "round-trip {lang} {canonical} via native {native:?}"
            );
        }
    }
}
