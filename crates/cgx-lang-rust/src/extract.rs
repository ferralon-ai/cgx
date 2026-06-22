//! The Rust extractor: a single recursive walk over the `tree-sitter-rust` parse
//! tree that builds [`FileFacts`].
//!
//! The walk threads a [`Ctx`] down the tree carrying:
//! - the current **FQN prefix** (the local module/type/fn path a def lives under),
//! - the current **scope id** (the lexical scope a ref lives in),
//! - the current **condition chain** (the ordered set of enclosing guarding
//!   constructs whose ADR-03 maximum is the edge condition for a call site), and
//! - a **method flag** (whether direct `function_item`s here are methods).
//!
//! Conditions are computed positionally: a call inherits `conditional` only when
//! it is in the *body* of an `if`/`match` arm (not the predicate/scrutinee),
//! `loop` only inside a loop *body*, `exception` inside an `Err`-arm body or as
//! the `?`-twin of a call, and `panic` at the panicking macro/method site itself.

use crate::effects::effects_of_call;
use crate::module::module_path_for_pkg;
use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::edge::ImplicitKind;
use cgx_core::effect::{Effect, EffectSet};
use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_core::signature::{Param, Signature};
use cgx_frontend::{
    CutHint, EffectFact, EntrypointHint, EntrypointKind, ExportFact, FileCtx, FileFacts,
    FrontendError, ImplRelation, ImportFact, ImportedName, Lang, LanguageFrontend, RawRef, RefKind,
    RelationKind, RelPath, ScopeId, SymbolDef,
};
use smallvec::SmallVec;
use tree_sitter::{Node, Parser};

/// The Rust language adapter (architecture §5, §6). Stateless; one instance
/// handles every `.rs` file.
#[derive(Debug, Default, Clone)]
pub struct RustFrontend;

impl RustFrontend {
    pub fn new() -> Self {
        RustFrontend
    }
}

/// Version of the Rust extraction rules; bumping invalidates cached fragments
/// (architecture §3 `frontend_version`). v3: FQN crate root now derives from the
/// owning `Cargo.toml` package for `src/`-at-root / no-`src` layouts (previously
/// the hardcoded `rust_sample` default), so v2 fragments may carry stale roots.
const RUST_FRAGMENT_VERSION: u32 = 3;

impl LanguageFrontend for RustFrontend {
    fn lang(&self) -> Lang {
        Lang::Rust
    }

    fn handles(&self, path: &RelPath) -> bool {
        path.extension().as_deref() == Some("rs")
    }

    fn fragment_version(&self) -> u32 {
        RUST_FRAGMENT_VERSION
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|e| FrontendError::Parser {
                lang: "rust".to_string(),
                detail: e.to_string(),
            })?;

        let tree = match parser.parse(src, None) {
            Some(tree) => tree,
            None => return Ok(FileFacts::empty()),
        };

        let module_prefix = module_path_for_pkg(ctx.path.as_str(), ctx.package.as_deref());
        let mut builder = Builder::new(src, ctx.path.as_str());
        // First pass over top-level items collects extern fn names so calls to
        // them can be marked via-FFI regardless of declaration order.
        builder.collect_extern_fns(tree.root_node());

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
    /// Whether a direct `function_item` here is a method (inside impl/trait).
    in_member_list: bool,
}

impl Ctx {
    fn root(module_prefix: String) -> Self {
        Ctx {
            fqn_prefix: module_prefix,
            scope: ScopeId::ROOT,
            conditions: SmallVec::new(),
            in_member_list: false,
        }
    }

    /// Enter a definition body: new FQN prefix + scope, reset conditions.
    fn enter_body(&self, fqn_prefix: String, scope: ScopeId) -> Self {
        Ctx {
            fqn_prefix,
            scope,
            conditions: SmallVec::new(),
            in_member_list: false,
        }
    }

    /// Enter an impl/trait member list: members are methods.
    fn enter_members(&self, fqn_prefix: String, scope: ScopeId) -> Self {
        Ctx {
            fqn_prefix,
            scope,
            conditions: SmallVec::new(),
            in_member_list: true,
        }
    }

    fn with_condition(&self, cond: EdgeCondition) -> Self {
        let mut conditions = self.conditions.clone();
        conditions.push(cond);
        Ctx {
            fqn_prefix: self.fqn_prefix.clone(),
            scope: self.scope,
            conditions,
            in_member_list: false,
        }
    }

    fn with_scope(&self, scope: ScopeId) -> Self {
        Ctx {
            fqn_prefix: self.fqn_prefix.clone(),
            scope,
            conditions: self.conditions.clone(),
            in_member_list: false,
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
    extern_fns: Vec<String>,
    /// Accumulated syntactic own-effects (GM-12 Phase 1), keyed by the enclosing
    /// definition's local FQN. Flushed into `facts.effects` in [`Builder::finish`].
    /// A `BTreeMap` keeps the flush order deterministic.
    effects: std::collections::BTreeMap<String, EffectSet>,
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], file: &str) -> Self {
        Builder {
            src,
            file: file.to_string(),
            facts: FileFacts::empty(),
            extern_fns: Vec::new(),
            effects: std::collections::BTreeMap::new(),
        }
    }

    fn finish(mut self) -> FileFacts {
        for (fqn, set) in self.effects {
            if !set.is_empty() {
                self.facts.effects.push(EffectFact { fqn, effects: set });
            }
        }
        self.facts
    }

    /// Record detected own-effects against the function whose body encloses the
    /// current site. Inside a function/method body, `ctx.fqn_prefix` is that
    /// callable's FQN (set by [`Ctx::enter_body`]); at module level there is no
    /// enclosing callable and the keyed FQN will not match any callable def, so
    /// the resolver drops it harmlessly.
    fn record_effects(&mut self, ctx: &Ctx, set: EffectSet) {
        if set.is_empty() {
            return;
        }
        self.effects
            .entry(ctx.fqn_prefix.clone())
            .or_default()
            .union_with(set);
    }

    /// Pre-scan for `extern "C" { fn name; }` declarations so call sites to them
    /// can be flagged via-FFI no matter the order they appear.
    fn collect_extern_fns(&mut self, root: Node<'_>) {
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if node.kind() == "foreign_mod_item" {
                if let Some(body) = node.child_by_field_name("body") {
                    let mut c = body.walk();
                    for child in body.children(&mut c) {
                        if child.kind() == "function_signature_item" {
                            if let Some(name) = self.field_text(child, "name") {
                                self.extern_fns.push(name);
                            }
                        }
                    }
                }
            }
            let mut c = node.walk();
            for child in node.children(&mut c) {
                stack.push(child);
            }
        }
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
            "function_item" => self.walk_function(node, ctx, false),
            "function_signature_item" => self.walk_function(node, ctx, true),
            "struct_item" | "enum_item" | "union_item" | "type_item" => self.walk_type(node, ctx),
            "trait_item" => self.walk_trait(node, ctx),
            "impl_item" => self.walk_impl(node, ctx),
            "mod_item" => self.walk_mod(node, ctx, stmt_index),
            "const_item" | "static_item" => self.record_value_def(node, ctx),
            "macro_definition" => self.record_macro_def(node, ctx),
            "use_declaration" => self.record_use(node, ctx),
            "foreign_mod_item" => self.walk_foreign_mod(node, ctx),
            "call_expression" => self.walk_call_expression(node, ctx, stmt_index),
            "struct_expression" => self.walk_struct_expression(node, ctx, stmt_index),
            "macro_invocation" => self.walk_macro_invocation(node, ctx, stmt_index),
            "if_expression" | "if_let_expression" => self.walk_if(node, ctx, stmt_index),
            "match_expression" => self.walk_match(node, ctx, stmt_index),
            "for_expression" | "while_expression" | "loop_expression" => {
                self.walk_loop(node, ctx, stmt_index)
            }
            "try_expression" => self.walk_try(node, ctx, stmt_index),
            "await_expression" => self.walk_await(node, ctx, stmt_index),
            "closure_expression" => self.walk_closure(node, ctx, stmt_index),
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

    fn walk_function(&mut self, node: Node<'_>, ctx: &Ctx, is_signature: bool) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        let kind = if ctx.in_member_list {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        let visibility = self.visibility_of(node);
        let signature = self.signature_of(node);

        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind,
            visibility,
            scope: ctx.scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: is_signature,
            signature,
        });

        self.record_fn_entrypoints(node, &name, &fqn);

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
        }
    }

    fn walk_type(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        self.facts.defs.push(SymbolDef {
            fqn,
            kind: SymbolKind::Type,
            visibility: self.visibility_of(node),
            scope: ctx.scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
        self.emit_derive_cut_hints(node);
    }

    fn walk_trait(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Type,
            visibility: self.visibility_of(node),
            scope: ctx.scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: true,
            signature: None,
        });

        // Supertrait bounds (`trait Sub: Super + Other`) → Inherits relations
        // (GM-2.2). The `bounds` field is a `trait_bounds` node whose named
        // children are the bound `_type`s (and lifetimes, which we skip).
        if let Some(bounds) = node.child_by_field_name("bounds") {
            let mut cursor = bounds.walk();
            for bound in bounds.children(&mut cursor) {
                if !bound.is_named() || bound.kind() == "lifetime" {
                    continue;
                }
                let super_name = self.type_name(bound);
                if super_name.is_empty() {
                    continue;
                }
                self.facts.impl_relations.push(ImplRelation {
                    kind: RelationKind::Inherits,
                    subject: smallvec_one(name.clone()),
                    object: smallvec_one(super_name),
                    span: self.span(node),
                });
            }
        }

        if let Some(body) = node.child_by_field_name("body") {
            let member_ctx = ctx.enter_members(fqn, scope);
            self.walk_member_list(body, &member_ctx);
        }
    }

    fn walk_impl(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        let type_name = self.type_name(type_node);
        let fqn = join(&ctx.fqn_prefix, &type_name);

        // The `trait` field is present on a trait impl (`impl Trait for Type`),
        // absent on an inherent impl (`impl Type`). When present, record the
        // Implements lattice relation (GM-2.2) and, per member, an Overrides
        // relation against the same-named trait method.
        let trait_name = node
            .child_by_field_name("trait")
            .map(|t| self.type_name(t));
        if let Some(trait_name) = &trait_name {
            self.facts.impl_relations.push(ImplRelation {
                kind: RelationKind::Implements,
                subject: smallvec_one(type_name.clone()),
                object: smallvec_one(trait_name.clone()),
                span: self.span(node),
            });
        }

        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        let member_ctx = ctx.enter_members(fqn.clone(), scope);
        if let Some(body) = node.child_by_field_name("body") {
            if let Some(trait_name) = &trait_name {
                self.record_impl_overrides(body, &fqn, trait_name, node);
            }
            self.walk_member_list(body, &member_ctx);
        }
    }

    /// For a trait impl, emit an `Overrides` relation for each method the impl
    /// defines, keyed to the same-named trait method. The resolver matches the
    /// trait-method short name against the actual trait member node.
    fn record_impl_overrides(
        &mut self,
        body: Node<'_>,
        impl_fqn: &str,
        trait_name: &str,
        impl_node: Node<'_>,
    ) {
        let span = self.span(impl_node);
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if !matches!(child.kind(), "function_item" | "function_signature_item") {
                continue;
            }
            let Some(method) = self.field_text(child, "name") else {
                continue;
            };
            self.facts.impl_relations.push(ImplRelation {
                kind: RelationKind::Overrides,
                subject: name_path_two(impl_fqn, &method),
                object: name_path_two(trait_name, &method),
                span: span.clone(),
            });
        }
    }

    fn walk_member_list(&mut self, body: Node<'_>, ctx: &Ctx) {
        let mut cursor = body.walk();
        let mut stmt_index = 0u32;
        for child in body.children(&mut cursor) {
            self.walk(child, ctx, &mut stmt_index);
        }
    }

    fn walk_mod(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Module,
            visibility: self.visibility_of(node),
            scope: ctx.scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
        if let Some(body) = node.child_by_field_name("body") {
            let inner = ctx.enter_body(fqn, scope);
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                self.walk(child, &inner, stmt_index);
            }
        }
    }

    fn record_value_def(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        self.facts.defs.push(SymbolDef {
            fqn: join(&ctx.fqn_prefix, &name),
            kind: SymbolKind::Constant,
            visibility: self.visibility_of(node),
            scope: ctx.scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
    }

    fn record_macro_def(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        self.facts.defs.push(SymbolDef {
            fqn: join(&ctx.fqn_prefix, &name),
            kind: SymbolKind::Macro,
            visibility: Visibility::Internal,
            scope: ctx.scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
    }

    fn walk_foreign_mod(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "function_signature_item" {
                if let Some(name) = self.field_text(child, "name") {
                    self.facts.defs.push(SymbolDef {
                        fqn: join(&ctx.fqn_prefix, &name),
                        kind: SymbolKind::Function,
                        visibility: self.visibility_of(child),
                        scope: ctx.scope,
                        span: self.span(child),
                        line_end: self.end_line(child),
                        is_abstract: true,
                        signature: self.signature_of(child),
                    });
                }
            }
        }
    }

    // --- imports / re-exports ---

    fn record_use(&mut self, node: Node<'_>, ctx: &Ctx) {
        let re_export = self.has_pub(node);
        let Some(arg) = node.child_by_field_name("argument") else {
            return;
        };
        let mut imports = Vec::new();
        self.flatten_use(arg, &[], re_export, ctx, &mut imports);
        for imp in imports {
            if imp.re_export {
                for n in &imp.names {
                    let exported = n.alias.clone().unwrap_or_else(|| n.name.clone());
                    self.facts.exports.push(ExportFact {
                        name: exported,
                        alias: n.alias.clone(),
                        from: Some(imp.specifier.clone()),
                        span: imp.span.clone(),
                    });
                }
            }
            self.facts.imports.push(imp);
        }
    }

    fn flatten_use(
        &self,
        node: Node<'_>,
        prefix: &[String],
        re_export: bool,
        ctx: &Ctx,
        out: &mut Vec<ImportFact>,
    ) {
        match node.kind() {
            "use_as_clause" => {
                let path = node.child_by_field_name("path");
                let alias = self.field_text(node, "alias");
                if let Some(path) = path {
                    let mut segs = prefix.to_vec();
                    self.collect_path_segments(path, &mut segs);
                    if let Some((name, specifier)) = split_specifier(&segs) {
                        out.push(ImportFact {
                            specifier,
                            names: vec![ImportedName { name, alias }],
                            glob: false,
                            re_export,
                            scope: ctx.scope,
                            span: self.span(node),
                        });
                    }
                }
            }
            "scoped_use_list" => {
                let mut segs = prefix.to_vec();
                if let Some(path) = node.child_by_field_name("path") {
                    self.collect_path_segments(path, &mut segs);
                }
                if let Some(list) = node.child_by_field_name("list") {
                    let mut cursor = list.walk();
                    for item in list.children(&mut cursor) {
                        if item.is_named() {
                            self.flatten_use(item, &segs, re_export, ctx, out);
                        }
                    }
                }
            }
            "use_list" => {
                let mut cursor = node.walk();
                for item in node.children(&mut cursor) {
                    if item.is_named() {
                        self.flatten_use(item, prefix, re_export, ctx, out);
                    }
                }
            }
            "use_wildcard" => {
                let mut segs = prefix.to_vec();
                let mut cursor = node.walk();
                for c in node.children(&mut cursor) {
                    if c.is_named() {
                        self.collect_path_segments(c, &mut segs);
                    }
                }
                out.push(ImportFact {
                    specifier: segs.join("::"),
                    names: Vec::new(),
                    glob: true,
                    re_export,
                    scope: ctx.scope,
                    span: self.span(node),
                });
            }
            "scoped_identifier" | "identifier" | "type_identifier" | "crate" | "self" | "super" => {
                let mut segs = prefix.to_vec();
                self.collect_path_segments(node, &mut segs);
                if let Some((name, specifier)) = split_specifier(&segs) {
                    out.push(ImportFact {
                        specifier,
                        names: vec![ImportedName { name, alias: None }],
                        glob: false,
                        re_export,
                        scope: ctx.scope,
                        span: self.span(node),
                    });
                }
            }
            _ => {}
        }
    }

    fn collect_path_segments(&self, node: Node<'_>, out: &mut Vec<String>) {
        match node.kind() {
            "scoped_identifier" => {
                if let Some(path) = node.child_by_field_name("path") {
                    self.collect_path_segments(path, out);
                }
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(self.text(name));
                }
            }
            "identifier" | "type_identifier" | "crate" | "self" | "super" => {
                out.push(self.text(node));
            }
            _ => {
                for seg in self.text(node).split("::") {
                    let seg = seg.trim();
                    if !seg.is_empty() {
                        out.push(seg.to_string());
                    }
                }
            }
        }
    }

    // --- references (calls) ---

    fn walk_call_expression(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(func) = node.child_by_field_name("function") {
            let func_text = self.text(func);
            // GM-12 own-effect detection: the textual callee implies effects
            // regardless of which call form (plain / constructor / method) follows.
            self.record_effects(ctx, effects_of_call(&func_text));
            if is_spawn_path(&func_text) {
                self.record_spawn(node, ctx, stmt_index);
                self.walk_spawn_children(node, ctx, stmt_index);
                return;
            }

            let arity = node.child_by_field_name("arguments").map(count_args);
            // A constructor call form (`T::new(..)`, `Box::new(x)`, `T::default()`,
            // …) is an instantiation, not a plain call: emit an Instantiate ref
            // naming the constructed type so RTA (P5) can see the type as live.
            if let Some(target) = self.constructor_target(func, node) {
                self.push_ref(
                    target,
                    RefKind::Instantiate,
                    ctx.condition(),
                    None,
                    node,
                    ctx,
                    *stmt_index,
                    arity,
                );
                *stmt_index += 1;
                self.walk_children(node, ctx, stmt_index);
                return;
            }
            let (name_path, kind) = self.classify_callee(func);
            if !name_path.is_empty() {
                self.push_ref(
                    name_path,
                    kind,
                    ctx.condition(),
                    None,
                    node,
                    ctx,
                    *stmt_index,
                    arity,
                );
                *stmt_index += 1;
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    /// A struct/enum-variant literal `T { .. }` (or `Mod::T { .. }`) is an
    /// instantiation of `T` (GM-2.2 Instantiates). The `name` field is the type
    /// being constructed; we record its type name as the instantiation target.
    fn walk_struct_expression(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(name_node) = node.child_by_field_name("name") {
            let type_name = self.type_name(name_node);
            if !type_name.is_empty() {
                self.push_ref(
                    smallvec_one(type_name),
                    RefKind::Instantiate,
                    ctx.condition(),
                    None,
                    node,
                    ctx,
                    *stmt_index,
                    None,
                );
                *stmt_index += 1;
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    /// If `func` is a constructor call form, return the type-name path it
    /// instantiates (for an Instantiate ref), else `None`:
    ///   `T::new`/`T::with_capacity`/`T::default` → `T`
    ///   `Box::new(x)`/`Rc::new(x)`/`Arc::new(x)`  → inner type of `x` when nameable
    /// `Default::default()` and `T::new` where `T` is a wrapper with a nameable
    /// inner argument are handled via the wrapper rule. A non-constructor call →
    /// `None` (falls through to plain-call classification).
    fn constructor_target(
        &self,
        func: Node<'_>,
        call: Node<'_>,
    ) -> Option<SmallVec<[String; 2]>> {
        let func = if func.kind() == "generic_function" {
            func.child_by_field_name("function")?
        } else {
            func
        };
        if func.kind() != "scoped_identifier" {
            return None;
        }
        let method = func.child_by_field_name("name").map(|n| self.text(n))?;
        let path = func.child_by_field_name("path")?;
        let type_name = self.type_name(path);
        if type_name.is_empty() {
            return None;
        }
        match method.as_str() {
            "default" if type_name == "Default" => {
                // `Default::default()` has no syntactically nameable concrete
                // type; the type is inferred. Skip (treated as a plain call).
                None
            }
            "new" | "with_capacity" | "default" => {
                if is_smart_pointer(&type_name) {
                    // `Box::new(x)` etc. construct the *inner* type, not the
                    // wrapper; record the inner type when it is syntactically
                    // nameable, else skip (the inner expr's own walk may still
                    // emit its Instantiate).
                    self.smart_pointer_inner_type(call)
                } else {
                    Some(smallvec_one(type_name))
                }
            }
            _ => None,
        }
    }

    /// The inner type constructed by a smart-pointer wrapper call's first arg,
    /// when syntactically nameable: `Box::new(Foo { .. })` / `Rc::new(Foo::new())`
    /// → `Foo`. Returns `None` for opaque arguments (a bare variable, a literal).
    fn smart_pointer_inner_type(&self, call: Node<'_>) -> Option<SmallVec<[String; 2]>> {
        let args = call.child_by_field_name("arguments")?;
        let mut cursor = args.walk();
        let first = args.children(&mut cursor).find(|c| c.is_named())?;
        match first.kind() {
            "struct_expression" => {
                let name_node = first.child_by_field_name("name")?;
                let t = self.type_name(name_node);
                (!t.is_empty()).then(|| smallvec_one(t))
            }
            "call_expression" => {
                let inner_func = first.child_by_field_name("function")?;
                self.constructor_target(inner_func, first)
            }
            _ => None,
        }
    }

    fn classify_callee(&self, func: Node<'_>) -> (SmallVec<[String; 2]>, RefKind) {
        match func.kind() {
            "identifier" => (smallvec_one(self.text(func)), RefKind::Call),
            "scoped_identifier" => {
                let mut segs = Vec::new();
                self.collect_path_segments(func, &mut segs);
                (segs.into_iter().collect(), RefKind::Call)
            }
            "field_expression" => {
                let method = func
                    .child_by_field_name("field")
                    .map(|f| self.text(f))
                    .unwrap_or_default();
                if method.is_empty() {
                    (SmallVec::new(), RefKind::Call)
                } else {
                    (smallvec_one(method), RefKind::CallVirtualReceiver)
                }
            }
            "generic_function" => func
                .child_by_field_name("function")
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

    fn walk_macro_invocation(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let macro_name = node
            .child_by_field_name("macro")
            .map(|m| self.text(m))
            .unwrap_or_default();
        if is_panic_macro(&macro_name) {
            self.push_ref(
                panic_target(),
                RefKind::Call,
                EdgeCondition::Panic,
                None,
                node,
                ctx,
                *stmt_index,
                None,
            );
            *stmt_index += 1;
        }
        self.walk_children(node, ctx, stmt_index);
    }

    // --- conditions ---

    fn walk_if(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(c) = node.child_by_field_name("condition") {
            self.walk(c, ctx, stmt_index);
        }
        let inner = ctx.with_condition(EdgeCondition::Conditional);
        if let Some(c) = node.child_by_field_name("consequence") {
            self.walk(c, &inner, stmt_index);
        }
        if let Some(a) = node.child_by_field_name("alternative") {
            self.walk(a, &inner, stmt_index);
        }
    }

    fn walk_match(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(value) = node.child_by_field_name("value") {
            self.walk(value, ctx, stmt_index);
        }
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut cursor = body.walk();
        for arm in body.children(&mut cursor) {
            if arm.kind() != "match_arm" {
                continue;
            }
            let is_err = arm
                .child_by_field_name("pattern")
                .map(|p| self.pattern_is_err(p))
                .unwrap_or(false);
            let arm_cond = if is_err {
                EdgeCondition::Exception
            } else {
                EdgeCondition::Conditional
            };
            let inner = ctx.with_condition(arm_cond);
            if let Some(value) = arm.child_by_field_name("value") {
                self.walk(value, &inner, stmt_index);
            }
        }
    }

    fn walk_loop(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if node.kind() == "for_expression" {
            if let Some(value) = node.child_by_field_name("value") {
                self.walk(value, ctx, stmt_index);
            }
            self.push_ref(
                iterator_next_target(),
                RefKind::Call,
                EdgeCondition::Loop,
                Some(ImplicitKind::Iterator),
                node,
                ctx,
                *stmt_index,
                None,
            );
            *stmt_index += 1;
        } else if let Some(cond) = node.child_by_field_name("condition") {
            self.walk(cond, ctx, stmt_index);
        }
        if let Some(body) = node.child_by_field_name("body") {
            let inner = ctx.with_condition(EdgeCondition::Loop);
            self.walk(body, &inner, stmt_index);
        }
    }

    fn walk_try(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let Some(operand) = node.named_child(0) else {
            return self.walk_children(node, ctx, stmt_index);
        };
        // The `always` ref (and nested calls) come from a normal walk.
        self.walk(operand, ctx, stmt_index);
        // The exception twin for the operand's top-level call.
        if let Some((name_path, kind)) = self.top_level_call_of(operand) {
            self.push_ref(
                name_path,
                kind,
                EdgeCondition::Exception,
                None,
                operand,
                ctx,
                *stmt_index,
                None,
            );
            *stmt_index += 1;
        }
    }

    fn walk_await(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let Some(inner) = node.named_child(0) else {
            return self.walk_children(node, ctx, stmt_index);
        };
        if inner.kind() == "call_expression" {
            if let Some(func) = inner.child_by_field_name("function") {
                let (name_path, _) = self.classify_callee(func);
                if !name_path.is_empty() {
                    let arity = inner.child_by_field_name("arguments").map(count_args);
                    self.push_ref(
                        name_path,
                        RefKind::CallAsync,
                        ctx.condition(),
                        None,
                        inner,
                        ctx,
                        *stmt_index,
                        arity,
                    );
                    *stmt_index += 1;
                }
            }
            if let Some(args) = inner.child_by_field_name("arguments") {
                self.walk(args, ctx, stmt_index);
            }
        } else {
            self.walk(inner, ctx, stmt_index);
        }
    }

    fn walk_closure(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        // A closure is an anonymous callable with a stable allocation site
        // (GM-1.1 Lambda). Its FQN is synthesized from the enclosing FQN plus the
        // source position, so it is deterministic and unique per closure.
        let fqn = format!(
            "{}::{{closure@{}:{}}}",
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
            signature: self.closure_signature(node),
        });
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn));
        let inner = ctx.with_scope(scope);
        if let Some(body) = node.child_by_field_name("body") {
            self.walk(body, &inner, stmt_index);
        } else {
            self.walk_children(node, &inner, stmt_index);
        }
    }

    // --- spawn ---

    fn record_spawn(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        // The spawn site is both a Spawns *edge* (recorded below) and a `spawns`
        // own-effect *label* on the enclosing function (GM-12).
        self.record_effects(ctx, EffectSet::single(Effect::Spawns));
        let Some(args) = node.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = args.walk();
        for arg in args.children(&mut cursor) {
            if !arg.is_named() {
                continue;
            }
            if let Some((name_path, _)) = self.top_level_call_of(arg) {
                self.push_ref(
                    name_path,
                    RefKind::Spawn,
                    ctx.condition(),
                    None,
                    node,
                    ctx,
                    *stmt_index,
                    None,
                );
                *stmt_index += 1;
            }
        }
    }

    fn walk_spawn_children(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let Some(args) = node.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = args.walk();
        for arg in args.children(&mut cursor) {
            if !arg.is_named() {
                continue;
            }
            if arg.kind() == "call_expression" {
                if let Some(inner_args) = arg.child_by_field_name("arguments") {
                    self.walk(inner_args, ctx, stmt_index);
                }
            } else {
                self.walk(arg, ctx, stmt_index);
            }
        }
    }

    // --- helpers ---

    fn top_level_call_of(&self, node: Node<'_>) -> Option<(SmallVec<[String; 2]>, RefKind)> {
        let call = match node.kind() {
            "call_expression" => node,
            "await_expression" | "try_expression" => {
                return self.top_level_call_of(node.named_child(0)?)
            }
            _ => return None,
        };
        let func = call.child_by_field_name("function")?;
        let (name_path, kind) = self.classify_callee(func);
        if name_path.is_empty() {
            None
        } else {
            Some((name_path, kind))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_ref(
        &mut self,
        name_path: SmallVec<[String; 2]>,
        kind: RefKind,
        condition: EdgeCondition,
        implicit: Option<ImplicitKind>,
        node: Node<'_>,
        ctx: &Ctx,
        stmt_index: u32,
        arity: Option<u8>,
    ) {
        let mut markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
        if self.is_ffi_target(&name_path) {
            markers.push(CutMarker::ViaFfi);
        }
        self.facts.refs.push(RawRef {
            name_path,
            scope: ctx.scope,
            kind,
            edge_condition: condition,
            implicit,
            span: self.span(node),
            stmt_index,
            arity,
            cut_markers: markers,
        });
    }

    fn visibility_of(&self, node: Node<'_>) -> Visibility {
        if self.has_pub(node) {
            Visibility::Public
        } else {
            Visibility::Private
        }
    }

    fn has_pub(&self, node: Node<'_>) -> bool {
        let mut cursor = node.walk();
        // The `any` result is bound before `cursor` drops: the iterator borrows
        // `cursor`, so returning the expression directly would extend that borrow
        // past the function's end (E0597).
        let found = node.children(&mut cursor).any(|c| self.is_pub_modifier(c));
        found
    }

    fn is_pub_modifier(&self, node: Node<'_>) -> bool {
        node.kind() == "visibility_modifier"
            && node
                .utf8_text(self.src)
                .map(|t| t.starts_with("pub"))
                .unwrap_or(false)
    }

    fn pattern_is_err(&self, pattern: Node<'_>) -> bool {
        self.text(pattern).trim_start().starts_with("Err")
    }

    fn is_ffi_target(&self, name_path: &[String]) -> bool {
        name_path.len() == 1
            && name_path
                .first()
                .map(|n| self.extern_fns.iter().any(|e| e == n))
                .unwrap_or(false)
    }

    /// The receiver type name of an `impl` target (`Adder` from `impl T for Adder`,
    /// `Counter` from `impl Counter`). Generic args are dropped.
    fn type_name(&self, node: Node<'_>) -> String {
        match node.kind() {
            "type_identifier" => self.text(node),
            "generic_type" => node
                .child_by_field_name("type")
                .map(|t| self.type_name(t))
                .unwrap_or_else(|| self.text(node)),
            "scoped_type_identifier" => node
                .child_by_field_name("name")
                .map(|n| self.text(n))
                .unwrap_or_else(|| self.text(node)),
            _ => {
                // e.g. `Adder(i32)` tuple-struct impls expose the bare ident.
                let t = self.text(node);
                t.split(['<', '(', ' '])
                    .next()
                    .and_then(|s| s.rsplit("::").next())
                    .unwrap_or(&t)
                    .to_string()
            }
        }
    }

    // --- entrypoints & cut hints ---

    fn record_fn_entrypoints(&mut self, node: Node<'_>, name: &str, fqn: &str) {
        if name == "main" {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.to_string(),
                kind: EntrypointKind::Main,
            });
        }
        for attr in self.preceding_attributes(node) {
            let kind = match attr.as_str() {
                "test" => Some(EntrypointKind::Test),
                "tokio::main" | "async_std::main" => Some(EntrypointKind::AsyncMain),
                _ => None,
            };
            if let Some(kind) = kind {
                self.facts.entrypoint_hints.push(EntrypointHint {
                    fqn: fqn.to_string(),
                    kind,
                });
            }
            if is_attribute_proc_macro(&attr) {
                self.facts.cut_hints.push(CutHint {
                    marker: CutMarker::UnexpandedMacro,
                    span: self.span(node),
                    macro_origin: Some(attr.clone()),
                });
            }
        }
    }

    fn emit_derive_cut_hints(&mut self, node: Node<'_>) {
        for attr in self.preceding_attributes(node) {
            if let Some(derives) = attr.strip_prefix("derive(") {
                let derives = derives.strip_suffix(')').unwrap_or(derives);
                for d in derives.split(',') {
                    let d = d.trim();
                    if d.is_empty() || is_builtin_derive(d) {
                        continue;
                    }
                    self.facts.cut_hints.push(CutHint {
                        marker: CutMarker::UnexpandedMacro,
                        span: self.span(node),
                        macro_origin: Some(d.to_string()),
                    });
                }
            }
        }
    }

    fn preceding_attributes(&self, node: Node<'_>) -> Vec<String> {
        let mut attrs = Vec::new();
        let mut sib = node.prev_sibling();
        while let Some(s) = sib {
            match s.kind() {
                "attribute_item" => {
                    if let Some(content) = self.attribute_content(s) {
                        attrs.push(content);
                    }
                    sib = s.prev_sibling();
                }
                "line_comment" | "block_comment" => sib = s.prev_sibling(),
                _ => break,
            }
        }
        attrs.reverse();
        attrs
    }

    fn attribute_content(&self, attr_item: Node<'_>) -> Option<String> {
        let mut cursor = attr_item.walk();
        let attr = attr_item
            .children(&mut cursor)
            .find(|c| c.kind() == "attribute")?;
        let path = attr
            .named_child(0)
            .map(|p| self.text(p))
            .unwrap_or_default();
        if let Some(args) = attr.child_by_field_name("arguments") {
            let inner = self.text(args);
            let inner = inner.trim_start_matches('(').trim_end_matches(')').trim();
            Some(format!("{path}({inner})"))
        } else {
            Some(path)
        }
    }

    // --- signatures (ADR-04) ---

    fn signature_of(&self, node: Node<'_>) -> Option<Signature> {
        let params_node = node.child_by_field_name("parameters")?;
        let mut sig = Signature::empty();
        let mut cursor = params_node.walk();
        for p in params_node.children(&mut cursor) {
            match p.kind() {
                "self_parameter" => sig.receiver = Some(self.text(p)),
                "parameter" => {
                    let name = self
                        .field_text(p, "pattern")
                        .unwrap_or_else(|| "_".to_string());
                    sig.params.push(Param {
                        name,
                        type_text: self.field_text(p, "type"),
                        has_default: false,
                        variadic: false,
                    });
                }
                "variadic_parameter" => sig.params.push(Param {
                    name: "...".to_string(),
                    type_text: None,
                    has_default: false,
                    variadic: true,
                }),
                _ => {}
            }
        }
        if let Some(ret) = node.child_by_field_name("return_type") {
            sig.return_type_text = Some(self.text(ret));
        }
        if let Some(tps) = node.child_by_field_name("type_parameters") {
            for tp in named_children(tps) {
                sig.type_params.push(self.text(tp));
            }
        }
        Some(sig)
    }

    /// Signature of a `closure_expression` (`|x: T| body` / `move |x| body`).
    /// Closures expose `closure_parameters` rather than `parameters`.
    fn closure_signature(&self, node: Node<'_>) -> Option<Signature> {
        let params = node.child_by_field_name("parameters")?;
        let mut sig = Signature::empty();
        let mut cursor = params.walk();
        for p in params.children(&mut cursor) {
            match p.kind() {
                "parameter" => sig.params.push(Param {
                    name: self
                        .field_text(p, "pattern")
                        .unwrap_or_else(|| "_".to_string()),
                    type_text: self.field_text(p, "type"),
                    has_default: false,
                    variadic: false,
                }),
                "identifier" => sig.params.push(Param {
                    name: self.text(p),
                    type_text: None,
                    has_default: false,
                    variadic: false,
                }),
                _ => {}
            }
        }
        if let Some(ret) = node.child_by_field_name("return_type") {
            sig.return_type_text = Some(self.text(ret));
        }
        Some(sig)
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

/// `(impl_fqn, method)` → the impl method's full FQN as `::`-split segments
/// (`("a::Circle", "area")` → `["a", "Circle", "area"]`). The resolver re-joins
/// the segments to look the method up by exact FQN.
fn name_path_two(prefix: &str, name: &str) -> SmallVec<[String; 2]> {
    join(prefix, name).split("::").map(str::to_owned).collect()
}

/// Whether a type is a standard smart-pointer/container wrapper whose `::new`
/// constructs an *inner* type rather than the wrapper itself.
fn is_smart_pointer(type_name: &str) -> bool {
    matches!(type_name, "Box" | "Rc" | "Arc" | "RefCell" | "Cell" | "Mutex" | "RwLock")
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| c.is_named())
        .collect()
}

fn count_args(args: Node<'_>) -> u8 {
    let mut cursor = args.walk();
    let mut n: u32 = 0;
    for child in args.children(&mut cursor) {
        if child.is_named() && !child.kind().contains("comment") {
            n += 1;
        }
    }
    n.min(u8::MAX as u32) as u8
}

fn split_specifier(segs: &[String]) -> Option<(String, String)> {
    let (last, rest) = segs.split_last()?;
    Some((last.clone(), rest.join("::")))
}

fn is_spawn_path(text: &str) -> bool {
    matches!(
        text,
        "tokio::spawn" | "task::spawn" | "tokio::task::spawn" | "async_std::task::spawn"
    )
}

fn is_panic_macro(name: &str) -> bool {
    matches!(
        name,
        "panic"
            | "unreachable"
            | "todo"
            | "unimplemented"
            | "assert"
            | "assert_eq"
            | "assert_ne"
            | "debug_assert"
            | "debug_assert_eq"
            | "debug_assert_ne"
    )
}

fn is_attribute_proc_macro(attr: &str) -> bool {
    let head = attr.split('(').next().unwrap_or(attr);
    !matches!(
        head,
        "test"
            | "cfg"
            | "cfg_attr"
            | "allow"
            | "deny"
            | "warn"
            | "forbid"
            | "inline"
            | "derive"
            | "doc"
            | "must_use"
            | "deprecated"
            | "non_exhaustive"
            | "repr"
            | "tokio::main"
            | "async_std::main"
            | "ignore"
            | "should_panic"
    )
}

fn is_builtin_derive(name: &str) -> bool {
    matches!(
        name,
        "Debug" | "Clone" | "Copy" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" | "Default"
    )
}

fn panic_target() -> SmallVec<[String; 2]> {
    ["core", "panicking", "panic_fmt"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn iterator_next_target() -> SmallVec<[String; 2]> {
    ["core", "iter", "Iterator", "next"]
        .into_iter()
        .map(str::to_string)
        .collect()
}
