//! Per-language tokenizers and their compile-time registry.
//!
//! Each tokenizer is the sole authority on its own lexing (dispatch §3): it splits
//! the raw argv string strictly on its language's separator and validates each
//! segment against that language's identifier grammar, rejecting the parse on any
//! illegal character. There is **no** central separator→language table; the driver
//! optimistically attempts every registered tokenizer. Adding a language is a new
//! [`inventory::submit!`] line — no central list to edit.
//!
//! The five families share one [`SeparatorTokenizer`] parameterized by separator;
//! Go is the one structural specialization (separator `/`, but a host segment
//! carries a literal `.`).

use crate::diagnostics::RejectReason;
use crate::family::Family;
use crate::grammar::{parse_segment, CharClass, Pattern, PatternSeg};

/// A raw selector lexer for one family.
pub trait Tokenizer: Sync {
    /// The family this tokenizer stamps onto its parses.
    fn family(&self) -> Family;

    /// Attempt to lex `raw` into the uniform `::`-segmented [`Pattern`]. Rejection
    /// contributes no interpretation to the multi-parse union.
    fn tokenize(&self, raw: &str) -> Result<TokenizeOutput, RejectReason>;
}

/// A clean parse plus any parse-time rewrites (surfaced as diagnostics upstream).
#[derive(Debug)]
pub struct TokenizeOutput {
    pub pattern: Pattern,
    /// `(original_segment, rewritten_segment)` for every `!*` → `*!` rewrite.
    pub rewrites: Vec<(String, String)>,
}

/// A registry entry. Collected by [`inventory`] at link time.
pub struct TokenizerRegistration {
    pub tokenizer: &'static (dyn Tokenizer + Sync),
}

impl std::fmt::Debug for TokenizerRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenizerRegistration")
            .field("family", &self.tokenizer.family())
            .finish()
    }
}

inventory::collect!(TokenizerRegistration);

/// All registered tokenizers, sorted by [`Family`] for deterministic output
/// regardless of `inventory` link order.
pub fn registered() -> Vec<&'static (dyn Tokenizer + Sync)> {
    let mut v: Vec<&'static (dyn Tokenizer + Sync)> = inventory::iter::<TokenizerRegistration>
        .into_iter()
        .map(|r| r.tokenizer)
        .collect();
    v.sort_by_key(|t| t.family());
    v
}

/// The separator a family splits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sep {
    ColonColon,
    Dot,
    Slash,
}

impl Sep {
    fn as_str(self) -> &'static str {
        match self {
            Sep::ColonColon => "::",
            Sep::Dot => ".",
            Sep::Slash => "/",
        }
    }
}

/// Structural mode — the only genuine per-language specialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Plain separator split (Java / TypeScript / Python).
    Plain,
    /// `::` family: a leading `::` is a root anchor, not an empty segment.
    RustAnchored,
    /// Go: separator `/`; segments may carry a literal `.` (host names).
    GoHost,
}

/// The shared separator lexer (dispatch §3). Const-constructible so each family's
/// instance can be a `static` referenced from an [`inventory::submit!`].
pub struct SeparatorTokenizer {
    family: Family,
    sep: Sep,
    mode: Mode,
    class: CharClass,
}

impl std::fmt::Debug for SeparatorTokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeparatorTokenizer")
            .field("family", &self.family)
            .finish()
    }
}

impl SeparatorTokenizer {
    const fn new(family: Family, sep: Sep, mode: Mode, class: CharClass) -> Self {
        SeparatorTokenizer {
            family,
            sep,
            mode,
            class,
        }
    }

    /// Split `raw` on this family's separator, respecting `(...)` depth so a
    /// separator inside an alternation group does not split.
    fn split(&self, raw: &str) -> Result<Vec<String>, RejectReason> {
        let sep = self.sep.as_str();
        let sep_chars: Vec<char> = sep.chars().collect();
        let chars: Vec<char> = raw.chars().collect();
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut depth: i32 = 0;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '(' => {
                    depth += 1;
                    cur.push(c);
                    i += 1;
                }
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(RejectReason::Malformed {
                            detail: "unbalanced `)`".to_string(),
                        });
                    }
                    cur.push(c);
                    i += 1;
                }
                _ if depth == 0 && at_sep(&chars, i, &sep_chars) => {
                    out.push(std::mem::take(&mut cur));
                    i += sep_chars.len();
                }
                _ => {
                    cur.push(c);
                    i += 1;
                }
            }
        }
        if depth != 0 {
            return Err(RejectReason::Malformed {
                detail: "unbalanced `(`".to_string(),
            });
        }
        out.push(cur);
        Ok(out)
    }
}

fn at_sep(chars: &[char], i: usize, sep: &[char]) -> bool {
    if i + sep.len() > chars.len() {
        return false;
    }
    chars[i..i + sep.len()] == *sep
}

impl Tokenizer for SeparatorTokenizer {
    fn family(&self) -> Family {
        self.family
    }

    fn tokenize(&self, raw: &str) -> Result<TokenizeOutput, RejectReason> {
        if raw.is_empty() {
            return Err(RejectReason::EmptySegment);
        }
        let mut segments = self.split(raw)?;

        // `::` family: a leading empty segment is the root/global anchor, not an
        // illegal empty segment. The whole pattern is already anchored, so drop it.
        if self.mode == Mode::RustAnchored
            && segments.len() > 1
            && segments.first().is_some_and(|s| s.is_empty())
        {
            segments.remove(0);
        }

        let mut out_segments: Vec<PatternSeg> = Vec::with_capacity(segments.len());
        let mut rewrites: Vec<(String, String)> = Vec::new();
        for seg in &segments {
            if seg.is_empty() {
                return Err(RejectReason::EmptySegment);
            }
            if seg == "**" {
                out_segments.push(PatternSeg::Globstar);
                continue;
            }
            let parsed = parse_segment(seg, self.class)?;
            if let Some(rw) = parsed.rewrite {
                rewrites.push(rw);
            }
            out_segments.push(PatternSeg::Segment(parsed.matcher));
        }

        Ok(TokenizeOutput {
            pattern: Pattern {
                segments: out_segments,
            },
            rewrites,
        })
    }
}

// --- The five registered tokenizers. A new language is one more block. ---

static RUST_TOKENIZER: SeparatorTokenizer = SeparatorTokenizer::new(
    Family::Rust,
    Sep::ColonColon,
    Mode::RustAnchored,
    CharClass::Rust,
);
static GO_TOKENIZER: SeparatorTokenizer =
    SeparatorTokenizer::new(Family::Go, Sep::Slash, Mode::GoHost, CharClass::Go);
static TS_TOKENIZER: SeparatorTokenizer =
    SeparatorTokenizer::new(Family::TypeScript, Sep::Dot, Mode::Plain, CharClass::JavaTs);
static JAVA_TOKENIZER: SeparatorTokenizer =
    SeparatorTokenizer::new(Family::Java, Sep::Dot, Mode::Plain, CharClass::JavaTs);
static PYTHON_TOKENIZER: SeparatorTokenizer =
    SeparatorTokenizer::new(Family::Python, Sep::Dot, Mode::Plain, CharClass::Python);

inventory::submit! { TokenizerRegistration { tokenizer: &RUST_TOKENIZER } }
inventory::submit! { TokenizerRegistration { tokenizer: &GO_TOKENIZER } }
inventory::submit! { TokenizerRegistration { tokenizer: &TS_TOKENIZER } }
inventory::submit! { TokenizerRegistration { tokenizer: &JAVA_TOKENIZER } }
inventory::submit! { TokenizerRegistration { tokenizer: &PYTHON_TOKENIZER } }
