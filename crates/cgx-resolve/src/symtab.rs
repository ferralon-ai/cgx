//! The symbol table: the cross-file index the link pass resolves against.
//!
//! Built once from every file's [`FileFacts`] before any reference is resolved.
//! Everything here is order-independent: maps are `BTreeMap`, candidate lists are
//! sorted, so the table is a pure function of the input set regardless of file
//! order (architecture §3 determinism).

use std::collections::BTreeMap;

use cgx_core::id::NodeId;
use cgx_core::node::{NodeRecord, SymbolKind};
use cgx_frontend::facts::{FileFacts, ImportFact, ScopeId};

/// A definition's address in the linked graph plus the bits the link pass needs.
#[derive(Debug, Clone)]
pub struct DefEntry {
    pub node_id: NodeId,
    pub fqn: String,
    pub kind: SymbolKind,
    /// Callee arity from the signature, when the surface pinned it (ADR-04).
    pub arity: Option<u8>,
}

/// One import binding visible somewhere in a file: a local name (possibly an
/// alias) bound to `(module specifier, exported name)`.
#[derive(Debug, Clone)]
pub struct ImportBinding {
    /// The name as used in this file (the alias if renamed, else the import name).
    pub local: String,
    /// The exported name in the source module.
    pub exported: String,
    /// The module specifier as written (`crate::direct`, `./direct`, `react`).
    pub specifier: String,
    /// Scope the binding is visible in.
    pub scope: ScopeId,
    /// Glob import (`use a::*`): `local`/`exported` are empty and every exported
    /// name of the module is in scope.
    pub glob: bool,
}

/// The symbol table for one link.
#[derive(Debug, Default)]
pub struct SymbolTable {
    /// FQN → def. Unique: two defs with the same FQN collapse (last wins after
    /// canonical sort, but FQNs are expected unique within a tree).
    by_fqn: BTreeMap<String, DefEntry>,
    /// Short (last-segment) name → defs, sorted by node id. The Tier-0 fallback
    /// and virtual-dispatch candidate sets read this.
    by_short: BTreeMap<String, Vec<DefEntry>>,
    /// Module FQN → its member defs (one level: direct children), for resolving
    /// `module::name` import specifiers. Keyed by the module-path prefix.
    by_module_prefix: BTreeMap<String, Vec<DefEntry>>,
    /// Re-export edges keyed by `(module-last-segment, exported-name)` →
    /// `(source specifier, source name)`. Lets `resolve_member` chase a
    /// `pub use`/`export … from` chain to the defining module (architecture §2
    /// "import/re-export chains"). Keyed by last segment to match the suffix
    /// resolution `resolve_member` already uses for path-style specifiers.
    reexports: BTreeMap<(String, String), (String, String)>,
}

impl SymbolTable {
    /// Build the table from the assigned nodes and the per-file facts. `nodes`
    /// must already carry their final dense [`NodeId`]s (assigned by the caller
    /// via the canonical sort). `files` supplies the re-export facts; each entry
    /// is `(module-last-segment, &FileFacts)`.
    pub fn build(nodes: &[NodeRecord], files: &[(String, &FileFacts)]) -> Self {
        let mut t = SymbolTable::default();
        for (module_seg, facts) in files {
            t.record_reexports(module_seg, facts);
        }
        for n in nodes {
            let entry = DefEntry {
                node_id: n.id,
                fqn: n.fqn.clone(),
                kind: n.kind,
                arity: n.signature.as_ref().map(|s| s.params.len().min(255) as u8),
            };
            t.by_fqn.insert(n.fqn.clone(), entry.clone());
            t.by_short
                .entry(short_name(&n.fqn).to_owned())
                .or_default()
                .push(entry.clone());
            if let Some(prefix) = module_prefix(&n.fqn) {
                t.by_module_prefix
                    .entry(prefix.to_owned())
                    .or_default()
                    .push(entry);
            }
        }
        // Keep candidate lists deterministic.
        for v in t.by_short.values_mut() {
            v.sort_by_key(|a| a.node_id.0);
        }
        for v in t.by_module_prefix.values_mut() {
            v.sort_by_key(|a| a.node_id.0);
        }
        t
    }

    /// Record the re-export edges a file declares (`pub use … from`, `export …
    /// from`). `module_seg` is the file's module last segment (the suffix key).
    fn record_reexports(&mut self, module_seg: &str, facts: &FileFacts) {
        for imp in &facts.imports {
            if !imp.re_export {
                continue;
            }
            for n in &imp.names {
                // The name visible from the re-exporting module is the alias if
                // present (`pub use a::b as c` re-exports `c`), else the name.
                let local = n.alias.clone().unwrap_or_else(|| n.name.clone());
                self.reexports.insert(
                    (module_seg.to_owned(), local),
                    (imp.specifier.clone(), n.name.clone()),
                );
            }
        }
        for exp in &facts.exports {
            if let Some(from) = &exp.from {
                let local = exp.alias.clone().unwrap_or_else(|| exp.name.clone());
                self.reexports.insert(
                    (module_seg.to_owned(), local),
                    (from.clone(), exp.name.clone()),
                );
            }
        }
    }

    /// Exact FQN lookup.
    pub fn def_by_fqn(&self, fqn: &str) -> Option<&DefEntry> {
        self.by_fqn.get(fqn)
    }

    /// All defs with a given short name, in node-id order (candidate-set source).
    pub fn defs_by_short(&self, name: &str) -> &[DefEntry] {
        self.by_short.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Resolve a member named `name` within the module addressed by `specifier`,
    /// chasing re-export chains (`pub use`, `export … from`) up to a bounded
    /// depth. Tries direct module-member resolution at each hop; when that finds
    /// nothing and a re-export edge exists for `(module, name)`, follows it.
    pub fn resolve_member(&self, specifier: &str, name: &str) -> ResolveOutcome<'_> {
        let mut spec = specifier.to_owned();
        let mut nm = name.to_owned();
        // Depth bound prevents an infinite loop on a pathological re-export cycle.
        for _ in 0..16 {
            match self.resolve_member_direct(&spec, &nm) {
                ResolveOutcome::None => {
                    let seg = last_segment(&spec);
                    match self.reexports.get(&(seg.to_owned(), nm.clone())) {
                        Some((next_spec, next_name)) => {
                            spec = next_spec.clone();
                            nm = next_name.clone();
                        }
                        None => return ResolveOutcome::None,
                    }
                }
                other => return other,
            }
        }
        ResolveOutcome::None
    }

    /// One hop of module-member resolution: try the specifier as a module prefix
    /// directly, then by suffix match (relative path `./direct` → `*::direct`).
    fn resolve_member_direct(&self, specifier: &str, name: &str) -> ResolveOutcome<'_> {
        for prefix in candidate_module_prefixes(specifier) {
            // Direct: the module FQN literally equals the prefix.
            if let Some(members) = self.by_module_prefix.get(prefix.as_str()) {
                let hits: Vec<&DefEntry> = members
                    .iter()
                    .filter(|d| short_name(&d.fqn) == name)
                    .collect();
                match hits.len() {
                    0 => {}
                    1 => return ResolveOutcome::Unique(hits[0]),
                    _ => return ResolveOutcome::Ambiguous(hits),
                }
            }
            // Suffix: the specifier's last segment matches the module-prefix's
            // last segment (relative path `./direct` → module `*::direct`).
            if let Some(last) = prefix.rsplit("::").next() {
                let mut hits: Vec<&DefEntry> = Vec::new();
                for (mod_fqn, members) in &self.by_module_prefix {
                    if mod_fqn.rsplit("::").next() == Some(last) {
                        for d in members {
                            if short_name(&d.fqn) == name {
                                hits.push(d);
                            }
                        }
                    }
                }
                hits.sort_by_key(|a| a.node_id.0);
                hits.dedup_by(|a, b| a.node_id == b.node_id);
                match hits.len() {
                    0 => {}
                    1 => return ResolveOutcome::Unique(hits[0]),
                    _ => return ResolveOutcome::Ambiguous(hits),
                }
            }
        }
        ResolveOutcome::None
    }
}

/// The result of a module-member lookup.
#[derive(Debug)]
pub enum ResolveOutcome<'a> {
    /// Exactly one def matched.
    Unique(&'a DefEntry),
    /// Several defs matched (overloads / re-exports collapsing).
    Ambiguous(Vec<&'a DefEntry>),
    /// No def matched.
    None,
}

/// Collect the import bindings of one file, flattened to one entry per local name.
pub fn collect_import_bindings(imports: &[ImportFact]) -> Vec<ImportBinding> {
    let mut out = Vec::new();
    for imp in imports {
        if imp.glob {
            out.push(ImportBinding {
                local: String::new(),
                exported: String::new(),
                specifier: imp.specifier.clone(),
                scope: imp.scope,
                glob: true,
            });
            continue;
        }
        for n in &imp.names {
            let local = n.alias.clone().unwrap_or_else(|| n.name.clone());
            out.push(ImportBinding {
                local,
                exported: n.name.clone(),
                specifier: imp.specifier.clone(),
                scope: imp.scope,
                glob: false,
            });
        }
    }
    out.sort_by(|a, b| (&a.local, &a.specifier).cmp(&(&b.local, &b.specifier)));
    out
}

/// The last `::`-delimited segment of an FQN.
pub fn short_name(fqn: &str) -> &str {
    fqn.rsplit("::").next().unwrap_or(fqn)
}

/// The module-path prefix of an FQN: everything before the last `::` segment.
/// `a::b::c` → `Some("a::b")`; a bare name → `None`.
fn module_prefix(fqn: &str) -> Option<&str> {
    fqn.rfind("::").map(|i| &fqn[..i])
}

/// The last meaningful segment of a module specifier, the key into the re-export
/// map. `crate::direct` → `direct`; `./direct` → `direct`; `react` → `react`.
pub fn last_segment(specifier: &str) -> &str {
    if specifier.contains("::") {
        specifier.rsplit("::").next().unwrap_or(specifier)
    } else {
        specifier
            .trim_start_matches("./")
            .trim_start_matches("../")
            .rsplit(['/', '.'])
            .find(|s| !s.is_empty())
            .unwrap_or(specifier)
    }
}

/// Candidate normalized module prefixes for an import specifier, most specific
/// first. Language-neutral: handles Rust `crate::`/`self::`/`super::`-style and
/// path-style (`./`, `../`) specifiers without knowing the language.
fn candidate_module_prefixes(specifier: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Rust-style `::`-pathed specifier.
    if specifier.contains("::") {
        out.push(specifier.to_string());
        // Strip a leading `crate::`/`self::`/`super::` keyword segment so the
        // remainder can match a crate-rooted module by suffix.
        for kw in ["crate::", "self::", "super::"] {
            if let Some(rest) = specifier.strip_prefix(kw) {
                out.push(rest.to_string());
            }
        }
    } else {
        // Path-style or bare-module specifier: keep the final segment so the
        // suffix match in `resolve_member` can find `*::<seg>`.
        let seg = specifier
            .trim_start_matches("./")
            .trim_start_matches("../")
            .rsplit(['/', '.'])
            .find(|s| !s.is_empty())
            .unwrap_or(specifier);
        out.push(seg.to_string());
    }
    out.dedup();
    out
}
