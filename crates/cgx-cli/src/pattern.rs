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

use cgx_core::{normalize_pattern_text, SymbolPattern};

/// Interpret a CLI symbol argument as a [`SymbolPattern`].
///
/// The raw text is first passed through [`normalize_pattern_text`] so a native
/// separator (`.`) is accepted identically to canonical `::` (additive: an
/// existing `::` string is unchanged). A native Go receiver query — one whose
/// only parenthesized segments are receiver tokens `(*Ident)`/`(Ident)` — is an
/// **exact** FQN even though it contains `(`/`*`/`)`, so it is classified as
/// [`PatternKind::Fqn`] rather than a glob.
pub fn parse_symbol(text: &str) -> SymbolPattern {
    let norm = normalize_pattern_text(text);
    if is_native_receiver_query(&norm) {
        SymbolPattern::fqn(norm)
    } else if norm.contains('*') || norm.contains('?') {
        SymbolPattern::glob(norm)
    } else if norm.contains("::") {
        SymbolPattern::fqn(norm)
    } else {
        SymbolPattern::short_name(norm)
    }
}

/// Whether `text` is a native Go receiver query and nothing else: every
/// parenthesized segment is a receiver token `(` `*`? Ident `)` (a Go pointer or
/// value receiver, e.g. `(*Chain)` / `Chain`), and the string carries no real
/// selector/glob grammar — no `!` affix negation, no globstar `**`, no
/// alternation `|`, no stray parenthesis, and no `*`/`?` glob metacharacter
/// outside a receiver token.
///
/// This is the one guarded exception that lets a native pointer-receiver method
/// (`go.(*Chain).chain`) reach the FQN matcher instead of the [`cgx_select`]
/// engine, which otherwise claims `(` and `*`. It is deliberately narrow:
/// anything that could be a genuine selector (alternation, negation, globstar) or
/// a genuine glob returns `false` and routes unchanged. Separator-agnostic, so it
/// gives the same verdict on the raw (`go.(*Chain).chain`) and normalized
/// (`go::(*Chain)::chain`) forms.
fn is_native_receiver_query(text: &str) -> bool {
    if !text.contains('(') || text.contains('!') || text.contains("**") {
        return false;
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut saw_receiver = false;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                // Parse a receiver token: '(' '*'? ident ')'.
                let mut j = i + 1;
                if bytes.get(j) == Some(&b'*') {
                    j += 1;
                }
                let ident_start = j;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                // Empty identifier or an unterminated token is not a receiver.
                if j == ident_start || bytes.get(j) != Some(&b')') {
                    return false;
                }
                saw_receiver = true;
                i = j + 1;
            }
            // A stray close-paren, an alternation, or a glob metacharacter outside
            // a receiver token means this is not a pure receiver query.
            b')' | b'|' | b'*' | b'?' => return false,
            _ => i += 1,
        }
    }
    saw_receiver
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
    // A native Go receiver query (`go.(*Chain).chain`) contains `(` and `*`, which
    // `looks_like_selector` would otherwise claim for the selector engine. Detect
    // it FIRST and send it to the legacy FQN matcher; the guard is tight enough
    // that a genuine selector (alternation, `!`, globstar) never matches it.
    if is_native_receiver_query(text) {
        SymbolQuery::Legacy(parse_symbol(text))
    } else if looks_like_selector(text) {
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

    /// Native separators normalize to canonical `::` before classification, so a
    /// dotted query resolves to the same pattern as its `::` form.
    #[test]
    fn native_separators_normalize() {
        assert_eq!(parse_symbol("go.Chain"), SymbolPattern::fqn("go::Chain"));
        assert_eq!(
            parse_symbol("rust_sample.direct.chain"),
            SymbolPattern::fqn("rust_sample::direct::chain")
        );
        // A bare dotted glob still routes to glob, with separators canonicalized.
        assert_eq!(parse_symbol("auth.*"), SymbolPattern::glob("auth::*"));
        // A bare short name is untouched.
        assert_eq!(parse_symbol("validate"), SymbolPattern::short_name("validate"));
    }

    /// The highest-risk edit: a native Go pointer/value-receiver query is an exact
    /// FQN (not a glob, not a selector), and it routes to the legacy engine —
    /// while genuine selector/glob grammar keeps routing unchanged.
    #[test]
    fn go_receiver_query_is_exact_fqn_and_routes_legacy() {
        // Pointer receiver: `(` and `*` do NOT trigger the selector/glob engines.
        let p = parse_symbol("go.(*Chain).chain");
        assert_eq!(p, SymbolPattern::fqn("go::(*Chain)::chain"));
        assert_eq!(p.kind, PatternKind::Fqn);
        assert!(matches!(
            route_symbol("go.(*Chain).chain"),
            SymbolQuery::Legacy(_)
        ));
        // Value receiver.
        assert_eq!(
            parse_symbol("go.(Chain).chain"),
            SymbolPattern::fqn("go::(Chain)::chain")
        );
        // Already-canonical receiver form round-trips too.
        assert!(matches!(
            route_symbol("go::(*Chain)::chain"),
            SymbolQuery::Legacy(_)
        ));
    }

    /// Regression guard: real selector grammar must NOT be swallowed by the
    /// receiver heuristic — alternation, negation, and globstar still route to the
    /// selector engine even though some contain `(`.
    #[test]
    fn selector_grammar_not_shadowed_by_receiver_heuristic() {
        for s in [
            "(foo|bar)::Svc",     // alternation
            "com::foo::!Mock*",   // negation
            "com::foo::**::*Svc", // globstar
            "**::sink",
        ] {
            assert!(
                matches!(route_symbol(s), SymbolQuery::Selector(_)),
                "{s:?} must route to the selector engine, not the receiver path"
            );
            assert!(
                !is_native_receiver_query(s),
                "{s:?} must not be misread as a Go receiver query"
            );
        }
        // A lone `*` glob outside any receiver token is still a legacy glob, never
        // a receiver query.
        assert!(!is_native_receiver_query("auth::*"));
        assert!(matches!(route_symbol("auth::*"), SymbolQuery::Legacy(_)));
    }
}
