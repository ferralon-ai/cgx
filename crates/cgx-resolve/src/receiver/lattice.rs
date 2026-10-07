//! The in-repo class lattice: classes, their declared bases (resolved through
//! [`ModuleIndex`]), subclasses, own methods and attributes. The lookups over
//! it live in `lookup.rs`.
//!
//! A class with a base nobody can resolve is *floating*: it is a runtime
//! subclass of something, possibly of any in-repo class, so it and its in-repo
//! subclasses (the floating closure) are candidates for every cone.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use cgx_core::node::SymbolKind;
use cgx_frontend::facts::{BaseExpr, TypeExpr, TypeFact};

use crate::input::FileInput;
use crate::symtab::SymbolTable;

use super::lang::{self, LangRules};
use super::locals::Locals;
use super::lookup::Lookup;
use super::modules::{Defs, ModuleIndex, Resolved};
use super::stats;

/// One declared base, resolved.
#[derive(Debug, Clone)]
pub(crate) enum Base {
    Classes(Vec<u32>),
    /// A named out-of-repo class, or a class produced by an out-of-repo call:
    /// its methods live outside the repo.
    Open,
    /// Nothing can be said about its methods or its ancestry.
    Unknown,
}

pub(super) type Memo = RefCell<HashMap<(u32, u32), Rc<Lookup>>>;

#[derive(Debug, Default)]
pub(crate) struct Lattice {
    pub(super) names: Vec<String>,
    pub(super) ids: BTreeMap<String, u32>,
    pub(super) rules: Vec<&'static LangRules>,
    /// Methods each class defines.
    pub(super) own: Vec<BTreeSet<String>>,
    /// Non-method attributes each class (or its instances) holds: class-level
    /// assignments, nested classes, and fields assigned through the receiver.
    pub(super) attrs: Vec<BTreeSet<String>>,
    pub(super) bases: Vec<Vec<Base>>,
    /// Some definition of the class declares no base: it inherits the
    /// language root's methods.
    pub(super) rootless: Vec<bool>,
    pub(super) subclasses: Vec<BTreeSet<u32>>,
    /// Classes whose transitive bases include a metaclass root (`type`):
    /// their `self`/`cls` is a class, not an instance of the cone.
    meta: Vec<bool>,
    /// The floating closure: classes with an unresolvable base, and their
    /// in-repo subclasses.
    pub(super) floating: BTreeSet<u32>,
    /// The def is abstract (a Go interface).
    is_abstract: Vec<bool>,
    /// The class declares `Protocol` as a base: values match it structurally.
    structural: Vec<bool>,
    /// In-repo classes with an out-of-repo base: the in-repo classes a value
    /// of any out-of-repo class can be (through any out-of-repo hierarchy).
    pub(super) ext_any: BTreeSet<u32>,
    pub(super) ext_memo: RefCell<HashMap<u32, Rc<Lookup>>>,
    /// Attribute names stored through a base of unknown type: possibly an
    /// instance attribute of any class, so a lookup that misses them is
    /// unknown.
    pub(super) foreign_attrs: BTreeSet<String>,
    pub(crate) fqn_collisions: usize,
    pub(crate) open_nonclass_bases: usize,
    pub(crate) unknown_bases: usize,
    pub(crate) floating_classes: usize,
    pub(super) method_ids: RefCell<HashMap<String, u32>>,
    pub(super) memo: Memo,
    pub(super) cone_memo: Memo,
    pub(super) super_memo: Memo,
    pub(super) cones: RefCell<HashMap<u32, Rc<BTreeSet<u32>>>>,
}

impl Lattice {
    /// Intern every class of a language with a lattice, in input order, and
    /// record each class's own methods and attributes. Same-FQN classes merge.
    pub(crate) fn intern(inputs: &[FileInput<'_>]) -> Self {
        let mut l = Lattice::default();
        for f in inputs {
            let Some(rules) = lang::rules(&f.lang) else {
                continue;
            };
            for d in f.facts.defs.iter().filter(|d| d.kind == SymbolKind::Type) {
                if l.ids.contains_key(&d.fqn) {
                    l.fqn_collisions += 1;
                    continue;
                }
                l.ids.insert(d.fqn.clone(), l.names.len() as u32);
                l.names.push(d.fqn.clone());
                l.rules.push(rules);
                l.is_abstract.push(d.is_abstract);
            }
        }
        let n = l.names.len();
        l.own = vec![BTreeSet::new(); n];
        l.attrs = vec![BTreeSet::new(); n];
        l.bases = vec![Vec::new(); n];
        l.rootless = vec![false; n];
        l.meta = vec![false; n];
        l.subclasses = vec![BTreeSet::new(); n];
        l.structural = vec![false; n];
        for f in inputs.iter().filter(|f| lang::rules(&f.lang).is_some()) {
            for d in &f.facts.defs {
                let Some((parent, m)) = d.fqn.rsplit_once("::") else {
                    continue;
                };
                let Some(&c) = l.ids.get(parent) else {
                    continue;
                };
                let set = match d.kind {
                    SymbolKind::Method | SymbolKind::Function => &mut l.own,
                    _ => &mut l.attrs,
                };
                set[c as usize].insert(m.into());
            }
            for tf in &f.facts.type_facts {
                if let TypeFact::FieldBind { func, field, .. } = tf {
                    if let Some(c) = l.owning_class(func) {
                        l.attrs[c as usize].insert(field.clone());
                    }
                }
            }
        }
        l
    }

    /// The innermost lattice class whose FQN is a proper prefix of `fqn`.
    fn owning_class(&self, fqn: &str) -> Option<u32> {
        let mut cur = fqn;
        loop {
            cur = cur.rsplit_once("::")?.0;
            if let Some(&c) = self.ids.get(cur) {
                return Some(c);
            }
        }
    }

    /// Record attribute stores seen outside the receiver: `typed` through a
    /// name typed as that class, `untyped` through anything else. Drops every
    /// memoized lookup, since stores turn misses into unknowns.
    pub(crate) fn add_attr_stores<'s>(
        &mut self,
        typed: impl IntoIterator<Item = (u32, &'s str)>,
        untyped: impl IntoIterator<Item = &'s str>,
    ) {
        for (c, f) in typed {
            self.attrs[c as usize].insert(f.to_owned());
        }
        self.foreign_attrs
            .extend(untyped.into_iter().map(str::to_owned));
        self.memo.borrow_mut().clear();
        self.cone_memo.borrow_mut().clear();
        self.super_memo.borrow_mut().clear();
        self.ext_memo.borrow_mut().clear();
    }

    pub(crate) fn ids(&self) -> &BTreeMap<String, u32> {
        &self.ids
    }

    pub(crate) fn name(&self, c: u32) -> &str {
        &self.names[c as usize]
    }

    pub(crate) fn is_meta(&self, c: u32) -> bool {
        self.meta[c as usize]
    }

    pub(crate) fn bases(&self, c: u32) -> &[Base] {
        &self.bases[c as usize]
    }

    pub(crate) fn rules(&self, c: u32) -> &'static LangRules {
        self.rules[c as usize]
    }

    /// Whether a value declared as `c` may be of a class outside `c`'s cone:
    /// a Go interface or a Python protocol.
    pub(crate) fn is_structural(&self, c: u32) -> bool {
        (self.is_abstract[c as usize] && !self.rules[c as usize].class_lattice)
            || self.structural[c as usize]
    }

    /// Resolve every class's declared bases (`ClassBases` facts), then derive
    /// subclasses, metaclasses and the floating closure. A class with no such
    /// fact has unknown bases.
    pub(crate) fn link_bases(
        &mut self,
        inputs: &[FileInput<'_>],
        modules: &ModuleIndex,
        table: &SymbolTable,
        locals: &Locals<'_>,
        trace: bool,
    ) {
        let mut resolved: Vec<(u32, Vec<Resolution>)> = Vec::new();
        {
            let d = Defs {
                class_ids: &self.ids,
                table,
            };
            for (i, f) in inputs.iter().enumerate() {
                for tf in &f.facts.type_facts {
                    let TypeFact::ClassBases { class, bases } = tf else {
                        continue;
                    };
                    let Some(&c) = self.ids.get(class) else {
                        continue;
                    };
                    let rules = self.rules[c as usize];
                    let scope = class.rsplit_once("::").map_or("", |(p, _)| p);
                    let bs: Vec<Resolution> = bases
                        .iter()
                        .filter(|b| !is_object(b))
                        .map(|b| resolve_base(&d, modules, locals, rules, i, scope, b))
                        .collect();
                    if trace {
                        for (b, r) in bases.iter().filter(|b| !is_object(b)).zip(&bs) {
                            if matches!(r.base, Base::Unknown) {
                                let tail = format!("{b:?}");
                                stats::trace(
                                    "base:unknown",
                                    &f.path,
                                    0,
                                    std::slice::from_ref(class),
                                    &tail,
                                );
                            }
                        }
                    }
                    resolved.push((c, bs));
                }
            }
        }
        let n = self.names.len();
        let mut seen = vec![false; n];
        let mut floating = BTreeSet::new();
        let mut meta_roots = Vec::new();
        for (c, bs) in resolved {
            seen[c as usize] = true;
            if bs.is_empty() {
                self.rootless[c as usize] = true;
            }
            for r in bs {
                self.open_nonclass_bases += usize::from(r.dynamic);
                if r.meta {
                    meta_roots.push(c);
                }
                if r.ext.as_deref() == Some("Protocol") {
                    self.structural[c as usize] = true;
                }
                if matches!(r.base, Base::Open) {
                    self.ext_any.insert(c);
                }
                match &r.base {
                    Base::Classes(cs) => {
                        for &s in cs {
                            self.subclasses[s as usize].insert(c);
                        }
                    }
                    Base::Open => {}
                    Base::Unknown => {
                        self.unknown_bases += 1;
                        floating.insert(c);
                    }
                }
                self.bases[c as usize].push(r.base);
            }
        }
        for (c, s) in seen.iter().enumerate() {
            if !s && self.rules[c].class_lattice {
                self.bases[c].push(Base::Unknown);
                floating.insert(c as u32);
            }
        }
        for c in meta_roots {
            for &s in self.cone(c).iter() {
                self.meta[s as usize] = true;
            }
        }
        for &f in &floating {
            let cone = self.cone(f);
            self.floating.extend(cone.iter().copied());
        }
        self.floating_classes = self.floating.len();
    }

    /// The lattice class whose method (possibly through nested functions)
    /// encloses `caller_fqn`.
    pub(crate) fn enclosing_class(&self, table: &SymbolTable, caller_fqn: &str) -> Option<u32> {
        let mut cur = caller_fqn;
        loop {
            let (parent, _) = cur.rsplit_once("::")?;
            let d = table.def_by_fqn(parent)?;
            match d.kind {
                SymbolKind::Type => return self.ids.get(parent).copied(),
                k if k.is_callable() => cur = parent,
                _ => return None,
            }
        }
    }
}

/// A resolved base and what it says about the class.
struct Resolution {
    base: Base,
    /// `Open` because a call outside the repo produced it.
    dynamic: bool,
    /// A named out-of-repo metaclass root (`type`, `ABCMeta`).
    meta: bool,
    /// The name of a named out-of-repo base class, as imported.
    ext: Option<String>,
}

fn is_object(b: &BaseExpr) -> bool {
    matches!(b, BaseExpr::Type(TypeExpr::Named { path, .. }) if path.len() == 1 && path[0] == "object")
}

/// A declared base. A nameable class resolves through the module index; a
/// named out-of-repo class, a value produced by an out-of-repo call, or an
/// out-of-repo factory call is `Open`; anything else (a name bound in an
/// enclosing function, a non-class value, an in-repo factory) is `Unknown`.
fn resolve_base(
    d: &Defs<'_>,
    modules: &ModuleIndex,
    locals: &Locals<'_>,
    rules: &LangRules,
    file: usize,
    scope: &str,
    b: &BaseExpr,
) -> Resolution {
    let res = |base, dynamic, meta| Resolution {
        base,
        dynamic,
        meta,
        ext: None,
    };
    let path = match b {
        BaseExpr::Type(TypeExpr::Named { path, .. } | TypeExpr::Generic { head: path, .. }) => path,
        BaseExpr::Call(path) => path,
        _ => return res(Base::Unknown, false, false),
    };
    // A name bound in an enclosing function is unknown, unless its only
    // binding is a class defined there.
    if let Some(o) = locals.owner(d.table, scope, &path[0]) {
        let class_def = d.class_ids.contains_key(&format!("{o}::{}", path[0]));
        if !class_def || !locals.sole_binding(o, &path[0]) {
            return res(Base::Unknown, false, false);
        }
    }
    let r = modules.resolve_name_in_file(d, file, scope, path);
    if let BaseExpr::Call(_) = b {
        return match r {
            Resolved::External | Resolved::Dynamic => res(Base::Open, true, false),
            _ => res(Base::Unknown, false, false),
        };
    }
    match r {
        Resolved::Class(ids) => res(Base::Classes(ids.into_iter().collect()), false, false),
        Resolved::External => {
            let last = path.last().map(String::as_str).unwrap_or("");
            Resolution {
                ext: Some(modules.external_name(file, path).to_owned()),
                ..res(Base::Open, false, rules.metaclass_roots.contains(&last))
            }
        }
        Resolved::Dynamic => res(Base::Open, true, false),
        Resolved::NonClass | Resolved::Module(_) | Resolved::NotFound => {
            res(Base::Unknown, false, false)
        }
    }
}
