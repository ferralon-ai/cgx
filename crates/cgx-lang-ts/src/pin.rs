//! Which closure calls are pinned to a single `const` declaration.
//!
//! The extractor classifies a bare call `f()` as a closure call when `f` names
//! a lambda binding somewhere in the file. The resolver may bind such a call
//! lexically to that declaration only when three things hold, none of which it
//! can check from the scope tree alone:
//!
//! 1. **Immutable.** The binding is `const f = <arrow | function expression>`.
//!    A `let`/`var` binding can be reassigned from anywhere it is visible,
//!    including other functions, so the declaration does not fix the value.
//! 2. **Unshadowed and visible.** Walking outwards from the call, the first
//!    function frame that binds `f` at all (parameters, declarations in any of
//!    its blocks, `catch`/`for` bindings, function and class declarations,
//!    imports) binds it exactly once, by that `const` declaration, and the call
//!    lies inside the declaration's block. Parameters are not definitions in the
//!    scope tree, so the resolver cannot see a parameter that shadows `f`.
//! 3. **Unambiguous for the resolver.** The scope tree has one scope per
//!    function, not per block, so a lambda declared in a block that does not
//!    enclose the call can still look like an enclosing binding. Requiring the
//!    declaring frame's whole subtree to hold exactly one lambda declaration
//!    named `f` rules that out.
//!
//! Frames are counted over their whole body rather than block by block, so a
//! same-named binding in an unrelated block of the same function unpins the
//! call. That loses some precision and never pins a call to the wrong target.
//!
//! Nothing is pinned in a file that contains a `with` statement or a direct
//! `eval(…)` call: in sloppy-mode scripts either can introduce a binding no
//! syntax shows. Nothing is pinned through a parse-error node: an ERROR node
//! between the call and its frame, or between the declaration and its frame,
//! means the bindings this pass counts may not be the ones the source has.
//!
//! A call that precedes its `const` in the same block (a temporal-dead-zone
//! error at run time) is still pinned: the declaration is the only binding it
//! can name.
//!
//! The pass runs once per file over the syntax tree and maps the start byte of
//! each pinned callee identifier to the start byte of its declarator. The
//! extractor keeps a pin only if it emitted a `Lambda` definition for that
//! declarator; the resolver binds the call to that definition.

use std::collections::HashMap;

use tree_sitter::Node;

/// Node kinds that open a function frame: their parameters and body bindings
/// do not escape them.
const FUNCTION_LIKE: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "generator_function",
    "arrow_function",
    "method_definition",
];

/// Node kinds that delimit the visibility of a `const` declared directly in them.
const BLOCK_LIKE: &[&str] = &[
    "program",
    "statement_block",
    "switch_body",
    "class_static_block",
    "for_statement",
    "for_in_statement",
];

/// Callee-identifier start byte → declarator start byte, for every pinned
/// closure call in the tree rooted at `root`.
pub(crate) fn pinned_closure_calls(root: Node<'_>, src: &[u8]) -> HashMap<usize, usize> {
    let mut analysis = Analysis {
        src,
        lambda_decls: HashMap::new(),
        frames: HashMap::new(),
    };
    let mut calls: Vec<(Node<'_>, String)> = Vec::new();

    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "with_statement" => return HashMap::new(),
            "variable_declarator" => {
                if let (Some(name), Some(value)) = (
                    n.child_by_field_name("name"),
                    n.child_by_field_name("value"),
                ) {
                    if name.kind() == "identifier" && is_lambda_value(value) {
                        analysis
                            .lambda_decls
                            .entry(analysis.text(name).to_owned())
                            .or_default()
                            .push(n.start_byte());
                    }
                }
            }
            "call_expression" => {
                if let Some(ident) = n.child_by_field_name("function").and_then(callee_ident) {
                    let name = analysis.text(ident);
                    if name == "eval" {
                        return HashMap::new();
                    }
                    calls.push((ident, name.to_owned()));
                }
            }
            _ => {}
        }
        push_named_children(n, &mut stack);
    }

    calls
        .into_iter()
        .filter_map(|(ident, name)| {
            if !analysis.lambda_decls.contains_key(&name) {
                return None;
            }
            analysis
                .is_pinned(ident, &name)
                .map(|decl| (ident.start_byte(), decl))
        })
        .collect()
}

/// The bindings of one name in one function frame.
#[derive(Clone, Copy, Default)]
struct FrameBindings {
    /// Every binding of the name in the frame (parameters and all blocks,
    /// nested frames excluded).
    count: u32,
    /// The frame's `const` lambda declaration of the name, when it has one
    /// and no ERROR node separates it from the frame.
    const_lambda: Option<ConstLambda>,
}

#[derive(Clone, Copy)]
struct ConstLambda {
    /// Start byte of the declarator.
    decl: usize,
    /// Byte range of the block the declaration is directly in.
    block: (usize, usize),
}

struct Analysis<'s> {
    src: &'s [u8],
    /// Lambda declarator start bytes, by bound name (all of `const`/`let`/`var`).
    lambda_decls: HashMap<String, Vec<usize>>,
    /// Memoized frame scans, keyed by (frame node id, name).
    frames: HashMap<(usize, String), FrameBindings>,
}

impl<'s> Analysis<'s> {
    fn text(&self, n: Node<'_>) -> &'s str {
        std::str::from_utf8(&self.src[n.start_byte()..n.end_byte()]).unwrap_or("")
    }

    /// The declarator start byte the call is pinned to, if it is pinned.
    fn is_pinned(&mut self, ident: Node<'_>, name: &str) -> Option<usize> {
        let mut cur = ident.parent();
        while let Some(frame) = cur {
            if frame.is_error() {
                return None;
            }
            if frame.kind() == "program" || FUNCTION_LIKE.contains(&frame.kind()) {
                let b = self.frame_bindings(frame, name);
                if b.count > 0 {
                    let c = b.const_lambda?;
                    let (start, end) = c.block;
                    if b.count != 1 || ident.start_byte() < start || ident.end_byte() > end {
                        return None;
                    }
                    let in_frame = self.lambda_decls[name]
                        .iter()
                        .filter(|&&at| frame.start_byte() <= at && at < frame.end_byte())
                        .count();
                    return (in_frame == 1).then_some(c.decl);
                }
            }
            cur = frame.parent();
        }
        None
    }

    fn frame_bindings(&mut self, frame: Node<'_>, name: &str) -> FrameBindings {
        let key = (frame.id(), name.to_owned());
        if let Some(b) = self.frames.get(&key) {
            return *b;
        }
        let mut b = FrameBindings::default();
        let mut stack: Vec<Node<'_>> = Vec::new();
        if frame.kind() == "program" {
            push_named_children(frame, &mut stack);
        } else {
            // A named function expression binds its own name inside itself.
            if matches!(frame.kind(), "function_expression" | "generator_function") {
                if let Some(own) = frame.child_by_field_name("name") {
                    b.count += u32::from(self.text(own) == name);
                }
            }
            for field in ["parameters", "parameter"] {
                if let Some(p) = frame.child_by_field_name(field) {
                    b.count += self.count_idents(p, name);
                }
            }
            if let Some(body) = frame.child_by_field_name("body") {
                stack.push(body);
            }
        }

        while let Some(n) = stack.pop() {
            let kind = n.kind();
            if FUNCTION_LIKE.contains(&kind) {
                // A nested function's bindings stay inside it; only a declared
                // function's own name lands in this frame.
                if matches!(
                    kind,
                    "function_declaration" | "generator_function_declaration"
                ) {
                    b.count += self.name_field_is(n, name);
                }
                continue;
            }
            match kind {
                "variable_declarator" => {
                    if let Some(target) = n.child_by_field_name("name") {
                        if target.kind() == "identifier" {
                            if self.text(target) == name {
                                b.count += 1;
                                if self.is_const_lambda(n) && !error_between(n, frame) {
                                    b.const_lambda = declaration_block(n).map(|block| ConstLambda {
                                        decl: n.start_byte(),
                                        block,
                                    });
                                }
                            }
                        } else {
                            b.count += self.count_idents(target, name);
                        }
                    }
                    if let Some(value) = n.child_by_field_name("value") {
                        stack.push(value);
                    }
                    continue;
                }
                "for_in_statement" => {
                    if let Some(left) = n.child_by_field_name("left") {
                        b.count += self.count_idents(left, name);
                    }
                }
                "catch_clause" => {
                    if let Some(param) = n.child_by_field_name("parameter") {
                        b.count += self.count_idents(param, name);
                    }
                }
                "import_statement" | "import_alias" => {
                    b.count += self.count_idents(n, name);
                    continue;
                }
                "class_declaration" | "abstract_class_declaration" | "class" | "enum_declaration"
                | "internal_module" => {
                    b.count += self.name_field_is(n, name);
                }
                _ => {}
            }
            push_named_children(n, &mut stack);
        }

        self.frames.insert(key, b);
        b
    }

    fn name_field_is(&self, n: Node<'_>, name: &str) -> u32 {
        n.child_by_field_name("name")
            .map(|id| u32::from(self.text(id) == name))
            .unwrap_or(0)
    }

    /// Identifiers spelled `name` anywhere under `n`. Used on binding patterns
    /// and parameter lists; it also counts uses inside default values and type
    /// annotations, which can only make a call less likely to be pinned.
    fn count_idents(&self, n: Node<'_>, name: &str) -> u32 {
        let mut count = 0;
        let mut stack = vec![n];
        while let Some(n) = stack.pop() {
            if matches!(n.kind(), "identifier" | "shorthand_property_identifier_pattern")
                && self.text(n) == name
            {
                count += 1;
            }
            push_named_children(n, &mut stack);
        }
        count
    }

    fn is_const_lambda(&self, declarator: Node<'_>) -> bool {
        let lambda = declarator
            .child_by_field_name("value")
            .is_some_and(is_lambda_value);
        let is_const = declarator.parent().is_some_and(|decl| {
            decl.kind() == "lexical_declaration"
                && decl
                    .child_by_field_name("kind")
                    .is_some_and(|k| self.text(k) == "const")
        });
        lambda && is_const
    }
}

fn is_lambda_value(value: Node<'_>) -> bool {
    matches!(value.kind(), "arrow_function" | "function_expression")
}

/// The callee identifier of a call, looking through parentheses (`(f)(x)`).
pub(crate) fn callee_ident(callee: Node<'_>) -> Option<Node<'_>> {
    match callee.kind() {
        "identifier" => Some(callee),
        "parenthesized_expression" => callee.named_child(0).and_then(callee_ident),
        _ => None,
    }
}

/// Whether an ERROR node lies on the path from `n` up to (excluding) `frame`.
fn error_between(n: Node<'_>, frame: Node<'_>) -> bool {
    let mut cur = n.parent();
    while let Some(a) = cur {
        if a.id() == frame.id() {
            return false;
        }
        if a.is_error() {
            return true;
        }
        cur = a.parent();
    }
    false
}

/// Byte range of the block a declarator's declaration is directly in.
fn declaration_block(declarator: Node<'_>) -> Option<(usize, usize)> {
    let mut cur = declarator.parent();
    while let Some(n) = cur {
        if BLOCK_LIKE.contains(&n.kind()) {
            return Some((n.start_byte(), n.end_byte()));
        }
        cur = n.parent();
    }
    None
}

fn push_named_children<'t>(n: Node<'t>, stack: &mut Vec<Node<'t>>) {
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        stack.push(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse(src: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .unwrap();
        parser.parse(src, None).unwrap()
    }

    /// Start byte of the identifier spelled `name` on 1-based `line`, and
    /// whether any of its ancestors is an ERROR node.
    fn ident_at(root: Node<'_>, src: &str, name: &str, line: usize) -> (usize, bool) {
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if n.kind() == "identifier"
                && n.start_position().row + 1 == line
                && &src[n.byte_range()] == name
            {
                let mut under_error = false;
                let mut cur = n.parent();
                while let Some(a) = cur {
                    under_error |= a.is_error();
                    cur = a.parent();
                }
                return (n.start_byte(), under_error);
            }
            push_named_children(n, &mut stack);
        }
        panic!("no `{name}` on line {line}");
    }

    #[test]
    fn a_call_under_an_error_node_is_not_pinned() {
        // The JSX makes the TypeScript grammar recover with an ERROR node that
        // holds `g`'s arrow, so the call on line 9 has an ERROR ancestor between
        // it and the declaring frame.
        let src = "const f = (v: string) => v;\nexport const C = () => {\n  return (\n    <Box>\n      {f(\"x\")}\n    </Box>\n  );\n};\nconst g = () => f(\"y\");\n";
        let tree = parse(src);
        let (call, under_error) = ident_at(tree.root_node(), src, "f", 9);
        assert!(under_error, "precondition: the call sits under an ERROR node");
        let pinned = pinned_closure_calls(tree.root_node(), src.as_bytes());
        assert!(!pinned.contains_key(&call));
    }
}
