//! The SCIP upgrade-only re-label pass (design §3.5, §3.6, §7).
//!
//! Runs **between** [`extract_and_link`](super::extract_and_link) and
//! [`store_graph`](super::store_graph), operating on the in-memory
//! [`ResolvedGraph`]. It consumes a [`ScipResolver`] (built once from a `.scip`
//! index) and walks the call-family edges in canonical [`EdgeId`] order, raising
//! each name-matched edge's confidence where SCIP gives a precise resolution.
//!
//! ## Invariant: upgrade-only (design §1, §5)
//!
//! The pass may only **raise** an edge's confidence, never lower it. Same-file
//! lexical calls already carry `Certain@ScopeGraph`; a SCIP occurrence that
//! resolves to a trait member (`probable` ceiling) or that SCIP saw weakly never
//! demotes them. An edge SCIP saw nothing at is left untouched (honest).
//!
//! ## Confidence rule (design §3.4, §3.5)
//!
//! For a matched call site, the target band is:
//! - **trait member** → `Probable` (the resolved symbol is a trait-method
//!   declaration, not the concrete impl — gotcha 1);
//! - else **`def_count == 1`** → `Certain` (unique definition);
//! - else (`def_count > 1`, the #18772 collision — gotcha 2) → `Probable`.
//!
//! Only when the target band exceeds the edge's current band is the edge
//! rewritten (`confidence`, `tier = Tier::Scip`, `rule = "scip-occurrence"`).
//!
//! ## GM-14 dependency edges (design §3.6, decision R5)
//!
//! When a matched ref's `(package, version)` differs from the local package, a
//! dependency call edge is emitted to a synthetic external node, encoding the
//! dependency coordinate in the denormalized `rule` as `scip-dep:<pkg>@<ver>`.
//! Zero schema change.
//!
//! ## Determinism (design §7)
//!
//! A confidence/tier/rule-only relabel is EdgeId-stable (the sort key is
//! untouched). Any `dst` redirect, candidate-group collapse, or new dependency
//! edge / external node sets [`ScipRelabel::dirty`]; the caller re-runs
//! [`cgx_resolve::canonicalize`] (and, for new nodes, this module's node
//! re-canonicalization) before storing.

use std::collections::BTreeMap;

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::edge::{EdgeKind, EdgeRecord, EdgeWithProvenance};
use cgx_core::id::{EdgeId, NodeId, SiteId};
use cgx_core::node::{NodeRecord, NodeWithProvenance, SymbolKind, Visibility};
use cgx_core::provenance::{Provenance, Span};
use cgx_resolve::ResolvedGraph;
use cgx_scip::{ScipResolver, SymClass, SymbolScheme};

/// Per-run SCIP counters, surfaced through [`IndexStats`](super::IndexStats) so
/// `cgx doctor` can report what the SCIP pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScipStats {
    /// Call edges upgraded to `Certain` by a unique SCIP definition.
    pub upgraded_certain: usize,
    /// Call edges upgraded to `Probable` (trait/virtual member or #18772/overload collision).
    pub upgraded_probable: usize,
    /// GM-14 cross-crate dependency edges emitted.
    pub dep_edges: usize,
    /// #18772 inherent-impl collisions observed (a matched symbol with >1 def).
    pub collisions: usize,
    /// Promotions that resolved to a unique definition but were capped at
    /// `Probable` by the FAIL-CLOSED partial-compdb policy — a `certain` that a
    /// complete compile_commands.json would have allowed. `cgx doctor` surfaces
    /// this as the honest coverage caveat (ADR B2 Decision 3 top risk).
    pub capped_partial_compdb: usize,
}

/// How complete the `compile_commands.json` backing a scip-clang index is known
/// to be (ADR B2 Decision 3 / Eric's binding fail-closed decision).
///
/// A partial compdb omits translation units, so scip-clang's merged index can
/// see one definition where the program has several — a *false singleton* that
/// would wrongly promote to `certain`. The policy is FAIL-CLOSED: `certain` is
/// emitted only when completeness is affirmatively known; otherwise the
/// promotion caps at `probable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompdbCompleteness {
    /// No `compile_commands.json` concept (e.g. a rust-analyzer index). The
    /// completeness gate does not apply; promotions proceed to `certain`.
    #[default]
    NotApplicable,
    /// The compdb is verified complete for every indexed TU. `certain`
    /// promotions are allowed.
    Complete,
    /// The compdb is known partial, or its completeness is unverified.
    /// FAIL-CLOSED: SCIP promotions are capped at `probable` and no candidate
    /// group is collapsed on a possibly-false singleton.
    Partial,
}

impl CompdbCompleteness {
    /// Whether the fail-closed cap is in force (completeness not affirmatively
    /// established).
    fn is_fail_closed(self) -> bool {
        matches!(self, CompdbCompleteness::Partial)
    }
}

/// The local package name SCIP resolution treats as "in-tree". A reference whose
/// resolved package differs is a cross-crate dependency (GM-14).
///
/// `None` infers the local package from the most common package across the
/// index's own definition sites — but the caller currently passes the crate
/// under index explicitly when known.
#[derive(Debug, Clone, Default)]
pub struct ScipRelabelOpts {
    /// The package name(s) considered local (in-tree). A ref resolving to any
    /// other package is a dependency edge. Empty ⇒ infer from def-site packages.
    pub local_packages: Vec<String>,
    /// The completeness of the `compile_commands.json` behind a scip-clang
    /// index. Gates the `certain` promotion FAIL-CLOSED (ADR B2 Decision 3).
    /// Defaults to [`CompdbCompleteness::NotApplicable`] so the rust-analyzer
    /// path is unchanged.
    pub compdb: CompdbCompleteness,
}

/// Apply the SCIP upgrade-only re-label pass to `graph` in place, returning the
/// counters and whether the graph must be re-canonicalized.
pub fn relabel(
    graph: &mut ResolvedGraph,
    scip: &ScipResolver,
    opts: &ScipRelabelOpts,
) -> ScipStats {
    let mut pass = ScipRelabel::new(graph, scip, opts);
    pass.run();
    pass.finish()
}

struct ScipRelabel<'g, 's> {
    graph: &'g mut ResolvedGraph,
    scip: &'s ScipResolver,
    /// `qname → NodeId` for every existing node (for dst-redirect lookups).
    node_by_fqn: BTreeMap<String, NodeId>,
    /// `(file, 1-based line_start) → NodeId` for span-precise dst redirect. Used
    /// by the scip-clang path to redirect an overloaded call to the exact
    /// resolved overload's node, which the signature-free qname cannot pick.
    node_by_loc: BTreeMap<(String, u32), NodeId>,
    /// FAIL-CLOSED: when the backing compdb is not known-complete, cap SCIP
    /// promotions at `probable` and leave candidate groups intact.
    fail_closed: bool,
    /// The set of packages owning an in-tree definition site, plus any caller
    /// override. A resolved package outside this set is a dependency.
    local_packages: std::collections::BTreeSet<String>,
    /// Synthetic external nodes minted for dependency targets, keyed by qname so
    /// repeated refs share one node. Value is the index into `new_nodes`.
    external_nodes: BTreeMap<String, usize>,
    new_nodes: Vec<NodeRecord>,
    new_edges: Vec<EdgeWithProvenance>,
    stats: ScipStats,
    dirty: bool,
}

impl<'g, 's> ScipRelabel<'g, 's> {
    fn new(
        graph: &'g mut ResolvedGraph,
        scip: &'s ScipResolver,
        opts: &ScipRelabelOpts,
    ) -> ScipRelabel<'g, 's> {
        let node_by_fqn: BTreeMap<String, NodeId> = graph
            .nodes
            .iter()
            .map(|n| (n.node.fqn.clone(), n.node.id))
            .collect();

        // (file, 1-based line) → node, for span-precise overload redirect. On a
        // span collision (two nodes starting on one line) the last wins; the
        // redirect only consults this for a SCIP-confirmed unique def site.
        let node_by_loc: BTreeMap<(String, u32), NodeId> = graph
            .nodes
            .iter()
            .map(|n| ((n.node.file.clone(), n.node.line_start), n.node.id))
            .collect();

        // Local packages: caller override ∪ packages that own a def site whose
        // qname is also an in-tree node. Falls back to all def-site packages.
        let mut local_packages: std::collections::BTreeSet<String> =
            opts.local_packages.iter().cloned().collect();
        if local_packages.is_empty() {
            for doc in &scip.index().documents {
                for occ in &doc.occurrences {
                    if occ.is_definition() {
                        if let Some(m) = scip.map_symbol(&occ.symbol) {
                            if node_by_fqn.contains_key(&m.qname) {
                                local_packages.insert(m.package);
                            }
                        }
                    }
                }
            }
        }

        ScipRelabel {
            graph,
            scip,
            node_by_fqn,
            node_by_loc,
            fail_closed: opts.compdb.is_fail_closed(),
            local_packages,
            external_nodes: BTreeMap::new(),
            new_nodes: Vec::new(),
            new_edges: Vec::new(),
            stats: ScipStats::default(),
            dirty: false,
        }
    }

    fn run(&mut self) {
        // Walk edges in canonical (EdgeId) order. The Vec is already canonical
        // (link::finalize ran); iterating by index keeps equal-key edges in their
        // canonical relative order while we mutate confidence/tier/rule in place.
        for i in 0..self.graph.edges.len() {
            if !self.graph.edges[i].edge.kind.is_call() {
                continue;
            }
            self.relabel_edge(i);
        }
        // Append minted external nodes + dependency edges after the in-place pass.
        if !self.new_nodes.is_empty() {
            self.commit_new_nodes();
        }
        if !self.new_edges.is_empty() {
            let edges = std::mem::take(&mut self.new_edges);
            self.graph.edges.extend(edges);
            self.dirty = true;
        }
    }

    fn relabel_edge(&mut self, i: usize) {
        // Locate the call site via the edge's provenance span (file + 1-based
        // line/col). SCIP ranges are 0-based, so convert before matching.
        let span = self.graph.edges[i].provenance.span.clone();
        let (line0, col0) = match span.col {
            Some(c) => (span.line.saturating_sub(1), c.saturating_sub(1)),
            // A call site with no column cannot be span-matched against SCIP.
            None => return,
        };

        let occ_symbol = match self.find_occurrence(&span.file, line0, col0) {
            Some(sym) => sym,
            // SCIP saw nothing at this span — leave the edge untouched (honest).
            None => return,
        };

        let mapped = match self.scip.map_symbol(&occ_symbol) {
            Some(m) => m,
            None => return,
        };
        let sym_class = self.scip.classify(&occ_symbol);
        // Overload-precise multiplicity: keyed on (qname, overload_key), so two
        // resolved C++ overloads of one qname are NOT a false collision.
        let def_count = self.scip.def_count(&mapped);
        if def_count > 1 {
            self.stats.collisions += 1;
        }

        let edge_kind = self.graph.edges[i].edge.kind;

        let mut target_conf = match sym_class {
            // Trait member (Rust) / virtual-overridable member (C++): the SCIP
            // occurrence binds the declaration, not the concrete impl/override.
            SymClass::TraitMember | SymClass::VirtualMember => Confidence::Probable,
            SymClass::FreeOrInherent if def_count == 1 => Confidence::Certain,
            SymClass::FreeOrInherent => Confidence::Probable, // #18772 / overload collision
        };

        // A virtual-dispatch call site is resolved by scip-clang to the static
        // declared target, which is not a devirtualization → never certain.
        if edge_kind == EdgeKind::CallsVirtual {
            target_conf = target_conf.min(Confidence::Probable);
        }

        // --- FAIL-CLOSED partial-compdb policy (ADR B2 Decision 3). ---
        // A unique-looking resolution from an incomplete compdb may be a false
        // singleton; never let it reach `certain`.
        if self.fail_closed && target_conf == Confidence::Certain {
            target_conf = Confidence::Probable;
            self.stats.capped_partial_compdb += 1;
        }

        // --- GM-14: cross-crate dependency edge (design §3.6). ---
        if !self.local_packages.contains(&mapped.package) {
            self.emit_dep_edge(i, &mapped.qname, &mapped.package, &mapped.version);
            // A dependency target is off-tree: do not redirect dst to an in-tree
            // node, but the confidence merge below still applies to the edge.
        }

        // --- UPGRADE-ONLY merge (design §3.5). ---
        let edge = &mut self.graph.edges[i].edge;
        if target_conf > edge.confidence {
            edge.confidence = target_conf;
            edge.tier = Tier::Scip;
            edge.rule = "scip-occurrence".to_string();
            self.graph.edges[i].provenance.rule = "scip-occurrence".to_string();
            self.graph.edges[i].provenance.tier = Tier::Scip;
            match target_conf {
                Confidence::Certain => self.stats.upgraded_certain += 1,
                Confidence::Probable => self.stats.upgraded_probable += 1,
                Confidence::Possible => {}
            }
        }

        // --- Optional dst-redirect / candidate-group collapse (design §3.5). ---
        // Only when SCIP resolved a single in-tree definition the policy trusts:
        // redirect to the unique target and collapse any candidate group. Under
        // the fail-closed cap the singleton may be false, so the group is left
        // intact (the ambiguity stays visible). Marks the graph dirty.
        if def_count == 1 && !self.fail_closed && self.local_packages.contains(&mapped.package) {
            let target = match self.scip.scheme() {
                // C++: rewrite the target only on a genuinely `certain` promotion
                // (a virtual/capped edge keeps its candidate set — honest), and
                // pick the exact resolved overload by its def-site span, since the
                // signature-free qname maps to an arbitrary overload node.
                SymbolScheme::ScipClang if target_conf == Confidence::Certain => {
                    self.resolve_overload_node(&mapped)
                }
                SymbolScheme::ScipClang => None,
                SymbolScheme::RustAnalyzer => self.node_by_fqn.get(&mapped.qname).copied(),
            };
            if let Some(target) = target {
                let edge = &mut self.graph.edges[i].edge;
                if edge.dst != target {
                    edge.dst = target;
                    self.dirty = true;
                }
                if edge.candidate_group.is_some() {
                    edge.candidate_group = None;
                    self.dirty = true;
                }
            }
        }
    }

    /// Resolve the node for a scip-clang overloaded call: the node at the unique
    /// definition's span (overload-precise), falling back to the signature-free
    /// qname map when the def span matches no node.
    fn resolve_overload_node(&self, mapped: &cgx_scip::MappedSymbol) -> Option<NodeId> {
        if let Some(loc) = self.scip.unique_def_loc(mapped) {
            let line1 = (loc.range.start_line as u32).saturating_add(1);
            if let Some(&id) = self.node_by_loc.get(&(loc.relative_path.clone(), line1)) {
                return Some(id);
            }
        }
        self.node_by_fqn.get(&mapped.qname).copied()
    }

    /// Find a SCIP reference occurrence in `file` whose start coordinate matches
    /// the call site (0-based `line`, `col`). Returns the resolved symbol string.
    fn find_occurrence(&self, file: &str, line0: u32, col0: u32) -> Option<String> {
        let refs = self.scip.refs_in_doc(file);
        // refs are sorted by (range, symbol); a linear scan over a small per-doc
        // slice is deterministic and adequate. Match on the start coordinate.
        refs.iter()
            .find(|(range, _)| {
                range.start_line == line0 as i32 && range.start_char == col0 as i32
            })
            .map(|(_, sym)| sym.clone())
    }

    /// Emit a GM-14 dependency call edge for a cross-crate reference, minting a
    /// synthetic external node for the target if one does not exist yet. The
    /// `(pkg, version)` coordinate rides the denormalized `rule` (decision R5).
    fn emit_dep_edge(&mut self, src_edge_idx: usize, qname: &str, pkg: &str, version: &str) {
        let src = self.graph.edges[src_edge_idx].edge.src;
        let kind = self.graph.edges[src_edge_idx].edge.kind;
        let span = self.graph.edges[src_edge_idx].provenance.span.clone();
        let stmt_index = self.graph.edges[src_edge_idx].edge.stmt_index;

        let dst = self.external_node(qname);
        let rule = format!("scip-dep:{pkg}@{version}");
        let site_id = Some(SiteId::derive(
            qname,
            &span.file,
            span.line,
            span.col.unwrap_or(0),
        ));
        let record = EdgeRecord {
            id: EdgeId(0), // reassigned by canonicalize
            src,
            dst,
            kind,
            condition: self.graph.edges[src_edge_idx].edge.condition,
            confidence: Confidence::Probable,
            tier: Tier::Scip,
            rule: rule.clone(),
            site_id,
            stmt_index,
            cut_markers: self.graph.edges[src_edge_idx].edge.cut_markers.clone(),
            implicit: self.graph.edges[src_edge_idx].edge.implicit,
            candidate_group: None,
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
            transform: None,
        };
        let prov = Provenance::new(span, rule, Tier::Scip, String::new());
        self.new_edges.push(EdgeWithProvenance {
            edge: record,
            provenance: prov,
        });
        self.stats.dep_edges += 1;
    }

    /// Resolve (or mint) the synthetic external node id for a dependency qname.
    /// External nodes are created with a sentinel file path that sorts after all
    /// in-tree files so node re-canonicalization is deterministic.
    fn external_node(&mut self, qname: &str) -> NodeId {
        if let Some(&existing) = self.node_by_fqn.get(qname) {
            return existing;
        }
        if let Some(&idx) = self.external_nodes.get(qname) {
            // Already minted this run; its id is assigned at commit time, so
            // return a placeholder offset that commit_new_nodes resolves.
            return NodeId(self.graph.nodes.len() as u32 + idx as u32);
        }
        let idx = self.new_nodes.len();
        self.new_nodes.push(NodeRecord {
            id: NodeId(0), // assigned at commit
            kind: SymbolKind::Function,
            fqn: qname.to_string(),
            file: format!("<scip-external>/{qname}"),
            line_start: 0,
            line_end: 0,
            lang: "rust".to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
            unresolved_calls: 0,
        });
        self.external_nodes.insert(qname.to_string(), idx);
        NodeId(self.graph.nodes.len() as u32 + idx as u32)
    }

    /// Append minted external nodes, re-sort all nodes into canonical order,
    /// reassign dense [`NodeId`]s, and remap every edge endpoint (existing + new)
    /// to the reassigned ids. Marks the graph dirty.
    fn commit_new_nodes(&mut self) {
        // Pre-commit, edges reference external nodes by the provisional id
        // `old_len + idx`. Build the old→new id map after the sort.
        let old_len = self.graph.nodes.len() as u32;

        let provisional: Vec<NodeRecord> = std::mem::take(&mut self.new_nodes)
            .into_iter()
            .enumerate()
            .map(|(idx, mut rec)| {
                rec.id = NodeId(old_len + idx as u32);
                rec
            })
            .collect();

        for rec in provisional {
            self.graph.nodes.push(NodeWithProvenance {
                provenance: Provenance::new(
                    Span::new(rec.file.clone(), 0, None),
                    "scip-external",
                    Tier::Scip,
                    String::new(),
                ),
                node: rec,
            });
        }

        // Canonical node order: (file, line_start, fqn). Sentinel files sort last.
        self.graph.nodes.sort_by(|a, b| {
            (
                a.node.file.as_str(),
                a.node.line_start,
                a.node.fqn.as_str(),
            )
                .cmp(&(
                    b.node.file.as_str(),
                    b.node.line_start,
                    b.node.fqn.as_str(),
                ))
        });

        // old id → new id remap.
        let mut remap: BTreeMap<u32, NodeId> = BTreeMap::new();
        for (i, n) in self.graph.nodes.iter_mut().enumerate() {
            remap.insert(n.node.id.0, NodeId(i as u32));
            n.node.id = NodeId(i as u32);
        }

        let apply = |id: NodeId| remap.get(&id.0).copied().unwrap_or(id);
        for e in &mut self.graph.edges {
            e.edge.src = apply(e.edge.src);
            e.edge.dst = apply(e.edge.dst);
        }
        for e in &mut self.new_edges {
            e.edge.src = apply(e.edge.src);
            e.edge.dst = apply(e.edge.dst);
        }
        // Candidate dsts also reference node ids.
        for c in &mut self.graph.candidates {
            c.dst = apply(c.dst);
        }
        self.dirty = true;
    }

    fn finish(self) -> ScipStats {
        if self.dirty {
            cgx_resolve::canonicalize(self.graph);
        }
        self.stats
    }
}
