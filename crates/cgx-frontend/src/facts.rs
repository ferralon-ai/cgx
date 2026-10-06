//! Per-file facts a [`LanguageFrontend`](crate::LanguageFrontend) emits.
//!
//! [`FileFacts`] is the language-agnostic output of extracting one source file.
//! It is the central contract of the system (architecture §2, §5): a frontend
//! emits facts *about a single file with zero cross-file knowledge*, and the
//! shared resolver ([`cgx-resolve`], WP-06) joins fragments into a linked graph.
//! That split is what makes blob-OID fragment caching sound (architecture §3,
//! IX-1): the fragment is a pure function of the file's bytes.
//!
//! Every `Vec` in `FileFacts` is sorted into a canonical order before encoding
//! ([`FileFacts::canonicalize`]) so the postcard fragment is byte-identical
//! across runs (architecture §3 canonical-bytes property; WP-12 asserts it).
//!
//! [`cgx-resolve`]: https://docs.rs/cgx-resolve

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::edge::ImplicitKind;
use cgx_core::effect::EffectSet;
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_core::signature::Signature;
use cgx_core::transform::Transform;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// One `::`-separated segment of a name as it appears in source (`a.b.c` →
/// `["a", "b", "c"]`). The resolver maps these to FQNs; the frontend records
/// only what the source text says (architecture §5 `RawRef.name_path`).
pub type Name = String;

/// Identifier of a lexical scope within a single file's [`ScopeTree`].
///
/// Scope ids are dense indices into [`ScopeTree::scopes`], assigned in the
/// order scopes are pushed during extraction. They are file-local and not
/// portable across files. The file root is always [`ScopeId::ROOT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeId(pub u32);

impl ScopeId {
    /// The file-level (module) scope. Every [`ScopeTree`] has it at index 0.
    pub const ROOT: ScopeId = ScopeId(0);

    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A definition extracted from a file (architecture §5 `SymbolDef`).
///
/// `fqn` is the *local* FQN path the frontend can construct without cross-file
/// knowledge (e.g. `module::Type::method`); the resolver may rewrite or extend
/// it. `signature` is best-effort (ADR-04): present on callable kinds where the
/// surface declares one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolDef {
    /// Local fully-qualified path, `::`-separated (GM-1.2).
    pub fqn: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    /// Lexical scope this definition lives in.
    pub scope: ScopeId,
    /// Source span of the definition.
    pub span: Span,
    /// Last line of the definition's body (for `line_end` in the node record).
    pub line_end: u32,
    /// Interface / abstract-method / trait-declaration flag (GM-1.3).
    pub is_abstract: bool,
    /// Structured signature on callable kinds (ADR-04); `None` otherwise.
    pub signature: Option<Signature>,
}

/// What kind of reference a [`RawRef`] is (a pre-resolution view of the GM-2
/// edge kinds the resolver will assign).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum RefKind {
    /// A plain call `f(...)`.
    Call,
    /// A method/dispatch call on a receiver whose type is unknown to the
    /// frontend (`x.m(...)`) — the resolver decides `calls` vs `calls:virtual`.
    CallVirtualReceiver,
    /// Invocation of a closure/lambda bound in scope.
    CallClosure,
    /// Invocation via a function-value argument (callback).
    CallCallback,
    /// A logical call across an async suspension boundary (`.await`).
    CallAsync,
    /// A detached, non-awaited launch (`spawn`, `go`, unhandled promise).
    Spawn,
    /// A use that is not a call (type annotation, constant use).
    Reference,
    /// A construction/allocation of a type (`new T`, `T { .. }`).
    Instantiate,
}

/// A raw, unresolved reference site (architecture §5 `RawRef`).
///
/// The frontend records what the source *says* (`name_path`) plus the lexical
/// scope and the syntactic edge condition; the resolver turns this into one or
/// more graph edges with confidence and a resolved target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RawRef {
    /// What the source says, segment by segment: `a.b.c(...)` → `["a","b","c"]`.
    pub name_path: SmallVec<[Name; 2]>,
    /// Lexical scope of the reference.
    pub scope: ScopeId,
    pub kind: RefKind,
    /// Edge condition lowered by the frontend (GM-3.1, ADR-03).
    pub edge_condition: EdgeCondition,
    /// Implicit-call mechanism (GM-16), if this ref is an implicit call.
    pub implicit: Option<ImplicitKind>,
    /// Source span of the reference.
    pub span: Span,
    /// Intra-procedural lexical statement index of the call (ADR-02 reservation).
    pub stmt_index: u32,
    /// Callee arity if syntactically determinable (used by the Tier-0 name+arity
    /// resolution rule). `None` when the surface does not pin it.
    pub arity: Option<u8>,
    /// Cut markers the frontend already knows apply at this site (e.g. a call
    /// inside an `unsafe extern` block, a reflective dispatch). Sorted/deduped.
    pub cut_markers: SmallVec<[CutMarker; 1]>,
}

/// What kind of structural type/trait relation an [`ImplRelation`] records — the
/// pre-resolution view of the GM-2.2 lattice edge kinds the resolver assigns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum RelationKind {
    /// A type implements a trait (`impl Trait for T` → `T → Trait`).
    Implements,
    /// A trait inherits from a supertrait (`trait Sub: Super` → `Sub → Super`).
    Inherits,
    /// An impl method overrides a trait method (`Type::m → Trait::m`).
    Overrides,
}

/// A structural type/trait-lattice relation extracted from a file (GM-2.2).
///
/// Unlike a [`RawRef`] (a call/use site), this records a *declared* relation
/// between two named symbols: `subject` and `object` are name paths as written
/// in source (`["Circle"]` → `["Shape"]` for `impl Shape for Circle`). The
/// resolver maps both names to node ids and emits the corresponding structural
/// [`EdgeKind`](cgx_core::edge::EdgeKind). For [`RelationKind::Overrides`],
/// `subject` is the impl method's local FQN segments and `object` is the trait
/// method's `[Trait, method]` path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImplRelation {
    pub kind: RelationKind,
    /// The relation's source symbol, segment by segment.
    pub subject: SmallVec<[Name; 2]>,
    /// The relation's target symbol, segment by segment.
    pub object: SmallVec<[Name; 2]>,
    /// Source span of the declaring construct (the `impl`/`trait` header).
    pub span: Span,
}

/// An import fact (architecture §5 `ImportFact`).
///
/// The resolver builds the import graph from these. `specifier` is the
/// module/path as written (`std::collections`, `./util`, `react`); `names`
/// lists the imported bindings, with optional local aliases.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImportFact {
    /// Module/path specifier as written in source.
    pub specifier: String,
    /// Imported bindings. Empty with `glob = true` means "import everything".
    pub names: Vec<ImportedName>,
    /// Whether this is a glob/wildcard import (`use a::*`, `from m import *`).
    pub glob: bool,
    /// Whether this import is itself re-exported (`pub use`, `export ... from`).
    pub re_export: bool,
    /// Scope the import binding is visible in.
    pub scope: ScopeId,
    pub span: Span,
}

/// One imported name with its optional local alias.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImportedName {
    /// Name as exported by the source module.
    pub name: String,
    /// Local alias, if renamed on import (`use a::b as c` → `Some("c")`).
    pub alias: Option<String>,
}

/// An export fact (architecture §5 `ExportFact`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExportFact {
    /// Exported local name (the symbol's short name as exported).
    pub name: String,
    /// Local alias if exported under a different name (`export { a as b }`).
    pub alias: Option<String>,
    /// Whether this is a re-export from another module (`export ... from "m"`).
    /// When set, `from` carries the source specifier.
    pub from: Option<String>,
    pub span: Span,
}

/// A node in the per-file lexical scope tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Scope {
    /// Parent scope; `None` only for [`ScopeId::ROOT`].
    pub parent: Option<ScopeId>,
    /// The symbol whose body opened this scope, if any (a function body, a type
    /// body). `None` for anonymous block scopes.
    pub owner_fqn: Option<String>,
}

/// The per-file lexical scope tree (architecture §5 `ScopeTree`).
///
/// Built bottom-up by the frontend; consumed by the resolver's per-file pass to
/// resolve references to local definitions before the cross-file link step.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScopeTree {
    /// Scopes indexed by [`ScopeId`]. Index 0 is always the file root.
    pub scopes: Vec<Scope>,
}

impl Default for ScopeTree {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeTree {
    /// A fresh tree with only the file-root scope.
    pub fn new() -> Self {
        ScopeTree {
            scopes: vec![Scope {
                parent: None,
                owner_fqn: None,
            }],
        }
    }

    /// Push a child scope under `parent`, returning its id.
    pub fn push(&mut self, parent: ScopeId, owner_fqn: Option<String>) -> ScopeId {
        let id = ScopeId(self.scopes.len() as u32);
        self.scopes.push(Scope {
            parent: Some(parent),
            owner_fqn,
        });
        id
    }

    /// The number of scopes (always ≥ 1).
    pub fn len(&self) -> usize {
        self.scopes.len()
    }

    /// Always false: a tree always has the root scope.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Walk from `scope` up to the root, yielding each scope id in order.
    pub fn ancestors(&self, scope: ScopeId) -> impl Iterator<Item = ScopeId> + '_ {
        let mut cur = Some(scope);
        std::iter::from_fn(move || {
            let id = cur?;
            cur = self.scopes.get(id.index()).and_then(|s| s.parent);
            Some(id)
        })
    }
}

/// A hint that a symbol is an entrypoint (architecture §5 `EntrypointHint`,
/// GM-7.1). The frontend detects these syntactically; the resolver records them
/// on the node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntrypointHint {
    /// Local FQN of the entrypoint symbol (matches a [`SymbolDef::fqn`]).
    pub fqn: String,
    pub kind: EntrypointKind,
}

/// A syntactic own-effect fact for one symbol (GM-12 Phase 1, architecture §5).
///
/// The frontend detects effects by the *names* of the call/macro targets in a
/// function's body (a heuristic, hence `possible`-grade — see
/// [`EffectSet`](cgx_core::effect::EffectSet)). It records the union of effects
/// for one definition, keyed by that definition's local FQN; the resolver stamps
/// the set onto the matching node's
/// [`own_effects`](cgx_core::node::NodeRecord::own_effects) at link time. A
/// frontend that detects no effects emits no fact for the symbol.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EffectFact {
    /// Local FQN of the symbol these effects belong to (matches a
    /// [`SymbolDef::fqn`]).
    pub fqn: String,
    /// The detected own-effect set for that symbol.
    pub effects: EffectSet,
}

/// A single intraprocedural dataflow fact (design §2.1, v0.3 DATA_FLOW): the
/// value bound at `derived` was produced, in whole or in part, from the value at
/// `source`. The resolver maps both name-paths to SSA value nodes (the same
/// intraprocedural scope resolution `RawRef` uses) and emits a `DerivesFrom`
/// [`EdgeKind`](cgx_core::edge::EdgeKind) tagged with `transform`.
///
/// Both endpoints share a scope subtree (intraprocedural — SC2 emits no
/// cross-function facts; a call result records an `opaque-call` cut hint
/// instead). `derived`/`source` are depth-≤1 access paths (`["u","name"]`);
/// deeper paths truncate to the base with a `truncated-access-path` cut.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DataFlowFact {
    /// Access-path of the derived binding (the SSA def being written).
    pub derived: SmallVec<[Name; 2]>,
    /// Access-path of the source binding (a use feeding the derived value).
    pub source: SmallVec<[Name; 2]>,
    /// The SSA version of `derived` at this definition. Re-assignment
    /// (`x = a; x = b;`) increments it, so the resolver mints distinct value
    /// nodes (criterion 2, flow-sensitivity).
    pub derived_version: u32,
    /// Lexical scope of the function body both endpoints live in.
    pub scope: ScopeId,
    /// Structural transform tag (design §1.5).
    pub transform: Transform,
    /// Edge condition lowered by the frontend, exactly like [`RawRef`].
    pub edge_condition: EdgeCondition,
    /// Cut markers the frontend already knows apply (e.g. `OpaqueCall` for a flow
    /// through a call result, `TruncatedAccessPath` for a depth-2+ field access).
    pub cut_markers: SmallVec<[CutMarker; 1]>,
    /// The syntactic callee name path of an `OpaqueCall` fact, `::`-joined
    /// (`helper`, `Foo::method`). `None` for non-call facts. The resolver grounds
    /// this against the symbol table to record a `summary_deps` row (v0.3 SC3): a
    /// resolved name yields the callee's real FQN, an unresolved/virtual one a
    /// `'*'` wildcard. Additive (`#[serde(default)]`) — no fragment-version bump.
    #[serde(default)]
    pub callee_fqn: Option<String>,
    /// Per-argument source access-paths of an `OpaqueCall` fact (v0.3 SC4): the
    /// `i`th entry is the access-path(s) the `i`th positional argument reads
    /// (`g(a, b.x)` → `[["a"], ["b","x"]]`). Empty for non-call facts. The IFDS
    /// summary-application pass maps a callee's `formal_in_i` to `args[i]` in the
    /// caller's value-node space, so a `r = g(a)` site can materialize the
    /// interprocedural `DerivesFrom` edge `r ⇝ a`. An argument that is not a plain
    /// name/field (a literal, a nested call) contributes an empty inner vec, so
    /// positional indices stay aligned with the callee's parameter order.
    /// Additive (`#[serde(default)]`) — no fragment-version bump.
    #[serde(default)]
    pub args: SmallVec<[SmallVec<[Name; 2]>; 4]>,
    /// Source span of the assignment.
    pub span: Span,
}

/// A hint that some call edges are structurally invisible at this site
/// (architecture §5 `CutHint`, GM-5.3 / ADR-07). The frontend records the cut
/// so the edge is never silently dropped; the resolver stamps the marker on the
/// emitted edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CutHint {
    pub marker: CutMarker,
    pub span: Span,
    /// The macro/codegen origin, when `marker == UnexpandedMacro` (GM-14.5).
    pub macro_origin: Option<String>,
}

/// A receiver-typing fact: the syntactic input the resolver uses to infer which
/// classes a method-call receiver can hold. Frontends normalize language syntax
/// into the structured [`TypeExpr`] / [`ValueSource`] payloads, so the resolver
/// stays language-generic.
///
/// `func` is the owning callable's local FQN and `class` a class's local FQN;
/// both match a [`SymbolDef::fqn`] in the same file.
///
/// **Binding completeness (the channel's contract).** For every callable
/// `func` the frontend emits a [`TypeFact::Param`] per declared parameter or
/// receiver, and **one [`TypeFact::Bind`] per binding site of every local
/// name**. That covers assignments, augmented assignments, walrus, tuple and
/// star targets, `for`/`with`/`except`/`match` captures, comprehension
/// variables, `global`/`nonlocal`, function-local `import`/`def`/`class`, and
/// `del`. It also covers names bound in code that is attributed to `func` but
/// scoped more tightly (lambda and func-literal parameters), names rebound
/// from a nested callable (`nonlocal`), and locals whose address is taken
/// (Go `&x`). A form the frontend does not type emits `Bind { src: Opaque }`.
///
/// The contract holds only for frontends that emit this channel (Python and
/// Go); a file without `type_facts` makes no claim about its names. Within such
/// a file, a name with no `Param` and no `Bind` in `func` is not local to
/// `func`: a consumer looks it up in the facts of the enclosing callables
/// (`func`'s FQN prefixes) before treating it as module-level, imported or
/// builtin.
///
/// **ABI.** Fragments are postcard-encoded, which writes the variant index.
/// Variants are append-only: never reorder them or change a variant's fields.
/// The same holds for every enum and struct in a variant's payload; a pinned
/// encoding test guards it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TypeFact {
    /// A declared parameter or method receiver of `func`.
    Param {
        /// Owning callable.
        func: String,
        /// Parameter name as written (`self`, `ctx`, the Go receiver name).
        name: Name,
        /// Position among the callable's parameters (0-based, saturating at
        /// `u8::MAX`); `None` for a [`ParamKind::Receiver`].
        index: Option<u8>,
        /// How the parameter binds arguments.
        kind: ParamKind,
        /// The declared type, normalized; `None` when unannotated.
        ty: Option<TypeExpr>,
    },
    /// One binding site of local `var` inside `func`.
    Bind {
        /// Owning callable.
        func: String,
        /// The bound local name.
        var: Name,
        /// Where the bound value comes from.
        src: ValueSource,
    },
    /// An instance-field assignment on the receiver (`self.f = …`,
    /// `this.f = …`) inside method `func`.
    FieldBind {
        /// Owning method.
        func: String,
        /// The field name (`f`).
        field: Name,
        /// Where the stored value comes from.
        src: ValueSource,
    },
    /// The declared base classes of `class`, in source order. A class with no
    /// explicit base gets an empty `bases` (still a fact: its lookup reaches
    /// the language root only). Bases the frontend cannot name are kept as
    /// [`BaseExpr::Unknown`] / [`BaseExpr::Call`], never dropped.
    ClassBases {
        /// The class's local FQN.
        class: String,
        /// Bases in declaration order (keyword arguments such as
        /// `metaclass=` are excluded).
        bases: Vec<BaseExpr>,
    },
    /// A method call whose receiver chain is rooted at a non-name expression
    /// (`"".join(x)`, `super().m()`, `Foo().m()`, `xs[0].m()`).
    AnonReceiver {
        /// Owning callable.
        func: String,
        /// Line of the call; equals the matching [`RawRef::span`] line.
        line: u32,
        /// Column of the call; equals the matching [`RawRef::span`] column.
        col: u32,
        /// The called method's name (the call ref's last name-path segment).
        method: Name,
        /// Number of attribute segments after the root (the call ref's
        /// name-path length).
        depth: u8,
        /// What the chain is rooted at.
        root: AnonRoot,
    },
    /// The arguments at one call site inside `func`.
    CallArgs {
        /// Owning callable.
        func: String,
        /// Line of the call; equals the matching [`RawRef::span`] line.
        line: u32,
        /// Column of the call; equals the matching [`RawRef::span`] column.
        col: u32,
        /// The syntactic callee name path (`f`, `pkg.F` as `["pkg","F"]`).
        callee: SmallVec<[Name; 2]>,
        /// Arguments in source order. A `Vec` rather than an inline small
        /// vector keeps every `TypeFact` small; the encoding is the same.
        args: Vec<CallArg>,
    },
    /// The declared return type of `func`.
    Return {
        /// The callable.
        func: String,
        /// Its declared return type, normalized.
        ty: TypeExpr,
    },
    /// The declared type of field `field` of `class`.
    FieldType {
        /// The class's local FQN.
        class: String,
        /// The field name.
        field: Name,
        /// Its declared type, normalized.
        ty: TypeExpr,
    },
}

/// How a [`TypeFact::Param`] binds call arguments. Variant order is ABI
/// (append-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ParamKind {
    /// A method receiver declared outside the parameter list (Go `func (r *T)`).
    Receiver,
    /// A positional (or positional-or-keyword) parameter. Python `self`/`cls`
    /// are ordinary positional parameters at index 0.
    Positional,
    /// A keyword-only parameter (Python, after `*` or `*args`).
    KeywordOnly,
    /// A variadic positional parameter (Python `*args`, Go `...T`).
    VarArgs,
    /// A variadic keyword parameter (Python `**kwargs`).
    VarKeywords,
}

/// One argument of a [`TypeFact::CallArgs`] site. Its fields are ABI like a
/// variant's: never reorder or change them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CallArg {
    /// The keyword, for a keyword argument (`f(x=1)`); `None` for positional.
    pub keyword: Option<Name>,
    /// Where the argument's value comes from.
    pub value: ValueSource,
}

/// Where a bound value comes from, as far as one file can see. Variant order
/// is ABI (append-only).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ValueSource {
    /// A copy of a local or parameter of the same callable.
    Var(Name),
    /// A syntactic allocation of exactly this type (`T{}`, `&T{}`, `new T()`).
    New(TypeExpr),
    /// The result of calling a name path (`Foo()`, `pkg.make()`). Whether that
    /// is a constructor or a function is for the resolver to decide.
    Call(SmallVec<[Name; 2]>),
    /// A declared bound: an annotation, Go `var v T`, Python `except E as e`.
    Declared(TypeExpr),
    /// The value a context manager yields: `with <src> as v`.
    Enter(Box<ValueSource>),
    /// An element of an iterable: `for v in <src>`.
    Element(Box<ValueSource>),
    /// A builtin literal of this kind.
    Literal(LiteralKind),
    /// `None` / `nil` / `null`: contributes no type.
    Null,
    /// Any other value. A binding from `Opaque` poisons the name: no type can
    /// be inferred for it in this callable.
    Opaque,
}

/// The kind of a builtin literal. Variant order is ABI (append-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LiteralKind {
    /// A text string.
    Str,
    /// A byte string.
    Bytes,
    /// An integer or floating-point number.
    Num,
    /// A boolean.
    Bool,
    /// A list (or list comprehension).
    List,
    /// A dictionary (or dict comprehension).
    Dict,
    /// A set (or set comprehension).
    Set,
    /// A tuple.
    Tuple,
}

/// The root of an [`TypeFact::AnonReceiver`] chain. Variant order is ABI
/// (append-only).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AnonRoot {
    /// A builtin literal (`"".join`, `[].append`).
    Literal(LiteralKind),
    /// A `super()` call.
    Super,
    /// The result of calling a name path (`Foo().m()`).
    Call(SmallVec<[Name; 2]>),
    /// A subscript (`xs[0].m()`).
    Subscript,
    /// Any other expression.
    Other,
}

/// One declared base of a [`TypeFact::ClassBases`]. Variant order is ABI
/// (append-only).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BaseExpr {
    /// A nameable base type (`Base`, `pkg.Base`, `Generic[T]`).
    Type(TypeExpr),
    /// A base produced by calling a name path (`declarative_base()`,
    /// `namedtuple(...)`).
    Call(SmallVec<[Name; 2]>),
    /// A base expression the frontend cannot name.
    Unknown,
}

/// A normalized type expression. Language syntax (`Optional[X]`, `X | None`,
/// string annotations, Go `*T`) is resolved by the frontend. Variant order is
/// ABI (append-only).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TypeExpr {
    /// A named type as written (`Foo`, `pkg.Foo` as `["pkg","Foo"]`).
    Named {
        /// The name path, unresolved.
        path: SmallVec<[Name; 2]>,
        /// Whether the value is held through a pointer or reference (Go `*T`).
        indirect: bool,
    },
    /// A generic instantiation (`List[Foo]`, `Box[T]`, Go `List[T]`).
    Generic {
        /// The generic type's name path.
        head: SmallVec<[Name; 2]>,
        /// Type arguments in source order.
        args: Vec<TypeExpr>,
        /// Whether the value is held through a pointer or reference (Go
        /// `*G[A]`).
        indirect: bool,
    },
    /// One of several types. `None`/`nil`/`null` members are dropped by the
    /// frontend, and a one-member union is collapsed to its member.
    Union(Vec<TypeExpr>),
    /// A type the frontend does not model (callables, literal types, `Any`,
    /// structural types).
    Unknown,
}

/// The complete set of facts extracted from one source file (architecture §5).
///
/// Canonical form (every `Vec` sorted) is what gets `postcard`-encoded into a
/// blob fragment. Call [`FileFacts::canonicalize`] before encoding; the helper
/// constructors here do not sort, so a frontend can build incrementally.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FileFacts {
    /// Definitions in this file.
    pub defs: Vec<SymbolDef>,
    /// Raw, unresolved references / call sites.
    pub refs: Vec<RawRef>,
    /// Structural type/trait-lattice relations (Implements/Inherits/Overrides).
    pub impl_relations: Vec<ImplRelation>,
    /// Import facts.
    pub imports: Vec<ImportFact>,
    /// Export facts.
    pub exports: Vec<ExportFact>,
    /// The lexical scope tree.
    pub scopes: ScopeTree,
    /// Entrypoint hints (GM-7).
    pub entrypoint_hints: Vec<EntrypointHint>,
    /// Cut hints (GM-5.3 / ADR-07).
    pub cut_hints: Vec<CutHint>,
    /// Syntactic own-effect facts (GM-12 Phase 1), one per symbol with any
    /// detected effect.
    pub effects: Vec<EffectFact>,
    /// Intraprocedural dataflow facts (v0.3 DATA_FLOW SC2). Empty for frontends
    /// that emit no dataflow (TS in the MVP) and for the base index path.
    #[serde(default)]
    pub data_flows: Vec<DataFlowFact>,
    /// Canonical module path this file's defs are rooted at — exactly the
    /// prefix the frontend joined every def FQN under (`django::db::models`,
    /// `example.com/app/svc`). `None` for frontends without a module identity.
    #[serde(default)]
    pub module: Option<String>,
    /// Receiver-typing facts (see [`TypeFact`]). Binding-complete per callable.
    #[serde(default)]
    pub type_facts: Vec<TypeFact>,
}

impl FileFacts {
    /// An empty, well-formed fact set: no defs/refs/etc., a scope tree with only
    /// the file root. This is the degraded baseline an unknown-language file
    /// produces — well-formed, never an error (architecture §5 Tier-0 posture).
    pub fn empty() -> Self {
        FileFacts {
            scopes: ScopeTree::new(),
            ..FileFacts::default()
        }
    }

    /// Sort every `Vec` into the canonical order required for byte-identical
    /// postcard encoding (architecture §3). Idempotent.
    ///
    /// Scope ids are *not* reordered (they are dense indices referenced by
    /// defs/refs/imports); only the fact vectors are sorted, by their derived
    /// `Ord`, which leads with the source span / FQN / name.
    pub fn canonicalize(&mut self) {
        self.defs.sort();
        self.refs.sort();
        self.impl_relations.sort();
        self.imports.sort();
        self.exports.sort();
        self.entrypoint_hints.sort();
        self.cut_hints.sort();
        self.effects.sort();
        self.data_flows.sort();
        self.type_facts.sort();
        self.type_facts.dedup();
        for r in &mut self.refs {
            r.cut_markers.sort_unstable();
            r.cut_markers.dedup();
        }
        for df in &mut self.data_flows {
            df.cut_markers.sort_unstable();
            df.cut_markers.dedup();
        }
    }

    /// Whether this fact set is the empty baseline (no facts beyond the root
    /// scope). Used by callers that want to flag a file the fallback could not
    /// extract anything from.
    pub fn is_degraded_empty(&self) -> bool {
        self.defs.is_empty()
            && self.refs.is_empty()
            && self.impl_relations.is_empty()
            && self.imports.is_empty()
            && self.exports.is_empty()
            && self.entrypoint_hints.is_empty()
            && self.cut_hints.is_empty()
            && self.effects.is_empty()
            && self.data_flows.is_empty()
            && self.type_facts.is_empty()
    }
}
