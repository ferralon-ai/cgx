//! The Python extractor: a single recursive walk over the `tree-sitter-python`
//! parse tree that builds [`FileFacts`].
//!
//! This is the Cycle-1 slice — **symbol defs only**. The walk threads a [`Ctx`]
//! down the tree carrying the current **FQN prefix** (the module/class path a def
//! lives under) and the current **scope id** (the lexical scope a def is recorded
//! in). Each `function_definition` becomes a `Function` at module level or a
//! `Method` inside a class body; `class_definition` becomes a `Type`;
//! module-level `assignment` targets become `Constant` (PEP-8 ALL_CAPS) or
//! `Variable` defs. `decorated_definition` is unwrapped to its inner def.
//!
//! Refs, imports, inheritance, effects, and intraprocedural dataflow are later
//! cycles; the [`Builder`] keeps its accumulators minimal accordingly.

use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::{
    FileCtx, FileFacts, FrontendError, Lang, LanguageFrontend, RelPath, ScopeId, SymbolDef,
};
use tree_sitter::{Node, Parser};

use crate::module::module_path_for;

/// The Python language adapter. Stateless; one instance handles every `.py` file.
#[derive(Debug, Default, Clone)]
pub struct PythonFrontend;

impl PythonFrontend {
    pub fn new() -> Self {
        PythonFrontend
    }
}

/// Version of the Python extraction rules; bumping invalidates cached fragments.
const PYTHON_FRAGMENT_VERSION: u32 = 1;

impl LanguageFrontend for PythonFrontend {
    fn lang(&self) -> Lang {
        Lang::Python
    }

    fn handles(&self, path: &RelPath) -> bool {
        path.extension().as_deref() == Some("py")
    }

    fn fragment_version(&self) -> u32 {
        PYTHON_FRAGMENT_VERSION
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .map_err(|e| FrontendError::Parser {
                lang: "python".to_string(),
                detail: e.to_string(),
            })?;

        let tree = match parser.parse(src, None) {
            Some(tree) => tree,
            None => return Ok(FileFacts::empty()),
        };

        let module_prefix = module_path_for(ctx.path.as_str());
        let mut builder = Builder::new(src, ctx.path.as_str());
        let root_ctx = Ctx::root(module_prefix);

        let mut cursor = tree.root_node().walk();
        for child in tree.root_node().children(&mut cursor) {
            builder.walk(child, &root_ctx);
        }
        Ok(builder.finish())
    }
}

/// Walk context threaded down the tree.
#[derive(Clone)]
struct Ctx {
    fqn_prefix: String,
    scope: ScopeId,
    /// Whether the current lexical context is the body of a class (so a nested
    /// `function_definition` is a `Method`, not a `Function`).
    in_class: bool,
}

impl Ctx {
    fn root(module_prefix: String) -> Self {
        Ctx {
            fqn_prefix: module_prefix,
            scope: ScopeId::ROOT,
            in_class: false,
        }
    }

    /// Enter a definition body: new FQN prefix + scope, recording whether the body
    /// is a class body.
    fn enter_body(&self, fqn_prefix: String, scope: ScopeId, in_class: bool) -> Self {
        Ctx {
            fqn_prefix,
            scope,
            in_class,
        }
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

    // --- main dispatch ---

    fn walk(&mut self, node: Node<'_>, ctx: &Ctx) {
        match node.kind() {
            "function_definition" => self.walk_function(node, ctx),
            "class_definition" => self.walk_class(node, ctx),
            "decorated_definition" => self.walk_decorated(node, ctx),
            // Module-level bindings live inside an `expression_statement` wrapper.
            "expression_statement" => self.walk_expression_statement(node, ctx),
            _ => {}
        }
    }

    fn walk_block(&mut self, block: Node<'_>, ctx: &Ctx) {
        let mut cursor = block.walk();
        for child in block.children(&mut cursor) {
            self.walk(child, ctx);
        }
    }

    // --- definitions ---

    /// A `function_definition`: a `Method` when the enclosing body is a class,
    /// otherwise a module-level `Function`.
    fn walk_function(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        let kind = if ctx.in_class {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind,
            visibility: vis(&name),
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });

        // Recurse into the body for nested defs (a nested `def` is a closure-like
        // local function; a nested `class` is a local type). The body is not a
        // class body, so nested functions are `Function`s.
        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope, false);
            self.walk_block(body, &body_ctx);
        }
    }

    fn walk_class(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };
        let fqn = join(&ctx.fqn_prefix, &name);
        let scope = self.facts.scopes.push(ctx.scope, Some(fqn.clone()));
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Type,
            visibility: vis(&name),
            scope: ctx.scope,
            span: self.def_span(node, "name"),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope, true);
            self.walk_block(body, &body_ctx);
        }
    }

    /// A `decorated_definition` wraps a `function_definition` or `class_definition`
    /// in one or more `decorator`s. Unwrap to the inner def and emit it; the
    /// decorators are noted on the def via [`SymbolDef::is_abstract`] for the
    /// `abstractmethod` decorator (refs/metadata for arbitrary decorators are a
    /// later cycle).
    fn walk_decorated(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(inner) = node.child_by_field_name("definition") else {
            return;
        };
        let decorators = self.decorator_names(node);
        let before = self.facts.defs.len();
        self.walk(inner, ctx);
        // The inner def was just pushed; mark it abstract if an `@abstractmethod`
        // (or `@abc.abstractmethod`) decorator is present.
        if self.facts.defs.len() > before
            && decorators
                .iter()
                .any(|d| d.rsplit('.').next() == Some("abstractmethod"))
        {
            if let Some(def) = self.facts.defs.last_mut() {
                def.is_abstract = true;
            }
        }
    }

    /// The decorator expression texts of a `decorated_definition` (`@app.route` →
    /// `"app.route"`), stripping the leading `@`.
    fn decorator_names(&self, node: Node<'_>) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "decorator" {
                // A decorator's child is the expression (a `call`, `attribute`, or
                // `identifier`); take its leading dotted name.
                if let Some(expr) = child.named_child(0) {
                    out.push(self.decorator_callee_name(expr));
                }
            }
        }
        out
    }

    /// The dotted callee name of a decorator expression: an `identifier` verbatim,
    /// an `attribute` joined with `.`, a `call`'s function reduced the same way.
    fn decorator_callee_name(&self, node: Node<'_>) -> String {
        match node.kind() {
            "identifier" => self.text(node),
            "attribute" => self.text(node),
            "call" => node
                .child_by_field_name("function")
                .map(|f| self.decorator_callee_name(f))
                .unwrap_or_default(),
            _ => self.text(node),
        }
    }

    /// A module- or class-level `expression_statement` that holds an `assignment`
    /// binds names. Each plain-identifier target becomes a def: `Constant` for an
    /// ALL_CAPS name (PEP-8 module constant), else `Variable` (a `Field` inside a
    /// class body).
    fn walk_expression_statement(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "assignment" {
                self.walk_assignment(child, ctx);
            }
        }
    }

    fn walk_assignment(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        let mut names = Vec::new();
        self.collect_binding_names(left, &mut names);
        for (name, name_node) in names {
            let kind = if ctx.in_class {
                SymbolKind::Field
            } else if is_screaming_snake(&name) {
                SymbolKind::Constant
            } else {
                SymbolKind::Variable
            };
            self.facts.defs.push(SymbolDef {
                fqn: join(&ctx.fqn_prefix, &name),
                kind,
                visibility: vis(&name),
                scope: ctx.scope,
                span: self.span(name_node),
                line_end: self.end_line(name_node),
                is_abstract: false,
                signature: None,
            });
        }
    }

    /// Collect the plain-identifier binding targets of an assignment LHS, handling
    /// the `pattern_list` (`a, b = …`) tuple form. Attribute/subscript targets
    /// (`self.x = …`, `d[k] = …`) are not module/class bindings and are skipped.
    fn collect_binding_names<'t>(&self, node: Node<'t>, out: &mut Vec<(String, Node<'t>)>) {
        match node.kind() {
            "identifier" => out.push((self.text(node), node)),
            "pattern_list" | "tuple_pattern" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() {
                        self.collect_binding_names(child, out);
                    }
                }
            }
            _ => {}
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

/// Python has no access modifiers; PEP-8 leading-underscore convention is the
/// only visibility signal. `__name` (name-mangled, not a dunder) → `Private`;
/// a single `_name` → `Internal`; everything else (incl. dunders like
/// `__init__`) → `Public`.
fn vis(name: &str) -> Visibility {
    let is_dunder = name.starts_with("__") && name.ends_with("__") && name.len() > 4;
    if is_dunder {
        Visibility::Public
    } else if name.starts_with("__") {
        Visibility::Private
    } else if name.starts_with('_') {
        Visibility::Internal
    } else {
        Visibility::Public
    }
}

/// PEP-8 module constant convention: a name that is non-empty, all-uppercase
/// (letters), and contains at least one letter (so `_` or `123` alone is not a
/// constant).
fn is_screaming_snake(name: &str) -> bool {
    name.chars().any(|c| c.is_alphabetic())
        && name
            .chars()
            .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
}
