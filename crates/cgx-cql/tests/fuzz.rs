//! proptest-based fuzz tests for the tokenizer and parser (design §8 / P10).
//!
//! Two properties:
//!
//! 1. **Tokenizer no-panic + span validity on arbitrary ASCII input.**
//!    The tokenizer must not panic on any input and, when it succeeds, every
//!    token span must be a valid half-open range within the input.
//!
//! 2. **Parser no-panic / clean-error on arbitrary token-ish strings.**
//!    The recursive-descent parser (including the Pratt expression sub-parser)
//!    must never panic, must never overflow the stack, and must return either a
//!    valid `Query` or a `CqlError` of one of the three known `ErrorKind`s.
//!    This exercises the recursive descent for stack safety.

mod common;

use cgx_cql::{run, ErrorKind};
use proptest::prelude::*;

// ── 1. Tokenizer no-panic + span validity ─────────────────────────────────────

proptest! {
    /// For any printable-ASCII string the tokenizer must not panic. When it
    /// returns `Ok`, every span must satisfy `start <= end <= src.len()`.
    #[test]
    fn tokenizer_no_panic_and_valid_spans(s in "[ -~]{0,128}") {
        match cgx_cql::token::tokenize(&s) {
            Ok(tokens) => {
                for tok in &tokens {
                    prop_assert!(
                        tok.span.start <= tok.span.end,
                        "span start > end: {:?}", tok
                    );
                    prop_assert!(
                        tok.span.end <= s.len(),
                        "span end out of bounds: {:?} for input len {}", tok, s.len()
                    );
                }
            }
            Err(e) => {
                // A lex error must carry a valid span into the source.
                prop_assert!(
                    e.span.start <= e.span.end,
                    "error span start > end: {:?}", e.span
                );
                prop_assert!(
                    e.span.end <= s.len(),
                    "error span end out of bounds: {:?} for input len {}", e.span, s.len()
                );
            }
        }
    }
}

// ── 2. Parser no-panic / clean-error ─────────────────────────────────────────

/// Build a "token-ish" strategy: strings made from the tokens and punctuation
/// the CQL grammar uses. This is more likely to produce partially-valid queries
/// than pure random ASCII while still exercising all error paths.
fn token_ish() -> impl Strategy<Value = String> {
    // Use a weighted mix of CQL keywords, identifier-like fragments, and
    // structural punctuation. No escape sequences — we just want shape, not
    // necessarily strings that the lexer can parse cleanly.
    let words = prop::sample::select(vec![
        "MATCH", "WHERE", "RETURN", "WITH", "CALL", "YIELD", "OPTIONAL",
        "UNION", "CREATE", "SET", "DELETE", "UNWIND", "MATCH", "RETURN",
        "ORDER", "BY", "AS", "DISTINCT", "LIMIT", "AND", "OR", "NOT",
        "IN", "NONE", "ANY", "ALL", "CALLS", "DATA_FLOW", "MEMBER_OF",
        "RESOLVES_TO", "PROVIDES_BODY", "COMPATIBLE_WITH", "FULFILLS",
        "a", "b", "n", "r", "p", "x", "foo", "bar",
        "(", ")", "[", "]", "{", "}", ":", ",", ".", "|", "*", "->", "<-",
        "-", "=", "<>", "<", ">", ">=", "<=", "\"hello\"", "\"\"",
        "42", "0", "true", "false", "null",
    ]);
    prop::collection::vec(words, 0..=24).prop_map(|v| v.join(" "))
}

proptest! {
    /// On any "token-ish" string the parser must not panic and must not
    /// stack-overflow. It may succeed (returning a Query) or fail (returning a
    /// CqlError of a known kind). The test asserts *only* those two outcomes.
    #[test]
    fn parser_no_panic_clean_error(s in token_ish()) {
        // Call `run` rather than `parse` directly so the full pipeline
        // (lex → parse → lower → eval) is exercised. We use an empty view so
        // eval does not hit the graph; but plan-level rejects are still checked.
        let view = cgx_query::GraphView::new(vec![], vec![], vec![]);
        match run(&view, &s) {
            Ok(_) => { /* valid — fine */ }
            Err(e) => {
                // The error kind must be one of the three known variants.
                prop_assert!(
                    matches!(e.kind, ErrorKind::Parse | ErrorKind::Plan | ErrorKind::Eval),
                    "unexpected error kind {:?} for input {:?}", e.kind, s
                );
                // The message must be non-empty.
                prop_assert!(!e.message.is_empty(), "empty error message for {:?}", s);
                // The span must be ordered (start ≤ end).
                prop_assert!(
                    e.span.start <= e.span.end,
                    "error span start > end: {:?} for {:?}", e.span, s
                );
            }
        }
    }
}

proptest! {
    /// Deep nesting: stress-test the recursive descent for stack safety with
    /// increasingly nested parentheses and path patterns. The parser must not
    /// stack-overflow at any depth up to 128 levels.
    #[test]
    fn parser_no_stack_overflow_on_deep_nesting(depth in 0usize..128) {
        // Build a deeply parenthesised expression: (((...1...)))
        let inner = "1".to_string();
        let nested = (0..depth).fold(inner, |acc, _| format!("({acc})"));
        let q = format!("MATCH (a)-[:CALLS]->(b) WHERE {nested} = 1 RETURN a.name");
        let view = cgx_query::GraphView::new(vec![], vec![], vec![]);
        // Must not panic or stack-overflow; any result (ok or err) is acceptable.
        let _ = run(&view, &q);
    }
}
