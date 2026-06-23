//! Dataflow transform tags (docs/04 DF-10) carried on a `DerivesFrom` edge.
//!
//! v0.3.0 populates only the **structural** subset: the shape of the derivation,
//! not its security meaning. The security-typed kinds that carry a class argument
//! (`encode(class)`, `parameterize(class)`, `validate(class)`, `coerce(from,to)`)
//! are the taint engine's job — they are *reserved* in this enum so the taint
//! cycle needs no schema bump, but are **not emitted** in v0.3.0 (design §1.5).

use serde::{Deserialize, Serialize};

/// The structural shape of a `DerivesFrom` derivation (design §1.5).
///
/// Carried as `EdgeRecord.transform: Option<Transform>`; `None` on every
/// non-dataflow edge. Only the variants in the structural subset are emitted in
/// v0.3.0; security-typed kinds are reserved for the taint cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum Transform {
    /// A whole-value copy (`let b = a;`, `return x`).
    Copy = 0,
    /// A field/index projection at depth 1 (`let n = u.name;`).
    Projection = 1,
    /// An arithmetic or binary expression (`let c = a + b;`).
    Arith = 2,
    /// String/sequence concatenation.
    Concat = 3,
    /// Struct/tuple assembly (`let p = Point { x: a, y: b };`).
    Composed = 4,
    /// A conditional-select / φ join (`let v = if c { a } else { b };`).
    Branched = 5,
    /// A reduction over a collection (`sum`, `fold`).
    Aggregation = 6,
    /// A parse from a serialized form.
    Parse = 7,
    /// A serialize to an external form.
    Serialize = 8,
    /// A derivation whose structural shape is none of the above.
    Other = 9,
}
