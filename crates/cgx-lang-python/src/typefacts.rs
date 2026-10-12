//! Receiver-typing facts ([`TypeFact`]) for one Python callable.
//!
//! [`collect`] walks a single `function_definition` and upholds the channel's
//! binding-completeness contract: one `Param` per declared parameter and one
//! `Bind` per binding site of every local name, with every form it does not
//! type emitted as [`ValueSource::Opaque`]. Nested functions and classes are
//! not descended into: a nested def's facts are collected under its own FQN,
//! and the nested name itself is an opaque binding here, as is every name a
//! nested scope rebinds through `nonlocal`. Lambda bodies are attributed to the
//! enclosing callable, so lambda parameters are opaque bindings of it.
//!
//! Field stores (`FieldBind`) are writes through `self`, `cls`, or the first
//! positional parameter of a callable defined directly in a class body,
//! whatever its name.
//!
//! Annotations are normalized into [`TypeExpr`] from the syntax tree, never
//! from text: `Optional[X]` and `X | None` drop the `None` member, string
//! annotations are re-parsed, and `Any` becomes `Unknown`.

use cgx_frontend::{
    AnonRoot, BaseExpr, LiteralKind, Name, ParamKind, TypeExpr, TypeFact, ValueSource,
};
use smallvec::SmallVec;
use tree_sitter::{Node, Parser};

type Path = SmallVec<[Name; 2]>;

/// Every `Param`/`Bind`/`FieldBind` fact of the callable `decl` (a
/// `function_definition`) whose FQN is `fqn`. `in_class` is whether `decl`
/// is defined directly in a class body.
pub(crate) fn collect(src: &[u8], decl: Node<'_>, fqn: &str, in_class: bool) -> Vec<TypeFact> {
    let mut c = Collector {
        src,
        fqn,
        receivers: vec!["self".to_owned(), "cls".to_owned()],
        out: Vec::new(),
    };
    if let Some(params) = decl.child_by_field_name("parameters") {
        c.params(params);
    }
    if in_class {
        let first = c.out.iter().find_map(|t| match t {
            TypeFact::Param {
                name,
                index: Some(0),
                kind: ParamKind::Positional,
                ..
            } => Some(name.clone()),
            _ => None,
        });
        c.receivers.extend(first);
    }
    if let Some(body) = decl.child_by_field_name("body") {
        c.walk(body);
    }
    c.out
}

/// The `ClassBases` payload of a `class_definition`: its `superclasses` in
/// source order, keyword arguments (`metaclass=`) excluded. Empty when the class
/// declares no base.
pub(crate) fn class_bases(src: &[u8], class: Node<'_>) -> Vec<BaseExpr> {
    let Some(list) = class.child_by_field_name("superclasses") else {
        return Vec::new();
    };
    let mut cur = list.walk();
    let kids: Vec<Node<'_>> = list.named_children(&mut cur).collect();
    kids.into_iter()
        .filter(|n| !matches!(n.kind(), "keyword_argument" | "comment"))
        .map(|n| match n.kind() {
            "identifier" | "attribute" => match name_path(src, n) {
                Some(path) => BaseExpr::Type(TypeExpr::Named {
                    path,
                    indirect: false,
                }),
                None => BaseExpr::Unknown,
            },
            "subscript" => match n
                .child_by_field_name("value")
                .and_then(|v| name_path(src, v))
            {
                Some(head) => BaseExpr::Type(TypeExpr::Generic {
                    head,
                    args: subscript_args(n)
                        .into_iter()
                        .map(|a| type_expr(src, a))
                        .collect(),
                    indirect: false,
                }),
                None => BaseExpr::Unknown,
            },
            "call" => match n
                .child_by_field_name("function")
                .and_then(|f| name_path(src, f))
            {
                Some(path) => BaseExpr::Call(path),
                None => BaseExpr::Unknown,
            },
            _ => BaseExpr::Unknown,
        })
        .collect()
}

/// Classify the root of a call's `function` operand when it is an attribute
/// chain rooted at something other than a name: `(root, depth)` for
/// [`TypeFact::AnonReceiver`], where `depth` counts the attribute segments.
/// `None` for a non-attribute callee or a chain rooted at an identifier (the
/// ordinary named-receiver case).
pub(crate) fn anon_root(src: &[u8], func: Node<'_>) -> Option<(AnonRoot, u8)> {
    if func.kind() != "attribute" {
        return None;
    }
    let mut depth: u8 = 0;
    let mut cur = func;
    while cur.kind() == "attribute" {
        depth = depth.saturating_add(1);
        cur = cur.child_by_field_name("object")?;
    }
    classify_root(src, cur).map(|root| (root, depth))
}

fn classify_root(src: &[u8], n: Node<'_>) -> Option<AnonRoot> {
    if let Some(kind) = literal_kind(src, n) {
        return Some(AnonRoot::Literal(kind));
    }
    Some(match n.kind() {
        "identifier" => return None,
        "call" => match n.child_by_field_name("function") {
            Some(f) if f.kind() == "identifier" && text(src, f) == "super" => AnonRoot::Super,
            Some(f) => match name_path(src, f) {
                Some(path) => AnonRoot::Call(path),
                None => AnonRoot::Other,
            },
            None => AnonRoot::Other,
        },
        "subscript" => AnonRoot::Subscript,
        "parenthesized_expression" => match n.named_child(0) {
            Some(inner) => return classify_root(src, inner),
            None => AnonRoot::Other,
        },
        _ => AnonRoot::Other,
    })
}

struct Collector<'a> {
    src: &'a [u8],
    fqn: &'a str,
    /// Names whose attribute stores are field binds.
    receivers: Vec<String>,
    out: Vec<TypeFact>,
}

impl Collector<'_> {
    fn text(&self, n: Node<'_>) -> String {
        text(self.src, n)
    }

    fn params(&mut self, params: Node<'_>) {
        let mut index: u8 = 0;
        let mut keyword_only = false;
        let mut cur = params.walk();
        let kids: Vec<Node<'_>> = params.named_children(&mut cur).collect();
        for p in kids {
            let (name, ty, kind) = match p.kind() {
                "identifier" => (Some(p), None, None),
                "default_parameter" => (p.child_by_field_name("name"), None, None),
                "typed_default_parameter" => (
                    p.child_by_field_name("name"),
                    p.child_by_field_name("type"),
                    None,
                ),
                "typed_parameter" => {
                    let head = p.named_child(0);
                    let ty = p.child_by_field_name("type");
                    match head.map(|h| h.kind()) {
                        Some("identifier") => (head, ty, None),
                        Some("list_splat_pattern") => (
                            head.and_then(|h| h.named_child(0)),
                            ty,
                            Some(ParamKind::VarArgs),
                        ),
                        Some("dictionary_splat_pattern") => (
                            head.and_then(|h| h.named_child(0)),
                            ty,
                            Some(ParamKind::VarKeywords),
                        ),
                        _ => continue,
                    }
                }
                "list_splat_pattern" => (p.named_child(0), None, Some(ParamKind::VarArgs)),
                "dictionary_splat_pattern" => {
                    (p.named_child(0), None, Some(ParamKind::VarKeywords))
                }
                "keyword_separator" => {
                    keyword_only = true;
                    continue;
                }
                _ => continue,
            };
            let Some(name) = name else { continue };
            let kind = kind.unwrap_or(if keyword_only {
                ParamKind::KeywordOnly
            } else {
                ParamKind::Positional
            });
            if kind == ParamKind::VarArgs {
                keyword_only = true;
            }
            self.out.push(TypeFact::Param {
                func: self.fqn.to_owned(),
                name: self.text(name),
                index: Some(index),
                kind,
                ty: ty.map(|t| type_expr(self.src, t)),
            });
            index = index.saturating_add(1);
        }
    }

    fn bind(&mut self, var: String, src: ValueSource) {
        self.out.push(TypeFact::Bind {
            func: self.fqn.to_owned(),
            var,
            src,
        });
    }

    fn field_bind(&mut self, field: String, src: ValueSource) {
        self.out.push(TypeFact::FieldBind {
            func: self.fqn.to_owned(),
            field,
            src,
        });
    }

    /// The field name of a `<receiver>.<attr>` target; `None` for any other
    /// target.
    fn self_field(&self, target: Node<'_>) -> Option<String> {
        if target.kind() != "attribute" {
            return None;
        }
        let obj = target.child_by_field_name("object")?;
        let attr = target.child_by_field_name("attribute")?;
        (obj.kind() == "identifier" && self.receivers.contains(&self.text(obj)))
            .then(|| self.text(attr))
    }

    /// Every name a parameter list (`parameters` / `lambda_parameters`) binds
    /// becomes an opaque binding of this callable.
    fn opaque_params(&mut self, params: Node<'_>) {
        let mut cur = params.walk();
        let kids: Vec<Node<'_>> = params.named_children(&mut cur).collect();
        for p in kids {
            let name = match p.kind() {
                "identifier" => Some(p),
                "default_parameter" | "typed_default_parameter" => p.child_by_field_name("name"),
                "typed_parameter" => p.named_child(0).and_then(|h| match h.kind() {
                    "identifier" => Some(h),
                    _ => h.named_child(0),
                }),
                "list_splat_pattern" | "dictionary_splat_pattern" => p.named_child(0),
                _ => None,
            };
            if let Some(n) = name.filter(|n| n.kind() == "identifier") {
                let t = self.text(n);
                self.bind(t, ValueSource::Opaque);
            }
        }
    }

    /// `nonlocal` names anywhere under a nested def/class may rebind a local of
    /// this callable; each is an opaque binding here.
    fn nested_nonlocals(&mut self, node: Node<'_>) {
        if node.kind() == "nonlocal_statement" {
            let mut cur = node.walk();
            let ids: Vec<Node<'_>> = node
                .named_children(&mut cur)
                .filter(|n| n.kind() == "identifier")
                .collect();
            for id in ids {
                let t = self.text(id);
                self.bind(t, ValueSource::Opaque);
            }
            return;
        }
        let mut cur = node.walk();
        let kids: Vec<Node<'_>> = node.named_children(&mut cur).collect();
        for k in kids {
            self.nested_nonlocals(k);
        }
    }

    /// An untyped binding target (tuple/star target, augmented assignment,
    /// non-identifier alias): every name it binds gets `Opaque`, and a
    /// `self.f` inside it an opaque `FieldBind`. Subscript and non-`self`
    /// attribute targets bind no local name.
    fn opaque_targets(&mut self, node: Node<'_>) {
        match node.kind() {
            "identifier" => {
                let t = self.text(node);
                self.bind(t, ValueSource::Opaque);
            }
            "attribute" => {
                if let Some(f) = self.self_field(node) {
                    self.field_bind(f, ValueSource::Opaque);
                }
            }
            "subscript" => {}
            _ => {
                let mut cur = node.walk();
                let kids: Vec<Node<'_>> = node.named_children(&mut cur).collect();
                for k in kids {
                    self.opaque_targets(k);
                }
            }
        }
    }

    /// Names a `del` statement unbinds (identifiers only; `del x.f` / `del
    /// x[i]` unbind no local).
    fn opaque_deleted(&mut self, node: Node<'_>) {
        match node.kind() {
            "identifier" => {
                let t = self.text(node);
                self.bind(t, ValueSource::Opaque);
            }
            "expression_list" | "tuple" | "list" | "parenthesized_expression" => {
                let mut cur = node.walk();
                let kids: Vec<Node<'_>> = node.named_children(&mut cur).collect();
                for k in kids {
                    self.opaque_deleted(k);
                }
            }
            _ => {}
        }
    }

    /// Capture names of a `match` `case_pattern`. A single-name `dotted_name` is
    /// a capture; a dotted value pattern (`Color.RED`), a class pattern's class
    /// name and a keyword pattern's keyword are not.
    fn pattern_captures(&mut self, node: Node<'_>) {
        let mut cur = node.walk();
        let kids: Vec<(Node<'_>, Option<&str>)> = node
            .children(&mut cur)
            .enumerate()
            .filter(|(_, k)| k.is_named())
            .map(|(i, k)| (k, node.field_name_for_child(i as u32)))
            .collect();
        match node.kind() {
            "dotted_name" => {
                if node.named_child_count() == 1 {
                    if let Some(id) = node.named_child(0) {
                        self.capture(id);
                    }
                }
            }
            "as_pattern" | "splat_pattern" => {
                for (k, _) in kids {
                    if k.kind() == "identifier" {
                        self.capture(k);
                    } else {
                        self.pattern_captures(k);
                    }
                }
            }
            "class_pattern" => {
                for (k, _) in kids.into_iter().skip(1) {
                    self.pattern_captures(k);
                }
            }
            "keyword_pattern" => {
                for (k, _) in kids {
                    if k.kind() != "identifier" {
                        self.pattern_captures(k);
                    }
                }
            }
            _ => {
                for (k, field) in kids {
                    if field != Some("key") {
                        self.pattern_captures(k);
                    }
                }
            }
        }
    }

    fn capture(&mut self, id: Node<'_>) {
        let t = self.text(id);
        if t != "_" {
            self.bind(t, ValueSource::Opaque);
        }
    }

    fn walk(&mut self, node: Node<'_>) {
        match node.kind() {
            "function_definition" | "class_definition" => {
                if let Some(n) = node.child_by_field_name("name") {
                    let t = self.text(n);
                    self.bind(t, ValueSource::Opaque);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.nested_nonlocals(body);
                }
                // Defaults and bases are evaluated in this scope.
                for field in ["parameters", "superclasses"] {
                    if let Some(n) = node.child_by_field_name(field) {
                        self.walk(n);
                    }
                }
                return;
            }
            "lambda" => {
                if let Some(params) = node.child_by_field_name("parameters") {
                    self.opaque_params(params);
                }
            }
            "assignment" => self.assignment(node),
            "augmented_assignment" => {
                if let Some(l) = node.child_by_field_name("left") {
                    self.opaque_targets(l);
                }
            }
            "named_expression" => {
                if let (Some(n), Some(v)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("value"),
                ) {
                    let name = self.text(n);
                    let src = self.source(v);
                    self.bind(name, src);
                }
            }
            "for_statement" | "for_in_clause" => {
                if let Some(l) = node.child_by_field_name("left") {
                    if l.kind() == "identifier" {
                        let elem = node
                            .child_by_field_name("right")
                            .map(|r| self.source(r))
                            .unwrap_or(ValueSource::Opaque);
                        let name = self.text(l);
                        self.bind(name, ValueSource::Element(Box::new(elem)));
                    } else {
                        self.opaque_targets(l);
                    }
                }
            }
            "with_item" => {
                if let Some(v) = node.child_by_field_name("value") {
                    if v.kind() == "as_pattern" {
                        self.with_alias(v);
                    }
                }
            }
            "except_clause" | "except_group_clause" => {
                if let Some(v) = node.child_by_field_name("value") {
                    if v.kind() == "as_pattern" {
                        self.except_alias(v);
                    }
                }
            }
            "case_pattern" => {
                self.pattern_captures(node);
                return;
            }
            "global_statement" | "nonlocal_statement" => {
                let mut cur = node.walk();
                let ids: Vec<Node<'_>> = node
                    .named_children(&mut cur)
                    .filter(|n| n.kind() == "identifier")
                    .collect();
                for id in ids {
                    let t = self.text(id);
                    self.bind(t, ValueSource::Opaque);
                }
                return;
            }
            "import_statement" | "import_from_statement" => {
                self.import(node);
                return;
            }
            "delete_statement" => {
                let mut cur = node.walk();
                let kids: Vec<Node<'_>> = node.named_children(&mut cur).collect();
                for k in kids {
                    self.opaque_deleted(k);
                }
                return;
            }
            "type_alias_statement" => {
                if let Some(id) = node
                    .child_by_field_name("left")
                    .and_then(|l| first_identifier(l))
                {
                    let t = self.text(id);
                    self.bind(t, ValueSource::Opaque);
                }
                return;
            }
            _ => {}
        }
        let mut cur = node.walk();
        let kids: Vec<Node<'_>> = node.named_children(&mut cur).collect();
        for k in kids {
            self.walk(k);
        }
    }

    fn assignment(&mut self, node: Node<'_>) {
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        let right = node.child_by_field_name("right");
        if let Some(ty) = node.child_by_field_name("type") {
            let declared = ValueSource::Declared(type_expr(self.src, ty));
            if left.kind() == "identifier" {
                let name = self.text(left);
                self.bind(name, declared);
            } else if let Some(f) = self.self_field(left) {
                self.field_bind(f, declared);
            }
            return;
        }
        let Some(right) = right else { return };
        if left.kind() == "identifier" {
            let name = self.text(left);
            let src = self.source(right);
            self.bind(name, src);
        } else if let Some(f) = self.self_field(left) {
            let src = self.source(right);
            self.field_bind(f, src);
        } else {
            self.opaque_targets(left);
        }
    }

    /// `with <expr> as <alias>`: an identifier alias binds `Enter(src(expr))`.
    fn with_alias(&mut self, as_pattern: Node<'_>) {
        let Some(alias) = as_pattern.child_by_field_name("alias") else {
            return;
        };
        let target = alias.named_child(0).unwrap_or(alias);
        let expr = as_pattern.named_child(0).filter(|e| e.id() != alias.id());
        if target.kind() == "identifier" {
            let src = expr.map(|e| self.source(e)).unwrap_or(ValueSource::Opaque);
            let name = self.text(target);
            self.bind(name, ValueSource::Enter(Box::new(src)));
        } else {
            self.opaque_targets(target);
        }
    }

    /// `except <E> as <e>`: `e` is declared as the caught exception type(s).
    fn except_alias(&mut self, as_pattern: Node<'_>) {
        let Some(alias) = as_pattern.child_by_field_name("alias") else {
            return;
        };
        let target = alias.named_child(0).unwrap_or(alias);
        if target.kind() != "identifier" {
            self.opaque_targets(target);
            return;
        }
        let ty = as_pattern
            .named_child(0)
            .filter(|e| e.id() != alias.id())
            .map(|e| exception_type(self.src, e))
            .unwrap_or(TypeExpr::Unknown);
        let name = self.text(target);
        self.bind(name, ValueSource::Declared(ty));
    }

    /// A function-local `import`: `import a.b` binds `a`, `import a.b as c`
    /// binds `c`, `from m import n [as k]` binds `n` (or `k`).
    fn import(&mut self, node: Node<'_>) {
        let from = node.kind() == "import_from_statement";
        let mut cur = node.walk();
        let names: Vec<Node<'_>> = node.children_by_field_name("name", &mut cur).collect();
        for n in names {
            let bound = match n.kind() {
                "aliased_import" => n.child_by_field_name("alias"),
                "dotted_name" if from => {
                    n.named_child((n.named_child_count() as u32).saturating_sub(1))
                }
                "dotted_name" => n.named_child(0),
                _ => None,
            };
            if let Some(id) = bound {
                let t = self.text(id);
                self.bind(t, ValueSource::Opaque);
            }
        }
    }

    fn source(&self, value: Node<'_>) -> ValueSource {
        if let Some(kind) = literal_kind(self.src, value) {
            return ValueSource::Literal(kind);
        }
        match value.kind() {
            "identifier" => ValueSource::Var(self.text(value)),
            "none" => ValueSource::Null,
            "call" => match value
                .child_by_field_name("function")
                .and_then(|f| name_path(self.src, f))
            {
                Some(path) => ValueSource::Call(path),
                None => ValueSource::Opaque,
            },
            "parenthesized_expression" => value
                .named_child(0)
                .map(|n| self.source(n))
                .unwrap_or(ValueSource::Opaque),
            // Chained `a = b = e`: `a` receives the innermost value.
            "assignment" => value
                .child_by_field_name("right")
                .map(|r| self.source(r))
                .unwrap_or(ValueSource::Opaque),
            _ => ValueSource::Opaque,
        }
    }
}

fn text(src: &[u8], n: Node<'_>) -> String {
    n.utf8_text(src)
        .map(str::to_string)
        .unwrap_or_else(|_| String::from_utf8_lossy(&src[n.byte_range()]).into_owned())
}

fn first_identifier(n: Node<'_>) -> Option<Node<'_>> {
    if n.kind() == "identifier" {
        return Some(n);
    }
    n.named_child(0).and_then(first_identifier)
}

/// The dotted name path of an `identifier` or an `attribute` chain rooted at
/// an identifier; `None` for any other expression.
fn name_path(src: &[u8], n: Node<'_>) -> Option<Path> {
    match n.kind() {
        "identifier" => Some(SmallVec::from_elem(text(src, n), 1)),
        "attribute" => {
            let mut path = name_path(src, n.child_by_field_name("object")?)?;
            path.push(text(src, n.child_by_field_name("attribute")?));
            Some(path)
        }
        _ => None,
    }
}

fn literal_kind(src: &[u8], n: Node<'_>) -> Option<LiteralKind> {
    Some(match n.kind() {
        "string" => string_kind(src, n),
        "concatenated_string" => n
            .named_child(0)
            .map(|s| string_kind(src, s))
            .unwrap_or(LiteralKind::Str),
        "integer" | "float" => LiteralKind::Num,
        "true" | "false" => LiteralKind::Bool,
        "list" | "list_comprehension" => LiteralKind::List,
        "dictionary" | "dictionary_comprehension" => LiteralKind::Dict,
        "set" | "set_comprehension" => LiteralKind::Set,
        "tuple" | "expression_list" => LiteralKind::Tuple,
        _ => return None,
    })
}

/// `Bytes` when the string's prefix carries `b`/`B`, else `Str`.
fn string_kind(src: &[u8], s: Node<'_>) -> LiteralKind {
    let prefix = s
        .named_child(0)
        .filter(|n| n.kind() == "string_start")
        .map(|n| text(src, n))
        .unwrap_or_default();
    if prefix.contains(['b', 'B']) {
        LiteralKind::Bytes
    } else {
        LiteralKind::Str
    }
}

fn subscript_args(subscript: Node<'_>) -> Vec<Node<'_>> {
    let mut cur = subscript.walk();
    subscript
        .children_by_field_name("subscript", &mut cur)
        .collect()
}

/// The exception type(s) of an `except` clause: a name, or a tuple of names
/// (a `Union`).
fn exception_type(src: &[u8], n: Node<'_>) -> TypeExpr {
    match n.kind() {
        "tuple" | "expression_list" => {
            let mut cur = n.walk();
            let members: Vec<Option<TypeExpr>> = n
                .named_children(&mut cur)
                .map(|m| Some(exception_type(src, m)))
                .collect();
            union(members).unwrap_or(TypeExpr::Unknown)
        }
        "parenthesized_expression" => n
            .named_child(0)
            .map(|i| exception_type(src, i))
            .unwrap_or(TypeExpr::Unknown),
        _ => type_expr(src, n),
    }
}

/// Normalize an annotation node into a [`TypeExpr`].
pub(crate) fn type_expr(src: &[u8], n: Node<'_>) -> TypeExpr {
    ty(src, n).unwrap_or(TypeExpr::Unknown)
}

/// `None` means the `None` type itself, which a union drops.
fn ty(src: &[u8], n: Node<'_>) -> Option<TypeExpr> {
    Some(match n.kind() {
        "type" | "parenthesized_expression" => match n.named_child(0) {
            Some(inner) => return ty(src, inner),
            None => TypeExpr::Unknown,
        },
        "none" => return None,
        "identifier" | "attribute" => match name_path(src, n) {
            Some(path) if is_any(&path) => TypeExpr::Unknown,
            Some(path) => TypeExpr::Named {
                path,
                indirect: false,
            },
            None => TypeExpr::Unknown,
        },
        "generic_type" => {
            let head = n.named_child(0).and_then(|h| name_path(src, h));
            let args: Vec<Node<'_>> = n
                .named_children(&mut n.walk())
                .filter(|c| c.kind() == "type_parameter")
                .flat_map(|tp| {
                    let mut cur = tp.walk();
                    tp.named_children(&mut cur).collect::<Vec<_>>()
                })
                .collect();
            return subscripted(src, head, &args);
        }
        "subscript" => {
            let head = n
                .child_by_field_name("value")
                .and_then(|h| name_path(src, h));
            return subscripted(src, head, &subscript_args(n));
        }
        "binary_operator" => {
            let is_pipe = n
                .child_by_field_name("operator")
                .is_some_and(|op| op.kind() == "|");
            if !is_pipe {
                return Some(TypeExpr::Unknown);
            }
            let l = n.child_by_field_name("left").map(|l| ty(src, l));
            let r = n.child_by_field_name("right").map(|r| ty(src, r));
            return union(l.into_iter().chain(r).collect());
        }
        "string" => return string_annotation(src, n),
        _ => TypeExpr::Unknown,
    })
}

/// `Head[args]`: `Optional[X]` → `X` (`None` dropped), `Union[..]` → a union,
/// anything else → `Generic`.
fn subscripted(src: &[u8], head: Option<Path>, args: &[Node<'_>]) -> Option<TypeExpr> {
    let head = head?;
    let last = head.last().map(String::as_str);
    if matches!(last, Some("Optional" | "Union")) {
        return union(args.iter().map(|a| ty(src, *a)).collect());
    }
    Some(TypeExpr::Generic {
        head,
        args: args.iter().map(|a| type_expr(src, *a)).collect(),
        indirect: false,
    })
}

/// Flatten nested unions, drop `None` members and duplicates (first wins),
/// collapse a single member. An empty result is the `None` type.
fn union(members: Vec<Option<TypeExpr>>) -> Option<TypeExpr> {
    let mut flat: Vec<TypeExpr> = Vec::new();
    for m in members.into_iter().flatten() {
        let items = match m {
            TypeExpr::Union(inner) => inner,
            other => vec![other],
        };
        for t in items {
            if !flat.contains(&t) {
                flat.push(t);
            }
        }
    }
    match flat.len() {
        0 => None,
        1 => flat.pop(),
        _ => Some(TypeExpr::Union(flat)),
    }
}

fn is_any(path: &Path) -> bool {
    match path.as_slice() {
        [a] => a == "Any",
        [m, a] => a == "Any" && matches!(m.as_str(), "typing" | "typing_extensions"),
        _ => false,
    }
}

/// A forward-reference annotation (`"pkg.Foo"`, `"Optional[Foo]"`): re-parse
/// its content as an expression. Anything that does not parse to a single
/// expression (including f-strings) is `Unknown`.
fn string_annotation(src: &[u8], s: Node<'_>) -> Option<TypeExpr> {
    let mut content = String::new();
    let mut cur = s.walk();
    for part in s.named_children(&mut cur) {
        match part.kind() {
            "string_start" | "string_end" => {}
            "string_content" => content.push_str(&text(src, part)),
            _ => return Some(TypeExpr::Unknown),
        }
    }
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .is_err()
    {
        return Some(TypeExpr::Unknown);
    }
    let Some(tree) = parser.parse(content.as_bytes(), None) else {
        return Some(TypeExpr::Unknown);
    };
    let root = tree.root_node();
    if root.has_error() || root.named_child_count() != 1 {
        return Some(TypeExpr::Unknown);
    }
    let expr = root
        .named_child(0)
        .filter(|st| st.kind() == "expression_statement" && st.named_child_count() == 1)
        .and_then(|st| st.named_child(0));
    match expr {
        Some(e) => ty(content.as_bytes(), e),
        None => Some(TypeExpr::Unknown),
    }
}
