//! Hand-built `FileFacts` fixtures for cross-file resolution tests.
//!
//! WP-06's convergence criterion explicitly allows developing against
//! hand-built `FileFacts` before the real language adapters land. These builders
//! model the cross-file scenarios the goldens describe, in the smallest form that
//! exercises one resolution rule each.

#![allow(dead_code)]

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::edge::ImplicitKind;
use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_core::signature::{Param, Signature};
use cgx_frontend::facts::{
    EntrypointHint, ExportFact, FileFacts, ImplRelation, ImportFact, ImportedName, RawRef, RefKind,
    RelationKind, ScopeId, SymbolDef,
};
use smallvec::smallvec;

/// A fluent builder for one file's `FileFacts`.
pub struct FileBuilder {
    facts: FileFacts,
}

impl FileBuilder {
    pub fn new() -> Self {
        FileBuilder {
            facts: FileFacts::empty(),
        }
    }

    /// Push a child scope owned by `owner_fqn` under `parent`, returning its id.
    pub fn scope(&mut self, parent: ScopeId, owner_fqn: Option<&str>) -> ScopeId {
        self.facts
            .scopes
            .push(parent, owner_fqn.map(|s| s.to_owned()))
    }

    /// Add a definition.
    #[allow(clippy::too_many_arguments)]
    pub fn def(
        &mut self,
        fqn: &str,
        kind: SymbolKind,
        scope: ScopeId,
        line: u32,
        visibility: Visibility,
        is_abstract: bool,
        arity: Option<usize>,
    ) -> &mut Self {
        let signature = arity.map(|n| Signature {
            params: (0..n)
                .map(|i| Param {
                    name: format!("p{i}"),
                    type_text: None,
                    has_default: false,
                    variadic: false,
                })
                .collect(),
            return_type_text: None,
            type_params: Vec::new(),
            receiver: None,
        });
        self.facts.defs.push(SymbolDef {
            fqn: fqn.to_owned(),
            kind,
            visibility,
            scope,
            span: Span::new("f", line, Some(1)),
            line_end: line,
            is_abstract,
            signature,
        });
        self
    }

    /// Add a call reference at `scope`.
    pub fn call(&mut self, name_path: &[&str], scope: ScopeId, line: u32) -> &mut Self {
        self.raw_ref(
            name_path,
            scope,
            line,
            RefKind::Call,
            EdgeCondition::Always,
            None,
        )
    }

    /// Add a virtual-receiver call (`x.m()`).
    pub fn vcall(&mut self, name_path: &[&str], scope: ScopeId, line: u32) -> &mut Self {
        self.raw_ref(
            name_path,
            scope,
            line,
            RefKind::CallVirtualReceiver,
            EdgeCondition::Always,
            None,
        )
    }

    /// Add a fully-specified reference.
    pub fn raw_ref(
        &mut self,
        name_path: &[&str],
        scope: ScopeId,
        line: u32,
        kind: RefKind,
        edge_condition: EdgeCondition,
        arity: Option<u8>,
    ) -> &mut Self {
        let names: smallvec::SmallVec<[String; 2]> =
            name_path.iter().map(|s| s.to_string()).collect();
        self.facts.refs.push(RawRef {
            name_path: names,
            scope,
            kind,
            edge_condition,
            implicit: None,
            span: Span::new("f", line, Some(1)),
            stmt_index: line,
            arity,
            cut_markers: smallvec![],
        });
        self
    }

    /// Mark the last-added ref with a frontend cut marker.
    pub fn last_ref_cut(&mut self, marker: CutMarker) -> &mut Self {
        if let Some(r) = self.facts.refs.last_mut() {
            r.cut_markers.push(marker);
        }
        self
    }

    /// Mark the last-added ref as an implicit call.
    pub fn last_ref_implicit(&mut self, kind: ImplicitKind) -> &mut Self {
        if let Some(r) = self.facts.refs.last_mut() {
            r.implicit = Some(kind);
        }
        self
    }

    /// Add a single-name import.
    pub fn import(
        &mut self,
        specifier: &str,
        name: &str,
        alias: Option<&str>,
        re_export: bool,
        scope: ScopeId,
    ) -> &mut Self {
        self.facts.imports.push(ImportFact {
            specifier: specifier.to_owned(),
            names: vec![ImportedName {
                name: name.to_owned(),
                alias: alias.map(|s| s.to_owned()),
            }],
            glob: false,
            re_export,
            scope,
            span: Span::new("f", 1, Some(1)),
        });
        self
    }

    /// Add an instantiation ref (`T { .. }`, `T::new(..)`) at `scope`.
    pub fn instantiate(&mut self, name_path: &[&str], scope: ScopeId, line: u32) -> &mut Self {
        self.raw_ref(
            name_path,
            scope,
            line,
            RefKind::Instantiate,
            EdgeCondition::Always,
            None,
        )
    }

    /// Add a structural type/trait-lattice relation.
    pub fn relation(
        &mut self,
        kind: RelationKind,
        subject: &[&str],
        object: &[&str],
        line: u32,
    ) -> &mut Self {
        self.facts.impl_relations.push(ImplRelation {
            kind,
            subject: subject.iter().map(|s| s.to_string()).collect(),
            object: object.iter().map(|s| s.to_string()).collect(),
            span: Span::new("f", line, Some(1)),
        });
        self
    }

    /// Add a glob import (`use a::*`).
    pub fn glob_import(&mut self, specifier: &str, re_export: bool, scope: ScopeId) -> &mut Self {
        self.facts.imports.push(ImportFact {
            specifier: specifier.to_owned(),
            names: Vec::new(),
            glob: true,
            re_export,
            scope,
            span: Span::new("f", 1, Some(1)),
        });
        self
    }

    /// Add an export fact.
    pub fn export(&mut self, name: &str, alias: Option<&str>, from: Option<&str>) -> &mut Self {
        self.facts.exports.push(ExportFact {
            name: name.to_owned(),
            alias: alias.map(|s| s.to_owned()),
            from: from.map(|s| s.to_owned()),
            span: Span::new("f", 1, Some(1)),
        });
        self
    }

    /// Add an entrypoint hint.
    pub fn entrypoint(&mut self, fqn: &str, kind: EntrypointKind) -> &mut Self {
        self.facts.entrypoint_hints.push(EntrypointHint {
            fqn: fqn.to_owned(),
            kind,
        });
        self
    }

    /// Finish, returning canonicalized facts.
    pub fn build(mut self) -> FileFacts {
        self.facts.canonicalize();
        self.facts
    }
}

impl Default for FileBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// The root scope id, for ergonomics in tests.
pub const ROOT: ScopeId = ScopeId::ROOT;
