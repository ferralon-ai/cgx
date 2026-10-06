//! Receiver-typing facts ([`TypeFact`]) for one Go function or method.
//!
//! [`collect`] upholds the channel's binding-completeness contract for a
//! `function_declaration` / `method_declaration`: a `Param` for the receiver
//! and every named parameter, and one `Bind` per binding site of every local
//! name. Typed forms are per-pair `:=` / `=` with equal arity, `var v = e`,
//! and `var v T [= e]` and named results (`Declared(T)`). Every other binding
//! (multi-value assignment, compound assignment, `++`/`--`, `range`, receive,
//! type-switch alias, local `const`, taking a local's address with `&v`, and
//! any binding inside a `func` literal, including the literal's own
//! parameters) is `Opaque`. The blank identifier `_` binds
//! nothing. Each call with at least one argument and a name-path callee gets a
//! `CallArgs` fact keyed by the call's `(line, col)`.

use cgx_frontend::{CallArg, LiteralKind, Name, ParamKind, TypeExpr, TypeFact, ValueSource};
use smallvec::SmallVec;
use tree_sitter::Node;

type Path = SmallVec<[Name; 2]>;

pub(crate) fn collect(src: &[u8], decl: Node<'_>, fqn: &str) -> Vec<TypeFact> {
    let mut c = Collector {
        src,
        fqn,
        out: Vec::new(),
    };
    if let Some(recv) = decl.child_by_field_name("receiver") {
        for (name, ty) in c.declarations(recv, "parameter_declaration") {
            c.out.push(TypeFact::Param {
                func: fqn.to_owned(),
                name,
                index: None,
                kind: ParamKind::Receiver,
                ty,
            });
        }
    }
    if let Some(params) = decl.child_by_field_name("parameters") {
        c.params(params);
    }
    if let Some(result) = decl.child_by_field_name("result") {
        if result.kind() == "parameter_list" {
            for (name, ty) in c.declarations(result, "parameter_declaration") {
                let src = ty.map(ValueSource::Declared).unwrap_or(ValueSource::Opaque);
                c.bind(name, src);
            }
        }
    }
    if let Some(body) = decl.child_by_field_name("body") {
        c.walk(body, false);
    }
    c.out
}

/// Normalize a Go type node: `T` / `pkg.T` → `Named`, `G[A]` → `Generic`, a
/// single `*` on either sets `indirect`, anything else → `Unknown`.
pub(crate) fn type_expr(src: &[u8], n: Node<'_>) -> TypeExpr {
    match n.kind() {
        "type_identifier" | "qualified_type" => match type_path(src, n) {
            Some(path) => TypeExpr::Named {
                path,
                indirect: false,
            },
            None => TypeExpr::Unknown,
        },
        "pointer_type" => match n.named_child(0).map(|inner| (inner.kind(), inner)) {
            Some(("type_identifier" | "qualified_type", inner)) => match type_path(src, inner) {
                Some(path) => TypeExpr::Named {
                    path,
                    indirect: true,
                },
                None => TypeExpr::Unknown,
            },
            Some(("generic_type", inner)) => match type_expr(src, inner) {
                TypeExpr::Generic { head, args, .. } => TypeExpr::Generic {
                    head,
                    args,
                    indirect: true,
                },
                other => other,
            },
            _ => TypeExpr::Unknown,
        },
        "generic_type" => {
            let head = n
                .child_by_field_name("type")
                .and_then(|t| type_path(src, t));
            let Some(head) = head else {
                return TypeExpr::Unknown;
            };
            let mut args = Vec::new();
            if let Some(ta) = n.child_by_field_name("type_arguments") {
                let mut cur = ta.walk();
                for elem in ta.named_children(&mut cur) {
                    let t = if elem.kind() == "type_elem" && elem.named_child_count() == 1 {
                        elem.named_child(0)
                    } else {
                        Some(elem)
                    };
                    args.push(t.map(|t| type_expr(src, t)).unwrap_or(TypeExpr::Unknown));
                }
            }
            TypeExpr::Generic {
                head,
                args,
                indirect: false,
            }
        }
        "parenthesized_type" => n
            .named_child(0)
            .map(|i| type_expr(src, i))
            .unwrap_or(TypeExpr::Unknown),
        _ => TypeExpr::Unknown,
    }
}

fn type_path(src: &[u8], n: Node<'_>) -> Option<Path> {
    match n.kind() {
        "type_identifier" => Some(SmallVec::from_elem(text(src, n), 1)),
        "qualified_type" => {
            let pkg = n.child_by_field_name("package")?;
            let name = n.child_by_field_name("name")?;
            Some(SmallVec::from_vec(vec![text(src, pkg), text(src, name)]))
        }
        _ => None,
    }
}

fn text(src: &[u8], n: Node<'_>) -> String {
    n.utf8_text(src)
        .map(str::to_string)
        .unwrap_or_else(|_| String::from_utf8_lossy(&src[n.byte_range()]).into_owned())
}

/// The name path of an identifier or a selector chain rooted at one
/// (`pkg.F`, `a.b.M`); `None` otherwise.
fn name_path(src: &[u8], n: Node<'_>) -> Option<Path> {
    match n.kind() {
        "identifier" => Some(SmallVec::from_elem(text(src, n), 1)),
        "selector_expression" => {
            let mut path = name_path(src, n.child_by_field_name("operand")?)?;
            path.push(text(src, n.child_by_field_name("field")?));
            Some(path)
        }
        _ => None,
    }
}

struct Collector<'a> {
    src: &'a [u8],
    fqn: &'a str,
    out: Vec<TypeFact>,
}

impl Collector<'_> {
    fn text(&self, n: Node<'_>) -> String {
        text(self.src, n)
    }

    /// `(name, type)` per named entry of a parameter/result list, in order.
    fn declarations(&self, list: Node<'_>, kind: &str) -> Vec<(String, Option<TypeExpr>)> {
        let mut out = Vec::new();
        let mut cur = list.walk();
        for pd in list.named_children(&mut cur) {
            if pd.kind() != kind {
                continue;
            }
            let ty = pd
                .child_by_field_name("type")
                .map(|t| type_expr(self.src, t));
            let mut nc = pd.walk();
            for n in pd.children_by_field_name("name", &mut nc) {
                out.push((self.text(n), ty.clone()));
            }
        }
        out
    }

    fn params(&mut self, params: Node<'_>) {
        let mut index: u8 = 0;
        let mut cur = params.walk();
        let decls: Vec<Node<'_>> = params.named_children(&mut cur).collect();
        for pd in decls {
            let kind = match pd.kind() {
                "parameter_declaration" => ParamKind::Positional,
                "variadic_parameter_declaration" => ParamKind::VarArgs,
                _ => continue,
            };
            let ty = pd
                .child_by_field_name("type")
                .map(|t| type_expr(self.src, t));
            let mut nc = pd.walk();
            let names: Vec<String> = pd
                .children_by_field_name("name", &mut nc)
                .map(|n| self.text(n))
                .collect();
            if names.is_empty() {
                // An unnamed parameter still occupies a position.
                index = index.saturating_add(1);
                continue;
            }
            for name in names {
                self.out.push(TypeFact::Param {
                    func: self.fqn.to_owned(),
                    name,
                    index: Some(index),
                    kind,
                    ty: ty.clone(),
                });
                index = index.saturating_add(1);
            }
        }
    }

    fn bind(&mut self, var: String, src: ValueSource) {
        if var == "_" {
            return;
        }
        self.out.push(TypeFact::Bind {
            func: self.fqn.to_owned(),
            var,
            src,
        });
    }

    /// Every identifier in `list` (an `expression_list` or a lone identifier)
    /// gets `Opaque`; selector / index targets bind no local.
    fn opaque_idents(&mut self, list: Node<'_>) {
        if list.kind() == "identifier" {
            let t = self.text(list);
            self.bind(t, ValueSource::Opaque);
            return;
        }
        let mut cur = list.walk();
        let ids: Vec<String> = list
            .named_children(&mut cur)
            .filter(|n| n.kind() == "identifier")
            .map(|n| self.text(n))
            .collect();
        for id in ids {
            self.bind(id, ValueSource::Opaque);
        }
    }

    fn walk(&mut self, node: Node<'_>, in_closure: bool) {
        match node.kind() {
            "func_literal" => {
                // The literal's parameters and named results shadow enclosing
                // names inside its body, so they are opaque bindings here.
                for field in ["parameters", "result"] {
                    if let Some(list) = node.child_by_field_name(field) {
                        if list.kind() == "parameter_list" {
                            let mut cur = list.walk();
                            let decls: Vec<Node<'_>> = list.named_children(&mut cur).collect();
                            for pd in decls {
                                let mut nc = pd.walk();
                                let names: Vec<String> = pd
                                    .children_by_field_name("name", &mut nc)
                                    .map(|n| self.text(n))
                                    .collect();
                                for n in names {
                                    self.bind(n, ValueSource::Opaque);
                                }
                            }
                        }
                    }
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.walk(body, true);
                }
                return;
            }
            "short_var_declaration" | "assignment_statement" => {
                let plain = node.kind() == "short_var_declaration"
                    || node
                        .child_by_field_name("operator")
                        .is_some_and(|o| self.text(o) == "=");
                if let (Some(l), Some(r)) = (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) {
                    if plain && !in_closure {
                        self.bind_pairs(l, r);
                    } else {
                        self.opaque_idents(l);
                    }
                }
            }
            "inc_statement" | "dec_statement" => {
                if let Some(id) = node.named_child(0).filter(|n| n.kind() == "identifier") {
                    self.opaque_idents(id);
                }
            }
            "var_spec" => self.var_spec(node, in_closure),
            "const_spec" => {
                let mut nc = node.walk();
                let names: Vec<String> = node
                    .children_by_field_name("name", &mut nc)
                    .map(|n| self.text(n))
                    .collect();
                for n in names {
                    self.bind(n, ValueSource::Opaque);
                }
            }
            "range_clause" | "receive_statement" => {
                if let Some(l) = node.child_by_field_name("left") {
                    self.opaque_idents(l);
                }
            }
            "type_switch_statement" => {
                if let Some(a) = node.child_by_field_name("alias") {
                    self.opaque_idents(a);
                }
            }
            "call_expression" => self.call(node, in_closure),
            "unary_expression" => {
                let is_addr = node
                    .child_by_field_name("operator")
                    .is_some_and(|o| self.text(o) == "&");
                let mut operand = node.child_by_field_name("operand");
                while let Some(p) = operand.filter(|o| o.kind() == "parenthesized_expression") {
                    operand = p.named_child(0);
                }
                if let Some(id) = operand.filter(|o| is_addr && o.kind() == "identifier") {
                    // A write through the pointer can rebind the local.
                    let t = self.text(id);
                    self.bind(t, ValueSource::Opaque);
                }
            }
            _ => {}
        }
        let mut cur = node.walk();
        let kids: Vec<Node<'_>> = node.named_children(&mut cur).collect();
        for k in kids {
            self.walk(k, in_closure);
        }
    }

    fn var_spec(&mut self, node: Node<'_>, in_closure: bool) {
        let mut nc = node.walk();
        let names: Vec<String> = node
            .children_by_field_name("name", &mut nc)
            .map(|n| self.text(n))
            .collect();
        if in_closure {
            for n in names {
                self.bind(n, ValueSource::Opaque);
            }
            return;
        }
        match (
            node.child_by_field_name("value"),
            node.child_by_field_name("type"),
        ) {
            (_, Some(t)) => {
                let ty = type_expr(self.src, t);
                for n in names {
                    self.bind(n, ValueSource::Declared(ty.clone()));
                }
            }
            (Some(v), None) => {
                let mut vc = v.walk();
                let vals: Vec<Node<'_>> = v
                    .named_children(&mut vc)
                    .filter(|n| n.kind() != "comment")
                    .collect();
                if vals.len() == names.len() {
                    for (n, val) in names.into_iter().zip(vals) {
                        let src = self.source(val);
                        self.bind(n, src);
                    }
                } else {
                    for n in names {
                        self.bind(n, ValueSource::Opaque);
                    }
                }
            }
            (None, None) => {
                for n in names {
                    self.bind(n, ValueSource::Opaque);
                }
            }
        }
    }

    /// `l1, l2 := r1, r2` binds pairwise; unequal arity (a multi-value call)
    /// makes every identifier on the left opaque.
    fn bind_pairs(&mut self, left: Node<'_>, right: Node<'_>) {
        let mut lc = left.walk();
        let ls: Vec<Node<'_>> = left
            .named_children(&mut lc)
            .filter(|n| n.kind() != "comment")
            .collect();
        let mut rc = right.walk();
        let rs: Vec<Node<'_>> = right
            .named_children(&mut rc)
            .filter(|n| n.kind() != "comment")
            .collect();
        if ls.len() != rs.len() {
            self.opaque_idents(left);
            return;
        }
        for (l, r) in ls.into_iter().zip(rs) {
            if l.kind() != "identifier" {
                continue;
            }
            let name = self.text(l);
            let src = self.source(r);
            self.bind(name, src);
        }
    }

    fn source(&self, value: Node<'_>) -> ValueSource {
        match value.kind() {
            "identifier" => ValueSource::Var(self.text(value)),
            "nil" => ValueSource::Null,
            "interpreted_string_literal" | "raw_string_literal" => {
                ValueSource::Literal(LiteralKind::Str)
            }
            "int_literal" | "float_literal" | "imaginary_literal" | "rune_literal" => {
                ValueSource::Literal(LiteralKind::Num)
            }
            "true" | "false" => ValueSource::Literal(LiteralKind::Bool),
            "composite_literal" => value
                .child_by_field_name("type")
                .map(|t| ValueSource::New(type_expr(self.src, t)))
                .unwrap_or(ValueSource::Opaque),
            "unary_expression" => {
                let is_addr = value
                    .child_by_field_name("operator")
                    .is_some_and(|o| self.text(o) == "&");
                match value.child_by_field_name("operand") {
                    Some(op) if is_addr && op.kind() == "composite_literal" => {
                        match self.source(op) {
                            ValueSource::New(TypeExpr::Named { path, .. }) => {
                                ValueSource::New(TypeExpr::Named {
                                    path,
                                    indirect: true,
                                })
                            }
                            other => other,
                        }
                    }
                    _ => ValueSource::Opaque,
                }
            }
            "call_expression" => match value
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
            _ => ValueSource::Opaque,
        }
    }

    fn call(&mut self, node: Node<'_>, in_closure: bool) {
        let Some(callee) = node
            .child_by_field_name("function")
            .and_then(|f| name_path(self.src, f))
        else {
            return;
        };
        let Some(args) = node.child_by_field_name("arguments") else {
            return;
        };
        let mut ac = args.walk();
        let arg_nodes: Vec<Node<'_>> = args
            .named_children(&mut ac)
            .filter(|n| n.kind() != "comment")
            .collect();
        if arg_nodes.is_empty() {
            return;
        }
        let args = arg_nodes
            .into_iter()
            .map(|a| CallArg {
                keyword: None,
                value: match self.source(a) {
                    ValueSource::Var(_) if in_closure => ValueSource::Opaque,
                    other => other,
                },
            })
            .collect();
        self.out.push(TypeFact::CallArgs {
            func: self.fqn.to_owned(),
            line: node.start_position().row as u32 + 1,
            col: node.start_position().column as u32 + 1,
            callee,
            args,
        });
    }
}
