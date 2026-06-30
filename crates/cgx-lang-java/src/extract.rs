//! The Java extractor: a recursive walk over the `tree-sitter-java` parse tree
//! that builds [`FileFacts`].
//!
//! ## Cycle scope
//! Cycle 1 emitted **defs + scopes** only. **Cycle 2** adds the
//! reference/import/hint channels:
//! - **call refs** ([`RawRef`]) for `method_invocation` and
//!   `object_creation_expression`, with the ADR-03 edge condition lowered from
//!   the enclosing `if`/loop/`catch`/`finally`/`switch`/ternary chain;
//! - **imports** ([`ImportFact`]) for `import_declaration` (static + `.*`
//!   wildcard); **exports** ([`ExportFact`]) for `public` declarations;
//! - **entrypoint hints** for `public static void main` and JUnit `@Test`
//!   (plus name-based JUnit3 `testXxx`);
//! - **cut hints** for `native` methods (JNI/FFI boundary), `Class.forName`
//!   (reflective) and `ClassLoader.loadClass` (dynamic);
//! - the Cycle-1 leftovers **record components** and **enum constants** as defs.
//!
//! Inheritance/overrides, effects, and dataflow remain stubs (Cycles 3–5).
//!
//! Unlike the Go adapter, the FQN prefix comes from the source
//! `package_declaration` node (see [`crate::module`]), not the file path.

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::{
    CutHint, EntrypointHint, ExportFact, FileCtx, FileFacts, FrontendError, ImportFact,
    ImportedName, Lang, LanguageFrontend, Name, RawRef, RefKind, RelPath, ScopeId, SymbolDef,
};
use smallvec::SmallVec;
use tree_sitter::{Node, Parser};

use crate::module::module_path_from_package;

/// The Java language adapter. Stateless; one instance handles every `.java` file.
#[derive(Debug, Default, Clone)]
pub struct JavaFrontend;

impl JavaFrontend {
    pub fn new() -> Self {
        JavaFrontend
    }
}

/// Version of the Java extraction rules; bumping invalidates cached fragments.
const JAVA_FRAGMENT_VERSION: u32 = 1;

impl LanguageFrontend for JavaFrontend {
    fn lang(&self) -> Lang {
        Lang::Java
    }

    fn handles(&self, path: &RelPath) -> bool {
        path.extension().as_deref() == Some("java")
    }

    fn fragment_version(&self) -> u32 {
        JAVA_FRAGMENT_VERSION
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .map_err(|e| FrontendError::Parser {
                lang: "java".to_string(),
                detail: e.to_string(),
            })?;

        let tree = match parser.parse(src, None) {
            Some(tree) => tree,
            None => return Ok(FileFacts::empty()),
        };

        let root = tree.root_node();
        let mut builder = Builder::new(src, ctx.path.as_str());
        // Java's FQN prefix is the declared package, read off the AST — not the
        // file path. A file with no `package` lives in the default package, whose
        // prefix is empty (top-level types are FQN'd by their bare class name).
        let pkg_prefix = builder.read_package_prefix(root).unwrap_or_default();

        let root_ctx = Ctx::root(pkg_prefix);
        let mut stmt_index = 0u32;
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            builder.walk(child, &root_ctx, &mut stmt_index);
        }
        Ok(builder.finish())
    }
}

/// Walk context threaded down the tree: the FQN prefix a def lives under, the
/// lexical scope id refs/defs are recorded in, and the ordered chain of enclosing
/// guarding constructs whose ADR-03 maximum is a call site's edge condition.
#[derive(Clone)]
struct Ctx {
    fqn_prefix: String,
    scope: ScopeId,
    conditions: SmallVec<[EdgeCondition; 4]>,
}

impl Ctx {
    fn root(prefix: String) -> Self {
        Ctx {
            fqn_prefix: prefix,
            scope: ScopeId::ROOT,
            conditions: SmallVec::new(),
        }
    }

    /// Enter a type/callable body: the body's FQN becomes the prefix for its
    /// members, the body's scope becomes their parent scope, and the condition
    /// chain resets (a body root is unguarded).
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

    /// The single ADR-03 edge condition for a call site under this chain.
    fn condition(&self) -> EdgeCondition {
        EdgeCondition::resolve(self.conditions.iter().copied())
    }
}

struct Builder<'a> {
    src: &'a [u8],
    file: String,
    facts: FileFacts,
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], file: &str) -> Self {
        Builder {
            src,
            file: file.to_string(),
            facts: FileFacts::empty(),
        }
    }

    fn finish(self) -> FileFacts {
        // Cycle 3+: impl_relations / effects / dataflow are flushed here once
        // those cycles accumulate them. Canonicalization is the registry's job.
        self.facts
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

    /// Span anchored at a def's `name` field when present (else the whole node).
    fn def_span(&self, node: Node<'_>, name_field: &str) -> Span {
        node.child_by_field_name(name_field)
            .map(|n| self.span(n))
            .unwrap_or_else(|| self.span(node))
    }

    /// The dotted package name from the file's `package_declaration`, lowered to a
    /// `::`-joined FQN prefix; `None` when the file is in the default package.
    fn read_package_prefix(&self, root: Node<'_>) -> Option<String> {
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() == "package_declaration" {
                let mut inner = child.walk();
                for c in child.children(&mut inner) {
                    if matches!(c.kind(), "identifier" | "scoped_identifier") {
                        return Some(module_path_from_package(&self.text(c)));
                    }
                }
            }
        }
        None
    }

    // --- dispatch ---

    fn walk(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        match node.kind() {
            "class_declaration" | "interface_declaration" | "enum_declaration"
            | "record_declaration" => self.walk_type(node, ctx),
            "method_declaration" => self.walk_method(node, ctx),
            "constructor_declaration" => self.walk_constructor(node, ctx),
            "field_declaration" => self.walk_field(node, ctx),
            "enum_constant" => self.walk_enum_constant(node, ctx),
            "import_declaration" => self.walk_import(node, ctx),
            "method_invocation" => self.walk_invocation(node, ctx, stmt_index),
            "object_creation_expression" => self.walk_object_creation(node, ctx, stmt_index),
            "if_statement" => self.walk_if(node, ctx, stmt_index),
            "ternary_expression" => self.walk_ternary(node, ctx, stmt_index),
            "while_statement" | "for_statement" | "enhanced_for_statement" | "do_statement" => {
                self.walk_loop(node, ctx, stmt_index)
            }
            "switch_expression" => self.walk_switch(node, ctx, stmt_index),
            "try_statement" | "try_with_resources_statement" => self.walk_try(node, ctx, stmt_index),
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

    /// Class, interface, enum, or record. Emits a `Type` def, opens a child scope,
    /// then walks the type body so members and nested types nest under its FQN.
    fn walk_type(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        // An interface is always abstract; a class is abstract iff it carries the
        // `abstract` modifier; enums and records never are.
        let is_abstract = node.kind() == "interface_declaration" || has_modifier(node, "abstract");
        let vis = vis_from_modifiers(node);
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Type,
            visibility: vis,
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract,
            signature: None,
        });
        self.maybe_export(&name, self.def_span(node, "name"), vis);

        let body_ctx = ctx.enter_body(fqn, scope);
        // Record components (`record P(int x, int y)`) live in the `parameters`
        // field, not the body; surface each as a (private backing) Field def.
        if node.kind() == "record_declaration" {
            self.emit_record_components(node, &body_ctx);
        }
        if let Some(body) = node.child_by_field_name("body") {
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
        }
        // Cycle 3: superclass/super_interfaces/extends_interfaces → ImplRelation.
    }

    /// Each `formal_parameter` of a record header → a `Field` def (the synthesized
    /// private final backing field). The public canonical accessor methods are
    /// deferred (Cycle 3+).
    fn emit_record_components(&mut self, node: Node<'_>, body_ctx: &Ctx) {
        let Some(params) = node.child_by_field_name("parameters") else {
            return;
        };
        let mut cursor = params.walk();
        for p in params.children(&mut cursor) {
            if p.kind() != "formal_parameter" {
                continue;
            }
            let Some(name_node) = p.child_by_field_name("name") else {
                continue;
            };
            self.facts.defs.push(SymbolDef {
                fqn: join(&body_ctx.fqn_prefix, &self.text(name_node)),
                kind: SymbolKind::Field,
                visibility: Visibility::Private,
                scope: body_ctx.scope,
                span: self.span(name_node),
                line_end: self.end_line(p),
                is_abstract: false,
                signature: None,
            });
        }
    }

    fn walk_method(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        // A method with no `body` field is a declaration only: interface methods
        // (non-default) and `abstract` methods.
        let is_abstract =
            node.child_by_field_name("body").is_none() || has_modifier(node, "abstract");
        let vis = vis_from_modifiers(node);
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Method,
            visibility: vis,
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract,
            signature: None,
        });
        self.maybe_export(&name, self.def_span(node, "name"), vis);
        self.record_entrypoints(node, &name, &fqn);
        // A `native` method's body lives across the JNI boundary the call graph
        // cannot see into — the Java analogue of Go's `import "C"` FFI cut.
        if has_modifier(node, "native") {
            self.facts.cut_hints.push(CutHint {
                marker: CutMarker::ViaFfi,
                span: self.def_span(node, "name"),
                macro_origin: None,
            });
        }

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
        }
    }

    fn walk_constructor(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        let vis = vis_from_modifiers(node);
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Function,
            visibility: vis,
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
        self.maybe_export(&name, self.def_span(node, "name"), vis);

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope);
            let mut inner = 0u32;
            self.walk(body, &body_ctx, &mut inner);
        }
    }

    /// A `field_declaration` may declare several variables (`int a, b;`); emit one
    /// `Field` def per `variable_declarator`. Visibility lives on the declaration's
    /// `modifiers`, shared by every declarator.
    fn walk_field(&mut self, node: Node<'_>, ctx: &Ctx) {
        let vis = vis_from_modifiers(node);
        let line_end = self.end_line(node);
        let mut cursor = node.walk();
        for decl in node.children_by_field_name("declarator", &mut cursor) {
            let Some(name_node) = decl.child_by_field_name("name") else {
                continue;
            };
            let name = self.text(name_node);
            self.facts.defs.push(SymbolDef {
                fqn: join(&ctx.fqn_prefix, &name),
                kind: SymbolKind::Field,
                visibility: vis,
                scope: ctx.scope,
                span: self.span(name_node),
                line_end,
                is_abstract: false,
                signature: None,
            });
            self.maybe_export(&name, self.span(name_node), vis);
        }
    }

    /// An `enum_constant` (`RED`, `GREEN`) is an implicit `public static final`
    /// member — emit it as a `Constant` def.
    fn walk_enum_constant(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        self.facts.defs.push(SymbolDef {
            fqn: join(&ctx.fqn_prefix, &name),
            kind: SymbolKind::Constant,
            visibility: Visibility::Public,
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
        self.maybe_export(&name, self.def_span(node, "name"), Visibility::Public);
    }

    fn maybe_export(&mut self, name: &str, span: Span, vis: Visibility) {
        if vis == Visibility::Public {
            self.facts.exports.push(ExportFact {
                name: name.to_string(),
                alias: None,
                from: None,
                span,
            });
        }
    }

    // --- imports / exports ---

    /// `import [static] a.b.C [.*];` → one [`ImportFact`]. The specifier is the
    /// dotted package/type path as written; a `.*` on-demand import is `glob`; a
    /// single-type/static import contributes its last segment as the bound name.
    /// (Java's static-vs-type distinction is not separately modelled — `ImportFact`
    /// has no such field; both record the dotted specifier.)
    fn walk_import(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut glob = false;
        let mut name_node = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "asterisk" => glob = true,
                "identifier" | "scoped_identifier" => name_node = Some(child),
                _ => {}
            }
        }
        let Some(name_node) = name_node else {
            return;
        };
        let specifier = self.text(name_node);
        let names = if glob {
            Vec::new()
        } else {
            let last = specifier
                .rsplit('.')
                .next()
                .unwrap_or(&specifier)
                .to_string();
            vec![ImportedName {
                name: last,
                alias: None,
            }]
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

    // --- references (calls) ---

    /// A `method_invocation` `recv.m(args)` / `m(args)`. With an `object`
    /// (receiver) the callee is a [`RefKind::CallVirtualReceiver`] whose
    /// `name_path` is the receiver segments plus the method name; a bare call is a
    /// [`RefKind::Call`]. The receiver/arguments are then walked for nested calls.
    fn walk_invocation(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        self.emit_call_cut_hints(node);
        if let Some(name_node) = node.child_by_field_name("name") {
            let name = self.text(name_node);
            let (name_path, kind) = match node.child_by_field_name("object") {
                Some(obj) => {
                    let mut segs = self.receiver_segments(obj);
                    segs.push(name);
                    (segs, RefKind::CallVirtualReceiver)
                }
                None => (smallvec_one(name), RefKind::Call),
            };
            if !name_path.is_empty() {
                self.push_ref(name_path, kind, ctx, self.span(node), *stmt_index);
                *stmt_index += 1;
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    /// A `new T(args)` construction → [`RefKind::Instantiate`] keyed by the simple
    /// type name. Arguments (and any anonymous-class body, deferred) are walked for
    /// nested calls.
    fn walk_object_creation(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(ty) = node.child_by_field_name("type") {
            let name = type_name(&self.text(ty));
            if !name.is_empty() {
                self.push_ref(
                    smallvec_one(name),
                    RefKind::Instantiate,
                    ctx,
                    self.span(node),
                    *stmt_index,
                );
                *stmt_index += 1;
            }
        }
        self.walk_children(node, ctx, stmt_index);
    }

    /// The segment path of a `method_invocation` receiver (`a.b.c` → `[a, b, c]`),
    /// recursing through field-access / chained-call / scoped operands.
    fn receiver_segments(&self, node: Node<'_>) -> SmallVec<[Name; 2]> {
        match node.kind() {
            "identifier" | "type_identifier" => smallvec_one(self.text(node)),
            "this" => smallvec_one("this".to_string()),
            "super" => smallvec_one("super".to_string()),
            "field_access" => {
                let mut out = node
                    .child_by_field_name("object")
                    .map(|o| self.receiver_segments(o))
                    .unwrap_or_default();
                if let Some(field) = node.child_by_field_name("field") {
                    out.push(self.text(field));
                }
                out
            }
            "method_invocation" => {
                let mut out = node
                    .child_by_field_name("object")
                    .map(|o| self.receiver_segments(o))
                    .unwrap_or_default();
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(self.text(name));
                }
                out
            }
            "scoped_identifier" => self.text(node).split('.').map(str::to_string).collect(),
            "parenthesized_expression" => node
                .named_child(0)
                .map(|n| self.receiver_segments(n))
                .unwrap_or_default(),
            _ => SmallVec::new(),
        }
    }

    /// `Class.forName(..)` → reflective cut; `*.loadClass(..)` → dynamic cut. The
    /// single-file Java analogues of Go's `reflect.*` / `plugin.*` package cuts
    /// (no import resolution here, so keyed off well-known reflective/dynamic API
    /// method names).
    fn emit_call_cut_hints(&mut self, node: Node<'_>) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let marker = match name.as_str() {
            "forName"
                if node
                    .child_by_field_name("object")
                    .map(|o| self.receiver_segments(o).last() == Some(&"Class".to_string()))
                    .unwrap_or(false) =>
            {
                Some(CutMarker::Reflective)
            }
            "loadClass" => Some(CutMarker::Dynamic),
            _ => None,
        };
        if let Some(marker) = marker {
            self.facts.cut_hints.push(CutHint {
                marker,
                span: self.span(node),
                macro_origin: None,
            });
        }
    }

    fn push_ref(
        &mut self,
        name_path: SmallVec<[Name; 2]>,
        kind: RefKind,
        ctx: &Ctx,
        span: Span,
        stmt_index: u32,
    ) {
        self.facts.refs.push(RawRef {
            name_path,
            scope: ctx.scope,
            kind,
            edge_condition: ctx.condition(),
            implicit: None,
            span,
            stmt_index,
            arity: None,
            cut_markers: SmallVec::new(),
        });
    }

    // --- edge conditions ---

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

    fn walk_ternary(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(c) = node.child_by_field_name("condition") {
            self.walk(c, ctx, stmt_index);
        }
        let inner = ctx.with_condition(EdgeCondition::Conditional);
        for field in ["consequence", "alternative"] {
            if let Some(b) = node.child_by_field_name(field) {
                self.walk(b, &inner, stmt_index);
            }
        }
    }

    fn walk_loop(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            let inner = ctx.with_condition(EdgeCondition::Loop);
            self.walk(body, &inner, stmt_index);
        }
        // Walk any non-body children (init/condition/update/range value) unguarded.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == body {
                continue;
            }
            self.walk(child, ctx, stmt_index);
        }
    }

    fn walk_switch(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        // The scrutinee (`condition`) is unguarded; every case body is conditional.
        if let Some(c) = node.child_by_field_name("condition") {
            self.walk(c, ctx, stmt_index);
        }
        if let Some(body) = node.child_by_field_name("body") {
            let inner = ctx.with_condition(EdgeCondition::Conditional);
            self.walk(body, &inner, stmt_index);
        }
    }

    /// `try`/`try`-with-resources. The try block (and resources) are unguarded;
    /// each `catch` body is [`EdgeCondition::Exception`]; the `finally` block is
    /// **always** (GM-3.1 carve-out — a finally edge is taken on entry regardless
    /// of how the protected block exits), so it gets no extra condition.
    fn walk_try(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(res) = node.child_by_field_name("resources") {
            self.walk(res, ctx, stmt_index);
        }
        if let Some(body) = node.child_by_field_name("body") {
            self.walk(body, ctx, stmt_index);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "catch_clause" => {
                    if let Some(cb) = child.child_by_field_name("body") {
                        let inner = ctx.with_condition(EdgeCondition::Exception);
                        self.walk(cb, &inner, stmt_index);
                    }
                }
                "finally_clause" => self.walk_children(child, ctx, stmt_index),
                _ => {}
            }
        }
    }

    /// The last-segment names of the annotations on a declaration (`@org.junit.Test`
    /// → `"Test"`). Annotations sit as `marker_annotation`/`annotation` children of
    /// the `modifiers` node, each with a `name` field
    /// (`identifier`/`scoped_identifier`).
    fn annotation_names(&self, node: Node<'_>) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() != "modifiers" {
                continue;
            }
            let mut inner = child.walk();
            for m in child.children(&mut inner) {
                if matches!(m.kind(), "marker_annotation" | "annotation") {
                    if let Some(name) = m.child_by_field_name("name") {
                        let text = self.text(name);
                        let last = text.rsplit('.').next().unwrap_or(&text).to_string();
                        out.push(last);
                    }
                }
            }
        }
        out
    }

    // --- entrypoints ---

    fn record_entrypoints(&mut self, node: Node<'_>, name: &str, fqn: &str) {
        let kind = if name == "main" && has_modifier(node, "static") {
            Some(EntrypointKind::Main)
        } else if self.annotation_names(node).iter().any(|a| a == "Test") || is_junit3_test(name) {
            Some(EntrypointKind::Test)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.to_string(),
                kind,
            });
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

fn smallvec_one(s: String) -> SmallVec<[Name; 2]> {
    let mut v = SmallVec::new();
    v.push(s);
    v
}

/// The simple type name of a `new`-expression type operand: drop any generic
/// arguments (`Foo<T>` → `Foo`) and package qualification (`a.b.Foo` → `Foo`).
fn type_name(text: &str) -> String {
    let base = text.split('<').next().unwrap_or(text).trim();
    base.rsplit('.').next().unwrap_or(base).trim().to_string()
}

/// The modifier-keyword kinds on a declaration. Java groups every leading
/// keyword/annotation (`public`, `static`, `abstract`, `native`, `@Override`, …)
/// under a single `modifiers` node; the keyword tokens are its anonymous
/// children, whose `kind` is the keyword itself.
fn modifier_kinds(node: Node<'_>) -> Vec<&str> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut inner = child.walk();
            for m in child.children(&mut inner) {
                out.push(m.kind());
            }
        }
    }
    out
}

/// Whether a declaration carries a given modifier keyword (e.g. `"abstract"`).
fn has_modifier(node: Node<'_>, keyword: &str) -> bool {
    modifier_kinds(node).contains(&keyword)
}

/// Java visibility from access modifiers: `public`/`private`/`protected` map to
/// the matching [`Visibility`]; the absence of any access modifier is
/// package-private ([`Visibility::Package`]).
fn vis_from_modifiers(node: Node<'_>) -> Visibility {
    let mods = modifier_kinds(node);
    if mods.contains(&"public") {
        Visibility::Public
    } else if mods.contains(&"private") {
        Visibility::Private
    } else if mods.contains(&"protected") {
        Visibility::Protected
    } else {
        Visibility::Package
    }
}

/// JUnit3 name-based test convention: an instance method whose name starts with
/// `test` followed by an uppercase letter (`testFoo`). The analogue of Go's
/// name-based `TestXxx` detection.
fn is_junit3_test(name: &str) -> bool {
    name.strip_prefix("test")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_uppercase())
}
