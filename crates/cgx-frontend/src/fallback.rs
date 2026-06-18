//! The Tier-0 generic fallback frontend (architecture §5, WP-03).
//!
//! This is the degraded baseline for any language `cgx` has no dedicated
//! adapter for. Given *any* tree-sitter grammar, it produces well-formed
//! [`FileFacts`] with **no grammar-specific rules**: it recognizes
//! definition-like and call-like nodes by the naming conventions tree-sitter
//! grammars share (`*_declaration`, `*_definition`, `call_expression`, …),
//! extracts a symbol inventory and intra-file name/arity call references, and
//! labels every reference [`Tier::NameSyntactic`] / [`Confidence::Possible`]
//! (LS-1 Tier 3, LS-6: "all edges labeled `possible`; no cross-file edges").
//!
//! It never errors on unknown syntax: a file it cannot parse, or one with no
//! recognizable constructs, yields [`FileFacts::empty`] (architecture §5
//! "well-formed empty/degraded facts"). Confidence honesty (LS-6: no label is
//! ever omitted) is preserved — nothing is silently dropped.
//!
//! When a language graduates to a real adapter (WP-04/05), that adapter
//! registers ahead of the fallback and claims its extensions; the fallback then
//! only catches the long tail.

use crate::facts::{FileFacts, RawRef, RefKind, Scope, ScopeId, ScopeTree, SymbolDef};
use crate::frontend::{FileCtx, FrontendError, Lang, LanguageFrontend, RelPath};
use cgx_core::condition::EdgeCondition;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::provenance::Span;
use smallvec::SmallVec;
use tree_sitter::{Language, Node, Parser};

/// The Tier-0 generic fallback frontend over one tree-sitter grammar.
///
/// Construct one per grammar (`FallbackFrontend::new(lang, "python", ["py"])`).
/// The same *code* handles any grammar; the instance just binds a grammar, a
/// language tag, and the extensions it claims.
pub struct FallbackFrontend {
    language: Language,
    lang_name: String,
    extensions: Vec<String>,
}

impl std::fmt::Debug for FallbackFrontend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackFrontend")
            .field("lang_name", &self.lang_name)
            .field("extensions", &self.extensions)
            .finish_non_exhaustive()
    }
}

impl FallbackFrontend {
    /// Build a fallback frontend for `language`, tagging produced facts with
    /// `lang_name` and claiming files with any of `extensions` (lowercase, no
    /// dot).
    pub fn new(
        language: Language,
        lang_name: impl Into<String>,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut extensions: Vec<String> = extensions
            .into_iter()
            .map(|e| e.into().to_ascii_lowercase())
            .collect();
        extensions.sort();
        extensions.dedup();
        FallbackFrontend {
            language,
            lang_name: lang_name.into(),
            extensions,
        }
    }
}

/// The fragment version of the Tier-0 extraction rules. Bump when the heuristics
/// below change in a way that alters emitted facts.
const FALLBACK_FRAGMENT_VERSION: u32 = 1;

impl LanguageFrontend for FallbackFrontend {
    fn lang(&self) -> Lang {
        Lang::Fallback(self.lang_name.clone())
    }

    fn handles(&self, path: &RelPath) -> bool {
        path.extension()
            .map(|ext| self.extensions.contains(&ext))
            .unwrap_or(false)
    }

    fn fragment_version(&self) -> u32 {
        FALLBACK_FRAGMENT_VERSION
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut parser = Parser::new();
        parser
            .set_language(&self.language)
            .map_err(|e| FrontendError::Parser {
                lang: self.lang_name.clone(),
                detail: e.to_string(),
            })?;

        let tree = match parser.parse(src, None) {
            Some(tree) => tree,
            // A grammar that refuses the input degrades to empty facts rather
            // than erroring — the Tier-0 honesty posture.
            None => return Ok(FileFacts::empty()),
        };

        let mut builder = Builder::new(src, ctx.path.as_str());
        builder.walk(tree.root_node(), ScopeId::ROOT, &mut 0);
        Ok(builder.finish())
    }
}

/// Accumulates facts while walking the parse tree. Owns the scope tree and the
/// growing def/ref vectors; `canonicalize` happens later in the registry.
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
            facts: FileFacts {
                scopes: ScopeTree::new(),
                ..FileFacts::default()
            },
        }
    }

    fn finish(self) -> FileFacts {
        self.facts
    }

    /// 1-based start line of a node.
    fn line(&self, node: Node<'_>) -> u32 {
        node.start_position().row as u32 + 1
    }

    /// 1-based end line of a node.
    fn end_line(&self, node: Node<'_>) -> u32 {
        node.end_position().row as u32 + 1
    }

    fn col(&self, node: Node<'_>) -> u32 {
        node.start_position().column as u32 + 1
    }

    fn span(&self, node: Node<'_>) -> Span {
        Span::new(self.file.clone(), self.line(node), Some(self.col(node)))
    }

    /// The source text of a node, lossily decoded (the fallback never errors on
    /// odd encodings — it just records what it can).
    fn text(&self, node: Node<'_>) -> String {
        node.utf8_text(self.src)
            .map(|s| s.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(&self.src[node.byte_range()]).into_owned())
    }

    /// Recursively walk `node`. `scope` is the lexical scope `node` lives in;
    /// `stmt_index` is a shared, monotonically increasing per-file ordinal used
    /// for the ADR-02 statement-index reservation (deterministic, source order).
    fn walk(&mut self, node: Node<'_>, scope: ScopeId, stmt_index: &mut u32) {
        if let Some(kind) = classify_def(node.kind()) {
            let (def_scope, name) = self.record_def(node, scope, kind);
            // Children of a definition live in the scope it opened.
            let child_scope = def_scope.unwrap_or(scope);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                self.walk(child, child_scope, stmt_index);
            }
            let _ = name;
            return;
        }

        if is_call(node.kind()) {
            if let Some(r) = self.record_call(node, scope, *stmt_index) {
                self.facts.refs.push(r);
                *stmt_index += 1;
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, scope, stmt_index);
        }
    }

    /// Record a definition node. Returns the scope it opened (for callable/type
    /// bodies) and the def's local name.
    fn record_def(
        &mut self,
        node: Node<'_>,
        scope: ScopeId,
        kind: SymbolKind,
    ) -> (Option<ScopeId>, Option<String>) {
        let name = self.def_name(node);
        let Some(name) = name else {
            return (None, None);
        };

        let parent_fqn = self.scope_owner_fqn(scope);
        let fqn = match &parent_fqn {
            Some(p) => format!("{p}::{name}"),
            None => name.clone(),
        };

        let opens_scope = matches!(
            kind,
            SymbolKind::Function
                | SymbolKind::Method
                | SymbolKind::Type
                | SymbolKind::Module
                | SymbolKind::Lambda
        );
        let new_scope = if opens_scope {
            Some(self.facts.scopes.push(scope, Some(fqn.clone())))
        } else {
            None
        };

        self.facts.defs.push(SymbolDef {
            fqn,
            kind,
            // The fallback cannot read language-specific visibility modifiers;
            // it reports `internal` (neither asserted-public nor asserted-private).
            visibility: Visibility::Internal,
            scope,
            span: self.span(node),
            line_end: self.end_line(node),
            is_abstract: false,
            // Signatures require per-language parameter parsing; Tier-0 leaves
            // them null (ADR-04: never inferred).
            signature: None,
        });

        (new_scope, Some(name))
    }

    /// Record a call node as a name+arity [`RawRef`], if a callee name can be
    /// recovered. All Tier-0 refs are `RefKind::Call` with `always` condition;
    /// the resolver assigns confidence `possible` from the Tier-0 tag.
    fn record_call(&self, node: Node<'_>, scope: ScopeId, stmt_index: u32) -> Option<RawRef> {
        let (name_path, arity) = self.call_target(node)?;
        if name_path.is_empty() {
            return None;
        }
        Some(RawRef {
            name_path,
            scope,
            kind: RefKind::Call,
            edge_condition: EdgeCondition::Always,
            implicit: None,
            span: self.span(node),
            stmt_index,
            arity,
            cut_markers: SmallVec::new(),
        })
    }

    /// The owner FQN of `scope`, walking up to the nearest scope that has one.
    fn scope_owner_fqn(&self, scope: ScopeId) -> Option<String> {
        self.facts
            .scopes
            .ancestors(scope)
            .find_map(|id| self.scope(id).and_then(|s| s.owner_fqn.clone()))
    }

    fn scope(&self, id: ScopeId) -> Option<&Scope> {
        self.facts.scopes.scopes.get(id.index())
    }

    /// Recover a definition's name. Tries the conventional `name` field first
    /// (most grammars expose it), then the first identifier-like child.
    fn def_name(&self, node: Node<'_>) -> Option<String> {
        if let Some(name_node) = node.child_by_field_name("name") {
            return Some(self.text(name_node));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if is_identifier(child.kind()) {
                return Some(self.text(child));
            }
        }
        None
    }

    /// Recover the callee name path and arity from a call node.
    ///
    /// Grammars conventionally expose the callee under a `function` field and
    /// the arguments under an `arguments` field. The callee text is split on the
    /// common member-access separators (`.`, `::`, `->`) into a name path.
    fn call_target(&self, node: Node<'_>) -> Option<(SmallVec<[String; 2]>, Option<u8>)> {
        let callee = node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("callee"))
            .or_else(|| first_identifier_child(node));
        let callee = callee?;
        let text = self.text(callee);
        let name_path = split_name_path(&text);
        if name_path.is_empty() {
            return None;
        }
        let arity = node
            .child_by_field_name("arguments")
            .map(|args| count_arguments(args));
        Some((name_path, arity))
    }
}

/// Classify a node kind as a definition, by the substrings tree-sitter grammars
/// conventionally use. Order matters: more specific kinds first.
fn classify_def(kind: &str) -> Option<SymbolKind> {
    // Methods before functions (a method kind often contains "method").
    if kind.contains("method") {
        return Some(SymbolKind::Method);
    }
    if kind.contains("constructor") {
        return Some(SymbolKind::Method);
    }
    // Functions: `function_definition`, `function_declaration`, `function_item`.
    if kind.contains("function") {
        return Some(SymbolKind::Function);
    }
    // Types: classes, structs, interfaces, traits, enums.
    if kind.contains("class")
        || kind.contains("struct")
        || kind.contains("interface")
        || kind.contains("trait")
        || kind.contains("enum_") // `enum_declaration`/`enum_item`, not `enumerator`
        || kind == "enum"
    {
        return Some(SymbolKind::Type);
    }
    if kind.contains("module") || kind.contains("namespace") {
        return Some(SymbolKind::Module);
    }
    None
}

/// Whether a node kind is a call expression, by convention.
fn is_call(kind: &str) -> bool {
    kind == "call"
        || kind == "call_expression"
        || kind == "function_call"
        || kind == "method_invocation"
        || kind == "call_expr"
        || kind.ends_with("_call")
}

/// Whether a node kind is an identifier the fallback may read as a name.
fn is_identifier(kind: &str) -> bool {
    kind == "identifier"
        || kind == "type_identifier"
        || kind == "field_identifier"
        || kind == "property_identifier"
        || kind == "name"
        || kind.ends_with("_identifier")
}

/// The first identifier-like direct child of a node, if any.
fn first_identifier_child<'a>(node: Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'a>> = node.children(&mut cursor).collect();
    children.into_iter().find(|c| is_identifier(c.kind()))
}

/// Split a callee expression text into a name path on member-access separators.
/// `a.b.c` → `["a","b","c"]`; `a::b` → `["a","b"]`; `a->b` → `["a","b"]`.
/// Whitespace and call-syntax noise are stripped; empty segments are dropped.
fn split_name_path(text: &str) -> SmallVec<[String; 2]> {
    text.split(['.', ':', '>', '-', '(', ' ', '\t', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().all(is_name_char))
        .map(str::to_string)
        .collect()
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Count argument expressions in an arguments node, ignoring punctuation
/// (`(`, `,`, `)`) and comments. Saturates at `u8::MAX`.
fn count_arguments(args: Node<'_>) -> u8 {
    let mut cursor = args.walk();
    let mut n: u32 = 0;
    for child in args.children(&mut cursor) {
        let k = child.kind();
        let is_punct =
            matches!(k, "(" | ")" | "," | "[" | "]") || k.contains("comment") || !child.is_named();
        if !is_punct {
            n += 1;
        }
    }
    n.min(u8::MAX as u32) as u8
}
