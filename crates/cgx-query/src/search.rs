//! `cgx search`: a pure node-table scan that resolves a partial/half-remembered
//! name to exact FQNs (architecture §4, but no graph walk).
//!
//! Unlike the callers/callees/reaches/paths surface, `search` never touches an
//! edge: it filters the symbol-node set ([`GraphView::nodes`] — the same iterator
//! `unused` enumerates) by a name predicate plus an optional kind, and returns the
//! matching definitions in FQN order. Default match is a case-insensitive
//! substring against the whole FQN (matches anywhere); `--regex` compiles the
//! pattern once and matches the whole FQN as a regex.

use cgx_core::SymbolKind;

use crate::view::GraphView;

/// One symbol matched by [`search_symbols`]. Carries the resolved definition by
/// value so the caller can render it without re-reading the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolHit {
    pub fqn: String,
    pub file: String,
    pub line: u32,
    pub kind: SymbolKind,
}

/// A rejected search request — an invalid `--regex` pattern or an empty pattern.
/// Carries the full, already-formatted message; the CLI maps it to the usage exit
/// code (2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError(pub String);

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SearchError {}

/// A compiled name predicate over the whole FQN. `--all` carries none (every node
/// passes); a pattern is either a compiled regex or a lowercased substring needle.
enum NamePredicate {
    Regex(regex::Regex),
    Substring(String),
}

/// How [`search_symbols`] selects nodes: an explicit pattern, or the `--all`
/// match-everything mode (B-1). Keeping both modes in one enum means they share the
/// single kind-filter + sort + collect path below — `--all` is *exactly* a
/// match-all pattern, just spelled explicitly instead of via the unobvious `.`
/// regex idiom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMatch<'a> {
    /// `--all`: every symbol, no name predicate (only the optional `--kind`).
    All,
    /// A name predicate over the whole FQN. `regex == false` is a case-insensitive
    /// substring (matches anywhere); `regex == true` is an unanchored regex.
    Pattern { pattern: &'a str, regex: bool },
}

/// Scan the node table for symbols matching `selector`, optionally narrowed to one
/// [`SymbolKind`]. A pure node scan — no edge is read.
///
/// - [`SearchMatch::Pattern`] with `regex == false`: case-insensitive **substring**
///   match against the whole FQN (matches anywhere).
/// - [`SearchMatch::Pattern`] with `regex == true`: `pattern` is compiled once and
///   matched (unanchored) against the whole FQN; an invalid pattern returns
///   [`SearchError`]; an empty pattern is rejected (it would smuggle "list
///   everything" back in — that is `--all`'s job).
/// - [`SearchMatch::All`]: every node, no name predicate.
///
/// Results are returned sorted by FQN (with `(file, line)` as a deterministic
/// tiebreak for identically-named symbols), so output is byte-identical across
/// runs. A no-match returns an empty vec — never an error.
pub fn search_symbols(
    view: &GraphView,
    selector: SearchMatch<'_>,
    kind_filter: Option<SymbolKind>,
) -> Result<Vec<SymbolHit>, SearchError> {
    let predicate: Option<NamePredicate> = match selector {
        SearchMatch::All => None,
        SearchMatch::Pattern { pattern, regex } => {
            // An empty *provided* pattern would match every symbol — the "dump the
            // whole table" path. That is `--all`'s job now; reject the empty pattern
            // so it can't smuggle "list everything" back in unobservably.
            if pattern.is_empty() {
                return Err(SearchError("pattern must not be empty".to_string()));
            }
            if regex {
                Some(NamePredicate::Regex(
                    regex::Regex::new(pattern)
                        .map_err(|e| SearchError(format!("invalid regex: {e}")))?,
                ))
            } else {
                Some(NamePredicate::Substring(pattern.to_lowercase()))
            }
        }
    };

    let mut hits: Vec<SymbolHit> = view
        .nodes()
        .iter()
        .filter(|node| kind_filter.is_none_or(|k| node.kind == k))
        .filter(|node| match &predicate {
            None => true,
            Some(NamePredicate::Regex(re)) => re.is_match(&node.fqn),
            Some(NamePredicate::Substring(needle)) => node.fqn.to_lowercase().contains(needle),
        })
        .map(|node| SymbolHit {
            fqn: node.fqn.clone(),
            file: node.file.clone(),
            line: node.line_start,
            kind: node.kind,
        })
        .collect();

    hits.sort_by(|a, b| {
        a.fqn
            .cmp(&b.fqn)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgx_core::{NodeId, NodeRecord, Visibility};

    fn node(id: u32, fqn: &str, file: &str, line: u32, kind: SymbolKind) -> NodeRecord {
        NodeRecord {
            id: NodeId(id),
            kind,
            fqn: fqn.to_string(),
            file: file.to_string(),
            line_start: line,
            line_end: line,
            lang: "rust".to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: Default::default(),
            transitive_effects: Default::default(),
            unresolved_calls: 0,
        }
    }

    fn sample_view() -> GraphView {
        let nodes = vec![
            node(0, "app::parse_input", "src/parse.rs", 10, SymbolKind::Function),
            node(1, "app::Parser", "src/parse.rs", 3, SymbolKind::Type),
            node(2, "app::http::send_request", "src/http.rs", 20, SymbolKind::Function),
            node(3, "app::http::Client", "src/http.rs", 5, SymbolKind::Type),
        ];
        GraphView::new(nodes, vec![], vec![])
    }

    /// Build a substring [`SearchMatch::Pattern`] (the default, non-regex mode).
    fn pat(p: &str) -> SearchMatch<'_> {
        SearchMatch::Pattern {
            pattern: p,
            regex: false,
        }
    }

    /// Build a regex [`SearchMatch::Pattern`].
    fn re(p: &str) -> SearchMatch<'_> {
        SearchMatch::Pattern {
            pattern: p,
            regex: true,
        }
    }

    #[test]
    fn substring_match_is_case_insensitive_and_anywhere() {
        let view = sample_view();
        let hits = search_symbols(&view, pat("PARSE"), None).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        // Matches `app::parse_input` (anywhere) and `app::Parser` (case-insensitive).
        assert_eq!(fqns, vec!["app::Parser", "app::parse_input"]);
    }

    #[test]
    fn regex_match_over_whole_fqn() {
        let view = sample_view();
        let hits = search_symbols(&view, re(r"^app::http::.*"), None).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        assert_eq!(fqns, vec!["app::http::Client", "app::http::send_request"]);
    }

    #[test]
    fn invalid_regex_is_an_error_not_a_panic() {
        let view = sample_view();
        let err = search_symbols(&view, re("(unclosed"), None);
        assert!(err.is_err(), "an unclosed group is an invalid regex");
    }

    #[test]
    fn kind_filter_narrows_to_one_kind() {
        let view = sample_view();
        let hits = search_symbols(&view, pat("app"), Some(SymbolKind::Type)).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        assert_eq!(fqns, vec!["app::Parser", "app::http::Client"]);
    }

    #[test]
    fn empty_pattern_is_rejected_not_a_full_dump() {
        let view = sample_view();
        let err = search_symbols(&view, pat(""), None);
        assert!(
            err.is_err(),
            "an empty pattern is a usage error, not a list-everything dump"
        );
    }

    #[test]
    fn no_match_returns_empty_not_error() {
        let view = sample_view();
        let hits = search_symbols(&view, pat("zzz_nonexistent"), None).unwrap();
        assert!(hits.is_empty(), "no match → empty vec, not an error");
    }

    #[test]
    fn results_are_sorted_by_fqn() {
        let view = sample_view();
        let hits = search_symbols(&view, pat("app"), None).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        let mut sorted = fqns.clone();
        sorted.sort();
        assert_eq!(fqns, sorted, "results come back in FQN order");
    }

    #[test]
    fn all_returns_every_symbol_no_pattern_needed() {
        let view = sample_view();
        let hits = search_symbols(&view, SearchMatch::All, None).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        // Every node, sorted by FQN — the explicit, discoverable match-everything.
        assert_eq!(
            fqns,
            vec![
                "app::Parser",
                "app::http::Client",
                "app::http::send_request",
                "app::parse_input",
            ]
        );
    }

    #[test]
    fn all_still_honors_the_kind_filter() {
        let view = sample_view();
        let hits = search_symbols(&view, SearchMatch::All, Some(SymbolKind::Type)).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        assert_eq!(fqns, vec!["app::Parser", "app::http::Client"]);
    }
}
