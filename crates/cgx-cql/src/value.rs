//! The CQL runtime value type and its deterministic total order.
//!
//! WHERE comparisons, `IN`, `ORDER BY`, aggregates (`MIN`/`MAX`/`collect`) and
//! `DISTINCT` all need a single, total, deterministic order over heterogeneous
//! values — including across types — so that result rows never depend on hash
//! iteration order (design §3.5, Q-17/IF-8). [`Value`] supplies it via `Ord`.
//!
//! Graph values ([`Value::Node`], [`Value::Edge`], [`Value::Path`]) hold the
//! dense `cgx_core` ids now; the eval phase will look the records up against the
//! `GraphView`. The shapes here are designed so the eval phase can read record
//! fields without changing these variants: a node is an id, an edge is an id,
//! and a path is the node/edge id sequence the walker produced.

use std::cmp::Ordering;

use cgx_core::{EdgeId, NodeId};

/// A value produced during evaluation and surfaced in a result row.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    /// A bound node, identified by its dense graph id.
    Node(NodeId),
    /// A bound relationship, identified by its dense graph id.
    Edge(EdgeId),
    /// A bound path: the node ids in traversal order plus the edge id of each
    /// hop (so `relationships(p)` and `nodes(p)` need no re-walk).
    Path(PathValue),
}

/// A concrete path binding: `nodes.len() == edges.len() + 1` for a non-empty
/// path; a single-node path has an empty `edges`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathValue {
    pub nodes: Vec<NodeId>,
    pub edges: Vec<EdgeId>,
}

impl Value {
    /// Rank used to give a total order across distinct variants. Stable and
    /// independent of the underlying enum layout.
    fn type_rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) => 2,
            Value::Float(_) => 3,
            Value::Str(_) => 4,
            Value::List(_) => 5,
            Value::Node(_) => 6,
            Value::Edge(_) => 7,
            Value::Path(_) => 8,
        }
    }

    /// `true` for everything except `Null` and `Bool(false)` — the truthiness
    /// used by WHERE / quantifier folds (three-valued logic collapses `Null`
    /// to false for filtering, design §3.3).
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Null | Value::Bool(false))
    }
}

/// Cross-type comparison rule for `=`/`<`/`>`: `Int` and `Float` compare
/// numerically with each other; otherwise distinct types are ordered by rank.
fn compare(a: &Value, b: &Value) -> Ordering {
    use Value::*;
    match (a, b) {
        (Int(x), Int(y)) => x.cmp(y),
        (Float(x), Float(y)) => total_f64(*x, *y),
        (Int(x), Float(y)) => total_f64(*x as f64, *y),
        (Float(x), Int(y)) => total_f64(*x, *y as f64),
        (Bool(x), Bool(y)) => x.cmp(y),
        (Str(x), Str(y)) => x.cmp(y),
        (List(x), List(y)) => cmp_list(x, y),
        (Node(x), Node(y)) => x.0.cmp(&y.0),
        (Edge(x), Edge(y)) => x.0.cmp(&y.0),
        (Path(x), Path(y)) => cmp_path(x, y),
        (Null, Null) => Ordering::Equal,
        _ => a.type_rank().cmp(&b.type_rank()),
    }
}

fn cmp_list(x: &[Value], y: &[Value]) -> Ordering {
    for (xa, ya) in x.iter().zip(y.iter()) {
        let o = compare(xa, ya);
        if o != Ordering::Equal {
            return o;
        }
    }
    x.len().cmp(&y.len())
}

fn cmp_path(x: &PathValue, y: &PathValue) -> Ordering {
    let nodes = x.nodes.iter().map(|n| n.0).cmp(y.nodes.iter().map(|n| n.0));
    if nodes != Ordering::Equal {
        return nodes;
    }
    x.edges.iter().map(|e| e.0).cmp(y.edges.iter().map(|e| e.0))
}

/// A total order over `f64` that places `NaN` last and treats `-0.0 == 0.0`,
/// so floats can participate in the value `Ord` without panicking.
fn total_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or_else(|| {
        match (a.is_nan(), b.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => unreachable!("partial_cmp is None only for NaN"),
        }
    })
}

impl Eq for Value {}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        compare(self, other)
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_float_compare_numerically() {
        assert_eq!(Value::Int(2).cmp(&Value::Float(2.5)), Ordering::Less);
        assert_eq!(Value::Float(2.0).cmp(&Value::Int(2)), Ordering::Equal);
    }

    #[test]
    fn cross_type_uses_rank() {
        assert_eq!(Value::Null.cmp(&Value::Int(0)), Ordering::Less);
        assert_eq!(Value::Str("a".into()).cmp(&Value::Bool(true)), Ordering::Greater);
    }

    #[test]
    fn nan_sorts_last_without_panicking() {
        let mut v = vec![Value::Float(f64::NAN), Value::Float(1.0), Value::Float(0.0)];
        v.sort();
        assert_eq!(v[0], Value::Float(0.0));
        assert_eq!(v[1], Value::Float(1.0));
        assert!(matches!(v[2], Value::Float(f) if f.is_nan()));
    }

    #[test]
    fn list_is_lexicographic() {
        let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::List(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(a.cmp(&b), Ordering::Less);
    }

    #[test]
    fn nodes_order_by_id() {
        assert_eq!(Value::Node(NodeId(1)).cmp(&Value::Node(NodeId(2))), Ordering::Less);
    }

    #[test]
    fn truthiness() {
        assert!(!Value::Null.is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Bool(true).is_truthy());
        assert!(Value::Int(0).is_truthy());
    }

    #[test]
    fn distinct_dedup_via_sort() {
        let mut v = vec![Value::Int(1), Value::Int(1), Value::Int(2)];
        v.sort();
        v.dedup();
        assert_eq!(v, vec![Value::Int(1), Value::Int(2)]);
    }
}
