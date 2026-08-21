//! End-to-end RTA-narrowing coverage (review NIT): RTA's `possible → probable`
//! happy path exercised through the *full* `index_path` pipeline (real Rust
//! extraction → link → CHA → RTA), not just resolver-level unit tests.
//!
//! The fixture is a `dyn Shape` call with three impls where one type
//! (`Triangle`) is never instantiated and NO cut marker covers the construction
//! sites, so RTA can soundly prune the never-constructed impl and upgrade the
//! surviving group to `probable`. This is the non-guarded path: on the committed
//! `rust-sample` fixture RTA prunes nothing (every site is cut-guarded or has no
//! construction evidence), so the happy path had no integration coverage before.

mod common;

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::edge::EdgeKind;
use cgx_index::{index_path, IndexOpts};
use common::*;

/// A single-file crate: trait `Shape` (area) with `Circle`/`Square`/`Triangle`
/// impls, a `dyn Shape` virtual call, and struct-literal constructions of
/// `Circle` and `Square` only — `Triangle` is never instantiated, and nothing is
/// behind a cut. So RTA must prune `Triangle::area` and keep `{Circle, Square}`.
const SHAPES_RS: &str = r#"
pub trait Shape {
    fn area(&self) -> f64;
}

pub struct Circle { pub r: f64 }
pub struct Square { pub s: f64 }
pub struct Triangle { pub b: f64, pub h: f64 }

impl Shape for Circle {
    fn area(&self) -> f64 { 3.14 * self.r * self.r }
}
impl Shape for Square {
    fn area(&self) -> f64 { self.s * self.s }
}
impl Shape for Triangle {
    fn area(&self) -> f64 { 0.5 * self.b * self.h }
}

pub fn total_area(shape: &dyn Shape) -> f64 {
    shape.area()
}

pub fn build() -> f64 {
    let c = Circle { r: 1.0 };
    let s = Square { s: 2.0 };
    total_area(&c) + total_area(&s)
}
"#;

#[test]
fn rta_narrows_a_dyn_call_to_probable_end_to_end() {
    let (_t, repo) = init_empty_repo();
    write_file(
        &repo,
        "Cargo.toml",
        "[package]\nname = \"shapes\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(&repo, "src/lib.rs", SHAPES_RS);
    commit_all(&repo, "shapes fixture");

    let registry = cgx_index::default_registry();
    let mut store = mem_store();
    let outcome = index_path(&repo, &registry, &mut store, &IndexOpts::default()).unwrap();

    // RTA pruned exactly the never-instantiated Triangle at the one dyn site.
    let rta = outcome.stats.rta;
    assert_eq!(rta.sites_pruned, 1, "the single dyn Shape call site is RTA-pruned");
    assert_eq!(
        rta.candidates_dropped, 1,
        "Triangle::area (never instantiated) is the one dropped candidate"
    );

    let g = read_graph(&store, &outcome.graph_key);
    let idx = GraphIndex::new(&g);

    // The surviving virtual-dispatch candidates are Circle::area + Square::area,
    // both `probable@cha_rta` with rule `rta-pruned`. Triangle::area is gone.
    let survivors: Vec<_> = g
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::CallsVirtual && e.rule == "rta-pruned")
        .collect();
    assert_eq!(survivors.len(), 2, "exactly Circle + Square survive the prune");
    for e in &survivors {
        assert_eq!(
            e.confidence,
            Confidence::Probable,
            "RTA upgrades the narrowed group possible → probable"
        );
        assert_eq!(e.tier, Tier::ChaRta, "tier is cha_rta");
    }

    let dsts: std::collections::BTreeSet<&str> =
        survivors.iter().map(|e| idx.fqn_of(e.dst)).collect();
    assert!(
        dsts.contains("shapes::Circle::area") && dsts.contains("shapes::Square::area"),
        "survivors are Circle::area + Square::area, got {dsts:?}"
    );
    assert!(
        !dsts.contains("shapes::Triangle::area"),
        "the never-instantiated Triangle::area is pruned"
    );
}
