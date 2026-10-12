//! Python receiver classifier: syntax plus the class lattice.
//!
//! | receiver | targets | rule |
//! |---|---|---|
//! | `self.m()` / `cls.m()` in a method of `C` | `⋃ lookup(S, m), S ∈ cone(C)` | `recv-self` |
//! | `C.m()`, `pkg.C.m()` with `C` a resolved in-repo class | `lookup(C, m)` | `recv-class` |
//! | `super().m()` in `C` | next in MRO over every runtime class in `cone(C)` | `recv-super` |
//! | `"lit".m()`, `[...].m()`, a number | a builtin type: no in-repo target | external |
//! | `mod.f()`, `mod` bound only to out-of-repo modules | no in-repo target | external |
//!
//! Everything else stays on the same-name path. A name the callable or an
//! enclosing callable binds (parameter or local) shadows any import or class
//! of that name, and a rebound `self`/`cls` is not the enclosing class's
//! instance.

use std::rc::Rc;

use cgx_core::node::SymbolKind;
use cgx_frontend::facts::AnonRoot;

use super::lookup::Lookup;
use super::modules::Resolved;
use super::stats::Cat;
use super::{LegacyReason, ReceiverIndex, Site};

/// The classifier's verdict, before targets are mapped to defs.
pub(crate) enum Classified {
    Lookup {
        cat: Cat,
        rule: &'static str,
        virtual_dispatch: bool,
        lookup: Rc<Lookup>,
    },
    External(Cat, &'static str),
    Legacy(LegacyReason),
}

use Classified::Legacy;
use LegacyReason::Unclassified;

pub(crate) fn classify(ix: &ReceiverIndex<'_>, site: &Site<'_>) -> Classified {
    let path = &site.raw.name_path;
    let m = site.method();
    let caller = site.caller.fqn.as_str();
    let lattice = &ix.lattice;

    if let Some(root) = ix.anon_root(site) {
        if path.len() != 1 {
            return Legacy(Unclassified);
        }
        return match root {
            AnonRoot::Literal(_) => Classified::External(Cat::Literal, "literal"),
            AnonRoot::Super => match lattice.enclosing_class(ix.table, caller) {
                Some(c) => Classified::Lookup {
                    cat: Cat::Super,
                    rule: "recv-super",
                    virtual_dispatch: true,
                    lookup: lattice.super_lookup(c, m),
                },
                None => Legacy(Unclassified),
            },
            AnonRoot::Call(_) | AnonRoot::Subscript | AnonRoot::Other => Legacy(Unclassified),
        };
    }
    if path.len() == 1 {
        return Legacy(Unclassified);
    }
    let head = path[0].as_str();

    let owner = ix.locals.owner(ix.table, caller, head);
    if head == "self" || head == "cls" {
        if let Some(c) = lattice.enclosing_class(ix.table, caller) {
            // `self` must be the parameter of a method defined directly in the
            // class (a closure sees it unchanged) and never rebound there. In a
            // metaclass, `self`/`cls` is a class, not an instance of the cone.
            let is_receiver = owner.is_some_and(|o| {
                o.rsplit_once("::").and_then(|(p, _)| lattice.ids().get(p)) == Some(&c)
                    && !ix.locals.rebinds(o, head)
            });
            if !is_receiver || path.len() != 2 || lattice.is_meta(c) {
                return Legacy(Unclassified);
            }
            return Classified::Lookup {
                cat: Cat::SelfCone,
                rule: "recv-self",
                virtual_dispatch: true,
                lookup: lattice.cone_lookup(c, m),
            };
        }
    }
    // A local shadows imports and module classes, unless its only binding is
    // a class defined in that function.
    if let Some(o) = owner {
        let class_def = lattice.ids().contains_key(&format!("{o}::{head}"));
        if !class_def || !ix.locals.sole_binding(o, head) {
            return Legacy(Unclassified);
        }
    }

    // A method body does not see its class body's names.
    let local_def = site.local_def.filter(|d| {
        let parent = d.fqn.rsplit_once("::").map_or("", |(p, _)| p);
        parent == caller || !lattice.ids().contains_key(parent)
    });
    let recv = &path[..path.len() - 1];
    let resolved = match local_def {
        Some(d) if d.kind != SymbolKind::Type => return Legacy(Unclassified),
        Some(d) if recv.len() == 1 => match lattice.ids().get(&d.fqn) {
            Some(&c) => Resolved::Class([c].into()),
            None => Resolved::NotFound,
        },
        _ => ix
            .modules
            .resolve_name_in_file(&ix.defs(), site.file_idx, caller, recv),
    };
    if let Resolved::Class(ids) = resolved {
        let mut lookup = Lookup::default();
        for c in ids {
            lookup.merge(&lattice.lookup(c, m));
        }
        return Classified::Lookup {
            cat: Cat::ClassHead,
            rule: "recv-class",
            virtual_dispatch: false,
            lookup: Rc::new(lookup),
        };
    }

    if local_def.is_none()
        && ix
            .modules
            .head_is_external_module(&ix.defs(), site.file_idx, head)
    {
        return Classified::External(Cat::ExternalModule, "external-module");
    }
    Legacy(Unclassified)
}

/// The receiver shape of a site left on the same-name path.
pub(crate) fn legacy_shape(ix: &ReceiverIndex<'_>, site: &Site<'_>) -> (Cat, &'static str) {
    let path = &site.raw.name_path;
    if path.len() == 1 || ix.anon_root(site).is_some() {
        (Cat::LegacyAnon, "legacy:anon")
    } else if path[0] == "self" || path[0] == "cls" {
        (Cat::LegacySelfChain, "legacy:self")
    } else if path.len() == 2 {
        (Cat::LegacyLocal, "legacy:local")
    } else {
        (Cat::LegacyChain, "legacy:chain")
    }
}
