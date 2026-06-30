//! The Python extractor: a single recursive walk over the `tree-sitter-python`
//! parse tree that builds [`FileFacts`].
//!
//! The walk threads a [`Ctx`] down the tree carrying the current **FQN prefix**
//! (the module/class path a def lives under), the current **scope id** (the
//! lexical scope a def/ref lives in), an **`in_class`** flag (method vs function),
//! and the current **condition chain** (the ordered set of enclosing guarding
//! constructs whose ADR-03 maximum is the edge condition for a call site).
//!
//! Cycle-1 emitted symbol defs only. Cycle-2 adds, by mirroring the Go adapter:
//! call refs (`Call` / `CallVirtualReceiver` / `Instantiate`), GM-3.1 edge
//! conditions, imports/exports, entrypoint hints, and cut hints.
//!
//! ## Later-cycle hook points
//! Inheritance/overrides (Cycle 3) flush from [`Builder::finish`] like Go's
//! `emit_in_file_implements`. Effects (Cycle 4) hook into [`Builder::walk_call`]
//! keyed by `ctx.fqn_prefix`; dataflow (Cycle 5) adds a second SSA pass over each
//! body, mirroring Go's `walk_function_ssa`. The Builder keeps its accumulators
//! minimal until then.

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::{
    CutHint, EntrypointHint, ExportFact, FileCtx, FileFacts, FrontendError, ImplRelation,
    ImportFact, ImportedName, Lang, LanguageFrontend, Name, RawRef, RefKind, RelPath, RelationKind,
    ScopeId, SymbolDef,
};
use smallvec::SmallVec;
use std::collections::{BTreeMap, BTreeSet};
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
const PYTHON_FRAGMENT_VERSION: u32 = 2;

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

        let root = tree.root_node();
        builder.collect_dunder_all(root);

        let mut stmt_index = 0u32;
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            builder.walk(child, &root_ctx, &mut stmt_index);
        }
        builder.emit_module_exports();
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
    /// Ordered chain of enclosing guarding constructs (ADR-03); the edge
    /// condition of a ref is the maximum of this set.
    conditions: SmallVec<[EdgeCondition; 4]>,
}

impl Ctx {
    fn root(module_prefix: String) -> Self {
        Ctx {
            fqn_prefix: module_prefix,
            scope: ScopeId::ROOT,
            in_class: false,
            conditions: SmallVec::new(),
        }
    }

    /// Enter a definition body: new FQN prefix + scope, reset conditions,
    /// recording whether the body is a class body.
    fn enter_body(&self, fqn_prefix: String, scope: ScopeId, in_class: bool) -> Self {
        Ctx {
            fqn_prefix,
            scope,
            in_class,
            conditions: SmallVec::new(),
        }
    }

    fn with_condition(&self, cond: EdgeCondition) -> Self {
        let mut conditions = self.conditions.clone();
        conditions.push(cond);
        Ctx {
            fqn_prefix: self.fqn_prefix.clone(),
            scope: self.scope,
            in_class: self.in_class,
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
    /// The names listed in a module-level `__all__` assignment, when present. When
    /// `Some`, only these names are exported; when `None`, public (non-underscore)
    /// module-level defs are exported.
    dunder_all: Option<BTreeSet<String>>,
    /// Public module-level defs (name + span) collected during the walk; flushed to
    /// `ExportFact`s in [`Builder::emit_module_exports`] per the `__all__`-or-convention rule.
    module_defs: Vec<(String, Span)>,
    /// Per-class inheritance state collected during the walk, keyed by the class's
    /// simple name. Flushed to `Overrides` relations in [`Builder::finish`] (so that
    /// a method overriding a base declared *later* in the file still matches —
    /// declaration order is irrelevant, mirroring Java's `flush_overrides`).
    classes: BTreeMap<String, ClassRec>,
}

/// Dedup key for an `Overrides` relation: `(subject FQN segments, [Base, method])`.
type OverrideKey = (SmallVec<[Name; 2]>, SmallVec<[Name; 2]>);

/// Per-class data gathered during the walk to resolve in-file overrides at flush.
#[derive(Default)]
struct ClassRec {
    /// In-file base classes by simple name, as written in `superclasses`
    /// (`keyword_argument`s like `metaclass=` already filtered out; dotted bases
    /// reduced to their last segment via `attribute_segments`).
    bases: Vec<String>,
    /// The class's methods: simple name → (full FQN, has-`@override`-decorator).
    methods: BTreeMap<String, MethodRec>,
}

struct MethodRec {
    fqn: String,
    /// Span of the method's `def` (anchored at the name), used for the `Overrides`
    /// relation's source span.
    span: Span,
    /// PEP 698 `@override` / `typing.override` decorator — authoritative (treated
    /// like Java's `@Override`): emit against every declared in-file base.
    explicit_override: bool,
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], file: &str) -> Self {
        Builder {
            src,
            file: file.to_string(),
            facts: FileFacts::empty(),
            dunder_all: None,
            module_defs: Vec::new(),
            classes: BTreeMap::new(),
        }
    }

    fn finish(mut self) -> FileFacts {
        self.flush_overrides();
        self.facts
    }

    /// Emit `Overrides` relations now that every class/method in the file is known.
    ///
    /// A method overrides when it shares a name with a method on an **in-file**
    /// declared supertype. Python has no `@Override` annotation, so name-match
    /// against in-file supertypes is the only single-file signal (transitive /
    /// external override resolution is the CHA/resolve layer's job, exactly the
    /// call Rust/Java made). A PEP 698 `@override` decorator is authoritative:
    /// emit against every declared in-file base regardless of whether that base's
    /// method set is visible here.
    ///
    /// `subject` is the overriding method's **full FQN segments** (REQUIRED: the
    /// resolver looks the override source up by exact `def_to_node` join).
    /// `object` is `[BaseSimpleName, method]`, grounded by `resolve_trait_method`.
    /// We emit against *every* candidate base; an `object` that doesn't exist
    /// simply grounds no edge. Dedup via a `BTreeSet`.
    fn flush_overrides(&mut self) {
        let mut rels: BTreeMap<OverrideKey, Span> = BTreeMap::new();
        for class in self.classes.values() {
            for (mname, mrec) in &class.methods {
                for base in &class.bases {
                    let base_defines = self
                        .classes
                        .get(base)
                        .map(|b| b.methods.contains_key(mname))
                        .unwrap_or(false);
                    if !mrec.explicit_override && !base_defines {
                        continue;
                    }
                    rels.entry((fqn_segments(&mrec.fqn), name_path_two(base, mname)))
                        .or_insert_with(|| mrec.span.clone());
                }
            }
        }
        for ((subject, object), span) in rels {
            self.facts.impl_relations.push(ImplRelation {
                kind: RelationKind::Overrides,
                subject,
                object,
                span,
            });
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

    /// Span anchored at a def's `name` field when present (else the whole node).
    fn def_span(&self, node: Node<'_>, name_field: &str) -> Span {
        node.child_by_field_name(name_field)
            .map(|n| self.span(n))
            .unwrap_or_else(|| self.span(node))
    }

    // --- main dispatch ---

    fn walk(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        match node.kind() {
            "function_definition" => self.walk_function(node, ctx),
            "class_definition" => self.walk_class(node, ctx),
            "decorated_definition" => self.walk_decorated(node, ctx),
            "import_statement" => self.walk_import(node, ctx),
            "import_from_statement" => self.walk_import_from(node, ctx),
            "call" => self.walk_call(node, ctx, stmt_index),
            "if_statement" => self.walk_if(node, ctx, stmt_index),
            "conditional_expression" => self.walk_conditional_expr(node, ctx, stmt_index),
            "match_statement" => self.walk_match(node, ctx, stmt_index),
            "for_statement" | "while_statement" => self.walk_loop(node, ctx, stmt_index),
            "try_statement" => self.walk_try(node, ctx, stmt_index),
            // Module-/class-level bindings live inside an `expression_statement`.
            "expression_statement" => self.walk_expression_statement(node, ctx, stmt_index),
            _ => self.walk_children(node, ctx, stmt_index),
        }
    }

    fn walk_children(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, ctx, stmt_index);
        }
    }

    fn walk_block(&mut self, block: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let mut cursor = block.walk();
        for child in block.children(&mut cursor) {
            self.walk(child, ctx, stmt_index);
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
        self.note_module_def(&name, ctx, node);
        self.record_fn_entrypoints(&name, &fqn, ctx);
        self.note_class_method(&name, &fqn, ctx, node);

        // Recurse into the body for nested defs and call refs. The body is not a
        // class body, so nested functions are `Function`s. A body opens a fresh
        // condition chain and a new statement index.
        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope, false);
            let mut inner = 0u32;
            self.walk_block(body, &body_ctx, &mut inner);
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
        self.note_module_def(&name, ctx, node);
        self.record_class_entrypoint(node, &name, &fqn, ctx);
        self.emit_inheritance(node, &name);

        if let Some(body) = node.child_by_field_name("body") {
            let body_ctx = ctx.enter_body(fqn, scope, true);
            let mut inner = 0u32;
            self.walk_block(body, &body_ctx, &mut inner);
        }
    }

    /// Emit one `Inherits` relation per base class and register the class's
    /// declared in-file base names for later override resolution.
    ///
    /// Bases live in the `class_definition`'s `superclasses` `argument_list`; its
    /// named children are `identifier` (bare base) / `attribute` (dotted `pkg.Base`)
    /// nodes, plus `keyword_argument`s (`metaclass=`, `total=False`) which are NOT
    /// bases and are filtered out. A dotted base reduces to its last segment
    /// (best-name); the resolver's `resolve_type_node` short-name fallback grounds
    /// it, and an unresolved external base (`Exception`, `unittest.TestCase`) simply
    /// grounds no edge — honest, never invented.
    fn emit_inheritance(&mut self, node: Node<'_>, name: &str) {
        let bases = self.base_names(node);
        let span = self.def_span(node, "name");
        for base in &bases {
            self.facts.impl_relations.push(ImplRelation {
                kind: RelationKind::Inherits,
                subject: smallvec_one(name.to_string()),
                object: smallvec_one(base.clone()),
                span: span.clone(),
            });
        }
        self.classes.entry(name.to_string()).or_default().bases = bases;
    }

    /// The base-class simple names declared in a `class_definition`'s `superclasses`
    /// list, in source order. `keyword_argument`s are skipped; a dotted base
    /// (`attribute`) reduces to its last segment.
    fn base_names(&self, node: Node<'_>) -> Vec<String> {
        let Some(supers) = node.child_by_field_name("superclasses") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut cursor = supers.walk();
        for c in supers.children(&mut cursor) {
            if !c.is_named() {
                continue;
            }
            match c.kind() {
                "identifier" => out.push(self.text(c)),
                "attribute" => {
                    if let Some(last) = self.attribute_segments(c).last() {
                        out.push(last.clone());
                    }
                }
                // `keyword_argument` (metaclass=…, total=False) and any other form
                // (a dynamic `get_base()` call, a subscript `Generic[T]`) are not
                // nominal in-file bases — skip.
                _ => {}
            }
        }
        out
    }

    /// A `decorated_definition` wraps a `function_definition` or `class_definition`
    /// in one or more `decorator`s. Unwrap to the inner def and emit it; the
    /// `abstractmethod` decorator flags the inner def abstract.
    fn walk_decorated(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(inner) = node.child_by_field_name("definition") else {
            return;
        };
        let decorators = self.decorator_names(node);
        let before = self.facts.defs.len();
        let mut throwaway = 0u32;
        self.walk(inner, ctx, &mut throwaway);
        if self.facts.defs.len() > before
            && decorators
                .iter()
                .any(|d| d.rsplit('.').next() == Some("abstractmethod"))
        {
            if let Some(def) = self.facts.defs.last_mut() {
                def.is_abstract = true;
            }
        }
        // PEP 698 `@override` / `typing.override`: authoritative override marker on
        // a class method (treated like Java's `@Override`). Flag the just-recorded
        // method so `flush_overrides` emits against every declared base.
        if ctx.in_class
            && inner.kind() == "function_definition"
            && decorators
                .iter()
                .any(|d| d.rsplit('.').next() == Some("override"))
        {
            if let (Some(class), Some(mname)) = (
                ctx.fqn_prefix.rsplit("::").next(),
                self.field_text(inner, "name"),
            ) {
                if let Some(rec) = self
                    .classes
                    .get_mut(class)
                    .and_then(|c| c.methods.get_mut(&mname))
                {
                    rec.explicit_override = true;
                }
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
    /// class body). Other expression statements (incl. bare `call`s) are walked for
    /// refs.
    fn walk_expression_statement(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "assignment" => self.walk_assignment(child, ctx, stmt_index),
                _ => self.walk(child, ctx, stmt_index),
            }
        }
    }

    fn walk_assignment(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(left) = node.child_by_field_name("left") {
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
                self.note_module_def(&name, ctx, name_node);
            }
        }
        // The RHS may contain call refs (`x = f()`); walk the whole assignment for
        // them. `left` is identifiers only, so no spurious refs come from it.
        self.walk_children(node, ctx, stmt_index);
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

    // --- references (calls) ---

    fn walk_call(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(func) = node.child_by_field_name("function") {
            self.emit_call_cut_hints(func, node);
            let (name_path, kind) = self.classify_callee(func);
            if !name_path.is_empty() {
                self.push_ref(name_path, kind, ctx, self.span(node), *stmt_index);
                *stmt_index += 1;
            }
            // The callee operand may itself nest a call (`getattr(o, "m")()`,
            // `factory().run()`); descend unless it is a plain name/attribute
            // already accounted for above.
            if !matches!(func.kind(), "identifier" | "attribute") {
                self.walk(func, ctx, stmt_index);
            }
        }
        // Walk the argument list for nested calls (`f(g())`).
        if let Some(args) = node.child_by_field_name("arguments") {
            self.walk(args, ctx, stmt_index);
        }
    }

    /// Classify a call's `function` operand into its `(name_path, RefKind)`:
    /// a bare `identifier` → `Call` (or `Instantiate` when the callee names a
    /// class declared in this file); an `attribute` `x.foo` → `CallVirtualReceiver`
    /// (the resolver decides module-call vs method via imports / CHA).
    fn classify_callee(&self, func: Node<'_>) -> (SmallVec<[String; 2]>, RefKind) {
        match func.kind() {
            "identifier" => {
                let name = self.text(func);
                let kind = if self.is_local_class(&name) {
                    RefKind::Instantiate
                } else {
                    RefKind::Call
                };
                (smallvec_one(name), kind)
            }
            "attribute" => {
                let segs = self.attribute_segments(func);
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
            _ => (SmallVec::new(), RefKind::Call),
        }
    }

    /// The segment path of an `attribute` chain (`a.b.c` → `[a, b, c]`), recursing
    /// through the `object` operand.
    fn attribute_segments(&self, node: Node<'_>) -> SmallVec<[String; 2]> {
        let mut out = SmallVec::new();
        match node.kind() {
            "attribute" => {
                if let Some(object) = node.child_by_field_name("object") {
                    out = self.attribute_segments(object);
                }
                if let Some(attr) = node.child_by_field_name("attribute") {
                    out.push(self.text(attr));
                }
            }
            "identifier" => out.push(self.text(node)),
            _ => {}
        }
        out
    }

    /// Whether `name` is a class (`SymbolKind::Type`) declared in this file (so
    /// `Foo()` is an instantiation, not a free call). Matched on the def's short
    /// name; a class is pushed before any body that calls it is walked.
    fn is_local_class(&self, name: &str) -> bool {
        self.facts
            .defs
            .iter()
            .any(|d| d.kind == SymbolKind::Type && d.fqn.rsplit("::").next() == Some(name))
    }

    fn push_ref(
        &mut self,
        name_path: SmallVec<[String; 2]>,
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

    // --- cut hints ---

    /// Dynamic/reflective cut hints keyed off the callee surface (no import
    /// resolution single-file, so this matches by well-known builtin/API names,
    /// the analogue of Java's `forName`/`loadClass`):
    /// - `eval(` / `exec(` / `__import__(` → Dynamic
    /// - `getattr(` / `setattr(` / `delattr(` → Reflective
    /// - `importlib.import_module(` → Dynamic
    /// - `ctypes.*` / `*.CDLL(` / `*.cdll.*` → ViaFfi
    fn emit_call_cut_hints(&mut self, func: Node<'_>, call: Node<'_>) {
        let marker = match func.kind() {
            "identifier" => match self.text(func).as_str() {
                "eval" | "exec" | "__import__" => Some(CutMarker::Dynamic),
                "getattr" | "setattr" | "delattr" | "hasattr" => Some(CutMarker::Reflective),
                _ => None,
            },
            "attribute" => {
                let segs = self.attribute_segments(func);
                let last = segs.last().map(String::as_str);
                let leading = segs.first().map(String::as_str);
                if last == Some("import_module") && leading == Some("importlib") {
                    Some(CutMarker::Dynamic)
                } else if leading == Some("ctypes") || last == Some("CDLL") || last == Some("LoadLibrary") {
                    Some(CutMarker::ViaFfi)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(marker) = marker {
            self.facts.cut_hints.push(CutHint {
                marker,
                span: self.span(call),
                macro_origin: None,
            });
        }
    }

    // --- conditions ---

    fn walk_if(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        // A module-level `if __name__ == "__main__":` guard marks the module's
        // executable entrypoint.
        if !ctx.in_class && ctx.scope == ScopeId::ROOT {
            self.detect_name_main_guard(node, ctx);
        }
        if let Some(c) = node.child_by_field_name("condition") {
            self.walk(c, ctx, stmt_index);
        }
        let inner = ctx.with_condition(EdgeCondition::Conditional);
        if let Some(c) = node.child_by_field_name("consequence") {
            self.walk(c, &inner, stmt_index);
        }
        // `alternative` is an `elif_clause` (own condition/consequence) or an
        // `else_clause`; both are conditional branches.
        if let Some(a) = node.child_by_field_name("alternative") {
            self.walk(a, &inner, stmt_index);
        }
    }

    /// A ternary `a if cond else b`: both arms are conditional. The
    /// `conditional_expression` holds three expression children (consequence,
    /// condition, alternative); guard them all conditional for simplicity.
    fn walk_conditional_expr(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let inner = ctx.with_condition(EdgeCondition::Conditional);
        self.walk_children(node, &inner, stmt_index);
    }

    /// A `match` statement: the subject is unguarded; each `case_clause` body is
    /// conditional (one arm of a multi-way branch).
    fn walk_match(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(subject) = node.child_by_field_name("subject") {
            self.walk(subject, ctx, stmt_index);
        }
        let inner = ctx.with_condition(EdgeCondition::Conditional);
        if let Some(body) = node.child_by_field_name("body") {
            // The match body block's children are `case_clause`s.
            let mut cursor = body.walk();
            for case in body.children(&mut cursor) {
                self.walk(case, &inner, stmt_index);
            }
        }
    }

    fn walk_loop(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            let inner = ctx.with_condition(EdgeCondition::Loop);
            self.walk(body, &inner, stmt_index);
        }
        // Walk any non-body children (iterable/condition/else clause) unguarded.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Some(child) == body {
                continue;
            }
            self.walk(child, ctx, stmt_index);
        }
    }

    /// A `try` statement: the `body`/`else_clause` are unguarded; each
    /// `except_clause` body is `Exception`; the `finally_clause` block is `Always`
    /// (GM-3.1 carve-out — a finally call is taken once the block is entered).
    fn walk_try(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        if let Some(body) = node.child_by_field_name("body") {
            self.walk(body, ctx, stmt_index);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "except_clause" => {
                    let inner = ctx.with_condition(EdgeCondition::Exception);
                    self.walk_children(child, &inner, stmt_index);
                }
                "finally_clause" => {
                    let inner = ctx.with_condition(EdgeCondition::Always);
                    self.walk_children(child, &inner, stmt_index);
                }
                "else_clause" => {
                    self.walk(child, ctx, stmt_index);
                }
                _ => {}
            }
        }
    }

    // --- imports / exports ---

    /// `import a.b.c [as d], sys` → one `ImportFact` per imported name.
    fn walk_import(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut cursor = node.walk();
        for child in node.children_by_field_name("name", &mut cursor) {
            let (specifier, alias) = self.import_name_parts(child);
            if specifier.is_empty() {
                continue;
            }
            let last = specifier
                .rsplit('.')
                .next()
                .unwrap_or(&specifier)
                .to_string();
            self.facts.imports.push(ImportFact {
                specifier,
                names: vec![ImportedName { name: last, alias }],
                glob: false,
                re_export: false,
                scope: ctx.scope,
                span: self.span(node),
            });
        }
    }

    /// `from a.b import c as d, e` → one `ImportFact` whose specifier is the module
    /// path and whose names list the imported bindings. `from x import *` → a glob
    /// import with empty names.
    fn walk_import_from(&mut self, node: Node<'_>, ctx: &Ctx) {
        let specifier = node
            .child_by_field_name("module_name")
            .map(|n| self.text(n))
            .unwrap_or_default();

        // `from x import *` — the `wildcard_import` is a positional child.
        let mut cursor = node.walk();
        let has_wildcard = node
            .children(&mut cursor)
            .any(|c| c.kind() == "wildcard_import");
        if has_wildcard {
            self.facts.imports.push(ImportFact {
                specifier,
                names: Vec::new(),
                glob: true,
                re_export: false,
                scope: ctx.scope,
                span: self.span(node),
            });
            return;
        }

        let mut names = Vec::new();
        let mut name_cursor = node.walk();
        for child in node.children_by_field_name("name", &mut name_cursor) {
            let (name, alias) = self.import_name_parts(child);
            if !name.is_empty() {
                names.push(ImportedName { name, alias });
            }
        }
        if names.is_empty() {
            return;
        }
        self.facts.imports.push(ImportFact {
            specifier,
            names,
            glob: false,
            re_export: false,
            scope: ctx.scope,
            span: self.span(node),
        });
    }

    /// The `(dotted_name, optional alias)` of an imported-name node, which is either
    /// a bare `dotted_name` or an `aliased_import` (`name` + `alias` fields).
    fn import_name_parts(&self, node: Node<'_>) -> (String, Option<String>) {
        match node.kind() {
            "dotted_name" => (self.text(node), None),
            "aliased_import" => {
                let name = self
                    .field_text(node, "name")
                    .unwrap_or_else(|| self.text(node));
                let alias = self.field_text(node, "alias");
                (name, alias)
            }
            _ => (String::new(), None),
        }
    }

    /// Pre-scan the module top level for a `__all__ = [...]` assignment and record
    /// the listed string names; when present it is the authoritative export list.
    fn collect_dunder_all(&mut self, root: Node<'_>) {
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() != "expression_statement" {
                continue;
            }
            let mut inner = child.walk();
            for stmt in child.children(&mut inner) {
                if stmt.kind() != "assignment" {
                    continue;
                }
                let is_all = stmt
                    .child_by_field_name("left")
                    .map(|l| l.kind() == "identifier" && self.text(l) == "__all__")
                    .unwrap_or(false);
                if !is_all {
                    continue;
                }
                if let Some(right) = stmt.child_by_field_name("right") {
                    let mut set = BTreeSet::new();
                    self.collect_string_items(right, &mut set);
                    self.dunder_all = Some(set);
                }
            }
        }
    }

    /// The string literal values of a `list`/`tuple`/`set` expression (`["a","b"]`
    /// → `{a, b}`), reading each `string`'s `string_content`.
    fn collect_string_items(&self, node: Node<'_>, out: &mut BTreeSet<String>) {
        match node.kind() {
            "list" | "tuple" | "set" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "string" {
                        if let Some(value) = self.string_content(child) {
                            out.insert(value);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The text of a `string` node's `string_content` child (the value without the
    /// surrounding quotes / prefix).
    fn string_content(&self, node: Node<'_>) -> Option<String> {
        let mut cursor = node.walk();
        let content = node
            .children(&mut cursor)
            .find(|c| c.kind() == "string_content")
            .map(|c| self.text(c));
        content
    }

    /// Record a class method for later `Overrides` resolution. Only a *direct*
    /// member of a class body counts (`ctx.in_class`); the enclosing class's simple
    /// name is the last segment of `ctx.fqn_prefix`. Nested functions inside a
    /// method have `in_class == false` and are skipped (not overridable members).
    fn note_class_method(&mut self, name: &str, fqn: &str, ctx: &Ctx, node: Node<'_>) {
        if !ctx.in_class {
            return;
        }
        let Some(class) = ctx.fqn_prefix.rsplit("::").next() else {
            return;
        };
        let span = self.def_span(node, "name");
        self.classes
            .entry(class.to_string())
            .or_default()
            .methods
            .insert(
                name.to_string(),
                MethodRec {
                    fqn: fqn.to_string(),
                    span,
                    explicit_override: false,
                },
            );
    }

    /// Record a public module-level def for later export emission. Only top-level
    /// (`!in_class`, root scope) defs are candidates; the `__all__`-vs-convention
    /// decision happens in [`Builder::emit_module_exports`].
    fn note_module_def(&mut self, name: &str, ctx: &Ctx, node: Node<'_>) {
        if ctx.in_class || ctx.scope != ScopeId::ROOT {
            return;
        }
        self.module_defs.push((name.to_string(), self.span(node)));
    }

    /// Flush the export facts: if `__all__` is present, export exactly those names
    /// (matched against the collected module defs); otherwise export every public
    /// (non-underscore) module-level def. Mirrors Go's `maybe_export`, keyed on the
    /// Python underscore-is-private convention.
    fn emit_module_exports(&mut self) {
        let module_defs = std::mem::take(&mut self.module_defs);
        match &self.dunder_all {
            Some(allowed) => {
                for (name, span) in module_defs {
                    if allowed.contains(&name) {
                        self.facts.exports.push(ExportFact {
                            name,
                            alias: None,
                            from: None,
                            span,
                        });
                    }
                }
            }
            None => {
                for (name, span) in module_defs {
                    if is_public(&name) {
                        self.facts.exports.push(ExportFact {
                            name,
                            alias: None,
                            from: None,
                            span,
                        });
                    }
                }
            }
        }
    }

    // --- entrypoints ---

    /// `def main(...)` at module level → `Main` candidate; a `test_*` function →
    /// `Test` (pytest/unittest convention).
    fn record_fn_entrypoints(&mut self, name: &str, fqn: &str, ctx: &Ctx) {
        if !ctx.in_class && name == "main" {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.to_string(),
                kind: EntrypointKind::Main,
            });
        } else if is_test_name(name) {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.to_string(),
                kind: EntrypointKind::Test,
            });
        }
    }

    /// A `class Test*` or a `unittest.TestCase` subclass → `Test`.
    fn record_class_entrypoint(&mut self, node: Node<'_>, name: &str, fqn: &str, _ctx: &Ctx) {
        let is_test = name.starts_with("Test") || self.has_test_case_base(node);
        if is_test {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.to_string(),
                kind: EntrypointKind::Test,
            });
        }
    }

    /// Whether a `class_definition`'s `superclasses` list names a `TestCase` base
    /// (`unittest.TestCase` or bare `TestCase`).
    fn has_test_case_base(&self, node: Node<'_>) -> bool {
        let Some(supers) = node.child_by_field_name("superclasses") else {
            return false;
        };
        let mut cursor = supers.walk();
        let found = supers.children(&mut cursor).any(|c| {
            if !c.is_named() {
                return false;
            }
            let base = match c.kind() {
                "attribute" => self
                    .attribute_segments(c)
                    .last()
                    .cloned()
                    .unwrap_or_default(),
                _ => self.text(c),
            };
            base == "TestCase"
        });
        found
    }

    /// Detect the `if __name__ == "__main__":` guard, emitting a `Main` entrypoint
    /// hint (`<module>::__main__`) for the module. Called from [`Builder::walk_if`]
    /// when the `if` is at module level.
    fn detect_name_main_guard(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(cond) = node.child_by_field_name("condition") else {
            return;
        };
        if cond.kind() != "comparison_operator" {
            return;
        }
        let mut has_name = false;
        let mut has_main = false;
        let mut cursor = cond.walk();
        for child in cond.children(&mut cursor) {
            match child.kind() {
                "identifier" if self.text(child) == "__name__" => has_name = true,
                "string" if self.string_content(child).as_deref() == Some("__main__") => {
                    has_main = true
                }
                _ => {}
            }
        }
        if has_name && has_main {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: join(&ctx.fqn_prefix, "__main__"),
                kind: EntrypointKind::Main,
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

fn smallvec_one(s: String) -> SmallVec<[String; 2]> {
    let mut v = SmallVec::new();
    v.push(s);
    v
}

/// A two-segment name path `[a, b]` (an `Overrides` object: `[Base, method]`).
fn name_path_two(a: &str, b: &str) -> SmallVec<[Name; 2]> {
    let mut v = SmallVec::new();
    v.push(a.to_string());
    v.push(b.to_string());
    v
}

/// Split a `::`-joined FQN into its segments (the `Overrides` subject the resolver
/// rejoins for an exact `def_to_node` lookup).
fn fqn_segments(fqn: &str) -> SmallVec<[Name; 2]> {
    fqn.split("::").map(str::to_string).collect()
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

/// Whether a name is public by Python convention: not leading-underscore. Dunders
/// (`__init__`) read as public, but a module-level dunder is not exported via the
/// no-`__all__` convention (it is special, not API), so exclude `__x` entirely.
fn is_public(name: &str) -> bool {
    !name.starts_with('_')
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

/// pytest/unittest test-function convention: a name starting with `test_` or
/// exactly `test`.
fn is_test_name(name: &str) -> bool {
    name == "test" || name.starts_with("test_")
}
