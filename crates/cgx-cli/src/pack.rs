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
//! This file landed in three surgical commits: the namespace + manifest
//! foundation, then `interface-map` (#8, this commit), then
//! `dependency-footprint` (#4).

use serde_json::{json, Value};

use cgx_core::{Confidence, EdgeKind, NodeId, SymbolPattern};
use cgx_doctor::DoctorReport;
use cgx_query::{over_only, Direction, EdgeFilter, GraphView};

/// Card tokens implemented so far. The single source of truth the CLI
/// dispatch validates requests against — keeps the accepted-token list and
/// the dispatch `match` from drifting apart.
pub const IMPLEMENTED_CARDS: &[&str] = &["interface-map"];

/// Implemented cards addressable with **no** required symbol/selector — the
/// set `cgx pack` (bare, no card token) assembles into the "full pack".
/// `dependency-footprint` is structural (by-FQN, spec §2): it always needs an
/// anchor, so it can never join an addressless bundle.
pub const ADDRESSLESS_IMPLEMENTED_CARDS: &[&str] = &["interface-map"];

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
        status: "implemented",
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

// --- #8 interface-map ---------------------------------------------------------

/// `cgx pack interface-map` selector (spec §2): `--interface`/`--type` are
/// mutually exclusive; the CLI layer rejects passing both before this is
/// constructed.
pub enum InterfaceMapSelector<'a> {
    All,
    Interface(&'a str),
    Type(&'a str),
}

/// Row cap for the repo-wide (`All`) and scoped windows alike, so a dense
/// implements-lattice never silently returns an unbounded document. Disclosed
/// via `row_limit_applied` whenever it actually truncates the window (spec
/// §2/§3) — never presented as the complete set.
const INTERFACE_MAP_ROW_CAP: usize = 500;

/// `cgx pack interface-map [--interface <FQN> | --type <FQN>]`: the populated
/// structural type-implements-interface edge (`EdgeKind::Implements`), never
/// inverted — `type` is always the concrete implementer, `interface` always
/// the interface (spec §3, direction is load-bearing).
pub fn interface_map(view: &GraphView, selector: InterfaceMapSelector<'_>) -> Value {
    let (query_scope, selector_text, anchor) = match selector {
        InterfaceMapSelector::All => ("all", None, None),
        InterfaceMapSelector::Interface(fqn) => match view.resolve_one(&SymbolPattern::fqn(fqn)) {
            Ok(id) => ("interface", Some(fqn.to_string()), Some(id)),
            Err(_) => return unresolved_interface_map("interface", fqn),
        },
        InterfaceMapSelector::Type(fqn) => match view.resolve_one(&SymbolPattern::fqn(fqn)) {
            Ok(id) => ("type", Some(fqn.to_string()), Some(id)),
            Err(_) => return unresolved_interface_map("type", fqn),
        },
    };

    let filter = EdgeFilter::default().with_kinds(vec![EdgeKind::Implements]);
    let pairs: Vec<(NodeId, NodeId, Confidence)> = match (query_scope, anchor) {
        ("all", _) => view
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Implements)
            .map(|e| (e.src, e.dst, e.confidence))
            .collect(),
        ("interface", Some(id)) => view
            .neighbors(id, Direction::Backward, &filter)
            .map(|er| (er.peer, id, er.edge.confidence))
            .collect(),
        ("type", Some(id)) => view
            .neighbors(id, Direction::Forward, &filter)
            .map(|er| (id, er.peer, er.edge.confidence))
            .collect(),
        _ => unreachable!("anchor is Some for every scope other than \"all\""),
    };

    let total_edges_in_window = pairs.len();
    let over = pairs.iter().any(|(_, _, c)| *c == Confidence::Possible);
    let capped = &pairs[..pairs.len().min(INTERFACE_MAP_ROW_CAP)];
    let row_limit_applied =
        (total_edges_in_window > INTERFACE_MAP_ROW_CAP).then_some(INTERFACE_MAP_ROW_CAP);

    let rows: Vec<Value> = capped
        .iter()
        .map(|(ty, iface, _)| {
            let t = view.node(*ty);
            let i = view.node(*iface);
            json!({
                "type": t.fqn,
                "interface": i.fqn,
                "type_file": t.file,
                "interface_file": i.file,
            })
        })
        .collect();

    json!({
        "card": "interface-map",
        "query_scope": query_scope,
        "selector": selector_text,
        "resolved": true,
        "approximation": serde_json::to_value(over_only(over))
            .expect("approximation contract serializes"),
        "total_edges_in_window": total_edges_in_window,
        "row_limit_applied": row_limit_applied,
        "edges": rows,
        "empty_semantics": interface_map_empty_semantics(total_edges_in_window),
        "honesty": interface_map_honesty(row_limit_applied),
    })
}

fn interface_map_empty_semantics(count: usize) -> String {
    if count == 0 {
        "this index returned zero structural type-implements-interface edges in this window; \
         empty is NOT proof that no type satisfies any interface — the population of this \
         edge type is index-build-dependent (the same rebuild that can empty one structural \
         layer can empty another). Reproduce with a fresh `cgx index` before treating this as \
         a real absence."
            .to_string()
    } else {
        format!(
            "{count} structural type-implements-interface edge(s) found in this window; a \
             missing row for a specific type/interface pair is not proof that pair doesn't \
             satisfy — this edge type's population is index-build-dependent."
        )
    }
}

fn interface_map_honesty(row_limit_applied: Option<usize>) -> String {
    let mut s = "`type` is always the concrete implementer and `interface` is always the \
                  interface it satisfies — never the reverse, regardless of which selector was \
                  used. This is a structural (syntactic) satisfaction edge, not a proven \
                  runtime-dispatch verdict."
        .to_string();
    if let Some(n) = row_limit_applied {
        s.push_str(&format!(
            " Row cap applied: only the first {n} edge(s) in this window are included — this \
             is NOT the complete implementer/satisfier set; see `row_limit_applied`."
        ));
    }
    s
}

fn unresolved_interface_map(which: &str, fqn: &str) -> Value {
    json!({
        "card": "interface-map",
        "query_scope": which,
        "selector": fqn,
        "resolved": false,
        "error": format!("no symbol matched exact FQN `{fqn}` in the current index"),
        "approximation": Value::Null,
        "total_edges_in_window": 0,
        "row_limit_applied": null,
        "edges": [],
        "empty_semantics": "the selector did not resolve at all — this is NOT the same as a \
                             resolved symbol with zero implementer/interface edges; re-check \
                             the FQN with `cgx search`",
        "honesty": "`type` is always the concrete implementer and `interface` is always the \
                     interface it satisfies — never the reverse.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgx_core::{Candidate, EdgeCondition, EdgeId, EdgeRecord, NodeRecord, SymbolKind, Tier, Visibility};
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

    fn node(id: u32, fqn: &str, kind: SymbolKind) -> NodeRecord {
        NodeRecord {
            id: NodeId(id),
            kind,
            fqn: fqn.to_string(),
            file: "src/lib.rs".to_string(),
            line_start: 10 + id,
            line_end: 10 + id,
            lang: "rust".to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
            unresolved_calls: 0,
        }
    }

    fn implements_edge(id: u32, ty: u32, iface: u32) -> EdgeRecord {
        EdgeRecord {
            id: EdgeId(id),
            src: NodeId(ty),
            dst: NodeId(iface),
            kind: EdgeKind::Implements,
            condition: EdgeCondition::Always,
            confidence: Confidence::Certain,
            tier: Tier::ScopeGraph,
            rule: "impl-relation".to_string(),
            site_id: None,
            stmt_index: None,
            cut_markers: cgx_core::CutMarkers::new(),
            implicit: None,
            candidate_group: None,
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
            transform: None,
        }
    }

    // Circle/Square implement Shape; Triangle does not.
    fn interface_map_view() -> GraphView {
        let nodes = vec![
            node(0, "shapes::Circle", SymbolKind::Type),
            node(1, "shapes::Square", SymbolKind::Type),
            node(2, "shapes::Triangle", SymbolKind::Type),
            node(3, "shapes::Shape", SymbolKind::Type),
        ];
        let edges = vec![implements_edge(0, 0, 3), implements_edge(1, 1, 3)];
        GraphView::new(nodes, edges, Vec::<Candidate>::new())
    }

    #[test]
    fn interface_map_direction_is_never_inverted() {
        let view = interface_map_view();
        let doc = interface_map(&view, InterfaceMapSelector::All);
        assert_eq!(doc["resolved"], true);
        assert_eq!(doc["total_edges_in_window"], 2);
        for row in doc["edges"].as_array().unwrap() {
            let ty = row["type"].as_str().unwrap();
            let iface = row["interface"].as_str().unwrap();
            assert_eq!(iface, "shapes::Shape");
            assert!(ty == "shapes::Circle" || ty == "shapes::Square");
        }
    }

    #[test]
    fn interface_map_interface_selector_scopes_correctly() {
        let view = interface_map_view();
        let doc = interface_map(&view, InterfaceMapSelector::Interface("shapes::Shape"));
        assert_eq!(doc["query_scope"], "interface");
        assert_eq!(doc["total_edges_in_window"], 2);
        for row in doc["edges"].as_array().unwrap() {
            assert_eq!(row["interface"], "shapes::Shape");
        }
    }

    #[test]
    fn interface_map_type_selector_scopes_correctly() {
        let view = interface_map_view();
        let doc = interface_map(&view, InterfaceMapSelector::Type("shapes::Circle"));
        assert_eq!(doc["query_scope"], "type");
        let edges = doc["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["type"], "shapes::Circle");
        assert_eq!(edges[0]["interface"], "shapes::Shape");
    }

    #[test]
    fn interface_map_empty_result_never_says_nothing_implements() {
        let nodes = vec![node(0, "shapes::Triangle", SymbolKind::Type)];
        let view = GraphView::new(nodes, Vec::new(), Vec::<Candidate>::new());
        let doc = interface_map(&view, InterfaceMapSelector::All);
        let text = doc["empty_semantics"].as_str().unwrap();
        assert!(!text.to_lowercase().contains("nothing implements"));
        assert!(text.to_lowercase().contains("not proof"));
    }

    #[test]
    fn interface_map_unresolvable_selector_is_labeled_not_crashed() {
        let view = interface_map_view();
        let doc = interface_map(&view, InterfaceMapSelector::Interface("shapes::Ghost"));
        assert_eq!(doc["resolved"], false);
        assert!(doc["error"].as_str().unwrap().contains("Ghost"));
        assert_eq!(doc["total_edges_in_window"], 0);
        assert_eq!(doc["edges"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn interface_map_row_cap_is_not_applied_below_the_cap() {
        let view = interface_map_view();
        let doc = interface_map(&view, InterfaceMapSelector::All);
        assert!(doc["row_limit_applied"].is_null());
    }
}
