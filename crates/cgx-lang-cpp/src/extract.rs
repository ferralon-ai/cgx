//! The C++ extractor: a single recursive walk over the `tree-sitter-cpp` parse
//! tree that builds [`FileFacts`].
//!
//! tree-sitter-cpp is a superset of tree-sitter-c, so this adapter reuses the C
//! kit wholesale — the grammar-only preprocessor model, the declarator-descent
//! helpers, the scope-attribution contract, the function-pointer/callback honesty
//! classification — and layers the C++ surface on top: namespaces and `::`-FQN
//! construction, classes/structs/unions with methods and constructors/destructors,
//! the inheritance lattice (`Inherits`/`Overrides`), CHA virtual dispatch,
//! `throw`/`catch` exception edges, templates, and overload candidate sets.
//!
//! ## Honesty ceiling (parity-map C++ column)
//! Tier-1 for **syntactic / hierarchy** facts (classes, namespaces/FQN,
//! inheritance/overrides, CHA virtual dispatch, throw/catch). Tier-2 **`possible`**
//! for **type-dependent** facts the frontend has no types for:
//! - **Virtual dispatch** is emitted as [`RefKind::CallVirtualReceiver`] (exactly
//!   the Java shape): the shared resolver's size-only banding
//!   (`cgx-resolve::link.rs::canonicalize_candidate_dsts`) makes a single-candidate
//!   method `probable` and a multi-candidate one `possible` **by construction** —
//!   this adapter implements no C++-specific band.
//! - **Overloads** share one signature-free FQN, so a call to an overloaded name
//!   resolves to the multi-candidate set → `possible`. Each overload is a distinct
//!   def (distinguished by span + [`SymbolDef::signature`], the overload key Phase
//!   F's scip-clang mapper keys `(qname, overload_key)` on).
//! - **Templates** are parsed, never instantiated: only edges that exist
//!   pre-instantiation are emitted; a call through a template-parameter-typed
//!   receiver is a member call → candidate set → `possible` (or honestly dangling),
//!   never a fabricated instantiation edge.
//! - **RAII destructor edges are never emitted** (the source spells no token for a
//!   compiler-inserted destructor call — a fabricated edge). Only *explicit*
//!   destructor calls that appear in source are recorded.
//!
//! ## Preprocessor (ADR B1 — grammar-only, expand nothing, fabricate nothing)
//! Identical to the C adapter: `#ifdef` arms all-walked (over-approximate),
//! `#include` → glob [`ImportFact`], a `call_expression` to a known function-like
//! macro → [`CutMarker::UnexpandedMacro`] with no call edge.

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_core::signature::{Param, Signature};
use cgx_frontend::{
    CutHint, EffectFact, EntrypointHint, FileCtx, FileFacts, FrontendError, ImplRelation,
    ImportFact, ImportedName, Lang, LanguageFrontend, Name, RawRef, RefKind, RelPath, RelationKind,
    ScopeId, SymbolDef,
};
use smallvec::SmallVec;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser};

use crate::effects::effects_of_call;

/// The C++ language adapter. Stateless; one instance handles every C++ file.
///
/// `.h` is intentionally **not** claimed here (it goes to `cgx-lang-c`, which owns
/// the shared C/C++ header extension); C++ headers conventionally use
/// `.hpp`/`.hh`/`.hxx`.
#[derive(Debug, Default, Clone)]
pub struct CppFrontend;

impl CppFrontend {
    pub fn new() -> Self {
        CppFrontend
    }
}

/// Version of the C++ extraction rules; bumping invalidates cached fragments.
const CPP_FRAGMENT_VERSION: u32 = 1;

impl LanguageFrontend for CppFrontend {
    fn lang(&self) -> Lang {
        Lang::Other("cpp".into())
    }

    fn handles(&self, path: &RelPath) -> bool {
        matches!(
            path.extension().as_deref(),
            Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") | Some("hh") | Some("hxx")
        )
    }

    fn fragment_version(&self) -> u32 {
        CPP_FRAGMENT_VERSION
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .map_err(|e| FrontendError::Parser {
                lang: "cpp".to_string(),
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

/// Walk context threaded down the tree: the FQN prefix defs live under, the
/// lexical scope refs/defs record in, the enclosing guarding-construct chain whose
/// ADR-03 maximum is a call site's edge condition, and any enclosing template's
/// type parameters (recorded onto a callable's signature).
#[derive(Clone)]
struct Ctx {
    fqn_prefix: String,
    scope: ScopeId,
    conditions: SmallVec<[EdgeCondition; 4]>,
    type_params: SmallVec<[String; 2]>,
}

impl Ctx {
    fn root() -> Self {
        Ctx {
            fqn_prefix: String::new(),
            scope: ScopeId::ROOT,
            conditions: SmallVec::new(),
            type_params: SmallVec::new(),
        }
    }

    /// Enter a namespace/type/callable body: the body's FQN becomes the prefix for
    /// members, its scope becomes their parent, and the condition chain resets (a
    /// body root is unguarded). Template type-params do not cross a body boundary.
    fn enter_body(&self, fqn_prefix: String, scope: ScopeId) -> Self {
        Ctx {
            fqn_prefix,
            scope,
            conditions: SmallVec::new(),
            type_params: SmallVec::new(),
        }
    }

    fn with_condition(&self, cond: EdgeCondition) -> Self {
        let mut c = self.clone();
        c.conditions.push(cond);
        c
    }

    fn with_type_params(&self, tps: SmallVec<[String; 2]>) -> Self {
        let mut c = self.clone();
        c.type_params = tps;
        c
    }

    fn condition(&self) -> EdgeCondition {
        EdgeCondition::resolve(self.conditions.iter().copied())
    }
}

/// A directly-declared method recorded while walking a type body, replayed in
/// [`Builder::flush_overrides`] to emit `Overrides` relations.
struct MethodRec {
    name: String,
    fqn: String,
    arity: usize,
    has_override: bool,
    span: Span,
}

struct Builder<'a> {
    src: &'a [u8],
    file: String,
    facts: FileFacts,
    /// Non-callable defs already emitted (types, fields, constants, modules,
    /// macros), deduped by FQN — a forward declaration + definition, or a
    /// `typedef`+tag collision, yields one def.
    seen_fqns: BTreeSet<String>,
    /// Callable defs already emitted, keyed by `(fqn, signature canonical)` → the
    /// def's index in `facts.defs` and whether it already carries a body. A
    /// declaration + out-of-line definition of the **same** signature collapse to
    /// one def (the body wins); **overloads** differ in signature canonical and
    /// stay distinct.
    seen_callables: BTreeMap<(String, String), (usize, bool)>,
    /// Function-like macro names (`#define FOO(x) …`); a call to one is an
    /// unexpanded-macro site, not a call edge (ADR B1).
    fn_like_macros: BTreeSet<String>,
    /// Per-type declared base-class simple names, keyed by the type's FQN. Drives
    /// override candidacy.
    type_supertypes: BTreeMap<String, Vec<String>>,
    /// Methods declared directly in each type body, keyed by the type's FQN.
    type_methods: BTreeMap<String, Vec<MethodRec>>,
    /// In-file `(method name, arity)` sets keyed by a type's *simple* name, so a
    /// subtype method can be name+arity matched against an in-file base.
    type_method_sigs: BTreeMap<String, BTreeSet<(String, usize)>>,
    /// Accumulated syntactic own-effects keyed by the enclosing callable's FQN.
    effects: BTreeMap<String, cgx_core::effect::EffectSet>,
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], file: &str) -> Self {
        Builder {
            src,
            file: file.to_string(),
            facts: FileFacts::empty(),
            seen_fqns: BTreeSet::new(),
            seen_callables: BTreeMap::new(),
            fn_like_macros: BTreeSet::new(),
            type_supertypes: BTreeMap::new(),
            type_methods: BTreeMap::new(),
            type_method_sigs: BTreeMap::new(),
            effects: BTreeMap::new(),
        }
    }

    fn finish(mut self) -> FileFacts {
        self.flush_overrides();
        for (fqn, set) in std::mem::take(&mut self.effects) {
            if !set.is_empty() {
                self.facts.effects.push(EffectFact { fqn, effects: set });
            }
        }
        self.facts
    }

    /// A first pass over the whole tree collecting function-like macro names a body
    /// walk needs up front.
    fn prescan(&mut self, node: Node<'_>) {
        if node.kind() == "preproc_function_def" {
            if let Some(name) = self.field_text(node, "name") {
                self.fn_like_macros.insert(name);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.prescan(child);
        }
    }

    /// Emit an `Overrides` relation for every directly-declared method that
    /// overrides a base method — `override`/`final` is authoritative; an
    /// unannotated method is matched by name+arity against an in-file base type's
    /// methods (the cross-file non-annotated case is left to a later CHA pass).
    fn flush_overrides(&mut self) {
        let type_methods = std::mem::take(&mut self.type_methods);
        let type_supertypes = std::mem::take(&mut self.type_supertypes);
        let type_method_sigs = std::mem::take(&mut self.type_method_sigs);
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        for (type_fqn, methods) in &type_methods {
            let Some(supers) = type_supertypes.get(type_fqn) else {
                continue;
            };
            for m in methods {
                for s in supers {
                    let matched = m.has_override
                        || type_method_sigs
                            .get(s)
                            .is_some_and(|sigs| sigs.contains(&(m.name.clone(), m.arity)));
                    if !matched {
                        continue;
                    }
                    let object = format!("{s}::{}", m.name);
                    if !seen.insert((m.fqn.clone(), object)) {
                        continue;
                    }
                    self.facts.impl_relations.push(ImplRelation {
                        kind: RelationKind::Overrides,
                        subject: fqn_segments(&m.fqn),
                        object: name_path_two(s, &m.name),
                        span: m.span.clone(),
                    });
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

    /// Push a non-callable def, deduped by FQN.
    fn push_simple_def(&mut self, def: SymbolDef) {
        if self.seen_fqns.insert(def.fqn.clone()) {
            self.facts.defs.push(def);
        }
    }

    /// Push a callable def, deduped by `(fqn, signature-canonical)` — a body
    /// replaces a prior bodyless declaration of the same signature; overloads
    /// (distinct signature) stay distinct.
    fn push_callable_def(&mut self, def: SymbolDef, has_body: bool) {
        let sig_key = def
            .signature
            .as_ref()
            .map(|s| s.canonical())
            .unwrap_or_default();
        let key = (def.fqn.clone(), sig_key);
        match self.seen_callables.get(&key).copied() {
            Some((idx, had_body)) => {
                if has_body && !had_body {
                    self.facts.defs[idx] = def;
                    self.seen_callables.insert(key, (idx, true));
                }
            }
            None => {
                let idx = self.facts.defs.len();
                self.seen_callables.insert(key, (idx, has_body));
                self.facts.defs.push(def);
            }
        }
    }

    // --- main dispatch ---

    fn walk(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        match node.kind() {
            "namespace_definition" => self.walk_namespace(node, ctx, stmt_index),
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                self.walk_class(node, ctx)
            }
            "enum_specifier" => self.walk_enum(node, ctx),
            "function_definition" => self.walk_function(node, ctx, false),
            "template_declaration" => self.walk_template(node, ctx, stmt_index),
            "declaration" => self.walk_declaration(node, ctx, stmt_index),
            "type_definition" => self.walk_typedef(node, ctx),
            "alias_declaration" => self.walk_alias(node, ctx),
            "using_declaration" => self.walk_using(node, ctx),
            "namespace_alias_definition" => self.walk_ns_alias(node, ctx),
            "preproc_include" => self.walk_include(node, ctx),
            "preproc_def" | "preproc_function_def" => self.walk_macro_def(node),
            "call_expression" => self.walk_call(node, ctx, stmt_index),
            "new_expression" => self.walk_new(node, ctx, stmt_index),
            "if_statement" => self.walk_if(node, ctx, stmt_index),
            "for_statement" | "while_statement" | "do_statement" | "for_range_loop" => {
                self.walk_loop(node, ctx, stmt_index)
            }
            "switch_statement" => self.walk_switch(node, ctx, stmt_index),
            "try_statement" => self.walk_try(node, ctx, stmt_index),
            _ => self.walk_children(node, ctx, stmt_index),
        }
    }

    fn walk_children(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, ctx, stmt_index);
        }
    }

    // --- namespaces ---

    fn walk_namespace(&mut self, node: Node<'_>, ctx: &Ctx, _stmt_index: &mut u32) {
        // An anonymous namespace keeps the enclosing prefix (internal linkage) and
        // opens no named module def.
        let prefix = match node.child_by_field_name("name") {
            Some(name) => {
                let p = join(&ctx.fqn_prefix, &self.text(name));
                self.push_simple_def(SymbolDef {
                    fqn: p.clone(),
                    kind: SymbolKind::Module,
                    visibility: Visibility::Public,
                    scope: ctx.scope,
                    span: self.span(name),
                    line_end: self.end_line(node),
                    is_abstract: false,
                    signature: None,
                });
                p
            }
            None => ctx.fqn_prefix.clone(),
        };
        let scope = self.facts.scopes.push(ctx.scope, Some(prefix.clone()));
        let body_ctx = ctx.enter_body(prefix, scope);
        if let Some(body) = node.child_by_field_name("body") {
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
        }
    }

    // --- classes / structs / unions ---

    fn walk_class(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            // An anonymous struct/union (e.g. inside a typedef) has no FQN to hang
            // members on; still recurse so nested content is not lost.
            if let Some(body) = node.child_by_field_name("body") {
                let mut inner = 0u32;
                self.walk(body, ctx, &mut inner);
            }
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        self.push_simple_def(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Type,
            visibility: Visibility::Public,
            scope: ctx.scope,
            span: self.span(node.child_by_field_name("name").unwrap_or(node)),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });

        // Inheritance lattice: every base class is an `Inherits` relation (C++ has
        // no interface/implements distinction). Record the base simple names for
        // override candidacy.
        let supers = self.emit_inheritance(node, &fqn);
        if !supers.is_empty() {
            self.type_supertypes.insert(fqn.clone(), supers);
        }

        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        let body_ctx = ctx.enter_body(fqn, scope);
        if let Some(body) = node.child_by_field_name("body") {
            // Default access: `class` → private, `struct`/`union` → public.
            let mut access = if node.kind() == "class_specifier" {
                Visibility::Private
            } else {
                Visibility::Public
            };
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                self.walk_member(child, &body_ctx, &name, &mut access);
            }
        }
    }

    /// Emit `Inherits` edges for a class's base-class clause and return the base
    /// simple names.
    fn emit_inheritance(&mut self, node: Node<'_>, type_fqn: &str) -> Vec<String> {
        let span = self.span(node.child_by_field_name("name").unwrap_or(node));
        let mut supers = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() != "base_class_clause" {
                continue;
            }
            let mut bc = child.walk();
            for b in child.children(&mut bc) {
                // Base names are `type_identifier` / `qualified_identifier` /
                // `template_type`; access specifiers and `virtual` are separate
                // tokens we skip.
                let name = match b.kind() {
                    "type_identifier" => self.text(b),
                    "qualified_identifier" | "template_type" => type_name(&self.text(b)),
                    _ => continue,
                };
                if name.is_empty() {
                    continue;
                }
                self.facts.impl_relations.push(ImplRelation {
                    kind: RelationKind::Inherits,
                    subject: fqn_segments(type_fqn),
                    object: smallvec_one(name.clone()),
                    span: span.clone(),
                });
                supers.push(name);
            }
        }
        supers
    }

    /// Walk one member of a class body, threading the current access specifier.
    fn walk_member(
        &mut self,
        node: Node<'_>,
        ctx: &Ctx,
        class_simple: &str,
        access: &mut Visibility,
    ) {
        match node.kind() {
            "access_specifier" => {
                if let Some(kw) = node.child(0) {
                    *access = match kw.kind() {
                        "public" => Visibility::Public,
                        "protected" => Visibility::Protected,
                        _ => Visibility::Private,
                    };
                }
            }
            // An inline method with a body.
            "function_definition" => self.walk_function_vis(node, ctx, true, *access),
            // A method/ctor/dtor declaration (no body), a nested type, or a data
            // member.
            "field_declaration" | "declaration" => {
                if find_function_declarator(node.child_by_field_name("declarator")).is_some() {
                    self.walk_function_vis(node, ctx, true, *access);
                } else if let Some(spec) = nested_type_specifier(node) {
                    // A nested `class`/`struct`/`union`/`enum` member type.
                    self.walk(spec, ctx, &mut 0u32);
                } else {
                    self.walk_data_members(node, ctx, *access);
                }
            }
            "template_declaration" => {
                // A template member (method/nested type): recurse so the inner
                // declaration is extracted as a member.
                let tps = self.template_type_params(node);
                let inner_ctx = ctx.with_type_params(tps);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if matches!(child.kind(), "template_parameter_list" | "template" | "<" | ">") {
                        continue;
                    }
                    self.walk_member(child, &inner_ctx, class_simple, access);
                }
            }
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                self.walk_class(node, ctx)
            }
            "enum_specifier" => self.walk_enum(node, ctx),
            "alias_declaration" => self.walk_alias(node, ctx),
            "using_declaration" => self.walk_using(node, ctx),
            "type_definition" => self.walk_typedef(node, ctx),
            _ => {
                // Anything else (friend declarations, static-assert, nested
                // preproc) — recurse for any nested calls, but members are only
                // emitted by the arms above.
                let mut inner = 0u32;
                self.walk_children(node, ctx, &mut inner);
            }
        }
    }

    /// Emit `Field` defs for a (possibly multi-declarator) data-member declaration.
    fn walk_data_members(&mut self, node: Node<'_>, ctx: &Ctx, access: Visibility) {
        let line_end = self.end_line(node);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let declarator = match child.kind() {
                "field_identifier" | "pointer_declarator" | "array_declarator"
                | "reference_declarator" => Some(child),
                "init_declarator" => child.child_by_field_name("declarator"),
                _ => None,
            };
            let Some(declarator) = declarator else { continue };
            if let Some(name) = base_declarator_name(self, Some(declarator)) {
                self.push_simple_def(SymbolDef {
                    fqn: join(&ctx.fqn_prefix, &name),
                    kind: SymbolKind::Field,
                    visibility: access,
                    scope: ctx.scope,
                    span: self.span(declarator),
                    line_end,
                    is_abstract: false,
                    signature: None,
                });
            }
        }
    }

    // --- enums ---

    fn walk_enum(&mut self, node: Node<'_>, ctx: &Ctx) {
        if let Some(name) = self.field_text(node, "name") {
            let fqn = join(&ctx.fqn_prefix, &name);
            self.push_simple_def(SymbolDef {
                fqn,
                kind: SymbolKind::Type,
                visibility: Visibility::Public,
                scope: ctx.scope,
                span: self.span(node.child_by_field_name("name").unwrap_or(node)),
                line_end: self.end_line(node),
                is_abstract: false,
                signature: None,
            });
        }
        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for e in body.children(&mut cursor) {
                if e.kind() != "enumerator" {
                    continue;
                }
                if let Some(cname) = self.field_text(e, "name") {
                    self.push_simple_def(SymbolDef {
                        fqn: join(&ctx.fqn_prefix, &cname),
                        kind: SymbolKind::Constant,
                        visibility: Visibility::Public,
                        scope: ctx.scope,
                        span: self.span(e),
                        line_end: self.end_line(e),
                        is_abstract: false,
                        signature: None,
                    });
                }
            }
        }
    }

    // --- callables (free functions, methods, ctors/dtors, out-of-line defs) ---

    fn walk_function(&mut self, node: Node<'_>, ctx: &Ctx, is_member: bool) {
        self.walk_function_vis(node, ctx, is_member, Visibility::Public);
    }

    fn walk_function_vis(
        &mut self,
        node: Node<'_>,
        ctx: &Ctx,
        is_member: bool,
        vis: Visibility,
    ) {
        let Some(fdecl) = find_function_declarator(node.child_by_field_name("declarator")) else {
            // Not a function (e.g. a bare global `declaration`): fall back.
            if !is_member {
                self.walk_declaration(node, ctx, &mut 0u32);
            }
            return;
        };
        let Some(name_node) = callable_name_node(self, fdecl) else {
            return;
        };
        // The FQN tail as written: a bare name, a `~Dtor`, an `operator+`, or a
        // qualified `Class::method` (out-of-line definition). `::` already matches
        // the cgx FQN separator, so a qualified tail joins directly.
        let tail = self.text(name_node);
        let fqn = join(&ctx.fqn_prefix, &tail);
        let short = short_of(&tail).to_string();
        let has_body = node.child_by_field_name("body").is_some();
        let is_abstract = is_pure_virtual(node);
        let signature = self.build_signature(fdecl, node, ctx);
        // An out-of-line definition names its owner (`Class::method`) with a
        // `qualified_identifier` — it is a member function even though it sits at
        // namespace scope, so classify it as a method too.
        let kind = if is_member || name_node.kind() == "qualified_identifier" {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };

        let body_scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.push_callable_def(
            SymbolDef {
                fqn: fqn.clone(),
                kind,
                visibility: vis,
                scope: ctx.scope,
                span: self.span(name_node),
                line_end: self.end_line(node),
                is_abstract,
                signature: Some(signature),
            },
            has_body,
        );

        if !is_member && short == "main" {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.clone(),
                kind: EntrypointKind::Main,
            });
        }

        // Record the method for the override pass (in-class declarations only; an
        // out-of-line definition is not a member here and the in-class decl already
        // recorded the override candidacy).
        if is_member {
            let arity = signature_arity(node);
            let has_override = has_virtual_specifier(node, fdecl);
            let span = self.span(name_node);
            self.type_methods
                .entry(ctx.fqn_prefix.clone())
                .or_default()
                .push(MethodRec {
                    name: short.clone(),
                    fqn: fqn.clone(),
                    arity,
                    has_override,
                    span,
                });
            let simple = short_of(&ctx.fqn_prefix).to_string();
            self.type_method_sigs
                .entry(simple)
                .or_default()
                .insert((short.clone(), arity));
        }

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, body_scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
        }
    }

    /// Build a best-effort [`Signature`] (ADR-04): parameter types as written (the
    /// overload key Phase F keys on), the return type, and any enclosing template's
    /// type-params. Names/types are recorded as written, never inferred.
    fn build_signature(&self, fdecl: Node<'_>, node: Node<'_>, ctx: &Ctx) -> Signature {
        let mut params = Vec::new();
        if let Some(plist) = fdecl.child_by_field_name("parameters") {
            let mut cursor = plist.walk();
            for p in plist.children(&mut cursor) {
                match p.kind() {
                    "parameter_declaration" | "optional_parameter_declaration" => {
                        let type_text = self.field_text(p, "type");
                        let name = base_declarator_name(self, p.child_by_field_name("declarator"))
                            .unwrap_or_default();
                        params.push(Param {
                            name,
                            type_text,
                            has_default: p.kind() == "optional_parameter_declaration",
                            variadic: false,
                        });
                    }
                    "variadic_parameter_declaration" | "..." => {
                        params.push(Param {
                            name: String::new(),
                            type_text: None,
                            has_default: false,
                            variadic: true,
                        });
                    }
                    _ => {}
                }
            }
        }
        Signature {
            params,
            return_type_text: self.field_text(node, "type"),
            type_params: ctx.type_params.iter().cloned().collect(),
            receiver: None,
        }
    }

    // --- templates ---

    fn walk_template(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        // Record the template's type-params and recurse into the templated
        // declaration. Only edges that exist pre-instantiation are emitted; a
        // template-dependent call is a member call → candidate set → `possible`.
        let tps = self.template_type_params(node);
        let inner_ctx = ctx.with_type_params(tps);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if matches!(
                child.kind(),
                "template_parameter_list" | "template" | "<" | ">"
            ) {
                continue;
            }
            self.walk(child, &inner_ctx, stmt_index);
        }
    }

    fn template_type_params(&self, node: Node<'_>) -> SmallVec<[String; 2]> {
        let mut out: SmallVec<[String; 2]> = SmallVec::new();
        if let Some(plist) = node.child_by_field_name("parameters") {
            let mut cursor = plist.walk();
            for p in plist.children(&mut cursor) {
                if matches!(
                    p.kind(),
                    "type_parameter_declaration"
                        | "optional_type_parameter_declaration"
                        | "variadic_type_parameter_declaration"
                        | "template_template_parameter_declaration"
                ) {
                    // The declared name is the trailing `type_identifier`.
                    let mut inner = p.walk();
                    let ids: Vec<Node<'_>> = p
                        .children(&mut inner)
                        .filter(|c| c.kind() == "type_identifier")
                        .collect();
                    if let Some(id) = ids.last() {
                        out.push(self.text(*id));
                    }
                }
            }
        }
        out
    }

    // --- C-style declarations (globals, prototypes, ctor/dtor decls) ---

    fn walk_declaration(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        // A declaration whose declarator descends to a function_declarator is a
        // function prototype / ctor / dtor declaration — a callable def (unless a
        // bare prototype we treat like C: a non-defining signature). We DO emit it
        // as a def so overloads and out-of-line targets have a node; a plain
        // prototype has no body and collapses into the definition by (fqn, sig).
        if find_function_declarator(node.child_by_field_name("declarator")).is_some() {
            self.walk_function_vis(node, ctx, false, Visibility::Public);
            // Recurse for any initializer / default-arg calls.
            self.walk_children(node, ctx, stmt_index);
            return;
        }
        // A declaration wrapping a `class`/`struct`/`union`/`enum` definition is a
        // type definition (e.g. `struct S { … } s;`): emit the type, then fall
        // through to record any declared instances as globals.
        if let Some(spec) = nested_type_specifier(node) {
            self.walk(spec, ctx, stmt_index);
        }
        // File-scope data declarations are globals; a body-local declaration is not
        // a symbol, but recurse either way for initializer calls.
        if ctx.scope == ScopeId::ROOT {
            let mut c = node.walk();
            for child in node.children(&mut c) {
                let declarator = match child.kind() {
                    "init_declarator" => child.child_by_field_name("declarator"),
                    "identifier" | "pointer_declarator" | "array_declarator"
                    | "reference_declarator" => Some(child),
                    _ => None,
                };
                let Some(declarator) = declarator else { continue };
                if let Some(name) = base_declarator_name(self, Some(declarator)) {
                    self.push_simple_def(SymbolDef {
                        fqn: join(&ctx.fqn_prefix, &name),
                        kind: SymbolKind::Variable,
                        visibility: Visibility::Public,
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

    fn walk_typedef(&mut self, node: Node<'_>, ctx: &Ctx) {
        if let Some(name) = base_declarator_name(self, node.child_by_field_name("declarator")) {
            self.push_simple_def(SymbolDef {
                fqn: join(&ctx.fqn_prefix, &name),
                kind: SymbolKind::Type,
                visibility: Visibility::Public,
                scope: ctx.scope,
                span: self.span(node.child_by_field_name("declarator").unwrap_or(node)),
                line_end: self.end_line(node),
                is_abstract: false,
                signature: None,
            });
        }
        if let Some(ty) = node.child_by_field_name("type") {
            if matches!(
                ty.kind(),
                "struct_specifier" | "union_specifier" | "enum_specifier" | "class_specifier"
            ) {
                self.walk(ty, ctx, &mut 0u32);
            }
        }
    }

    /// `using Alias = Target;` → a `Type` def for the alias name.
    fn walk_alias(&mut self, node: Node<'_>, ctx: &Ctx) {
        if let Some(name) = self.field_text(node, "name") {
            self.push_simple_def(SymbolDef {
                fqn: join(&ctx.fqn_prefix, &name),
                kind: SymbolKind::Type,
                visibility: Visibility::Public,
                scope: ctx.scope,
                span: self.span(node),
                line_end: self.end_line(node),
                is_abstract: false,
                signature: None,
            });
        }
    }

    /// `using ns::name;` → a named import; `using namespace ns;` → a glob import.
    /// The target namespace/scope is left for the shared resolver; this never
    /// resolves a file itself.
    fn walk_using(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut cursor = node.walk();
        let is_namespace = node.children(&mut cursor).any(|c| c.kind() == "namespace");
        let mut cursor = node.walk();
        let qid = node.children(&mut cursor).find(|c| {
            matches!(
                c.kind(),
                "qualified_identifier" | "identifier" | "namespace_identifier"
            )
        });
        let Some(qid) = qid else {
            return;
        };
        let full = self.text(qid);
        let (specifier, names, glob) = if is_namespace {
            (full, Vec::new(), true)
        } else {
            let last = short_of(&full).to_string();
            let spec = full
                .rfind("::")
                .map(|i| full[..i].to_string())
                .unwrap_or_default();
            (
                spec,
                vec![ImportedName {
                    name: last,
                    alias: None,
                }],
                false,
            )
        };
        self.facts.imports.push(ImportFact {
            specifier,
            names,
            glob,
            re_export: false,
            scope: ctx.scope,
            span: self.span(node),
        });
    }

    /// `namespace alias = target;` → a glob import of `target` under the alias.
    fn walk_ns_alias(&mut self, node: Node<'_>, ctx: &Ctx) {
        let alias = self.field_text(node, "name");
        let target = {
            let mut cursor = node.walk();
            let found = node
                .children(&mut cursor)
                .find(|c| {
                    matches!(
                        c.kind(),
                        "nested_namespace_specifier"
                            | "namespace_identifier"
                            | "qualified_identifier"
                    )
                })
                .map(|c| self.text(c));
            found
        };
        if let (Some(_alias), Some(target)) = (alias, target) {
            self.facts.imports.push(ImportFact {
                specifier: target,
                names: Vec::new(),
                glob: true,
                re_export: false,
                scope: ctx.scope,
                span: self.span(node),
            });
        }
    }

    fn walk_macro_def(&mut self, node: Node<'_>) {
        if let Some(name) = self.field_text(node, "name") {
            self.push_simple_def(SymbolDef {
                fqn: name,
                kind: SymbolKind::Macro,
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
            "system_lib_string" => {
                let t = self.text(path_node);
                t.trim_start_matches('<').trim_end_matches('>').to_string()
            }
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
        self.facts.imports.push(ImportFact {
            specifier,
            names: Vec::<ImportedName>::new(),
            glob: true,
            re_export: false,
            scope: ctx.scope,
            span: self.span(node),
        });
    }

    // --- references (calls / constructions) ---

    fn walk_call(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(func) = node.child_by_field_name("function") {
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

    /// `new T(args)` → an [`RefKind::Instantiate`] keyed by the simple type name.
    fn walk_new(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(ty) = node.child_by_field_name("type") {
            let name = type_name(&self.text(ty));
            if !name.is_empty() {
                self.record_effects(ctx, effects_of_call(&name));
                self.push_ref(
                    smallvec_one(name),
                    RefKind::Instantiate,
                    ctx,
                    self.span(node),
                    *stmt_index,
                    None,
                );
                *stmt_index += 1;
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    /// Classify a call's `function` operand into `(name_path, RefKind)`:
    /// - bare `identifier` → direct `Call`;
    /// - `qualified_identifier` (`ns::f`, `Type::f`) → direct `Call`, segmented;
    /// - `field_expression` (`obj.m`, `ptr->m`) → `CallVirtualReceiver` (the Java
    ///   shape — the shared size-only CHA band applies by construction);
    /// - `template_function` (`f<T>()`) → classify its `name` operand;
    /// - `(*fp)(…)` / `*fp(…)` → indirect `CallCallback` (`possible`).
    fn classify_callee(&self, func: Node<'_>) -> (SmallVec<[Name; 2]>, RefKind) {
        match func.kind() {
            "identifier" => (smallvec_one(self.text(func)), RefKind::Call),
            "qualified_identifier" => (split_qualified(&self.text(func)), RefKind::Call),
            "field_expression" => {
                let mut segs = func
                    .child_by_field_name("argument")
                    .map(|a| self.receiver_segments(a))
                    .unwrap_or_default();
                match func.child_by_field_name("field") {
                    Some(field) => {
                        segs.push(self.text(field));
                        (segs, RefKind::CallVirtualReceiver)
                    }
                    None => (SmallVec::new(), RefKind::CallVirtualReceiver),
                }
            }
            "template_function" => match func.child_by_field_name("name") {
                Some(name) => self.classify_callee(name),
                None => (SmallVec::new(), RefKind::Call),
            },
            "parenthesized_expression" => func
                .named_child(0)
                .map(|inner| {
                    let (path, _) = self.classify_callee(inner);
                    (path, RefKind::CallCallback)
                })
                .unwrap_or((SmallVec::new(), RefKind::CallCallback)),
            "pointer_expression" => {
                let path = func
                    .child_by_field_name("argument")
                    .map(|a| self.callee_ident(a))
                    .unwrap_or_default();
                (path, RefKind::CallCallback)
            }
            _ => (self.callee_ident(func), RefKind::CallCallback),
        }
    }

    /// The segment path of a call receiver (`a.b->c` → `[a, b, c]`).
    fn receiver_segments(&self, node: Node<'_>) -> SmallVec<[Name; 2]> {
        match node.kind() {
            "identifier" | "field_identifier" | "type_identifier" | "namespace_identifier" => {
                smallvec_one(self.text(node))
            }
            "this" => smallvec_one("this".to_string()),
            "qualified_identifier" => split_qualified(&self.text(node)),
            "field_expression" => {
                let mut out = node
                    .child_by_field_name("argument")
                    .map(|a| self.receiver_segments(a))
                    .unwrap_or_default();
                if let Some(field) = node.child_by_field_name("field") {
                    out.push(self.text(field));
                }
                out
            }
            "call_expression" => node
                .child_by_field_name("function")
                .map(|f| self.receiver_segments(f))
                .unwrap_or_default(),
            "parenthesized_expression" => node
                .named_child(0)
                .map(|n| self.receiver_segments(n))
                .unwrap_or_default(),
            "pointer_expression" => node
                .child_by_field_name("argument")
                .map(|a| self.receiver_segments(a))
                .unwrap_or_default(),
            "subscript_expression" => node
                .child_by_field_name("argument")
                .map(|a| self.receiver_segments(a))
                .unwrap_or_default(),
            _ => SmallVec::new(),
        }
    }

    /// Best-effort leading identifier of an arbitrary callee expression.
    fn callee_ident(&self, node: Node<'_>) -> SmallVec<[Name; 2]> {
        match node.kind() {
            "identifier" | "field_identifier" | "type_identifier" => smallvec_one(self.text(node)),
            "qualified_identifier" => split_qualified(&self.text(node)),
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
        name_path: SmallVec<[Name; 2]>,
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

    fn owner_fqn(&self, scope: ScopeId) -> Option<String> {
        let mut cur = Some(scope);
        while let Some(id) = cur {
            let s = self.facts.scopes.scopes.get(id.index())?;
            if let Some(owner) = &s.owner_fqn {
                // A namespace/type scope owner is not a callable; keep walking up
                // only matters for call attribution, which always sits in a
                // callable body scope — the nearest owner is the callable.
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
        if let Some(c) = node.child_by_field_name("condition") {
            self.walk(c, ctx, stmt_index);
        }
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            let inner = ctx.with_condition(EdgeCondition::Conditional);
            self.walk(body, &inner, stmt_index);
        }
    }

    /// `try { … } catch (…) { … }`: the try block is unguarded; each `catch` body
    /// carries [`EdgeCondition::Exception`] so call edges inside a handler are
    /// exception-path edges (the Java-grade throw/catch model — no types needed).
    fn walk_try(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(body) = node.child_by_field_name("body") {
            self.walk(body, ctx, stmt_index);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "catch_clause" {
                let inner = ctx.with_condition(EdgeCondition::Exception);
                if let Some(cb) = child.child_by_field_name("body") {
                    self.walk(cb, &inner, stmt_index);
                }
                // The exception-declaration (parameters) may hold calls too, but is
                // not a guarded site; walk it unguarded.
                if let Some(params) = child.child_by_field_name("parameters") {
                    self.walk(params, ctx, stmt_index);
                }
            }
        }
    }
}

// === free functions ===

fn smallvec_one(s: String) -> SmallVec<[Name; 2]> {
    let mut v = SmallVec::new();
    v.push(s);
    v
}

/// Segments of a `::`-qualified name (`geo::helper` → `[geo, helper]`).
fn split_qualified(text: &str) -> SmallVec<[Name; 2]> {
    text.split("::")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Join an FQN prefix and a (possibly already `::`-qualified) tail.
fn join(prefix: &str, tail: &str) -> String {
    if prefix.is_empty() {
        tail.to_string()
    } else {
        format!("{prefix}::{tail}")
    }
}

/// The last `::`-segment of a name (`geo::Circle::area` → `area`).
fn short_of(fqn: &str) -> &str {
    fqn.rsplit("::").next().unwrap_or(fqn)
}

/// The simple type name of a (possibly qualified/templated/pointer) type text
/// (`geo::Circle*` → `Circle`, `std::vector<int>` → `vector`).
fn type_name(text: &str) -> String {
    let base = text.split(['<', '*', '&', ' ']).next().unwrap_or(text);
    short_of(base.trim()).to_string()
}

/// The FQN split into `::` segments.
fn fqn_segments(fqn: &str) -> SmallVec<[Name; 2]> {
    fqn.split("::").map(str::to_string).collect()
}

fn name_path_two(a: &str, b: &str) -> SmallVec<[Name; 2]> {
    let mut v = SmallVec::new();
    v.push(a.to_string());
    v.push(b.to_string());
    v
}

/// The number of declared formal parameters of a callable node's
/// function_declarator (used for name+arity override matching).
fn signature_arity(node: Node<'_>) -> usize {
    let Some(fdecl) = find_function_declarator(node.child_by_field_name("declarator")) else {
        return 0;
    };
    let Some(plist) = fdecl.child_by_field_name("parameters") else {
        return 0;
    };
    let mut cursor = plist.walk();
    plist
        .children(&mut cursor)
        .filter(|c| {
            matches!(
                c.kind(),
                "parameter_declaration"
                    | "optional_parameter_declaration"
                    | "variadic_parameter_declaration"
            )
        })
        .count()
}

/// Whether a method declaration is pure-virtual (`virtual T f() = 0;`) — detected
/// by a trailing `= 0` default on the declaration.
fn is_pure_virtual(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let mut saw_eq = false;
    for c in node.children(&mut cursor) {
        if c.kind() == "=" {
            saw_eq = true;
        } else if saw_eq && c.kind() == "number_literal" {
            return true;
        }
    }
    false
}

/// Whether a method carries an `override`/`final` virtual specifier (authoritative
/// override signal, the C++ analogue of Java's `@Override`).
fn has_virtual_specifier(node: Node<'_>, fdecl: Node<'_>) -> bool {
    let has = |n: Node<'_>| {
        let mut cursor = n.walk();
        let found = n
            .children(&mut cursor)
            .any(|c| c.kind() == "virtual_specifier");
        found
    };
    has(fdecl) || has(node)
}

/// Descend a `function_declarator`'s name chain to the node naming the callable:
/// a plain `identifier` (free fn / ctor), a `field_identifier` (in-class method),
/// a `qualified_identifier` (out-of-line `Class::method`), a `destructor_name`
/// (`~T`), or an `operator_name`.
fn callable_name_node<'t>(_b: &Builder<'_>, fdecl: Node<'t>) -> Option<Node<'t>> {
    let mut cur = fdecl.child_by_field_name("declarator");
    while let Some(n) = cur {
        match n.kind() {
            "identifier" | "field_identifier" | "qualified_identifier" | "destructor_name"
            | "operator_name" | "operator_cast" => return Some(n),
            k if DECLARATOR_KINDS.contains(&k) => cur = descend(n),
            _ => return None,
        }
    }
    None
}

/// A nested `class`/`struct`/`union`/`enum` specifier directly inside a
/// `field_declaration`/`declaration` (a member type definition), if any.
fn nested_type_specifier<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|c| {
        matches!(
            c.kind(),
            "class_specifier" | "struct_specifier" | "union_specifier" | "enum_specifier"
        ) && c.child_by_field_name("body").is_some()
    });
    found
}

/// The next declarator to descend into (handles `parenthesized_declarator`, whose
/// inner declarator is an UNNAMED child).
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
    "reference_declarator",
    "init_declarator",
];

/// The base identifier text at the bottom of a declarator chain.
fn base_declarator_name(b: &Builder<'_>, node: Option<Node<'_>>) -> Option<String> {
    let mut cur = node;
    while let Some(n) = cur {
        match n.kind() {
            "identifier" | "field_identifier" | "type_identifier" => return Some(b.text(n)),
            "qualified_identifier" | "destructor_name" | "operator_name" => return Some(b.text(n)),
            k if DECLARATOR_KINDS.contains(&k) => cur = descend(n),
            "function_declarator" => cur = n.child_by_field_name("declarator"),
            _ => return None,
        }
    }
    None
}

/// The first `function_declarator` along a declarator chain, if any.
fn find_function_declarator(node: Option<Node<'_>>) -> Option<Node<'_>> {
    let mut cur = node;
    while let Some(n) = cur {
        match n.kind() {
            "function_declarator" => return Some(n),
            "pointer_declarator" | "parenthesized_declarator" | "array_declarator"
            | "reference_declarator" | "init_declarator" => cur = descend(n),
            _ => return None,
        }
    }
    None
}
