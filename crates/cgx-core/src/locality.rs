//! Locality of a call candidate relative to its call site.
//!
//! The Tier-0 bare-name(+arity) fallback (`name-arity`) over-approximates a call
//! to every same-short-name def that survives the language/kind/arity filters. It
//! cannot narrow that set without risking a real edge (a facade method delegating
//! to a same-named free function in another module of the same crate is a genuine
//! call that a locality *cut* would drop). Instead the resolver keeps every
//! candidate and *orders* them nearest-first by this locality classification, so
//! the candidate `rank` and the denormalized `rule` carry the structure and a
//! downstream `--max-candidates` cut can trim from the far end. Ordering, not
//! cutting: no edge is ever dropped.

/// How near a resolved-call candidate is to its call site, in coarse structural
/// buckets derived from file identity and shared FQN prefix. Declaration order is
/// nearest-first, so the derived [`Ord`] doubles as the candidate rank key
/// ([`LocalityTier::SameFile`] sorts before [`LocalityTier::Global`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LocalityTier {
    /// The candidate is defined in the same file as the call site.
    SameFile,
    /// Same module: the two FQNs share ≥2 leading `::`-separated segments.
    SameModule,
    /// Same crate/top-level package: the two FQNs share exactly 1 leading segment.
    SameCrate,
    /// No shared leading segment — a bare-name collision with an unrelated def.
    Global,
}

impl LocalityTier {
    /// Stable lowercase tag for denormalized provenance (`name-arity:loc:<tag>`).
    pub fn as_str(self) -> &'static str {
        match self {
            LocalityTier::SameFile => "same-file",
            LocalityTier::SameModule => "same-module",
            LocalityTier::SameCrate => "same-crate",
            LocalityTier::Global => "global",
        }
    }
}

/// Classify a candidate's locality relative to a call site. `call_file`/`caller_fqn`
/// describe the call site; `cand_file`/`cand_fqn` the candidate definition.
///
/// - candidate defined in the same (non-empty) file → [`LocalityTier::SameFile`];
/// - otherwise by the count of shared leading `::`-separated FQN segments:
///   ≥2 → [`LocalityTier::SameModule`], ==1 → [`LocalityTier::SameCrate`],
///   0 → [`LocalityTier::Global`].
///
/// Shared-prefix — not module-parent equality — is deliberate. An FQN
/// `crate::m::Type::method` carries no marker separating the module path from an
/// enclosing type, so a co-located free fn `crate::m::f` and method
/// `crate::m::T::g` share the `crate::m` prefix (module tier) even though their
/// FQN *parents* (`crate::m` vs `crate::m::T`) differ. The function is pure and
/// total, so it is a deterministic ordering key (AR-10).
pub fn locality_tier(
    call_file: &str,
    caller_fqn: &str,
    cand_file: &str,
    cand_fqn: &str,
) -> LocalityTier {
    if !call_file.is_empty() && cand_file == call_file {
        return LocalityTier::SameFile;
    }
    let shared = caller_fqn
        .split("::")
        .zip(cand_fqn.split("::"))
        .take_while(|(a, b)| a == b)
        .count();
    match shared {
        0 => LocalityTier::Global,
        1 => LocalityTier::SameCrate,
        _ => LocalityTier::SameModule,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_wins_regardless_of_fqn() {
        // Same file beats any FQN relationship, even a global-looking one.
        assert_eq!(
            locality_tier("src/lib.rs", "crate::a::f", "src/lib.rs", "other::g"),
            LocalityTier::SameFile
        );
    }

    #[test]
    fn empty_call_file_never_matches_same_file() {
        // A missing call-site file must not collapse every empty-file def to same-file.
        assert_eq!(
            locality_tier("", "crate::a::f", "", "crate::a::g"),
            LocalityTier::SameModule
        );
    }

    #[test]
    fn co_located_free_fn_and_method_are_same_module() {
        // The case module-parent equality gets wrong: a free fn and a method in the
        // same module share the `crate::m` prefix (2 segments) though their FQN
        // parents differ. Different files so the file check does not short-circuit.
        assert_eq!(
            locality_tier("src/a.rs", "crate::m::T::g", "src/b.rs", "crate::m::f"),
            LocalityTier::SameModule
        );
    }

    #[test]
    fn shared_crate_only_is_same_crate() {
        // The `map_symbol` shape: `crate::ScipResolver::map_symbol` (call site) and
        // `crate::symbol::map_symbol` (candidate) share only the crate segment.
        assert_eq!(
            locality_tier(
                "src/lib.rs",
                "crate::ScipResolver::map_symbol",
                "src/symbol.rs",
                "crate::symbol::map_symbol"
            ),
            LocalityTier::SameCrate
        );
    }

    #[test]
    fn no_shared_prefix_is_global() {
        assert_eq!(
            locality_tier("src/a.rs", "acrate::x::f", "src/b.rs", "bcrate::y::f"),
            LocalityTier::Global
        );
    }

    #[test]
    fn nearest_first_ordering() {
        // The derived Ord must rank nearest-first so it can key the candidate sort.
        assert!(LocalityTier::SameFile < LocalityTier::SameModule);
        assert!(LocalityTier::SameModule < LocalityTier::SameCrate);
        assert!(LocalityTier::SameCrate < LocalityTier::Global);
    }
}
