//! The TypeScript [`LanguageFrontend`] implementation and the AST-walking
//! extraction engine.
//!
//! # Design
//!
//! The extractor does a single recursive walk of the tree-sitter parse tree,
//! carrying:
//!
//! - **`scope`** — the current [`ScopeId`] (updated when entering a callable or
//!   type body).
//! - **`condition_stack`** — a stack of [`EdgeCondition`] frames; the ADR-03
//!   maximum of all active frames is the condition assigned to calls at any point.
//! - **`lambda_names`** — a set of local binding names that hold arrow functions
//!   or function expressions; calls to these emit [`RefKind::CallClosure`].
//! - **`stmt_index`** — a shared monotonic counter used as the intra-procedural
//!   statement index (ADR-02 reservation).
//!
//! The visitor first pushes any new scope or condition frame, records defs/imports
//! /exports, then recurses into children. On exit from a scope/condition frame the
//! stack is popped.

use crate::module::module_path_for;
use crate::query;
use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_core::signature::{Param, Signature};
use cgx_frontend::facts::{
    CutHint, EntrypointHint, ExportFact, FileFacts, ImportFact, ImportedName, RawRef, RefKind,
    ScopeId, ScopeTree, SymbolDef,
};
use cgx_frontend::frontend::{FileCtx, FrontendError, Lang, LanguageFrontend, RelPath};
use smallvec::SmallVec;
use std::collections::HashSet;
use tree_sitter::{Language, Node, Parser};

/// Fragment version for the TS adapter extraction rules.  Bump this when
/// extraction logic changes in a way that invalidates cached fragments.
const TS_FRAGMENT_VERSION: u32 = 1;

/// The TypeScript/JavaScript language frontend.
///
/// Handles `.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts` files.
/// Uses `tree-sitter-typescript`'s `LANGUAGE_TYPESCRIPT` grammar for TS/TSX
/// and the same grammar (with JS compatibility) for JS variants.
pub struct TypeScriptFrontend {
    ts_language: Language,
}

impl std::fmt::Debug for TypeScriptFrontend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeScriptFrontend").finish_non_exhaustive()
    }
}

impl TypeScriptFrontend {
    /// Create a new TypeScript frontend using the tree-sitter-typescript grammar.
    pub fn new() -> Self {
        TypeScriptFrontend {
            ts_language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }
}

impl Default for TypeScriptFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageFrontend for TypeScriptFrontend {
    fn lang(&self) -> Lang {
        Lang::TypeScript
    }

    fn handles(&self, path: &RelPath) -> bool {
        path.extension()
            .map(|ext| {
                matches!(
                    ext.as_str(),
                    "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs"
                )
            })
            .unwrap_or(false)
    }

    fn fragment_version(&self) -> u32 {
        TS_FRAGMENT_VERSION
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut parser = Parser::new();
        parser
            .set_language(&self.ts_language)
            .map_err(|e| FrontendError::Parser {
                lang: "typescript".to_string(),
                detail: e.to_string(),
            })?;

        let tree = match parser.parse(src, None) {
            Some(t) => t,
            None => return Ok(FileFacts::empty()),
        };

        let module_prefix = module_path_for(ctx.path.as_str());

        let mut builder = Builder::new(src, ctx.path.as_str(), &module_prefix);
        builder.walk_program(tree.root_node());
        let mut facts = builder.finish();
        facts.canonicalize();
        Ok(facts)
    }
}

// ---------------------------------------------------------------------------
// Builder — the extraction state machine
// ---------------------------------------------------------------------------

/// Condition context frame pushed onto the condition stack.
#[derive(Clone, Copy)]
struct CondFrame {
    condition: EdgeCondition,
}

/// The full extraction state for one file walk.
struct Builder<'a> {
    src: &'a [u8],
    file: String,
    module_prefix: String,
    facts: FileFacts,
    /// Stack of edge condition frames; the effective condition is the max over
    /// all active frames (ADR-03).
    condition_stack: Vec<CondFrame>,
    /// Monotonic statement index within the current callable scope.
    stmt_index: u32,
    /// FQNs of local lambda bindings (`const f = () => …`). When we see a
    /// `call_expression` whose callee is one of these names, we emit
    /// `RefKind::CallClosure` instead of `RefKind::Call`.
    lambda_names: HashSet<String>,
    /// FQNs of callable parameter bindings (function-typed parameters). Calls
    /// to these emit `RefKind::CallCallback`.
    callback_names: HashSet<String>,
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], file: &str, module_prefix: &str) -> Self {
        Builder {
            src,
            file: file.to_string(),
            module_prefix: module_prefix.to_string(),
            facts: FileFacts {
                scopes: ScopeTree::new(),
                ..FileFacts::default()
            },
            condition_stack: Vec::new(),
            stmt_index: 0,
            lambda_names: HashSet::new(),
            callback_names: HashSet::new(),
        }
    }

    fn finish(self) -> FileFacts {
        self.facts
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn text(&self, node: Node<'_>) -> &str {
        node.utf8_text(self.src).unwrap_or("")
    }

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

    /// Current effective edge condition: maximum by precedence of all active
    /// frames, or `Always` when the stack is empty.
    fn current_condition(&self) -> EdgeCondition {
        self.condition_stack
            .iter()
            .fold(EdgeCondition::Always, |acc, f| acc.max(f.condition))
    }

    fn push_condition(&mut self, cond: EdgeCondition) {
        self.condition_stack.push(CondFrame { condition: cond });
    }

    fn pop_condition(&mut self) {
        self.condition_stack.pop();
    }

    fn fqn(&self, owner: Option<&str>, name: &str) -> String {
        match owner {
            Some(o) if !o.is_empty() => format!("{o}::{name}"),
            _ => format!("{}::{name}", self.module_prefix),
        }
    }

    /// Retrieve the FQN of the owner of scope `id` (walking up to the first
    /// scope with an owner_fqn).
    fn scope_owner_fqn(&self, scope: ScopeId) -> Option<String> {
        self.facts.scopes.ancestors(scope).find_map(|id| {
            self.facts
                .scopes
                .scopes
                .get(id.index())
                .and_then(|s| s.owner_fqn.clone())
        })
    }

    // -----------------------------------------------------------------------
    // Top-level program walk — processes imports, exports, and top-level defs
    // -----------------------------------------------------------------------

    fn walk_program(&mut self, root: Node<'_>) {
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            match child.kind() {
                "import_statement" => self.handle_import(child),
                "export_statement" => self.handle_export(child, ScopeId::ROOT),
                "lexical_declaration" | "variable_declaration" => {
                    self.handle_lexical_decl(child, ScopeId::ROOT)
                }
                "function_declaration" | "generator_function_declaration" => {
                    self.handle_function_decl(child, ScopeId::ROOT, None)
                }
                "class_declaration" | "abstract_class_declaration" => {
                    self.handle_class_decl(child, ScopeId::ROOT, None, Visibility::Private)
                }
                "interface_declaration" => {
                    self.handle_interface_decl(child, ScopeId::ROOT, None, Visibility::Private)
                }
                "type_alias_declaration" | "enum_declaration" => {
                    self.handle_named_type(child, ScopeId::ROOT, None, Visibility::Private)
                }
                "expression_statement" => self.handle_expr_stmt(child, ScopeId::ROOT),
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Import handling
    // -----------------------------------------------------------------------

    fn handle_import(&mut self, node: Node<'_>) {
        // import { a, b as c } from "./module"
        // import * as ns from "./module"
        // import type { ... } from "./module"
        let source_node = node.child_by_field_name("source");
        let specifier = source_node
            .and_then(|n| query::extract_string_value(self.src, n))
            .unwrap_or_default();

        if specifier.is_empty() {
            return;
        }

        let span = self.span(node);

        // Walk the import clause to collect named bindings.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "import_clause" {
                self.handle_import_clause(child, &specifier, span.clone(), false);
            }
        }
    }

    fn handle_import_clause(
        &mut self,
        clause: Node<'_>,
        specifier: &str,
        span: Span,
        re_export: bool,
    ) {
        let mut cursor = clause.walk();
        for child in clause.children(&mut cursor) {
            match child.kind() {
                "named_imports" => {
                    self.handle_named_imports(child, specifier, span.clone(), re_export);
                }
                "namespace_import" => {
                    // import * as ns from "m" — not extracting as separate names
                    // but record a glob import
                    self.facts.imports.push(ImportFact {
                        specifier: specifier.to_string(),
                        names: vec![],
                        glob: true,
                        re_export,
                        scope: ScopeId::ROOT,
                        span: span.clone(),
                    });
                }
                "identifier" => {
                    // import defaultExport from "m"
                    let name = self.text(child).to_string();
                    self.facts.imports.push(ImportFact {
                        specifier: specifier.to_string(),
                        names: vec![ImportedName {
                            name: "default".to_string(),
                            alias: Some(name),
                        }],
                        glob: false,
                        re_export,
                        scope: ScopeId::ROOT,
                        span: span.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    fn handle_named_imports(
        &mut self,
        named: Node<'_>,
        specifier: &str,
        span: Span,
        re_export: bool,
    ) {
        let mut names = Vec::new();
        let mut cursor = named.walk();
        for child in named.children(&mut cursor) {
            if child.kind() == "import_specifier" {
                let name_node = child.child_by_field_name("name");
                let alias_node = child.child_by_field_name("alias");
                let name = name_node
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_default();
                let alias = alias_node.map(|n| self.text(n).to_string());
                if !name.is_empty() {
                    names.push(ImportedName { name, alias });
                }
            }
        }
        if !names.is_empty() {
            self.facts.imports.push(ImportFact {
                specifier: specifier.to_string(),
                names,
                glob: false,
                re_export,
                scope: ScopeId::ROOT,
                span,
            });
        }
    }

    // -----------------------------------------------------------------------
    // Export handling
    // -----------------------------------------------------------------------

    fn handle_export(&mut self, node: Node<'_>, scope: ScopeId) {
        // export { a, b as c } from "./m"  — re-export
        // export * from "./m"              — glob re-export
        // export function foo() { ... }    — exported declaration
        // export default expr              — default export
        // export { a, b }                  — local re-export (no `from`)
        let source_node = node.child_by_field_name("source");
        let from_specifier = source_node.and_then(|n| query::extract_string_value(self.src, n));

        let span = self.span(node);

        // Check for `export * from "m"`
        let mut cursor = node.walk();
        let mut has_namespace_export = false;
        let mut decl_child: Option<Node<'_>> = None;
        for child in node.children(&mut cursor) {
            match child.kind() {
                "namespace_export" => {
                    has_namespace_export = true;
                }
                // Plain `export * from "m"` uses a bare `*` token child
                "*" => {
                    has_namespace_export = true;
                }
                "export_clause" => {
                    // Handle individual specifiers
                    let mut c2 = child.walk();
                    for spec in child.children(&mut c2) {
                        if spec.kind() == "export_specifier" {
                            let name_node = spec.child_by_field_name("name");
                            let alias_node = spec.child_by_field_name("alias");
                            let name = name_node
                                .map(|n| self.text(n).to_string())
                                .unwrap_or_default();
                            let alias = alias_node.map(|n| self.text(n).to_string());

                            if let Some(ref from) = from_specifier {
                                // Re-export from another module: record both as
                                // an import (re_export=true) and as an export fact.
                                self.facts.imports.push(ImportFact {
                                    specifier: from.clone(),
                                    names: vec![ImportedName {
                                        name: name.clone(),
                                        alias: alias.clone(),
                                    }],
                                    glob: false,
                                    re_export: true,
                                    scope: ScopeId::ROOT,
                                    span: span.clone(),
                                });
                            }

                            if !name.is_empty() {
                                self.facts.exports.push(ExportFact {
                                    name: name.clone(),
                                    alias: alias.clone(),
                                    from: from_specifier.clone(),
                                    span: span.clone(),
                                });
                            }
                        }
                    }
                }
                "function_declaration"
                | "generator_function_declaration"
                | "function_expression" => {
                    decl_child = Some(child);
                }
                "class_declaration" | "abstract_class_declaration" | "class" => {
                    decl_child = Some(child);
                }
                "interface_declaration" => {
                    decl_child = Some(child);
                }
                "lexical_declaration" | "variable_declaration" => {
                    decl_child = Some(child);
                }
                "type_alias_declaration" | "enum_declaration" => {
                    decl_child = Some(child);
                }
                _ => {}
            }
        }

        if has_namespace_export {
            // export * from "m" or export * as ns from "m"
            if let Some(ref from) = from_specifier {
                self.facts.imports.push(ImportFact {
                    specifier: from.clone(),
                    names: vec![],
                    glob: true,
                    re_export: true,
                    scope: ScopeId::ROOT,
                    span: span.clone(),
                });
            }
        }

        // Process the declared child with public visibility
        if let Some(decl) = decl_child {
            match decl.kind() {
                "function_declaration" | "generator_function_declaration" => {
                    self.handle_function_decl(decl, scope, Some(Visibility::Public));
                }
                "class_declaration" | "abstract_class_declaration" | "class" => {
                    self.handle_class_decl(decl, scope, None, Visibility::Public);
                }
                "interface_declaration" => {
                    self.handle_interface_decl(decl, scope, None, Visibility::Public);
                }
                "lexical_declaration" | "variable_declaration" => {
                    self.handle_lexical_decl_with_vis(decl, scope, Visibility::Public);
                }
                "type_alias_declaration" | "enum_declaration" => {
                    self.handle_named_type(decl, scope, None, Visibility::Public);
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Function declaration handling
    // -----------------------------------------------------------------------

    fn handle_function_decl(
        &mut self,
        node: Node<'_>,
        parent_scope: ScopeId,
        forced_vis: Option<Visibility>,
    ) {
        let name_node = node.child_by_field_name("name");
        let name = match name_node {
            Some(n) => self.text(n).to_string(),
            None => return, // anonymous function expression at top level — skip
        };

        let parent_fqn = self.scope_owner_fqn(parent_scope);
        let fqn = self.fqn(parent_fqn.as_deref(), &name);

        let is_async = self.node_has_async_modifier(node);
        let vis = forced_vis.unwrap_or(Visibility::Private);

        let signature = self.extract_function_signature(node, None);
        let new_scope = self.facts.scopes.push(parent_scope, Some(fqn.clone()));

        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Function,
            visibility: vis,
            scope: parent_scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: Some(signature),
        });

        // Detect entrypoints
        if name == "main" {
            self.facts.entrypoint_hints.push(EntrypointHint {
                fqn: fqn.clone(),
                kind: EntrypointKind::Main,
            });
        }

        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            // If async, the body context doesn't change edge conditions itself
            // (await expressions inside push CallAsync refs)
            let _ = is_async;
            self.walk_body(body, new_scope);
        }
    }

    fn node_has_async_modifier(&self, node: Node<'_>) -> bool {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "async" {
                return true;
            }
        }
        false
    }

    fn extract_function_signature(&self, node: Node<'_>, receiver: Option<&str>) -> Signature {
        let params_node = node.child_by_field_name("parameters");
        let return_node = node.child_by_field_name("return_type");
        let type_params_node = node.child_by_field_name("type_parameters");

        let params = params_node
            .map(|n| self.extract_params(n))
            .unwrap_or_default();

        let return_type_text = return_node.and_then(|n| {
            // The return_type node is `type_annotation`; its text includes `:`.
            // Grab the text after the colon.
            let full = n.utf8_text(self.src).ok()?;
            Some(
                full.trim_start_matches(':')
                    .trim_start_matches("=>")
                    .trim()
                    .to_string(),
            )
        });

        let type_params = type_params_node
            .map(|n| self.extract_type_params(n))
            .unwrap_or_default();

        Signature {
            params,
            return_type_text,
            type_params,
            receiver: receiver.map(|s| s.to_string()),
        }
    }

    fn extract_params(&self, params_node: Node<'_>) -> Vec<Param> {
        let mut params = Vec::new();
        let mut cursor = params_node.walk();
        for child in params_node.children(&mut cursor) {
            match child.kind() {
                "required_parameter" | "optional_parameter" => {
                    let is_optional = child.kind() == "optional_parameter";
                    let name_node = child.child_by_field_name("pattern");
                    let type_node = child.child_by_field_name("type");
                    let name = name_node
                        .map(|n| self.text(n).to_string())
                        .unwrap_or_else(|| "_".to_string());
                    let type_text = type_node.and_then(|n| {
                        let t = n.utf8_text(self.src).ok()?;
                        Some(t.trim_start_matches(':').trim().to_string())
                    });
                    params.push(Param {
                        name,
                        type_text,
                        has_default: is_optional,
                        variadic: false,
                    });
                }
                "rest_pattern" => {
                    let inner = child
                        .named_child(0)
                        .map(|n| self.text(n).to_string())
                        .unwrap_or_else(|| "args".to_string());
                    params.push(Param {
                        name: inner,
                        type_text: None,
                        has_default: false,
                        variadic: true,
                    });
                }
                _ => {}
            }
        }
        params
    }

    fn extract_type_params(&self, tp_node: Node<'_>) -> Vec<String> {
        let mut result = Vec::new();
        let mut cursor = tp_node.walk();
        for child in tp_node.children(&mut cursor) {
            if child.kind() == "type_parameter" {
                let text = self.text(child);
                if !text.is_empty() {
                    result.push(text.to_string());
                }
            }
        }
        result
    }

    // -----------------------------------------------------------------------
    // Class declaration handling
    // -----------------------------------------------------------------------

    fn handle_class_decl(
        &mut self,
        node: Node<'_>,
        parent_scope: ScopeId,
        forced_name: Option<&str>,
        vis: Visibility,
    ) {
        let name_node = node.child_by_field_name("name");
        let name = forced_name
            .map(|s| s.to_string())
            .or_else(|| name_node.map(|n| self.text(n).to_string()));

        let name = match name {
            Some(n) => n,
            None => return,
        };

        let is_abstract = node.kind() == "abstract_class_declaration";
        let parent_fqn = self.scope_owner_fqn(parent_scope);
        let fqn = self.fqn(parent_fqn.as_deref(), &name);

        let new_scope = self.facts.scopes.push(parent_scope, Some(fqn.clone()));

        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Type,
            visibility: vis,
            scope: parent_scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract,
            signature: None,
        });

        // Walk the class body
        if let Some(body) = node.child_by_field_name("body") {
            self.walk_class_body(body, new_scope, &fqn, vis);
        }
    }

    fn walk_class_body(
        &mut self,
        body: Node<'_>,
        class_scope: ScopeId,
        class_fqn: &str,
        class_vis: Visibility,
    ) {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "method_definition" => {
                    self.handle_method_def(child, class_scope, class_fqn, class_vis);
                }
                "abstract_method_signature" => {
                    self.handle_abstract_method(child, class_scope, class_fqn, class_vis);
                }
                "public_field_definition" => {
                    self.handle_field_def(child, class_scope, class_fqn, class_vis);
                }
                _ => {}
            }
        }
    }

    fn handle_method_def(
        &mut self,
        node: Node<'_>,
        class_scope: ScopeId,
        class_fqn: &str,
        _class_vis: Visibility,
    ) {
        let name_node = node.child_by_field_name("name");
        let name = match name_node {
            Some(n) => self.text(n).to_string(),
            None => return,
        };

        // Determine visibility: look for accessibility_modifier
        let vis = self.method_visibility(node);

        let fqn = format!("{class_fqn}::{name}");

        // is_abstract comes from the abstract keyword on the class
        let is_abstract = self.node_has_modifier(node, "abstract");

        let signature = self.extract_function_signature(node, Some(class_fqn));
        let new_scope = self.facts.scopes.push(class_scope, Some(fqn.clone()));

        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Method,
            visibility: vis,
            scope: class_scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract,
            signature: Some(signature),
        });

        // Walk the method body
        if let Some(body) = node.child_by_field_name("body") {
            self.walk_body(body, new_scope);
        }
    }

    fn handle_abstract_method(
        &mut self,
        node: Node<'_>,
        class_scope: ScopeId,
        class_fqn: &str,
        _class_vis: Visibility,
    ) {
        let name_node = node.child_by_field_name("name");
        let name = match name_node {
            Some(n) => self.text(n).to_string(),
            None => return,
        };

        let vis = self.method_visibility(node);
        let fqn = format!("{class_fqn}::{name}");
        let signature = self.extract_function_signature(node, Some(class_fqn));

        self.facts.defs.push(SymbolDef {
            fqn,
            kind: SymbolKind::Method,
            visibility: vis,
            scope: class_scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: true,
            signature: Some(signature),
        });
        // No body to walk for abstract methods
    }

    fn handle_field_def(
        &mut self,
        node: Node<'_>,
        class_scope: ScopeId,
        class_fqn: &str,
        _class_vis: Visibility,
    ) {
        let name_node = node.child_by_field_name("name");
        let name = match name_node {
            Some(n) => self.text(n).to_string(),
            None => return,
        };

        let vis = self.method_visibility(node);
        let fqn = format!("{class_fqn}::{name}");

        self.facts.defs.push(SymbolDef {
            fqn,
            kind: SymbolKind::Field,
            visibility: vis,
            scope: class_scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
    }

    fn method_visibility(&self, node: Node<'_>) -> Visibility {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "accessibility_modifier" {
                match self.text(child) {
                    "public" => return Visibility::Public,
                    "protected" => return Visibility::Protected,
                    "private" => return Visibility::Private,
                    _ => {}
                }
            }
        }
        // Default in TypeScript: public if no modifier
        Visibility::Public
    }

    fn node_has_modifier(&self, node: Node<'_>, modifier: &str) -> bool {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == modifier {
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // Interface declaration handling
    // -----------------------------------------------------------------------

    fn handle_interface_decl(
        &mut self,
        node: Node<'_>,
        parent_scope: ScopeId,
        _forced_name: Option<&str>,
        vis: Visibility,
    ) {
        let name_node = node.child_by_field_name("name");
        let name = match name_node {
            Some(n) => self.text(n).to_string(),
            None => return,
        };

        let parent_fqn = self.scope_owner_fqn(parent_scope);
        let fqn = self.fqn(parent_fqn.as_deref(), &name);

        let new_scope = self.facts.scopes.push(parent_scope, Some(fqn.clone()));

        // Interfaces are abstract types
        self.facts.defs.push(SymbolDef {
            fqn: fqn.clone(),
            kind: SymbolKind::Type,
            visibility: vis,
            scope: parent_scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: true,
            signature: None,
        });

        // Walk interface body for method signatures
        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                if child.kind() == "method_signature" || child.kind() == "call_signature" {
                    let m_name_node = child.child_by_field_name("name");
                    if let Some(m_name_node) = m_name_node {
                        let m_name = self.text(m_name_node).to_string();
                        let m_fqn = format!("{fqn}::{m_name}");
                        let sig = self.extract_function_signature(child, Some(&fqn));
                        self.facts.defs.push(SymbolDef {
                            fqn: m_fqn,
                            kind: SymbolKind::Method,
                            visibility: Visibility::Public,
                            scope: new_scope,
                            span: self.span(child),
                            line_end: self.end_line(child),
                            is_abstract: true,
                            signature: Some(sig),
                        });
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Named type (type alias, enum) handling
    // -----------------------------------------------------------------------

    fn handle_named_type(
        &mut self,
        node: Node<'_>,
        parent_scope: ScopeId,
        _forced_name: Option<&str>,
        vis: Visibility,
    ) {
        let name_node = node.child_by_field_name("name");
        let name = match name_node {
            Some(n) => self.text(n).to_string(),
            None => return,
        };

        let parent_fqn = self.scope_owner_fqn(parent_scope);
        let fqn = self.fqn(parent_fqn.as_deref(), &name);

        self.facts.defs.push(SymbolDef {
            fqn,
            kind: SymbolKind::Type,
            visibility: vis,
            scope: parent_scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            signature: None,
        });
    }

    // -----------------------------------------------------------------------
    // Lexical declaration handling (const/let/var)
    // -----------------------------------------------------------------------

    fn handle_lexical_decl(&mut self, node: Node<'_>, scope: ScopeId) {
        self.handle_lexical_decl_with_vis(node, scope, Visibility::Private);
    }

    fn handle_lexical_decl_with_vis(&mut self, node: Node<'_>, scope: ScopeId, vis: Visibility) {
        // lexical_declaration = (const|let) variable_declarator+
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "variable_declarator" {
                self.handle_variable_declarator(child, scope, vis);
            }
        }
    }

    fn handle_variable_declarator(&mut self, node: Node<'_>, scope: ScopeId, vis: Visibility) {
        let name_node = node.child_by_field_name("name");
        let value_node = node.child_by_field_name("value");

        let name = match name_node {
            Some(n) => self.text(n).to_string(),
            None => return,
        };

        // Detect arrow functions / function expressions
        let value_kind = value_node.map(|n| n.kind());
        let is_lambda = value_kind
            .map(|k| k == "arrow_function" || k == "function_expression")
            .unwrap_or(false);

        let parent_fqn = self.scope_owner_fqn(scope);
        let fqn = self.fqn(parent_fqn.as_deref(), &name);

        if is_lambda {
            let lambda_node = value_node.unwrap();
            let signature = self.extract_function_signature(lambda_node, None);
            let new_scope = self.facts.scopes.push(scope, Some(fqn.clone()));

            self.lambda_names.insert(name.clone());

            self.facts.defs.push(SymbolDef {
                fqn: fqn.clone(),
                kind: SymbolKind::Lambda,
                visibility: vis,
                scope,
                span: self.span(node),
                line_end: self.end_line(node),
                is_abstract: false,
                signature: Some(signature),
            });

            if let Some(body) = lambda_node.child_by_field_name("body") {
                self.walk_body(body, new_scope);
            }
        } else {
            // Plain constant/variable
            self.facts.defs.push(SymbolDef {
                fqn: fqn.clone(),
                kind: SymbolKind::Constant,
                visibility: vis,
                scope,
                span: self.span(node),
                line_end: self.end_line(node),
                is_abstract: false,
                signature: None,
            });

            // Walk value for any embedded calls
            if let Some(val) = value_node {
                self.walk_expr(val, scope);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expression statement (for top-level calls like test(), describe())
    // -----------------------------------------------------------------------

    fn handle_expr_stmt(&mut self, node: Node<'_>, scope: ScopeId) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "call_expression" {
                // Check for jest-style test/describe/it calls at the top level
                let callee = child.child_by_field_name("function");
                if let Some(callee_node) = callee {
                    let callee_text = self.text(callee_node);
                    if matches!(callee_text, "test" | "it" | "describe") {
                        // Extract the first string arg as the test name
                        if let Some(args_node) = child.child_by_field_name("arguments") {
                            let mut ac = args_node.walk();
                            for arg in args_node.children(&mut ac) {
                                if arg.kind() == "string" {
                                    let test_name = query::extract_string_value(self.src, arg)
                                        .unwrap_or_else(|| callee_text.to_string());
                                    let fqn = format!("{}::{}", self.module_prefix, test_name);
                                    self.facts.entrypoint_hints.push(EntrypointHint {
                                        fqn,
                                        kind: EntrypointKind::Test,
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
                // Also record the call itself
                self.walk_expr(child, scope);
            } else {
                self.walk_expr(child, scope);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Statement body walking — walks statements inside a function/method body
    // -----------------------------------------------------------------------

    fn walk_body(&mut self, body: Node<'_>, scope: ScopeId) {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            self.walk_stmt(child, scope);
        }
    }

    fn walk_stmt(&mut self, node: Node<'_>, scope: ScopeId) {
        match node.kind() {
            "return_statement" | "expression_statement" | "throw_statement" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() {
                        self.walk_expr(child, scope);
                    }
                }
            }
            "if_statement" => self.walk_if_stmt(node, scope),
            "switch_statement" => self.walk_switch_stmt(node, scope),
            "for_statement" | "for_in_statement" => self.walk_for_stmt(node, scope),
            "while_statement" | "do_statement" => self.walk_while_stmt(node, scope),
            "try_statement" => self.walk_try_stmt(node, scope),
            "lexical_declaration" | "variable_declaration" => self.handle_lexical_decl(node, scope),
            "function_declaration" | "generator_function_declaration" => {
                self.handle_function_decl(node, scope, None)
            }
            "class_declaration" | "abstract_class_declaration" => {
                self.handle_class_decl(node, scope, None, Visibility::Private)
            }
            "statement_block" => self.walk_body(node, scope),
            _ => {
                // Recurse into nested expressions or statements
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() {
                        self.walk_expr(child, scope);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Control flow walking — pushes condition frames
    // -----------------------------------------------------------------------

    fn walk_if_stmt(&mut self, node: Node<'_>, scope: ScopeId) {
        // condition: walk the test expression without a condition frame (it's
        // just computing a value, not calling under a condition)
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "parenthesized_expression" => {
                    // test condition — evaluate without condition frame
                    self.walk_expr(child, scope);
                }
                "statement_block"
                | "return_statement"
                | "expression_statement"
                | "lexical_declaration"
                | "variable_declaration"
                | "throw_statement" => {
                    // consequent / alternate — conditional
                    self.push_condition(EdgeCondition::Conditional);
                    self.walk_stmt(child, scope);
                    self.pop_condition();
                }
                "else_clause" => {
                    self.push_condition(EdgeCondition::Conditional);
                    // Walk children of else clause
                    let mut ec = child.walk();
                    for else_child in child.children(&mut ec) {
                        if else_child.is_named() {
                            self.walk_stmt(else_child, scope);
                        }
                    }
                    self.pop_condition();
                }
                _ => {}
            }
        }
    }

    fn walk_switch_stmt(&mut self, node: Node<'_>, scope: ScopeId) {
        // Walk switch body — all cases are conditional
        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                if child.kind() == "switch_case" || child.kind() == "switch_default" {
                    self.push_condition(EdgeCondition::Conditional);
                    let mut cc = child.walk();
                    for case_child in child.children(&mut cc) {
                        if case_child.is_named() {
                            self.walk_stmt(case_child, scope);
                        }
                    }
                    self.pop_condition();
                }
            }
        }
    }

    fn walk_for_stmt(&mut self, node: Node<'_>, scope: ScopeId) {
        // For loop body: loop condition
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            self.push_condition(EdgeCondition::Loop);
            self.walk_stmt(body, scope);
            self.pop_condition();
        }
    }

    fn walk_while_stmt(&mut self, node: Node<'_>, scope: ScopeId) {
        // While / do-while body: loop condition
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            self.push_condition(EdgeCondition::Loop);
            self.walk_stmt(body, scope);
            self.pop_condition();
        }
    }

    fn walk_try_stmt(&mut self, node: Node<'_>, scope: ScopeId) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "statement_block" => {
                    // try body — always condition (GM-3.1: try body = always)
                    self.walk_body(child, scope);
                }
                "catch_clause" => {
                    // catch body — exception condition
                    self.push_condition(EdgeCondition::Exception);
                    // Walk the catch body (statement_block inside catch_clause)
                    let mut cc = child.walk();
                    for catch_child in child.children(&mut cc) {
                        if catch_child.kind() == "statement_block" {
                            self.walk_body(catch_child, scope);
                        }
                    }
                    self.pop_condition();
                }
                "finally_clause" => {
                    // finally body — always condition (GM-3.1 carve-out)
                    // The condition is already Always by default but we want to
                    // ensure any outer exception/conditional frames don't affect
                    // finally body: we temporarily clear by pushing Always.
                    // Actually per GM-3.1, finally is always regardless of outer.
                    // We push Always to override any outer exception frame.
                    self.push_condition(EdgeCondition::Always);
                    let mut fc = child.walk();
                    for fc_child in child.children(&mut fc) {
                        if fc_child.kind() == "statement_block" {
                            self.walk_body(fc_child, scope);
                        }
                    }
                    self.pop_condition();
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expression walking — handles calls, awaits, arrow functions etc.
    // -----------------------------------------------------------------------

    fn walk_expr(&mut self, node: Node<'_>, scope: ScopeId) {
        match node.kind() {
            "call_expression" => self.handle_call_expr(node, scope),
            "new_expression" => self.handle_new_expr(node, scope),
            "await_expression" => self.handle_await_expr(node, scope),
            "arrow_function" | "function_expression" => {
                // Anonymous inline function — create a lambda scope, walk its body
                let new_scope = self.facts.scopes.push(scope, None);
                if let Some(body) = node.child_by_field_name("body") {
                    match body.kind() {
                        "statement_block" => self.walk_body(body, new_scope),
                        _ => self.walk_expr(body, new_scope),
                    }
                }
            }
            "if_statement" => self.walk_if_stmt(node, scope),
            "switch_statement" => self.walk_switch_stmt(node, scope),
            "for_statement" | "for_in_statement" => self.walk_for_stmt(node, scope),
            "while_statement" | "do_statement" => self.walk_while_stmt(node, scope),
            "try_statement" => self.walk_try_stmt(node, scope),
            "ternary_expression" => self.walk_ternary(node, scope),
            "binary_expression"
            | "assignment_expression"
            | "augmented_assignment_expression"
            | "sequence_expression"
            | "parenthesized_expression"
            | "non_null_expression"
            | "as_expression"
            | "type_assertion"
            | "satisfies_expression" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() {
                        self.walk_expr(child, scope);
                    }
                }
            }
            "member_expression" | "subscript_expression" | "optional_chain" => {
                // Member access — walk sub-expressions but don't emit a call ref
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() && child.kind() != "property_identifier" {
                        self.walk_expr(child, scope);
                    }
                }
            }
            "array" | "object" | "template_string" => {
                // Container literals — walk elements
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() {
                        self.walk_expr(child, scope);
                    }
                }
            }
            "statement_block" => self.walk_body(node, scope),
            _ => {
                // Recurse for any other named node
                if node.is_named() {
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        if child.is_named() {
                            self.walk_expr(child, scope);
                        }
                    }
                }
            }
        }
    }

    fn walk_ternary(&mut self, node: Node<'_>, scope: ScopeId) {
        // ternary: condition ? consequent : alternate
        // condition — no frame; consequence and alternate — conditional
        let mut cursor = node.walk();
        let mut children: Vec<Node<'_>> = Vec::new();
        for child in node.children(&mut cursor) {
            if child.is_named() {
                children.push(child);
            }
        }
        for (i, child) in children.iter().enumerate() {
            if i == 0 {
                // condition expression
                self.walk_expr(*child, scope);
            } else {
                // consequent or alternate — conditional
                self.push_condition(EdgeCondition::Conditional);
                self.walk_expr(*child, scope);
                self.pop_condition();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Call expression handling
    // -----------------------------------------------------------------------

    fn handle_call_expr(&mut self, node: Node<'_>, scope: ScopeId) {
        let callee = node.child_by_field_name("function");
        let callee = match callee {
            Some(n) => n,
            None => return,
        };

        let args_node = node.child_by_field_name("arguments");
        let arity = args_node.map(|a| self.count_args(a));

        // Detect dynamic import: call_expression where function = "import"
        // tree-sitter may parse `import(...)` as a call with `import` keyword
        let callee_text = self.text(callee);
        if callee_text == "import" {
            self.handle_dynamic_import(node, args_node, scope);
            return;
        }

        // Detect require()
        if callee_text == "require" {
            self.handle_require(node, args_node, scope);
            return;
        }

        // Detect eval() and new Function()
        if callee_text == "eval" {
            self.emit_reflective_cut(node, scope);
            // Walk args
            if let Some(args) = args_node {
                self.walk_args(args, scope);
            }
            return;
        }

        // Determine the call name path and kind
        let (name_path, ref_kind) = self.classify_callee(callee, scope);
        if name_path.is_empty() {
            // Still walk args
            if let Some(args) = args_node {
                self.walk_args(args, scope);
            }
            return;
        }

        // Check for loop-method callbacks (map, forEach, etc.) — inside callback
        // arguments, calls are under loop condition
        let is_loop_method = name_path
            .last()
            .map(|n| query::is_loop_method(n.as_str()))
            .unwrap_or(false);

        let condition = self.current_condition();

        // Emit the primary call ref
        self.facts.refs.push(RawRef {
            name_path: name_path.clone(),
            scope,
            kind: ref_kind,
            edge_condition: condition,
            implicit: None,
            span: self.span(node),
            stmt_index: self.stmt_index,
            arity,
            cut_markers: SmallVec::new(),
        });
        self.stmt_index += 1;

        // Walk arguments — if this is a loop-method, callback args have loop condition
        if let Some(args) = args_node {
            if is_loop_method {
                self.push_condition(EdgeCondition::Loop);
                self.walk_args(args, scope);
                self.pop_condition();
            } else {
                self.walk_args(args, scope);
            }
        }
    }

    fn handle_new_expr(&mut self, node: Node<'_>, scope: ScopeId) {
        let callee = node.child_by_field_name("constructor");
        let callee = match callee {
            Some(n) => n,
            None => return,
        };

        let callee_text = self.text(callee);

        // new Function(...) — reflective
        if callee_text == "Function" {
            self.emit_reflective_cut(node, scope);
            let args = node.child_by_field_name("arguments");
            if let Some(args) = args {
                self.walk_args(args, scope);
            }
            return;
        }

        // Normal new expression — emit as Instantiate ref
        let name: SmallVec<[String; 2]> = SmallVec::from_vec(vec![callee_text.to_string()]);
        let args_node = node.child_by_field_name("arguments");
        let arity = args_node.map(|a| self.count_args(a));
        let condition = self.current_condition();

        self.facts.refs.push(RawRef {
            name_path: name,
            scope,
            kind: RefKind::Instantiate,
            edge_condition: condition,
            implicit: None,
            span: self.span(node),
            stmt_index: self.stmt_index,
            arity,
            cut_markers: SmallVec::new(),
        });
        self.stmt_index += 1;

        if let Some(args) = args_node {
            self.walk_args(args, scope);
        }
    }

    fn handle_await_expr(&mut self, node: Node<'_>, scope: ScopeId) {
        // await expr — emit a CallAsync ref for whatever is being awaited
        let inner = {
            let mut cursor = node.walk();
            let mut found = None;
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    found = Some(child);
                    break;
                }
            }
            found
        };

        if let Some(inner_node) = inner {
            // If the inner expression is a call expression, emit it as CallAsync
            if inner_node.kind() == "call_expression" {
                let callee = inner_node.child_by_field_name("function");
                let args_node = inner_node.child_by_field_name("arguments");
                let arity = args_node.map(|a| self.count_args(a));

                if let Some(callee_node) = callee {
                    let callee_text = self.text(callee_node);

                    // Dynamic import inside await
                    if callee_text == "import" {
                        self.handle_dynamic_import_async(inner_node, args_node, scope);
                        return;
                    }

                    let (name_path, _) = self.classify_callee(callee_node, scope);
                    if !name_path.is_empty() {
                        let condition = self.current_condition();
                        self.facts.refs.push(RawRef {
                            name_path,
                            scope,
                            kind: RefKind::CallAsync,
                            edge_condition: condition,
                            implicit: None,
                            span: self.span(node),
                            stmt_index: self.stmt_index,
                            arity,
                            cut_markers: SmallVec::new(),
                        });
                        self.stmt_index += 1;

                        if let Some(args) = args_node {
                            self.walk_args(args, scope);
                        }
                        return;
                    }
                }
            }
            // For other await expressions (identifiers, member expressions), walk
            self.walk_expr(inner_node, scope);
        }
    }

    /// Classify a callee node into a name path and ref kind.
    fn classify_callee(
        &self,
        callee: Node<'_>,
        _scope: ScopeId,
    ) -> (SmallVec<[String; 2]>, RefKind) {
        match callee.kind() {
            "identifier" => {
                let name = self.text(callee).to_string();
                if name.is_empty() {
                    return (SmallVec::new(), RefKind::Call);
                }
                // Check if it's a known lambda binding
                let kind = if self.lambda_names.contains(&name) {
                    RefKind::CallClosure
                } else if self.callback_names.contains(&name) {
                    RefKind::CallCallback
                } else {
                    RefKind::Call
                };
                (SmallVec::from_vec(vec![name]), kind)
            }
            "member_expression" => {
                // x.method or x?.method
                let obj = callee.child_by_field_name("object");
                let prop = callee.child_by_field_name("property");
                let mut path: SmallVec<[String; 2]> = SmallVec::new();
                if let Some(obj) = obj {
                    let obj_text = self.text(obj);
                    // Don't expand deeply nested member expressions for now
                    if !obj_text.is_empty() {
                        path.push(obj_text.to_string());
                    }
                }
                if let Some(prop) = prop {
                    let prop_text = self.text(prop);
                    if !prop_text.is_empty() {
                        path.push(prop_text.to_string());
                    }
                }
                // A method call on a receiver → CallVirtualReceiver (resolver decides
                // calls vs calls:virtual based on whether the receiver type is known)
                (path, RefKind::CallVirtualReceiver)
            }
            "optional_chain" => {
                // x?.method() — also virtual receiver
                let text = self.text(callee);
                let parts: SmallVec<[String; 2]> = text
                    .split(['.', '?'])
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                (parts, RefKind::CallVirtualReceiver)
            }
            "parenthesized_expression" => {
                // (expr)(...) — try to descend
                let inner = {
                    let mut cursor = callee.walk();
                    let mut found = None;
                    for child in callee.children(&mut cursor) {
                        if child.is_named() {
                            found = Some(child);
                            break;
                        }
                    }
                    found
                };
                if let Some(inner) = inner {
                    self.classify_callee(inner, _scope)
                } else {
                    (SmallVec::new(), RefKind::Call)
                }
            }
            _ => {
                // Unknown callee shape
                (SmallVec::new(), RefKind::Call)
            }
        }
    }

    fn count_args(&self, args_node: Node<'_>) -> u8 {
        let mut cursor = args_node.walk();
        let mut n: u32 = 0;
        for child in args_node.children(&mut cursor) {
            let k = child.kind();
            let is_punct = matches!(k, "(" | ")" | ",") || !child.is_named();
            if !is_punct {
                n += 1;
            }
        }
        n.min(u8::MAX as u32) as u8
    }

    fn walk_args(&mut self, args_node: Node<'_>, scope: ScopeId) {
        let mut cursor = args_node.walk();
        for child in args_node.children(&mut cursor) {
            if child.is_named() {
                self.walk_expr(child, scope);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Dynamic import and require handling
    // -----------------------------------------------------------------------

    fn handle_dynamic_import(
        &mut self,
        node: Node<'_>,
        args_node: Option<Node<'_>>,
        scope: ScopeId,
    ) {
        // import(specifier) — static specifier → probable, dynamic → cut:dynamic
        let specifier_and_is_literal = args_node.and_then(|args| {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if child.is_named() {
                    let is_lit = child.kind() == "string";
                    let val = if is_lit {
                        query::extract_string_value(self.src, child)
                    } else {
                        None
                    };
                    return Some((val, is_lit));
                }
            }
            None
        });

        let condition = self.current_condition();

        match specifier_and_is_literal {
            Some((Some(specifier), true)) => {
                // String literal — probable resolution (still has dynamic cut marker per golden)
                let name_path: SmallVec<[String; 2]> = SmallVec::from_vec(vec![specifier]);
                let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
                cut_markers.push(CutMarker::Dynamic);
                self.facts.refs.push(RawRef {
                    name_path,
                    scope,
                    kind: RefKind::Call,
                    edge_condition: condition,
                    implicit: None,
                    span: self.span(node),
                    stmt_index: self.stmt_index,
                    arity: Some(1),
                    cut_markers,
                });
                self.stmt_index += 1;

                // Also record as cut hint
                self.facts.cut_hints.push(CutHint {
                    marker: CutMarker::Dynamic,
                    span: self.span(node),
                    macro_origin: None,
                });
            }
            _ => {
                // Dynamic or no argument — dynamic cut marker
                self.emit_dynamic_import_cut(node, condition, scope);
            }
        }
    }

    fn handle_dynamic_import_async(
        &mut self,
        node: Node<'_>,
        args_node: Option<Node<'_>>,
        scope: ScopeId,
    ) {
        // await import(specifier) — same as above but CallAsync
        let specifier_and_is_literal = args_node.and_then(|args| {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if child.is_named() {
                    let is_lit = child.kind() == "string";
                    let val = if is_lit {
                        query::extract_string_value(self.src, child)
                    } else {
                        None
                    };
                    return Some((val, is_lit));
                }
            }
            None
        });

        let condition = self.current_condition();

        match specifier_and_is_literal {
            Some((Some(specifier), true)) => {
                let name_path: SmallVec<[String; 2]> = SmallVec::from_vec(vec![specifier]);
                let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
                cut_markers.push(CutMarker::Dynamic);
                self.facts.refs.push(RawRef {
                    name_path,
                    scope,
                    kind: RefKind::CallAsync,
                    edge_condition: condition,
                    implicit: None,
                    span: self.span(node),
                    stmt_index: self.stmt_index,
                    arity: Some(1),
                    cut_markers,
                });
                self.stmt_index += 1;

                self.facts.cut_hints.push(CutHint {
                    marker: CutMarker::Dynamic,
                    span: self.span(node),
                    macro_origin: None,
                });
            }
            _ => {
                self.emit_dynamic_import_cut_async(node, condition, scope);
            }
        }
    }

    fn emit_dynamic_import_cut(
        &mut self,
        node: Node<'_>,
        condition: EdgeCondition,
        scope: ScopeId,
    ) {
        let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
        cut_markers.push(CutMarker::Dynamic);
        let name_path: SmallVec<[String; 2]> = SmallVec::from_vec(vec!["<unresolved>".to_string()]);
        self.facts.refs.push(RawRef {
            name_path,
            scope,
            kind: RefKind::Call,
            edge_condition: condition,
            implicit: None,
            span: self.span(node),
            stmt_index: self.stmt_index,
            arity: None,
            cut_markers,
        });
        self.stmt_index += 1;

        self.facts.cut_hints.push(CutHint {
            marker: CutMarker::Dynamic,
            span: self.span(node),
            macro_origin: None,
        });
    }

    fn emit_dynamic_import_cut_async(
        &mut self,
        node: Node<'_>,
        condition: EdgeCondition,
        scope: ScopeId,
    ) {
        let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
        cut_markers.push(CutMarker::Dynamic);
        let name_path: SmallVec<[String; 2]> = SmallVec::from_vec(vec!["<unresolved>".to_string()]);
        self.facts.refs.push(RawRef {
            name_path,
            scope,
            kind: RefKind::CallAsync,
            edge_condition: condition,
            implicit: None,
            span: self.span(node),
            stmt_index: self.stmt_index,
            arity: None,
            cut_markers,
        });
        self.stmt_index += 1;

        self.facts.cut_hints.push(CutHint {
            marker: CutMarker::Dynamic,
            span: self.span(node),
            macro_origin: None,
        });
    }

    fn handle_require(&mut self, node: Node<'_>, args_node: Option<Node<'_>>, scope: ScopeId) {
        let specifier_and_is_literal = args_node.and_then(|args| {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if child.is_named() {
                    let is_lit = child.kind() == "string";
                    let val = if is_lit {
                        query::extract_string_value(self.src, child)
                    } else {
                        None
                    };
                    return Some((val, is_lit));
                }
            }
            None
        });

        let condition = self.current_condition();
        let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
        cut_markers.push(CutMarker::Dynamic);

        match specifier_and_is_literal {
            Some((Some(specifier), true)) => {
                // Static require("./path") — probable but still tagged dynamic per golden
                let name_path: SmallVec<[String; 2]> = SmallVec::from_vec(vec![specifier]);
                self.facts.refs.push(RawRef {
                    name_path,
                    scope,
                    kind: RefKind::Call,
                    edge_condition: condition,
                    implicit: None,
                    span: self.span(node),
                    stmt_index: self.stmt_index,
                    arity: Some(1),
                    cut_markers,
                });
                self.stmt_index += 1;
                self.facts.cut_hints.push(CutHint {
                    marker: CutMarker::Dynamic,
                    span: self.span(node),
                    macro_origin: None,
                });
            }
            _ => {
                // Dynamic require(var) — unresolved + dynamic
                let name_path: SmallVec<[String; 2]> =
                    SmallVec::from_vec(vec!["<unresolved>".to_string()]);
                self.facts.refs.push(RawRef {
                    name_path,
                    scope,
                    kind: RefKind::Call,
                    edge_condition: condition,
                    implicit: None,
                    span: self.span(node),
                    stmt_index: self.stmt_index,
                    arity: None,
                    cut_markers,
                });
                self.stmt_index += 1;
                self.facts.cut_hints.push(CutHint {
                    marker: CutMarker::Dynamic,
                    span: self.span(node),
                    macro_origin: None,
                });
            }
        }
    }

    fn emit_reflective_cut(&mut self, node: Node<'_>, _scope: ScopeId) {
        let mut cut_markers: SmallVec<[CutMarker; 1]> = SmallVec::new();
        cut_markers.push(CutMarker::Reflective);
        let name_path: SmallVec<[String; 2]> = SmallVec::from_vec(vec!["<reflective>".to_string()]);
        let condition = self.current_condition();
        self.facts.refs.push(RawRef {
            name_path,
            scope: _scope,
            kind: RefKind::Call,
            edge_condition: condition,
            implicit: None,
            span: self.span(node),
            stmt_index: self.stmt_index,
            arity: None,
            cut_markers,
        });
        self.stmt_index += 1;

        self.facts.cut_hints.push(CutHint {
            marker: CutMarker::Reflective,
            span: self.span(node),
            macro_origin: None,
        });
    }
}
