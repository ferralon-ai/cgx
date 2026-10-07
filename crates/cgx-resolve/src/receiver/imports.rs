//! File-level import bindings for receiver narrowing, with each language's
//! binding semantics and canonical (`::`-joined) import specs.

use std::collections::BTreeMap;

use cgx_frontend::facts::ImportFact;

use crate::input::FileInput;

/// A canonical (`::`-joined) import spec. A relative spec is always in repo.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Spec {
    pub(super) path: String,
    pub(super) relative: bool,
}

/// What a file-level import binds a local name to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Target {
    Module(Spec),
    Member(Spec, String),
    /// `import a.b as d` and `from a.b import b as d` record the same fact:
    /// either the module `a.b` or its member `b`.
    ModuleOrMember(Spec, String),
}

/// One file's module-level import bindings.
#[derive(Debug, Default)]
pub(super) struct FileScope {
    pub(super) module: Option<String>,
    pub(super) lang: String,
    pub(super) bindings: BTreeMap<String, Vec<Target>>,
    pub(super) globs: Vec<Spec>,
}

pub(super) fn file_scope(f: &FileInput<'_>) -> FileScope {
    let mut fs = FileScope {
        module: f.facts.module.clone(),
        lang: f.lang.clone(),
        ..FileScope::default()
    };
    let is_package = f.path.ends_with("/__init__.py") || f.path == "__init__.py";
    for imp in f.facts.imports.iter().filter(|i| i.scope.index() == 0) {
        match f.lang.as_str() {
            "python" => python_import(&mut fs, imp, is_package),
            "go" => {
                let spec = Spec {
                    path: imp.specifier.clone(),
                    relative: false,
                };
                if imp.glob {
                    fs.globs.push(spec.clone());
                }
                for n in &imp.names {
                    let local = n.alias.clone().unwrap_or_else(|| n.name.clone());
                    bind(&mut fs, local, Target::Module(spec.clone()));
                }
            }
            _ => {}
        }
    }
    fs
}

/// Python binding semantics. `import a.b.c` binds `a`; `import a.b.c as d`
/// binds `d` to `a.b.c`; `from M import n as k` binds `k` to `M.n`. The
/// frontend records `import a.b.c` as `names: [c]`, the same fact as
/// `from a.b.c import c`, so that shape binds both readings.
fn python_import(fs: &mut FileScope, imp: &ImportFact, is_package: bool) {
    let Some(spec) = python_spec(&imp.specifier, fs.module.as_deref(), is_package) else {
        return;
    };
    if imp.glob {
        fs.globs.push(spec);
        return;
    }
    let last = imp.specifier.rsplit('.').next().unwrap_or("");
    for n in &imp.names {
        if spec.relative || n.name != last {
            let local = n.alias.clone().unwrap_or_else(|| n.name.clone());
            bind(fs, local, Target::Member(spec.clone(), n.name.clone()));
            continue;
        }
        match &n.alias {
            Some(alias) => bind(
                fs,
                alias.clone(),
                Target::ModuleOrMember(spec.clone(), n.name.clone()),
            ),
            None => {
                let head = imp.specifier.split('.').next().unwrap_or("");
                let head_spec = Spec {
                    path: head.to_owned(),
                    relative: false,
                };
                bind(fs, head.to_owned(), Target::Module(head_spec));
                if head != n.name {
                    bind(
                        fs,
                        n.name.clone(),
                        Target::Member(spec.clone(), n.name.clone()),
                    );
                }
            }
        }
    }
}

fn bind(fs: &mut FileScope, local: String, t: Target) {
    let v = fs.bindings.entry(local).or_default();
    if !v.contains(&t) {
        v.push(t);
    }
}

/// Canonicalize a Python import spec. A relative spec resolves against the
/// importing file's module: a package is its own base, a module's base is its
/// parent, and each extra leading dot goes up one level.
fn python_spec(written: &str, module: Option<&str>, is_package: bool) -> Option<Spec> {
    let dots = written.chars().take_while(|c| *c == '.').count();
    let rest = &written[dots..];
    if dots == 0 {
        return (!rest.is_empty()).then(|| Spec {
            path: rest.replace('.', "::"),
            relative: false,
        });
    }
    let module = module?;
    let mut base = if is_package { module } else { parent(module)? };
    for _ in 1..dots {
        base = parent(base)?;
    }
    let path = match (base.is_empty(), rest.is_empty()) {
        (_, true) => base.to_owned(),
        (true, false) => rest.replace('.', "::"),
        (false, false) => format!("{base}::{}", rest.replace('.', "::")),
    };
    (!path.is_empty()).then_some(Spec {
        path,
        relative: true,
    })
}

/// The parent module path; the empty string for a top-level module.
fn parent(module: &str) -> Option<&str> {
    if module.is_empty() {
        return None;
    }
    Some(module.rsplit_once("::").map_or("", |(p, _)| p))
}
