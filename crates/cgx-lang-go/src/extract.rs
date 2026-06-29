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
use cgx_core::edge::ImplicitKind;
use cgx_core::effect::EffectSet;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::{
    EffectFact, ExportFact, FileCtx, FileFacts, FrontendError, ImportFact, ImportedName, Lang,
    LanguageFrontend, RawRef, RefKind, RelPath, ScopeId, SymbolDef,
};
use smallvec::SmallVec;
use std::collections::BTreeMap;
use tree_sitter::{Node, Parser};

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
            effects: BTreeMap::new(),
            data_flows: BTreeMap::new(),
        }
    }

    fn finish(mut self) -> FileFacts {
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

    /// Record an own-effect set against the enclosing callable (Phase-2 hook;
    /// unused in Phase 1). Inside a body `ctx.fqn_prefix` is the callable's FQN.
    #[allow(dead_code)]
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

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
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

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
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
            self.facts.defs.push(SymbolDef {
                fqn,
                kind: SymbolKind::Type,
                visibility: vis(&name),
                scope: ctx.scope,
                span: self.def_span(spec, "name"),
                line_end: self.end_line(spec),
                is_abstract: is_interface,
                signature: None,
            });
            self.maybe_export(&name, spec);
        }
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
            let (name_path, kind) = self.classify_callee(func);
            if !name_path.is_empty() {
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
                    let (name_path, kind) = self.classify_callee(func);
                    if !name_path.is_empty() {
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

    fn walk_func_literal(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
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
