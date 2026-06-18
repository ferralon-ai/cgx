//! Hand-written lexer for the CQL Cypher subset (design §2).
//!
//! Every [`Token`] carries a byte [`Span`] into the original source so parse and
//! plan errors can render a caret under the exact offending text (design §7).
//! The lexer is deterministic and allocation-light: string and numeric literals
//! own their decoded value, everything else borrows nothing.

use std::ops::Range;

use crate::error::{CqlError, ErrorKind};

/// A half-open byte range `[start, end)` into the query source.
pub type Span = Range<usize>;

/// A lexical token plus its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// The token classes the grammar distinguishes.
///
/// Keywords are recognised case-insensitively (Cypher convention) and folded to
/// dedicated variants so the parser never string-compares. Identifiers retain
/// their original casing. Literals carry their decoded value.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords.
    Match,
    Where,
    Return,
    With,
    Call,
    Yield,
    Order,
    By,
    As,
    Distinct,
    Limit,
    Asc,
    Desc,
    And,
    Or,
    Not,
    In,
    None,
    Any,
    All,
    True,
    False,
    Null,

    // Operands.
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),

    // Punctuation.
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    LBrace,   // {
    RBrace,   // }
    Colon,    // :
    Comma,    // ,
    Dot,      // .
    Pipe,     // |
    At,       // @
    Star,     // *
    DotDot,   // ..

    // Comparison / arrow operators.
    Eq,       // =
    Ne,       // <>
    Lt,       // <
    Le,       // <=
    Gt,       // >
    Ge,       // >=
    Dash,     // -
    ArrowR,   // ->
    ArrowL,   // <-

    Eof,
}

/// Tokenize `src` into a vector terminated by a single [`TokenKind::Eof`] whose
/// span is the empty range at end-of-input.
///
/// Returns a [`CqlError`] of kind [`ErrorKind::Parse`] on an unterminated string,
/// a bad escape, a malformed number, or an unexpected character.
pub fn tokenize(src: &str) -> Result<Vec<Token>, CqlError> {
    Lexer::new(src).run()
}

struct Lexer<'s> {
    src: &'s str,
    bytes: &'s [u8],
    pos: usize,
}

impl<'s> Lexer<'s> {
    fn new(src: &'s str) -> Self {
        Lexer { src, bytes: src.as_bytes(), pos: 0 }
    }

    fn run(mut self) -> Result<Vec<Token>, CqlError> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            if self.pos >= self.bytes.len() {
                out.push(Token { kind: TokenKind::Eof, span: self.pos..self.pos });
                return Ok(out);
            }
            out.push(self.next_token()?);
        }
    }

    fn skip_trivia(&mut self) {
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                self.pos += 1;
            } else if b == b'/' && self.peek_at(self.pos + 1) == Some(b'/') {
                // Line comment to end of line.
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn peek_at(&self, i: usize) -> Option<u8> {
        self.bytes.get(i).copied()
    }

    fn next_token(&mut self) -> Result<Token, CqlError> {
        let start = self.pos;
        let b = self.bytes[start];

        // Identifiers / keywords.
        if is_ident_start(b) {
            return Ok(self.lex_ident(start));
        }
        // Numbers.
        if b.is_ascii_digit() {
            return self.lex_number(start);
        }
        // Strings.
        if b == b'"' {
            return self.lex_string(start);
        }

        // Punctuation & operators (longest match first).
        let two = self.two_char(start);
        let (kind, len): (TokenKind, usize) = match (b, two) {
            (b'<', Some(b'>')) => (TokenKind::Ne, 2),
            (b'<', Some(b'=')) => (TokenKind::Le, 2),
            (b'<', Some(b'-')) => (TokenKind::ArrowL, 2),
            (b'>', Some(b'=')) => (TokenKind::Ge, 2),
            (b'-', Some(b'>')) => (TokenKind::ArrowR, 2),
            (b'.', Some(b'.')) => (TokenKind::DotDot, 2),
            (b'(', _) => (TokenKind::LParen, 1),
            (b')', _) => (TokenKind::RParen, 1),
            (b'[', _) => (TokenKind::LBracket, 1),
            (b']', _) => (TokenKind::RBracket, 1),
            (b'{', _) => (TokenKind::LBrace, 1),
            (b'}', _) => (TokenKind::RBrace, 1),
            (b':', _) => (TokenKind::Colon, 1),
            (b',', _) => (TokenKind::Comma, 1),
            (b'.', _) => (TokenKind::Dot, 1),
            (b'|', _) => (TokenKind::Pipe, 1),
            (b'@', _) => (TokenKind::At, 1),
            (b'*', _) => (TokenKind::Star, 1),
            (b'=', _) => (TokenKind::Eq, 1),
            (b'<', _) => (TokenKind::Lt, 1),
            (b'>', _) => (TokenKind::Gt, 1),
            (b'-', _) => (TokenKind::Dash, 1),
            _ => {
                let ch_len = char_len(self.src, start);
                return Err(CqlError::new(
                    start..start + ch_len,
                    ErrorKind::Parse,
                    format!("unexpected character `{}`", &self.src[start..start + ch_len]),
                ));
            }
        };
        self.pos = start + len;
        Ok(Token { kind, span: start..self.pos })
    }

    fn two_char(&self, start: usize) -> Option<u8> {
        self.peek_at(start + 1)
    }

    fn lex_ident(&mut self, start: usize) -> Token {
        let mut end = start + 1;
        while end < self.bytes.len() && is_ident_continue(self.bytes[end]) {
            end += 1;
        }
        self.pos = end;
        let text = &self.src[start..end];
        let kind = keyword(text).unwrap_or_else(|| TokenKind::Ident(text.to_string()));
        Token { kind, span: start..end }
    }

    fn lex_number(&mut self, start: usize) -> Result<Token, CqlError> {
        let mut end = start;
        while end < self.bytes.len() && self.bytes[end].is_ascii_digit() {
            end += 1;
        }
        // A float requires a `.` followed by a digit; a lone `..` is the range
        // operator and must not be consumed here.
        let mut is_float = false;
        if self.peek_at(end) == Some(b'.') && self.peek_at(end + 1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            end += 1;
            while end < self.bytes.len() && self.bytes[end].is_ascii_digit() {
                end += 1;
            }
        }
        // Optional exponent.
        if matches!(self.peek_at(end), Some(b'e') | Some(b'E')) {
            let mut e = end + 1;
            if matches!(self.peek_at(e), Some(b'+') | Some(b'-')) {
                e += 1;
            }
            if self.peek_at(e).is_some_and(|c| c.is_ascii_digit()) {
                is_float = true;
                end = e;
                while end < self.bytes.len() && self.bytes[end].is_ascii_digit() {
                    end += 1;
                }
            }
        }
        self.pos = end;
        let text = &self.src[start..end];
        let span = start..end;
        if is_float {
            text.parse::<f64>()
                .map(|f| Token { kind: TokenKind::Float(f), span: span.clone() })
                .map_err(|_| CqlError::new(span, ErrorKind::Parse, format!("invalid float literal `{text}`")))
        } else {
            text.parse::<i64>()
                .map(|i| Token { kind: TokenKind::Int(i), span: span.clone() })
                .map_err(|_| CqlError::new(span, ErrorKind::Parse, format!("integer literal `{text}` out of range")))
        }
    }

    fn lex_string(&mut self, start: usize) -> Result<Token, CqlError> {
        let mut value = String::new();
        let mut i = start + 1; // skip opening quote
        while i < self.bytes.len() {
            let b = self.bytes[i];
            match b {
                b'"' => {
                    self.pos = i + 1;
                    return Ok(Token { kind: TokenKind::Str(value), span: start..self.pos });
                }
                b'\\' => {
                    let esc = self.peek_at(i + 1).ok_or_else(|| {
                        CqlError::new(start..self.bytes.len(), ErrorKind::Parse, "unterminated string literal".into())
                    })?;
                    let decoded = match esc {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        _ => {
                            return Err(CqlError::new(
                                i..i + 2,
                                ErrorKind::Parse,
                                format!("invalid string escape `\\{}`", esc as char),
                            ));
                        }
                    };
                    value.push(decoded);
                    i += 2;
                }
                _ => {
                    // Copy one UTF-8 char (the source is valid UTF-8).
                    let ch_len = char_len(self.src, i);
                    value.push_str(&self.src[i..i + ch_len]);
                    i += ch_len;
                }
            }
        }
        Err(CqlError::new(start..self.bytes.len(), ErrorKind::Parse, "unterminated string literal".into()))
    }
}

fn keyword(text: &str) -> Option<TokenKind> {
    // Cypher keywords are case-insensitive; literals true/false/null are lower
    // by convention but accepted in any case for ergonomics.
    let upper = text.to_ascii_uppercase();
    Some(match upper.as_str() {
        "MATCH" => TokenKind::Match,
        "WHERE" => TokenKind::Where,
        "RETURN" => TokenKind::Return,
        "WITH" => TokenKind::With,
        "CALL" => TokenKind::Call,
        "YIELD" => TokenKind::Yield,
        "ORDER" => TokenKind::Order,
        "BY" => TokenKind::By,
        "AS" => TokenKind::As,
        "DISTINCT" => TokenKind::Distinct,
        "LIMIT" => TokenKind::Limit,
        "ASC" => TokenKind::Asc,
        "DESC" => TokenKind::Desc,
        "AND" => TokenKind::And,
        "OR" => TokenKind::Or,
        "NOT" => TokenKind::Not,
        "IN" => TokenKind::In,
        "NONE" => TokenKind::None,
        "ANY" => TokenKind::Any,
        "ALL" => TokenKind::All,
        "TRUE" => TokenKind::True,
        "FALSE" => TokenKind::False,
        "NULL" => TokenKind::Null,
        _ => return Option::None,
    })
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Byte length of the UTF-8 character beginning at `i` in `src`.
fn char_len(src: &str, i: usize) -> usize {
    src[i..].chars().next().map_or(1, |c| c.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn keywords_are_case_insensitive() {
        assert_eq!(kinds("match Where RETURN"), vec![
            TokenKind::Match,
            TokenKind::Where,
            TokenKind::Return,
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn all_keyword_classes() {
        let src = "MATCH WHERE RETURN WITH CALL YIELD ORDER BY AS DISTINCT LIMIT ASC DESC AND OR NOT IN NONE ANY ALL true false null";
        assert_eq!(kinds(src), vec![
            TokenKind::Match, TokenKind::Where, TokenKind::Return, TokenKind::With,
            TokenKind::Call, TokenKind::Yield, TokenKind::Order, TokenKind::By,
            TokenKind::As, TokenKind::Distinct, TokenKind::Limit, TokenKind::Asc,
            TokenKind::Desc, TokenKind::And, TokenKind::Or, TokenKind::Not,
            TokenKind::In, TokenKind::None, TokenKind::Any, TokenKind::All,
            TokenKind::True, TokenKind::False, TokenKind::Null, TokenKind::Eof,
        ]);
    }

    #[test]
    fn identifier_class() {
        assert_eq!(kinds("foo _bar baz123"), vec![
            TokenKind::Ident("foo".into()),
            TokenKind::Ident("_bar".into()),
            TokenKind::Ident("baz123".into()),
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn integer_class_and_span() {
        let toks = tokenize("42 0 9001").unwrap();
        assert_eq!(toks[0], Token { kind: TokenKind::Int(42), span: 0..2 });
        assert_eq!(toks[1], Token { kind: TokenKind::Int(0), span: 3..4 });
        assert_eq!(toks[2], Token { kind: TokenKind::Int(9001), span: 5..9 });
    }

    #[test]
    fn float_class() {
        assert_eq!(kinds("3.25 1.0e3 2E-2"), vec![
            TokenKind::Float(3.25),
            TokenKind::Float(1000.0),
            TokenKind::Float(0.02),
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn float_vs_range_operator() {
        // `1..5` must lex as Int DotDot Int, not a malformed float.
        assert_eq!(kinds("1..5"), vec![
            TokenKind::Int(1),
            TokenKind::DotDot,
            TokenKind::Int(5),
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn string_class_with_escapes() {
        assert_eq!(kinds(r#""he\"llo\n" "a::b""#), vec![
            TokenKind::Str("he\"llo\n".into()),
            TokenKind::Str("a::b".into()),
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn string_span_includes_quotes() {
        let toks = tokenize(r#""ab""#).unwrap();
        assert_eq!(toks[0].span, 0..4);
    }

    #[test]
    fn punctuation_and_arrows() {
        assert_eq!(kinds("( ) [ ] { } : , . | @ * .."), vec![
            TokenKind::LParen, TokenKind::RParen, TokenKind::LBracket,
            TokenKind::RBracket, TokenKind::LBrace, TokenKind::RBrace,
            TokenKind::Colon, TokenKind::Comma, TokenKind::Dot, TokenKind::Pipe,
            TokenKind::At, TokenKind::Star, TokenKind::DotDot, TokenKind::Eof,
        ]);
    }

    #[test]
    fn comparison_and_relationship_operators() {
        assert_eq!(kinds("= <> < <= > >= - -> <-"), vec![
            TokenKind::Eq, TokenKind::Ne, TokenKind::Lt, TokenKind::Le,
            TokenKind::Gt, TokenKind::Ge, TokenKind::Dash, TokenKind::ArrowR,
            TokenKind::ArrowL, TokenKind::Eof,
        ]);
    }

    #[test]
    fn rel_pattern_spans_are_exact() {
        let toks = tokenize("-[:CALLS]->").unwrap();
        assert_eq!(toks[0], Token { kind: TokenKind::Dash, span: 0..1 });
        assert_eq!(toks[1], Token { kind: TokenKind::LBracket, span: 1..2 });
        assert_eq!(toks[2], Token { kind: TokenKind::Colon, span: 2..3 });
        assert_eq!(toks[3], Token { kind: TokenKind::Ident("CALLS".into()), span: 3..8 });
        assert_eq!(toks[4], Token { kind: TokenKind::RBracket, span: 8..9 });
        assert_eq!(toks[5], Token { kind: TokenKind::ArrowR, span: 9..11 });
    }

    #[test]
    fn line_comments_are_trivia() {
        assert_eq!(kinds("MATCH // a comment\nRETURN"), vec![
            TokenKind::Match,
            TokenKind::Return,
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn unterminated_string_is_parse_error() {
        let err = tokenize(r#""no end"#).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn bad_escape_is_parse_error() {
        let err = tokenize(r#""x\qy""#).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn unexpected_char_is_parse_error() {
        let err = tokenize("MATCH (a) ~").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
        assert_eq!(err.span, 10..11);
    }
}
