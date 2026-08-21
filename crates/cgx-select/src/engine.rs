//! The optimistic multi-parse driver and the compiled [`Selector`] — the public
//! entry the wiring engineer builds against.

use crate::diagnostics::{Diagnostic, NodeMatch, RejectReason, SelectorError};
use crate::family::Family;
use crate::nfa::Nfa;
use crate::tokenizer::{registered, TokenizeOutput};
use cgx_core::node::NodeRecord;

/// The default active-state cap for the subset simulation. The NFA is flat
/// (O(|pattern|) states) so this is not normally reached; it is the honesty
/// backstop for pathological `**::x::**::y::**` shapes (dispatch §6).
pub const DEFAULT_ACTIVE_STATE_CAP: usize = 1024;

/// Match-time knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchOptions {
    /// `false` (default): a node matches only via a parse whose family owns the
    /// node's `lang` tag. `true`: drop the family gate and match every symbol
    /// against the normalized pattern (dispatch §8).
    pub agnostic: bool,
    /// Active-state cap for the subset simulation.
    pub active_state_cap: usize,
}

impl Default for MatchOptions {
    fn default() -> Self {
        MatchOptions {
            agnostic: false,
            active_state_cap: DEFAULT_ACTIVE_STATE_CAP,
        }
    }
}

impl MatchOptions {
    /// Native (family-restricted) matching with the default cap.
    pub fn native() -> Self {
        MatchOptions::default()
    }

    /// Language-agnostic matching with the default cap.
    pub fn agnostic() -> Self {
        MatchOptions {
            agnostic: true,
            ..MatchOptions::default()
        }
    }
}

/// One clean parse: its NFA and the family that produced it.
#[derive(Debug, Clone)]
struct Parse {
    family: Family,
    nfa: Nfa,
}

/// A compiled selector: the union of every tokenizer's clean parse, plus the
/// parse-time diagnostics. Evaluate it per node with [`Selector::evaluate`], or
/// use the [`Selector::matches`] bool predicate as a drop-in for the
/// `resolve_symbol` scan.
#[derive(Debug, Clone)]
pub struct Selector {
    parses: Vec<Parse>,
    families: Vec<Family>,
    diagnostics: Vec<Diagnostic>,
}

/// Compile a raw argv selector string. Attempts every registered tokenizer and
/// carries every clean parse; **zero clean parses is always a labeled error**,
/// never a silent empty match (dispatch §4).
pub fn compile(selector: &str) -> Result<Selector, SelectorError> {
    if selector.is_empty() {
        return Err(SelectorError::Empty);
    }
    let mut parses: Vec<Parse> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut rejects: Vec<RejectReason> = Vec::new();

    for tok in registered() {
        match tok.tokenize(selector) {
            Ok(TokenizeOutput { pattern, rewrites }) => {
                for (original, rewritten) in rewrites {
                    diagnostics.push(Diagnostic::NegationRewrite {
                        family: tok.family(),
                        original_segment: original,
                        rewritten_segment: rewritten,
                    });
                }
                parses.push(Parse {
                    family: tok.family(),
                    nfa: Nfa::compile(&pattern),
                });
            }
            Err(reason) => rejects.push(reason),
        }
    }

    if parses.is_empty() {
        return Err(SelectorError::from_rejects(selector, &rejects));
    }

    // `registered()` is already family-sorted, so parses and families are
    // deterministically ordered.
    let mut families: Vec<Family> = parses.iter().map(|p| p.family).collect();
    families.dedup();

    Ok(Selector {
        parses,
        families,
        diagnostics,
    })
}

impl Selector {
    /// The families whose tokenizer accepted this selector (its provenance union).
    pub fn families(&self) -> &[Family] {
        &self.families
    }

    /// Parse-time diagnostics (e.g. the `!*` → `*!` rewrite warning).
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Evaluate one node, returning the match plus provenance and honesty signals.
    pub fn evaluate(&self, node: &NodeRecord, opts: &MatchOptions) -> NodeMatch {
        let segments: Vec<&str> = node.fqn.split("::").collect();
        let node_lang = node.lang.as_str();

        let mut native_families: Vec<Family> = Vec::new();
        let mut all_families: Vec<Family> = Vec::new();
        let mut truncated = false;

        for parse in &self.parses {
            let outcome = parse.nfa.simulate(&segments, opts.active_state_cap);
            if outcome.truncated {
                truncated = true;
            }
            if outcome.matched {
                all_families.push(parse.family);
                if parse.family.lang_tags().contains(&node_lang) {
                    native_families.push(parse.family);
                }
            }
        }

        let native_hit = !native_families.is_empty();
        let any_hit = !all_families.is_empty();

        let matched = if opts.agnostic { any_hit } else { native_hit };
        let agnostic_cross_language = opts.agnostic && any_hit && !native_hit;

        let mut families = if !matched {
            Vec::new()
        } else if opts.agnostic {
            all_families
        } else {
            native_families
        };
        families.sort();
        families.dedup();

        NodeMatch {
            matched,
            families,
            truncated,
            agnostic_cross_language,
        }
    }

    /// Bool predicate — a drop-in for `pat.matches(node)` in the `resolve_symbol`
    /// scan. Discards the diagnostics [`evaluate`](Self::evaluate) carries; use
    /// `evaluate` when truncation/provenance must reach the caller.
    pub fn matches(&self, node: &NodeRecord, opts: &MatchOptions) -> bool {
        self.evaluate(node, opts).matched
    }

    /// Convenience: native match with default options.
    pub fn matches_native(&self, node: &NodeRecord) -> bool {
        self.matches(node, &MatchOptions::native())
    }
}
