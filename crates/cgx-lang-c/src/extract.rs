//! The C extractor: a single recursive walk over the `tree-sitter-c` parse tree
//! that builds [`FileFacts`].
//!
//! C has a flat global namespace (no classes, no packages), so a definition's
//! local FQN is simply its source name — a non-`static` function `helper` links
//! as the bare symbol `helper`, exactly as C's external-linkage model treats it.
//! `static` gives file-local [`Visibility::Internal`].
//!
//! ## The scope-attribution contract (why this adapter exists)
//! The shared Tier-0 [`FallbackFrontend`] attributes every call to [`ScopeId::
//! ROOT`] because a C `function_definition`'s name sits one level deep (inside the
//! `function_declarator`), so the generic walk never opens the function's own
//! scope over its body. The resolver's `enclosing_def` then finds no caller and
//! silently drops the ref. This extractor fixes that by construction: it opens a
//! scope **owned by the function's FQN** and walks the body under it, so every
//! call ref carries the enclosing function's scope and resolves end to end.
//!
//! ## Preprocessor (ADR B1 — grammar-only, expand nothing, fabricate nothing)
//! - `#ifdef`/`#if` arms are all walked (ordinary content nodes) — over-approximate.
//! - `#include` → an [`ImportFact`] (glob), local vs system by the path-node kind.
//! - A `call_expression` whose callee is a known function-like macro (from a
//!   `preproc_function_def`) emits a [`CutMarker::UnexpandedMacro`] cut hint and
//!   **no** call edge — never a fabricated edge to the macro name.
//!
//! ## Honesty ceiling (Tier 2)
//! - Function-pointer / callback dispatch (`(*fp)(…)`, a fn-pointer parameter or
//!   local, `obj->handler(…)`) → [`RefKind::CallCallback`], which the shared
//!   resolver bands to `possible` — never a fabricated concrete target.
//! - Cross-file / cross-TU direct calls resolve through the shared `link()` to its
//!   `NameSyntactic` → `possible` ceiling (candidate-set banding); this adapter
//!   never forks the linker.
//! - `panic`/`setjmp`/`longjmp` are not modeled as structured edges.

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::{
    CutHint, EffectFact, EntrypointHint, FileCtx, FileFacts, FrontendError, ImportFact,
    ImportedName, Lang, LanguageFrontend, RawRef, RefKind, RelPath, ScopeId, SymbolDef,
};
use smallvec::SmallVec;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser};

use crate::effects::effects_of_call;

/// The C language adapter. Stateless; one instance handles every `.c`/`.h` file.
///
/// `.h` is claimed here, not by `cgx-lang-cpp`: C and C++ share the extension and
/// tree-sitter-c cannot tell them apart from the path alone.
/// // TODO(phase-E): .h C-vs-C++ disambiguation.
#[derive(Debug, Default, Clone)]
pub struct CFrontend;

impl CFrontend {
    pub fn new() -> Self {
        CFrontend
    }
}

/// Version of the C extraction rules; bumping invalidates cached fragments.
const C_FRAGMENT_VERSION: u32 = 1;

impl LanguageFrontend for CFrontend {
    fn lang(&self) -> Lang {
        Lang::Other("c".into())
    }

    fn handles(&self, path: &RelPath) -> bool {
        matches!(path.extension().as_deref(), Some("c") | Some("h"))
    }

    fn fragment_version(&self) -> u32 {
        C_FRAGMENT_VERSION
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .map_err(|e| FrontendError::Parser {
                lang: "c".to_string(),
                detail: e.to_string(),
            })?;

        let tree = match parser.parse(src, None) {
            Some(tree) => tree,
            None => return Ok(FileFacts::empty()),
        };

        let mut builder = Builder::new(src, ctx.path.as_str());
        builder.prescan(tree.root_node());

        let root_ctx = Ctx::root();
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
    scope: ScopeId,
    conditions: SmallVec<[EdgeCondition; 4]>,
}

impl Ctx {
    fn root() -> Self {
        Ctx {
            scope: ScopeId::ROOT,
            conditions: SmallVec::new(),
        }
    }

    fn enter_body(&self, scope: ScopeId) -> Self {
        Ctx {
            scope,
            conditions: SmallVec::new(),
        }
    }

    fn with_condition(&self, cond: EdgeCondition) -> Self {
        let mut conditions = self.conditions.clone();
        conditions.push(cond);
        Ctx {
            scope: self.scope,
            conditions,
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
    /// FQNs already emitted, so a `typedef struct Point {…} Point;` (tag + typedef
    /// name collide) yields one def, not two.
    seen_fqns: BTreeSet<String>,
    /// Names of function-like macros (`#define FOO(x) …`); a call to one of these
    /// is an unexpanded-macro site, not a call edge.
    fn_like_macros: BTreeSet<String>,
    /// Typedef names whose underlying type is a function pointer
    /// (`typedef int (*BinOp)(int);`) — a binding of such a type is indirect.
    fn_ptr_typedefs: BTreeSet<String>,
    /// File-scope function-pointer globals (`int (*handler)(int);`).
    global_fn_ptrs: BTreeSet<String>,
    /// Function-pointer bindings (params + locals + globals) visible in the body
    /// currently being walked. A call whose callee is one of these is indirect.
    fn_ptrs: BTreeSet<String>,
    /// Accumulated syntactic own-effects keyed by the enclosing function's FQN.
    effects: BTreeMap<String, cgx_core::effect::EffectSet>,
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], file: &str) -> Self {
        Builder {
            src,
            file: file.to_string(),
            facts: FileFacts::empty(),
            seen_fqns: BTreeSet::new(),
            fn_like_macros: BTreeSet::new(),
            fn_ptr_typedefs: BTreeSet::new(),
            global_fn_ptrs: BTreeSet::new(),
            fn_ptrs: BTreeSet::new(),
            effects: BTreeMap::new(),
        }
    }

    fn finish(mut self) -> FileFacts {
        for (fqn, set) in std::mem::take(&mut self.effects) {
            if !set.is_empty() {
                self.facts.effects.push(EffectFact { fqn, effects: set });
            }
        }
        self.facts
    }

    /// A first pass over the top level that collects whole-file facts a body walk
    /// needs up front: function-like macro names, function-pointer typedef names,
    /// and file-scope function-pointer globals.
    fn prescan(&mut self, root: Node<'_>) {
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            match child.kind() {
                "preproc_function_def" => {
                    if let Some(name) = self.field_text(child, "name") {
                        self.fn_like_macros.insert(name);
                    }
                }
                "type_definition" => {
                    if is_fn_pointer_declarator(child.child_by_field_name("declarator")) {
                        if let Some(name) =
                            base_declarator_name(self, child.child_by_field_name("declarator"))
                        {
                            self.fn_ptr_typedefs.insert(name);
                        }
                    }
                }
                _ => {}
            }
        }
        // Second micro-pass: file-scope fn-pointer globals, now that fn-ptr
        // typedef names are known (a global may be declared via such a typedef).
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() == "declaration" {
                for name in self.fn_ptr_binding_names(child) {
                    self.global_fn_ptrs.insert(name);
                }
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

    fn push_def(&mut self, def: SymbolDef) {
        if self.seen_fqns.insert(def.fqn.clone()) {
            self.facts.defs.push(def);
        }
    }

    // --- main dispatch ---

    fn walk(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        match node.kind() {
            "function_definition" => self.walk_function(node, ctx, stmt_index),
            "declaration" => self.walk_declaration(node, ctx, stmt_index),
            "type_definition" => self.walk_typedef(node, ctx, stmt_index),
            "struct_specifier" | "union_specifier" | "enum_specifier" => self.walk_record(node),
            "preproc_include" => self.walk_include(node, ctx),
            "preproc_def" | "preproc_function_def" => self.walk_macro_def(node),
            "call_expression" => self.walk_call_expression(node, ctx, stmt_index),
            "if_statement" => self.walk_if(node, ctx, stmt_index),
            "for_statement" | "while_statement" | "do_statement" => {
                self.walk_loop(node, ctx, stmt_index)
            }
            "switch_statement" => self.walk_switch(node, ctx, stmt_index),
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

    fn walk_function(&mut self, node: Node<'_>, ctx: &Ctx, _stmt_index: &mut u32) {
        let Some(decl) = find_function_declarator(node.child_by_field_name("declarator")) else {
            return;
        };
        let Some(name) = base_declarator_name(self, decl.child_by_field_name("declarator")) else {
            return;
        };
        let fqn = name.clone();
        let vis = if has_static(node) {
            Visibility::Internal
        } else {
            Visibility::Public
        };
        // Open a scope OWNED by this function so every call in its body attributes
        // to it (the scope-attribution contract above).
        let body_scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.push_def(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Function,
            visibility: vis,
            scope: ctx.scope,
            span: self.span(name_node(decl).unwrap_or(node)),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
        if name == "main" {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.clone(),
                kind: EntrypointKind::Main,
            });
        }

        // Collect the function-pointer bindings (params + body locals + globals)
        // visible in this body before walking it.
        let mut fn_ptrs = self.global_fn_ptrs.clone();
        if let Some(params) = decl.child_by_field_name("parameters") {
            let mut c = params.walk();
            for p in params.children(&mut c) {
                if p.kind() == "parameter_declaration" {
                    fn_ptrs.extend(self.fn_ptr_binding_names(p));
                }
            }
        }
        if let Some(body) = node.child_by_field_name("body") {
            self.collect_body_fn_ptrs(body, &mut fn_ptrs);
        }
        self.fn_ptrs = fn_ptrs;

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(body_scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
        }
        self.fn_ptrs = BTreeSet::new();
    }

    fn walk_declaration(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        // Only file-scope declarations are symbols; a declaration inside a body is
        // a local (not a def) — but we still recurse to catch initializer calls.
        if ctx.scope == ScopeId::ROOT {
            let vis = if has_static(node) {
                Visibility::Internal
            } else {
                Visibility::Public
            };
            let mut c = node.walk();
            for child in node.children(&mut c) {
                let declarator = match child.kind() {
                    "init_declarator" => child.child_by_field_name("declarator"),
                    "identifier" | "pointer_declarator" | "array_declarator"
                    | "function_declarator" => Some(child),
                    _ => None,
                };
                let Some(declarator) = declarator else { continue };
                // A bare `function_declarator` with a plain-identifier base is a
                // prototype (declaration, not definition) — not a symbol here.
                if is_prototype_declarator(Some(declarator)) {
                    continue;
                }
                if let Some(name) = base_declarator_name(self, Some(declarator)) {
                    self.push_def(SymbolDef {
                        fqn: name,
                        kind: SymbolKind::Variable,
                        visibility: vis,
                        scope: ctx.scope,
                        span: self.span(declarator),
                        line_end: self.end_line(node),
                        is_abstract: false,
                        signature: None,
                    });
                }
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    fn walk_typedef(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(name) = base_declarator_name(self, node.child_by_field_name("declarator")) {
            self.push_def(SymbolDef {
                fqn: name,
                kind: SymbolKind::Type,
                visibility: Visibility::Public,
                scope: ctx.scope,
                span: self.span(node.child_by_field_name("declarator").unwrap_or(node)),
                line_end: self.end_line(node),
                is_abstract: false,
                signature: None,
            });
        }
        // Emit any named struct/union/enum the typedef wraps (e.g.
        // `typedef struct Point {…} Point;`), deduped against the typedef name.
        if let Some(ty) = node.child_by_field_name("type") {
            if matches!(
                ty.kind(),
                "struct_specifier" | "union_specifier" | "enum_specifier"
            ) {
                self.walk_record(ty);
            }
        }
        let _ = stmt_index;
    }

    /// Emit a `struct`/`union`/`enum` tag as a `Type` def (plus `enum` constants as
    /// `Constant` defs). Anonymous records emit nothing for the tag.
    fn walk_record(&mut self, node: Node<'_>) {
        if let Some(name) = self.field_text(node, "name") {
            self.push_def(SymbolDef {
                fqn: name,
                kind: SymbolKind::Type,
                visibility: Visibility::Public,
                scope: ScopeId::ROOT,
                span: self.span(node.child_by_field_name("name").unwrap_or(node)),
                line_end: self.end_line(node),
                is_abstract: false,
                signature: None,
            });
        }
        if node.kind() == "enum_specifier" {
            if let Some(body) = node.child_by_field_name("body") {
                let mut c = body.walk();
                for e in body.children(&mut c) {
                    if e.kind() == "enumerator" {
                        if let Some(cname) = self.field_text(e, "name") {
                            self.push_def(SymbolDef {
                                fqn: cname,
                                kind: SymbolKind::Constant,
                                visibility: Visibility::Public,
                                scope: ScopeId::ROOT,
                                span: self.span(e),
                                line_end: self.end_line(e),
                                is_abstract: false,
                                signature: None,
                            });
                        }
                    }
                }
            }
        }
    }

    fn walk_macro_def(&mut self, node: Node<'_>) {
        // A macro name is a file-scope definition the inventory records; its body
        // (`preproc_arg`) is an opaque leaf we never parse (ADR B1).
        if let Some(name) = self.field_text(node, "name") {
            self.push_def(SymbolDef {
                fqn: name,
                kind: SymbolKind::Constant,
                visibility: Visibility::Public,
                scope: ScopeId::ROOT,
                span: self.span(node.child_by_field_name("name").unwrap_or(node)),
                line_end: self.end_line(node),
                is_abstract: false,
                signature: None,
            });
        }
    }

    // --- includes ---

    fn walk_include(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(path_node) = node.child_by_field_name("path") else {
            return;
        };
        let specifier = match path_node.kind() {
            // `<stdio.h>` — system header. Text already carries the angle brackets.
            "system_lib_string" => {
                let t = self.text(path_node);
                t.trim_start_matches('<').trim_end_matches('>').to_string()
            }
            // `"add.h"` — local header. Prefer the inner `string_content` child.
            "string_literal" => {
                let mut c = path_node.walk();
                let content = path_node
                    .children(&mut c)
                    .find(|n| n.kind() == "string_content")
                    .map(|n| self.text(n))
                    .unwrap_or_else(|| self.text(path_node).trim_matches('"').to_string());
                content
            }
            _ => self.text(path_node).trim_matches(['<', '>', '"']).to_string(),
        };
        // A C header brings all its declarations into scope — model as a glob
        // import so the shared import-graph builder makes the header's defs
        // visible. The target file is NOT resolved here (that is `link()`'s job).
        self.facts.imports.push(ImportFact {
            specifier,
            names: Vec::<ImportedName>::new(),
            glob: true,
            re_export: false,
            scope: ctx.scope,
            span: self.span(node),
        });
    }

    // --- references (calls) ---

    fn walk_call_expression(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(func) = node.child_by_field_name("function") {
            // An unexpanded function-like macro site: record the honest cut, emit
            // NO call edge to the macro name (ADR B1).
            if func.kind() == "identifier" && self.fn_like_macros.contains(&self.text(func)) {
                self.facts.cut_hints.push(CutHint {
                    marker: CutMarker::UnexpandedMacro,
                    span: self.span(node),
                    macro_origin: Some(self.text(func)),
                });
            } else {
                let (name_path, kind) = self.classify_callee(func);
                if !name_path.is_empty() {
                    self.record_effects(ctx, effects_of_call(&name_path.join("::")));
                    let arity = self.call_arity(node);
                    self.push_ref(name_path, kind, ctx, self.span(node), *stmt_index, arity);
                    *stmt_index += 1;
                }
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    /// Classify a call's `function` operand into `(name_path, RefKind)`:
    /// - a bare `identifier` naming a fn-pointer binding → indirect `CallCallback`;
    /// - a bare `identifier` otherwise → direct `Call`;
    /// - `obj->m` / `obj.m` (`field_expression`) → indirect `CallCallback` (a C
    ///   struct function-pointer member);
    /// - `(*fp)(…)` (`parenthesized_expression`/`pointer_expression`) → indirect.
    fn classify_callee(&self, func: Node<'_>) -> (SmallVec<[String; 2]>, RefKind) {
        match func.kind() {
            "identifier" => {
                let name = self.text(func);
                let kind = if self.fn_ptrs.contains(&name) {
                    RefKind::CallCallback
                } else {
                    RefKind::Call
                };
                (smallvec_one(name), kind)
            }
            "field_expression" => match func.child_by_field_name("field") {
                Some(field) => (smallvec_one(self.text(field)), RefKind::CallCallback),
                None => (SmallVec::new(), RefKind::Call),
            },
            "parenthesized_expression" => func
                .named_child(0)
                .map(|inner| {
                    // `(*fp)(…)` → a dereferenced function pointer: indirect.
                    let (path, _) = self.classify_callee(inner);
                    (path, RefKind::CallCallback)
                })
                .unwrap_or((SmallVec::new(), RefKind::CallCallback)),
            "pointer_expression" => {
                // `*fp` as a callee operand.
                let path = func
                    .child_by_field_name("argument")
                    .map(|a| self.callee_ident(a))
                    .unwrap_or_default();
                (path, RefKind::CallCallback)
            }
            _ => (self.callee_ident(func), RefKind::CallCallback),
        }
    }

    /// Best-effort leading identifier of an arbitrary callee expression.
    fn callee_ident(&self, node: Node<'_>) -> SmallVec<[String; 2]> {
        match node.kind() {
            "identifier" | "field_identifier" | "type_identifier" => smallvec_one(self.text(node)),
            _ => {
                let seg: String = self
                    .text(node)
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if seg.is_empty() {
                    SmallVec::new()
                } else {
                    smallvec_one(seg)
                }
            }
        }
    }

    fn call_arity(&self, call: Node<'_>) -> Option<u8> {
        let args = call.child_by_field_name("arguments")?;
        let mut c = args.walk();
        let n = args.children(&mut c).filter(|a| a.is_named()).count();
        Some(n.min(255) as u8)
    }

    fn push_ref(
        &mut self,
        name_path: SmallVec<[String; 2]>,
        kind: RefKind,
        ctx: &Ctx,
        span: Span,
        stmt_index: u32,
        arity: Option<u8>,
    ) {
        self.facts.refs.push(RawRef {
            name_path,
            scope: ctx.scope,
            kind,
            edge_condition: ctx.condition(),
            implicit: None,
            span,
            stmt_index,
            arity,
            cut_markers: SmallVec::new(),
        });
    }

    fn record_effects(&mut self, ctx: &Ctx, set: cgx_core::effect::EffectSet) {
        if set.is_empty() {
            return;
        }
        if let Some(owner) = self.owner_fqn(ctx.scope) {
            self.effects.entry(owner).or_default().union_with(set);
        }
    }

    /// The owning function FQN of a scope (walk to the nearest owner).
    fn owner_fqn(&self, scope: ScopeId) -> Option<String> {
        let mut cur = Some(scope);
        while let Some(id) = cur {
            let s = self.facts.scopes.scopes.get(id.index())?;
            if let Some(owner) = &s.owner_fqn {
                return Some(owner.clone());
            }
            cur = s.parent;
        }
        None
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

    fn walk_loop(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            let inner = ctx.with_condition(EdgeCondition::Loop);
            self.walk(body, &inner, stmt_index);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == body {
                continue;
            }
            self.walk(child, ctx, stmt_index);
        }
    }

    fn walk_switch(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            let inner = ctx.with_condition(EdgeCondition::Conditional);
            self.walk(body, &inner, stmt_index);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == body {
                continue;
            }
            self.walk(child, ctx, stmt_index);
        }
    }

    // --- function-pointer binding detection ---

    /// The names bound as function pointers by one declaration / parameter: either
    /// an inline fn-pointer declarator (`int (*op)(int)`) or a binding whose
    /// declared type is a known fn-pointer typedef (`BinOp f;`).
    fn fn_ptr_binding_names(&self, node: Node<'_>) -> Vec<String> {
        let mut out = Vec::new();
        let type_is_fn_ptr_typedef = node
            .child_by_field_name("type")
            .map(|t| t.kind() == "type_identifier" && self.fn_ptr_typedefs.contains(&self.text(t)))
            .unwrap_or(false);
        let mut c = node.walk();
        for child in node.children(&mut c) {
            let declarator = match child.kind() {
                "init_declarator" => child.child_by_field_name("declarator"),
                "identifier" | "pointer_declarator" | "function_declarator" => Some(child),
                _ => None,
            };
            let Some(declarator) = declarator else { continue };
            if is_fn_pointer_declarator(Some(declarator))
                || (type_is_fn_ptr_typedef && !is_prototype_declarator(Some(declarator)))
            {
                if let Some(name) = base_declarator_name(self, Some(declarator)) {
                    out.push(name);
                }
            }
        }
        // A parameter like `BinOp op` has its name as a direct `identifier`
        // declarator and the type carrying the fn-pointer typedef.
        if out.is_empty() && type_is_fn_ptr_typedef {
            if let Some(name) = base_declarator_name(self, node.child_by_field_name("declarator")) {
                out.push(name);
            }
        }
        out
    }

    /// Recursively collect function-pointer local names declared anywhere in a
    /// function body (so a call to one is classified indirect regardless of order).
    fn collect_body_fn_ptrs(&self, node: Node<'_>, out: &mut BTreeSet<String>) {
        if node.kind() == "declaration" {
            for name in self.fn_ptr_binding_names(node) {
                out.insert(name);
            }
        }
        let mut c = node.walk();
        for child in node.children(&mut c) {
            self.collect_body_fn_ptrs(child, out);
        }
    }
}

// === free functions ===

fn smallvec_one(s: String) -> SmallVec<[String; 2]> {
    let mut v = SmallVec::new();
    v.push(s);
    v
}

/// Whether a declaration/definition carries a `static` storage-class specifier.
/// A `storage_class_specifier` wraps the keyword as its first child, whose node
/// kind is literally the keyword text (`static`).
fn has_static(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|c| {
        c.kind() == "storage_class_specifier" && c.child(0).map(|k| k.kind()) == Some("static")
    });
    found
}

/// The next declarator node to descend into. Most declarator nodes expose their
/// inner declarator via the `declarator` field, but a `parenthesized_declarator`
/// (`(*op)`) wraps its inner declarator as an unnamed child, so fall back to its
/// first named child.
fn descend<'t>(n: Node<'t>) -> Option<Node<'t>> {
    if n.kind() == "parenthesized_declarator" {
        let mut c = n.walk();
        return n.children(&mut c).find(|k| k.is_named());
    }
    n.child_by_field_name("declarator")
}

const DECLARATOR_KINDS: &[&str] = &[
    "pointer_declarator",
    "parenthesized_declarator",
    "array_declarator",
    "function_declarator",
    "init_declarator",
];

/// Descend the declarator chain of a `function_declarator` to its base name node.
fn name_node<'t>(function_declarator: Node<'t>) -> Option<Node<'t>> {
    let mut cur = descend(function_declarator);
    while let Some(n) = cur {
        match n.kind() {
            "identifier" | "field_identifier" | "type_identifier" => return Some(n),
            k if DECLARATOR_KINDS.contains(&k) => cur = descend(n),
            _ => return None,
        }
    }
    None
}

/// The base identifier text at the bottom of a declarator chain
/// (`(*BinOp)(int)` → `BinOp`, `y = …` → `y`).
fn base_declarator_name(b: &Builder<'_>, node: Option<Node<'_>>) -> Option<String> {
    let mut cur = node;
    while let Some(n) = cur {
        match n.kind() {
            "identifier" | "field_identifier" | "type_identifier" => return Some(b.text(n)),
            k if DECLARATOR_KINDS.contains(&k) => cur = descend(n),
            _ => return None,
        }
    }
    None
}

/// The first `function_declarator` along a declarator chain, if any (handles a
/// pointer-returning function `int *foo()` whose declarator wraps the function).
fn find_function_declarator(node: Option<Node<'_>>) -> Option<Node<'_>> {
    let mut cur = node;
    while let Some(n) = cur {
        match n.kind() {
            "function_declarator" => return Some(n),
            "pointer_declarator" | "parenthesized_declarator" | "array_declarator"
            | "init_declarator" => cur = descend(n),
            _ => return None,
        }
    }
    None
}

/// Whether a declarator chain declares a *function pointer* — it passes through a
/// `function_declarator` AND a pointer/parenthesized indirection before the name.
/// (A plain `function_declarator` with an identifier base is a prototype, not a
/// pointer.)
fn is_fn_pointer_declarator(node: Option<Node<'_>>) -> bool {
    let mut cur = node;
    let mut saw_function = false;
    let mut saw_pointer = false;
    while let Some(n) = cur {
        match n.kind() {
            "function_declarator" => {
                saw_function = true;
                cur = descend(n);
            }
            "pointer_declarator" => {
                saw_pointer = true;
                cur = descend(n);
            }
            "parenthesized_declarator" | "array_declarator" | "init_declarator" => cur = descend(n),
            _ => break,
        }
    }
    saw_function && saw_pointer
}

/// Whether a declarator is a bare function prototype: a `function_declarator`
/// whose base is a plain identifier, with no pointer indirection.
fn is_prototype_declarator(node: Option<Node<'_>>) -> bool {
    match node {
        Some(n) if n.kind() == "function_declarator" => {
            // base is a plain identifier (not a parenthesized pointer).
            matches!(
                n.child_by_field_name("declarator").map(|d| d.kind()),
                Some("identifier")
            )
        }
        _ => false,
    }
}
