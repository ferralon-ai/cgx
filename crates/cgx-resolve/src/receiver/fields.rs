//! Python instance-field type states, for `self.f.m()` receivers.
//!
//! A field's type state joins every store to it that can reach an instance of
//! the reading class: `self.f = …` in the class's cone and ancestry
//! (`FieldBind`), stores through a name typed as one of those classes
//! (`AttrStore` with a typed base), and stores in classes with an unresolvable
//! base, which may be unseen subclasses. A store through any other base (an
//! untyped name, a call result, an attribute chain, module-level code) could
//! reach any object, so it makes that field name untyped everywhere.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use cgx_frontend::facts::ValueSource;

use crate::input::FileInput;

use super::lattice::{Base, Lattice};
use super::python::{self, Src};
use super::stats;
use super::types::{Flow, TypeSet};
use super::ReceiverIndex;

type FieldKey<'a> = (u32, &'a str);
type FieldMemo = RefCell<HashMap<(u32, String), Option<Rc<TypeSet>>>>;

/// An `AttrStore` fact awaiting the base's type: `(file, func, base, field,
/// src)`.
type Pending<'a> = (usize, &'a str, &'a ValueSource, &'a str, &'a ValueSource);

#[derive(Debug, Default)]
pub(crate) struct Fields<'a> {
    stores: BTreeMap<FieldKey<'a>, TypeSet>,
    /// Stores of a local's value, joined once the locals are solved.
    copies: Vec<(FieldKey<'a>, &'a str, &'a str)>,
    pending: Vec<Pending<'a>>,
    /// Field names stored through a base of unknown type.
    poisoned: BTreeSet<&'a str>,
    /// `(class, field)` stored through a name typed as that class.
    typed_stores: BTreeSet<FieldKey<'a>>,
    /// Classes that may be subclasses of any class: the lattice's floating
    /// closure, and their ancestors.
    floating: BTreeSet<u32>,
    memo: FieldMemo,
}

impl<'a> Fields<'a> {
    /// A `self.f = src` store in method `func`.
    pub(crate) fn field_bind(
        &mut self,
        ix: &ReceiverIndex<'a>,
        file: usize,
        func: &'a str,
        field: &'a str,
        src: &'a ValueSource,
    ) {
        let Some(c) = ix.lattice.enclosing_class(ix.table, func) else {
            return;
        };
        self.store(ix, file, func, (c, field), src);
    }

    fn store(
        &mut self,
        ix: &ReceiverIndex<'a>,
        file: usize,
        func: &'a str,
        key: FieldKey<'a>,
        src: &'a ValueSource,
    ) {
        let slot = self.stores.entry(key).or_default();
        match python::source(ix, file, func, src) {
            Src::Set(ts) => {
                slot.join(&ts);
            }
            Src::Copy(v) => match ix.locals.owner(ix.table, func, v) {
                Some(o) => self.copies.push((key, o, v)),
                None => {
                    slot.join(&TypeSet::top());
                }
            },
        }
    }

    /// A `base.f = src` store through something other than the receiver.
    pub(crate) fn attr_store(
        &mut self,
        file: usize,
        func: &'a str,
        base: &'a ValueSource,
        field: &'a str,
        src: &'a ValueSource,
    ) {
        self.pending.push((file, func, base, field, src));
    }

    /// Resolve what needs the solved locals: copies, typed and untyped
    /// attribute-store bases, and the floating set.
    pub(crate) fn finish(
        &mut self,
        ix: &ReceiverIndex<'a>,
        inputs: &[FileInput<'_>],
        flow: &Flow<'a>,
        trace: bool,
    ) {
        for (file, func, base, field, src) in std::mem::take(&mut self.pending) {
            let classes = match base {
                ValueSource::Var(v) => ix
                    .locals
                    .owner(ix.table, func, v)
                    .and_then(|o| flow.get(o, v))
                    .filter(|ts| !ts.top && !ts.types.is_empty() && ts.ext.is_empty())
                    .map(|ts| ts.types.iter().map(|&(c, _)| c).collect::<Vec<_>>()),
                _ => None,
            };
            match classes {
                Some(cs) => {
                    for c in cs {
                        self.typed_stores.insert((c, field));
                        self.store(ix, file, func, (c, field), src);
                    }
                }
                None => {
                    if self.poisoned.insert(field) && trace {
                        let path = std::slice::from_ref(&inputs[file].path);
                        stats::trace("field:poisoned", &inputs[file].path, 0, path, field);
                    }
                }
            }
        }
        for (key, o, v) in std::mem::take(&mut self.copies) {
            let ts = flow.get(o, v).cloned().unwrap_or_else(TypeSet::top);
            self.stores.entry(key).or_default().join(&ts);
        }
        self.floating = with_ancestors(&ix.lattice, ix.lattice.floating.clone()).0;
    }

    /// Attribute names stored outside the receiver: per class through a typed
    /// name, and anywhere through a name of unknown type.
    pub(crate) fn attr_stores(&self) -> (Vec<(u32, &'a str)>, Vec<&'a str>) {
        (
            self.typed_stores.iter().copied().collect(),
            self.poisoned.iter().copied().collect(),
        )
    }

    /// The type state of instance field `f` as seen from class `c`: joined
    /// over `c`'s cone and the ancestry of every class in it, plus the
    /// floating set's stores. `None` when nothing assigns it, when the family
    /// or a floating class defines `f` at class level (an attribute, method or
    /// property), when an ancestor is unresolvable, or when `f` is stored
    /// through a base of unknown type anywhere.
    pub(crate) fn field_types(
        &self,
        ix: &ReceiverIndex<'_>,
        c: u32,
        f: &str,
    ) -> Option<Rc<TypeSet>> {
        let key = (c, f.to_owned());
        if let Some(hit) = self.memo.borrow().get(&key) {
            return hit.clone();
        }
        let out = self.uncached(ix, c, f).map(Rc::new);
        self.memo.borrow_mut().insert(key, out.clone());
        out
    }

    fn uncached(&self, ix: &ReceiverIndex<'_>, c: u32, f: &str) -> Option<TypeSet> {
        if self.poisoned.contains(f) {
            return None;
        }
        let lattice = &ix.lattice;
        let (family, unknown) = with_ancestors(lattice, (*lattice.cone(c)).clone());
        if unknown {
            return None;
        }
        let owners = ix.table.defs_by_short(f).iter().filter_map(|d| {
            let (p, _) = d.fqn.rsplit_once("::")?;
            lattice.ids().get(p)
        });
        for o in owners {
            if family.contains(o) || self.floating.contains(o) {
                return None;
            }
        }
        let mut ts = TypeSet::default();
        let mut seen = false;
        for &x in family.iter().chain(&self.floating) {
            if let Some(s) = self.stores.get(&(x, f)) {
                seen = true;
                ts.join(s);
            }
        }
        seen.then_some(ts)
    }
}

/// `classes` and all their in-repo ancestors, and whether some ancestor is
/// unresolvable.
fn with_ancestors(lattice: &Lattice, mut classes: BTreeSet<u32>) -> (BTreeSet<u32>, bool) {
    let mut stack: Vec<u32> = classes.iter().copied().collect();
    let mut unknown = false;
    while let Some(x) = stack.pop() {
        for b in lattice.bases(x) {
            match b {
                Base::Classes(cs) => {
                    stack.extend(cs.iter().copied().filter(|&s| classes.insert(s)));
                }
                Base::Unknown => unknown = true,
                Base::Open => {}
            }
        }
    }
    (classes, unknown)
}
