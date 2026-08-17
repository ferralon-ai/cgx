//! `cgx pack` card construction — the pure, testable logic behind the `pack`
//! CLI wiring in `main.rs`.
//!
//! Each card wraps an existing `cgx-query` primitive **in-process** (no
//! subprocess, no re-embedding an envelope across a process boundary) and
//! emits self-describing JSON per the contract specs in
//! `the pack-card contract specs`. Cards
//! resolve **exact FQNs only** — the canonical `::`-separated form a user
//! copies from `cgx search`/`cgx symbols` output — via
//! `cgx_core::SymbolPattern::fqn`, never the glob/short-name fallback
//! `cgx_cli::pattern::parse_symbol` uses for the rest of the CLI.
//! Selector/glob/regex matching is a separate, still-open design (out of
//! scope this batch).
//!
//! ## The honesty spine
//!
//! - Every card carries the wrapped primitive's `approximation` envelope
//!   verbatim (see `cgx_query::contract`), or an explicit per-row
//!   `confidence`/`condition` label where the primitive has no envelope of
//!   its own.
//! - **Empty ≠ proven absent.** A zero-row card states what was checked, at
//!   what depth/confidence floor, never "has no dependencies" or "nothing
//!   implements it."
//! - **Unresolved FQN ≠ resolved-and-empty.** Every card carries a `resolved`
//!   flag: `false` marks a selector/anchor that never matched a symbol at
//!   all (a labeled error, not a crash and not indistinguishable from
//!   "resolved it, found zero").
//!
//! ## This module's stages
//!
//! This file lands in three surgical commits: the namespace + manifest
//! foundation (this commit — no cards yet), then `interface-map` (#8), then
//! `dependency-footprint` (#4).

use serde_json::{json, Value};

use cgx_doctor::DoctorReport;

/// Card tokens implemented so far. The single source of truth the CLI
/// dispatch validates requests against — keeps the accepted-token list and
/// the dispatch `match` from drifting apart. Empty until the first card
/// lands.
pub const IMPLEMENTED_CARDS: &[&str] = &[];

/// Implemented cards addressable with **no** required symbol/selector — the
/// set `cgx pack` (bare, no card token) assembles into the "full pack".
/// `dependency-footprint` is structural (by-FQN, spec §2): it always needs an
/// anchor, so it can never join an addressless bundle.
pub const ADDRESSLESS_IMPLEMENTED_CARDS: &[&str] = &[];

/// One row of the card-token registry embedded in `manifest.json`. Every card
/// in the ten-card design is listed (not just the ones implemented so far),
/// so an agent reading the manifest sees the whole roadmap and each card's
/// current status — never invents an unimplemented card by omission.
pub struct CardMeta {
    pub token: &'static str,
    pub name: &'static str,
    pub shelf: &'static str,
    pub address_scheme: &'static str,
    pub tier: &'static str,
    pub token_cost_estimate: &'static str,
    pub reach_for: &'static str,
    pub status: &'static str,
}

/// The full ten-card registry (README.md's card-token table), in table order.
pub const CARD_REGISTRY: &[CardMeta] = &[
    CardMeta {
        token: "hotspot-map",
        name: "Repo Atlas & Hotspot Map",
        shelf: "structural",
        address_scheme: "none (repo-wide)",
        tier: "light",
        token_cost_estimate: "small, fixed — one row per ranked hub, capped",
        reach_for: "orient in an unfamiliar repo: where the graph hubs and hotspots are",
        status: "not yet implemented natively (proven Go POC reference impl exists)",
    },
    CardMeta {
        token: "symbol-card",
        name: "Symbol Card",
        shelf: "structural",
        address_scheme: "by-FQN",
        tier: "light",
        token_cost_estimate: "small, fixed — one symbol's full provenance",
        reach_for: "everything cgx knows about one symbol, before editing it",
        status: "not yet implemented natively (proven Go POC reference impl exists)",
    },
    CardMeta {
        token: "blast-radius",
        name: "Blast-Radius Card",
        shelf: "structural",
        address_scheme: "by-FQN",
        tier: "scales with depth",
        token_cost_estimate: "scales with --depth and caller fan-out",
        reach_for: "who breaks if X changes",
        status: "not yet implemented natively (proven Go POC reference impl exists)",
    },
    CardMeta {
        token: "dependency-footprint",
        name: "Dependency Footprint",
        shelf: "structural",
        address_scheme: "by-FQN",
        tier: "light at low --depth, scales toward heavy",
        token_cost_estimate: "~150 tokens fixed + ~50 tokens/callee row; grows with --depth",
        reach_for: "what X transitively touches, before editing X",
        status: "not yet implemented natively (planned this cycle)",
    },
    CardMeta {
        token: "reachability",
        name: "Reachability & Paths",
        shelf: "structural",
        address_scheme: "by-FQN pair",
        tier: "heavy",
        token_cost_estimate: "large — path enumeration, work-budgeted",
        reach_for: "whether/how A reaches B",
        status: "blocked on the node-selector (EntrypointSelector) design",
    },
    CardMeta {
        token: "dead-code",
        name: "Dead-Code Inventory",
        shelf: "semantic",
        address_scheme: "none (repo-wide)",
        tier: "light",
        token_cost_estimate: "small — one row per dead symbol",
        reach_for: "what's unreachable from any entrypoint",
        status: "not yet implemented natively (proven Go POC reference impl exists)",
    },
    CardMeta {
        token: "attack-surface",
        name: "Entrypoint & Attack-Surface Catalog",
        shelf: "semantic",
        address_scheme: "none (repo-wide)",
        tier: "light (index-only slice)",
        token_cost_estimate: "small — one row per entrypoint hub",
        reach_for: "where does control enter this repo",
        status: "held pending the EntrypointSelector redesign it would need to reuse",
    },
    CardMeta {
        token: "interface-map",
        name: "Interface / Implementer Map",
        shelf: "semantic",
        address_scheme: "by-question, with by-FQN --interface/--type scoping",
        tier: "light",
        token_cost_estimate: "~100 tokens fixed + ~40 tokens/edge row, capped",
        reach_for: "what implements X, or what interfaces X satisfies",
        status: "not yet implemented natively (planned this cycle)",
    },
    CardMeta {
        token: "data-flow",
        name: "Data-Flow Provenance",
        shelf: "semantic",
        address_scheme: "by-FQN (value node)",
        tier: "unspecified",
        token_cost_estimate: "scales with slice size",
        reach_for: "where a value came from, or where it flows",
        status: "not yet implemented natively (proven Go POC reference impl exists)",
    },
    CardMeta {
        token: "call-graph-diff",
        name: "PR Call-Graph Diff",
        shelf: "semantic",
        address_scheme: "by-ref-pair",
        tier: "unspecified",
        token_cost_estimate: "scales with diff size",
        reach_for: "what a PR changed in the call graph",
        status: "not yet implemented natively (proven Go POC reference impl exists)",
    },
];

/// Build the `manifest` object: the `cgx doctor`-equivalent graph-honesty
/// header (embedded verbatim as [`DoctorReport`]'s own fields — node/edge
/// counts, confidence split, trust, unresolved rate) plus the full card
/// registry, so an agent sees the precision floor before consuming any card.
pub fn manifest(doctor: &DoctorReport) -> Value {
    let card_registry: Vec<Value> = CARD_REGISTRY
        .iter()
        .map(|c| {
            json!({
                "token": c.token,
                "name": c.name,
                "shelf": c.shelf,
                "address_scheme": c.address_scheme,
                "tier": c.tier,
                "token_cost_estimate": c.token_cost_estimate,
                "reach_for": c.reach_for,
                "status": c.status,
            })
        })
        .collect();
    json!({
        "generated_by": "cgx pack",
        "cgx_version": env!("CARGO_PKG_VERSION"),
        "graph_honesty": serde_json::to_value(doctor).expect("doctor report serializes"),
        "card_registry": card_registry,
    })
}

/// Assemble the multi-card (`--onedoc` / bare full-pack) document: the
/// manifest header plus every requested card's own JSON, keyed by token.
/// `manifest.json` is not a separate file here — the manifest is embedded as
/// a top-level `manifest` key, the same "still carries the manifest header"
/// shape the `--onedoc` contract describes.
pub fn pack_document(doctor: &DoctorReport, card_docs: &[(&str, Value)]) -> Value {
    let mut cards = serde_json::Map::new();
    for (token, doc) in card_docs {
        cards.insert((*token).to_string(), doc.clone());
    }
    json!({
        "manifest": manifest(doctor),
        "cards": Value::Object(cards),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgx_doctor::{ConfidenceBreakdown, CutMarkerCount, TrustLevel};

    fn empty_doctor() -> DoctorReport {
        DoctorReport {
            node_count: 3,
            edge_count: 2,
            call_edge_count: 2,
            confidence: ConfidenceBreakdown {
                certain: 2,
                probable: 0,
                possible: 0,
            },
            total_refs: 2,
            unresolved_count: 0,
            unresolved_rate: Some(0.0),
            unsupported_files: 0,
            total_files: None,
            unsupported_share: None,
            cut_markers: CutMarkerCount::default(),
            anomalies: Vec::new(),
            trust: TrustLevel::High,
        }
    }

    #[test]
    fn manifest_carries_the_full_ten_card_registry() {
        let doc = manifest(&empty_doctor());
        assert_eq!(doc["card_registry"].as_array().unwrap().len(), 10);
    }

    #[test]
    fn manifest_embeds_the_doctor_equivalent_graph_honesty_header() {
        let doc = manifest(&empty_doctor());
        assert_eq!(doc["graph_honesty"]["node_count"], 3);
        assert_eq!(doc["graph_honesty"]["trust"], "high");
    }

    #[test]
    fn pack_document_wraps_manifest_and_cards() {
        let doc = pack_document(&empty_doctor(), &[]);
        assert!(doc["manifest"].is_object());
        assert!(doc["cards"].is_object());
        assert_eq!(doc["cards"].as_object().unwrap().len(), 0);
    }
}
