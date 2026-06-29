//! The Go extractor: a single recursive walk over the `tree-sitter-go` parse
//! tree that builds [`FileFacts`].
//!
//! The walk threads a [`Ctx`] down the tree carrying the current **FQN prefix**
//! (the local package/type path a def lives under), the current **scope id** (the
//! lexical scope a ref lives in), and the current **condition chain** (the ordered
//! set of enclosing guarding constructs whose ADR-03 maximum is the edge condition
//! for a call site).
//!
//! ## Phase-2 hook points (effects / dataflow)
//! The [`Builder`] already carries empty `effects` and `data_flows` accumulators
//! that [`Builder::finish`] flushes into [`FileFacts`]. Phase 2 wires:
//! - **own-effects**: call `self.record_effects(ctx, effects_of_call(&callee))`
//!   from inside [`Builder::walk_call_expression`] (the textual callee is in hand
//!   there), keyed by `ctx.fqn_prefix` (the enclosing callable's FQN inside a
//!   body, set by [`Ctx::enter_body`]).
//! - **SSA dataflow**: add a second, dataflow-only traversal of each function body
//!   in [`Builder::walk_function`]/[`Builder::walk_method`] right after the call
//!   walk, mirroring the Rust adapter's `walk_function_ssa`.

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::edge::ImplicitKind;
use cgx_core::effect::EffectSet;
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_core::transform::Transform;
use cgx_frontend::{
    CutHint, EffectFact, EntrypointHint, ExportFact, FileCtx, FileFacts, FrontendError,
    ImplRelation, ImportFact, ImportedName, Lang, LanguageFrontend, RawRef, RefKind, RelPath,
    RelationKind, ScopeId, SymbolDef,
};
use smallvec::SmallVec;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser};

use crate::effects::effects_of_call;
use crate::module::module_path_for_pkg;

/// The Go language adapter. Stateless; one instance handles every `.go` file.
#[derive(Debug, Default, Clone)]
pub struct GoFrontend;

impl GoFrontend {
    pub fn new() -> Self {
        GoFrontend
    }
}

/// Version of the Go extraction rules; bumping invalidates cached fragments.
const GO_FRAGMENT_VERSION: u32 = 1;

impl LanguageFrontend for GoFrontend {
    fn lang(&self) -> Lang {
        Lang::Go
    }

    fn handles(&self, path: &RelPath) -> bool {
        path.extension().as_deref() == Some("go")
    }

    fn fragment_version(&self) -> u32 {
        GO_FRAGMENT_VERSION
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .map_err(|e| FrontendError::Parser {
                lang: "go".to_string(),
                detail: e.to_string(),
            })?;

        let tree = match parser.parse(src, None) {
            Some(tree) => tree,
            None => return Ok(FileFacts::empty()),
        };

        let module_prefix = module_path_for_pkg(ctx.path.as_str(), ctx.package.as_deref());
        let mut builder = Builder::new(src, ctx.path.as_str());
        builder.package_name = builder.read_package_name(tree.root_node());

        let root_ctx = Ctx::root(module_prefix);
        let mut stmt_index = 0u32;
        let mut cursor = tree.root_node().walk();
        for child in tree.root_node().children(&mut cursor) {
            builder.walk(child, &root_ctx, &mut stmt_index);
        }
        Ok(builder.finish())
    }
}

/// Walk context threaded down the tree.
#[derive(Clone)]
struct Ctx {
    fqn_prefix: String,
    scope: ScopeId,
    conditions: SmallVec<[EdgeCondition; 4]>,
}

impl Ctx {
    fn root(module_prefix: String) -> Self {
        Ctx {
            fqn_prefix: module_prefix,
            scope: ScopeId::ROOT,
            conditions: SmallVec::new(),
        }
    }

    /// Enter a definition body: new FQN prefix + scope, reset conditions.
    fn enter_body(&self, fqn_prefix: String, scope: ScopeId) -> Self {
        Ctx {
            fqn_prefix,
            scope,
            conditions: SmallVec::new(),
        }
    }

    fn with_condition(&self, cond: EdgeCondition) -> Self {
        let mut conditions = self.conditions.clone();
        conditions.push(cond);
        Ctx {
            fqn_prefix: self.fqn_prefix.clone(),
            scope: self.scope,
            conditions,
        }
    }

    fn with_scope(&self, scope: ScopeId) -> Self {
        Ctx {
            fqn_prefix: self.fqn_prefix.clone(),
            scope,
            conditions: self.conditions.clone(),
        }
    }

    fn condition(&self) -> EdgeCondition {
        EdgeCondition::resolve(self.conditions.iter().copied())
    }
}

struct Builder<'a> {
    src: &'a [u8],
    file: String,
    facts: FileFacts,
    /// The `package` clause name (`package main` → `"main"`), used to gate the
    /// `main` entrypoint hint.
    package_name: String,
    /// In-file interface method-name sets, keyed by interface type name; used in
    /// [`Builder::finish`] to emit same-file `Implements` relations.
    interfaces: BTreeMap<String, BTreeSet<String>>,
    /// In-file concrete-type method-name sets (collected from method receivers).
    type_methods: BTreeMap<String, BTreeSet<String>>,
    /// Span of each concrete type's declaration, for the `Implements` relation.
    type_spans: BTreeMap<String, Span>,
    /// Phase-2 hook: accumulated syntactic own-effects keyed by the enclosing
    /// definition's local FQN. Flushed into `facts.effects` in [`Builder::finish`].
    /// Empty in Phase 1.
    effects: BTreeMap<String, EffectSet>,
    /// Phase-2 hook: accumulated intraprocedural dataflow facts keyed by the owning
    /// function's local FQN. Flushed into `facts.data_flows` in [`Builder::finish`].
    /// Empty in Phase 1.
    data_flows: BTreeMap<String, Vec<cgx_frontend::DataFlowFact>>,
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], file: &str) -> Self {
        Builder {
            src,
            file: file.to_string(),
            facts: FileFacts::empty(),
            package_name: String::new(),
            interfaces: BTreeMap::new(),
            type_methods: BTreeMap::new(),
            type_spans: BTreeMap::new(),
            effects: BTreeMap::new(),
            data_flows: BTreeMap::new(),
        }
    }

    fn finish(mut self) -> FileFacts {
        self.emit_in_file_implements();
        for (fqn, set) in self.effects {
            if !set.is_empty() {
                self.facts.effects.push(EffectFact { fqn, effects: set });
            }
        }
        for (_fqn, facts) in self.data_flows {
            self.facts.data_flows.extend(facts);
        }
        self.facts
    }

    /// A concrete type whose method set ⊇ a (non-empty) interface's method set
    /// satisfies that interface in-file. Cross-file satisfaction is the CHA pass's
    /// job; this only emits the relations both of whose ends are declared here.
    fn emit_in_file_implements(&mut self) {
        let mut rels = Vec::new();
        for (ty, methods) in &self.type_methods {
            for (iface, iface_methods) in &self.interfaces {
                if iface_methods.is_empty() || ty == iface {
                    continue;
                }
                if iface_methods.is_subset(methods) {
                    let span = self
                        .type_spans
                        .get(ty)
                        .cloned()
                        .unwrap_or_else(|| Span::new(self.file.clone(), 1, None));
                    rels.push(ImplRelation {
                        kind: RelationKind::Implements,
                        subject: smallvec_one(ty.clone()),
                        object: smallvec_one(iface.clone()),
                        span,
                    });
                }
            }
        }
        self.facts.impl_relations.extend(rels);
    }

    fn read_package_name(&self, root: Node<'_>) -> String {
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() == "package_clause" {
                if let Some(id) = child.named_child(0) {
                    return self.text(id);
                }
            }
        }
        String::new()
    }

    /// Record an own-effect set against the enclosing callable. Inside a body
    /// `ctx.fqn_prefix` is the callable's FQN (set by [`Ctx::enter_body`]).
    fn record_effects(&mut self, ctx: &Ctx, set: EffectSet) {
        if set.is_empty() {
            return;
        }
        self.effects
            .entry(ctx.fqn_prefix.clone())
            .or_default()
            .union_with(set);
    }

    // --- span/text helpers ---

    fn line(&self, node: Node<'_>) -> u32 {
        node.start_position().row as u32 + 1
    }
    fn end_line(&self, node: Node<'_>) -> u32 {
        node.end_position().row as u32 + 1
    }
    fn col(&self, node: Node<'_>) -> u32 {
        node.start_position().column as u32 + 1
    }
    fn span(&self, node: Node<'_>) -> Span {
        Span::new(self.file.clone(), self.line(node), Some(self.col(node)))
    }
    fn text(&self, node: Node<'_>) -> String {
        node.utf8_text(self.src)
            .map(str::to_string)
            .unwrap_or_else(|_| String::from_utf8_lossy(&self.src[node.byte_range()]).into_owned())
    }
    fn field_text(&self, node: Node<'_>, field: &str) -> Option<String> {
        node.child_by_field_name(field).map(|n| self.text(n))
    }

    // --- main dispatch ---

    fn walk(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        match node.kind() {
            "function_declaration" => self.walk_function(node, ctx),
            "method_declaration" => self.walk_method(node, ctx),
            "type_declaration" => self.walk_type_declaration(node, ctx),
            "const_declaration" => self.walk_value_declaration(node, ctx, SymbolKind::Constant),
            "var_declaration" => self.walk_value_declaration(node, ctx, SymbolKind::Variable),
            "import_declaration" => self.walk_import(node, ctx),
            "call_expression" => self.walk_call_expression(node, ctx, stmt_index),
            "go_statement" => self.walk_go(node, ctx, stmt_index),
            "defer_statement" => self.walk_defer(node, ctx, stmt_index),
            "if_statement" => self.walk_if(node, ctx, stmt_index),
            "for_statement" => self.walk_loop(node, ctx, stmt_index),
            "expression_switch_statement" | "type_switch_statement" | "select_statement" => {
                self.walk_switch(node, ctx, stmt_index)
            }
            "func_literal" => self.walk_func_literal(node, ctx, stmt_index),
            _ => self.walk_children(node, ctx, stmt_index),
        }
    }

    fn walk_children(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, ctx, stmt_index);
        }
    }

    // --- definitions ---

    fn walk_function(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Function,
            visibility: vis(&name),
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
        self.maybe_export(&name, node);
        self.record_fn_entrypoints(&name, &fqn);

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
            let fn_fqn = body_ctx.fqn_prefix.clone();
            self.walk_function_ssa(body, &body_ctx, &fn_fqn);
        }
    }

    fn walk_method(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let Some((recv_type, is_pointer)) = self.receiver_type(node) else {
            return;
        };
        let recv_segment = if is_pointer {
            format!("(*{recv_type})")
        } else {
            recv_type.clone()
        };
        let fqn = format!("{}::{}::{}", ctx.fqn_prefix, recv_segment, name);
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Method,
            visibility: vis(&name),
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });

        // The receiver type's method set drives same-file `Implements` detection.
        self.type_methods
            .entry(recv_type)
            .or_default()
            .insert(name.clone());

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
            let fn_fqn = body_ctx.fqn_prefix.clone();
            self.walk_function_ssa(body, &body_ctx, &fn_fqn);
        }
    }

    /// The receiver `(type_name, is_pointer)` of a `method_declaration`: the
    /// receiver `parameter_list` holds one `parameter_declaration` whose `type` is
    /// a `pointer_type` (pointer receiver) or a bare `type_identifier`.
    fn receiver_type(&self, node: Node<'_>) -> Option<(String, bool)> {
        let recv = node.child_by_field_name("receiver")?;
        let mut cursor = recv.walk();
        let decl = recv
            .children(&mut cursor)
            .find(|c| c.kind() == "parameter_declaration")?;
        let ty = decl.child_by_field_name("type")?;
        if ty.kind() == "pointer_type" {
            let inner = ty.named_child(0)?;
            Some((self.text(inner), true))
        } else {
            Some((self.text(ty), false))
        }
    }

    fn walk_type_declaration(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut cursor = node.walk();
        for spec in node.children(&mut cursor) {
            if !matches!(spec.kind(), "type_spec" | "type_alias") {
                continue;
            }
            let Some(name) = self.field_text(spec, "name") else {
                continue;
            };
            let type_node = spec.child_by_field_name("type");
            let is_interface = type_node
                .map(|t| t.kind() == "interface_type")
                .unwrap_or(false);
            let fqn = join(&ctx.fqn_prefix, &name);
            let span = self.def_span(spec, "name");
            self.facts.defs.push(SymbolDef {
                fqn,
                kind: SymbolKind::Type,
                visibility: vis(&name),
                scope: ctx.scope,
                span: span.clone(),
                line_end: self.end_line(spec),
                is_abstract: is_interface,
                signature: None,
            });
            self.maybe_export(&name, spec);
            self.type_spans.entry(name.clone()).or_insert(span);

            if is_interface {
                if let Some(t) = type_node {
                    let methods = self.interface_methods(t);
                    self.interfaces.insert(name, methods);
                }
            }
        }
    }

    /// The method-name set declared by an `interface_type` (`method_elem` members).
    fn interface_methods(&self, iface: Node<'_>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut cursor = iface.walk();
        for m in iface.children(&mut cursor) {
            if m.kind() == "method_elem" {
                if let Some(name) = self.field_text(m, "name") {
                    out.insert(name);
                }
            }
        }
        out
    }

    fn walk_value_declaration(&mut self, node: Node<'_>, ctx: &Ctx, kind: SymbolKind) {
        let mut spec_cursor = node.walk();
        for spec in node.children(&mut spec_cursor) {
            if !matches!(spec.kind(), "const_spec" | "var_spec") {
                continue;
            }
            let mut name_cursor = spec.walk();
            for name_node in spec.children_by_field_name("name", &mut name_cursor) {
                let name = self.text(name_node);
                self.facts.defs.push(SymbolDef {
                    fqn: join(&ctx.fqn_prefix, &name),
                    kind,
                    visibility: vis(&name),
                    scope: ctx.scope,
                    span: self.span(name_node),
                    line_end: self.end_line(spec),
                    is_abstract: false,
                    signature: None,
                });
                self.maybe_export(&name, name_node);
            }
        }
    }

    /// Span anchored at a def's `name` field when present (else the whole node).
    fn def_span(&self, node: Node<'_>, name_field: &str) -> Span {
        node.child_by_field_name(name_field)
            .map(|n| self.span(n))
            .unwrap_or_else(|| self.span(node))
    }

    fn maybe_export(&mut self, name: &str, node: Node<'_>) {
        if is_exported(name) {
            self.facts.exports.push(ExportFact {
                name: name.to_string(),
                alias: None,
                from: None,
                span: self.span(node),
            });
        }
    }

    // --- imports ---

    fn walk_import(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "import_spec" => self.record_import_spec(child, ctx),
                "import_spec_list" => {
                    let mut inner = child.walk();
                    for spec in child.children(&mut inner) {
                        if spec.kind() == "import_spec" {
                            self.record_import_spec(spec, ctx);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn record_import_spec(&mut self, spec: Node<'_>, ctx: &Ctx) {
        let Some(path_node) = spec.child_by_field_name("path") else {
            return;
        };
        let specifier = string_literal_value(&self.text(path_node));
        let last_segment = specifier
            .rsplit('/')
            .next()
            .unwrap_or(&specifier)
            .to_string();

        let mut glob = false;
        let names = match spec.child_by_field_name("name") {
            Some(name_node) => match name_node.kind() {
                // `. "path"` (dot import) pulls every exported name into scope.
                "dot" => {
                    glob = true;
                    Vec::new()
                }
                // `_ "path"` (blank import) binds no name — a side-effect import.
                "blank_identifier" => Vec::new(),
                // `m "path"` (alias).
                _ => {
                    let alias = self.text(name_node);
                    vec![ImportedName {
                        name: last_segment,
                        alias: Some(alias),
                    }]
                }
            },
            None => vec![ImportedName {
                name: last_segment,
                alias: None,
            }],
        };

        // cgo: `import "C"` pulls in a foreign (C) translation unit — every call
        // through it crosses the FFI boundary the graph cannot see into.
        if specifier == "C" {
            self.facts.cut_hints.push(CutHint {
                marker: CutMarker::ViaFfi,
                span: self.span(spec),
                macro_origin: None,
            });
        }

        self.facts.imports.push(ImportFact {
            specifier,
            names,
            glob,
            re_export: false,
            scope: ctx.scope,
            span: self.span(spec),
        });
    }

    // --- references (calls) ---

    fn walk_call_expression(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(func) = node.child_by_field_name("function") {
            self.emit_call_cut_hints(func);
            let (name_path, kind) = self.classify_callee(func);
            if !name_path.is_empty() {
                self.record_effects(ctx, effects_of_call(&name_path.join("::")));
                self.push_ref(name_path, kind, ctx, self.span(node), *stmt_index, None);
                *stmt_index += 1;
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    fn walk_go(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(call) = node.named_child(0) {
            if call.kind() == "call_expression" {
                if let Some(func) = call.child_by_field_name("function") {
                    let (name_path, _) = self.classify_callee(func);
                    if !name_path.is_empty() {
                        // `go f()` launches detached concurrent work; the spawned
                        // callee's own effects also attribute to this body.
                        let mut set = effects_of_call(&name_path.join("::"));
                        set.insert(cgx_core::effect::Effect::Spawns);
                        self.record_effects(ctx, set);
                        self.push_ref(
                            name_path,
                            RefKind::Spawn,
                            ctx,
                            self.span(call),
                            *stmt_index,
                            None,
                        );
                        *stmt_index += 1;
                    }
                }
                self.walk_call_args(call, ctx, stmt_index);
                return;
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    fn walk_defer(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(call) = node.named_child(0) {
            if call.kind() == "call_expression" {
                if let Some(func) = call.child_by_field_name("function") {
                    self.emit_call_cut_hints(func);
                    let (name_path, kind) = self.classify_callee(func);
                    if !name_path.is_empty() {
                        self.record_effects(ctx, effects_of_call(&name_path.join("::")));
                        self.push_ref(
                            name_path,
                            kind,
                            ctx,
                            self.span(call),
                            *stmt_index,
                            Some(ImplicitKind::Defer),
                        );
                        *stmt_index += 1;
                    }
                }
                self.walk_call_args(call, ctx, stmt_index);
                return;
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    fn walk_call_args(&mut self, call: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(args) = call.child_by_field_name("arguments") {
            self.walk(args, ctx, stmt_index);
        }
    }

    fn walk_func_literal(&mut self, node: Node<'_>, ctx: &Ctx, _stmt_index: &mut u32) {
        // An anonymous callable with a stable allocation site (GM-1.1 Lambda); its
        // FQN is synthesized from the enclosing FQN plus the source position.
        let fqn = format!(
            "{}::{{func@{}:{}}}",
            ctx.fqn_prefix,
            self.line(node),
            self.col(node)
        );
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Lambda,
            visibility: Visibility::Private,
            scope: ctx.scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn));
        let inner = ctx.with_scope(scope);
        if let Some(body) = node.child_by_field_name("body") {
            let mut local = 0u32;
            self.walk(body, &inner, &mut local);
        }
    }

    /// Classify a call's `function` operand into its `(name_path, RefKind)`:
    /// a bare `identifier` → `Call`; a `selector_expression` `x.Foo` →
    /// `CallVirtualReceiver` (the resolver decides package-call vs method via
    /// imports / CHA).
    fn classify_callee(&self, func: Node<'_>) -> (SmallVec<[String; 2]>, RefKind) {
        match func.kind() {
            "identifier" => (smallvec_one(self.text(func)), RefKind::Call),
            "selector_expression" => {
                let segs = self.selector_segments(func);
                if segs.is_empty() {
                    (SmallVec::new(), RefKind::Call)
                } else {
                    (segs, RefKind::CallVirtualReceiver)
                }
            }
            "parenthesized_expression" => func
                .named_child(0)
                .map(|inner| self.classify_callee(inner))
                .unwrap_or((SmallVec::new(), RefKind::Call)),
            _ => {
                let seg: String = self
                    .text(func)
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if seg.is_empty() {
                    (SmallVec::new(), RefKind::Call)
                } else {
                    (smallvec_one(seg), RefKind::Call)
                }
            }
        }
    }

    /// The segment path of a `selector_expression` (`a.b.c` → `[a, b, c]`),
    /// recursing through the operand chain.
    fn selector_segments(&self, node: Node<'_>) -> SmallVec<[String; 2]> {
        let mut out = SmallVec::new();
        match node.kind() {
            "selector_expression" => {
                if let Some(operand) = node.child_by_field_name("operand") {
                    out = self.selector_segments(operand);
                }
                if let Some(field) = node.child_by_field_name("field") {
                    out.push(self.text(field));
                }
            }
            "identifier" | "field_identifier" | "type_identifier" | "package_identifier" => {
                out.push(self.text(node));
            }
            _ => {}
        }
        out
    }

    /// `reflect.*` → reflective cut, `plugin.*` → dynamic cut (GM-5.3). Keyed off
    /// the selector's leading package operand.
    fn emit_call_cut_hints(&mut self, func: Node<'_>) {
        if func.kind() != "selector_expression" {
            return;
        }
        let Some(operand) = func.child_by_field_name("operand") else {
            return;
        };
        if operand.kind() != "identifier" {
            return;
        }
        let marker = match self.text(operand).as_str() {
            "reflect" => Some(CutMarker::Reflective),
            "plugin" => Some(CutMarker::Dynamic),
            _ => None,
        };
        if let Some(marker) = marker {
            self.facts.cut_hints.push(CutHint {
                marker,
                span: self.span(func),
                macro_origin: None,
            });
        }
    }

    fn push_ref(
        &mut self,
        name_path: SmallVec<[String; 2]>,
        kind: RefKind,
        ctx: &Ctx,
        span: Span,
        stmt_index: u32,
        implicit: Option<ImplicitKind>,
    ) {
        self.facts.refs.push(RawRef {
            name_path,
            scope: ctx.scope,
            kind,
            edge_condition: ctx.condition(),
            implicit,
            span,
            stmt_index,
            arity: None,
            cut_markers: SmallVec::new(),
        });
    }

    // --- conditions ---

    fn walk_if(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(c) = node.child_by_field_name("condition") {
            self.walk(c, ctx, stmt_index);
        }
        // An `if err != nil { … }` guard is exceptional control flow; any other
        // boolean branch is merely conditional.
        let is_exc = node
            .child_by_field_name("condition")
            .map(|c| self.is_err_nil_guard(c))
            .unwrap_or(false);
        let branch = if is_exc {
            EdgeCondition::Exception
        } else {
            EdgeCondition::Conditional
        };
        let inner = ctx.with_condition(branch);
        if let Some(c) = node.child_by_field_name("consequence") {
            self.walk(c, &inner, stmt_index);
        }
        if let Some(a) = node.child_by_field_name("alternative") {
            self.walk(a, &inner, stmt_index);
        }
    }

    /// Whether a condition is a `<expr> != nil` error guard: a `!=` comparison
    /// against `nil` whose other operand names an `err`-ish binding.
    fn is_err_nil_guard(&self, cond: Node<'_>) -> bool {
        if cond.kind() != "binary_expression" {
            return false;
        }
        let has_nil = ["left", "right"]
            .iter()
            .filter_map(|f| cond.child_by_field_name(f))
            .any(|n| n.kind() == "nil");
        if !has_nil {
            return false;
        }
        let text = self.text(cond);
        text.contains("!=") && text.to_lowercase().contains("err")
    }

    fn walk_loop(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            let inner = ctx.with_condition(EdgeCondition::Loop);
            self.walk(body, &inner, stmt_index);
        }
        // Walk any non-body children (init/condition/range clause) unguarded.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == body {
                continue;
            }
            self.walk(child, ctx, stmt_index);
        }
    }

    fn walk_switch(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let inner = ctx.with_condition(EdgeCondition::Conditional);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            // Case bodies are conditional; the scrutinee/guard is unguarded.
            if matches!(
                child.kind(),
                "expression_case" | "type_case" | "default_case" | "communication_case"
            ) {
                self.walk(child, &inner, stmt_index);
            } else {
                self.walk(child, ctx, stmt_index);
            }
        }
    }

    // --- entrypoints ---

    fn record_fn_entrypoints(&mut self, name: &str, fqn: &str) {
        if name == "main" && self.package_name == "main" {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.to_string(),
                kind: EntrypointKind::Main,
            });
        } else if is_test_func(name) {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.to_string(),
                kind: EntrypointKind::Test,
            });
        }
    }

    // --- intraprocedural SSA dataflow (M3) ---

    /// A second, dataflow-only traversal of a function body that lowers each
    /// production site (short-var / assignment / binary / projection / return)
    /// into a [`DataFlowFact`](cgx_frontend::DataFlowFact). Re-assignment
    /// (`x = a; x = b;`) increments the per-name SSA version so the resolver mints
    /// distinct value nodes (flow-sensitivity). A call result (`r := g(a)`) is an
    /// opaque source: it carries an `OpaqueCall` cut + the callee name + per-arg
    /// access-paths so the interprocedural IFDS pass can ground it (SC3/SC4).
    fn walk_function_ssa(&mut self, body: Node<'_>, ctx: &Ctx, fn_fqn: &str) {
        let mut versions: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        self.ssa_walk(body, ctx, fn_fqn, &mut versions);
    }

    fn ssa_walk(
        &mut self,
        node: Node<'_>,
        ctx: &Ctx,
        fn_fqn: &str,
        versions: &mut std::collections::HashMap<String, u32>,
    ) {
        match node.kind() {
            // `a := b` and `a = b` share the `left`/`right` expression-list shape.
            "short_var_declaration" | "assignment_statement" => {
                let cond = ctx.condition();
                let lefts = self.expr_list_children(node, "left");
                let rights = self.expr_list_children(node, "right");
                if lefts.len() == rights.len() {
                    for (l, r) in lefts.iter().zip(rights.iter()) {
                        if let Some(name) = self.binding_name(*l) {
                            self.ssa_assignment(&name, *r, ctx, fn_fqn, cond, versions);
                        }
                    }
                }
                for r in &rights {
                    self.ssa_walk(*r, ctx, fn_fqn, versions);
                }
            }
            "return_statement" => {
                let cond = ctx.condition();
                for expr in self.return_exprs(node) {
                    self.ssa_return(expr, ctx, fn_fqn, cond, versions);
                    self.ssa_walk(expr, ctx, fn_fqn, versions);
                }
            }
            // Control flow lowers an edge condition exactly like the call walk, so a
            // flow inside an `if`/case/loop body carries the matching condition.
            "if_statement" => {
                if let Some(c) = node.child_by_field_name("condition") {
                    self.ssa_walk(c, ctx, fn_fqn, versions);
                }
                let is_exc = node
                    .child_by_field_name("condition")
                    .map(|c| self.is_err_nil_guard(c))
                    .unwrap_or(false);
                let branch = if is_exc {
                    EdgeCondition::Exception
                } else {
                    EdgeCondition::Conditional
                };
                let inner = ctx.with_condition(branch);
                if let Some(c) = node.child_by_field_name("consequence") {
                    self.ssa_walk(c, &inner, fn_fqn, versions);
                }
                if let Some(a) = node.child_by_field_name("alternative") {
                    self.ssa_walk(a, &inner, fn_fqn, versions);
                }
            }
            "for_statement" => {
                if let Some(body) = node.child_by_field_name("body") {
                    let inner = ctx.with_condition(EdgeCondition::Loop);
                    self.ssa_walk(body, &inner, fn_fqn, versions);
                }
            }
            "expression_switch_statement" | "type_switch_statement" | "select_statement" => {
                let inner = ctx.with_condition(EdgeCondition::Conditional);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if matches!(
                        child.kind(),
                        "expression_case" | "type_case" | "default_case" | "communication_case"
                    ) {
                        self.ssa_walk(child, &inner, fn_fqn, versions);
                    } else {
                        self.ssa_walk(child, ctx, fn_fqn, versions);
                    }
                }
            }
            // `ch <- v` (DF-20): model the channel as an intermediary the value
            // flows into. The matching receive `x := <-ch` links `x ⇝ ch` via the
            // unary-receive operand, so `v ⇝ ch ⇝ x` is recoverable through `ch`.
            "send_statement" => {
                let cond = ctx.condition();
                if let (Some(chan_node), Some(value)) = (
                    node.child_by_field_name("channel"),
                    node.child_by_field_name("value"),
                ) {
                    if let Some(name) = self.binding_name(chan_node) {
                        let version = {
                            let v = versions.entry(name.clone()).or_insert(0);
                            *v += 1;
                            *v
                        };
                        let derived = smallvec_one(name);
                        let span = self.span(value);
                        for source in self.operand_sources(value) {
                            self.push_data_flow(
                                fn_fqn,
                                derived.clone(),
                                version,
                                source,
                                ctx.scope,
                                Transform::Other,
                                cond,
                                SmallVec::new(),
                                None,
                                SmallVec::new(),
                                span.clone(),
                            );
                        }
                    }
                    self.ssa_walk(value, ctx, fn_fqn, versions);
                }
            }
            // A func literal opens a new binding world; SC2 does not descend into
            // closure bodies for SSA (closure-capture dataflow is deferred).
            "func_literal" => {}
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.ssa_walk(child, ctx, fn_fqn, versions);
                }
            }
        }
    }

    /// The expression operands of a `return_statement` (the `expression_list`'s
    /// named children, or a direct expression child for the single-value form).
    fn return_exprs<'t>(&self, node: Node<'t>) -> Vec<Node<'t>> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "return" => {}
                "expression_list" => {
                    let mut inner = child.walk();
                    out.extend(child.children(&mut inner).filter(|c| c.is_named()));
                }
                _ if child.is_named() => out.push(child),
                _ => {}
            }
        }
        out
    }

    /// The named operand nodes of an assignment's `left`/`right` `expression_list`
    /// field (or the bare field node when not wrapped).
    fn expr_list_children<'t>(&self, node: Node<'t>, field: &str) -> Vec<Node<'t>> {
        let Some(list) = node.child_by_field_name(field) else {
            return Vec::new();
        };
        if list.kind() == "expression_list" {
            let mut cursor = list.walk();
            list.children(&mut cursor)
                .filter(|c| c.is_named())
                .collect()
        } else {
            vec![list]
        }
    }

    /// Emit the `DataFlowFact`(s) for `name = <value>`: bump the SSA version of
    /// `name`, classify the RHS into a [`Transform`], and emit one fact per source
    /// operand (`derived --derives-from(transform)--> source`).
    fn ssa_assignment(
        &mut self,
        name: &str,
        value: Node<'_>,
        ctx: &Ctx,
        fn_fqn: &str,
        cond: EdgeCondition,
        versions: &mut std::collections::HashMap<String, u32>,
    ) {
        let version = {
            let v = versions.entry(name.to_string()).or_insert(0);
            *v += 1;
            *v
        };
        let derived: SmallVec<[String; 2]> = smallvec_one(name.to_string());
        let span = self.span(value);
        let (transform, sources, cut) = self.classify_rhs(value);
        let is_opaque_call = cut == Some(CutMarker::OpaqueCall);
        let callee = is_opaque_call
            .then(|| self.opaque_callee_name(value))
            .flatten();
        let args = if is_opaque_call {
            self.opaque_call_arg_paths(value)
        } else {
            SmallVec::new()
        };
        if sources.is_empty() && is_opaque_call {
            let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
            cut_markers.push(CutMarker::OpaqueCall);
            self.push_data_flow(
                fn_fqn,
                derived,
                version,
                SmallVec::new(),
                ctx.scope,
                transform,
                cond,
                cut_markers,
                callee,
                args,
                span,
            );
            return;
        }
        for source in sources {
            let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
            if let Some(c) = cut {
                cut_markers.push(c);
            }
            self.push_data_flow(
                fn_fqn,
                derived.clone(),
                version,
                source,
                ctx.scope,
                transform,
                cond,
                cut_markers,
                callee.clone(),
                SmallVec::new(),
                span.clone(),
            );
        }
    }

    /// Emit the return dataflow fact: `<fn>::return --derives-from(copy)--> <expr>`
    /// for the syntactic operand of a `return`.
    fn ssa_return(
        &mut self,
        value: Node<'_>,
        ctx: &Ctx,
        fn_fqn: &str,
        cond: EdgeCondition,
        versions: &mut std::collections::HashMap<String, u32>,
    ) {
        let version = {
            let v = versions.entry("return".to_string()).or_insert(0);
            *v += 1;
            *v
        };
        let derived: SmallVec<[String; 2]> = {
            let mut v = SmallVec::new();
            v.push(fn_fqn.to_string());
            v.push("return".to_string());
            v
        };
        let span = self.span(value);
        let (_t, sources, cut) = self.classify_rhs(value);
        let is_opaque_call = cut == Some(CutMarker::OpaqueCall);
        let callee = is_opaque_call
            .then(|| self.opaque_callee_name(value))
            .flatten();
        let args = if is_opaque_call {
            self.opaque_call_arg_paths(value)
        } else {
            SmallVec::new()
        };
        if sources.is_empty() && is_opaque_call {
            let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
            cut_markers.push(CutMarker::OpaqueCall);
            self.push_data_flow(
                fn_fqn,
                derived,
                version,
                SmallVec::new(),
                ctx.scope,
                Transform::Copy,
                cond,
                cut_markers,
                callee,
                args,
                span,
            );
            return;
        }
        for source in sources {
            let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
            if let Some(c) = cut {
                cut_markers.push(c);
            }
            self.push_data_flow(
                fn_fqn,
                derived.clone(),
                version,
                source,
                ctx.scope,
                Transform::Copy,
                cond,
                cut_markers,
                callee.clone(),
                SmallVec::new(),
                span.clone(),
            );
        }
    }

    /// Classify a RHS expression into `(transform, source access-paths, cut)`.
    fn classify_rhs(
        &self,
        value: Node<'_>,
    ) -> (Transform, Vec<SmallVec<[String; 2]>>, Option<CutMarker>) {
        match value.kind() {
            "identifier" => (Transform::Copy, vec![smallvec_one(self.text(value))], None),
            "selector_expression" => self.classify_field(value),
            "binary_expression" => {
                let mut sources = Vec::new();
                for field in ["left", "right"] {
                    if let Some(n) = value.child_by_field_name(field) {
                        sources.extend(self.operand_sources(n));
                    }
                }
                (Transform::Arith, sources, None)
            }
            "unary_expression" => {
                let sources = value
                    .child_by_field_name("operand")
                    .map(|n| self.operand_sources(n))
                    .unwrap_or_default();
                // A channel receive (`<-ch`) is a transfer, not arithmetic.
                let transform = if self.unary_operator(value).as_deref() == Some("<-") {
                    Transform::Other
                } else {
                    Transform::Arith
                };
                (transform, sources, None)
            }
            "composite_literal" => (Transform::Composed, self.composed_sources(value), None),
            // A call result is opaque — no intraprocedural source (SC4 grounds it).
            "call_expression" => (Transform::Other, Vec::new(), Some(CutMarker::OpaqueCall)),
            "parenthesized_expression" => value
                .named_child(0)
                .map(|n| self.classify_rhs(n))
                .unwrap_or((Transform::Other, Vec::new(), None)),
            _ => (Transform::Other, Vec::new(), None),
        }
    }

    /// Classify a `selector_expression` RHS: depth-1 (`u.name`) is a clean
    /// `Projection`; depth-2+ (`u.cfg.timeout`) truncates to the base value with a
    /// `TruncatedAccessPath` cut.
    fn classify_field(
        &self,
        value: Node<'_>,
    ) -> (Transform, Vec<SmallVec<[String; 2]>>, Option<CutMarker>) {
        let Some(base) = value.child_by_field_name("operand") else {
            return (Transform::Projection, Vec::new(), None);
        };
        let field = value
            .child_by_field_name("field")
            .map(|f| self.text(f))
            .unwrap_or_default();
        if base.kind() == "identifier" {
            let mut path: SmallVec<[String; 2]> = SmallVec::new();
            path.push(self.text(base));
            path.push(field);
            (Transform::Projection, vec![path], None)
        } else {
            let base_id = self.deepest_base_ident(base);
            let sources = base_id.map(|b| vec![smallvec_one(b)]).unwrap_or_default();
            (
                Transform::Projection,
                sources,
                Some(CutMarker::TruncatedAccessPath),
            )
        }
    }

    /// The deepest base identifier of a nested selector chain (`u.cfg.timeout` →
    /// `u`). `None` if the base is not a plain name.
    fn deepest_base_ident(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "identifier" => Some(self.text(node)),
            "selector_expression" => node
                .child_by_field_name("operand")
                .and_then(|v| self.deepest_base_ident(v)),
            _ => None,
        }
    }

    /// The operator token text of a `unary_expression` (`-x` → `-`, `<-ch` → `<-`).
    fn unary_operator(&self, node: Node<'_>) -> Option<String> {
        node.child_by_field_name("operator").map(|n| self.text(n))
    }

    /// The source bindings of one operand of an arith/composed expression.
    fn operand_sources(&self, node: Node<'_>) -> Vec<SmallVec<[String; 2]>> {
        match node.kind() {
            "identifier" => vec![smallvec_one(self.text(node))],
            "selector_expression" => {
                let (_t, sources, _c) = self.classify_field(node);
                sources
            }
            "binary_expression" => {
                let mut out = Vec::new();
                for field in ["left", "right"] {
                    if let Some(n) = node.child_by_field_name(field) {
                        out.extend(self.operand_sources(n));
                    }
                }
                out
            }
            "parenthesized_expression" => node
                .named_child(0)
                .map(|n| self.operand_sources(n))
                .unwrap_or_default(),
            "unary_expression" => node
                .child_by_field_name("operand")
                .map(|n| self.operand_sources(n))
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Source bindings assembled by a `composite_literal` (`U{name: a}` → `[a]`):
    /// every named identifier operand, not descending into nested calls/literals.
    fn composed_sources(&self, node: Node<'_>) -> Vec<SmallVec<[String; 2]>> {
        let mut out: Vec<String> = Vec::new();
        self.collect_idents(node, true, &mut out);
        out.into_iter().map(smallvec_one).collect()
    }

    fn collect_idents(&self, node: Node<'_>, is_root: bool, out: &mut Vec<String>) {
        if !is_root && matches!(node.kind(), "func_literal" | "call_expression") {
            return;
        }
        if node.kind() == "identifier" {
            let t = self.text(node);
            if !out.contains(&t) {
                out.push(t);
            }
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_idents(child, false, out);
        }
    }

    /// The bound name of an assignment LHS, when it is a plain identifier
    /// (`x := …`, `x = …`). Blank `_` and field/index targets are not versioned.
    fn binding_name(&self, lhs: Node<'_>) -> Option<String> {
        match lhs.kind() {
            "identifier" => Some(self.text(lhs)),
            _ => None,
        }
    }

    /// Push one `DataFlowFact` into the per-function accumulator (flushed in
    /// [`Builder::finish`]).
    #[allow(clippy::too_many_arguments)]
    fn push_data_flow(
        &mut self,
        fn_fqn: &str,
        derived: SmallVec<[String; 2]>,
        derived_version: u32,
        source: SmallVec<[String; 2]>,
        scope: ScopeId,
        transform: Transform,
        edge_condition: EdgeCondition,
        cut_markers: SmallVec<[CutMarker; 1]>,
        callee_fqn: Option<String>,
        args: SmallVec<[SmallVec<[String; 2]>; 4]>,
        span: Span,
    ) {
        self.data_flows
            .entry(fn_fqn.to_string())
            .or_default()
            .push(cgx_frontend::DataFlowFact {
                derived,
                source,
                derived_version,
                scope,
                transform,
                edge_condition,
                cut_markers,
                callee_fqn,
                args,
                span,
            });
    }

    /// Per-positional-argument source access-paths of an opaque-call RHS
    /// (`g(a, b.x, 1)` → `[["a"], ["b","x"], []]`): each named argument reduced
    /// through `operand_sources`, taking its first access-path; a literal/nested
    /// call contributes an empty inner vec to keep positional alignment.
    fn opaque_call_arg_paths(&self, value: Node<'_>) -> SmallVec<[SmallVec<[String; 2]>; 4]> {
        let call = match value.kind() {
            "parenthesized_expression" => {
                return value
                    .named_child(0)
                    .map(|n| self.opaque_call_arg_paths(n))
                    .unwrap_or_default();
            }
            "call_expression" => value,
            _ => return SmallVec::new(),
        };
        let Some(args) = call.child_by_field_name("arguments") else {
            return SmallVec::new();
        };
        let mut out: SmallVec<[SmallVec<[String; 2]>; 4]> = SmallVec::new();
        let mut cursor = args.walk();
        for arg in args.children(&mut cursor) {
            if !arg.is_named() {
                continue;
            }
            let path = self
                .operand_sources(arg)
                .into_iter()
                .next()
                .unwrap_or_default();
            out.push(path);
        }
        out
    }

    /// The syntactic callee name path of a call-result RHS, `::`-joined (`g`,
    /// `pkg::Foo`). `None` for a virtual/method call on a receiver (recorded as a
    /// wildcard summary dep by the resolver instead).
    fn opaque_callee_name(&self, value: Node<'_>) -> Option<String> {
        let call = match value.kind() {
            "parenthesized_expression" => {
                return value
                    .named_child(0)
                    .and_then(|n| self.opaque_callee_name(n));
            }
            "call_expression" => value,
            _ => return None,
        };
        let func = call.child_by_field_name("function")?;
        let (name_path, kind) = self.classify_callee(func);
        if name_path.is_empty() || kind == RefKind::CallVirtualReceiver {
            None
        } else {
            Some(name_path.join("::"))
        }
    }
}

// === free functions ===

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}::{name}")
    }
}

fn smallvec_one(s: String) -> SmallVec<[String; 2]> {
    let mut v = SmallVec::new();
    v.push(s);
    v
}

/// Go visibility: an exported (Capitalized) identifier is `Public`; anything else
/// is `Package`-scoped.
fn vis(name: &str) -> Visibility {
    if is_exported(name) {
        Visibility::Public
    } else {
        Visibility::Package
    }
}

fn is_exported(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

/// Go test-binary entrypoints: `TestXxx`, `BenchmarkXxx`, `ExampleXxx` (where the
/// suffix does not start with a lowercase letter, per `go test` conventions).
fn is_test_func(name: &str) -> bool {
    for prefix in ["Test", "Benchmark", "Example"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if rest.is_empty() || rest.chars().next().is_some_and(|c| !c.is_lowercase()) {
                return true;
            }
        }
    }
    false
}

/// The value of a Go string literal: strip the surrounding quotes/backticks.
fn string_literal_value(literal: &str) -> String {
    let trimmed = literal.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'`' && last == b'`') {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}
