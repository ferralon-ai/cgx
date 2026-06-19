//! SCIP symbol string → cgx qualified-name mapping (design §3.3) plus the two
//! gotchas the mapping must encode (§3.4).
//!
//! ## Grammar (from `findings/scip-ingestion.md` §1)
//!
//! ```text
//! <symbol>     ::= <scheme> ' ' <package> ' ' <descriptor>+ | 'local ' <id>
//! <package>    ::= <manager> ' ' <package-name> ' ' <version>
//! <descriptor> ::= <name> '/'              (namespace/module)
//!                | <name> '#'              (type: struct/enum/trait)
//!                | <name> '(' <disamb>? ').'  (method)
//!                | <name> '.'              (term/const/static)
//!                | <name> '!'              (macro)
//!                | '[' <name> ']'          (type-parameter — dropped)
//!                | '(' <name> ')'          (parameter — dropped)
//! ```
//!
//! Worked example (design §3.3):
//! `rust-analyzer cargo cgx_core 0.1.0 cut/CutMarkers#iter().`
//! → qname `cgx_core::cut::CutMarkers::iter`, pkg `cgx_core`, version `0.1.0`.
//!
//! The descriptor names are joined with `::`; the package name is **prepended**
//! so the qname is crate-rooted, matching cgx's FQN convention.

use std::collections::BTreeMap;

/// SCIP `Kind` enum value for `Trait`. The SCIP schema assigns `Trait = 64`.
/// Used as a corroborating signal for trait-member classification; the
/// descriptor-grammar path is the primary classifier.
pub const SCIP_KIND_TRAIT: i32 = 64;

/// The classification of a resolved SCIP symbol for confidence purposes
/// (gotcha 1). A trait member can never yield `certain` downstream because
/// rust-analyzer resolves the occurrence to the trait *declaration*, not the
/// concrete impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymClass {
    /// A free function or an inherent (non-trait) method — eligible for
    /// `certain` when its def-site is unique.
    FreeOrInherent,
    /// A trait method / associated item — capped at `probable` downstream.
    TraitMember,
}

/// What kind of entity a descriptor names (decided by its trailing sigil).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DescKind {
    /// `name/` — namespace / module.
    Namespace,
    /// `name#` — type (struct / enum / trait).
    Type,
    /// `name().` — method.
    Method,
    /// `name.` — term / const / static.
    Term,
    /// `name!` — macro.
    Macro,
    /// `[name]` or `(name)` — type-param / parameter (dropped from the qname).
    Dropped,
}

/// A single parsed descriptor: its surviving name and what it names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Descriptor {
    name: String,
    kind: DescKind,
}

/// The result of mapping a SCIP symbol to cgx coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedSymbol {
    /// The crate-rooted cgx qualified name (e.g. `cgx_core::cut::CutMarkers::iter`).
    pub qname: String,
    /// The Cargo package name (GM-14 dependency attribute).
    pub package: String,
    /// The semver version string (GM-14 dependency attribute).
    pub version: String,
}

/// True for a `local <id>` symbol (intra-document only; no cross-file qname).
pub fn is_local(symbol: &str) -> bool {
    symbol == "local" || symbol.starts_with("local ")
}

/// Map a SCIP symbol string to a cgx qname + `(pkg, version)`.
///
/// Returns `None` for a `local` symbol (no cross-file identity) or a symbol that
/// is too short / malformed to carry a package + descriptor.
pub fn map_symbol(symbol: &str) -> Option<MappedSymbol> {
    if is_local(symbol) {
        return None;
    }
    let (package, version, descriptors) = tokenize(symbol)?;
    let mut names: Vec<String> = Vec::new();
    for d in &descriptors {
        match d.kind {
            DescKind::Dropped => {}
            _ => names.push(d.name.clone()),
        }
    }
    if names.is_empty() {
        return None;
    }
    let qname = std::iter::once(package.clone())
        .chain(names)
        .collect::<Vec<_>>()
        .join("::");
    Some(MappedSymbol {
        qname,
        package,
        version,
    })
}

/// Classify a symbol as trait-member vs free/inherent (gotcha 1).
///
/// Decision order:
/// 1. If the symbol or its enclosing symbol is *known* to be trait-kinded via
///    the SCIP `Kind` table, it is a [`SymClass::TraitMember`].
/// 2. Otherwise fall back to the descriptor grammar: the descriptor enclosing
///    the final method/term is the symbol's container; if that container `Type`
///    descriptor is trait-kinded we'd need the kind table — without it, we treat
///    a method whose enclosing type is recorded as a trait as a member, and
///    everything else as free/inherent.
///
/// The grammar alone cannot distinguish a trait method from an inherent method
/// (both are `Type#method().`), so the [`Kind`]/`enclosing_symbol` tables are
/// the authoritative signal; the conservative fallback when no table entry
/// exists is `FreeOrInherent` (the def-site multiplicity in [`crate::ScipResolver`]
/// still guards uniqueness for the #18772 case).
///
/// [`Kind`]: SCIP_KIND_TRAIT
pub fn classify(
    symbol: &str,
    kind_by_symbol: &BTreeMap<String, i32>,
    enclosing_by_symbol: &BTreeMap<String, String>,
) -> SymClass {
    if is_local(symbol) {
        return SymClass::FreeOrInherent;
    }

    // 1. Direct kind: the symbol itself is a Trait (rare for a call target, but
    //    a trait associated const/fn symbol may carry Trait kind on enclosing).
    if let Some(enclosing) = enclosing_by_symbol.get(symbol) {
        if kind_by_symbol.get(enclosing) == Some(&SCIP_KIND_TRAIT)
            || enclosing_is_trait_by_grammar(enclosing)
        {
            return SymClass::TraitMember;
        }
    }

    // 2. Reconstruct the enclosing type symbol from this symbol's own
    //    descriptors and consult the kind table for it.
    if let Some(enclosing) = enclosing_type_symbol(symbol) {
        if kind_by_symbol.get(&enclosing) == Some(&SCIP_KIND_TRAIT) {
            return SymClass::TraitMember;
        }
    }

    SymClass::FreeOrInherent
}

/// True when a symbol's *trailing* type descriptor was, per a heuristic on the
/// symbol grammar, a trait. Used only as a corroborating signal; the SCIP
/// `Kind` table is authoritative.
fn enclosing_is_trait_by_grammar(symbol: &str) -> bool {
    // We cannot tell trait from struct/enum by sigil alone (`#` covers all);
    // this hook exists so the resolver can plug a richer signal in later. For
    // now, no grammar-only trait detection.
    let _ = symbol;
    false
}

/// Given a symbol whose final descriptor is a method/term, rebuild the symbol
/// string of its enclosing `Type#` so the caller can look it up in the kind
/// table. Returns `None` if there is no enclosing type descriptor.
fn enclosing_type_symbol(symbol: &str) -> Option<String> {
    let (_pkg, _ver, descriptors) = tokenize(symbol)?;
    // Find the last Type descriptor that precedes the final named descriptor.
    let last_named = descriptors
        .iter()
        .rposition(|d| matches!(d.kind, DescKind::Method | DescKind::Term | DescKind::Macro))?;
    let type_idx = descriptors[..last_named]
        .iter()
        .rposition(|d| d.kind == DescKind::Type)?;
    // Rebuild: <scheme> <mgr> <pkg> <ver> <descriptors[..=type_idx]>.
    let prefix: Vec<&str> = symbol.splitn(5, ' ').take(4).collect();
    if prefix.len() < 4 {
        return None;
    }
    let mut rebuilt = prefix.join(" ");
    rebuilt.push(' ');
    for d in &descriptors[..=type_idx] {
        rebuilt.push_str(&render_descriptor(d));
    }
    Some(rebuilt)
}

/// Re-emit a descriptor in SCIP wire form (for symbol reconstruction).
fn render_descriptor(d: &Descriptor) -> String {
    match d.kind {
        DescKind::Namespace => format!("{}/", d.name),
        DescKind::Type => format!("{}#", d.name),
        DescKind::Method => format!("{}().", d.name),
        DescKind::Term => format!("{}.", d.name),
        DescKind::Macro => format!("{}!", d.name),
        DescKind::Dropped => String::new(),
    }
}

/// Split a symbol into `(package, version, descriptors)`.
///
/// `<scheme> <manager> <package> <version> <descriptors...>` — the first four
/// space-separated tokens are the prefix; the remainder is the descriptor tail.
fn tokenize(symbol: &str) -> Option<(String, String, Vec<Descriptor>)> {
    // Split into at most 5 parts: scheme, manager, package, version, tail.
    let mut parts = symbol.splitn(5, ' ');
    let _scheme = parts.next()?;
    let _manager = parts.next()?;
    let package = parts.next()?;
    let version = parts.next()?;
    let tail = parts.next().unwrap_or("");
    if package.is_empty() || version.is_empty() {
        return None;
    }
    Some((
        package.to_string(),
        version.to_string(),
        parse_descriptors(tail),
    ))
}

/// Parse the descriptor tail into a list of typed descriptors, classifying each
/// by its trailing sigil.
fn parse_descriptors(tail: &str) -> Vec<Descriptor> {
    let mut out = Vec::new();
    let chars: Vec<char> = tail.chars().collect();
    let mut i = 0usize;
    let n = chars.len();
    while i < n {
        match chars[i] {
            // `[name]` type-parameter — dropped.
            '[' => {
                if let Some(close) = find_close(&chars, i, '[', ']') {
                    out.push(Descriptor {
                        name: chars[i + 1..close].iter().collect(),
                        kind: DescKind::Dropped,
                    });
                    i = close + 1;
                } else {
                    break;
                }
            }
            // `(name)` parameter — dropped (only when not a method's `().`).
            '(' => {
                if let Some(close) = find_close(&chars, i, '(', ')') {
                    out.push(Descriptor {
                        name: chars[i + 1..close].iter().collect(),
                        kind: DescKind::Dropped,
                    });
                    i = close + 1;
                } else {
                    break;
                }
            }
            _ => {
                // Read a name up to the next sigil, allowing `(` to start a
                // method's disambiguator group `name(...).`.
                let start = i;
                while i < n && !matches!(chars[i], '/' | '#' | '.' | '!' | '(') {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect();
                if i >= n {
                    // Trailing bare name with no sigil — treat as a term-like leaf.
                    if !name.is_empty() {
                        out.push(Descriptor {
                            name,
                            kind: DescKind::Term,
                        });
                    }
                    break;
                }
                match chars[i] {
                    '/' => {
                        out.push(Descriptor {
                            name,
                            kind: DescKind::Namespace,
                        });
                        i += 1;
                    }
                    '#' => {
                        out.push(Descriptor {
                            name,
                            kind: DescKind::Type,
                        });
                        i += 1;
                    }
                    '!' => {
                        out.push(Descriptor {
                            name,
                            kind: DescKind::Macro,
                        });
                        i += 1;
                    }
                    '(' => {
                        // Method: `name(<disamb>?).` — skip the paren group, then
                        // the trailing `.`.
                        if let Some(close) = find_close(&chars, i, '(', ')') {
                            // Expect a `.` immediately after the `)`.
                            let after = close + 1;
                            out.push(Descriptor {
                                name,
                                kind: DescKind::Method,
                            });
                            i = if after < n && chars[after] == '.' {
                                after + 1
                            } else {
                                after
                            };
                        } else {
                            break;
                        }
                    }
                    '.' => {
                        out.push(Descriptor {
                            name,
                            kind: DescKind::Term,
                        });
                        i += 1;
                    }
                    _ => unreachable!("loop condition excludes other chars"),
                }
            }
        }
    }
    out
}

/// Find the index of the matching close delimiter for an open at `open_idx`.
fn find_close(chars: &[char], open_idx: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, &c) in chars[open_idx..].iter().enumerate() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(open_idx + offset);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_tables() -> (BTreeMap<String, i32>, BTreeMap<String, String>) {
        (BTreeMap::new(), BTreeMap::new())
    }

    #[test]
    fn maps_design_worked_example() {
        let sym = "rust-analyzer cargo cgx_core 0.1.0 cut/CutMarkers#iter().";
        let m = map_symbol(sym).expect("should map");
        assert_eq!(m.qname, "cgx_core::cut::CutMarkers::iter");
        assert_eq!(m.package, "cgx_core");
        assert_eq!(m.version, "0.1.0");
    }

    #[test]
    fn maps_free_function() {
        let sym = "rust-analyzer cargo mycrate 1.2.3 parse().";
        let m = map_symbol(sym).expect("should map");
        assert_eq!(m.qname, "mycrate::parse");
        assert_eq!(m.package, "mycrate");
    }

    #[test]
    fn maps_inherent_method_with_module_path() {
        let sym = "rust-analyzer cargo mycrate 1.2.3 io/Reader#read().";
        let m = map_symbol(sym).expect("should map");
        assert_eq!(m.qname, "mycrate::io::Reader::read");
    }

    #[test]
    fn maps_term_const() {
        let sym = "rust-analyzer cargo mycrate 1.2.3 config/MAX_SIZE.";
        let m = map_symbol(sym).expect("should map");
        assert_eq!(m.qname, "mycrate::config::MAX_SIZE");
    }

    #[test]
    fn maps_macro() {
        let sym = "rust-analyzer cargo mycrate 1.2.3 my_macro!";
        let m = map_symbol(sym).expect("should map");
        assert_eq!(m.qname, "mycrate::my_macro");
    }

    #[test]
    fn drops_type_parameters_and_parameters() {
        let sym = "rust-analyzer cargo mycrate 1.2.3 Vec#[T]push().(value)";
        let m = map_symbol(sym).expect("should map");
        // `[T]` and `(value)` dropped; `Vec` + `push` survive.
        assert_eq!(m.qname, "mycrate::Vec::push");
    }

    #[test]
    fn local_symbol_has_no_qname() {
        assert!(map_symbol("local 7").is_none());
        assert!(map_symbol("local 0").is_none());
        assert!(is_local("local 42"));
    }

    #[test]
    fn malformed_short_symbol_is_none() {
        assert!(map_symbol("rust-analyzer cargo").is_none());
        assert!(map_symbol("").is_none());
    }

    #[test]
    fn method_with_disambiguator() {
        let sym = "rust-analyzer cargo mycrate 1.2.3 Foo#bar(+1).";
        let m = map_symbol(sym).expect("should map");
        assert_eq!(m.qname, "mycrate::Foo::bar");
    }

    #[test]
    fn classify_free_function_when_no_table() {
        let (kinds, encl) = empty_tables();
        let sym = "rust-analyzer cargo mycrate 1.2.3 parse().";
        assert_eq!(classify(sym, &kinds, &encl), SymClass::FreeOrInherent);
    }

    #[test]
    fn classify_inherent_method_default_free() {
        let (kinds, encl) = empty_tables();
        // No kind info → conservative FreeOrInherent (uniqueness still guards).
        let sym = "rust-analyzer cargo mycrate 1.2.3 io/Reader#read().";
        assert_eq!(classify(sym, &kinds, &encl), SymClass::FreeOrInherent);
    }

    #[test]
    fn classify_trait_member_via_kind_table() {
        // The enclosing type `Iterator#` is recorded as a Trait in the kind
        // table → the method is a TraitMember.
        let method = "rust-analyzer cargo core 0.1.0 iter/Iterator#next().";
        let trait_sym = "rust-analyzer cargo core 0.1.0 iter/Iterator#";
        let mut kinds = BTreeMap::new();
        kinds.insert(trait_sym.to_string(), SCIP_KIND_TRAIT);
        let encl = BTreeMap::new();
        assert_eq!(classify(method, &kinds, &encl), SymClass::TraitMember);
    }

    #[test]
    fn classify_trait_member_via_enclosing_symbol() {
        let method = "rust-analyzer cargo core 0.1.0 some_assoc().";
        let trait_sym = "rust-analyzer cargo core 0.1.0 MyTrait#";
        let mut kinds = BTreeMap::new();
        kinds.insert(trait_sym.to_string(), SCIP_KIND_TRAIT);
        let mut encl = BTreeMap::new();
        encl.insert(method.to_string(), trait_sym.to_string());
        assert_eq!(classify(method, &kinds, &encl), SymClass::TraitMember);
    }

    #[test]
    fn classify_local_is_free() {
        let (kinds, encl) = empty_tables();
        assert_eq!(classify("local 3", &kinds, &encl), SymClass::FreeOrInherent);
    }
}
