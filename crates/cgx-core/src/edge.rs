//! Edge taxonomy (GM-2, GM-9, GM-16, GM-17) and the edge record.

use crate::condition::EdgeCondition;
use crate::confidence::{Confidence, Tier};
use crate::cut::CutMarkers;
use crate::id::{EdgeId, NodeId, SiteId};
use crate::provenance::Provenance;
use crate::transform::Transform;
use serde::{Deserialize, Serialize};

/// Edge kinds (GM-2.1 call edges, GM-2.2 structural/dataflow, GM-9 `spawns`).
///
/// `spawns` is an edge *kind*, orthogonal to the edge-condition label set
/// (GM-9.1); a spawn inside a `catch` is `Spawns` with condition `Exception`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum EdgeKind {
    // --- Call edges (GM-2.1) ---
    /// Direct invocation of a callable.
    Calls,
    /// Dispatch through a virtual/interface/trait method.
    CallsVirtual,
    /// Invocation of a closure or lambda captured from scope.
    CallsClosure,
    /// Invocation via a function-value argument.
    CallsCallback,
    /// Logical call across an async suspension boundary (`.await`).
    CallsAsync,
    /// Call via function pointer without resolved target.
    CallsIndirect,

    // --- Concurrency (GM-9) ---
    /// Detached, asynchronous launch of a task; spawner does not await it.
    Spawns,

    // --- Structural / dataflow edges (GM-2.2) ---
    /// Lexical containment (module contains type; type contains method/field).
    Contains,
    /// One module imports a symbol from another.
    Imports,
    /// A value is derived from another value (docs/04).
    DerivesFrom,
    /// A method overrides a method in a supertype.
    Overrides,
    /// A type implements an interface or trait.
    Implements,
    /// A class inherits from a superclass.
    Inherits,
    /// A symbol uses another without calling it (type annotation, constant use).
    References,
    /// A call site allocates an instance of a type (`new T`, `T { }`).
    Instantiates,
    /// A callable may propagate an exception of a given type to its callers.
    Throws,
    /// A callable handles an exception of a given type.
    Catches,
    /// A method reads a specific field from an instance in scope.
    ReadsField,
    /// A method writes a specific field on an instance in scope.
    WritesField,
}

impl EdgeKind {
    /// Whether this is a call-family edge (GM-2.1) — the kinds that carry a
    /// `site_id` (ADR-01) and an edge condition (GM-3). `Spawns` is included:
    /// it originates at a call site and carries a condition (GM-9.1).
    pub fn is_call(self) -> bool {
        matches!(
            self,
            EdgeKind::Calls
                | EdgeKind::CallsVirtual
                | EdgeKind::CallsClosure
                | EdgeKind::CallsCallback
                | EdgeKind::CallsAsync
                | EdgeKind::CallsIndirect
                | EdgeKind::Spawns
        )
    }
}

/// The mechanism behind an implicit call site (GM-16.1). Present on a `Calls`
/// edge that has no explicit call syntax in source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum ImplicitKind {
    Drop,
    Destructor,
    Deref,
    Coercion,
    Operator,
    Iterator,
    ContextEnter,
    ContextExit,
    Property,
    Defer,
    StaticInit,
    Conversion,
}

/// Evidence class for a mediated call edge (GM-17.1 `established-by`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum EstablishedBy {
    /// Declared by a metadata annotation on one or both symbols.
    Annotation,
    /// Declared in an external configuration file.
    ConfigFile,
    /// Established by an explicit register/subscribe call in source.
    RegistrationSite,
    /// Inferred from a naming or structural convention.
    Convention,
}

/// An edge record (GM-2.3 common attributes plus the call-edge extensions).
///
/// `id` is the dense [`EdgeId`] within the linked graph. `site_id` is present on
/// call-family edges (ADR-01 / GM-2.3) and `None` on structural edges.
/// `candidate_group` groups the parallel edges of a single over-approximated
/// call site into one candidate set (GM-2.1); members are listed in the
/// graph's candidate table (architecture §3 `candidates`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeRecord {
    pub id: EdgeId,
    pub src: NodeId,
    pub dst: NodeId,
    pub kind: EdgeKind,
    /// The single edge-condition label (GM-3, ADR-03). Meaningful for call-family
    /// edges; structural edges carry [`EdgeCondition::Always`].
    pub condition: EdgeCondition,
    pub confidence: Confidence,
    pub tier: Tier,
    /// Producing rule name (mirrors `provenance.rule`; denormalized for cheap
    /// filtering, architecture §3 `edges.rule`).
    pub rule: String,
    /// Call-site this edge originates from (ADR-01); `None` on structural edges.
    pub site_id: Option<SiteId>,
    /// Intra-procedural ordering reservation (ADR-02): lexical statement index of
    /// the call within the caller's body.
    pub stmt_index: Option<u32>,
    pub cut_markers: CutMarkers,
    /// Implicit-call mechanism (GM-16), if this edge is an implicit call.
    pub implicit: Option<ImplicitKind>,
    /// Candidate-set group id (GM-2.1) when the call is over-approximated.
    pub candidate_group: Option<u32>,
    /// Mediated-edge evidence class (GM-17), if applicable.
    pub established_by: Option<EstablishedBy>,
    /// Build-configuration condition (GM-19); `None` if unconditionally present.
    pub cfg_condition: Option<String>,
    /// Macro/codegen origin (GM-14.5); `None` if developer-written.
    pub macro_origin: Option<String>,
    /// Structural transform tag on a `DerivesFrom` dataflow edge (design §1.5);
    /// `None` on every non-dataflow edge. `#[serde(default)]` so existing
    /// postcard rows (which predate this field) decode as `None`.
    #[serde(default)]
    pub transform: Option<Transform>,
}

/// An edge record paired with its provenance (GM-6).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeWithProvenance {
    pub edge: EdgeRecord,
    pub provenance: Provenance,
}

/// One member of a candidate set (architecture §3 `candidates` table; GM-2.1).
/// `rank` orders candidates by estimated probability (lower = more likely).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Candidate {
    pub candidate_group: u32,
    pub dst: NodeId,
    pub rank: u32,
}
