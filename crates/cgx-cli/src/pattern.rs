//! Parsing a user-supplied symbol string into a [`SymbolPattern`].
//!
//! Heuristic (deterministic, no flags needed for the common case):
//! - contains a glob metacharacter (`*`, `?`) → [`PatternKind::Glob`];
//! - contains `::` → exact [`PatternKind::Fqn`];
//! - otherwise → [`PatternKind::ShortName`] (a bare `foo` matches any symbol
//!   whose last path segment is `foo`).
//!
//! `cgx-core` owns the matcher, so resolution is identical across CLI/MCP.

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
}
