//! The Java extractor: a recursive walk over the `tree-sitter-java` parse tree
//! that builds [`FileFacts`].
//!
//! ## Cycle scope
//! This is **Cycle 1** of the Java adapter: it emits **defs only**
//! ([`SymbolDef`]) for classes, interfaces, enums, records, methods,
//! constructors, and fields, plus the lexical [`ScopeTree`](cgx_frontend) that
//! nests them. Every other [`FileFacts`] channel — refs, imports/exports,
//! entrypoint/cut hints, effects, dataflow, and `impl_relations` — is left as
//! the empty default; later cycles fill them in. The `// Cycle N` markers below
//! flag the hook points.
//!
//! Unlike the Go adapter, the FQN prefix comes from the source
//! `package_declaration` node (see [`crate::module`]), not the file path.

use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::{
    FileCtx, FileFacts, FrontendError, Lang, LanguageFrontend, RelPath, ScopeId, SymbolDef,
};
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
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            builder.walk(child, &root_ctx);
        }
        Ok(builder.finish())
    }
}

/// Walk context threaded down the tree: the FQN prefix a def lives under and the
/// lexical scope id it is recorded in.
#[derive(Clone)]
struct Ctx {
    fqn_prefix: String,
    scope: ScopeId,
}

impl Ctx {
    fn root(prefix: String) -> Self {
        Ctx {
            fqn_prefix: prefix,
            scope: ScopeId::ROOT,
        }
    }

    /// Enter a type body: the type's FQN becomes the prefix for its members, and
    /// the type's scope becomes their parent scope.
    fn enter(&self, fqn_prefix: String, scope: ScopeId) -> Self {
        Ctx { fqn_prefix, scope }
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
        // Cycle N: refs/imports/exports/effects/dataflow/impl_relations are flushed
        // here once later cycles accumulate them. Cycle 1 emits defs + scopes only.
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

    fn walk(&mut self, node: Node<'_>, ctx: &Ctx) {
        match node.kind() {
            "class_declaration" | "interface_declaration" | "enum_declaration"
            | "record_declaration" => self.walk_type(node, ctx),
            "method_declaration" => self.walk_method(node, ctx),
            "constructor_declaration" => self.walk_constructor(node, ctx),
            "field_declaration" => self.walk_field(node, ctx),
            // Cycle N: import_declaration, method_invocation, object_creation_expression,
            // entrypoint/cut hints, effects, and dataflow dispatch land here.
            _ => self.walk_children(node, ctx),
        }
    }

    fn walk_children(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, ctx);
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
        let is_abstract =
            node.kind() == "interface_declaration" || has_modifier(node, "abstract");
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Type,
            visibility: vis_from_modifiers(node),
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract,
            signature: None,
        });

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter(fqn, scope);
            self.walk_children(body, &body_ctx);
        }
        // Cycle N: superclass/super_interfaces/extends_interfaces → ImplRelation.
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
        // Open the method's own scope (its body is not walked in Cycle 1; local and
        // anonymous classes are deferred).
        let _scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn,
            kind: SymbolKind::Method,
            visibility: vis_from_modifiers(node),
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract,
            signature: None,
        });
    }

    fn walk_constructor(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        let _scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn,
            kind: SymbolKind::Function,
            visibility: vis_from_modifiers(node),
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
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

/// The modifier-keyword kinds on a declaration. Java groups every leading
/// keyword/annotation (`public`, `static`, `abstract`, `@Override`, …) under a
/// single `modifiers` node; the keyword tokens are its anonymous children, whose
/// `kind` is the keyword itself. Returned owned so no parse-tree cursor escapes.
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
