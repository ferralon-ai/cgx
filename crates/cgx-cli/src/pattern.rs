//! Parsing a user-supplied symbol string into a [`SymbolPattern`].
//!
//! Heuristic (deterministic, no flags needed for the common case):
//! - contains a glob metacharacter (`*`, `?`) → [`PatternKind::Glob`];
//! - contains a segment separator (`::` or `/`) → exact [`PatternKind::Fqn`];
//! - otherwise → [`PatternKind::ShortName`] (a bare `foo` matches any symbol
//!   whose last path segment is `foo`).
//!
//! `cgx-core` owns the matcher, so resolution is identical across CLI/MCP.
//!
//! ## Selector routing
//!
//! A newer, richer surface — the language-agnostic **node selector** engine
//! ([`cgx_select`]) — coexists with the legacy glob/`::`/short-name heuristic. The
//! two share one matcher core (a per-node predicate over the loaded view); only the
//! feeder differs. The anchor-resolution path in `main` is the chokepoint that
//! decides which surface an input wants, using the two predicates here:
//! [`looks_like_selector`] (selector-only grammar) and [`has_language_separator`]
//! (a native separator in any supported language). A single `*`/`?` glob without
//! selector grammar stays a legacy glob, so the common `auth::*` case is untouched.

use cgx_core::SymbolPattern;

/// Interpret a CLI symbol argument as a [`SymbolPattern`].
pub fn parse_symbol(text: &str) -> SymbolPattern {
    if text.contains('*') || text.contains('?') {
        SymbolPattern::glob(text)
    } else if cgx_core::pattern::contains_segment_separator(text) {
        SymbolPattern::fqn(text)
    } else {
        SymbolPattern::short_name(text)
    }
}

/// Does this input use selector-only grammar (globstar `**`, an alternation `(`,
/// or an affix negation `!`)? These three never occur in a legacy glob/fqn/short-
/// name string, so their presence unambiguously requests the selector engine. A
/// lone `*` is deliberately excluded — it stays a legacy glob.
pub fn looks_like_selector(text: &str) -> bool {
    text.contains("**") || text.contains('(') || text.contains('!')
}

/// Does this input carry a native separator in any supported language — Rust/C++
/// `::`, Go/scoped `/`, or the dotted families' `.`? A string with none is a bare
/// single identifier (a short-name). This is the signal that a non-glob input is a
/// multi-segment native symbol the selector engine should interpret, rather than a
/// short-name lookup; `.` is included (unlike [`cgx_core::pattern::
/// contains_segment_separator`]) precisely so native dotted symbols route to the
/// engine instead of falling through to a short-name that can never match.
pub fn has_language_separator(text: &str) -> bool {
    text.contains("::") || text.contains('/') || text.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgx_core::PatternKind;

    #[test]
    fn bare_name_is_short_name() {
        assert_eq!(parse_symbol("validate").kind, PatternKind::ShortName);
    }

    #[test]
    fn path_separated_is_fqn() {
        assert_eq!(parse_symbol("auth::validate").kind, PatternKind::Fqn);
    }

    #[test]
    fn slash_bearing_root_is_fqn_not_short_name() {
        // A `/`-bearing root (Go import path, TS scoped name) is an FQN, not a
        // short-name — the bug this surface previously fell through on.
        assert_eq!(parse_symbol("example.com/app/store").kind, PatternKind::Fqn);
        assert_eq!(parse_symbol("@acme/utils").kind, PatternKind::Fqn);
        // Regression: `::` → FQN, bare name → short-name (both unchanged).
        assert_eq!(parse_symbol("a::b").kind, PatternKind::Fqn);
        assert_eq!(parse_symbol("Open").kind, PatternKind::ShortName);
    }

    #[test]
    fn star_is_glob() {
        assert_eq!(parse_symbol("auth::*").kind, PatternKind::Glob);
    }

    #[test]
    fn double_star_is_glob() {
        assert_eq!(parse_symbol("**::sink").kind, PatternKind::Glob);
    }

    /// `looks_like_selector` fires only on selector-only grammar (`**`, `(`, `!`),
    /// never on a lone `*`/`?` glob or a plain `::`/short-name string.
    #[test]
    fn looks_like_selector_classification() {
        assert!(looks_like_selector("com::foo::**::*Service"));
        assert!(looks_like_selector("(foo|bar)::Svc"));
        assert!(looks_like_selector("com::foo::!Mock*Service"));
        assert!(looks_like_selector("**::sink"));
        assert!(!looks_like_selector("auth::validate"));
        assert!(!looks_like_selector("auth::*"));
        assert!(!looks_like_selector("validate"));
        assert!(!looks_like_selector("Parser*"));
    }

    /// `has_language_separator` distinguishes a multi-segment native symbol (which
    /// the engine should interpret) from a bare short-name. `.` counts — that is
    /// the fix that lets native dotted symbols reach the engine.
    #[test]
    fn has_language_separator_classification() {
        assert!(has_language_separator("auth::validate")); // Rust / C++
        assert!(has_language_separator("example.com/app/store")); // Go
        assert!(has_language_separator("com.example.Service")); // Java / Python / TS
        assert!(has_language_separator("@acme/utils")); // scoped
        assert!(!has_language_separator("validate")); // bare short-name
        assert!(!has_language_separator("Parser")); // bare short-name
    }
}
