//! CI assertion mode (IF-5) and the ADR-08 vacuity guard.
//!
//! A subcommand produces a result count and, for the vacuity check, two extra
//! facts: whether the symbol pattern matched any symbol at all (clause a), and
//! whether an unfiltered run of the same query would have produced results that
//! the active confidence/edge-condition filters then excluded (clause b). The
//! [`evaluate`] function maps `(--assert-empty, --allow-vacuous, counts)` to an
//! [`AssertionOutcome`] carrying the IF-4/ADR-08 exit code.

use crate::exit::ExitCode;

/// The CI assertion requested on the command line (Phase-1 subset: `--assert-empty`).
#[derive(Debug, Clone, Copy, Default)]
pub struct AssertionSpec {
    /// `--assert-empty`: the query must return zero results.
    pub assert_empty: bool,
    /// `--allow-vacuous`: downgrade a vacuous pass (exit 4) to a plain pass (0).
    pub allow_vacuous: bool,
}

/// The inputs the vacuity guard needs beyond the assertion spec.
#[derive(Debug, Clone, Copy)]
pub struct ResultFacts {
    /// Number of results the query returned (with all filters applied).
    pub filtered_count: usize,
    /// Whether the symbol pattern(s) resolved to at least one symbol (clause a:
    /// false ⇒ zero-symbol match ⇒ vacuous).
    pub any_symbol_matched: bool,
    /// Number of results the *same* query returns with confidence/edge-condition
    /// filters removed (clause b). Equal to `filtered_count` when no such filter
    /// is active.
    pub unfiltered_count: usize,
}

/// What the assertion layer decided: the process exit code and whether the pass
/// was vacuous (threaded into JSON/SARIF output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionOutcome {
    pub exit: ExitCode,
    /// Set when an `--assert-empty` gate passed vacuously (ADR-08). Surfaced in
    /// `--format json` (`"vacuous": true`) and as a SARIF `note` even when
    /// `--allow-vacuous` downgrades the exit code.
    pub vacuous: bool,
    /// A human warning to print to stderr (vacuous pass), if any.
    pub warning: Option<String>,
}

/// Apply the assertion + vacuity rules (IF-5 / ADR-08).
///
/// - No `--assert-empty`: always exit 0.
/// - `--assert-empty` with results: assertion fired → exit 1.
/// - `--assert-empty` with zero results: passed. Then the vacuity guard:
///   - clause (a) zero-symbol match, OR
///   - clause (b) unfiltered query had results but filters excluded them all
///     → vacuous. Exit 4 unless `--allow-vacuous` (then exit 0 + warning),
///     `vacuous` flag set either way.
pub fn evaluate(spec: AssertionSpec, facts: ResultFacts) -> AssertionOutcome {
    if !spec.assert_empty {
        return AssertionOutcome {
            exit: ExitCode::Ok,
            vacuous: false,
            warning: None,
        };
    }

    if facts.filtered_count > 0 {
        return AssertionOutcome {
            exit: ExitCode::AssertionFailed,
            vacuous: false,
            warning: None,
        };
    }

    // Assertion passed (zero results). Was the pass vacuous?
    let zero_symbol = !facts.any_symbol_matched;
    let filter_excluded = facts.unfiltered_count > 0; // filtered_count is 0 here
    let vacuous = zero_symbol || filter_excluded;

    if !vacuous {
        return AssertionOutcome {
            exit: ExitCode::Ok,
            vacuous: false,
            warning: None,
        };
    }

    let reason = if zero_symbol {
        "the symbol pattern matched zero symbols"
    } else {
        "confidence/edge-condition filters excluded every candidate result"
    };

    if spec.allow_vacuous {
        AssertionOutcome {
            exit: ExitCode::Ok,
            vacuous: true,
            warning: Some(format!(
                "warning: --assert-empty passed vacuously ({reason}); allowed by --allow-vacuous"
            )),
        }
    } else {
        AssertionOutcome {
            exit: ExitCode::Vacuous,
            vacuous: true,
            warning: Some(format!(
                "warning: --assert-empty passed vacuously ({reason}); exit 4 (suppress with --allow-vacuous)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(filtered: usize, matched: bool, unfiltered: usize) -> ResultFacts {
        ResultFacts {
            filtered_count: filtered,
            any_symbol_matched: matched,
            unfiltered_count: unfiltered,
        }
    }

    #[test]
    fn no_assertion_is_always_ok() {
        let out = evaluate(AssertionSpec::default(), facts(5, true, 5));
        assert_eq!(out.exit, ExitCode::Ok);
        assert!(!out.vacuous);
    }

    #[test]
    fn assert_empty_with_results_fails() {
        let spec = AssertionSpec {
            assert_empty: true,
            allow_vacuous: false,
        };
        let out = evaluate(spec, facts(3, true, 3));
        assert_eq!(out.exit, ExitCode::AssertionFailed);
    }

    #[test]
    fn assert_empty_genuinely_empty_passes() {
        let spec = AssertionSpec {
            assert_empty: true,
            allow_vacuous: false,
        };
        // Symbols matched, unfiltered also had zero results: a real, non-vacuous pass.
        let out = evaluate(spec, facts(0, true, 0));
        assert_eq!(out.exit, ExitCode::Ok);
        assert!(!out.vacuous);
    }

    #[test]
    fn zero_symbol_match_is_vacuous_exit_4() {
        let spec = AssertionSpec {
            assert_empty: true,
            allow_vacuous: false,
        };
        let out = evaluate(spec, facts(0, false, 0));
        assert_eq!(out.exit, ExitCode::Vacuous);
        assert!(out.vacuous);
    }

    #[test]
    fn filter_excluded_all_is_vacuous_exit_4() {
        let spec = AssertionSpec {
            assert_empty: true,
            allow_vacuous: false,
        };
        // Pattern matched, unfiltered query had 7 results, filters excluded all.
        let out = evaluate(spec, facts(0, true, 7));
        assert_eq!(out.exit, ExitCode::Vacuous);
        assert!(out.vacuous);
    }

    #[test]
    fn allow_vacuous_downgrades_to_ok_but_keeps_flag() {
        let spec = AssertionSpec {
            assert_empty: true,
            allow_vacuous: true,
        };
        let out = evaluate(spec, facts(0, false, 0));
        assert_eq!(out.exit, ExitCode::Ok);
        assert!(out.vacuous);
        assert!(out.warning.is_some());
    }
}
