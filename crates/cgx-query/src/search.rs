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

/// Scan the node table for symbols whose FQN matches `pattern`, optionally
/// narrowed to one [`SymbolKind`]. A pure node scan — no edge is read.
///
/// - `regex == false`: case-insensitive **substring** match against the whole FQN
///   (matches anywhere).
/// - `regex == true`: `pattern` is compiled once and matched (unanchored) against
///   the whole FQN; an invalid pattern returns [`SearchError`].
///
/// Results are returned sorted by FQN (with `(file, line)` as a deterministic
/// tiebreak for identically-named symbols), so output is byte-identical across
/// runs. A no-match returns an empty vec — never an error.
pub fn search_symbols(
    view: &GraphView,
    pattern: &str,
    regex: bool,
    kind_filter: Option<SymbolKind>,
) -> Result<Vec<SymbolHit>, SearchError> {
    // An empty pattern would match every symbol — the "dump the whole table" path
    // the spec puts out of scope (search is name-only; a bare `cgx search` is
    // already a usage error). Reject it so an empty *provided* pattern can't
    // smuggle "list everything" back in.
    if pattern.is_empty() {
        return Err(SearchError("pattern must not be empty".to_string()));
    }
    let re = if regex {
        Some(
            regex::Regex::new(pattern)
                .map_err(|e| SearchError(format!("invalid regex: {e}")))?,
        )
    } else {
        None
    };
    let needle = pattern.to_lowercase();

    let mut hits: Vec<SymbolHit> = view
        .nodes()
        .iter()
        .filter(|node| kind_filter.map_or(true, |k| node.kind == k))
        .filter(|node| match &re {
            Some(re) => re.is_match(&node.fqn),
            None => node.fqn.to_lowercase().contains(&needle),
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

    #[test]
    fn substring_match_is_case_insensitive_and_anywhere() {
        let view = sample_view();
        let hits = search_symbols(&view, "PARSE", false, None).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        // Matches `app::parse_input` (anywhere) and `app::Parser` (case-insensitive).
        assert_eq!(fqns, vec!["app::Parser", "app::parse_input"]);
    }

    #[test]
    fn regex_match_over_whole_fqn() {
        let view = sample_view();
        let hits = search_symbols(&view, r"^app::http::.*", true, None).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        assert_eq!(fqns, vec!["app::http::Client", "app::http::send_request"]);
    }

    #[test]
    fn invalid_regex_is_an_error_not_a_panic() {
        let view = sample_view();
        let err = search_symbols(&view, "(unclosed", true, None);
        assert!(err.is_err(), "an unclosed group is an invalid regex");
    }

    #[test]
    fn kind_filter_narrows_to_one_kind() {
        let view = sample_view();
        let hits = search_symbols(&view, "app", false, Some(SymbolKind::Type)).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        assert_eq!(fqns, vec!["app::Parser", "app::http::Client"]);
    }

    #[test]
    fn empty_pattern_is_rejected_not_a_full_dump() {
        let view = sample_view();
        let err = search_symbols(&view, "", false, None);
        assert!(
            err.is_err(),
            "an empty pattern is a usage error, not a list-everything dump"
        );
    }

    #[test]
    fn no_match_returns_empty_not_error() {
        let view = sample_view();
        let hits = search_symbols(&view, "zzz_nonexistent", false, None).unwrap();
        assert!(hits.is_empty(), "no match → empty vec, not an error");
    }

    #[test]
    fn results_are_sorted_by_fqn() {
        let view = sample_view();
        let hits = search_symbols(&view, "app", false, None).unwrap();
        let fqns: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
        let mut sorted = fqns.clone();
        sorted.sort();
        assert_eq!(fqns, sorted, "results come back in FQN order");
    }
}
