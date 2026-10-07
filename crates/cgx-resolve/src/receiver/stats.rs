//! Receiver-narrowing counters, reported by `cgx index --receiver-narrowing`.

/// Counters for one receiver category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CatStats {
    /// Sites narrowed to an in-repo target set.
    pub sites: usize,
    /// Same-name method fan-out those sites would have had without narrowing.
    pub edges_before: usize,
    /// Edges the narrowed sites emitted.
    pub edges_after: usize,
    /// Sites proven to have no in-repo target: no edge, an `External` dangle.
    pub dangling: usize,
    /// Same-name fan-out of the dangling sites.
    pub dangling_edges_before: usize,
    /// Sites this category recognised but handed back to the same-name set.
    pub fallback: usize,
    /// Same-name fan-out the fallback sites kept.
    pub fallback_edges: usize,
}

/// Per-link receiver-narrowing counters. All zero when narrowing is off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrecisionStats {
    /// `self.m()` / `cls.m()` bounded by the enclosing class's cone.
    pub self_cone: CatStats,
    /// `C.m()` with `C` a resolved in-repo class.
    pub class_head: CatStats,
    /// `super().m()`.
    pub super_call: CatStats,
    /// Method calls on builtin literals.
    pub literal: CatStats,
    /// Attribute chains rooted at an out-of-repo module.
    pub external_module: CatStats,
    /// `x.m()` on a local or parameter whose type state is known.
    pub typed_local: CatStats,
    /// `C(...).m()` on a constructor call.
    pub ctor_call: CatStats,
    /// `self.f.m()` on an instance field whose type state is known.
    pub self_field: CatStats,
    /// Go `v.m()` on a closed set of types that flow into `v`.
    pub go_vta: CatStats,
    /// Sites handed back because their lookup crossed a base (or class
    /// attribute) whose methods cannot be known (`fallback` / `fallback_edges`).
    pub unknown_base: CatStats,
    /// Sites left on the same-name path, by receiver shape (`sites`;
    /// `edges_before` = `edges_after` = the same-name fan-out they keep).
    pub legacy_local: CatStats,
    pub legacy_self_chain: CatStats,
    pub legacy_chain: CatStats,
    pub legacy_anon: CatStats,
    /// Class bases bound to a non-class value or produced by an out-of-repo
    /// call: treated as an out-of-repo method provider.
    pub open_nonclass_bases: usize,
    /// Class bases that resolve to nothing nameable in the repo or builtins.
    pub unknown_bases: usize,
    /// Class FQNs defined more than once (merged into one lattice class).
    pub fqn_collisions: usize,
    /// Import specifiers that match several in-repo modules by suffix.
    pub spec_ambiguous: usize,
    /// Classes with an unresolvable base, and their in-repo subclasses: they
    /// join every cone and `super()` lookup that names one of their methods.
    pub floating_classes: usize,
}

impl PrecisionStats {
    /// Category rows for reporting, in a fixed order.
    pub fn rows(&self) -> [(&'static str, CatStats); 14] {
        [
            ("self-cone", self.self_cone),
            ("class-head", self.class_head),
            ("super", self.super_call),
            ("literal", self.literal),
            ("external-module", self.external_module),
            ("typed-local", self.typed_local),
            ("ctor-call", self.ctor_call),
            ("self-field", self.self_field),
            ("go-vta", self.go_vta),
            ("fallback:unknown-base", self.unknown_base),
            ("legacy:local", self.legacy_local),
            ("legacy:self-chain", self.legacy_self_chain),
            ("legacy:chain", self.legacy_chain),
            ("legacy:anon", self.legacy_anon),
        ]
    }

    /// Scalar counters for reporting, in a fixed order.
    pub fn counts(&self) -> [(&'static str, usize); 5] {
        [
            ("open:nonclass-base", self.open_nonclass_bases),
            ("unknown-base", self.unknown_bases),
            ("fqn-collisions", self.fqn_collisions),
            ("spec-ambiguous", self.spec_ambiguous),
            ("floating-classes", self.floating_classes),
        ]
    }

    pub(crate) fn cat(&mut self, cat: Cat) -> &mut CatStats {
        match cat {
            Cat::SelfCone => &mut self.self_cone,
            Cat::ClassHead => &mut self.class_head,
            Cat::Super => &mut self.super_call,
            Cat::Literal => &mut self.literal,
            Cat::ExternalModule => &mut self.external_module,
            Cat::TypedLocal => &mut self.typed_local,
            Cat::CtorCall => &mut self.ctor_call,
            Cat::SelfField => &mut self.self_field,
            Cat::GoVta => &mut self.go_vta,
            Cat::LegacyLocal => &mut self.legacy_local,
            Cat::LegacySelfChain => &mut self.legacy_self_chain,
            Cat::LegacyChain => &mut self.legacy_chain,
            Cat::LegacyAnon => &mut self.legacy_anon,
        }
    }
}

/// The receiver category a site was classified into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cat {
    SelfCone,
    ClassHead,
    Super,
    Literal,
    ExternalModule,
    TypedLocal,
    CtorCall,
    SelfField,
    GoVta,
    LegacyLocal,
    LegacySelfChain,
    LegacyChain,
    LegacyAnon,
}

/// One trace line (`LinkOpts::narrowing_trace`): tab-separated
/// `label, file:line, name path, fan-out -> kept, targets`.
pub(crate) fn trace(label: &str, file: &str, line: u32, path: &[String], tail: &str) {
    eprintln!("{label}\t{file}:{line}\t{}\t{tail}", path.join("."));
}
