//! Structured signature record (ADR-04, GM-1.3).
//!
//! A nullable structured record on `function`/`method`/`lambda` symbols.
//! `type_text` is the declared type *as written in source* (Tier 1) or the
//! SCIP-resolved type (Tier 2+); it is never inferred to fill a gap. The
//! `canonical` string is derived deterministically from the structured fields.
//!
//! Equality for the semver/breaking-change theme compares the structured fields
//! field-wise (the derived `PartialEq`), not the canonical string — so a
//! pure-formatting change does not register as a signature change (ADR-04).

use serde::{Deserialize, Serialize};

/// One declared parameter of a callable (ADR-04).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Param {
    /// Parameter name as written. Dynamic languages without annotations still
    /// carry the name with a `None` `type_text`.
    pub name: String,
    /// Declared type as written in source, or SCIP-resolved; `None` when the
    /// surface declares no type. Never inferred.
    pub type_text: Option<String>,
    /// Whether the parameter has a default value.
    pub has_default: bool,
    /// Whether the parameter is variadic (`...args`, `*args`, `args: ...T`).
    pub variadic: bool,
}

/// Structured signature of a callable symbol (ADR-04, GM-1.3 `signature`).
///
/// Populated best-effort from Phase 1 where the language surface declares the
/// pieces; fields are `None`/empty where it does not. Nullable as a whole: a
/// symbol whose kind is not callable carries no `Signature`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Signature {
    /// Ordered parameter list.
    pub params: Vec<Param>,
    /// Declared return type as written, or `None`.
    pub return_type_text: Option<String>,
    /// Generic/type parameters as written (e.g. `T`, `T: Clone`).
    pub type_params: Vec<String>,
    /// Receiver type for methods (`self`/`this` type), or `None`.
    pub receiver: Option<String>,
}

impl Signature {
    /// An empty signature: no params, no return type, no generics, no receiver.
    pub fn empty() -> Self {
        Signature {
            params: Vec::new(),
            return_type_text: None,
            type_params: Vec::new(),
            receiver: None,
        }
    }

    /// Render the deterministic canonical string from the structured fields
    /// (ADR-04 `canonical`). A single normalization rule, language-tagged by the
    /// caller via `fqn`/`lang` elsewhere — this rendering is language-neutral.
    ///
    /// Shape: `[<T1, T2>] (name: type, name2, ...) -> ret` where missing pieces
    /// are elided. Untyped params render as the bare name; typed params as
    /// `name: type`; variadic params are prefixed with `...`; defaulted params
    /// are suffixed with ` = …`.
    pub fn canonical(&self) -> String {
        let mut out = String::new();

        if let Some(receiver) = &self.receiver {
            out.push('(');
            out.push_str(receiver);
            out.push_str(") ");
        }

        if !self.type_params.is_empty() {
            out.push('<');
            for (i, tp) in self.type_params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(tp);
            }
            out.push_str("> ");
        }

        out.push('(');
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            if p.variadic {
                out.push_str("...");
            }
            out.push_str(&p.name);
            if let Some(ty) = &p.type_text {
                out.push_str(": ");
                out.push_str(ty);
            }
            if p.has_default {
                out.push_str(" = …");
            }
        }
        out.push(')');

        if let Some(ret) = &self.return_type_text {
            out.push_str(" -> ");
            out.push_str(ret);
        }

        out
    }
}
