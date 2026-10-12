//! Per-callable name bindings and anonymous receiver roots, from the
//! receiver-typing facts.

use std::collections::{BTreeMap, BTreeSet};

use cgx_frontend::facts::{AnonRoot, TypeFact};

use crate::input::FileInput;
use crate::symtab::SymbolTable;

use super::lang;

/// `(callable, line, col, method, depth)`, as `TypeFact::AnonReceiver` keys it.
pub(crate) type AnonKey<'a> = (&'a str, u32, u32, &'a str, u8);

/// Two same-FQN callables (in two files) recording different roots at one
/// position: neither can be trusted.
static CONFLICT: AnonRoot = AnonRoot::Other;

#[derive(Debug, Default)]
pub(crate) struct Locals<'a> {
    /// `(callable, name)` for every parameter and bound local.
    locals: BTreeSet<(&'a str, &'a str)>,
    /// `(callable, name)` → the number of distinct binding sites of `name` in
    /// the body (`Bind` facts after deduplication).
    binds: BTreeMap<(&'a str, &'a str), u32>,
    /// `(callable, name)` for every parameter.
    params: BTreeSet<(&'a str, &'a str)>,
    anon: BTreeMap<AnonKey<'a>, &'a AnonRoot>,
}

impl<'a> Locals<'a> {
    pub(crate) fn build(inputs: &'a [FileInput<'a>]) -> Self {
        let mut l = Locals::default();
        for f in inputs.iter().filter(|f| lang::rules(&f.lang).is_some()) {
            for tf in &f.facts.type_facts {
                match tf {
                    TypeFact::Param { func, name, .. } => {
                        l.locals.insert((func, name));
                        l.params.insert((func, name));
                    }
                    TypeFact::Bind { func, var, .. } => {
                        l.locals.insert((func, var));
                        *l.binds.entry((func, var)).or_default() += 1;
                    }
                    TypeFact::AnonReceiver {
                        func,
                        line,
                        col,
                        method,
                        depth,
                        root,
                    } => {
                        let slot = l
                            .anon
                            .entry((func, *line, *col, method, *depth))
                            .or_insert(root);
                        if *slot != root {
                            *slot = &CONFLICT;
                        }
                    }
                    _ => {}
                }
            }
        }
        l
    }

    /// The callable that binds `name` for code in `scope`: `scope` itself or
    /// the nearest enclosing callable with a parameter or binding of that name.
    /// `None` when the name is not bound by any enclosing callable (a module
    /// or builtin name). Class bodies and modules are not callables.
    pub(crate) fn owner<'c>(
        &self,
        table: &SymbolTable,
        scope: &'c str,
        name: &str,
    ) -> Option<&'c str> {
        let mut cur = scope;
        loop {
            if !table.def_by_fqn(cur)?.kind.is_callable() {
                return None;
            }
            if self.locals.contains(&(cur, name)) {
                return Some(cur);
            }
            cur = cur.rsplit_once("::")?.0;
        }
    }

    /// Whether `func`'s body rebinds `name` (beyond its parameter).
    pub(crate) fn rebinds(&self, func: &str, name: &str) -> bool {
        self.binds.contains_key(&(func, name))
    }

    /// Whether `name` has exactly one binding site in `func` and is not a
    /// parameter: a class defined there and never reassigned. Binding sites
    /// with identical facts are deduplicated by the frontend, so a
    /// reassignment from an untyped value (`Opaque`, the same fact a nested
    /// class def records) is indistinguishable from the def itself.
    pub(crate) fn sole_binding(&self, func: &str, name: &str) -> bool {
        self.binds.get(&(func, name)) == Some(&1) && !self.params.contains(&(func, name))
    }

    pub(crate) fn anon(&self, key: &AnonKey<'_>) -> Option<&'a AnonRoot> {
        self.anon.get(key).copied()
    }
}
