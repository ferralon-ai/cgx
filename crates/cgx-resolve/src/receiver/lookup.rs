//! Method lookups over the [`Lattice`].
//!
//! `lookup(C, m)` walks `C`'s bases and stops at the nearest definition **per
//! branch**, so a diamond yields every branch's nearest definition: a superset
//! of the MRO answer. A branch that ends at an out-of-repo base marks the
//! lookup `open`. It is `unknown` when a branch reaches a base nobody can
//! resolve, a class holds a non-method attribute of that name (an instance
//! field, a class-level assignment, a store through a name typed as the
//! class), the lookup finds nothing and the name is stored through a base
//! of unknown type somewhere, or attribute access is customised
//! (`__getattribute__`, or `__getattr__` when nothing defines `m`).
//!
//! Cone and `super()` lookups also take the floating closure (classes with an
//! unresolvable base and their subclasses) into account: any of them may be a
//! runtime subclass of the cone's root. For a cone, a floating class's own `m`
//! is a target, an attribute `m` makes the lookup `unknown`, and otherwise its
//! resolved non-floating bases are looked up. For `super()`, only the bases of
//! floating classes outside the cone count (a subclass is never after the
//! class in its MRO): non-floating ones are looked up, floating ones
//! contribute their own `m`. The unresolvable bases themselves are
//! not followed, and neither contributes `open`: where a floating class's
//! runtime MRO places a class relative to the root is not modelled.

use std::collections::BTreeSet;
use std::rc::Rc;

use super::lattice::{Base, Lattice};

/// A lookup result: classes whose own definition of the method is a target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Lookup {
    pub targets: BTreeSet<u32>,
    pub open: bool,
    pub unknown: bool,
}

impl Lookup {
    pub(crate) fn merge(&mut self, r: &Lookup) {
        self.targets.extend(r.targets.iter().copied());
        self.open |= r.open;
        self.unknown |= r.unknown;
    }
}

const GETATTR: &str = "__getattr__";
const GETATTRIBUTE: &str = "__getattribute__";

impl Lattice {
    fn mid(&self, method: &str) -> u32 {
        if let Some(&id) = self.method_ids.borrow().get(method) {
            return id;
        }
        let mut ids = self.method_ids.borrow_mut();
        let id = ids.len() as u32;
        ids.insert(method.to_owned(), id);
        id
    }

    pub(crate) fn lookup(&self, class: u32, method: &str) -> Rc<Lookup> {
        self.lookup_id(class, self.mid(method), method)
    }

    fn lookup_id(&self, class: u32, mid: u32, method: &str) -> Rc<Lookup> {
        let key = (class, mid);
        if let Some(hit) = self.memo.borrow().get(&key) {
            return hit.clone();
        }
        // A cycle (malformed or merged lattice) reaching this entry again
        // learns nothing it can trust.
        let seed = Lookup {
            unknown: true,
            ..Lookup::default()
        };
        self.memo.borrow_mut().insert(key, Rc::new(seed));
        let c = class as usize;
        let mut out = Lookup::default();
        if self.attrs[c].contains(method) {
            out.unknown = true;
        } else if self.own[c].contains(method) {
            out.targets.insert(class);
        } else {
            out.open = self.rootless[c] && self.rules[c].implicit_root_methods.contains(&method);
            for b in &self.bases[c] {
                self.visit(b, mid, method, None, &mut out);
            }
        }
        // `__getattribute__` intercepts every access; `__getattr__` only a
        // miss. (A `__getattr__` lookup that is merely unknown crossed the same
        // bases as this miss, which is then unknown already.)
        if method != GETATTR && method != GETATTRIBUTE {
            let intercepted = !self.lookup(class, GETATTRIBUTE).targets.is_empty() || {
                let r = self.lookup(class, GETATTR);
                out.targets.is_empty() && (!r.targets.is_empty() || r.unknown)
            };
            if intercepted {
                out.unknown = true;
            }
        }
        // A name stored through a base of unknown type may be an instance
        // attribute of this class: a miss is not proof of an out-of-repo
        // method. (Shadowing a method that is found is excluded by the
        // assumption that methods are not assigned from outside the family.)
        if out.targets.is_empty() && self.foreign_attrs.contains(method) {
            out.unknown = true;
        }
        let out = Rc::new(out);
        self.memo.borrow_mut().insert(key, out.clone());
        out
    }

    /// Fold one base into `out`, skipping classes in `exclude`.
    fn visit(
        &self,
        b: &Base,
        mid: u32,
        method: &str,
        exclude: Option<&BTreeSet<u32>>,
        out: &mut Lookup,
    ) {
        match b {
            Base::Open => out.open = true,
            Base::Unknown => out.unknown = true,
            Base::Classes(cs) => {
                for &s in cs.iter().filter(|s| exclude.is_none_or(|x| !x.contains(s))) {
                    out.merge(&self.lookup_id(s, mid, method));
                }
            }
        }
    }

    /// `class` and every in-repo subclass, transitively.
    pub(crate) fn cone(&self, class: u32) -> Rc<BTreeSet<u32>> {
        if let Some(c) = self.cones.borrow().get(&class) {
            return c.clone();
        }
        let mut seen = BTreeSet::new();
        let mut stack = vec![class];
        while let Some(c) = stack.pop() {
            if seen.insert(c) {
                stack.extend(self.subclasses[c as usize].iter().copied());
            }
        }
        let rc = Rc::new(seen);
        self.cones.borrow_mut().insert(class, rc.clone());
        rc
    }

    /// The floating closure's contribution to a lookup of `method`.
    /// `exclude` (a `super()` cone) drops the classes themselves as targets and
    /// as bases to follow.
    fn floating_part(&self, mid: u32, method: &str, exclude: Option<&BTreeSet<u32>>) -> Lookup {
        let mut out = Lookup::default();
        let skip = |c: &u32| exclude.is_some_and(|x| x.contains(c));
        for &f in self.floating.iter().filter(|f| !skip(f)) {
            let c = f as usize;
            if exclude.is_none() {
                if self.attrs[c].contains(method) {
                    out.unknown = true;
                    continue;
                }
                if self.own[c].contains(method) {
                    out.targets.insert(f);
                    continue;
                }
            }
            for b in &self.bases[c] {
                let Base::Classes(cs) = b else { continue };
                for &s in cs.iter().filter(|s| !skip(s)) {
                    if !self.floating.contains(&s) {
                        let r = self.lookup_id(s, mid, method);
                        out.targets.extend(r.targets.iter().copied());
                        out.unknown |= r.unknown;
                    } else if exclude.is_some() {
                        // A floating sibling base can follow the class in the
                        // MRO: its own `m` counts; its bases are not followed.
                        let s = s as usize;
                        if self.attrs[s].contains(method) {
                            out.unknown = true;
                        } else if self.own[s].contains(method) {
                            out.targets.insert(s as u32);
                        }
                    }
                }
            }
        }
        out
    }

    /// Whether `class` or an in-repo class in its MRO defines `name` (a method
    /// or an attribute of its family), or an ancestor is unresolvable. Names
    /// stored on unknown objects from outside the class family do not count:
    /// methods are assumed not to be assigned from outside the family.
    pub(crate) fn mro_may_define(&self, class: u32, name: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut stack = vec![class];
        while let Some(c) = stack.pop() {
            if !seen.insert(c) {
                continue;
            }
            let i = c as usize;
            if self.own[i].contains(name) || self.attrs[i].contains(name) {
                return true;
            }
            for b in &self.bases[i] {
                match b {
                    Base::Classes(cs) => stack.extend(cs.iter().copied()),
                    Base::Unknown => return true,
                    Base::Open => {}
                }
            }
        }
        false
    }

    /// `m` on a value of an out-of-repo class or any of its subclasses: always
    /// open, with every in-repo class that has an out-of-repo base (any
    /// out-of-repo class may derive from any other), their cones, and the
    /// floating closure as the in-repo candidates.
    pub(crate) fn ext_cone_lookup(&self, method: &str) -> Rc<Lookup> {
        let mid = self.mid(method);
        if let Some(hit) = self.ext_memo.borrow().get(&mid) {
            return hit.clone();
        }
        let mut out = Lookup {
            open: true,
            ..Lookup::default()
        };
        for &c in &self.ext_any {
            let r = self.cone_lookup(c, method);
            out.targets.extend(r.targets.iter().copied());
            out.unknown |= r.unknown;
        }
        let floating = self.floating_part(mid, method, None);
        out.targets.extend(floating.targets);
        out.unknown |= floating.unknown;
        let out = Rc::new(out);
        self.ext_memo.borrow_mut().insert(mid, out.clone());
        out
    }

    /// `⋃ lookup(S, m), S ∈ cone(C)`, plus the floating closure. Open when some
    /// class in the cone has no in-repo `m` and an out-of-repo branch.
    pub(crate) fn cone_lookup(&self, class: u32, method: &str) -> Rc<Lookup> {
        let mid = self.mid(method);
        if let Some(hit) = self.cone_memo.borrow().get(&(class, mid)) {
            return hit.clone();
        }
        let mut out = Lookup::default();
        for &s in self.cone(class).iter() {
            let r = self.lookup_id(s, mid, method);
            out.open |= r.targets.is_empty() && r.open;
            out.unknown |= r.unknown;
            out.targets.extend(r.targets.iter().copied());
        }
        let floating = self.floating_part(mid, method, None);
        out.targets.extend(floating.targets);
        out.unknown |= floating.unknown;
        let out = Rc::new(out);
        self.cone_memo
            .borrow_mut()
            .insert((class, mid), out.clone());
        out
    }

    /// `super().m()` in `C`: for every runtime class `S ∈ cone(C)`, the methods
    /// after `C` in `S`'s MRO, over-approximated by `C`'s own bases plus every
    /// base of a proper subclass that lies outside `cone(C)` (cooperative
    /// multiple inheritance can route `super()` to a sibling mixin), plus the
    /// floating closure. `C` and its subclasses are never after `C` in a
    /// runtime MRO.
    pub(crate) fn super_lookup(&self, class: u32, method: &str) -> Rc<Lookup> {
        let mid = self.mid(method);
        if let Some(hit) = self.super_memo.borrow().get(&(class, mid)) {
            return hit.clone();
        }
        let cone = self.cone(class);
        let mut out = Lookup::default();
        for &s in cone.iter() {
            let c = s as usize;
            out.open |= self.rootless[c] && self.rules[c].implicit_root_methods.contains(&method);
            for b in &self.bases[c] {
                self.visit(b, mid, method, Some(&cone), &mut out);
            }
        }
        let floating = self.floating_part(mid, method, Some(&cone));
        out.targets.extend(floating.targets);
        out.unknown |= floating.unknown;
        let out = Rc::new(out);
        self.super_memo
            .borrow_mut()
            .insert((class, mid), out.clone());
        out
    }
}
