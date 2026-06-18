//! Recursive-descent parser for clauses + a Pratt parser for `Expr`
//! (design §2). Produces [`ast::Query`]. Every failure is a
//! [`CqlError`] of kind [`ErrorKind::Parse`] carrying a real byte span.
//!
//! The grammar splits a query into pipeline *parts* separated by `WITH`. Each
//! part is a run of reading clauses (`MATCH` / `CALL`) followed by either a
//! terminal `RETURN` (the last part) or a `WITH` projection that opens the next
//! part. We parse parts greedily: a `WITH` closes the current part and starts a
//! new one; the loop ends at `RETURN` + EOF.

use crate::ast::*;
use crate::error::{CqlError, ErrorKind};
use crate::token::{Span, Token, TokenKind};
use crate::value::Value;

/// Parse `src` into a [`Query`].
pub fn parse(src: &str) -> Result<Query, CqlError> {
    let tokens = crate::token::tokenize(src)?;
    Parser::new(tokens).parse_query()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    // ---- token cursor -----------------------------------------------------

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn peek_span(&self) -> Span {
        self.tokens[self.pos].span.clone()
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if !matches!(t.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek() == kind {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind, what: &str) -> Result<Token, CqlError> {
        if self.peek() == kind {
            Ok(self.bump())
        } else {
            Err(self.err_here(format!("expected {what}, found {}", describe(self.peek()))))
        }
    }

    fn err_here(&self, msg: String) -> CqlError {
        CqlError::new(self.peek_span(), ErrorKind::Parse, msg)
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, Span), CqlError> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                let span = self.peek_span();
                self.bump();
                Ok((name, span))
            }
            other => Err(self.err_here(format!("expected {what}, found {}", describe(&other)))),
        }
    }

    // ---- query / parts ----------------------------------------------------

    fn parse_query(&mut self) -> Result<Query, CqlError> {
        let mut parts = Vec::new();
        loop {
            let part = self.parse_part()?;
            let ended = part.with.is_none();
            parts.push(part);
            if ended {
                break;
            }
        }
        if !self.at_eof() {
            // `UNION` after a RETURN is a recognised-but-deferred clause; give a
            // Plan error rather than a confusing "unexpected trailing input".
            if let TokenKind::Ident(name) = self.peek().clone() {
                if name.eq_ignore_ascii_case("UNION") {
                    let span = self.peek_span();
                    return Err(CqlError::plan(
                        span,
                        "`UNION` is not supported in this release (deferred)",
                    ));
                }
            }
            return Err(self.err_here(format!("unexpected trailing input {}", describe(self.peek()))));
        }
        // The final part must RETURN.
        match parts.last() {
            Some(p) if p.ret.is_some() => Ok(Query { parts }),
            _ => Err(self.err_here("query must end with a RETURN clause".into())),
        }
    }

    fn parse_part(&mut self) -> Result<QueryPart, CqlError> {
        let mut reading = Vec::new();
        loop {
            match self.peek() {
                TokenKind::Match => reading.push(ReadingClause::Match(self.parse_match()?)),
                TokenKind::Call => reading.push(ReadingClause::Call(self.parse_call()?)),
                _ => break,
            }
        }

        if self.eat(&TokenKind::With) {
            let with = self.parse_with()?;
            return Ok(QueryPart { reading, ret: None, with: Some(with) });
        }

        if matches!(self.peek(), TokenKind::Return) {
            let ret = self.parse_return()?;
            return Ok(QueryPart { reading, ret: Some(ret), with: None });
        }

        // Recognised-but-deferred clause keywords that would otherwise surface as
        // a confusing syntax error. Each gets a Plan error naming the feature and
        // stating it is deferred, per design §7 and §5.
        if let TokenKind::Ident(name) = self.peek().clone() {
            let span = self.peek_span();
            match name.to_ascii_uppercase().as_str() {
                "CREATE" | "SET" | "DELETE" | "DETACH" => {
                    return Err(CqlError::plan(
                        span,
                        format!(
                            "`{name}` is not supported: cgx query is a read-only language; \
                             write clauses (CREATE/SET/DELETE) are deferred"
                        ),
                    ));
                }
                "UNWIND" => {
                    return Err(CqlError::plan(
                        span,
                        "`UNWIND` is not supported in this release (deferred)",
                    ));
                }
                "OPTIONAL" => {
                    return Err(CqlError::plan(
                        span,
                        "`OPTIONAL MATCH` is not supported in this release (deferred)",
                    ));
                }
                "UNION" => {
                    return Err(CqlError::plan(
                        span,
                        "`UNION` is not supported in this release (deferred)",
                    ));
                }
                _ => {}
            }
        }

        if reading.is_empty() {
            return Err(self.err_here(format!(
                "expected MATCH, CALL, WITH, or RETURN, found {}",
                describe(self.peek())
            )));
        }
        Err(self.err_here(format!("expected WITH or RETURN, found {}", describe(self.peek()))))
    }

    // ---- MATCH ------------------------------------------------------------

    fn parse_match(&mut self) -> Result<MatchClause, CqlError> {
        self.expect(&TokenKind::Match, "MATCH")?;
        // `MATCH ALL … MUST PASS THROUGH / AVOIDING` (Q-20 guarded cut) is
        // recognised but deferred; intercept before the `(` check to give a
        // Plan error rather than a confusing "expected `(`" message.
        if matches!(self.peek(), TokenKind::All) {
            let span = self.peek_span();
            return Err(CqlError::plan(
                span,
                "`MATCH ALL … MUST PASS THROUGH/AVOIDING` (Q-20 guarded-cut) is \
                 not supported in this release (deferred)",
            ));
        }
        let mut patterns = vec![self.parse_path_pattern()?];
        while self.eat(&TokenKind::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        let where_clause = if self.eat(&TokenKind::Where) { Some(self.parse_expr()?) } else { None };
        Ok(MatchClause { patterns, where_clause })
    }

    fn parse_path_pattern(&mut self) -> Result<PathPattern, CqlError> {
        // Optional `path =` binding: an identifier immediately followed by `=`.
        let path_var = if let TokenKind::Ident(name) = self.peek().clone() {
            if self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Eq) {
                self.bump(); // ident
                self.bump(); // =
                Some(name)
            } else {
                None
            }
        } else {
            None
        };

        let start = self.parse_node_pat()?;
        let mut steps = Vec::new();
        while matches!(self.peek(), TokenKind::Dash | TokenKind::ArrowL) {
            let rel = self.parse_rel_pat()?;
            let node = self.parse_node_pat()?;
            steps.push((rel, node));
        }
        Ok(PathPattern { path_var, start, steps })
    }

    fn parse_node_pat(&mut self) -> Result<NodePat, CqlError> {
        self.expect(&TokenKind::LParen, "`(` to begin a node pattern")?;
        let var = self.parse_opt_ident();
        let label = if self.eat(&TokenKind::Colon) {
            let (name, span) = self.expect_ident("node label after `:`")?;
            Some(Spanned::new(name, span))
        } else {
            None
        };
        let props = if matches!(self.peek(), TokenKind::LBrace) { self.parse_prop_map()? } else { Vec::new() };
        self.expect(&TokenKind::RParen, "`)` to close the node pattern")?;
        Ok(NodePat { var, label, props })
    }

    fn parse_rel_pat(&mut self) -> Result<RelPat, CqlError> {
        // Leading arrow side: either `-` (forward/undirected-left) or `<-`.
        let leading_back = match self.peek() {
            TokenKind::Dash => {
                self.bump();
                false
            }
            TokenKind::ArrowL => {
                self.bump();
                true
            }
            _ => return Err(self.err_here("expected `-` or `<-` to begin a relationship".into())),
        };

        let (var, types, var_len, props) = if self.eat(&TokenKind::LBracket) {
            let var = self.parse_opt_ident();
            let types = if self.eat(&TokenKind::Colon) { self.parse_rel_types()? } else { Vec::new() };
            let var_len = if matches!(self.peek(), TokenKind::Star) { Some(self.parse_var_len()?) } else { None };
            let props = if matches!(self.peek(), TokenKind::LBrace) { self.parse_prop_map()? } else { Vec::new() };
            self.expect(&TokenKind::RBracket, "`]` to close the relationship")?;
            (var, types, var_len, props)
        } else {
            (None, Vec::new(), None, Vec::new())
        };

        // Trailing arrow side: `->` (forward) or `-` (backward).
        let trailing_forward = match self.peek() {
            TokenKind::ArrowR => {
                self.bump();
                true
            }
            TokenKind::Dash => {
                self.bump();
                false
            }
            _ => return Err(self.err_here("expected `->` or `-` to close a relationship".into())),
        };

        let direction = match (leading_back, trailing_forward) {
            (false, true) => Direction::Forward,  // -[...]->
            (true, false) => Direction::Backward, // <-[...]-
            _ => {
                return Err(self.err_here(
                    "ambiguous relationship direction: use `-[...]->` or `<-[...]-`".into(),
                ));
            }
        };

        Ok(RelPat { var, direction, types, var_len, props })
    }

    fn parse_rel_types(&mut self) -> Result<Vec<Spanned<String>>, CqlError> {
        let (name, span) = self.expect_ident("relationship type after `:`")?;
        // `CALLS:super` — a sub-type qualifier using `:` is the Theme-13 syntax
        // (not yet supported). Give a Plan error with a clear message.
        if matches!(self.peek(), TokenKind::Colon) {
            let colon_span = self.peek_span();
            return Err(CqlError::plan(
                colon_span,
                format!(
                    "sub-type qualifier `{name}:…` (e.g. `CALLS:super`) is recognised \
                     but not supported in this release (Theme-13, deferred)"
                ),
            ));
        }
        let mut types = vec![Spanned::new(name, span)];
        while self.eat(&TokenKind::Pipe) {
            let (name, span) = self.expect_ident("relationship type after `|`")?;
            types.push(Spanned::new(name, span));
        }
        Ok(types)
    }

    fn parse_var_len(&mut self) -> Result<VarLen, CqlError> {
        self.expect(&TokenKind::Star, "`*`")?;
        // Forms: `*`, `*n`, `*n..`, `*n..m`, `*..m`.
        let min = self.parse_opt_int_u32()?;
        let max = if self.eat(&TokenKind::DotDot) {
            self.parse_opt_int_u32()?
        } else {
            // `*n` (no `..`) means exactly n: min == max.
            min
        };
        Ok(VarLen { min, max })
    }

    fn parse_opt_int_u32(&mut self) -> Result<Option<u32>, CqlError> {
        if let TokenKind::Int(n) = *self.peek() {
            let span = self.peek_span();
            self.bump();
            let v = u32::try_from(n)
                .map_err(|_| CqlError::new(span, ErrorKind::Parse, format!("length bound `{n}` out of range")))?;
            Ok(Some(v))
        } else {
            Ok(None)
        }
    }

    fn parse_prop_map(&mut self) -> Result<Vec<PropEntry>, CqlError> {
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut entries = Vec::new();
        if !matches!(self.peek(), TokenKind::RBrace) {
            loop {
                let (key, span) = self.parse_prop_key()?;
                self.expect(&TokenKind::Colon, "`:` after property key")?;
                let value = self.parse_literal()?;
                entries.push(PropEntry { key: Spanned::new(key, span), value });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RBrace, "`}` to close the property map")?;
        Ok(entries)
    }

    /// A property-map key is an identifier or one of the few keywords that
    /// double as field names (`kind`, `in`, etc.). We accept identifiers only;
    /// the field set is validated at lowering time.
    fn parse_prop_key(&mut self) -> Result<(String, Span), CqlError> {
        self.expect_ident("property key")
    }

    // ---- CALL -------------------------------------------------------------

    fn parse_call(&mut self) -> Result<CallClause, CqlError> {
        self.expect(&TokenKind::Call, "CALL")?;
        let (head, head_span) = self.expect_ident("procedure name")?;
        let mut qualified = head;
        let mut end = head_span.end;
        while self.eat(&TokenKind::Dot) {
            let (seg, span) = self.expect_ident("procedure name segment after `.`")?;
            qualified.push('.');
            qualified.push_str(&seg);
            end = span.end;
        }
        let proc = Spanned::new(qualified, head_span.start..end);

        self.expect(&TokenKind::LParen, "`(` after procedure name")?;
        let mut args = Vec::new();
        if !matches!(self.peek(), TokenKind::RParen) {
            args.push(self.parse_expr()?);
            while self.eat(&TokenKind::Comma) {
                args.push(self.parse_expr()?);
            }
        }
        self.expect(&TokenKind::RParen, "`)` to close arguments")?;

        self.expect(&TokenKind::Yield, "YIELD")?;
        let mut yields = vec![self.parse_yield_item()?];
        while self.eat(&TokenKind::Comma) {
            yields.push(self.parse_yield_item()?);
        }

        let where_clause = if self.eat(&TokenKind::Where) { Some(self.parse_expr()?) } else { None };
        Ok(CallClause { proc, args, yields, where_clause })
    }

    fn parse_yield_item(&mut self) -> Result<YieldItem, CqlError> {
        let (name, span) = self.expect_ident("yielded column name")?;
        let alias = if self.eat(&TokenKind::As) { Some(self.expect_ident("alias after AS")?.0) } else { None };
        Ok(YieldItem { name: Spanned::new(name, span), alias })
    }

    // ---- RETURN / WITH ----------------------------------------------------

    fn parse_return(&mut self) -> Result<ReturnClause, CqlError> {
        self.expect(&TokenKind::Return, "RETURN")?;
        let distinct = self.eat(&TokenKind::Distinct);
        let items = self.parse_return_items()?;
        let order_by = self.parse_opt_order_by()?;
        let limit = self.parse_opt_limit()?;
        Ok(ReturnClause { distinct, items, order_by, limit })
    }

    fn parse_with(&mut self) -> Result<WithClause, CqlError> {
        let distinct = self.eat(&TokenKind::Distinct);
        let items = self.parse_return_items()?;
        let order_by = self.parse_opt_order_by()?;
        let limit = self.parse_opt_limit()?;
        let where_clause = if self.eat(&TokenKind::Where) { Some(self.parse_expr()?) } else { None };
        Ok(WithClause { distinct, items, order_by, limit, where_clause })
    }

    fn parse_return_items(&mut self) -> Result<Vec<ReturnItem>, CqlError> {
        let mut items = vec![self.parse_return_item()?];
        while self.eat(&TokenKind::Comma) {
            items.push(self.parse_return_item()?);
        }
        Ok(items)
    }

    fn parse_return_item(&mut self) -> Result<ReturnItem, CqlError> {
        let expr = self.parse_expr()?;
        let alias = if self.eat(&TokenKind::As) { Some(self.expect_ident("alias after AS")?.0) } else { None };
        Ok(ReturnItem { expr, alias })
    }

    fn parse_opt_order_by(&mut self) -> Result<Vec<OrderBy>, CqlError> {
        if !self.eat(&TokenKind::Order) {
            return Ok(Vec::new());
        }
        self.expect(&TokenKind::By, "BY after ORDER")?;
        let mut items = vec![self.parse_sort_item()?];
        while self.eat(&TokenKind::Comma) {
            items.push(self.parse_sort_item()?);
        }
        Ok(items)
    }

    fn parse_sort_item(&mut self) -> Result<OrderBy, CqlError> {
        let expr = self.parse_expr()?;
        let descending = if self.eat(&TokenKind::Desc) {
            true
        } else {
            self.eat(&TokenKind::Asc);
            false
        };
        Ok(OrderBy { expr, descending })
    }

    fn parse_opt_limit(&mut self) -> Result<Option<u64>, CqlError> {
        if !self.eat(&TokenKind::Limit) {
            return Ok(None);
        }
        match *self.peek() {
            TokenKind::Int(n) if n >= 0 => {
                self.bump();
                Ok(Some(n as u64))
            }
            _ => Err(self.err_here("expected a non-negative integer after LIMIT".into())),
        }
    }

    // ---- expressions (Pratt) ---------------------------------------------

    fn parse_expr(&mut self) -> Result<Expr, CqlError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, CqlError> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), TokenKind::Or) {
            let span = self.peek_span();
            self.bump();
            let rhs = self.parse_and()?;
            lhs = Expr::Binary { op: BinOp::Or, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, CqlError> {
        let mut lhs = self.parse_not()?;
        while matches!(self.peek(), TokenKind::And) {
            let span = self.peek_span();
            self.bump();
            let rhs = self.parse_not()?;
            lhs = Expr::Binary { op: BinOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> Result<Expr, CqlError> {
        if matches!(self.peek(), TokenKind::Not) {
            self.bump();
            // `NOT (m)<-[:CALLS]-()` — a negated bare pattern predicate.
            if let Some(pat) = self.try_pattern_predicate()? {
                return Ok(Expr::PatternPredicate { negated: true, pattern: Box::new(pat) });
            }
            let inner = self.parse_not()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        // A non-negated bare pattern predicate, e.g. `(m)<-[:CALLS]-()`.
        if let Some(pat) = self.try_pattern_predicate()? {
            return Ok(Expr::PatternPredicate { negated: false, pattern: Box::new(pat) });
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr, CqlError> {
        let lhs = self.parse_primary()?;
        let op = match self.peek() {
            TokenKind::Eq => BinOp::Eq,
            TokenKind::Ne => BinOp::Ne,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::Le => BinOp::Le,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::Ge => BinOp::Ge,
            TokenKind::In => BinOp::In,
            // `IS [NOT] EMPTY` / `IS NULL` — recognised-but-deferred; give a
            // Plan error with a clear message rather than a confusing parse fail.
            TokenKind::Ident(name) if name.eq_ignore_ascii_case("IS") => {
                let span = self.peek_span();
                return Err(CqlError::plan(
                    span,
                    "`IS EMPTY` / `IS NULL` predicates are not supported in this \
                     release (IS EMPTY requires type reconstruction; deferred)",
                ));
            }
            _ => return Ok(lhs),
        };
        let span = self.peek_span();
        self.bump();
        let rhs = self.parse_primary()?;
        Ok(Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span })
    }

    fn parse_primary(&mut self) -> Result<Expr, CqlError> {
        match self.peek().clone() {
            TokenKind::LParen => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect(&TokenKind::RParen, "`)` to close a parenthesised expression")?;
                Ok(e)
            }
            TokenKind::LBracket if self.bracket_is_comprehension() => self.parse_list_comp(),
            TokenKind::LBracket => Ok(Expr::Literal(self.parse_list_literal()?)),
            TokenKind::None | TokenKind::Any | TokenKind::All => self.parse_quantifier(),
            TokenKind::Ident(name) => {
                let span = self.peek_span();
                self.bump();
                // Function call vs property path.
                if matches!(self.peek(), TokenKind::LParen) {
                    self.parse_function_call(Spanned::new(name, span))
                } else {
                    self.parse_property(Spanned::new(name, span))
                }
            }
            TokenKind::Str(_) | TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::True
            | TokenKind::False | TokenKind::Null => Ok(Expr::Literal(self.parse_literal()?)),
            other => Err(self.err_here(format!("expected an expression, found {}", describe(&other)))),
        }
    }

    fn parse_property(&mut self, head: Spanned<String>) -> Result<Expr, CqlError> {
        let mut segments = Vec::new();
        while self.eat(&TokenKind::Dot) {
            let (seg, span) = self.expect_ident("property name after `.`")?;
            segments.push(Spanned::new(seg, span));
        }
        Ok(Expr::Property { head, segments })
    }

    fn parse_function_call(&mut self, name: Spanned<String>) -> Result<Expr, CqlError> {
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut args = Vec::new();
        // `count(*)` is the one star-argument form.
        if matches!(self.peek(), TokenKind::Star) {
            let span = self.peek_span();
            self.bump();
            args.push(Expr::Property { head: Spanned::new("*".into(), span), segments: Vec::new() });
        } else if !matches!(self.peek(), TokenKind::RParen) {
            args.push(self.parse_expr()?);
            while self.eat(&TokenKind::Comma) {
                args.push(self.parse_expr()?);
            }
        }
        self.expect(&TokenKind::RParen, "`)` to close the argument list")?;
        Ok(Expr::FunctionCall { name, args })
    }

    /// A `[` opens a comprehension iff it is `[ <ident> IN ...`; otherwise it is
    /// a list literal. (Comprehension variables are always plain identifiers.)
    fn bracket_is_comprehension(&self) -> bool {
        matches!(self.tokens.get(self.pos + 1).map(|t| &t.kind), Some(TokenKind::Ident(_)))
            && matches!(self.tokens.get(self.pos + 2).map(|t| &t.kind), Some(TokenKind::In))
    }

    fn parse_list_comp(&mut self) -> Result<Expr, CqlError> {
        self.expect(&TokenKind::LBracket, "`[`")?;
        let (var, _) = self.expect_ident("comprehension variable")?;
        self.expect(&TokenKind::In, "IN in a list comprehension")?;
        let list = self.parse_expr()?;
        let filter = if self.eat(&TokenKind::Where) { Some(Box::new(self.parse_expr()?)) } else { None };
        self.expect(&TokenKind::Pipe, "`|` before the comprehension projection")?;
        let projection = self.parse_expr()?;
        self.expect(&TokenKind::RBracket, "`]` to close the comprehension")?;
        Ok(Expr::ListComp { var, list: Box::new(list), filter, projection: Box::new(projection) })
    }

    fn parse_quantifier(&mut self) -> Result<Expr, CqlError> {
        let kind = match self.bump().kind {
            TokenKind::None => QuantifierKind::None,
            TokenKind::Any => QuantifierKind::Any,
            TokenKind::All => QuantifierKind::All,
            _ => unreachable!("parse_quantifier called on a non-quantifier token"),
        };
        self.expect(&TokenKind::LParen, "`(` after a quantifier")?;
        let (var, _) = self.expect_ident("quantifier variable")?;
        self.expect(&TokenKind::In, "IN in a quantifier")?;
        let list = self.parse_expr()?;
        self.expect(&TokenKind::Where, "WHERE in a quantifier")?;
        let predicate = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "`)` to close the quantifier")?;
        Ok(Expr::Quantifier { kind, var, list: Box::new(list), predicate: Box::new(predicate) })
    }

    /// Try to parse a bare pattern predicate starting at a `(`. Returns `None`
    /// (without consuming) when the parenthesised form is an ordinary grouped
    /// expression rather than a node pattern followed by a relationship.
    ///
    /// Disambiguation: a pattern predicate is `( ... )` immediately followed by
    /// a relationship arrow (`-`/`<-`). A grouped expression's `)` is followed
    /// by something else (an operator, `)`, comma, RETURN, …). We look ahead by
    /// scanning to the matching `)` and checking the next token.
    fn try_pattern_predicate(&mut self) -> Result<Option<PathPattern>, CqlError> {
        if !matches!(self.peek(), TokenKind::LParen) {
            return Ok(None);
        }
        if !self.paren_group_is_pattern() {
            return Ok(None);
        }
        let pat = self.parse_path_pattern()?;
        // A pattern predicate must contain at least one relationship step.
        if pat.steps.is_empty() {
            return Err(CqlError::new(
                self.peek_span(),
                ErrorKind::Parse,
                "a pattern predicate requires a relationship, e.g. `(m)<-[:CALLS]-()`".into(),
            ));
        }
        Ok(Some(pat))
    }

    /// Look ahead from the current `(` to its matching `)` and report whether the
    /// token after it is a relationship arrow — the signal that this is a node
    /// pattern, not a grouped expression.
    fn paren_group_is_pattern(&self) -> bool {
        let mut depth = 0usize;
        let mut i = self.pos;
        while i < self.tokens.len() {
            match &self.tokens[i].kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(
                            self.tokens.get(i + 1).map(|t| &t.kind),
                            Some(TokenKind::Dash) | Some(TokenKind::ArrowL)
                        );
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    // ---- literals ---------------------------------------------------------

    fn parse_literal(&mut self) -> Result<Value, CqlError> {
        match self.peek().clone() {
            TokenKind::Str(s) => {
                self.bump();
                Ok(Value::Str(s))
            }
            TokenKind::Int(n) => {
                self.bump();
                Ok(Value::Int(n))
            }
            TokenKind::Float(f) => {
                self.bump();
                Ok(Value::Float(f))
            }
            TokenKind::True => {
                self.bump();
                Ok(Value::Bool(true))
            }
            TokenKind::False => {
                self.bump();
                Ok(Value::Bool(false))
            }
            TokenKind::Null => {
                self.bump();
                Ok(Value::Null)
            }
            TokenKind::LBracket => self.parse_list_literal(),
            other => Err(self.err_here(format!("expected a literal, found {}", describe(&other)))),
        }
    }

    fn parse_list_literal(&mut self) -> Result<Value, CqlError> {
        self.expect(&TokenKind::LBracket, "`[`")?;
        let mut items = Vec::new();
        if !matches!(self.peek(), TokenKind::RBracket) {
            items.push(self.parse_literal()?);
            while self.eat(&TokenKind::Comma) {
                items.push(self.parse_literal()?);
            }
        }
        self.expect(&TokenKind::RBracket, "`]` to close the list literal")?;
        Ok(Value::List(items))
    }

    // ---- misc -------------------------------------------------------------

    fn parse_opt_ident(&mut self) -> Option<String> {
        if let TokenKind::Ident(name) = self.peek().clone() {
            self.bump();
            Some(name)
        } else {
            None
        }
    }
}

/// A human-readable name for a token, for error messages.
fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Eof => "end of input".into(),
        TokenKind::Ident(s) => format!("identifier `{s}`"),
        TokenKind::Int(n) => format!("integer `{n}`"),
        TokenKind::Float(f) => format!("float `{f}`"),
        TokenKind::Str(s) => format!("string {s:?}"),
        other => format!("`{}`", token_text(other)),
    }
}

fn token_text(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Match => "MATCH",
        TokenKind::Where => "WHERE",
        TokenKind::Return => "RETURN",
        TokenKind::With => "WITH",
        TokenKind::Call => "CALL",
        TokenKind::Yield => "YIELD",
        TokenKind::Order => "ORDER",
        TokenKind::By => "BY",
        TokenKind::As => "AS",
        TokenKind::Distinct => "DISTINCT",
        TokenKind::Limit => "LIMIT",
        TokenKind::Asc => "ASC",
        TokenKind::Desc => "DESC",
        TokenKind::And => "AND",
        TokenKind::Or => "OR",
        TokenKind::Not => "NOT",
        TokenKind::In => "IN",
        TokenKind::None => "NONE",
        TokenKind::Any => "ANY",
        TokenKind::All => "ALL",
        TokenKind::True => "true",
        TokenKind::False => "false",
        TokenKind::Null => "null",
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        TokenKind::LBracket => "[",
        TokenKind::RBracket => "]",
        TokenKind::LBrace => "{",
        TokenKind::RBrace => "}",
        TokenKind::Colon => ":",
        TokenKind::Comma => ",",
        TokenKind::Dot => ".",
        TokenKind::Pipe => "|",
        TokenKind::At => "@",
        TokenKind::Star => "*",
        TokenKind::DotDot => "..",
        TokenKind::Eq => "=",
        TokenKind::Ne => "<>",
        TokenKind::Lt => "<",
        TokenKind::Le => "<=",
        TokenKind::Gt => ">",
        TokenKind::Ge => ">=",
        TokenKind::Dash => "-",
        TokenKind::ArrowR => "->",
        TokenKind::ArrowL => "<-",
        TokenKind::Ident(_) | TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::Str(_) | TokenKind::Eof => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Query {
        parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"))
    }

    fn err(src: &str) -> CqlError {
        parse(src).expect_err(&format!("expected parse error for {src:?}"))
    }

    #[test]
    fn simple_match_return() {
        let q = p("MATCH (a) RETURN a");
        assert_eq!(q.parts.len(), 1);
        let MatchClause { patterns, where_clause } = match &q.parts[0].reading[0] {
            ReadingClause::Match(m) => m.clone(),
            _ => panic!(),
        };
        assert_eq!(patterns.len(), 1);
        assert!(where_clause.is_none());
        assert!(q.parts[0].ret.is_some());
    }

    #[test]
    fn node_label_and_props() {
        let q = p(r#"MATCH (m:method {name:"foo", kind:"method"}) RETURN m"#);
        let m = match &q.parts[0].reading[0] {
            ReadingClause::Match(m) => m,
            _ => panic!(),
        };
        let node = &m.patterns[0].start;
        assert_eq!(node.var.as_deref(), Some("m"));
        assert_eq!(node.label.as_ref().unwrap().value, "method");
        assert_eq!(node.props.len(), 2);
        assert_eq!(node.props[0].key.value, "name");
        assert_eq!(node.props[0].value, Value::Str("foo".into()));
    }

    #[test]
    fn forward_and_backward_rels() {
        let fwd = p("MATCH (a)-[:CALLS]->(b) RETURN a");
        let m = match &fwd.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        assert_eq!(m.patterns[0].steps[0].0.direction, Direction::Forward);
        assert_eq!(m.patterns[0].steps[0].0.types[0].value, "CALLS");

        let back = p("MATCH (a)<-[:CALLS]-(b) RETURN a");
        let m = match &back.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        assert_eq!(m.patterns[0].steps[0].0.direction, Direction::Backward);
    }

    #[test]
    fn multi_type_relationship() {
        let q = p("MATCH (a)-[:READS_FIELD|WRITES_FIELD]->(b) RETURN a");
        let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        let types: Vec<_> = m.patterns[0].steps[0].0.types.iter().map(|t| t.value.clone()).collect();
        assert_eq!(types, vec!["READS_FIELD", "WRITES_FIELD"]);
    }

    #[test]
    fn var_length_forms() {
        let cases = [
            ("MATCH (a)-[:CALLS*]->(b) RETURN a", VarLen { min: None, max: None }),
            ("MATCH (a)-[:CALLS*3]->(b) RETURN a", VarLen { min: Some(3), max: Some(3) }),
            ("MATCH (a)-[:CALLS*1..]->(b) RETURN a", VarLen { min: Some(1), max: None }),
            ("MATCH (a)-[:CALLS*..5]->(b) RETURN a", VarLen { min: None, max: Some(5) }),
            ("MATCH (a)-[:CALLS*1..5]->(b) RETURN a", VarLen { min: Some(1), max: Some(5) }),
        ];
        for (src, expected) in cases {
            let q = p(src);
            let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
            assert_eq!(m.patterns[0].steps[0].0.var_len, Some(expected), "{src}");
        }
    }

    #[test]
    fn path_var_binding() {
        let q = p("MATCH p = (a)-[:CALLS*]->(b) RETURN p");
        let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        assert_eq!(m.patterns[0].path_var.as_deref(), Some("p"));
    }

    #[test]
    fn multi_pattern_match() {
        let q = p("MATCH (a)-[:CALLS]->(b), (c)-[:CALLS]->(d) RETURN a");
        let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        assert_eq!(m.patterns.len(), 2);
    }

    #[test]
    fn where_with_and_or_not() {
        let q = p(r#"MATCH (a) WHERE a.name = "x" AND NOT a.file = "y" OR a.line > 3 RETURN a"#);
        let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        // OR binds loosest, so the root is an Or.
        match m.where_clause.as_ref().unwrap() {
            Expr::Binary { op: BinOp::Or, .. } => {}
            other => panic!("expected Or at root, got {other:?}"),
        }
    }

    #[test]
    fn where_in_list() {
        let q = p(r#"MATCH (a) WHERE a.kind IN ["method", "function"] RETURN a"#);
        let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        match m.where_clause.as_ref().unwrap() {
            Expr::Binary { op: BinOp::In, .. } => {}
            other => panic!("expected In, got {other:?}"),
        }
    }

    #[test]
    fn return_distinct_alias_order_limit() {
        let q = p("MATCH (a) RETURN DISTINCT a.name AS n ORDER BY a.line DESC, n ASC LIMIT 10");
        let r = q.parts[0].ret.as_ref().unwrap();
        assert!(r.distinct);
        assert_eq!(r.items[0].alias.as_deref(), Some("n"));
        assert_eq!(r.order_by.len(), 2);
        assert!(r.order_by[0].descending);
        assert!(!r.order_by[1].descending);
        assert_eq!(r.limit, Some(10));
    }

    #[test]
    fn with_pipeline() {
        let q = p("MATCH (m) WITH collect(m.name) AS reached MATCH (t) RETURN t");
        assert_eq!(q.parts.len(), 2);
        assert!(q.parts[0].with.is_some());
        assert_eq!(q.parts[0].with.as_ref().unwrap().items[0].alias.as_deref(), Some("reached"));
        assert!(q.parts[1].ret.is_some());
    }

    #[test]
    fn with_where_filter() {
        let q = p("MATCH (a) WITH a WHERE a.line > 1 RETURN a");
        assert!(q.parts[0].with.as_ref().unwrap().where_clause.is_some());
    }

    #[test]
    fn call_yield_where() {
        let q = p(r#"CALL cgx.mutation_fanout("x") YIELD mutator, confidence AS c WHERE c = "certain" RETURN mutator"#);
        let c = match &q.parts[0].reading[0] { ReadingClause::Call(c) => c, _ => panic!() };
        assert_eq!(c.proc.value, "cgx.mutation_fanout");
        assert_eq!(c.args.len(), 1);
        assert_eq!(c.yields.len(), 2);
        assert_eq!(c.yields[1].alias.as_deref(), Some("c"));
        assert!(c.where_clause.is_some());
    }

    #[test]
    fn function_call_and_count_star() {
        let q = p("MATCH (a) RETURN count(*), length(p), collect(a.name)");
        let r = q.parts[0].ret.as_ref().unwrap();
        assert_eq!(r.items.len(), 3);
        match &r.items[0].expr {
            Expr::FunctionCall { name, args } => {
                assert_eq!(name.value, "count");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn list_comprehension() {
        let q = p("MATCH (a) RETURN [r IN relationships(p) WHERE r.condition = \"always\" | r.condition]");
        let r = q.parts[0].ret.as_ref().unwrap();
        match &r.items[0].expr {
            Expr::ListComp { var, filter, .. } => {
                assert_eq!(var, "r");
                assert!(filter.is_some());
            }
            other => panic!("expected ListComp, got {other:?}"),
        }
    }

    #[test]
    fn quantifier_none() {
        let q = p("MATCH p = (a)-[:CALLS*]->(b) WHERE NONE(r IN relationships(p) WHERE r.condition = \"exception\") RETURN p");
        let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        match m.where_clause.as_ref().unwrap() {
            Expr::Quantifier { kind: QuantifierKind::None, var, .. } => assert_eq!(var, "r"),
            other => panic!("expected NONE quantifier, got {other:?}"),
        }
    }

    #[test]
    fn bare_pattern_predicate_negated() {
        let q = p("MATCH (m:method) WHERE NOT (m)<-[:CALLS]-() RETURN m");
        let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        match m.where_clause.as_ref().unwrap() {
            Expr::PatternPredicate { negated, pattern } => {
                assert!(negated);
                assert_eq!(pattern.steps.len(), 1);
                assert_eq!(pattern.steps[0].0.direction, Direction::Backward);
            }
            other => panic!("expected PatternPredicate, got {other:?}"),
        }
    }

    #[test]
    fn grouped_expr_not_mistaken_for_pattern() {
        // `(a.line > 1)` is a grouped expression, not a node pattern.
        let q = p("MATCH (a) WHERE (a.line > 1) AND a.name = \"x\" RETURN a");
        let m = match &q.parts[0].reading[0] { ReadingClause::Match(m) => m, _ => panic!() };
        match m.where_clause.as_ref().unwrap() {
            Expr::Binary { op: BinOp::And, .. } => {}
            other => panic!("expected And, got {other:?}"),
        }
    }

    #[test]
    fn docs_05_worked_examples_parse() {
        // The five docs/05 canonical worked-example queries (design §8). These
        // must parse; lowering may later reject unbacked properties/edge types,
        // but parsing is syntactic and must succeed.
        let examples = [
            // Example 1
            r#"MATCH path = (src {name:"main::foo"})-[:CALLS*]->(dst {name:"vulnerable::bar"})
               WHERE NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
               RETURN path"#,
            // Example 2
            r#"MATCH flow = (src)-[:DATA_FLOW*1..8]->(sink {name:"amount", scope:"PaymentProcessor::charge"})
               RETURN src.name, src.file, src.line,
                      [r IN relationships(flow) | r.transformation] AS transformations"#,
            // Example 3 form 1
            r#"MATCH (m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
               WHERE NOT (m)<-[:CALLS]-()
               RETURN m.name, m.file, m.line
               ORDER BY m.name"#,
            // Example 3 form 2
            r#"MATCH (alloc {name:"new", type:"MyStruct"})-[:CALLS*]->(m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
               WITH collect(m.name) AS reached
               MATCH (all_m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
               WHERE NOT all_m.name IN reached
               RETURN all_m.name, all_m.file, all_m.line"#,
            // Example 4
            r#"MATCH path = (src)-[:CALLS* {condition: "exception"}]->(dst {name:"vulnerable::bar"})
               WHERE src.introduced_in_branch = "feature/risky-change"
               RETURN src.name, src.file, src.line, length(path) AS hops"#,
        ];
        for src in examples {
            parse(src).unwrap_or_else(|e| panic!("example failed to parse: {e}\n{src}"));
        }
    }

    // ---- one error case per Parse shape ----------------------------------

    #[test]
    fn err_missing_return() {
        assert_eq!(err("MATCH (a)").kind, ErrorKind::Parse);
    }

    #[test]
    fn err_unclosed_node() {
        assert_eq!(err("MATCH (a RETURN a").kind, ErrorKind::Parse);
    }

    #[test]
    fn err_unexpected_token_in_expr() {
        assert_eq!(err("MATCH (a) RETURN ,").kind, ErrorKind::Parse);
    }

    #[test]
    fn err_ambiguous_direction() {
        // `-[...]-` with no arrow on either side is ambiguous.
        assert_eq!(err("MATCH (a)-[:CALLS]-(b) RETURN a").kind, ErrorKind::Parse);
    }

    #[test]
    fn err_trailing_input() {
        assert_eq!(err("MATCH (a) RETURN a MATCH (b)").kind, ErrorKind::Parse);
    }

    #[test]
    fn err_bad_limit() {
        assert_eq!(err("MATCH (a) RETURN a LIMIT x").kind, ErrorKind::Parse);
    }

    #[test]
    fn err_span_points_at_token() {
        let e = err("MATCH (a) RETURN ,");
        // The offending comma is at byte 17.
        assert_eq!(e.span.start, 17);
    }
}
