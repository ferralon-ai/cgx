//! Node taxonomy (GM-1) and the call-site node kind reserved by ADR-01.

use crate::id::{NodeId, SiteId};
use crate::provenance::Provenance;
use crate::signature::Signature;
use serde::{Deserialize, Serialize};

/// Symbol kinds (GM-1.1). Every named program entity is a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum SymbolKind {
    /// Named, freestanding callable (`fn parse_input`, `def process`).
    Function,
    /// Callable bound to a type (`impl Foo { fn bar() }`).
    Method,
    /// Struct, class, interface, trait, enum, union.
    Type,
    /// Named member of a type (`User.id`).
    Field,
    /// Local, parameter, or module-level binding.
    Variable,
    /// File- or package-level namespace.
    Module,
    /// Compile-time named value (`MAX_RETRIES`).
    Constant,
    /// Hygienic macro or preprocessor definition (`macro_rules!`, `#define`).
    Macro,
    /// Anonymous callable with a stable allocation site (closure, arrow fn).
    Lambda,
    /// Declared reachability root (GM-7).
    Entrypoint,
}

impl SymbolKind {
    /// Whether a symbol of this kind may carry a [`Signature`] (ADR-04: only
    /// `function`/`method`/`lambda`).
    pub fn is_callable(self) -> bool {
        matches!(
            self,
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Lambda
        )
    }
}

/// Node flavor (architecture §3 schema `flavor` column; ADR-01).
///
/// Two granularities share the node space. Symbol nodes are the primary query
/// surface. Call-site nodes are subordinate (ADR-01): they never appear in
/// symbol-pattern results and are reached only through a call edge's `site_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum NodeFlavor {
    /// A symbol node (GM-1). Discriminant 0 per the architecture schema.
    Symbol = 0,
    /// A call-site node (GM-1.4 / ADR-01). Discriminant 1 (reserved). Populated
    /// cheaply at index time (identity + ordinal + `suspends` only in Phase 1).
    CallSite = 1,
}

/// Language-mapped visibility (GM-1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Visibility {
    Public,
    Protected,
    Internal,
    Package,
    Private,
}

/// The class of an auto-detected or declared entrypoint (GM-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum EntrypointKind {
    /// Process entry (`fn main`, `func main`).
    Main,
    /// Test function (`#[test]`, `@Test`, `test_*`, jest `test()`).
    Test,
    /// HTTP/route handler.
    HttpHandler,
    /// Async runtime entry (`#[tokio::main]`).
    AsyncMain,
    /// Explicitly declared by the user (GM-7.2).
    Declared,
}

/// A symbol node record (GM-1.3). The persisted/queryable form of a symbol.
///
/// `id` is the dense [`NodeId`] for the linked graph; `confidence` is filled by
/// the resolver as the weakest confidence of any contributing fact (GM-1.3) and
/// is therefore not part of the frontend-emitted record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeRecord {
    pub id: NodeId,
    pub kind: SymbolKind,
    /// Fully-qualified name, `::`-separated (GM-1.2).
    pub fqn: String,
    /// Repo-relative source path.
    pub file: String,
    pub line_start: u32,
    pub line_end: u32,
    /// Source language tag.
    pub lang: String,
    pub visibility: Visibility,
    pub is_abstract: bool,
    /// Set when this symbol is an entrypoint (GM-7).
    pub entrypoint_kind: Option<EntrypointKind>,
    /// Structured signature (ADR-04); `None` for non-callable kinds or when the
    /// surface declares none.
    pub signature: Option<Signature>,
}

/// A call-site node record (GM-1.4 / ADR-01). Subordinate node kind.
///
/// Phase-1 attributes only: identity, the caller it belongs to, the lexical
/// ordinal within that caller, and the syntactic `suspends` flag (GM-10). The
/// reserved Phase-3 attributes (`cfg_block`, `dominating_sites`, `lock_set`,
/// `is_return_site`, arg bindings) are intentionally absent here — adding them
/// is an additive struct change, not a schema-flavor change.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CallSite {
    /// Deterministic content-addressed identity (ADR-01).
    pub site_id: SiteId,
    /// The symbol whose body contains this call site.
    pub caller: NodeId,
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// Lexical index of this site within the caller's body (GM-1.4 `ordinal`).
    pub ordinal: u32,
    /// Whether this site is a known suspension point (GM-10).
    pub suspends: bool,
}

impl CallSite {
    /// Construct a call-site record, deriving its [`SiteId`] from the identity
    /// tuple. `caller_fqn` is the FQN of the caller symbol (used only for the id
    /// derivation; the stored back-reference is `caller`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        caller: NodeId,
        caller_fqn: &str,
        file: impl Into<String>,
        line: u32,
        col: u32,
        ordinal: u32,
        suspends: bool,
    ) -> Self {
        let file = file.into();
        let site_id = SiteId::derive(caller_fqn, &file, line, col);
        CallSite {
            site_id,
            caller,
            file,
            line,
            col,
            ordinal,
            suspends,
        }
    }
}

/// A node record paired with its provenance, as carried through the model.
/// Provenance is separable from [`NodeRecord`] so callers that only need the
/// queryable surface need not materialize evidence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeWithProvenance {
    pub node: NodeRecord,
    pub provenance: Provenance,
}
