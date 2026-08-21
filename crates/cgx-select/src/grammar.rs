//! The uniform `::`-segmented pattern grammar, its parser, and the per-segment
//! affix-constraint matcher.
//!
//! Terminology (dispatch §2). A [`Pattern`] is an ordered list of [`PatternSeg`]s:
//! a `**` globstar, or a single-segment [`SegmentMatcher`]. A segment matcher is a
//! sequence of [`Piece`]s — `*` stars interleaved with [`RunPred`] *runs* (a run is
//! a maximal star-free concatenation of literals/alternation groups). A run's
//! surrounding stars fix its *affix constraint* (`Equals`/`StartsWith`/`EndsWith`/
//! `Contains`); `!` inverts exactly that constraint, siblings still AND.
//!
//! This grammar is **not** glob and **not** regex — it is package pattern matching.
//! In particular `(a|b)*` means *starts with a or b*, never "zero-or-more of (a|b)".
//! Unlike the coexisting [`SymbolPattern`](cgx_core::pattern::SymbolPattern) glob,
//! this grammar drops `?`.

use crate::diagnostics::RejectReason;

/// The character class a family admits inside a literal segment atom. Grammar
/// metacharacters (`* ( ) | !`) are always structural; only literal runs are
/// validated against this class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    /// Rust: `[A-Za-z0-9_]`.
    Rust,
    /// Java / TypeScript: identifier chars plus `$`.
    JavaTs,
    /// Python: identifier chars.
    Python,
    /// Go: identifier chars plus `.` and `-` (host segments carry a literal dot).
    Go,
}

impl CharClass {
    fn admits(self, c: char) -> bool {
        let base = c.is_ascii_alphanumeric() || c == '_';
        match self {
            CharClass::Rust | CharClass::Python => base,
            CharClass::JavaTs => base || c == '$',
            CharClass::Go => base || c == '.' || c == '-',
        }
    }
}

/// A whole selector pattern in the uniform internal form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub segments: Vec<PatternSeg>,
}

/// One element of a [`Pattern`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternSeg {
    /// `**` — zero-or-more whole segments (globstar across depth).
    Globstar,
    /// A matcher consuming exactly one segment.
    Segment(SegmentMatcher),
}

/// A single-segment matcher: stars and runs in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentMatcher {
    pieces: Vec<Piece>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    /// `*` — zero-or-more chars within the segment.
    Star,
    /// A star-free run of concatenated atoms, reduced to a match predicate.
    Run(RunPred),
}

/// A run reduced to a match predicate over one segment substring.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RunPred {
    /// The cross-product of the run's literal/group alternatives, optionally
    /// negated as a whole (`!seg`, `!(a|b)`).
    Set { options: Vec<String>, negated: bool },
    /// The niche standalone `(!a|b)` form: per-option polarity, OR semantics.
    MixedGroup(Vec<GroupOpt>),
}

/// One alternative inside an alternation group, with its own polarity (for the
/// niche `(!a|b)` form).
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupOpt {
    text: String,
    negated: bool,
}

/// The affix position a run occupies, derived from its surrounding stars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Affix {
    Equals,
    StartsWith,
    EndsWith,
    Contains,
}

impl SegmentMatcher {
    /// Whether one FQN segment satisfies this matcher. Pure, O(segment length ×
    /// alternatives); no backtracking.
    pub fn matches(&self, seg: &str) -> bool {
        let leading_star = matches!(self.pieces.first(), Some(Piece::Star));
        let trailing_star = matches!(self.pieces.last(), Some(Piece::Star));
        let runs: Vec<&RunPred> = self
            .pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Run(r) => Some(r),
                Piece::Star => None,
            })
            .collect();
        let k = runs.len();
        if k == 0 {
            // Only stars (a full-segment `*`): matches any single segment.
            return true;
        }
        for (i, run) in runs.iter().enumerate() {
            let before = i > 0 || leading_star;
            let after = i < k - 1 || trailing_star;
            let affix = match (before, after) {
                (false, false) => Affix::Equals,
                (false, true) => Affix::StartsWith,
                (true, false) => Affix::EndsWith,
                (true, true) => Affix::Contains,
            };
            if !run.eval(affix, seg) {
                return false;
            }
        }
        true
    }
}

impl RunPred {
    fn eval(&self, affix: Affix, seg: &str) -> bool {
        match self {
            RunPred::Set { options, negated } => {
                let base = options.iter().any(|o| affix_hit(affix, o, seg));
                negated ^ base
            }
            RunPred::MixedGroup(opts) => opts.iter().any(|o| {
                let base = affix_hit(affix, &o.text, seg);
                o.negated ^ base
            }),
        }
    }
}

fn affix_hit(affix: Affix, needle: &str, seg: &str) -> bool {
    match affix {
        Affix::Equals => seg == needle,
        Affix::StartsWith => seg.starts_with(needle),
        Affix::EndsWith => seg.ends_with(needle),
        Affix::Contains => seg.contains(needle),
    }
}

/// The result of parsing one raw segment string.
#[derive(Debug)]
pub struct SegmentParse {
    pub matcher: SegmentMatcher,
    /// `Some((original, rewritten))` when a degenerate `!*` was rewritten to `*!`.
    pub rewrite: Option<(String, String)>,
}

/// Parse one segment string (already split off the separator) under `class`.
/// A segment equal to `**` must be handled by the caller as a [`PatternSeg::Globstar`]
/// before reaching here.
pub fn parse_segment(raw: &str, class: CharClass) -> Result<SegmentParse, RejectReason> {
    let (working, rewritten) = rewrite_bang_star(raw);
    let matcher = parse_segment_body(&working, class)?;
    let rewrite = if rewritten {
        Some((raw.to_string(), working.clone()))
    } else {
        None
    };
    Ok(SegmentParse { matcher, rewrite })
}

/// Move any `!` that immediately precedes a run of `*` to just after that run
/// (`!*Test` → `*!Test`). Returns the rewritten string and whether it changed.
fn rewrite_bang_star(raw: &str) -> (String, bool) {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '!' && i + 1 < chars.len() && chars[i + 1] == '*' {
            changed = true;
            i += 1; // skip the `!`
            while i < chars.len() && chars[i] == '*' {
                out.push('*');
                i += 1;
            }
            out.push('!'); // reattach past the wildcard run
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    (out, changed)
}

/// Parse a segment body that no longer contains a `!*` sequence.
fn parse_segment_body(s: &str, class: CharClass) -> Result<SegmentMatcher, RejectReason> {
    let chars: Vec<char> = s.chars().collect();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut atoms: Vec<Atom> = Vec::new();
    let mut lit = String::new();
    let mut run_negated = false;
    let mut i = 0;

    // Flush the accumulated literal buffer into an atom.
    macro_rules! flush_lit {
        () => {
            if !lit.is_empty() {
                atoms.push(Atom::Lit(std::mem::take(&mut lit)));
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            '?' => return Err(RejectReason::QuestionMark { pos: i }),
            '*' => {
                flush_lit!();
                flush_run(&mut pieces, &mut atoms, &mut run_negated)?;
                push_star(&mut pieces);
                i += 1;
            }
            '!' => {
                // Only valid at the start of a run (before any atom).
                if !lit.is_empty() || !atoms.is_empty() || run_negated {
                    return Err(RejectReason::Malformed {
                        detail: "`!` must prefix a literal or `(...)` group".to_string(),
                    });
                }
                run_negated = true;
                i += 1;
            }
            '(' => {
                flush_lit!();
                let (group, consumed) = parse_group(&chars[i..], class)?;
                atoms.push(group);
                i += consumed;
            }
            ')' | '|' => {
                return Err(RejectReason::Malformed {
                    detail: format!("unexpected {c:?} outside a group"),
                });
            }
            _ if class.admits(c) => {
                lit.push(c);
                i += 1;
            }
            _ => return Err(RejectReason::IllegalChar { ch: c, pos: i }),
        }
    }
    flush_lit!();
    flush_run(&mut pieces, &mut atoms, &mut run_negated)?;

    Ok(SegmentMatcher { pieces })
}

#[derive(Debug)]
enum Atom {
    Lit(String),
    Group(Vec<GroupOpt>),
}

/// Parse a `(a|b|...)` group starting at `chars[0] == '('`. Returns the atom and
/// the number of chars consumed (including both parens).
fn parse_group(chars: &[char], class: CharClass) -> Result<(Atom, usize), RejectReason> {
    debug_assert_eq!(chars[0], '(');
    let mut opts: Vec<GroupOpt> = Vec::new();
    let mut cur = String::new();
    let mut negated = false;
    let mut started_opt = false;
    let mut i = 1;
    loop {
        if i >= chars.len() {
            return Err(RejectReason::Malformed {
                detail: "unbalanced `(` in alternation group".to_string(),
            });
        }
        let c = chars[i];
        match c {
            ')' => {
                push_opt(&mut opts, &mut cur, &mut negated, started_opt)?;
                if opts.is_empty() {
                    return Err(RejectReason::Malformed {
                        detail: "empty alternation group `()`".to_string(),
                    });
                }
                return Ok((Atom::Group(opts), i + 1));
            }
            '|' => {
                push_opt(&mut opts, &mut cur, &mut negated, started_opt)?;
                started_opt = false;
                i += 1;
            }
            '!' if !started_opt && cur.is_empty() && !negated => {
                negated = true;
                started_opt = true;
                i += 1;
            }
            '(' => {
                return Err(RejectReason::Malformed {
                    detail: "nested alternation groups are not supported".to_string(),
                });
            }
            '*' | '?' => {
                return Err(RejectReason::Malformed {
                    detail: format!("wildcard {c:?} inside an alternation group"),
                });
            }
            _ if class.admits(c) => {
                cur.push(c);
                started_opt = true;
                i += 1;
            }
            _ => return Err(RejectReason::IllegalChar { ch: c, pos: i }),
        }
    }
}

fn push_opt(
    opts: &mut Vec<GroupOpt>,
    cur: &mut String,
    negated: &mut bool,
    started_opt: bool,
) -> Result<(), RejectReason> {
    if cur.is_empty() && !started_opt {
        return Err(RejectReason::Malformed {
            detail: "empty alternative in group".to_string(),
        });
    }
    if cur.is_empty() {
        // `!` with no text, or an empty option like `(a|)`.
        return Err(RejectReason::Malformed {
            detail: "empty alternative in group".to_string(),
        });
    }
    opts.push(GroupOpt {
        text: std::mem::take(cur),
        negated: *negated,
    });
    *negated = false;
    Ok(())
}

fn push_star(pieces: &mut Vec<Piece>) {
    // Collapse consecutive stars.
    if matches!(pieces.last(), Some(Piece::Star)) {
        return;
    }
    pieces.push(Piece::Star);
}

/// Reduce the accumulated atoms into a [`RunPred`] and push it.
fn flush_run(
    pieces: &mut Vec<Piece>,
    atoms: &mut Vec<Atom>,
    run_negated: &mut bool,
) -> Result<(), RejectReason> {
    if atoms.is_empty() {
        // A dangling `!` (e.g. from a `!*` rewrite with nothing after) is dropped.
        *run_negated = false;
        return Ok(());
    }
    let negated = std::mem::take(run_negated);
    let taken = std::mem::take(atoms);

    // Niche: a standalone group carrying per-option negation.
    if taken.len() == 1 {
        if let Atom::Group(opts) = &taken[0] {
            if opts.iter().any(|o| o.negated) {
                if negated {
                    return Err(RejectReason::Malformed {
                        detail: "`!` cannot prefix a group that already negates options"
                            .to_string(),
                    });
                }
                pieces.push(Piece::Run(RunPred::MixedGroup(opts.clone())));
                return Ok(());
            }
        }
    }

    // General case: cross-product expansion of positive alternatives.
    let mut strings: Vec<String> = vec![String::new()];
    for atom in &taken {
        match atom {
            Atom::Lit(s) => {
                for p in &mut strings {
                    p.push_str(s);
                }
            }
            Atom::Group(opts) => {
                if opts.iter().any(|o| o.negated) {
                    return Err(RejectReason::Malformed {
                        detail: "negated option in a concatenated group is unsupported".to_string(),
                    });
                }
                let mut expanded = Vec::with_capacity(strings.len() * opts.len());
                for p in &strings {
                    for o in opts {
                        expanded.push(format!("{p}{}", o.text));
                    }
                }
                strings = expanded;
            }
        }
    }
    pieces.push(Piece::Run(RunPred::Set {
        options: strings,
        negated,
    }));
    Ok(())
}
