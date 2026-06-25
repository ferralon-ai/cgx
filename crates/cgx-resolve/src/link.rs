//! The two-pass link: build the symbol table, then resolve every reference into
//! a graph edge at an honest confidence tier (architecture §2).

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::{CutMarker, CutMarkers};
use cgx_core::edge::{Candidate, EdgeKind, EdgeRecord, EdgeWithProvenance};
use cgx_core::id::{EdgeId, NodeId, SiteId};
use cgx_core::node::{EntrypointKind, NodeRecord, NodeWithProvenance, SymbolKind, Visibility};
use cgx_core::provenance::{Provenance, Span};
use cgx_core::transform::Transform;
use cgx_frontend::facts::{
    DataFlowFact, ImplRelation, RawRef, RefKind, RelationKind, ScopeId, ScopeTree, SymbolDef,
};

use crate::graph::{DataflowOutput, ResolvedGraph, UnresolvedRef};
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
    let (mut nodes, def_to_node) = build_nodes(inputs);
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

    // --- v0.3 DATA_FLOW SC2 (opt-in): materialize SSA value nodes + DerivesFrom
    // edges from each file's `data_flows`. A no-op when `opts.dataflow` is off, so
    // the base index keeps zero value nodes / zero DerivesFrom edges. ---
    let dataflow = if opts.dataflow {
        resolve_data_flows(
            inputs,
            &def_to_node,
            &table,
            &mut nodes,
            &mut edges,
            &opts.prior_fn_cache,
            opts.max_summary_edges
                .unwrap_or(crate::ifds::DEFAULT_MAX_SUMMARY_EDGES),
        )
    } else {
        DataflowOutput::default()
    };

    let mut graph = finalize(nodes, edges, candidates, unresolved);
    graph.dataflow = dataflow;
    graph
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

/// v0.3 DATA_FLOW SC2: materialize SSA value nodes and intraprocedural
/// `DerivesFrom` edges from every file's `data_flows`.
///
/// Value nodes are minted as `NodeRecord`s (kind `Variable`, `Value` flavor in
/// the model) with a content-derived synthetic FQN `<fn>::<local>#<version>`, so
/// re-assignment yields distinct nodes (flow-sensitivity) and two indexings of an
/// unchanged blob mint identical FQNs (determinism). They are **appended** after
/// the symbol nodes — symbol node ids are left untouched, so a base index (this
/// pass not run) is byte-identical.
///
/// Source/derived name-paths resolve via the same intraprocedural scoping
/// `RawRef` uses: the resolver replays each function's facts in span order,
/// tracking the current SSA version of each local, so a `source` use binds to the
/// value node of the version live at that point (or version 0 = the parameter /
/// initial binding). Endpoints that resolve cleanly get `Certain`/`Probable` per
/// the transform (design §1.3); a fact carrying an `OpaqueCall` cut has no source
/// and emits no edge (SC4 resolves it through a summary).
fn resolve_data_flows(
    inputs: &[FileInput<'_>],
    def_to_node: &std::collections::BTreeMap<String, NodeId>,
    table: &SymbolTable,
    nodes: &mut Vec<NodeWithProvenance>,
    edges: &mut Vec<EdgeWithProvenance>,
    prior_cache: &std::collections::HashMap<(String, String), String>,
    summary_budget: u64,
) -> DataflowOutput {
    let mut output = build_incremental_substrate(inputs, table, prior_cache);

    // Intern value nodes by synthetic FQN so repeated references to the same SSA
    // def share one node. A `BTreeMap` keeps minting order deterministic.
    let mut value_nodes: std::collections::BTreeMap<String, ValueNodeStage> =
        std::collections::BTreeMap::new();
    let mut pending_edges: Vec<PendingDataFlowEdge> = Vec::new();

    // v0.3 SC4 IFDS inputs, accumulated per function as we walk the facts.
    let mut fn_dataflow: std::collections::BTreeMap<String, crate::ifds::FnDataflow> =
        std::collections::BTreeMap::new();
    // The owning file blob OID per function (for persisting summaries keyed by
    // (blob_oid, fn_fqn), matching the SC3 cache keying).
    let mut fn_blob: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    // Decision C: explicitly mint every parameter's formal-in value node
    // `<fn>::<param>#0` BEFORE the IFDS pass, so a param that is never read as an
    // intraproc source still has a node and can be a summary source.
    for file in inputs {
        let facts = file.facts;
        for def in &facts.defs {
            let Some(sig) = def.signature.as_ref() else {
                continue;
            };
            if !def.kind.is_callable() {
                continue;
            }
            let entry = fn_dataflow.entry(def.fqn.clone()).or_default();
            fn_blob.entry(def.fqn.clone()).or_insert_with(|| file.blob_oid.clone());
            for p in &sig.params {
                let fqn = value_fqn(&def.fqn, &p.name, 0);
                intern_param_node(&mut value_nodes, &fqn, file, def);
                entry.formal_ins.push(fqn);
            }
        }
    }

    for file in inputs {
        // Group this file's facts by owning function so SSA versions are tracked
        // per function. The function of a fact is the scope's enclosing def.
        let facts = file.facts;
        // Per-function current version of each source name (0 = param / initial).
        // The forward pass below reconstructs def-use order, so it must iterate in
        // true PROGRAM order. `canonicalize()` stores `data_flows` sorted by the
        // derived NAME (for byte-identical postcard encoding), NOT by span — a
        // binding whose name sorts before its source's def would otherwise resolve
        // the source to a dead phantom version `#0` (Gap A). Resolve over a
        // span-ordered view so source-version reconstruction is independent of the
        // canonical storage order; storage stays name-sorted (determinism intact).
        let mut current: std::collections::HashMap<(String, String), u32> =
            std::collections::HashMap::new();

        let mut ordered: Vec<&DataFlowFact> = facts.data_flows.iter().collect();
        ordered.sort_by(|a, b| {
            (&a.span, &a.derived, a.derived_version)
                .cmp(&(&b.span, &b.derived, b.derived_version))
        });

        for df in ordered {
            let Some(caller) = enclosing_def(&facts.scopes, &facts.defs, df.scope) else {
                continue;
            };
            let fn_fqn = caller.fqn.as_str();
            fn_blob
                .entry(fn_fqn.to_string())
                .or_insert_with(|| file.blob_oid.clone());

            // The derived endpoint: a fresh value node for this SSA def.
            let derived_local = df.derived.last().cloned().unwrap_or_default();
            let derived_fqn = value_fqn(fn_fqn, &derived_local, df.derived_version);
            intern_value_node(&mut value_nodes, &derived_fqn, file, df);

            // A return fact's derived local is "return" (the derived path is
            // `[fn, "return"]`). Record the return value node so IFDS can search
            // backward from it to the formal-ins.
            let is_return = derived_local == "return";
            if is_return {
                fn_dataflow
                    .entry(fn_fqn.to_string())
                    .or_default()
                    .returns
                    .insert(derived_fqn.clone());
            }

            // Resolve the source endpoint at its currently-live version BEFORE
            // recording the derived def. Def-use order: `b = b + 1` reads the old
            // `b` (the version live on entry, or 0 = param) and writes a new one,
            // so a self-named source binds to the prior value node, never itself.
            let source_node_fqn = if df.source.is_empty() {
                None
            } else {
                // Resolve the BASE local of the access path, not the field tail:
                // `let x = p.lo;` emits source path `["p","lo"]` and must flow from
                // `p` (the local), with `lo` carried as the `Projection` transform
                // (depth-1 field sensitivity, design §B). Taking `.last()` here bound
                // `x` to a phantom local named after the field (Gap 2). Depth-2+
                // paths are already truncated to their base by the frontend, so
                // `.first()` is correct for them too.
                let source_local = df.source.first().cloned().unwrap_or_default();
                let source_version = *current
                    .get(&(fn_fqn.to_string(), source_local.clone()))
                    .unwrap_or(&0);
                let source_fqn = value_fqn(fn_fqn, &source_local, source_version);
                intern_value_node(&mut value_nodes, &source_fqn, file, df);
                Some(source_fqn)
            };

            // An opaque-call fact records an IFDS call site (decision F): resolve
            // each argument access-path to the live value-node FQN at this point,
            // so the summary maps `formal_in_i ⇝ arg_i`.
            if df.cut_markers.contains(&CutMarker::OpaqueCall) {
                let args: Vec<Option<String>> = df
                    .args
                    .iter()
                    .map(|path| {
                        // Base local of the arg access-path (`g(b.x)` flows the arg
                        // from `b`, not the field `x`). Mirrors the Gap-2 fix above.
                        let base = path.first()?;
                        let version = *current
                            .get(&(fn_fqn.to_string(), base.clone()))
                            .unwrap_or(&0);
                        let fqn = value_fqn(fn_fqn, base, version);
                        intern_value_node(&mut value_nodes, &fqn, file, df);
                        Some(fqn)
                    })
                    .collect();
                fn_dataflow
                    .entry(fn_fqn.to_string())
                    .or_default()
                    .calls
                    .push(crate::ifds::CallSite {
                        result_fqn: derived_fqn.clone(),
                        callee_fqn: ground_summary_callee(table, df.callee_fqn.as_deref()),
                        args,
                        condition: df.edge_condition,
                        span: Span::new(file.path.clone(), df.span.line, df.span.col),
                    });
            }

            // Record that `derived_local` is now at `derived_version` for later
            // uses (after the source above was resolved against the prior version).
            current.insert(
                (fn_fqn.to_string(), derived_local.clone()),
                df.derived_version,
            );

            // An opaque-call fact (empty source) mints the derived node but emits
            // no intraproc edge — SC4 resolves it through the callee summary.
            let Some(source_fqn) = source_node_fqn else {
                continue;
            };

            // Record the intraproc hop for IFDS reachability composition.
            fn_dataflow
                .entry(fn_fqn.to_string())
                .or_default()
                .intra
                .push(crate::ifds::IntraEdge {
                    derived_fqn: derived_fqn.clone(),
                    source_fqn: source_fqn.clone(),
                    transform: df.transform,
                    condition: df.edge_condition,
                });

            pending_edges.push(PendingDataFlowEdge {
                derived_fqn,
                source_fqn,
                transform: df.transform,
                condition: df.edge_condition,
                cut_markers: df.cut_markers.iter().copied().collect(),
                span: Span::new(file.path.clone(), df.span.line, df.span.col),
            });
        }
    }

    // Assign dense ids to value nodes, appended after the existing symbol nodes,
    // in canonical (file, line, fqn) order. Symbol node ids are unchanged.
    let mut staged: Vec<ValueNodeStage> = value_nodes.into_values().collect();
    staged.sort_by(|a, b| {
        (a.file.as_str(), a.line, a.fqn.as_str()).cmp(&(b.file.as_str(), b.line, b.fqn.as_str()))
    });
    let mut value_fqn_to_node: std::collections::BTreeMap<String, NodeId> =
        std::collections::BTreeMap::new();
    let base = nodes.len() as u32;
    for (offset, stage) in staged.into_iter().enumerate() {
        let id = NodeId(base + offset as u32);
        value_fqn_to_node.insert(stage.fqn.clone(), id);
        nodes.push(stage.into_node(id));
    }

    // Resolve and push the DerivesFrom edges now that every value node has an id.
    // A source name path may also reference a function-boundary symbol (e.g. a
    // parameter that is itself a def) — fall back to the symbol node map.
    for pe in pending_edges {
        let Some(derived) = value_fqn_to_node.get(&pe.derived_fqn).copied() else {
            continue;
        };
        let source = value_fqn_to_node
            .get(&pe.source_fqn)
            .copied()
            .or_else(|| def_to_node.get(&pe.source_fqn).copied());
        let Some(source) = source else {
            continue;
        };
        push_data_flow_edge(edges, derived, source, &pe);
    }

    // v0.3 SC4: run the IFDS interprocedural-summary worklist and materialize the
    // interproc edges into the SAME `edges` vector (no second execution path,
    // criterion 5). The summary edges are appended fresh for all functions every
    // run (matching SC3's edge-always-rebuilt invariant — never substitute cached
    // interproc edges).
    let resolve = |fqn: &str| value_fqn_to_node.get(fqn).copied();
    let ifds = crate::ifds::run_ifds_summaries(&fn_dataflow, &resolve, summary_budget);
    edges.extend(ifds.edges);

    // Persist the computed summaries keyed by (blob_oid, fn_fqn).
    for (fn_fqn, facts) in &ifds.summaries {
        if facts.is_empty() {
            continue;
        }
        let blob = fn_blob.get(fn_fqn).cloned().unwrap_or_default();
        let bytes = cgx_core::codec::encode(facts).unwrap_or_default();
        output.fn_summaries.push(((blob, fn_fqn.clone()), bytes));
    }
    output.fn_summaries.sort();
    output.ifds_stats = crate::graph::IfdsDataflowStats {
        summaries_computed: ifds.stats.summaries_computed,
        summary_edges_materialized: ifds.stats.summary_edges_materialized,
        budget_exceeded_sccs: ifds.stats.budget_exceeded_sccs,
    };

    output.stats.functions_recomputed = output.changed_fns.len();
    output.stats.functions_reused =
        output.fn_intraproc_cache.len() - output.changed_fns.len();
    output
}

/// Ground an opaque call's syntactic callee name to a resolved FQN for IFDS
/// summary application. Unlike [`ground_callee`] (which falls back to the `"*"`
/// wildcard for `summary_deps` over-invalidation), this returns `None` when the
/// callee is not a single uniquely-resolvable callable — a virtual/unknown callee
/// has no single summary to apply, so it stays an opaque cut.
fn ground_summary_callee(table: &SymbolTable, callee_fqn: Option<&str>) -> Option<String> {
    let name = callee_fqn?;
    if let Some(def) = table.def_by_fqn(name) {
        if def.kind.is_callable() {
            return Some(def.fqn.clone());
        }
    }
    let last = name.rsplit("::").next().unwrap_or(name);
    let callables: Vec<&DefEntry> = table
        .defs_by_short(last)
        .iter()
        .filter(|d| d.kind.is_callable())
        .collect();
    match callables.as_slice() {
        [only] => Some(only.fqn.clone()),
        _ => None,
    }
}

/// Build the v0.3 SC3 incremental-dataflow substrate: per-function content
/// hashes, the `summary_deps` relation, and the own-dirty set, by partitioning
/// every file's `data_flows` by owning function.
///
/// A function's cache key is `(blob_oid, fn_fqn)`; its value is the FNV-1a hash
/// of the canonical postcard bytes of its (already span-sorted) `DataFlowFact`
/// subset. A function whose hash differs from `prior_cache` (or is absent) is
/// *changed* (a recompute); a match is a reuse. Each `OpaqueCall` fact records a
/// `summary_deps` row: its `callee_fqn` grounded against the symbol table to a
/// real FQN when uniquely resolvable, else the `"*"` wildcard (conservative
/// fallback — over-invalidate rather than miss a staleness).
fn build_incremental_substrate(
    inputs: &[FileInput<'_>],
    table: &SymbolTable,
    prior_cache: &std::collections::HashMap<(String, String), String>,
) -> DataflowOutput {
    // Partition facts by (blob_oid, fn_fqn). A BTreeMap keeps the persisted cache
    // and summary_deps rows in a deterministic order.
    let mut per_fn: std::collections::BTreeMap<(String, String), Vec<&DataFlowFact>> =
        std::collections::BTreeMap::new();
    for file in inputs {
        let facts = file.facts;
        for df in &facts.data_flows {
            let Some(caller) = enclosing_def(&facts.scopes, &facts.defs, df.scope) else {
                continue;
            };
            per_fn
                .entry((file.blob_oid.clone(), caller.fqn.clone()))
                .or_default()
                .push(df);
        }
    }

    // Reuse is detected by `fn_fqn`, not by the full `(blob_oid, fn_fqn)` key:
    // editing one function in a file changes the WHOLE file's blob OID, so every
    // function in it would otherwise miss. A function is reused iff its recomputed
    // `facts_hash` matches the hash recorded for that `fn_fqn` in the prior run.
    let prior_by_fqn: std::collections::HashMap<&str, &str> = prior_cache
        .iter()
        .map(|((_blob, fqn), hash)| (fqn.as_str(), hash.as_str()))
        .collect();

    let mut output = DataflowOutput::default();
    // summary_deps deduped per function: a fn may call the same callee twice.
    let mut deps: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();

    for ((blob_oid, fn_fqn), fn_facts) in &per_fn {
        // Content hash of the function's canonical fact subset. `data_flows` is
        // already span-sorted by `canonicalize()`, so the encoding is stable.
        let bytes = cgx_core::codec::encode(fn_facts).unwrap_or_default();
        let facts_hash = cgx_core::id::content_hash_hex(&bytes);

        let key = (blob_oid.clone(), fn_fqn.clone());
        let unchanged = prior_by_fqn
            .get(fn_fqn.as_str())
            .is_some_and(|h| *h == facts_hash);
        if !unchanged {
            output.changed_fns.push(fn_fqn.clone());
        }
        output.fn_intraproc_cache.push((key, facts_hash));

        // Record summary deps for every opaque-call fact in this function.
        for df in fn_facts {
            if !df.cut_markers.contains(&CutMarker::OpaqueCall) {
                continue;
            }
            let callee = ground_callee(table, df.callee_fqn.as_deref());
            deps.insert((fn_fqn.clone(), callee));
        }
    }

    output.summary_deps = deps.into_iter().collect();
    output
}

/// Ground an opaque call's syntactic callee name to a `summary_deps` callee key:
/// the unique resolved FQN when the name binds to exactly one callable def, else
/// the `"*"` wildcard. A missing callee name (a method call on a receiver
/// expression — virtual / duck-typed) is also a wildcard (conservative).
fn ground_callee(table: &SymbolTable, callee_fqn: Option<&str>) -> String {
    const WILDCARD: &str = "*";
    let Some(name) = callee_fqn else {
        return WILDCARD.to_string();
    };
    if let Some(def) = table.def_by_fqn(name) {
        if def.kind.is_callable() {
            return def.fqn.clone();
        }
    }
    let last = name.rsplit("::").next().unwrap_or(name);
    let callables: Vec<&DefEntry> = table
        .defs_by_short(last)
        .iter()
        .filter(|d| d.kind.is_callable())
        .collect();
    match callables.as_slice() {
        [only] => only.fqn.clone(),
        _ => WILDCARD.to_string(),
    }
}

/// A value node staged before id assignment.
///
/// Identity is the content-derived [`ValueId`] (design §A.1); it is rendered into
/// the deterministic synthetic `fqn` (`<fn>::<local>#<version>`) under which the
/// node is persisted, so two indexings of an unchanged blob mint the same node.
struct ValueNodeStage {
    fqn: String,
    file: String,
    line: u32,
    span: Span,
    blob_oid: String,
}

impl ValueNodeStage {
    fn into_node(self, id: NodeId) -> NodeWithProvenance {
        let record = NodeRecord {
            id,
            kind: SymbolKind::Variable,
            fqn: self.fqn,
            file: self.file,
            line_start: self.line,
            line_end: self.line,
            lang: String::new(),
            visibility: Visibility::Private,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: cgx_core::EffectSet::new(),
            transitive_effects: cgx_core::EffectSet::new(),
        };
        let prov = Provenance::new(self.span, "ssa-value", Tier::ScopeGraph, self.blob_oid);
        NodeWithProvenance {
            node: record,
            provenance: prov,
        }
    }
}

/// A DerivesFrom edge whose endpoints are resolved after value-node ids exist.
struct PendingDataFlowEdge {
    derived_fqn: String,
    source_fqn: String,
    transform: Transform,
    condition: cgx_core::condition::EdgeCondition,
    cut_markers: Vec<CutMarker>,
    span: Span,
}

/// The deterministic synthetic FQN of an SSA value node:
/// `<fn_fqn>::<local>#<version>`. Version 0 is the parameter / initial binding.
fn value_fqn(fn_fqn: &str, local: &str, version: u32) -> String {
    format!("{fn_fqn}::{local}#{version}")
}

/// Intern (or refresh) a staged value node under its synthetic FQN. The first
/// definition site (the fact whose span we see first) anchors the node's span; a
/// later use of the same SSA version reuses it.
fn intern_value_node(
    value_nodes: &mut std::collections::BTreeMap<String, ValueNodeStage>,
    fqn: &str,
    file: &FileInput<'_>,
    df: &DataFlowFact,
) {
    value_nodes.entry(fqn.to_string()).or_insert_with(|| ValueNodeStage {
        fqn: fqn.to_string(),
        file: file.path.clone(),
        line: df.span.line,
        span: Span::new(file.path.clone(), df.span.line, df.span.col),
        blob_oid: file.blob_oid.clone(),
    });
}

/// Intern a parameter's formal-in value node `<fn>::<param>#0` (decision C). Its
/// span anchors to the function definition (a param has no statement of its own);
/// a later intraproc *use* of the same `#0` version reuses this entry.
fn intern_param_node(
    value_nodes: &mut std::collections::BTreeMap<String, ValueNodeStage>,
    fqn: &str,
    file: &FileInput<'_>,
    def: &SymbolDef,
) {
    value_nodes.entry(fqn.to_string()).or_insert_with(|| ValueNodeStage {
        fqn: fqn.to_string(),
        file: file.path.clone(),
        line: def.span.line,
        span: Span::new(file.path.clone(), def.span.line, def.span.col),
        blob_oid: file.blob_oid.clone(),
    });
}

/// Push one resolved `DerivesFrom` edge: directed derived → source (docs/04
/// DF-3.1), tagged with the transform and a confidence per design §1.3.
fn push_data_flow_edge(
    edges: &mut Vec<EdgeWithProvenance>,
    derived: NodeId,
    source: NodeId,
    pe: &PendingDataFlowEdge,
) {
    // Confidence ladder (design §1.3): a direct copy/projection/return with both
    // endpoints resolved is `certain`; a derivation through a recognized transform
    // (arith/composed/branched) is `probable`; a truncated access path drops to
    // `possible`.
    let truncated = pe.cut_markers.contains(&CutMarker::TruncatedAccessPath);
    let confidence = if truncated {
        Confidence::Possible
    } else {
        match pe.transform {
            Transform::Copy | Transform::Projection => Confidence::Certain,
            _ => Confidence::Probable,
        }
    };
    let mut cut_markers = CutMarkers::new();
    for m in &pe.cut_markers {
        cut_markers.insert(*m);
    }
    let record = EdgeRecord {
        id: EdgeId(0),
        src: derived,
        dst: source,
        kind: EdgeKind::DerivesFrom,
        condition: pe.condition,
        confidence,
        tier: Tier::ScopeGraph,
        rule: "derives-from".to_owned(),
        site_id: None,
        stmt_index: None,
        cut_markers,
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
        transform: Some(pe.transform),
    };
    let prov = Provenance::new(pe.span.clone(), "derives-from", Tier::ScopeGraph, String::new());
    edges.push(EdgeWithProvenance {
        edge: record,
        provenance: prov,
    });
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
        transform: None,
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
        transform: None,
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
        dataflow: DataflowOutput::default(),
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
