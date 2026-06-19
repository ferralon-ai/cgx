//! The two-pass link: build the symbol table, then resolve every reference into
//! a graph edge at an honest confidence tier (architecture §2).

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::{CutMarker, CutMarkers};
use cgx_core::edge::{Candidate, EdgeKind, EdgeRecord, EdgeWithProvenance};
use cgx_core::id::{EdgeId, NodeId, SiteId};
use cgx_core::node::{EntrypointKind, NodeRecord, NodeWithProvenance, SymbolKind};
use cgx_core::provenance::{Provenance, Span};
use cgx_frontend::facts::{RawRef, RefKind, ScopeId, ScopeTree, SymbolDef};

use crate::graph::{ResolvedGraph, UnresolvedRef};
use crate::input::{FileInput, LinkOpts};
use crate::symtab::{
    collect_import_bindings, short_name, DefEntry, ImportBinding, ResolveOutcome, SymbolTable,
};

/// Link a set of per-file fact fragments into one resolved graph.
///
/// Deterministic and order-independent: the output is a pure function of the set
/// of `(blob, path, lang, facts)` tuples, regardless of the order they are
/// supplied (WP-06 convergence criterion). See the crate docs for the full
/// tier → confidence mapping.
pub fn link(inputs: &[FileInput<'_>], opts: &LinkOpts) -> ResolvedGraph {
    // --- Pass 0: materialize nodes and assign dense ids by canonical sort. ---
    let (nodes, def_to_node) = build_nodes(inputs);
    let files: Vec<(String, &cgx_frontend::facts::FileFacts)> = inputs
        .iter()
        .map(|f| (module_segment(f), f.facts))
        .collect();
    let table = SymbolTable::build(&node_records(&nodes), &files);

    // --- Pass A/B: resolve every reference. ---
    let mut edges: Vec<EdgeWithProvenance> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut unresolved: Vec<UnresolvedRef> = Vec::new();
    let mut next_group: u32 = 0;

    for file in inputs {
        let facts = file.facts;
        let bindings = collect_import_bindings(&facts.imports);
        for raw in &facts.refs {
            let caller = match enclosing_def(&facts.scopes, &facts.defs, raw.scope) {
                Some(d) => d,
                None => continue, // a ref with no enclosing symbol has no edge source.
            };
            let caller_id = match def_to_node.get(caller.fqn.as_str()) {
                Some(id) => *id,
                None => continue,
            };
            resolve_ref(
                file,
                raw,
                caller,
                caller_id,
                &facts.scopes,
                &facts.defs,
                &bindings,
                &table,
                opts,
                &mut edges,
                &mut candidates,
                &mut unresolved,
                &mut next_group,
            );
        }
    }

    // --- Structural edges from imports (GM-2.2 Imports). ---
    // (Phase 1: only the call/structural edges the goldens exercise; the import
    // edge set is derivable but not required by WP-08's contract, so kept minimal.)

    finalize(nodes, edges, candidates, unresolved)
}

/// Build node records from every file's defs, then sort canonically and assign
/// dense [`NodeId`]s. Returns the assigned nodes (with provenance) and a
/// `fqn → NodeId` map.
fn build_nodes(
    inputs: &[FileInput<'_>],
) -> (
    Vec<NodeWithProvenance>,
    std::collections::BTreeMap<String, NodeId>,
) {
    let mut staged: Vec<(NodeRecord, Provenance)> = Vec::new();
    for file in inputs {
        let entry_kinds = entrypoint_kinds(file);
        for def in &file.facts.defs {
            let signature = def.signature.clone();
            let entrypoint_kind = entry_kinds
                .iter()
                .find(|(fqn, _)| *fqn == def.fqn)
                .map(|(_, k)| *k);
            let record = NodeRecord {
                id: NodeId(0), // assigned after sort
                kind: def.kind,
                fqn: def.fqn.clone(),
                file: file.path.clone(),
                line_start: def.span.line,
                line_end: def.line_end,
                lang: file.lang.clone(),
                visibility: def.visibility,
                is_abstract: def.is_abstract,
                entrypoint_kind,
                signature,
            };
            let prov = Provenance::new(
                def.span.clone(),
                "def",
                Tier::ScopeGraph,
                file.blob_oid.clone(),
            );
            staged.push((record, prov));
        }
    }

    // Canonical node order: (file, line_start, fqn). Assign dense ids by position.
    staged.sort_by(|a, b| {
        (a.0.file.as_str(), a.0.line_start, a.0.fqn.as_str()).cmp(&(
            b.0.file.as_str(),
            b.0.line_start,
            b.0.fqn.as_str(),
        ))
    });

    let mut map = std::collections::BTreeMap::new();
    let mut nodes = Vec::with_capacity(staged.len());
    for (i, (mut record, prov)) in staged.into_iter().enumerate() {
        record.id = NodeId(i as u32);
        map.insert(record.fqn.clone(), record.id);
        nodes.push(NodeWithProvenance {
            node: record,
            provenance: prov,
        });
    }
    (nodes, map)
}

fn node_records(nodes: &[NodeWithProvenance]) -> Vec<NodeRecord> {
    nodes.iter().map(|n| n.node.clone()).collect()
}

/// The module last-segment of a file: the key under which its re-exports are
/// recorded, matching the suffix an importer's specifier resolves to. Taken from
/// the module prefix of the file's defs (their common `::`-parent last segment),
/// falling back to the path's file stem when the file has no defs.
fn module_segment(file: &FileInput<'_>) -> String {
    if let Some(def) = file.facts.defs.iter().find(|d| d.fqn.contains("::")) {
        if let Some(idx) = def.fqn.rfind("::") {
            let module = &def.fqn[..idx];
            return module.rsplit("::").next().unwrap_or(module).to_owned();
        }
    }
    // Fall back to the file stem: `src/imports.rs` → `imports`.
    let name = file.path.rsplit(['/', '\\']).next().unwrap_or(&file.path);
    name.split('.').next().unwrap_or(name).to_owned()
}

/// Collect entrypoint (fqn, kind) hints for a file.
fn entrypoint_kinds(file: &FileInput<'_>) -> Vec<(String, EntrypointKind)> {
    file.facts
        .entrypoint_hints
        .iter()
        .map(|h| (h.fqn.clone(), h.kind))
        .collect()
}

/// Find the definition whose body lexically encloses `scope` — the caller of a
/// reference at that scope. Walks the scope ancestry to the nearest owner; then
/// matches the owner FQN to a def.
fn enclosing_def<'a>(
    scopes: &ScopeTree,
    defs: &'a [SymbolDef],
    scope: ScopeId,
) -> Option<&'a SymbolDef> {
    for sid in scopes.ancestors(scope) {
        if let Some(owner) = scopes
            .scopes
            .get(sid.index())
            .and_then(|s| s.owner_fqn.as_ref())
        {
            if let Some(def) = defs.iter().find(|d| &d.fqn == owner) {
                return Some(def);
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_ref(
    file: &FileInput<'_>,
    raw: &RawRef,
    caller: &SymbolDef,
    caller_id: NodeId,
    scopes: &ScopeTree,
    defs: &[SymbolDef],
    bindings: &[ImportBinding],
    table: &SymbolTable,
    opts: &LinkOpts,
    edges: &mut Vec<EdgeWithProvenance>,
    candidates: &mut Vec<Candidate>,
    unresolved: &mut Vec<UnresolvedRef>,
    next_group: &mut u32,
) {
    let span = Span::new(file.path.clone(), raw.span.line, raw.span.col);
    let last = match raw.name_path.last() {
        Some(n) => n.as_str(),
        None => return,
    };
    let virtual_receiver = matches!(raw.kind, RefKind::CallVirtualReceiver);

    // --- Step 1: same-file lexical resolution (Pass A). ---
    // A bare name (single segment) that resolves to a same-file def in an
    // enclosing scope is a direct, unambiguous call → certain (Tier 1 intra-file).
    if !virtual_receiver && raw.name_path.len() == 1 {
        if let Some(local) = resolve_local(scopes, defs, raw.scope, last) {
            if let Some(id) = table.def_by_fqn(&local.fqn).map(|d| d.node_id) {
                push_edge(
                    edges,
                    EdgeBuild {
                        src: caller_id,
                        dst: id,
                        kind: edge_kind(raw.kind, false),
                        confidence: Confidence::Certain,
                        tier: Tier::ScopeGraph,
                        rule: "scope-ref",
                        raw,
                        caller_fqn: &caller.fqn,
                        span: &span,
                        candidate_group: None,
                    },
                );
                return;
            }
        }
    }

    // --- Step 2: import-binding resolution (Pass B tier 1). ---
    let head = raw.name_path[0].as_str();
    if let Some(binding) = lookup_binding(bindings, scopes, raw.scope, head) {
        match table.resolve_member(&binding.specifier, &binding.exported_for(head, last)) {
            ResolveOutcome::Unique(def) => {
                push_edge(
                    edges,
                    EdgeBuild {
                        src: caller_id,
                        dst: def.node_id,
                        kind: edge_kind(raw.kind, virtual_receiver),
                        confidence: Confidence::Probable,
                        tier: Tier::ScopeGraph,
                        rule: "import-ref",
                        raw,
                        caller_fqn: &caller.fqn,
                        span: &span,
                        candidate_group: None,
                    },
                );
                return;
            }
            ResolveOutcome::Ambiguous(hits) => {
                emit_candidate_set(
                    edges,
                    candidates,
                    next_group,
                    &hits,
                    caller_id,
                    raw,
                    virtual_receiver,
                    "import-ref",
                    &caller.fqn,
                    &span,
                );
                return;
            }
            ResolveOutcome::None => { /* fall through to virtual / name-arity */ }
        }
    }

    // --- Step 3: virtual / duck-typed dispatch — same-name method candidate set. ---
    if virtual_receiver {
        let hits = method_candidates(table, last);
        if !hits.is_empty() {
            emit_candidate_set(
                edges,
                candidates,
                next_group,
                &hits.iter().collect::<Vec<_>>(),
                caller_id,
                raw,
                true,
                "name-method",
                &caller.fqn,
                &span,
            );
            return;
        }
    }

    // --- Step 4: Tier-0 global name(+arity) fallback (Pass B tier 0). ---
    if opts.name_arity_fallback {
        let mut hits: Vec<&DefEntry> = table
            .defs_by_short(last)
            .iter()
            .filter(|d| d.kind.is_callable() || matches!(d.kind, SymbolKind::Type))
            .collect();
        if opts.arity_filter {
            if let Some(arity) = raw.arity {
                let filtered: Vec<&DefEntry> = hits
                    .iter()
                    .copied()
                    .filter(|d| d.arity.is_none_or(|a| a == arity))
                    .collect();
                if !filtered.is_empty() {
                    hits = filtered;
                }
            }
        }
        if !hits.is_empty() {
            emit_candidate_set(
                edges,
                candidates,
                next_group,
                &hits,
                caller_id,
                raw,
                virtual_receiver,
                "name-arity",
                &caller.fqn,
                &span,
            );
            return;
        }
    }

    // --- Step 5: unresolved — leave dangling honestly (LS-6). ---
    unresolved.push(UnresolvedRef {
        caller_fqn: caller.fqn.clone(),
        name_path: raw.name_path.iter().cloned().collect(),
        span,
        marker: CutMarker::Unresolved,
    });
}

/// Resolve a bare name to a same-file def visible from `scope`: a def in `scope`
/// or any ancestor scope whose short name matches. Returns the nearest.
fn resolve_local<'a>(
    scopes: &ScopeTree,
    defs: &'a [SymbolDef],
    scope: ScopeId,
    name: &str,
) -> Option<&'a SymbolDef> {
    for sid in scopes.ancestors(scope) {
        if let Some(def) = defs
            .iter()
            .filter(|d| d.scope == sid && short_name(&d.fqn) == name)
            .min_by(|a, b| a.fqn.cmp(&b.fqn))
        {
            return Some(def);
        }
    }
    None
}

/// Find the import binding for a head name visible at `scope` (the binding's
/// scope must be an ancestor of, or equal to, the ref's scope), or a glob import.
fn lookup_binding<'a>(
    bindings: &'a [ImportBinding],
    scopes: &ScopeTree,
    scope: ScopeId,
    head: &str,
) -> Option<&'a ImportBinding> {
    let visible: Vec<ScopeId> = scopes.ancestors(scope).collect();
    bindings
        .iter()
        .find(|b| !b.glob && b.local == head && visible.contains(&b.scope))
        .or_else(|| {
            bindings
                .iter()
                .find(|b| b.glob && visible.contains(&b.scope))
        })
}

impl ImportBinding {
    /// The exported name to look up in the source module for a given ref.
    ///
    /// For a non-glob binding the exported name is fixed. For a glob (`use a::*`)
    /// or when the head is a namespace alias and the call is `head::last` /
    /// `head.last`, the relevant exported name is the reference's *last* segment.
    fn exported_for(&self, head: &str, last: &str) -> String {
        if self.glob {
            return last.to_owned();
        }
        if self.local == head && head != last {
            // `Counter::increment` where `Counter` is the import: look up the
            // member by its last segment within the imported type's module.
            last.to_owned()
        } else {
            self.exported.clone()
        }
    }
}

/// Same-name method candidates for virtual dispatch (GM-8.4): all `Method` (and
/// abstract trait-method) defs whose short name matches.
fn method_candidates(table: &SymbolTable, name: &str) -> Vec<DefEntry> {
    table
        .defs_by_short(name)
        .iter()
        .filter(|d| matches!(d.kind, SymbolKind::Method | SymbolKind::Function))
        .cloned()
        .collect()
}

/// Map a [`RefKind`] to a graph [`EdgeKind`]. `virtual_receiver_resolved` is set
/// when a [`RefKind::CallVirtualReceiver`] was bound to a single concrete target
/// — it stays a `Calls` (the resolver narrowed it), otherwise `CallsVirtual`.
fn edge_kind(kind: RefKind, virtual_dispatch: bool) -> EdgeKind {
    match kind {
        RefKind::Call => EdgeKind::Calls,
        RefKind::CallVirtualReceiver => {
            if virtual_dispatch {
                EdgeKind::CallsVirtual
            } else {
                EdgeKind::Calls
            }
        }
        RefKind::CallClosure => EdgeKind::CallsClosure,
        RefKind::CallCallback => EdgeKind::CallsCallback,
        RefKind::CallAsync => EdgeKind::CallsAsync,
        RefKind::Spawn => EdgeKind::Spawns,
        RefKind::Reference => EdgeKind::References,
        RefKind::Instantiate => EdgeKind::Instantiates,
    }
}

/// Parameters for pushing one resolved edge.
struct EdgeBuild<'a> {
    src: NodeId,
    dst: NodeId,
    kind: EdgeKind,
    confidence: Confidence,
    tier: Tier,
    rule: &'static str,
    raw: &'a RawRef,
    caller_fqn: &'a str,
    span: &'a Span,
    candidate_group: Option<u32>,
}

fn push_edge(edges: &mut Vec<EdgeWithProvenance>, b: EdgeBuild<'_>) {
    let site_id = if b.kind.is_call() {
        Some(SiteId::derive(
            b.caller_fqn,
            &b.span.file,
            b.span.line,
            b.span.col.unwrap_or(0),
        ))
    } else {
        None
    };
    let mut cut_markers = CutMarkers::new();
    for m in b.raw.cut_markers.iter() {
        cut_markers.insert(*m);
    }
    let record = EdgeRecord {
        id: EdgeId(0), // assigned after canonical sort
        src: b.src,
        dst: b.dst,
        kind: b.kind,
        condition: b.raw.edge_condition,
        confidence: b.confidence,
        tier: b.tier,
        rule: b.rule.to_owned(),
        site_id,
        stmt_index: Some(b.raw.stmt_index),
        cut_markers,
        implicit: b.raw.implicit,
        candidate_group: b.candidate_group,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
    };
    let prov = Provenance::new(b.span.clone(), b.rule, b.tier, String::new());
    edges.push(EdgeWithProvenance {
        edge: record,
        provenance: prov,
    });
}

/// Emit one edge per candidate, all sharing a candidate group, at `possible`
/// (multiple) or `probable` (single). Records the candidate table rows.
#[allow(clippy::too_many_arguments)]
fn emit_candidate_set(
    edges: &mut Vec<EdgeWithProvenance>,
    candidates: &mut Vec<Candidate>,
    next_group: &mut u32,
    hits: &[&DefEntry],
    caller_id: NodeId,
    raw: &RawRef,
    virtual_dispatch: bool,
    rule: &'static str,
    caller_fqn: &str,
    span: &Span,
) {
    // Sort candidates by node id for determinism; single hit → probable, else
    // possible (the over-approximation band, GM-5.1).
    let mut ordered: Vec<&DefEntry> = hits.to_vec();
    ordered.sort_by_key(|a| a.node_id.0);
    ordered.dedup_by(|a, b| a.node_id == b.node_id);

    let confidence = if ordered.len() == 1 {
        Confidence::Probable
    } else {
        Confidence::Possible
    };
    let group = if ordered.len() > 1 {
        let g = *next_group;
        *next_group += 1;
        Some(g)
    } else {
        None
    };
    let tier = if rule == "name-arity" {
        Tier::NameSyntactic
    } else {
        Tier::ScopeGraph
    };

    for (rank, def) in ordered.iter().enumerate() {
        if let Some(g) = group {
            candidates.push(Candidate {
                candidate_group: g,
                dst: def.node_id,
                rank: rank as u32,
            });
        }
        push_edge(
            edges,
            EdgeBuild {
                src: caller_id,
                dst: def.node_id,
                kind: edge_kind(raw.kind, virtual_dispatch),
                confidence,
                tier,
                rule,
                raw,
                caller_fqn,
                span,
                candidate_group: group,
            },
        );
    }
}

/// Sort nodes/edges into canonical order, assign dense edge ids, sort candidates.
fn finalize(
    nodes: Vec<NodeWithProvenance>,
    edges: Vec<EdgeWithProvenance>,
    candidates: Vec<Candidate>,
    mut unresolved: Vec<UnresolvedRef>,
) -> ResolvedGraph {
    unresolved.sort();
    let mut graph = ResolvedGraph {
        nodes,
        edges,
        candidates,
        unresolved,
    };
    canonicalize(&mut graph);
    graph
}

/// Re-establish the canonical edge order, dense [`EdgeId`] assignment, and
/// candidate-set order on an already-built [`ResolvedGraph`].
///
/// This is the determinism contract (architecture §4): edges are sorted by
/// `(src, dst, kind, stmt_index, candidate_group)` — matching
/// [`cgx_core::sort::edge_sort_key`] — and assigned dense ids by position;
/// candidates are sorted by `(group, rank, dst)`. Nodes are left untouched (the
/// link step assigns their ids once at build time, before any post-pass runs).
///
/// [`link`] calls this internally. A post-pass that mutates edge endpoints or
/// candidate-group membership (e.g. the SCIP relabel pass redirecting a `dst` or
/// collapsing a group) must call it again before the graph is stored, so stored
/// `EdgeId`s stay canonical (§7). A pass that rewrites only
/// `confidence`/`tier`/`rule` leaves the sort key untouched and need not call it.
pub fn canonicalize(graph: &mut ResolvedGraph) {
    // Edges: canonical order is (src, dst, kind, stmt_index, candidate_group),
    // matching cgx_core::sort::edge_sort_key. Assign dense ids by position.
    graph.edges.sort_by(|a, b| {
        let ka = (
            a.edge.src.0,
            a.edge.dst.0,
            a.edge.kind as u8,
            a.edge.stmt_index.unwrap_or(u32::MAX),
            a.edge.candidate_group.unwrap_or(u32::MAX),
        );
        let kb = (
            b.edge.src.0,
            b.edge.dst.0,
            b.edge.kind as u8,
            b.edge.stmt_index.unwrap_or(u32::MAX),
            b.edge.candidate_group.unwrap_or(u32::MAX),
        );
        ka.cmp(&kb)
    });
    for (i, e) in graph.edges.iter_mut().enumerate() {
        e.edge.id = EdgeId(i as u32);
    }

    graph.candidates.sort_by(|a, b| {
        (a.candidate_group, a.rank, a.dst.0).cmp(&(b.candidate_group, b.rank, b.dst.0))
    });
}
