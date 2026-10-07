//! Module identity for receiver narrowing.
//!
//! Every file's canonical module path (`FileFacts::module`) forms the module
//! set. An import spec is in-repo when some module equals it or ends with it on
//! a `::` boundary (a sys.path root below the repo root, such as a `tests/`
//! directory); otherwise it is out of repo. Names resolve exactly: a module's
//! own defs, its submodules, then its explicit imports and its glob imports
//! (package re-exports), cycle-guarded. There is no global short-name fallback:
//! a name with no lexical, import or glob resolution is `NotFound`.
//!
//! A module-level value resolves through its binding sites: an alias of a
//! class (`Compat = Real`) is that class, the result of an out-of-repo call
//! (`Base = declarative_base()`) is `Dynamic`, and anything else is a
//! non-class value. A method body does not see its class body's names.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use cgx_frontend::facts::{TypeFact, ValueSource};

use crate::input::FileInput;
use crate::symtab::SymbolTable;

use super::imports::{file_scope, FileScope, Spec, Target};
use super::lang;

/// The resolution of a dotted name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// In-repo classes (lattice ids); several when the name is ambiguous.
    Class(BTreeSet<u32>),
    Module(String),
    /// An in-repo def that is not a class (function, variable).
    NonClass,
    /// Proven out of repo (an out-of-repo module, a builtin).
    External,
    /// A value produced by calling something out of repo.
    Dynamic,
    NotFound,
}

enum InRepo<'m> {
    Exact(&'m str),
    Suffix(Vec<&'m str>),
    /// A relative spec naming no module.
    Missing,
    External,
}

/// The defs `resolve` consults: lattice class ids by FQN, and the symbol table.
pub(crate) struct Defs<'a> {
    pub class_ids: &'a BTreeMap<String, u32>,
    pub table: &'a SymbolTable,
}

#[derive(Debug, Default)]
pub(crate) struct ModuleIndex {
    modules: BTreeMap<String, Vec<usize>>,
    by_last: BTreeMap<String, Vec<String>>,
    files: Vec<FileScope>,
    /// `(module, name)` → the sources of each module-level binding of `name`.
    values: BTreeMap<(String, String), Vec<ValueSource>>,
    /// Distinct import specs that match several in-repo modules by suffix.
    pub(crate) spec_ambiguous: usize,
    memo: RefCell<HashMap<(Target, Vec<String>), Resolved>>,
}

type Seen = BTreeSet<(String, String)>;

impl ModuleIndex {
    pub(crate) fn build(inputs: &[FileInput<'_>]) -> Self {
        let mut idx = ModuleIndex::default();
        for (i, f) in inputs.iter().enumerate() {
            let Some(m) = &f.facts.module else { continue };
            idx.modules.entry(m.clone()).or_default().push(i);
            for tf in &f.facts.type_facts {
                if let TypeFact::Bind { func, var, src } = tf {
                    if func == m {
                        let key = (m.clone(), var.clone());
                        idx.values.entry(key).or_default().push(src.clone());
                    }
                }
            }
            if f.lang == "python" {
                let mut cur = m.as_str();
                while let Some((parent, _)) = cur.rsplit_once("::") {
                    idx.modules.entry(parent.to_owned()).or_default();
                    cur = parent;
                }
            }
        }
        for m in idx.modules.keys() {
            let last = m.rsplit("::").next().unwrap_or(m);
            idx.by_last
                .entry(last.to_owned())
                .or_default()
                .push(m.clone());
        }
        idx.files = inputs.iter().map(file_scope).collect();
        let mut ambiguous = BTreeSet::new();
        for fs in &idx.files {
            let targets = fs.bindings.values().flatten().map(|t| match t {
                Target::Module(s) | Target::Member(s, _) | Target::ModuleOrMember(s, _) => s,
            });
            for s in targets.chain(&fs.globs) {
                if matches!(idx.in_repo(s), InRepo::Suffix(ref v) if v.len() > 1) {
                    ambiguous.insert(&s.path);
                }
            }
        }
        idx.spec_ambiguous = ambiguous.len();
        idx
    }

    fn in_repo(&self, spec: &Spec) -> InRepo<'_> {
        if let Some((k, _)) = self.modules.get_key_value(&spec.path) {
            return InRepo::Exact(k);
        }
        if spec.relative {
            return InRepo::Missing;
        }
        // A module ending with the spec (a sys.path root below the module's
        // own root), or a spec ending with a module (a nested project root
        // below the importer's sys.path root).
        let last = spec.path.rsplit("::").next().unwrap_or(&spec.path);
        let suffix = format!("::{}", spec.path);
        let hits: Vec<&str> = self
            .by_last
            .get(last)
            .into_iter()
            .flatten()
            .filter(|m| m.ends_with(&suffix) || spec.path.ends_with(&format!("::{m}")))
            .map(String::as_str)
            .collect();
        if hits.is_empty() {
            InRepo::External
        } else {
            InRepo::Suffix(hits)
        }
    }

    /// Whether every import binding of `head` in `file` names an out-of-repo
    /// module (and no same-module def shadows it): an attribute chain rooted at
    /// `head` cannot reach an in-repo method.
    pub(crate) fn head_is_external_module(&self, d: &Defs<'_>, file: usize, head: &str) -> bool {
        let fs = &self.files[file];
        if let Some(m) = &fs.module {
            if d.table.def_by_fqn(&format!("{m}::{head}")).is_some() {
                return false;
            }
        }
        let Some(targets) = fs.bindings.get(head) else {
            return false;
        };
        targets.iter().all(|t| match t {
            Target::Module(s) | Target::ModuleOrMember(s, _) => {
                matches!(self.in_repo(s), InRepo::External)
            }
            Target::Member(..) => false,
        })
    }

    /// Resolve a dotted name as written in `file`, inside the lexical container
    /// `scope_fqn`: same-file defs (innermost container first), then the file's
    /// import bindings, then its glob imports, then the language's builtins.
    pub(crate) fn resolve_name_in_file(
        &self,
        d: &Defs<'_>,
        file: usize,
        scope_fqn: &str,
        path: &[String],
    ) -> Resolved {
        let fs = &self.files[file];
        let Some((head, rest)) = path.split_first() else {
            return Resolved::NotFound;
        };
        if let Some(module) = fs.module.as_deref().filter(|m| scope_fqn.starts_with(*m)) {
            let mut scope = scope_fqn;
            loop {
                let other_class_body = scope != scope_fqn && d.class_ids.contains_key(scope);
                let fqn = format!("{scope}::{head}");
                if !other_class_body {
                    if let Some(r) = self.def_at(d, &fqn, rest, &mut Seen::new()) {
                        return r;
                    }
                }
                match scope.rsplit_once("::") {
                    Some((parent, _)) if scope.len() > module.len() => scope = parent,
                    _ => break,
                }
            }
        }
        if let Some(targets) = fs.bindings.get(head) {
            let mut acc = None;
            for t in targets {
                join(&mut acc, self.resolve_target_fresh(d, t, rest));
            }
            return acc.unwrap_or(Resolved::NotFound);
        }
        let mut acc = None;
        for g in &fs.globs {
            join_found(
                &mut acc,
                self.resolve_target_fresh(d, &Target::Module(g.clone()), path),
            );
        }
        if let Some(r) = acc {
            return r;
        }
        match lang::rules(&fs.lang) {
            Some(r) if r.builtin_types.contains(&head.as_str()) => Resolved::External,
            _ => Resolved::NotFound,
        }
    }

    /// [`Self::resolve_target`] with a fresh cycle guard: a pure function of its
    /// arguments, so it is memoized.
    fn resolve_target_fresh(&self, d: &Defs<'_>, t: &Target, rest: &[String]) -> Resolved {
        let key = (t.clone(), rest.to_vec());
        if let Some(hit) = self.memo.borrow().get(&key) {
            return hit.clone();
        }
        let out = self.resolve_target(d, t, rest, &mut Seen::new());
        self.memo.borrow_mut().insert(key, out.clone());
        out
    }

    fn resolve_target(
        &self,
        d: &Defs<'_>,
        t: &Target,
        rest: &[String],
        seen: &mut Seen,
    ) -> Resolved {
        let member = |s: &Spec, n: &String, seen: &mut Seen| {
            let path: Vec<String> = std::iter::once(n.clone())
                .chain(rest.iter().cloned())
                .collect();
            self.resolve_spec(d, s, &path, seen)
        };
        match t {
            Target::Module(s) => self.resolve_spec(d, s, rest, seen),
            Target::Member(s, n) => member(s, n, seen),
            Target::ModuleOrMember(s, n) => {
                let as_module = self.resolve_spec(d, s, rest, seen);
                match member(s, n, seen) {
                    Resolved::NotFound => as_module,
                    m if m == as_module => m,
                    _ => Resolved::NotFound,
                }
            }
        }
    }

    fn resolve_spec(
        &self,
        d: &Defs<'_>,
        spec: &Spec,
        path: &[String],
        seen: &mut Seen,
    ) -> Resolved {
        match self.in_repo(spec) {
            InRepo::Exact(m) => self.resolve_in(d, m, path, seen),
            InRepo::Suffix(ms) => {
                let mut acc = None;
                for m in ms {
                    join_found(&mut acc, self.resolve_in(d, m, path, seen));
                }
                acc.unwrap_or(Resolved::NotFound)
            }
            InRepo::Missing => Resolved::NotFound,
            InRepo::External => Resolved::External,
        }
    }

    /// `path` inside in-repo `module`: its def, its submodule, its explicit
    /// imports (recursing on their targets), then its glob imports (union).
    fn resolve_in(&self, d: &Defs<'_>, module: &str, path: &[String], seen: &mut Seen) -> Resolved {
        let Some((n, rest)) = path.split_first() else {
            return Resolved::Module(module.to_owned());
        };
        let fqn = format!("{module}::{n}");
        if let Some(r) = self.def_at(d, &fqn, rest, seen) {
            return r;
        }
        if self.modules.contains_key(&fqn) {
            return self.resolve_in(d, &fqn, rest, seen);
        }
        if !seen.insert((module.to_owned(), n.clone())) {
            return Resolved::NotFound;
        }
        let files = self.modules.get(module).map(Vec::as_slice).unwrap_or(&[]);
        let mut acc = None;
        for &f in files {
            for t in self.files[f].bindings.get(n).into_iter().flatten() {
                join(&mut acc, self.resolve_target(d, t, rest, seen));
            }
        }
        if let Some(r) = acc {
            return r;
        }
        for &f in files {
            for g in &self.files[f].globs {
                join_found(&mut acc, self.resolve_spec(d, g, path, seen));
            }
        }
        acc.unwrap_or(Resolved::NotFound)
    }

    /// The def at `fqn` (then the nested `rest`): a class, a module-level value
    /// resolved through its bindings, or another def.
    fn def_at(
        &self,
        d: &Defs<'_>,
        fqn: &str,
        rest: &[String],
        seen: &mut Seen,
    ) -> Option<Resolved> {
        if let Some(&c) = d.class_ids.get(fqn) {
            if rest.is_empty() {
                return Some(Resolved::Class(BTreeSet::from([c])));
            }
            let nested = format!("{fqn}::{}", rest.join("::"));
            return Some(match d.class_ids.get(&nested) {
                Some(&n) => Resolved::Class(BTreeSet::from([n])),
                None => Resolved::NonClass,
            });
        }
        d.table.def_by_fqn(fqn)?;
        Some(self.value_at(d, fqn, rest, seen))
    }

    /// A module-level value: the join over its binding sites.
    fn value_at(&self, d: &Defs<'_>, fqn: &str, rest: &[String], seen: &mut Seen) -> Resolved {
        let Some((module, var)) = fqn.rsplit_once("::") else {
            return Resolved::NonClass;
        };
        let key = (module.to_owned(), var.to_owned());
        let Some(srcs) = self.values.get(&key) else {
            return Resolved::NonClass;
        };
        if !seen.insert((module.to_owned(), format!("={var}"))) {
            return Resolved::NotFound;
        }
        let mut acc = None;
        for src in srcs {
            let r = match src {
                ValueSource::Var(x) => {
                    let path: Vec<String> = std::iter::once(x.clone())
                        .chain(rest.iter().cloned())
                        .collect();
                    self.resolve_in(d, module, &path, seen)
                }
                ValueSource::Call(callee) if rest.is_empty() => {
                    match self.resolve_in(d, module, callee, seen) {
                        Resolved::External | Resolved::Dynamic => Resolved::Dynamic,
                        _ => Resolved::NonClass,
                    }
                }
                _ => Resolved::NonClass,
            };
            join(&mut acc, r);
        }
        acc.unwrap_or(Resolved::NonClass)
    }
}

/// Union of several bindings of one name: classes merge; any disagreement is
/// `NotFound` (the binding that wins at run time is not known).
fn join(acc: &mut Option<Resolved>, r: Resolved) {
    *acc = Some(match acc.take() {
        None => r,
        Some(Resolved::Class(mut a)) => match r {
            Resolved::Class(b) => {
                a.extend(b);
                Resolved::Class(a)
            }
            _ => Resolved::NotFound,
        },
        Some(prev) if prev == r => prev,
        Some(_) => Resolved::NotFound,
    });
}

/// As [`join`], ignoring candidates that do not provide the name at all.
fn join_found(acc: &mut Option<Resolved>, r: Resolved) {
    if r != Resolved::NotFound {
        join(acc, r);
    }
}
