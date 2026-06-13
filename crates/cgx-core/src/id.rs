//! Deterministic identifiers for the graph (architecture §3, ADR-01).
//!
//! Two distinct identity schemes coexist:
//!
//! - **Dense indices** ([`NodeId`], [`EdgeId`]) are assigned to a *linked graph*
//!   after sorting its symbols by [`NodeSortKey`] (`(file, line_start, fqn)`).
//!   They are stable for a fixed tree but are not portable across trees. Two
//!   index runs of the same tree produce identical assignments.
//! - **Content-addressed site identity** ([`SiteId`]) for call-site nodes
//!   (ADR-01) is derived from `(caller_fqn, file, line, col)` so it is stable
//!   across re-indexes of an unchanged blob, independent of insertion order.

use serde::{Deserialize, Serialize};

/// Dense index of a symbol node within a single linked graph.
///
/// Assigned by sorting all symbols by [`NodeSortKey`] and taking the position.
/// Never random, never insertion-ordered (architecture §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u32);

impl NodeId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Dense index of an edge within a single linked graph. Assigned after edges are
/// placed in canonical sort order ([`crate::sort::EdgeSortKey`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EdgeId(pub u32);

impl EdgeId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Deterministic, content-addressed identity of a call-site node (ADR-01).
///
/// Identity is `(caller symbol FQN, file, line, col)`. The stored value is a
/// 64-bit FNV-1a hash of the canonical rendering of those fields, which makes it
/// stable across re-indexes of an unchanged blob and independent of insertion
/// order — the determinism requirement ADR-01 places on `site_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SiteId(pub u64);

impl SiteId {
    /// Derive the deterministic site id from its identity tuple.
    ///
    /// The fields are folded with a length-prefixed, separator-free encoding so
    /// that, e.g., `("a", "bc")` and `("ab", "c")` cannot collide by
    /// concatenation. Pure function of its inputs: no randomness, no time.
    pub fn derive(caller_fqn: &str, file: &str, line: u32, col: u32) -> SiteId {
        let mut hash = FNV_OFFSET;
        fold_str(&mut hash, caller_fqn);
        fold_str(&mut hash, file);
        fold_u32(&mut hash, line);
        fold_u32(&mut hash, col);
        SiteId(hash)
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
fn fold_byte(hash: &mut u64, byte: u8) {
    *hash ^= byte as u64;
    *hash = hash.wrapping_mul(FNV_PRIME);
}

#[inline]
fn fold_u32(hash: &mut u64, value: u32) {
    for byte in value.to_le_bytes() {
        fold_byte(hash, byte);
    }
}

#[inline]
fn fold_str(hash: &mut u64, s: &str) {
    // Length prefix removes concatenation ambiguity between adjacent fields.
    fold_u32(hash, s.len() as u32);
    for byte in s.as_bytes() {
        fold_byte(hash, *byte);
    }
}

/// The canonical ordering key for a symbol node: `(file, line_start, fqn)`
/// (architecture §3). Sorting nodes by this key, then taking dense positions,
/// is the [`NodeId`] assignment rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeSortKey {
    pub file: String,
    pub line_start: u32,
    pub fqn: String,
}

impl NodeSortKey {
    pub fn new(file: impl Into<String>, line_start: u32, fqn: impl Into<String>) -> Self {
        NodeSortKey {
            file: file.into(),
            line_start,
            fqn: fqn.into(),
        }
    }
}
