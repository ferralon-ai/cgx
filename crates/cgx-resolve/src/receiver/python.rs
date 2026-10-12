//! Python receiver classifier: syntax plus the class lattice.
//!
//! | receiver | targets | rule |
//! |---|---|---|
//! | `self.m()` / `cls.m()` in a method of `C` | `⋃ lookup(S, m), S ∈ cone(C)` | `recv-self` |
//! | `C.m()`, `pkg.C.m()` with `C` a resolved in-repo class | `lookup(C, m)` | `recv-class` |
//! | `super().m()` in `C` | next in MRO over every runtime class in `cone(C)` | `recv-super` |
//! | `"lit".m()`, `[...].m()`, a number | a builtin type: no in-repo target | external |
//! | `mod.f()`, `mod` bound only to out-of-repo modules | no in-repo target | external |
//! | `C(...).m()` with `C` a resolved in-repo class | `lookup(C, m)` | `recv-ctor` |
//! | `self.f.m()`, `f` an instance field of known type | lookup on the field's type state | `recv-field` |
//! | `x.m()`, `x` a local or parameter of known type | lookup on its type state | `recv-local` |
//!
//! Everything else stays on the same-name path. A name the callable or an
//! enclosing callable binds (parameter or local) shadows any import or class
//! of that name, and a rebound `self`/`cls` is not the enclosing class's
//! instance. A typed receiver of an out-of-repo class with no in-repo
//! subclass defining `m` is proven external.

use std::rc::Rc;

use cgx_core::node::SymbolKind;
use cgx_frontend::facts::{AnonRoot, LiteralKind, ValueSource};

use super::lang;
use super::lookup::Lookup;
use super::modules::Resolved;
use super::stats::Cat;
use super::types::{self, TypeSet};
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
            AnonRoot::Call(callee) => {
                let ts = constructed(ix, site.file_idx, caller, callee);
                if ts.top || ts.types.is_empty() {
                    return Legacy(Unclassified);
                }
                Classified::Lookup {
                    cat: Cat::CtorCall,
                    rule: "recv-ctor",
                    virtual_dispatch: false,
                    lookup: Rc::new(types::typed_lookup(lattice, &ts, m)),
                }
            }
            AnonRoot::Subscript | AnonRoot::Other => Legacy(Unclassified),
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
            if !is_receiver || lattice.is_meta(c) {
                return Legacy(Unclassified);
            }
            if path.len() == 2 {
                return Classified::Lookup {
                    cat: Cat::SelfCone,
                    rule: "recv-self",
                    virtual_dispatch: true,
                    lookup: lattice.cone_lookup(c, m),
                };
            }
            if path.len() == 3 && head == "self" {
                if let Some(ts) = ix.types.fields.field_types(ix, c, &path[1]) {
                    if ts.known() {
                        return Classified::Lookup {
                            cat: Cat::SelfField,
                            rule: "recv-field",
                            virtual_dispatch: true,
                            lookup: Rc::new(types::typed_lookup(lattice, &ts, m)),
                        };
                    }
                }
            }
            return Legacy(Unclassified);
        }
    }
    // A local shadows imports and module classes, unless its only binding is
    // a class defined in that function; a typed one is narrowed by its type.
    if let Some(o) = owner {
        let class_def = lattice.ids().contains_key(&format!("{o}::{head}"));
        if !class_def || !ix.locals.sole_binding(o, head) {
            return match ix.types.flow.get(o, head) {
                Some(ts) if path.len() == 2 && ts.known() => Classified::Lookup {
                    cat: Cat::TypedLocal,
                    rule: "recv-local",
                    virtual_dispatch: true,
                    lookup: Rc::new(types::typed_lookup(lattice, ts, m)),
                },
                _ => Legacy(Unclassified),
            };
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

/// Where one Python binding's value comes from, after resolution.
pub(crate) enum Src<'a> {
    Set(TypeSet),
    Copy(&'a str),
}

/// The Python value-source mapping: annotations are cones, constructor calls
/// and builtin literals are exact, a copy is an edge, everything else `top`.
pub(crate) fn source<'a>(
    ix: &ReceiverIndex<'a>,
    file: usize,
    func: &'a str,
    src: &'a ValueSource,
) -> Src<'a> {
    Src::Set(match src {
        ValueSource::Var(v) => return Src::Copy(v),
        ValueSource::New(t) => types::resolve_type(ix, file, func, t, false),
        ValueSource::Declared(t) => types::resolve_type(ix, file, func, t, true),
        ValueSource::Call(path) => constructed(ix, file, func, path),
        ValueSource::Literal(k) => TypeSet {
            ext: literal_types(*k)
                .iter()
                .map(|s| (s.to_string(), false))
                .collect(),
            ..TypeSet::default()
        },
        ValueSource::Null => TypeSet::default(),
        ValueSource::Enter(_) | ValueSource::Element(_) | ValueSource::Opaque => TypeSet::top(),
    })
}

/// The result of calling `path` in `func`: an instance of exactly an in-repo
/// class, of a builtin class, or `top` (a function result, a callee bound by a
/// local, or a class whose MRO has an in-repo `__new__`, which can return an
/// instance of any class).
pub(crate) fn constructed(
    ix: &ReceiverIndex<'_>,
    file: usize,
    func: &str,
    path: &[String],
) -> TypeSet {
    let Some(head) = path.first() else {
        return TypeSet::top();
    };
    if ix.locals.owner(ix.table, func, head).is_some() {
        return TypeSet::top();
    }
    match ix
        .modules
        .resolve_name_in_file(&ix.defs(), file, func, path)
    {
        Resolved::Class(ids) => {
            // An in-repo `__new__` can return an instance of any class.
            let mut ts = TypeSet::default();
            for c in ids {
                if ix.lattice.mro_may_define(c, "__new__") {
                    return TypeSet::top();
                }
                ts.join(&TypeSet::class(c, false));
            }
            ts
        }
        Resolved::External if path.len() == 1 && !matches!(head.as_str(), "type" | "super") => {
            match lang::rules(ix.modules.lang(file)) {
                Some(r) if r.builtin_types.contains(&head.as_str()) => {
                    types::external(ix, file, path, false)
                }
                _ => TypeSet::top(),
            }
        }
        _ => TypeSet::top(),
    }
}

fn literal_types(k: LiteralKind) -> &'static [&'static str] {
    match k {
        LiteralKind::Str => &["str"],
        LiteralKind::Bytes => &["bytes"],
        LiteralKind::Num => &["int", "float", "complex"],
        LiteralKind::Bool => &["bool"],
        LiteralKind::List => &["list"],
        LiteralKind::Dict => &["dict"],
        LiteralKind::Set => &["set"],
        LiteralKind::Tuple => &["tuple"],
    }
}
