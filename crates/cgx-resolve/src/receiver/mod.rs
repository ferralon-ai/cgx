//! Receiver-kind narrowing of virtual call sites (opt-in via
//! [`LinkOpts::receiver_narrowing`](crate::LinkOpts::receiver_narrowing)).
//!
//! Without it, the link pass binds `x.m()` to every same-language method named
//! `m` in the repository (`rule = "name-method"`). This module first asks what
//! kind of value the receiver is. Where the syntax, the receiver-typing facts
//! and the in-repo class lattice answer that, it replaces the global set with a
//! receiver-scoped one (`recv-*` rules), or proves the receiver out of repo and
//! leaves the site with no edge under [`CutMarker::External`].
//!
//! Honesty rules:
//! - A site dangles only on proof: a literal receiver, a head bound only to
//!   out-of-repo modules, or a lookup that crossed only out-of-repo bases and
//!   found no in-repo method.
//! - A lookup that crossed a base nobody can resolve keeps the same-name set.
//! - A closed lookup that finds nothing (an attribute holding a callable)
//!   keeps the same-name set.
//! - A language whose frontend emits no receiver-typing facts or module
//!   identity is never narrowed.
//! - A name with any binding the facts do not type is never narrowed by type,
//!   and nothing is typed across calls.
//!
//! Assumptions: `self`/`cls` is an instance or subclass of the enclosing
//! class; annotations are truthful; methods are not assigned from outside the
//! class family, and attributes are not set dynamically (`setattr`,
//! `__dict__`); a constructor call of a class without an in-repo `__new__`
//! returns an instance of exactly that class (with one, it is untyped); a base
//! bound to a non-class value or produced by an out-of-repo call is a
//! dynamically created class whose methods live outside the repo.
//!
//! [`CutMarker::External`]: cgx_core::cut::CutMarker::External

mod fields;
mod go_vta;
mod imports;
mod lang;
mod lattice;
mod ledger;
mod locals;
mod lookup;
mod modules;
mod python;
mod stats;
mod types;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use cgx_core::id::NodeId;
use cgx_frontend::facts::{AnonRoot, RawRef, SymbolDef};

use crate::input::FileInput;
use crate::symtab::{DefEntry, SymbolTable};

pub(crate) use ledger::Ledger;
pub use stats::{CatStats, PrecisionStats};

use lattice::Lattice;
use locals::Locals;
use lookup::Lookup;
use modules::{Defs, ModuleIndex};
use python::Classified;
use types::TypeEnv;

/// One virtual call site, as the link pass sees it.
pub(crate) struct Site<'s> {
    pub file: &'s FileInput<'s>,
    /// The file's position in the link inputs.
    pub file_idx: usize,
    pub raw: &'s RawRef,
    pub caller: &'s SymbolDef,
    pub caller_id: NodeId,
    /// The same-file def the receiver head names lexically, if any.
    pub local_def: Option<&'s SymbolDef>,
}

impl Site<'_> {
    pub(crate) fn method(&self) -> &str {
        self.raw.name_path.last().map(String::as_str).unwrap_or("")
    }
}

/// Why a site keeps the same-name candidate set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegacyReason {
    /// No receiver kind applies.
    Unclassified,
    /// The receiver kind applies but its lookup is closed and found nothing.
    ClosedFallback,
    /// The lookup crossed a base (or class attribute) of unknown methods.
    UnknownBase,
    /// The language is not narrowed.
    Unsupported,
}

/// The rule's kind of evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Basis {
    Typed,
}

/// A site's outcome.
pub(crate) enum Narrowed<'t> {
    Typed {
        hits: Vec<&'t DefEntry>,
        rule: &'static str,
        virtual_dispatch: bool,
        #[allow(dead_code)]
        basis: Basis,
    },
    /// Proven out of repo: no edge, a `CutMarker::External` dangle.
    External,
    Legacy(#[allow(dead_code)] LegacyReason),
}

/// `(lang, method)` → same-language method defs named `method`, by owner type.
type HitsMemo<'a> = HashMap<(String, String), BTreeMap<String, Vec<&'a DefEntry>>>;

pub(crate) struct ReceiverIndex<'a> {
    table: &'a SymbolTable,
    modules: ModuleIndex,
    lattice: Lattice,
    locals: Locals<'a>,
    types: TypeEnv<'a>,
    hits_memo: RefCell<HitsMemo<'a>>,
}

impl<'a> ReceiverIndex<'a> {
    pub(crate) fn build(
        inputs: &'a [FileInput<'a>],
        table: &'a SymbolTable,
        ledger: &mut Ledger,
    ) -> Self {
        let modules = ModuleIndex::build(inputs);
        let locals = Locals::build(inputs);
        let mut lattice = Lattice::intern(inputs);
        lattice.link_bases(inputs, &modules, table, &locals, ledger.tracing());
        let mut ix = ReceiverIndex {
            table,
            modules,
            lattice,
            locals,
            types: TypeEnv::default(),
            hits_memo: RefCell::default(),
        };
        let types = TypeEnv::build(&ix, inputs, ledger.tracing());
        let (typed, untyped) = types.fields.attr_stores();
        ix.lattice.add_attr_stores(typed, untyped);
        ix.types = types;
        let s = ledger.stats_mut();
        s.fqn_collisions = ix.lattice.fqn_collisions;
        s.open_nonclass_bases = ix.lattice.open_nonclass_bases;
        s.unknown_bases = ix.lattice.unknown_bases;
        s.spec_ambiguous = ix.modules.spec_ambiguous;
        s.floating_classes = ix.lattice.floating_classes;
        ix
    }

    fn defs(&self) -> Defs<'_> {
        Defs {
            class_ids: self.lattice.ids(),
            table: self.table,
        }
    }

    fn anon_root(&self, site: &Site<'_>) -> Option<&'a AnonRoot> {
        let raw = site.raw;
        let depth = raw.name_path.len().min(255) as u8;
        let key = (
            site.caller.fqn.as_str(),
            raw.span.line,
            raw.span.col.unwrap_or(0),
            site.method(),
            depth,
        );
        self.locals.anon(&key)
    }

    fn supported(site: &Site<'_>) -> bool {
        matches!(site.file.lang.as_str(), "python" | "go") && site.file.facts.module.is_some()
    }

    /// Classify a virtual call site and narrow it when its receiver kind is
    /// decidable (link Step 1b).
    pub(crate) fn narrow(&self, site: &Site<'_>, ledger: &mut Ledger) -> Narrowed<'a> {
        if !Self::supported(site) {
            return Narrowed::Legacy(LegacyReason::Unsupported);
        }
        if site.file.lang == "go" {
            return match go_vta::classify(self, site) {
                Some(hits) => {
                    let virtual_dispatch = hits.len() > 1;
                    ledger.typed(self.table, site, stats::Cat::GoVta, "recv-vta", &hits);
                    Narrowed::Typed {
                        hits,
                        rule: "recv-vta",
                        virtual_dispatch,
                        basis: Basis::Typed,
                    }
                }
                None => Narrowed::Legacy(LegacyReason::Unclassified),
            };
        }
        match python::classify(self, site) {
            Classified::Lookup {
                cat,
                rule,
                virtual_dispatch,
                lookup,
            } => self.finish(site, ledger, cat, rule, virtual_dispatch, &lookup),
            Classified::External(cat, label) => {
                ledger.external(self.table, site, cat, label);
                Narrowed::External
            }
            Classified::Legacy(reason) => Narrowed::Legacy(reason),
        }
    }

    fn finish(
        &self,
        site: &Site<'_>,
        ledger: &mut Ledger,
        cat: stats::Cat,
        rule: &'static str,
        virtual_dispatch: bool,
        lookup: &Lookup,
    ) -> Narrowed<'a> {
        let reason = if lookup.unknown {
            LegacyReason::UnknownBase
        } else {
            let hits = self.hits_for(&lookup.targets, site.method(), &site.file.lang);
            if !hits.is_empty() {
                ledger.typed(self.table, site, cat, rule, &hits);
                return Narrowed::Typed {
                    hits,
                    rule,
                    virtual_dispatch,
                    basis: Basis::Typed,
                };
            }
            if lookup.open {
                ledger.external(self.table, site, cat, rule);
                return Narrowed::External;
            }
            LegacyReason::ClosedFallback
        };
        ledger.fallback(self.table, site, cat, reason);
        Narrowed::Legacy(reason)
    }

    /// Every same-language method def of `m` owned by each target class `C`
    /// (`C::m`; for Go also `C`'s pointer and generic receiver forms). Two
    /// defs can share an FQN; both are targets.
    fn hits_for(&self, classes: &BTreeSet<u32>, m: &str, lang: &str) -> Vec<&'a DefEntry> {
        let mut memo = self.hits_memo.borrow_mut();
        let by_owner = memo
            .entry((lang.to_owned(), m.to_owned()))
            .or_insert_with(|| {
                let mut by_owner: BTreeMap<String, Vec<&'a DefEntry>> = BTreeMap::new();
                for d in self
                    .table
                    .defs_by_short(m)
                    .iter()
                    .filter(|d| ledger::is_method_candidate(d, lang))
                {
                    if let Some(owner) = method_owner(&d.fqn, lang) {
                        by_owner.entry(owner).or_default().push(d);
                    }
                }
                by_owner
            });
        classes
            .iter()
            .flat_map(|&c| by_owner.get(self.lattice.name(c)))
            .flatten()
            .copied()
            .collect()
    }

    /// Record a supported-language site that reached the same-name step.
    pub(crate) fn record_legacy(&self, site: &Site<'_>, fanout: usize, ledger: &mut Ledger) {
        if Self::supported(site) {
            let (cat, label) = python::legacy_shape(self, site);
            ledger.legacy(site, cat, label, fanout);
        }
    }
}

/// The type a method def belongs to: its FQN's parent, with a Go receiver's
/// pointer and type-parameter forms (`pkg::(*T[K])::m`) reduced to `pkg::T`.
fn method_owner(fqn: &str, lang: &str) -> Option<String> {
    let (parent, _) = fqn.rsplit_once("::")?;
    if lang != "go" {
        return Some(parent.to_owned());
    }
    let (pkg, recv) = parent.rsplit_once("::")?;
    let recv = recv
        .strip_prefix("(*")
        .and_then(|r| r.strip_suffix(')'))
        .unwrap_or(recv);
    let recv = recv.split_once('[').map_or(recv, |(t, _)| t);
    Some(format!("{pkg}::{recv}"))
}
