//! The plain-data AST the parser produces (design §2).
//!
//! These types are pure syntax: no graph ids, no resolution, no lowering
//! decisions. Spans are retained on the nodes the lowering and eval phases must
//! point errors at (edge types, properties, function names) so a Plan/Eval error
//! can render a caret without re-lexing.

use std::ops::Range;

use crate::value::Value;

/// A whole query: one or more parts chained by `WITH`. The final part must
/// carry a `RETURN` (enforced by the parser).
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub parts: Vec<QueryPart>,
}

/// One pipeline stage: a run of reading clauses, an optional terminal `RETURN`,
/// and the `WITH` projection that opens the *next* part (if any).
#[derive(Debug, Clone, PartialEq)]
pub struct QueryPart {
    pub reading: Vec<ReadingClause>,
    pub ret: Option<ReturnClause>,
    /// The `WITH` that closes this part and feeds the next. Present on every
    /// part except the last.
    pub with: Option<WithClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReadingClause {
    Match(MatchClause),
    Call(CallClause),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchClause {
    pub patterns: Vec<PathPattern>,
    pub where_clause: Option<Expr>,
}

/// A single comma-separated pattern, optionally bound to a `path =` variable.
#[derive(Debug, Clone, PartialEq)]
pub struct PathPattern {
    pub path_var: Option<String>,
    pub start: NodePat,
    /// Each step is the relationship plus the node it lands on.
    pub steps: Vec<(RelPat, NodePat)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodePat {
    pub var: Option<String>,
    /// The `:label` (a `SymbolKind` filter); span points at the label text.
    pub label: Option<Spanned<String>>,
    pub props: Vec<PropEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelPat {
    pub var: Option<String>,
    pub direction: Direction,
    /// One or more `:TYPE` alternatives (`A|B`); spans point at each type name.
    pub types: Vec<Spanned<String>>,
    pub var_len: Option<VarLen>,
    pub props: Vec<PropEntry>,
}

/// Arrow direction as written. `MEMBER_OF` inversion is a lowering concern, not
/// a parse concern; the AST records the literal arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// `-[...]->`
    Forward,
    /// `<-[...]-`
    Backward,
}

/// Variable-length bounds from `*`, `*n`, `*n..`, `*..m`, `*n..m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarLen {
    pub min: Option<u32>,
    pub max: Option<u32>,
}

/// One `key: literal` entry in a `{...}` property map; span covers the key.
#[derive(Debug, Clone, PartialEq)]
pub struct PropEntry {
    pub key: Spanned<String>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallClause {
    /// The qualified procedure name, e.g. `cgx.mutation_fanout`; span included.
    pub proc: Spanned<String>,
    pub args: Vec<Expr>,
    pub yields: Vec<YieldItem>,
    pub where_clause: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YieldItem {
    pub name: Spanned<String>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnClause {
    pub distinct: bool,
    pub items: Vec<ReturnItem>,
    pub order_by: Vec<OrderBy>,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnItem {
    pub expr: Expr,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderBy {
    pub expr: Expr,
    pub descending: bool,
}

/// A `WITH` projection: the `RETURN`-shaped carry set plus an optional filter
/// applied to the carried rows (design §3.4).
#[derive(Debug, Clone, PartialEq)]
pub struct WithClause {
    pub distinct: bool,
    pub items: Vec<ReturnItem>,
    pub order_by: Vec<OrderBy>,
    pub limit: Option<u64>,
    pub where_clause: Option<Expr>,
}

/// An expression node. Operator nodes carry the operator's span for error
/// reporting; leaf nodes carry the span of their salient identifier.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Value),
    /// A property path `n.name` (head + dotted segments). `head` alone (no
    /// segments) is a bare variable reference.
    Property { head: Spanned<String>, segments: Vec<Spanned<String>> },
    /// `name(arg, ...)` — function or aggregate call; span covers the name.
    FunctionCall { name: Spanned<String>, args: Vec<Expr> },
    /// `[x IN list WHERE p | proj]` — `filter` optional.
    ListComp { var: String, list: Box<Expr>, filter: Option<Box<Expr>>, projection: Box<Expr> },
    /// `NONE|ANY|ALL(x IN list WHERE pred)`.
    Quantifier { kind: QuantifierKind, var: String, list: Box<Expr>, predicate: Box<Expr> },
    /// A bare pattern used as a boolean predicate, optionally negated:
    /// `NOT (m)<-[:CALLS]-()` (Canonical Example 3).
    PatternPredicate { negated: bool, pattern: Box<PathPattern> },
    /// `lhs <op> rhs`.
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Range<usize> },
    /// `NOT expr`.
    Not(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantifierKind {
    None,
    Any,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    And,
    Or,
}

/// A value paired with its source span, for nodes that error reporting points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Range<usize>,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Range<usize>) -> Self {
        Spanned { value, span }
    }
}
