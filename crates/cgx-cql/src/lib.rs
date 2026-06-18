//! # cgx-cql
//!
//! The Layer-2 Cypher-subset query language. A **frontend** that lowers `MATCH`
//! patterns onto the Layer-1 primitives (`cgx_query::GraphView::neighbors` +
//! `PathWalker` + `EdgeFilter`) rather than a second execution path — so the
//! transience / spawn-domain semantics (GM-4 / GM-9.2) come for free, and the
//! engine stays deterministic end-to-end (design §0, §3.5).
//!
//! ## Public contract (frozen at P1)
//!
//! The CLI and any other embedder build against exactly two items:
//!
//! - [`run`] — parse, plan, evaluate a query string against a [`GraphView`],
//!   yielding a [`ResultTable`] or a [`CqlError`].
//! - [`ResultTable`] — ordered column names plus ordered rows of [`Value`], with
//!   an optional path channel so a `RETURN path` query can feed a graph emitter.
//!
//! This signature and these shapes are stable; later phases fill in lowering and
//! evaluation behind them without changing the contract.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod ast;
pub mod error;
pub mod eval;
pub mod lower;
pub mod parser;
pub mod token;
pub mod value;

pub use error::{CqlError, ErrorKind};
pub use value::{PathValue, Value};

use cgx_query::GraphView;

/// The result of a successful query: a deterministic table.
///
/// `columns` are the RETURN item names (alias, else the rendered expression).
/// Each row in `rows` has `columns.len()` values, in column order. When the
/// query returned whole paths, `paths` carries the corresponding [`PathValue`]s
/// in row order so the CLI can pick a path/graph emitter (dot/mermaid/d2);
/// tabular queries leave it empty.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub paths: Vec<PathValue>,
}

/// Parse, plan, and evaluate `src` against `view`.
///
/// Errors carry a byte span into `src`; render them with
/// [`CqlError::render`](crate::error::CqlError::render).
pub fn run(view: &GraphView, src: &str) -> Result<ResultTable, CqlError> {
    let query = parser::parse(src)?;
    eval::eval(view, &query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgx_query::GraphView;

    #[test]
    fn run_lexes_then_reports_unimplemented() {
        let view = GraphView::new(Vec::new(), Vec::new(), Vec::new());
        let err = run(&view, "MATCH (a) RETURN a").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Plan);
    }

    #[test]
    fn run_surfaces_lex_errors() {
        let view = GraphView::new(Vec::new(), Vec::new(), Vec::new());
        let err = run(&view, r#"MATCH (a) WHERE a.name = "unterminated"#).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn result_table_default_is_empty() {
        let t = ResultTable::default();
        assert!(t.columns.is_empty() && t.rows.is_empty() && t.paths.is_empty());
    }
}
