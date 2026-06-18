//! The single CQL error type, with a `Display` that renders a source line and a
//! caret under the offending span (design §7).
//!
//! ```text
//! error: unknown edge type `RESOLVES_TO`
//!   --> query:1:13
//!    |
//!  1 | MATCH (a)-[:RESOLVES_TO]->(b) RETURN a
//!    |             ^^^^^^^^^^^^ not supported in this release
//! ```
//!
//! `Display` alone has no source text, so it renders the headline only; call
//! [`CqlError::render`] with the query string to produce the framed caret view.

use std::fmt;
use std::ops::Range;

/// The phase that produced the error. Determines the headline verb and (at the
/// CLI) the process exit code (design §7: Parse/Plan → usage, Eval-load → graph).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Unexpected token / EOF — a lexical or grammatical failure.
    Parse,
    /// Recognised-but-unsupported feature, or a property/edge type with no
    /// backing field. The message names the feature and that it is deferred.
    Plan,
    /// A runtime failure: unresolved symbol, ambiguous FQN, etc.
    Eval,
}

impl ErrorKind {
    fn label(self) -> &'static str {
        match self {
            ErrorKind::Parse => "parse error",
            ErrorKind::Plan => "plan error",
            ErrorKind::Eval => "eval error",
        }
    }
}

/// A CQL error carrying the byte span it points at, its phase, and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CqlError {
    pub span: Range<usize>,
    pub kind: ErrorKind,
    pub message: String,
}

impl CqlError {
    pub fn new(span: Range<usize>, kind: ErrorKind, message: String) -> Self {
        CqlError { span, kind, message }
    }

    pub fn parse(span: Range<usize>, message: impl Into<String>) -> Self {
        CqlError::new(span, ErrorKind::Parse, message.into())
    }

    pub fn plan(span: Range<usize>, message: impl Into<String>) -> Self {
        CqlError::new(span, ErrorKind::Plan, message.into())
    }

    pub fn eval(span: Range<usize>, message: impl Into<String>) -> Self {
        CqlError::new(span, ErrorKind::Eval, message.into())
    }

    /// Render the framed, caret-annotated view against the original `src`.
    ///
    /// The span is clamped to `src`'s bounds so a stale or out-of-range span can
    /// never panic. A zero-width span (e.g. EOF) still draws a single caret.
    pub fn render(&self, src: &str) -> String {
        let len = src.len();
        let start = self.span.start.min(len);
        let end = self.span.end.clamp(start, len);

        // Locate the line containing `start`.
        let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = src[start..].find('\n').map_or(len, |i| start + i);
        let line_no = src[..start].bytes().filter(|&b| b == b'\n').count() + 1;
        let col = start - line_start + 1;
        let line_text = &src[line_start..line_end];

        // The caret run: one `^` per byte of the span on this line, min one.
        let caret_offset = start - line_start;
        let caret_len = (end.min(line_end) - start).max(1);

        let gutter = line_no.to_string();
        let pad = " ".repeat(gutter.len());

        let mut out = String::new();
        out.push_str(&format!("error: {}\n", self.message));
        out.push_str(&format!("{pad}--> query:{line_no}:{col}\n"));
        out.push_str(&format!("{pad} |\n"));
        out.push_str(&format!("{gutter} | {line_text}\n"));
        out.push_str(&format!("{pad} | {}{}", " ".repeat(caret_offset), "^".repeat(caret_len)));
        out
    }
}

impl fmt::Display for CqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} (at bytes {}..{})", self.kind.label(), self.message, self.span.start, self.span.end)
    }
}

impl std::error::Error for CqlError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_golden() {
        let src = "MATCH (a)-[:RESOLVES_TO]->(b) RETURN a";
        let err = CqlError::plan(12..24, "unknown edge type `RESOLVES_TO`");
        let rendered = err.render(src);
        let expected = "\
error: unknown edge type `RESOLVES_TO`
 --> query:1:13
  |
1 | MATCH (a)-[:RESOLVES_TO]->(b) RETURN a
  |             ^^^^^^^^^^^^";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn caret_on_second_line() {
        let src = "MATCH (a)\nWHERE a.col = 1\nRETURN a";
        // `col` begins at byte 18.
        let err = CqlError::plan(18..21, "unknown node property `col`");
        let rendered = err.render(src);
        let expected = "\
error: unknown node property `col`
 --> query:2:9
  |
2 | WHERE a.col = 1
  |         ^^^";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn zero_width_span_draws_one_caret() {
        let src = "RETURN";
        let err = CqlError::parse(6..6, "unexpected end of input");
        let rendered = err.render(src);
        assert!(rendered.contains("^"));
        assert!(rendered.ends_with("^"));
    }

    #[test]
    fn out_of_range_span_does_not_panic() {
        let src = "MATCH";
        let err = CqlError::parse(100..200, "x");
        let _ = err.render(src);
    }
}
