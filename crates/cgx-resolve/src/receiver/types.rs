//! Intraprocedural receiver typing: the type state of every local, parameter
//! and (Python) instance field, from the receiver-typing facts.
//!
//! Nodes are `(callable, name)`. Seeds come from annotations, constructor
//! calls, allocations, literals and Go method receivers; copy edges from
//! `v = w` (`go_vta` seeds the Go nodes). Nothing flows across calls. The fixpoint
//! is flow-insensitive and monotone: a node's set only grows, and `top`
//! (unknown) is sticky. Every binding the facts do not type is `top`, so a
//! name with any untyped binding is never narrowed.
//!
//! Type names resolve through [`ModuleIndex`] only. An out-of-repo class is
//! kept by name (`ext`), as exactly that class (a literal, a builtin
//! constructor) or as it and its in-repo subclasses (an annotation); a name
//! that resolves to nothing is `top`.

use std::collections::{BTreeMap, BTreeSet};

use cgx_frontend::facts::{ParamKind, TypeExpr, TypeFact};

use crate::input::FileInput;

use super::fields::Fields;
use super::lang;
use super::lattice::Lattice;
use super::lookup::Lookup;
use super::modules::Resolved;
use super::{go_vta, python, ReceiverIndex};

/// A receiver's type state: in-repo classes `(id, cone?)`, out-of-repo classes
/// by name (the value is that class or an in-repo subclass of it), or `top`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TypeSet {
    pub types: BTreeSet<(u32, bool)>,
    pub ext: BTreeSet<(String, bool)>,
    pub top: bool,
}

impl TypeSet {
    pub(crate) fn top() -> Self {
        TypeSet {
            top: true,
            ..TypeSet::default()
        }
    }

    pub(crate) fn class(c: u32, cone: bool) -> Self {
        TypeSet {
            types: BTreeSet::from([(c, cone)]),
            ..TypeSet::default()
        }
    }

    pub(crate) fn ext(name: &str, cone: bool) -> Self {
        TypeSet {
            ext: BTreeSet::from([(name.to_owned(), cone)]),
            ..TypeSet::default()
        }
    }

    /// Join `o` into `self`; whether `self` changed.
    pub(crate) fn join(&mut self, o: &TypeSet) -> bool {
        if self.top {
            return false;
        }
        if o.top {
            *self = TypeSet::top();
            return true;
        }
        let before = (self.types.len(), self.ext.len());
        self.types.extend(o.types.iter().copied());
        self.ext.extend(o.ext.iter().cloned());
        before != (self.types.len(), self.ext.len())
    }

    pub(crate) fn known(&self) -> bool {
        !self.top && (!self.types.is_empty() || !self.ext.is_empty())
    }
}

type Key<'a> = (&'a str, &'a str);

/// The propagation graph: one node per `(callable, name)`.
#[derive(Debug, Default)]
pub(crate) struct Flow<'a> {
    ids: BTreeMap<Key<'a>, usize>,
    state: Vec<TypeSet>,
    copies: Vec<(usize, usize)>,
}

impl<'a> Flow<'a> {
    pub(crate) fn node(&mut self, func: &'a str, var: &'a str) -> usize {
        let next = self.state.len();
        let id = *self.ids.entry((func, var)).or_insert(next);
        if id == next {
            self.state.push(TypeSet::default());
        }
        id
    }

    pub(crate) fn seed(&mut self, node: usize, ts: &TypeSet) {
        self.state[node].join(ts);
    }

    pub(crate) fn copy(&mut self, from: usize, to: usize) {
        if from != to {
            self.copies.push((from, to));
        }
    }

    /// Propagate along copy edges until nothing changes.
    fn solve(&mut self) {
        loop {
            let mut changed = false;
            for &(from, to) in &self.copies {
                let (src, dst) = if from < to {
                    let (a, b) = self.state.split_at_mut(to);
                    (&a[from], &mut b[0])
                } else {
                    let (a, b) = self.state.split_at_mut(from);
                    (&b[0], &mut a[to])
                };
                changed |= dst.join(src);
            }
            if !changed {
                break;
            }
        }
    }

    pub(crate) fn get(&self, func: &str, var: &str) -> Option<&TypeSet> {
        self.ids.get(&(func, var)).map(|&i| &self.state[i])
    }
}

/// Every typed local and parameter, and Python instance fields by class.
#[derive(Debug, Default)]
pub(crate) struct TypeEnv<'a> {
    pub(crate) flow: Flow<'a>,
    pub(crate) fields: Fields<'a>,
}

impl<'a> TypeEnv<'a> {
    pub(crate) fn build(ix: &ReceiverIndex<'a>, inputs: &'a [FileInput<'a>], trace: bool) -> Self {
        let mut env = TypeEnv::default();
        for (i, f) in inputs.iter().enumerate() {
            if f.lang != "python" || f.facts.module.is_none() {
                continue;
            }
            for tf in &f.facts.type_facts {
                match tf {
                    TypeFact::Param {
                        func,
                        name,
                        kind,
                        ty,
                        ..
                    } => {
                        let ts = match (kind, ty) {
                            (ParamKind::VarArgs | ParamKind::VarKeywords, _) | (_, None) => {
                                TypeSet::top()
                            }
                            (_, Some(t)) => resolve_type(ix, i, func, t, true),
                        };
                        let n = env.flow.node(func, name);
                        env.flow.seed(n, &ts);
                    }
                    TypeFact::Bind { func, var, src } => {
                        let n = env.flow.node(func, var);
                        match python::source(ix, i, func, src) {
                            python::Src::Set(ts) => env.flow.seed(n, &ts),
                            python::Src::Copy(v) => match ix.locals.owner(ix.table, func, v) {
                                Some(o) => {
                                    let from = env.flow.node(o, v);
                                    env.flow.copy(from, n);
                                }
                                None => env.flow.seed(n, &TypeSet::top()),
                            },
                        }
                    }
                    TypeFact::FieldBind { func, field, src } => {
                        env.fields.field_bind(ix, i, func, field, src);
                    }
                    TypeFact::AttrStore {
                        func,
                        base,
                        field,
                        src,
                    } => env.fields.attr_store(i, func, base, field, src),
                    _ => {}
                }
            }
        }
        go_vta::seed(ix, inputs, &mut env.flow);
        env.flow.solve();
        env.fields.finish(ix, inputs, &env.flow, trace);
        env
    }
}

/// The method targets of `m` on a receiver of type state `ts`. An
/// out-of-repo class makes the lookup open; when the value may be a subclass
/// of it, every in-repo class that can derive from an out-of-repo class is a
/// candidate ([`Lattice::ext_cone_lookup`]).
pub(crate) fn typed_lookup(lattice: &Lattice, ts: &TypeSet, m: &str) -> Lookup {
    let mut out = Lookup::default();
    let fold = |r: &Lookup, out: &mut Lookup| {
        out.open |= r.targets.is_empty() && r.open;
        out.unknown |= r.unknown;
        out.targets.extend(r.targets.iter().copied());
    };
    if !ts.ext.is_empty() {
        out.open = true;
    }
    if ts.ext.iter().any(|(_, cone)| *cone) {
        let r = lattice.ext_cone_lookup(m);
        out.unknown |= r.unknown;
        out.targets.extend(r.targets.iter().copied());
    }
    for &(c, cone) in &ts.types {
        let r = if cone {
            lattice.cone_lookup(c, m)
        } else {
            lattice.lookup(c, m)
        };
        fold(&r, &mut out);
    }
    out
}

/// A declared type as written in `file` inside `scope`. `cone` is whether the
/// value may be any subclass (an annotation) or is exactly the class.
pub(crate) fn resolve_type(
    ix: &ReceiverIndex<'_>,
    file: usize,
    scope: &str,
    ty: &TypeExpr,
    cone: bool,
) -> TypeSet {
    let path = match ty {
        TypeExpr::Named { path, .. } | TypeExpr::Generic { head: path, .. } => path,
        TypeExpr::Union(members) => {
            let mut ts = TypeSet::default();
            for m in members {
                ts.join(&resolve_type(ix, file, scope, m, cone));
            }
            return if ts.known() { ts } else { TypeSet::top() };
        }
        TypeExpr::Unknown => return TypeSet::top(),
    };
    match ix
        .modules
        .resolve_name_in_file(&ix.defs(), file, scope, path)
    {
        Resolved::Class(ids) => {
            let mut ts = TypeSet::default();
            for c in ids {
                if ix.lattice.is_structural(c) {
                    return TypeSet::top();
                }
                ts.join(&TypeSet::class(
                    c,
                    cone && ix.lattice.rules(c).class_lattice,
                ));
            }
            ts
        }
        Resolved::External => external(ix, file, path, cone),
        _ => TypeSet::top(),
    }
}

/// An out-of-repo class by name, or `top` when the name does not bound the
/// value's class.
pub(crate) fn external(
    ix: &ReceiverIndex<'_>,
    file: usize,
    path: &[String],
    cone: bool,
) -> TypeSet {
    let name = ix.modules.external_name(file, path);
    match lang::rules(ix.modules.lang(file)).and_then(|r| r.ext_name(name)) {
        Some(n) => TypeSet::ext(n, cone),
        None => TypeSet::top(),
    }
}
