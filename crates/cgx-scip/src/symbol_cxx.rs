//! scip-clang (C/C++) SCIP-symbol → cgx-qname mapping, selected behind the
//! per-index scheme seam ([`crate::SymbolScheme`]). Sibling to the rust-analyzer
//! mapper in [`crate::symbol`]; the descriptor grammar is shared
//! ([`crate::symbol::parse_descriptors`]) — only the *packaging* and the
//! *overload identity* differ (ADR B2 Decision 1).
//!
//! ## What differs from the Rust mapper
//!
//! 1. **No Cargo package prefix.** scip-clang emits the standard SCIP 5-token
//!    prefix `<scheme> <manager> <package> <version>`, but a project-local C++
//!    symbol carries the placeholder `.` in every package field (SCIP spec). The
//!    qname is therefore built from the descriptors alone — `cxx . . . ns/f().`
//!    → `ns::f` — never crate-rooted. A dependency symbol carries a real
//!    package-map coordinate, which rides the GM-14 dep edge.
//!
//! 2. **Overload identity is preserved.** cgx qnames are signature-free, so the
//!    two overloads `ns::f(int)` and `ns::f(double)` both yield qname `ns::f`.
//!    The method descriptor's disambiguator (`f(<disamb>).`) is what scip-clang
//!    uses to tell them apart. [`map_symbol`] surfaces the signature-free qname
//!    *and* an `overload_key` (the raw descriptor tail, disambiguator included)
//!    so [`crate::ScipResolver::def_count`] computes multiplicity on `(qname,
//!    overload_key)`. Two occurrences of the *same* overload (header decl + `.cpp`
//!    def) carry an identical symbol string, hence an identical key — so the
//!    cross-TU collapse is a single def, not a false `def_count > 1` collision.
//!
//! 3. **Classification.** Rust's trait-member cap is replaced by a C++
//!    virtual/overridable cap: a symbol that participates in a SCIP `override`
//!    relationship is [`crate::SymClass::VirtualMember`] (capped `probable`);
//!    everything else is `FreeOrInherent`, eligible for `certain` at a unique def.

use std::collections::BTreeSet;

use crate::symbol::{is_local, parse_descriptors, DescKind, MappedSymbol, SymClass};

/// The SCIP placeholder for an empty package field (SCIP spec §Symbol).
const PLACEHOLDER: &str = ".";

/// Map a scip-clang C/C++ SCIP symbol string to cgx coordinates.
///
/// Returns `None` for a `local <id>` symbol (no cross-file identity) or a symbol
/// too short / malformed to carry a descriptor tail.
pub fn map_symbol(symbol: &str) -> Option<MappedSymbol> {
    if is_local(symbol) {
        return None;
    }
    let (package, version, tail) = tokenize(symbol)?;
    let descriptors = parse_descriptors(tail);
    let names: Vec<&str> = descriptors
        .iter()
        .filter(|d| d.kind != DescKind::Dropped)
        .map(|d| d.name.as_str())
        .collect();
    if names.is_empty() {
        return None;
    }
    // Signature-free qname: descriptors joined, NO package prefix (C++ qnames
    // are not crate-rooted).
    let qname = names.join("::");
    // Overload key: the raw descriptor tail carries the method disambiguator, so
    // it is identical across def sites of one overload and distinct between
    // overloads of the same qname.
    let overload_key = tail.to_string();
    Some(MappedSymbol {
        qname,
        overload_key,
        package,
        version,
    })
}

/// Classify a scip-clang symbol. A symbol in the `overridable` set (built from
/// SCIP `override` relationships) is a [`SymClass::VirtualMember`]; everything
/// else — free functions, non-virtual members, resolved overloads, template
/// instantiations — is [`SymClass::FreeOrInherent`].
pub fn classify(symbol: &str, overridable: &BTreeSet<String>) -> SymClass {
    if !is_local(symbol) && overridable.contains(symbol) {
        SymClass::VirtualMember
    } else {
        SymClass::FreeOrInherent
    }
}

/// Split a scip-clang symbol into `(package, version, descriptor-tail)`.
///
/// The SCIP prefix is `<scheme> <manager> <package-name> <version>`; the fifth
/// token onward is the descriptor tail. A placeholder `.` package/version is
/// normalized to the empty string (a project-local symbol). Unlike the Rust
/// tokenizer, the package is NOT prepended to the qname.
fn tokenize(symbol: &str) -> Option<(String, String, &str)> {
    let mut parts = symbol.splitn(5, ' ');
    let _scheme = parts.next()?;
    let _manager = parts.next()?;
    let package = parts.next()?;
    let version = parts.next()?;
    let tail = parts.next().unwrap_or("");
    if tail.is_empty() {
        return None;
    }
    let norm = |s: &str| {
        if s == PLACEHOLDER {
            String::new()
        } else {
            s.to_string()
        }
    };
    Some((norm(package), norm(version), tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overridable(syms: &[&str]) -> BTreeSet<String> {
        syms.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn free_function_has_no_package_prefix() {
        let m = map_symbol("cxx . . . parse().").expect("maps");
        assert_eq!(m.qname, "parse");
        assert_eq!(m.package, "", "placeholder package normalizes to empty");
        assert_eq!(m.version, "");
    }

    #[test]
    fn namespaced_function_joins_descriptors() {
        let m = map_symbol("cxx . . . ns/inner/f().").expect("maps");
        assert_eq!(m.qname, "ns::inner::f");
    }

    #[test]
    fn member_function_qname_is_signature_free() {
        let m = map_symbol("cxx . . . ns/Widget#draw().").expect("maps");
        assert_eq!(m.qname, "ns::Widget::draw");
    }

    #[test]
    fn overloads_share_qname_but_differ_in_overload_key() {
        // The two overloads of `ns::f` differ only by the method disambiguator.
        let a = map_symbol("cxx . . . ns/f(e9a1).").expect("maps");
        let b = map_symbol("cxx . . . ns/f(7c3d).").expect("maps");
        assert_eq!(a.qname, b.qname, "signature-free qname collapses overloads");
        assert_eq!(a.qname, "ns::f");
        assert_ne!(
            a.overload_key, b.overload_key,
            "the disambiguator keeps the two overloads distinct"
        );
    }

    #[test]
    fn same_overload_across_tus_shares_overload_key() {
        // Header declaration and .cpp definition carry an identical symbol string
        // → identical overload key → a single logical definition, not a collision.
        let decl = map_symbol("cxx . . . ns/f(e9a1).").expect("maps");
        let def = map_symbol("cxx . . . ns/f(e9a1).").expect("maps");
        assert_eq!(decl.overload_key, def.overload_key);
    }

    #[test]
    fn dependency_symbol_carries_package_coordinate() {
        let m = map_symbol("cxx cargo abseil-cpp 4ffaea74 absl/StrCat().").expect("maps");
        assert_eq!(m.qname, "absl::StrCat");
        assert_eq!(m.package, "abseil-cpp");
        assert_eq!(m.version, "4ffaea74");
    }

    #[test]
    fn local_symbol_is_none() {
        assert!(map_symbol("local 3").is_none());
    }

    #[test]
    fn malformed_short_symbol_is_none() {
        assert!(map_symbol("cxx . . .").is_none());
        assert!(map_symbol("").is_none());
    }

    #[test]
    fn classify_virtual_vs_free() {
        let virt = "cxx . . . ns/Base#area().";
        let free = "cxx . . . ns/util/clamp().";
        let set = overridable(&[virt]);
        assert_eq!(classify(virt, &set), SymClass::VirtualMember);
        assert_eq!(classify(free, &set), SymClass::FreeOrInherent);
        assert_eq!(classify("local 1", &set), SymClass::FreeOrInherent);
    }
}
