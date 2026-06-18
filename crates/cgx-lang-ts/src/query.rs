//! Node-kind classification helpers for the TypeScript tree-sitter grammar.
//!
//! Maps tree-sitter node kind strings to the cgx facts the TS frontend needs.
//! All names come from the grammar's `node-types.json`; they are stable within a
//! pinned grammar version.

/// Whether a method name looks like a higher-order array method that implies a
/// loop-condition on calls inside the callback argument.
pub fn is_loop_method(name: &str) -> bool {
    matches!(
        name,
        "map"
            | "forEach"
            | "filter"
            | "reduce"
            | "reduceRight"
            | "some"
            | "every"
            | "find"
            | "findIndex"
            | "flatMap"
            | "flat"
    )
}

/// Extract a string value from a string node (strips surrounding quotes).
pub fn extract_string_value(src: &[u8], node: tree_sitter::Node<'_>) -> Option<String> {
    let text = node.utf8_text(src).ok()?;
    if text.len() < 2 {
        return None;
    }
    let first = text.chars().next()?;
    let last = text.chars().last()?;
    let is_quoted = (first == '"' && last == '"')
        || (first == '\'' && last == '\'')
        || (first == '`' && last == '`');
    if is_quoted {
        Some(text[1..text.len() - 1].to_string())
    } else {
        None
    }
}
