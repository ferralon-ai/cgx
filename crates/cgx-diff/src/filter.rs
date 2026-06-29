//! Diff post-filters (CH-11 S0): pure set math over a [`GraphDiff`].
//!
//! Every filter here is a **post-classification** predicate over an
//! already-computed [`GraphDiff`] — no re-parse, no graph walk, no I/O. Filtering
//! is order-preserving (the buckets stay in their deterministic [`EdgeIdentity`]
//! order) and idempotent, so the determinism guarantee in `diff.rs`'s module doc
//! survives unchanged.
//!
//! The flags compose: a [`DiffFilter`] with several predicates set keeps only the
//! edges that satisfy every active predicate, in the buckets that are selected.

use crate::diff::{ChangedEdge, DiffEdge, GraphDiff};
use cgx_core::{EdgeCondition, EdgeKind, SymbolPattern};

/// Which diff buckets a filter prints. The default selects all three edge buckets
/// (added/removed/changed) — `cgx diff` with no bucket flag is unchanged.
///
/// Node buckets follow the edge selection: `added` keeps added nodes, `removed`
/// keeps removed nodes. (A node has no "changed" bucket.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketSelect {
    pub added: bool,
    pub removed: bool,
    pub changed: bool,
}

impl Default for BucketSelect {
    /// All buckets — the no-flag default of `cgx diff`.
    fn default() -> Self {
        BucketSelect {
            added: true,
            removed: true,
            changed: true,
        }
    }
}

impl BucketSelect {
    /// Build from the three CLI flags. When none of `added`/`removed`/`changed`
    /// were passed (all `false`), default to all three (the no-flag behavior);
    /// otherwise honor exactly the buckets the user named.
    pub fn from_flags(added: bool, removed: bool, changed: bool) -> Self {
        if !added && !removed && !changed {
            BucketSelect::default()
        } else {
            BucketSelect {
                added,
                removed,
                changed,
            }
        }
    }
}

/// A pure post-filter over a [`GraphDiff`] (CH-11 S0).
///
/// All fields are independent predicates; an unset predicate (`None` / empty)
/// admits everything. [`DiffFilter::apply`] returns a new `GraphDiff` keeping only
/// the selected buckets' records that satisfy every active predicate.
#[derive(Debug, Clone, Default)]
pub struct DiffFilter {
    /// Which buckets to keep.
    pub buckets: BucketSelect,
    /// Keep only edges whose [`EdgeKind`] is in this set. Empty = any kind.
    pub kinds: Vec<EdgeKind>,
    /// Keep only edges with exactly this edge condition. `None` = any condition.
    pub edge_condition: Option<EdgeCondition>,
    /// Keep only edges whose **source** FQN matches this glob. `None` = any source.
    pub from: Option<SymbolPattern>,
    /// Keep only edges whose **destination** FQN matches this glob. `None` = any.
    pub to: Option<SymbolPattern>,
}

impl DiffFilter {
    /// Build a filter with the given buckets and no edge predicates.
    pub fn with_buckets(buckets: BucketSelect) -> Self {
        DiffFilter {
            buckets,
            ..Default::default()
        }
    }

    /// Whether any edge predicate (kind/condition/from/to) is active. When false,
    /// `apply` only does bucket selection.
    fn has_edge_predicate(&self) -> bool {
        !self.kinds.is_empty()
            || self.edge_condition.is_some()
            || self.from.is_some()
            || self.to.is_some()
    }

    /// Whether `src_fqn -> dst_fqn` of the given kind/condition passes every active
    /// edge predicate.
    fn admits(
        &self,
        src_fqn: &str,
        dst_fqn: &str,
        kind: EdgeKind,
        condition: EdgeCondition,
    ) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&kind) {
            return false;
        }
        if let Some(want) = self.edge_condition {
            if condition != want {
                return false;
            }
        }
        if let Some(pat) = &self.from {
            if !glob_matches_fqn(pat, src_fqn) {
                return false;
            }
        }
        if let Some(pat) = &self.to {
            if !glob_matches_fqn(pat, dst_fqn) {
                return false;
            }
        }
        true
    }

    fn admits_diff_edge(&self, e: &DiffEdge) -> bool {
        self.admits(
            &e.identity.src_fqn,
            &e.identity.dst_fqn,
            e.identity.edge_kind,
            e.record.condition,
        )
    }

    fn admits_changed_edge(&self, e: &ChangedEdge) -> bool {
        // A changed edge's condition may differ between sides; admit it if *either*
        // side's condition passes the condition predicate (the change is in scope
        // if it touches a matching condition on either end).
        if !self.kinds.is_empty() && !self.kinds.contains(&e.identity.edge_kind) {
            return false;
        }
        if let Some(want) = self.edge_condition {
            if e.base.condition != want && e.head.condition != want {
                return false;
            }
        }
        if let Some(pat) = &self.from {
            if !glob_matches_fqn(pat, &e.identity.src_fqn) {
                return false;
            }
        }
        if let Some(pat) = &self.to {
            if !glob_matches_fqn(pat, &e.identity.dst_fqn) {
                return false;
            }
        }
        true
    }

    /// Apply the filter to `diff`, returning a new `GraphDiff` containing only the
    /// selected buckets' records that satisfy every active predicate. Order within
    /// each surviving bucket is preserved (the input is already canonical).
    pub fn apply(&self, diff: &GraphDiff) -> GraphDiff {
        let keep_edge = |e: &DiffEdge| self.admits_diff_edge(e);

        let added_edges = if self.buckets.added {
            diff.added_edges
                .iter()
                .filter(|e| keep_edge(e))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let removed_edges = if self.buckets.removed {
            diff.removed_edges
                .iter()
                .filter(|e| keep_edge(e))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let changed_edges = if self.buckets.changed {
            diff.changed_edges
                .iter()
                .filter(|e| self.admits_changed_edge(e))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        // Node buckets: only meaningful when no edge predicate is active. An edge
        // predicate (kind/condition/from/to) is about edges; applying it would drop
        // every node, which is surprising. So when an edge predicate is set we
        // suppress node output entirely; otherwise nodes follow bucket selection.
        let (added_nodes, removed_nodes) = if self.has_edge_predicate() {
            (Vec::new(), Vec::new())
        } else {
            (
                if self.buckets.added {
                    diff.added_nodes.clone()
                } else {
                    Vec::new()
                },
                if self.buckets.removed {
                    diff.removed_nodes.clone()
                } else {
                    Vec::new()
                },
            )
        };

        GraphDiff {
            added_edges,
            removed_edges,
            changed_edges,
            added_nodes,
            removed_nodes,
        }
    }
}

/// Match an FQN against a [`SymbolPattern`]. The diff filter always uses a glob
/// pattern (the CLI builds `SymbolPattern::glob`), so this delegates to the
/// pattern's own matcher via a synthetic node-free path: `SymbolPattern::matches`
/// needs a `NodeRecord`, but glob matching only reads the FQN, so we expose the
/// FQN-only form here to avoid constructing a throwaway record.
fn glob_matches_fqn(pat: &SymbolPattern, fqn: &str) -> bool {
    pat.matches_fqn(fqn)
}
