//! Parsing a user-supplied symbol string into a [`SymbolPattern`].
//!
//! Heuristic (deterministic, no flags needed for the common case):
//! - contains a glob metacharacter (`*`, `?`) → [`PatternKind::Glob`];
//! - contains `::` → exact [`PatternKind::Fqn`];
//! - otherwise → [`PatternKind::ShortName`] (a bare `foo` matches any symbol
//!   whose last path segment is `foo`).
//!
//! `cgx-core` owns the matcher, so resolution is identical across CLI/MCP.
//!
//! ## Selector routing
//!
//! A newer, richer surface — the language-agnostic **node selector** engine
//! ([`cgx_select`]) — coexists with the legacy glob/`::`/short-name heuristic. The
//! two share one matcher core (a per-node predicate over the loaded view); only
//! the feeder differs. [`route_symbol`] is the single chokepoint that decides
//! which surface an input string wants: a string carrying selector-only grammar
//! (`**`, `(`, or `!`) is a [`SymbolQuery::Selector`] compiled by the engine; every
//! other string falls through to [`parse_symbol`]'s heuristic **unchanged**. A
//! single `*` alone is *not* a routing trigger — it stays a legacy glob, so the
//! common `auth::*` case is untouched.

use cgx_core::SymbolPattern;

/// Interpret a CLI symbol argument as a [`SymbolPattern`].
pub fn parse_symbol(text: &str) -> SymbolPattern {
    if text.contains('*') || text.contains('?') {
        SymbolPattern::glob(text)
    } else if text.contains("::") {
        SymbolPattern::fqn(text)
    } else {
        SymbolPattern::short_name(text)
    }
}

/// The two symbol-resolution surfaces a CLI string can route to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolQuery {
    /// The legacy glob/`::`/short-name heuristic ([`parse_symbol`]).
    Legacy(SymbolPattern),
    /// The node-selector engine ([`cgx_select`]). Carries the raw selector source;
    /// the caller compiles it (so a compile error can be surfaced as a labeled
    /// usage error, never a silent empty result) and evaluates it via
    /// `GraphView::resolve_select`.
    Selector(String),
}

/// Does this input use selector-only grammar (globstar `**`, an alternation `(`,
/// or an affix negation `!`)? These three never occur in a legacy glob/fqn/short-
/// name string, so their presence unambiguously requests the selector engine. A
/// lone `*` is deliberately excluded — it stays a legacy glob.
pub fn looks_like_selector(text: &str) -> bool {
    text.contains("**") || text.contains('(') || text.contains('!')
}

/// Route a CLI symbol string to the surface its grammar requests: the selector
/// engine for selector-only grammar, else the legacy [`parse_symbol`] heuristic.
pub fn route_symbol(text: &str) -> SymbolQuery {
    if looks_like_selector(text) {
        SymbolQuery::Selector(text.to_string())
    } else {
        SymbolQuery::Legacy(parse_symbol(text))
    }
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
    fn star_is_glob() {
        assert_eq!(parse_symbol("auth::*").kind, PatternKind::Glob);
    }

    #[test]
    fn double_star_is_glob() {
        assert_eq!(parse_symbol("**::sink").kind, PatternKind::Glob);
    }

    /// The router sends selector-grammar strings to the engine and everything else
    /// to the legacy heuristic. `parse_symbol` itself is left untouched (the two
    /// `double_star_is_glob`/`star_is_glob` cases above still hold for it).
    #[test]
    fn route_symbol_classification() {
        struct Case {
            input: &'static str,
            selector: bool,
        }
        let cases = [
            // Selector-only grammar → engine.
            Case {
                input: "com::foo::**::*Service",
                selector: true,
            },
            Case {
                input: "(foo|bar)::Svc",
                selector: true,
            },
            Case {
                input: "com::foo::!Mock*Service",
                selector: true,
            },
            Case {
                input: "**::sink",
                selector: true,
            },
            // Legacy heuristic → unchanged.
            Case {
                input: "auth::validate",
                selector: false,
            },
            Case {
                input: "auth::*",
                selector: false,
            },
            Case {
                input: "validate",
                selector: false,
            },
            Case {
                input: "Parser*",
                selector: false,
            },
        ];
        for c in cases {
            let routed = route_symbol(c.input);
            let is_selector = matches!(routed, SymbolQuery::Selector(_));
            assert_eq!(
                is_selector, c.selector,
                "route_symbol({:?}) selector-routed = {is_selector}, want {}",
                c.input, c.selector
            );
            // Legacy routes carry the same pattern parse_symbol would have produced.
            if let SymbolQuery::Legacy(p) = routed {
                assert_eq!(p, parse_symbol(c.input));
            }
        }
    }
}
