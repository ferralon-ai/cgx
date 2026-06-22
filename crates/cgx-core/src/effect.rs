//! Syntactic own-effect labels (GM-12 Phase 1).
//!
//! An [`Effect`] is one member of the fixed GM-12 effect-label set. A function's
//! *own effects* are the effects its own body performs directly (a name-based
//! syntactic heuristic — see [`crate::node::NodeRecord::own_effects`]); the
//! *transitive effects* (GM-12 Phase 2 / P8b) union a function's own effects with
//! those of everything it calls. This module provides only the label type and the
//! [`EffectSet`] container; it computes nothing.
//!
//! ## Encoding
//!
//! [`EffectSet`] is a hand-rolled `u16` bitset (no external `bitflags` crate). The
//! set is `Ord` and serializes as its `u16` (transparent), so the canonical
//! postcard bytes are a pure function of the value — order never leaks (every bit
//! has a fixed position; [`EffectSet::iter`] yields effects in ascending bit
//! order). This keeps the type zero-alloc and deterministic, matching the
//! determinism contract of [`crate::cut::CutMarkers`].

use serde::{Deserialize, Serialize};

/// One GM-12 syntactic effect label. The discriminant is the bit position used
/// by [`EffectSet`]; it is stable and must never be reordered (it is the on-disk
/// encoding via the bitset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum Effect {
    /// Blocks the calling thread (`thread::sleep`, blocking lock/`recv`).
    Blocking = 0,
    /// Launches detached concurrent work (`thread::spawn`, `tokio::spawn`).
    Spawns = 1,
    /// Filesystem I/O (`std::fs`, `File`, `tokio::fs`).
    IoFile = 2,
    /// Network I/O (`std::net`, `TcpStream`/`TcpListener`, `reqwest`/`hyper`).
    IoNet = 3,
    /// Process I/O (`std::process::Command`, `std::process::*`).
    IoProc = 4,
    /// Loads or evaluates code at runtime (`libloading`, eval-like surfaces).
    DynamicCode = 5,
    /// Reads a nondeterministic source (`SystemTime::now`, `Instant::now`, `rand`).
    Nondeterministic = 6,
}

impl Effect {
    /// All effects in canonical (ascending bit) order. Iteration order for the
    /// whole crate; never reorder.
    pub const ALL: [Effect; 7] = [
        Effect::Blocking,
        Effect::Spawns,
        Effect::IoFile,
        Effect::IoNet,
        Effect::IoProc,
        Effect::DynamicCode,
        Effect::Nondeterministic,
    ];

    /// The bit this effect occupies in an [`EffectSet`].
    #[inline]
    const fn bit(self) -> u16 {
        1u16 << (self as u8)
    }

    /// The canonical dotted/kebab string form used for output and CQL
    /// (`blocking`, `io.file`, `dynamic-code`, …). Matches the GM-12 label set.
    pub const fn as_str(self) -> &'static str {
        match self {
            Effect::Blocking => "blocking",
            Effect::Spawns => "spawns",
            Effect::IoFile => "io.file",
            Effect::IoNet => "io.net",
            Effect::IoProc => "io.proc",
            Effect::DynamicCode => "dynamic-code",
            Effect::Nondeterministic => "nondeterministic",
        }
    }

    /// Parse a canonical string form back into an [`Effect`]. Inverse of
    /// [`Effect::as_str`]; `None` for any other string.
    pub fn parse(s: &str) -> Option<Effect> {
        Effect::ALL.into_iter().find(|e| e.as_str() == s)
    }
}

/// A deterministic set of [`Effect`]s, encoded as a `u16` bitset.
///
/// Unlike the `SmallVec`-backed [`CutMarkers`](crate::cut::CutMarkers), an effect
/// set is bounded and small, so a fixed-width bitset is both cheaper and trivially
/// canonical: equal sets have equal `u16`s regardless of insertion order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectSet(u16);

impl EffectSet {
    /// An empty effect set.
    pub const fn new() -> Self {
        EffectSet(0)
    }

    /// A set containing exactly one effect.
    pub const fn single(effect: Effect) -> Self {
        EffectSet(effect.bit())
    }

    /// Build a set from any iterator of effects (order-independent).
    pub fn from_iter_canonical<I: IntoIterator<Item = Effect>>(iter: I) -> Self {
        let mut set = EffectSet::new();
        for e in iter {
            set.insert(e);
        }
        set
    }

    /// Insert an effect (idempotent).
    pub fn insert(&mut self, effect: Effect) {
        self.0 |= effect.bit();
    }

    /// Union another set into this one in place.
    pub fn union_with(&mut self, other: EffectSet) {
        self.0 |= other.0;
    }

    /// The union of two sets.
    pub fn union(self, other: EffectSet) -> EffectSet {
        EffectSet(self.0 | other.0)
    }

    /// Whether a given effect is present.
    pub fn contains(self, effect: Effect) -> bool {
        self.0 & effect.bit() != 0
    }

    /// Whether the set is empty.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Number of effects in the set.
    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// The effects in canonical (ascending bit) order.
    pub fn iter(self) -> impl Iterator<Item = Effect> {
        Effect::ALL.into_iter().filter(move |e| self.contains(*e))
    }
}
