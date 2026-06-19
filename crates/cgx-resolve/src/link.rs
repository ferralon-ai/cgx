//! The two-pass link: build the symbol table, then resolve every reference into
//! a graph edge at an honest confidence tier (architecture §2).

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::{CutMarker, CutMarkers};
use cgx_core::edge::{Candidate, EdgeKind, EdgeRecord, EdgeWithProvenance};
use cgx_core::id::{EdgeId, NodeId, SiteId};
use cgx_core::node::{EntrypointKind, NodeRecord, NodeWithProvenance, SymbolKind};
use cgx_core::provenance::{Provenance, Span};
use cgx_frontend::facts::{ImplRelation, RawRef, RefKind, RelationKind, ScopeId, ScopeTree, SymbolDef};

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

    // --- Structural type/trait-lattice edges (GM-2.2): Implements / Inherits /
    // Overrides. These are declared relations, not call sites, so they resolve
    // by FQN-or-short-name against the symbol table and never carry a site_id. ---
    for file in inputs {
        for rel in &file.facts.impl_relations {
            resolve_impl_relation(file, rel, &def_to_node, &table, &mut edges);
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
            // GM-12 Phase 1: stamp the frontend's syntactic own-effects onto the
            // matching def. Only callable kinds carry effects; a non-callable def
            // with a same-named effect fact (cannot happen for the Rust frontend,
            // which keys by enclosing-fn FQN) would still pick it up harmlessly.
            let own_effects = file
                .facts
                .effects
                .iter()
                .find(|e| e.fqn == def.fqn)
                .map(|e| e.effects)
                .unwrap_or_default();
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
                own_effects,
                // P8b populates this via the transitive closure pass; empty here.
                transitive_effects: cgx_core::EffectSet::new(),
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

    // --- Step 0: indirect call through a function value (closure / fn-pointer /
    // callback). These cannot be bound to a single target by name: the value that
    // flows to the call site is not named at the call (`f(x)` where `f` is a
    // closure binding or a `fn(i32) -> i32` parameter). The frontend has no static
    // type for the value, so name resolution would either misfire on an unrelated
    // same-named def or leave it dangling. Emit a single placeholder edge carrying
    // the syntactic call-site arity in the denormalized `rule` (`indirect:<arity>`
    // — same no-schema encoding pattern as the SCIP dep-attr suffix). The P6
    // signature post-pass (`crate::sig`) replaces it with the signature-compatible
    // candidate set over `Lambda` + free `Function` nodes. Arity-less sites encode
    // `indirect:?`. (DF-18 value-flow narrowing — which closure actually flows here
    // — is Phase 3 and explicitly out of scope.)
    if matches!(raw.kind, RefKind::CallClosure | RefKind::CallCallback) {
        let arity_tag = raw.arity.map(|a| a.to_string()).unwrap_or_else(|| "?".to_owned());
        push_edge(
            edges,
            EdgeBuild {
                src: caller_id,
                // Self-edge sentinel: the placeholder has no real target until the
                // signature post-pass expands it; `canonicalize` keeps it stable and
                // `crate::sig::run_sig` drops it when building the candidate set.
                dst: caller_id,
                kind: edge_kind(raw.kind, false),
                confidence: Confidence::Possible,
                tier: Tier::NameSyntactic,
                rule: "indirect",
                raw,
                caller_fqn: &caller.fqn,
                span: &span,
                candidate_group: None,
            },
        );
        // Stamp the call-site arity into the denormalized rule string
        // (`indirect:<arity>`) — `EdgeBuild::rule` is `&'static`, so the dynamic
        // arity tag is written after the push. Read back by `crate::sig`.
        if let Some(e) = edges.last_mut() {
            let r = format!("indirect:{arity_tag}");
            e.edge.rule = r.clone();
            e.provenance.rule = r;
        }
        return;
    }

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

/// Resolve one structural type/trait-lattice relation into a graph edge
/// (`Implements` / `Inherits` / `Overrides`). The subject/object name paths are
/// resolved against the symbol table; ambiguous or unresolved endpoints leave no
/// edge (honest: a relation we cannot ground is silently absent, not invented).
fn resolve_impl_relation(
    file: &FileInput<'_>,
    rel: &ImplRelation,
    def_to_node: &std::collections::BTreeMap<String, NodeId>,
    table: &SymbolTable,
    edges: &mut Vec<EdgeWithProvenance>,
) {
    let (src, dst, kind) = match rel.kind {
        RelationKind::Implements | RelationKind::Inherits => {
            // subject/object are type/trait short names; resolve the unique Type def.
            let Some(src) = resolve_type_node(table, &rel.subject) else {
                return;
            };
            let Some(dst) = resolve_type_node(table, &rel.object) else {
                return;
            };
            let kind = if rel.kind == RelationKind::Implements {
                EdgeKind::Implements
            } else {
                EdgeKind::Inherits
            };
            (src, dst, kind)
        }
        RelationKind::Overrides => {
            // subject is the impl method's full FQN; object is `[Trait, method]`.
            let impl_fqn = rel.subject.join("::");
            let Some(src) = def_to_node.get(impl_fqn.as_str()).copied() else {
                return;
            };
            let Some(dst) = resolve_trait_method(table, &rel.object) else {
                return;
            };
            if src == dst {
                return; // a default method matched to itself: no override.
            }
            (src, dst, EdgeKind::Overrides)
        }
    };

    let span = Span::new(file.path.clone(), rel.span.line, rel.span.col);
    push_structural_edge(edges, src, dst, kind, &span);
}

/// Resolve a type/trait name path to a unique `Type` node id. Prefers an exact
/// FQN match; falls back to a unique `Type` def with the matching short name.
fn resolve_type_node(table: &SymbolTable, name_path: &[String]) -> Option<NodeId> {
    let joined = name_path.join("::");
    if let Some(d) = table.def_by_fqn(&joined) {
        if matches!(d.kind, SymbolKind::Type) {
            return Some(d.node_id);
        }
    }
    let last = name_path.last()?;
    let types: Vec<&DefEntry> = table
        .defs_by_short(last)
        .iter()
        .filter(|d| matches!(d.kind, SymbolKind::Type))
        .collect();
    match types.as_slice() {
        [only] => Some(only.node_id),
        _ => None, // unresolved or ambiguous → no edge.
    }
}

/// Resolve a `[Trait, method]` path to the trait's method node. Prefers an exact
/// `Trait::method` FQN; falls back to a unique Method/Function whose FQN ends in
/// `Trait::method`.
fn resolve_trait_method(table: &SymbolTable, name_path: &[String]) -> Option<NodeId> {
    if name_path.len() < 2 {
        return None;
    }
    let trait_name = &name_path[name_path.len() - 2];
    let method = name_path.last()?;
    let exact = format!("{trait_name}::{method}");
    if let Some(d) = table.def_by_fqn(&exact) {
        if matches!(d.kind, SymbolKind::Method | SymbolKind::Function) {
            return Some(d.node_id);
        }
    }
    let suffix = format!("::{trait_name}::{method}");
    let hits: Vec<&DefEntry> = table
        .defs_by_short(method)
        .iter()
        .filter(|d| {
            matches!(d.kind, SymbolKind::Method | SymbolKind::Function) && d.fqn.ends_with(&suffix)
        })
        .collect();
    match hits.as_slice() {
        [only] => Some(only.node_id),
        _ => None,
    }
}

/// Push a structural (non-call) edge at `Certain`/`ScopeGraph`: these relations
/// are stated directly in source (`impl Trait for T`), so they are not
/// over-approximations. No site_id, no candidate group.
fn push_structural_edge(
    edges: &mut Vec<EdgeWithProvenance>,
    src: NodeId,
    dst: NodeId,
    kind: EdgeKind,
    span: &Span,
) {
    let rule = match kind {
        EdgeKind::Implements => "impl-relation",
        EdgeKind::Inherits => "supertrait",
        EdgeKind::Overrides => "trait-override",
        _ => "impl-relation",
    };
    let record = EdgeRecord {
        id: EdgeId(0),
        src,
        dst,
        kind,
        condition: cgx_core::condition::EdgeCondition::Always,
        confidence: Confidence::Certain,
        tier: Tier::ScopeGraph,
        rule: rule.to_owned(),
        site_id: None,
        stmt_index: None,
        cut_markers: CutMarkers::new(),
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
    };
    let prov = Provenance::new(span.clone(), rule, Tier::ScopeGraph, String::new());
    edges.push(EdgeWithProvenance {
        edge: record,
        provenance: prov,
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

/// Canonicalize a candidate destination set: sort by `node_id` and dedup. The
/// **single** canonical-candidate-set discipline (GM-2.1, determinism §4) — the
/// link pass *and* the CHA post-pass ([`crate::cha`]) both route through this so
/// candidate ordering, dedup, and the single→`probable` / multi→`possible` band
/// are defined in exactly one place. Returns the confidence band for the
/// (deduped) set: one target ⇒ [`Confidence::Probable`], several ⇒
/// [`Confidence::Possible`].
pub(crate) fn canonicalize_candidate_dsts(dsts: &mut Vec<NodeId>) -> Confidence {
    dsts.sort_by_key(|id| id.0);
    dsts.dedup();
    if dsts.len() == 1 {
        Confidence::Probable
    } else {
        Confidence::Possible
    }
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
    // Canonicalize the dst set through the shared helper so ordering/dedup/band
    // match the CHA post-pass exactly.
    let mut dsts: Vec<NodeId> = hits.iter().map(|d| d.node_id).collect();
    let confidence = canonicalize_candidate_dsts(&mut dsts);
    let group = if dsts.len() > 1 {
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

    for (rank, dst) in dsts.iter().enumerate() {
        if let Some(g) = group {
            candidates.push(Candidate {
                candidate_group: g,
                dst: *dst,
                rank: rank as u32,
            });
        }
        push_edge(
            edges,
            EdgeBuild {
                src: caller_id,
                dst: *dst,
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
