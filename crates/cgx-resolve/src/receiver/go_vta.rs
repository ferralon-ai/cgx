//! Go VTA-lite: intraprocedural receiver typing for `v.m()`.
//!
//! Seeds and edges on the shared propagation graph ([`Flow`]):
//! - a method receiver has exactly its declared type (Go has no
//!   inheritance); `v := T{}`, `v := &T{}` and `var v T` (`T` not an
//!   interface) seed `v` with `T`;
//! - `v := w` is a copy edge;
//! - everything else is `top`: every function parameter, any untyped
//!   binding, and a local copied from a non-local.
//!
//! Parameters are never typed from call arguments: the facts do not record
//! every place a function value escapes (field stores, composite literals,
//! returns, package-level initializers, func-literal bodies), so no
//! function's caller set can be shown complete.
//!
//! A site `v.m()` whose head has a non-empty, closed set of in-repo types is
//! narrowed to `{ T.m | T ∈ types(v) }` (value or pointer receiver). If any
//! `T` lacks an in-repo `m` (embedding and promotion are not modelled) the
//! site keeps the same-name set.

use std::collections::BTreeSet;

use cgx_frontend::facts::{ParamKind, TypeFact, ValueSource};

use crate::input::FileInput;
use crate::symtab::DefEntry;

use super::types::{resolve_type, Flow, TypeSet};
use super::{ReceiverIndex, Site};

const GO: &str = "go";

/// Seed the Go nodes and edges of `flow`.
pub(crate) fn seed<'a>(ix: &ReceiverIndex<'a>, inputs: &'a [FileInput<'a>], flow: &mut Flow<'a>) {
    let files = inputs
        .iter()
        .enumerate()
        .filter(|(_, f)| f.lang == GO && f.facts.module.is_some());
    for (i, f) in files {
        for tf in &f.facts.type_facts {
            match tf {
                TypeFact::Param {
                    func,
                    name,
                    kind,
                    ty,
                    ..
                } => {
                    let n = flow.node(func, name);
                    let ts = match (kind, ty) {
                        (ParamKind::Receiver, Some(t)) => resolve_type(ix, i, func, t, false),
                        _ => TypeSet::top(),
                    };
                    flow.seed(n, &ts);
                }
                TypeFact::Bind { func, var, src } => {
                    let n = flow.node(func, var);
                    match src {
                        ValueSource::Var(v) => copy_from(ix, flow, func, v, n),
                        ValueSource::New(t) | ValueSource::Declared(t) => {
                            let ts = resolve_type(ix, i, func, t, false);
                            flow.seed(n, &ts);
                        }
                        ValueSource::Null => {}
                        _ => flow.seed(n, &TypeSet::top()),
                    }
                }
                _ => {}
            }
        }
    }
}

/// A copy edge from `v` as `func` sees it (its own or an enclosing
/// callable's binding) into node `to`; `top` when `v` is not a local.
fn copy_from<'a>(
    ix: &ReceiverIndex<'a>,
    flow: &mut Flow<'a>,
    func: &'a str,
    v: &'a str,
    to: usize,
) {
    match ix.locals.owner(ix.table, func, v) {
        Some(o) => {
            let from = flow.node(o, v);
            flow.copy(from, to);
        }
        None => flow.seed(to, &TypeSet::top()),
    }
}

/// Narrow `v.m()` to the methods of `v`'s closed, in-repo type set; `None`
/// when the set is open, empty or unknown, or some type lacks `m`.
pub(crate) fn classify<'t>(ix: &ReceiverIndex<'t>, site: &Site<'_>) -> Option<Vec<&'t DefEntry>> {
    let path = &site.raw.name_path;
    if path.len() != 2 {
        return None;
    }
    let caller = site.caller.fqn.as_str();
    let owner = ix.locals.owner(ix.table, caller, &path[0])?;
    let ts = ix.types.flow.get(owner, &path[0])?;
    if ts.top || ts.types.is_empty() || !ts.ext.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for &(c, _) in &ts.types {
        let hits = ix.hits_for(&BTreeSet::from([c]), site.method(), GO);
        if hits.is_empty() {
            return None;
        }
        out.extend(hits);
    }
    Some(out)
}
