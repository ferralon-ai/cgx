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
//! **Cycle 3** adds the inheritance lattice ([`ImplRelation`]):
//! - `Inherits` for a class `extends` superclass and an interface `extends`
//!   super-interfaces; `Implements` for a class/enum/record `implements`
//!   interfaces;
//! - `Overrides` for a method that overrides a supertype method — driven by the
//!   `@Override` annotation (authoritative, cross-file capable) or, when
//!   unannotated, name+arity matching against an in-file supertype.
//!
//! **Cycle 4** adds syntactic own-effects ([`EffectFact`]) and concurrency/async
//! hints, both folded into the same [`EffectSet`]:
//! - per-call effects from [`effects_of_call`] (I/O, nondeterminism, reflection),
//!   recorded against the enclosing method/constructor and flushed in
//!   [`Builder::finish`];
//! - concurrency: `Spawns` at thread/executor/async launch sites
//!   (`start`/`submit`/`execute`/`*Async`), `Blocking` at lock/sleep/await/join
//!   sites and on `synchronized` methods and `synchronized` statement blocks.
//!
//! **Cycle 5** adds intraprocedural dataflow ([`DataFlowFact`]): a second,
//! dataflow-only walk of each method/constructor body lowers each production
//! site (local-var initializer, assignment, return, for-each binding) into SSA
//! value nodes + `DerivesFrom` edges (derived→source), mirroring the Go adapter's
//! model exactly (same fact shape, same SSA versioning, same `OpaqueCall`/
//! `TruncatedAccessPath` under-approximation posture).
//!
//! Unlike the Go adapter, the FQN prefix comes from the source
//! `package_declaration` node (see [`crate::module`]), not the file path.

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::effect::{Effect, EffectSet};
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_core::transform::Transform;
use cgx_frontend::{
    CutHint, DataFlowFact, EffectFact, EntrypointHint, ExportFact, FileCtx, FileFacts,
    FrontendError, ImplRelation, ImportFact, ImportedName, Lang, LanguageFrontend, Name, RawRef,
    RefKind, RelPath, RelationKind, ScopeId, SymbolDef,
};
use smallvec::SmallVec;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tree_sitter::{Node, Parser};

use crate::effects::effects_of_call;
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
    /// Per-type declared supertype simple names (superclass + interfaces +
    /// super-interfaces), keyed by the type's FQN. Drives override candidacy.
    type_supertypes: BTreeMap<String, Vec<String>>,
    /// Methods declared directly in each type body, keyed by the type's FQN.
    type_methods: BTreeMap<String, Vec<MethodRec>>,
    /// In-file `(method name, arity)` sets keyed by a type's *simple* name, so a
    /// subtype method can be name+arity matched against an in-file supertype.
    type_method_sigs: BTreeMap<String, BTreeSet<(String, usize)>>,
    /// Cycle 4: accumulated syntactic own-effects (incl. concurrency `Spawns`/
    /// `Blocking`) keyed by the enclosing callable's FQN. Flushed into
    /// `facts.effects` in [`Builder::finish`]. `BTreeMap` keeps the flush order
    /// deterministic.
    effects: BTreeMap<String, EffectSet>,
    /// Cycle 5: accumulated intraprocedural dataflow facts keyed by the owning
    /// callable's FQN. Flushed into `facts.data_flows` in [`Builder::finish`]
    /// (canonicalization sorts them, so the key only organizes the walk).
    data_flows: BTreeMap<String, Vec<DataFlowFact>>,
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], file: &str) -> Self {
        Builder {
            src,
            file: file.to_string(),
            facts: FileFacts::empty(),
            type_supertypes: BTreeMap::new(),
            type_methods: BTreeMap::new(),
            type_method_sigs: BTreeMap::new(),
            effects: BTreeMap::new(),
            data_flows: BTreeMap::new(),
        }
    }

    fn finish(mut self) -> FileFacts {
        // Cycle 3: flush the in-file `Overrides` relations accumulated during the
        // walk. `Inherits`/`Implements` are emitted eagerly in `walk_type` (the
        // supertype clause is local to the declaration). Dataflow (Cycle 5) still
        // slots in here. Canonicalization is the registry's job.
        self.flush_overrides();
        // Cycle 4: flush accumulated own-effects (incl. concurrency).
        for (fqn, set) in std::mem::take(&mut self.effects) {
            if !set.is_empty() {
                self.facts.effects.push(EffectFact { fqn, effects: set });
            }
        }
        // Cycle 5: flush accumulated intraprocedural dataflow facts.
        for (_fqn, facts) in std::mem::take(&mut self.data_flows) {
            self.facts.data_flows.extend(facts);
        }
        self.facts
    }

    /// Record an own-effect set against the enclosing callable. Inside a body
    /// `ctx.fqn_prefix` is the callable's FQN (set by [`Ctx::enter_body`]); a
    /// site reached outside any body (e.g. a field initializer) attributes to its
    /// enclosing prefix, matching the Go adapter.
    fn record_effects(&mut self, ctx: &Ctx, set: EffectSet) {
        if set.is_empty() {
            return;
        }
        self.effects
            .entry(ctx.fqn_prefix.clone())
            .or_default()
            .union_with(set);
    }

    /// Record an own-effect set directly against a callable's FQN (used for the
    /// `synchronized` *method* modifier, whose `Blocking` attaches to the method
    /// itself rather than to a call site inside it).
    fn record_effect_for(&mut self, fqn: &str, set: EffectSet) {
        if set.is_empty() {
            return;
        }
        self.effects
            .entry(fqn.to_string())
            .or_default()
            .union_with(set);
    }

    /// Emit an `Overrides` relation for every directly-declared method that
    /// overrides a supertype method. `@Override` is authoritative and emits
    /// against *every* declared supertype (cross-file linking is the resolver's
    /// job — a `Super::method` that does not exist simply grounds no edge). A
    /// method *without* `@Override` is matched by name + arity against the method
    /// set of any supertype declared **in this file** (the cross-file
    /// non-annotated case has no local signal and is left to a later CHA pass).
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
                    let matched = if m.has_override {
                        true
                    } else {
                        type_method_sigs
                            .get(s)
                            .is_some_and(|sigs| sigs.contains(&(m.name.clone(), m.arity)))
                    };
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
            "synchronized_statement" => self.walk_synchronized(node, ctx, stmt_index),
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

        // Inheritance lattice (GM-2.2): emit `Inherits`/`Implements` from the
        // `extends`/`implements` clauses and stash the supertype names so the
        // override pass (finish) can ground per-method `Overrides`.
        let supers = self.emit_inheritance(node, &fqn);
        if !supers.is_empty() {
            self.type_supertypes.insert(fqn.clone(), supers);
        }

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
    }

    /// Emit the `Inherits`/`Implements` lattice edges declared by a type's
    /// `extends`/`implements` clauses and return the flat list of supertype
    /// simple names (used for override candidacy). The `subject` is the type's
    /// full FQN segments (the resolver prefers an exact-FQN match, falling back
    /// to a unique short name); the `object` is the supertype's simple name as
    /// written — an unresolved external/JDK supertype simply grounds no edge.
    fn emit_inheritance(&mut self, node: Node<'_>, type_fqn: &str) -> Vec<String> {
        let span = self.def_span(node, "name");
        let mut supers = Vec::new();
        match node.kind() {
            "class_declaration" => {
                if let Some(sc) = self.superclass_name(node) {
                    self.push_relation(RelationKind::Inherits, type_fqn, &sc, &span);
                    supers.push(sc);
                }
                for iface in self.interface_names(node) {
                    self.push_relation(RelationKind::Implements, type_fqn, &iface, &span);
                    supers.push(iface);
                }
            }
            "enum_declaration" | "record_declaration" => {
                for iface in self.interface_names(node) {
                    self.push_relation(RelationKind::Implements, type_fqn, &iface, &span);
                    supers.push(iface);
                }
            }
            "interface_declaration" => {
                for sup in self.extends_interface_names(node) {
                    self.push_relation(RelationKind::Inherits, type_fqn, &sup, &span);
                    supers.push(sup);
                }
            }
            _ => {}
        }
        supers
    }

    /// The superclass simple name from a `class_declaration`'s `superclass` field
    /// (`extends B` → `B`; generics/qualification stripped).
    fn superclass_name(&self, node: Node<'_>) -> Option<String> {
        let sc = node.child_by_field_name("superclass")?;
        let mut cursor = sc.walk();
        for c in sc.children(&mut cursor) {
            if c.is_named() {
                let n = type_name(&self.text(c));
                if !n.is_empty() {
                    return Some(n);
                }
            }
        }
        None
    }

    /// The `implements` interface simple names from a class/enum/record's
    /// `interfaces` field (a `super_interfaces` node wrapping a `type_list`).
    fn interface_names(&self, node: Node<'_>) -> Vec<String> {
        node.child_by_field_name("interfaces")
            .map(|si| self.type_list_names(si))
            .unwrap_or_default()
    }

    /// The `extends` interface simple names from an `interface_declaration`'s
    /// `extends_interfaces` child (a child node, not a field, wrapping a
    /// `type_list`).
    fn extends_interface_names(&self, node: Node<'_>) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for c in node.children(&mut cursor) {
            if c.kind() == "extends_interfaces" {
                out.extend(self.type_list_names(c));
            }
        }
        out
    }

    /// The simple names of every `_type` in the `type_list` wrapped by `container`
    /// (a `super_interfaces`/`extends_interfaces` node).
    fn type_list_names(&self, container: Node<'_>) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor = container.walk();
        for c in container.children(&mut cursor) {
            if c.kind() != "type_list" {
                continue;
            }
            let mut inner = c.walk();
            for t in c.children(&mut inner) {
                if !t.is_named() {
                    continue;
                }
                let n = type_name(&self.text(t));
                if !n.is_empty() {
                    out.push(n);
                }
            }
        }
        out
    }

    fn push_relation(&mut self, kind: RelationKind, subject_fqn: &str, object: &str, span: &Span) {
        self.facts.impl_relations.push(ImplRelation {
            kind,
            subject: fqn_segments(subject_fqn),
            object: smallvec_one(object.to_string()),
            span: span.clone(),
        });
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
        // Record this method for the override pass. `ctx.fqn_prefix` is the
        // enclosing type's FQN (set by `enter_body` in `walk_type`), so methods
        // are grouped under the type that declares them.
        self.record_method(&ctx.fqn_prefix, &name, &fqn, node);
        // A `synchronized` method acquires its monitor on entry — a `Blocking`
        // own-effect on the method itself (the Java analogue of holding a lock for
        // the whole body). Attach it to the method FQN directly: `ctx.fqn_prefix`
        // here is still the enclosing type, not the method.
        if has_modifier(node, "synchronized") {
            self.record_effect_for(&fqn, EffectSet::single(Effect::Blocking));
        }
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
            let fn_fqn = body_ctx.fqn_prefix.clone();
            self.walk_body_ssa(body, &body_ctx, &fn_fqn);
        }
    }

    /// Stash a directly-declared method for the override pass: its arity (formal
    /// parameter count) and whether it carries `@Override`. Also index its
    /// `(name, arity)` under the enclosing type's simple name so subtypes can
    /// name+arity match against it in-file.
    fn record_method(&mut self, type_fqn: &str, name: &str, method_fqn: &str, node: Node<'_>) {
        let arity = self.method_arity(node);
        let has_override = self.annotation_names(node).iter().any(|a| a == "Override");
        let span = self.def_span(node, "name");
        self.type_methods
            .entry(type_fqn.to_string())
            .or_default()
            .push(MethodRec {
                name: name.to_string(),
                fqn: method_fqn.to_string(),
                arity,
                has_override,
                span,
            });
        let simple = type_fqn.rsplit("::").next().unwrap_or(type_fqn).to_string();
        self.type_method_sigs
            .entry(simple)
            .or_default()
            .insert((name.to_string(), arity));
    }

    /// The number of formal parameters of a `method_declaration` (varargs counts
    /// as one). Used for name+arity override matching.
    fn method_arity(&self, node: Node<'_>) -> usize {
        node.child_by_field_name("parameters")
            .map(|p| {
                let mut cursor = p.walk();
                p.children(&mut cursor)
                    .filter(|c| matches!(c.kind(), "formal_parameter" | "spread_parameter"))
                    .count()
            })
            .unwrap_or(0)
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
            let fn_fqn = body_ctx.fqn_prefix.clone();
            self.walk_body_ssa(body, &body_ctx, &fn_fqn);
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
                self.record_effects(ctx, effects_of_call(&name_path.join("::")));
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
                // A constructor of an effectful I/O type (`new FileReader(..)`,
                // `new Socket(..)`, `new ProcessBuilder(..)`) carries that type's
                // effect even before any method is called on it.
                self.record_effects(ctx, effects_of_call(&name));
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

    /// A `synchronized (lock) { … }` block acquires `lock`'s monitor — a
    /// `Blocking` own-effect on the enclosing callable. The guarded body is then
    /// walked normally for nested calls/effects.
    fn walk_synchronized(&mut self, node: Node<'_>, ctx: &Ctx, stmt_index: &mut u32) {
        self.record_effects(ctx, EffectSet::single(Effect::Blocking));
        self.walk_children(node, ctx, stmt_index);
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

    // --- intraprocedural SSA dataflow (Cycle 5) ---

    /// A second, dataflow-only traversal of a method/constructor body that lowers
    /// each production site (local-var initializer, assignment, return, for-each
    /// binding) into a [`DataFlowFact`] (`derived --derives-from(transform)-->
    /// source`). Re-assignment (`x = a; x = b;`) bumps the per-name SSA version so
    /// the resolver mints distinct value nodes (flow-sensitivity). A call/`new`
    /// result is an opaque source: it carries an `OpaqueCall` cut plus the callee
    /// name and per-arg access-paths so the interprocedural IFDS pass can ground
    /// it. This is the structural mirror of the Go adapter's `walk_function_ssa`.
    fn walk_body_ssa(&mut self, body: Node<'_>, ctx: &Ctx, fn_fqn: &str) {
        let mut versions: HashMap<String, u32> = HashMap::new();
        self.ssa_walk(body, ctx, fn_fqn, &mut versions);
    }

    fn ssa_walk(
        &mut self,
        node: Node<'_>,
        ctx: &Ctx,
        fn_fqn: &str,
        versions: &mut HashMap<String, u32>,
    ) {
        match node.kind() {
            // `T a = <init>, b = <init>;` — one assignment per initialized
            // declarator (an uninitialized declarator produces no flow).
            "local_variable_declaration" => {
                let cond = ctx.condition();
                let mut cursor = node.walk();
                let declarators: Vec<Node<'_>> = node
                    .children_by_field_name("declarator", &mut cursor)
                    .collect();
                for decl in declarators {
                    let Some(value) = decl.child_by_field_name("value") else {
                        continue;
                    };
                    if let Some(name) = decl
                        .child_by_field_name("name")
                        .and_then(|n| self.binding_name(n))
                    {
                        self.ssa_assignment(&name, value, ctx, fn_fqn, cond, versions);
                    }
                    self.ssa_walk(value, ctx, fn_fqn, versions);
                }
            }
            // `x = <rhs>` / `x += <rhs>` — only a plain-identifier target is
            // versioned (a field/array target is not a tracked local). Compound
            // operators under-approximate to the RHS shape, matching Go.
            "assignment_expression" => {
                let cond = ctx.condition();
                if let (Some(left), Some(right)) = (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) {
                    if let Some(name) = self.binding_name(left) {
                        self.ssa_assignment(&name, right, ctx, fn_fqn, cond, versions);
                    }
                    self.ssa_walk(right, ctx, fn_fqn, versions);
                }
            }
            "return_statement" => {
                let cond = ctx.condition();
                if let Some(expr) = node.named_child(0) {
                    self.ssa_return(expr, ctx, fn_fqn, cond, versions);
                    self.ssa_walk(expr, ctx, fn_fqn, versions);
                }
            }
            // Control flow lowers the same edge condition as the call walk, so a
            // flow inside an `if`/case/loop body carries the matching condition.
            "if_statement" => {
                if let Some(c) = node.child_by_field_name("condition") {
                    self.ssa_walk(c, ctx, fn_fqn, versions);
                }
                let inner = ctx.with_condition(EdgeCondition::Conditional);
                if let Some(c) = node.child_by_field_name("consequence") {
                    self.ssa_walk(c, &inner, fn_fqn, versions);
                }
                if let Some(a) = node.child_by_field_name("alternative") {
                    self.ssa_walk(a, &inner, fn_fqn, versions);
                }
            }
            "while_statement" | "for_statement" | "do_statement" => {
                if let Some(body) = node.child_by_field_name("body") {
                    let inner = ctx.with_condition(EdgeCondition::Loop);
                    self.ssa_walk(body, &inner, fn_fqn, versions);
                }
                // Walk non-body children (init/condition/update) unguarded.
                let body = node.child_by_field_name("body");
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if Some(child) == body {
                        continue;
                    }
                    self.ssa_walk(child, ctx, fn_fqn, versions);
                }
            }
            // `for (E e : iterable)` binds the loop var from each element of the
            // iterable — a Java-specific lowering with no Go analogue. The element
            // shape is unknown (no `Transform` variant captures "element-of"), so
            // the edge is tagged `Other` (under-approximate, but the derived→source
            // reachability `e ⇝ iterable` is recorded). A non-name iterable (a call
            // result) contributes no source, matching the opaque posture.
            "enhanced_for_statement" => {
                let loop_cond = ctx.with_condition(EdgeCondition::Loop).condition();
                if let (Some(name_node), Some(value)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("value"),
                ) {
                    if let Some(name) = self.binding_name(name_node) {
                        let version = bump(versions, &name);
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
                                loop_cond,
                                SmallVec::new(),
                                None,
                                SmallVec::new(),
                                span.clone(),
                            );
                        }
                    }
                    self.ssa_walk(value, ctx, fn_fqn, versions);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    let inner = ctx.with_condition(EdgeCondition::Loop);
                    self.ssa_walk(body, &inner, fn_fqn, versions);
                }
            }
            "switch_expression" => {
                if let Some(c) = node.child_by_field_name("condition") {
                    self.ssa_walk(c, ctx, fn_fqn, versions);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    let inner = ctx.with_condition(EdgeCondition::Conditional);
                    self.ssa_walk(body, &inner, fn_fqn, versions);
                }
            }
            // A lambda/anonymous-class body opens a new binding world; Cycle 5 does
            // not descend into it for SSA (closure-capture dataflow is deferred,
            // matching the Go adapter's `func_literal` skip).
            "lambda_expression" => {}
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.ssa_walk(child, ctx, fn_fqn, versions);
                }
            }
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
        versions: &mut HashMap<String, u32>,
    ) {
        let version = bump(versions, name);
        let derived: SmallVec<[Name; 2]> = smallvec_one(name.to_string());
        let span = self.span(value);
        let (transform, sources, cut) = self.classify_rhs(value);
        self.emit_flows(
            fn_fqn, derived, version, sources, cut, transform, ctx, cond, value, span,
        );
    }

    /// Emit the return dataflow fact: `<fn>::return --derives-from(copy)--> <expr>`
    /// for the syntactic operand of a `return`.
    fn ssa_return(
        &mut self,
        value: Node<'_>,
        ctx: &Ctx,
        fn_fqn: &str,
        cond: EdgeCondition,
        versions: &mut HashMap<String, u32>,
    ) {
        let version = bump(versions, "return");
        let mut derived: SmallVec<[Name; 2]> = SmallVec::new();
        derived.push(fn_fqn.to_string());
        derived.push("return".to_string());
        let span = self.span(value);
        let (_t, sources, cut) = self.classify_rhs(value);
        self.emit_flows(
            fn_fqn,
            derived,
            version,
            sources,
            cut,
            Transform::Copy,
            ctx,
            cond,
            value,
            span,
        );
    }

    /// Shared tail of [`Self::ssa_assignment`]/[`Self::ssa_return`]: push one fact
    /// per source operand, or a single source-less `OpaqueCall` fact (carrying the
    /// callee + per-arg access-paths) when the RHS is an opaque call/`new`.
    #[allow(clippy::too_many_arguments)]
    fn emit_flows(
        &mut self,
        fn_fqn: &str,
        derived: SmallVec<[Name; 2]>,
        version: u32,
        sources: Vec<SmallVec<[Name; 2]>>,
        cut: Option<CutMarker>,
        transform: Transform,
        ctx: &Ctx,
        cond: EdgeCondition,
        value: Node<'_>,
        span: Span,
    ) {
        let is_opaque_call = cut == Some(CutMarker::OpaqueCall);
        let callee = is_opaque_call
            .then(|| self.opaque_callee_name(value))
            .flatten();
        if sources.is_empty() && is_opaque_call {
            let args = self.opaque_call_arg_paths(value);
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

    /// Classify a RHS expression into `(transform, source access-paths, cut)`.
    /// Mirrors the Go adapter's `classify_rhs`: a call/`new` result is opaque (no
    /// intraprocedural source), a binary op is `Arith` over both operands, a
    /// ternary is a `Branched` φ-join over both branches.
    fn classify_rhs(
        &self,
        value: Node<'_>,
    ) -> (Transform, Vec<SmallVec<[Name; 2]>>, Option<CutMarker>) {
        match value.kind() {
            "identifier" => (Transform::Copy, vec![smallvec_one(self.text(value))], None),
            "field_access" => self.classify_field(value),
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
                (Transform::Arith, sources, None)
            }
            // `c ? a : b` selects from both branches (φ-join).
            "ternary_expression" => {
                let mut sources = Vec::new();
                for field in ["consequence", "alternative"] {
                    if let Some(n) = value.child_by_field_name(field) {
                        sources.extend(self.operand_sources(n));
                    }
                }
                (Transform::Branched, sources, None)
            }
            "cast_expression" => value
                .child_by_field_name("value")
                .map(|n| self.classify_rhs(n))
                .unwrap_or((Transform::Other, Vec::new(), None)),
            "parenthesized_expression" => value
                .named_child(0)
                .map(|n| self.classify_rhs(n))
                .unwrap_or((Transform::Other, Vec::new(), None)),
            // A method call or `new T(..)` runs opaque code (the resolver grounds
            // it interprocedurally via the callee + arg access-paths).
            "method_invocation" | "object_creation_expression" => {
                (Transform::Other, Vec::new(), Some(CutMarker::OpaqueCall))
            }
            _ => (Transform::Other, Vec::new(), None),
        }
    }

    /// Classify a `field_access` RHS: depth-1 (`u.name`) is a clean `Projection`;
    /// depth-2+ (`u.cfg.timeout`) truncates to the base local with a
    /// `TruncatedAccessPath` cut.
    fn classify_field(
        &self,
        value: Node<'_>,
    ) -> (Transform, Vec<SmallVec<[Name; 2]>>, Option<CutMarker>) {
        let Some(base) = value.child_by_field_name("object") else {
            return (Transform::Projection, Vec::new(), None);
        };
        let field = value
            .child_by_field_name("field")
            .map(|f| self.text(f))
            .unwrap_or_default();
        if base.kind() == "identifier" {
            let mut path: SmallVec<[Name; 2]> = SmallVec::new();
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

    /// The deepest base identifier of a nested `field_access` chain (`u.cfg.timeout`
    /// → `u`). `None` if the base is not a plain name.
    fn deepest_base_ident(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "identifier" => Some(self.text(node)),
            "field_access" => node
                .child_by_field_name("object")
                .and_then(|v| self.deepest_base_ident(v)),
            "parenthesized_expression" => node
                .named_child(0)
                .and_then(|v| self.deepest_base_ident(v)),
            _ => None,
        }
    }

    /// The source bindings of one operand of an arith/ternary expression.
    fn operand_sources(&self, node: Node<'_>) -> Vec<SmallVec<[Name; 2]>> {
        match node.kind() {
            "identifier" => vec![smallvec_one(self.text(node))],
            "field_access" => {
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
            "ternary_expression" => {
                let mut out = Vec::new();
                for field in ["consequence", "alternative"] {
                    if let Some(n) = node.child_by_field_name(field) {
                        out.extend(self.operand_sources(n));
                    }
                }
                out
            }
            "unary_expression" => node
                .child_by_field_name("operand")
                .map(|n| self.operand_sources(n))
                .unwrap_or_default(),
            "cast_expression" => node
                .child_by_field_name("value")
                .map(|n| self.operand_sources(n))
                .unwrap_or_default(),
            "parenthesized_expression" => node
                .named_child(0)
                .map(|n| self.operand_sources(n))
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// The bound name of an assignment/declaration target, when it is a plain
    /// identifier. Underscore patterns, field-access and array-access targets are
    /// not versioned (they are not tracked locals), matching the Go adapter.
    fn binding_name(&self, lhs: Node<'_>) -> Option<String> {
        match lhs.kind() {
            "identifier" => Some(self.text(lhs)),
            _ => None,
        }
    }

    /// Per-positional-argument source access-paths of an opaque-call RHS
    /// (`g(a, b.x, 1)` → `[["a"], ["b","x"], []]`): each named argument reduced
    /// through `operand_sources`, taking its first access-path; a literal/nested
    /// call contributes an empty inner vec to keep positional alignment.
    fn opaque_call_arg_paths(&self, value: Node<'_>) -> SmallVec<[SmallVec<[Name; 2]>; 4]> {
        let call = match value.kind() {
            "parenthesized_expression" => {
                return value
                    .named_child(0)
                    .map(|n| self.opaque_call_arg_paths(n))
                    .unwrap_or_default();
            }
            "cast_expression" => {
                return value
                    .child_by_field_name("value")
                    .map(|n| self.opaque_call_arg_paths(n))
                    .unwrap_or_default();
            }
            "method_invocation" | "object_creation_expression" => value,
            _ => return SmallVec::new(),
        };
        let Some(args) = call.child_by_field_name("arguments") else {
            return SmallVec::new();
        };
        let mut out: SmallVec<[SmallVec<[Name; 2]>; 4]> = SmallVec::new();
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

    /// The syntactic callee name path of an opaque-call RHS, `::`-joined. A bare
    /// `m(..)` yields `m`; a `new T(..)` yields the simple type name; a virtual
    /// `recv.m(..)` yields `None` (the resolver records a wildcard summary dep).
    fn opaque_callee_name(&self, value: Node<'_>) -> Option<String> {
        match value.kind() {
            "parenthesized_expression" => {
                value.named_child(0).and_then(|n| self.opaque_callee_name(n))
            }
            "cast_expression" => value
                .child_by_field_name("value")
                .and_then(|n| self.opaque_callee_name(n)),
            "method_invocation" => {
                if value.child_by_field_name("object").is_some() {
                    None
                } else {
                    value.child_by_field_name("name").map(|n| self.text(n))
                }
            }
            "object_creation_expression" => value
                .child_by_field_name("type")
                .map(|t| type_name(&self.text(t)))
                .filter(|n| !n.is_empty()),
            _ => None,
        }
    }

    /// Push one `DataFlowFact` into the per-callable accumulator (flushed in
    /// [`Builder::finish`]).
    #[allow(clippy::too_many_arguments)]
    fn push_data_flow(
        &mut self,
        fn_fqn: &str,
        derived: SmallVec<[Name; 2]>,
        derived_version: u32,
        source: SmallVec<[Name; 2]>,
        scope: ScopeId,
        transform: Transform,
        edge_condition: EdgeCondition,
        cut_markers: SmallVec<[CutMarker; 1]>,
        callee_fqn: Option<String>,
        args: SmallVec<[SmallVec<[Name; 2]>; 4]>,
        span: Span,
    ) {
        self.data_flows
            .entry(fn_fqn.to_string())
            .or_default()
            .push(DataFlowFact {
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

/// Increment and return the SSA version of `name` in `versions` (1 on first use).
fn bump(versions: &mut HashMap<String, u32>, name: &str) -> u32 {
    let v = versions.entry(name.to_string()).or_insert(0);
    *v += 1;
    *v
}

/// Split a `::`-joined FQN into its segments (`a::B::m` → `[a, B, m]`). The
/// resolver re-joins them for an exact-FQN lookup.
fn fqn_segments(fqn: &str) -> SmallVec<[Name; 2]> {
    fqn.split("::").map(str::to_string).collect()
}

/// A two-segment `[type, member]` name path (`("Shape", "area")` →
/// `["Shape", "area"]`). The override `object`: a supertype's simple name plus
/// the overridden method name, which the resolver grounds via `Shape::area`.
fn name_path_two(type_name: &str, member: &str) -> SmallVec<[Name; 2]> {
    let mut v = SmallVec::new();
    v.push(type_name.to_string());
    v.push(member.to_string());
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
