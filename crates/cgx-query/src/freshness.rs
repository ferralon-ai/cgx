//! The per-answer **index-freshness envelope**: how far the indexed state the
//! answer describes is from the working state on disk.
//!
//! The approximation contract ([`crate::contract`]) says which direction an answer
//! can be wrong *given the graph it was computed over*. This says what that graph
//! actually was, and whether the tree on disk has moved away from it — the second
//! half of the same honesty posture, and the question an agent asks after every
//! answer: *is this still true of my checkout?*
//!
//! ## Divergence, not age
//!
//! The envelope carries **no timestamp** — not the index build time, not an age,
//! not a "seconds since". Two reasons, in the order that decided it:
//!
//! 1. Age is the wrong primitive. An index built ten seconds ago against a tree
//!    since rewritten is stale; one built last week against an untouched tree is
//!    perfectly fresh. Divergence answers the question exactly; age only proxies it.
//! 2. A `now()`-derived field is nondeterministic output, and AR-10 makes
//!    byte-identical repeat answers a hard property of this system. Every field
//!    here is a pure function of (index state, working-tree state), so determinism
//!    holds by construction rather than by convention.
//!
//! A caller who genuinely wants age can compute it: the tree OID is emitted, and
//! resolving it to a time is a local git operation. cgx reports what it knows for
//! certain and declines to guess.
//!
//! ## `null` means "not established", never "zero"
//!
//! [`head_tree`](FreshnessEnvelope::head_tree)/[`matches_head`](FreshnessEnvelope::matches_head)
//! are `None` where no `HEAD` tree could be resolved (not a git repository, or a
//! repository with no commits), [`indexed_tree`](FreshnessEnvelope::indexed_tree)
//! is `None` where the answer's own graph key could not be recovered, and
//! [`dirty_files`](FreshnessEnvelope::dirty_files) is `None` where the surface did
//! not inspect the working tree at all. A surface that looked and found nothing
//! reports `Some(0)` — the two are deliberately distinguishable, the same way a
//! negative answer carries its scope.
//!
//! The same distinction leads the human line: it says `current` only when both
//! halves were established and both came back clean, `stale` when a divergence
//! was established, and `unknown` when something was never looked at. A `null`
//! must not render as the reassuring word.

use serde::Serialize;

/// How far the indexed state is from the working state (no wall-clock: this is
/// divergence, not age).
///
/// Construct with [`FreshnessEnvelope::new`] — [`stale`](Self::stale) and
/// [`matches_head`](Self::matches_head) are *derived* there and are never sourced
/// independently, so the one-glance boolean cannot drift from the facts under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FreshnessEnvelope {
    /// The Layer-2 key of the graph this answer was computed over: a git tree OID
    /// for an indexed committed tree, or a synthetic `workdir:<digest>` key when
    /// the graph came from a working-directory index whose content is not some
    /// committed tree's. `null` only where the surface could not recover its own
    /// graph key.
    pub indexed_tree: Option<String>,
    /// The tree OID of the current `HEAD` commit; `null` when no `HEAD` tree could
    /// be resolved (not a git repository, or no commits yet).
    pub head_tree: Option<String>,
    /// Whether [`indexed_tree`](Self::indexed_tree) is the current `HEAD` tree.
    /// `false` means the index is pinned to, or lagging behind, a different commit.
    /// `null` when either tree is unknown — never `false`, which would assert a
    /// divergence nobody established.
    pub matches_head: Option<bool>,
    /// How many working-tree files diverge from the indexed tree (content differs,
    /// or the file was added/removed). `null` when the surface did not inspect the
    /// working tree; `0` when it did and found none.
    ///
    /// Ignored paths are excluded and nested repositories (submodules) are not
    /// descended into, matching what the index itself covers. The exclude rules
    /// git applies include machine-global ones — `$GIT_DIR/info/exclude`,
    /// `core.excludesFile`, `$XDG_CONFIG_HOME/git/ignore` — which are not part of
    /// the tree, so two checkouts identical byte-for-byte can report different
    /// counts on differently configured machines. Repeated runs on one machine are
    /// byte-stable (AR-10); the count is not portable across them.
    pub dirty_files: Option<usize>,
    /// The one-glance signal, **derived** from the fields above: `true` iff a
    /// divergence was actually established — the indexed tree is known not to be
    /// `HEAD`'s, or at least one working-tree file is known to differ. `false` is
    /// therefore "no divergence within what this envelope reports", not a claim
    /// about what was never looked at; the `null`s above say which is which.
    pub stale: bool,
}

impl FreshnessEnvelope {
    /// Build an envelope from the facts, deriving `matches_head` and `stale`.
    ///
    /// `head_tree`/`dirty_files` are `None` where the caller could not resolve
    /// `HEAD` / did not inspect the working tree (see the module docs).
    pub fn new(
        indexed_tree: Option<String>,
        head_tree: Option<String>,
        dirty_files: Option<usize>,
    ) -> Self {
        let matches_head = match (&indexed_tree, &head_tree) {
            (Some(i), Some(h)) => Some(i == h),
            _ => None,
        };
        let stale = matches_head == Some(false) || dirty_files.is_some_and(|n| n > 0);
        FreshnessEnvelope {
            indexed_tree,
            head_tree,
            matches_head,
            dirty_files,
            stale,
        }
    }

    /// The envelope for an output shape that carries no freshness signal at all —
    /// the path-graph emitters, and the `search`/`symbols` bare JSON arrays.
    ///
    /// Every field is "not established", which is the literal truth for a surface
    /// that never looked: the caller must not pay for a working-tree walk whose
    /// result is discarded, and if this ever *is* rendered it reads `unknown`
    /// rather than a fabricated clean bill.
    pub fn uninspected() -> Self {
        Self::new(None, None, None)
    }

    /// The compact single-line human rendering, mirroring
    /// [`ApproximationContract::human_summary`](crate::ApproximationContract::human_summary):
    /// the verdict, the tree the answer describes, and only the qualifiers that
    /// actually apply.
    pub fn human_summary(&self) -> String {
        let mut s = format!(
            "freshness: {} | indexed tree {}",
            self.verdict(),
            match &self.indexed_tree {
                Some(t) => short_oid(t),
                None => "unknown".to_string(),
            }
        );
        match self.matches_head {
            Some(true) => {}
            Some(false) => {
                let head = self.head_tree.as_deref().unwrap_or("?");
                s.push_str(&format!(" (behind HEAD {})", short_oid(head)));
            }
            None => s.push_str(" (HEAD unknown)"),
        }
        match self.dirty_files {
            Some(0) => s.push_str(", working tree clean"),
            Some(n) => s.push_str(&format!(
                ", {n} dirty file{}",
                if n == 1 { "" } else { "s" }
            )),
            None => s.push_str(", working tree not inspected"),
        }
        s
    }

    /// The leading verdict word. `current` is reserved for the case where both
    /// halves were established *and* both came back clean; anything the surface
    /// did not look at yields `unknown`, never the reassuring word. `stale` is
    /// [`stale`](Self::stale) itself — a divergence that was established.
    fn verdict(&self) -> &'static str {
        match (self.stale, self.matches_head, self.dirty_files) {
            (true, _, _) => "stale",
            (false, Some(true), Some(0)) => "current",
            _ => "unknown",
        }
    }
}

/// The short form of a tree key for the human line, matching the MCP
/// `graph_version` convention (7 leading characters). A synthetic
/// `workdir:<digest>` key keeps its prefix so the form stays recognizable.
fn short_oid(key: &str) -> String {
    match key.strip_prefix("workdir:") {
        Some(digest) => format!("workdir:{}", digest.chars().take(7).collect::<String>()),
        None => key.chars().take(7).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The derived fields are a pure function of the three sourced ones — including
    /// the degenerate inputs (no HEAD, uninspected working tree, a pinned index
    /// ahead of/behind HEAD).
    #[test]
    fn derived_fields_follow_the_sourced_ones() {
        struct Case {
            name: &'static str,
            indexed: &'static str,
            head: Option<&'static str>,
            dirty: Option<usize>,
            want_matches_head: Option<bool>,
            want_stale: bool,
        }

        let cases = [
            Case {
                name: "clean tree at HEAD",
                indexed: "aaaa",
                head: Some("aaaa"),
                dirty: Some(0),
                want_matches_head: Some(true),
                want_stale: false,
            },
            Case {
                name: "at HEAD but dirty",
                indexed: "aaaa",
                head: Some("aaaa"),
                dirty: Some(3),
                want_matches_head: Some(true),
                want_stale: true,
            },
            Case {
                name: "behind HEAD, clean",
                indexed: "aaaa",
                head: Some("bbbb"),
                dirty: Some(0),
                want_matches_head: Some(false),
                want_stale: true,
            },
            Case {
                name: "behind HEAD and dirty",
                indexed: "aaaa",
                head: Some("bbbb"),
                dirty: Some(9),
                want_matches_head: Some(false),
                want_stale: true,
            },
            Case {
                name: "no HEAD resolvable",
                indexed: "aaaa",
                head: None,
                dirty: Some(0),
                want_matches_head: None,
                want_stale: false,
            },
            Case {
                name: "no HEAD, dirty files known",
                indexed: "aaaa",
                head: None,
                dirty: Some(1),
                want_matches_head: None,
                want_stale: true,
            },
            Case {
                name: "working tree not inspected",
                indexed: "aaaa",
                head: Some("aaaa"),
                dirty: None,
                want_matches_head: Some(true),
                want_stale: false,
            },
            Case {
                name: "not inspected but behind HEAD",
                indexed: "aaaa",
                head: Some("bbbb"),
                dirty: None,
                want_matches_head: Some(false),
                want_stale: true,
            },
        ];

        for c in cases {
            let env = FreshnessEnvelope::new(
                Some(c.indexed.to_owned()),
                c.head.map(str::to_owned),
                c.dirty,
            );
            assert_eq!(env.matches_head, c.want_matches_head, "{}", c.name);
            assert_eq!(env.stale, c.want_stale, "{}", c.name);
        }
    }

    #[test]
    fn json_shape_is_stable_and_distinguishes_null_from_zero() {
        let clean = FreshnessEnvelope::new(Some("aaaa".into()), Some("aaaa".into()), Some(0));
        let v = serde_json::to_value(&clean).unwrap();
        assert_eq!(v["dirty_files"], serde_json::json!(0));
        assert_eq!(v["matches_head"], serde_json::json!(true));
        assert_eq!(v["stale"], serde_json::json!(false));

        let uninspected = FreshnessEnvelope::new(None, None, None);
        let v = serde_json::to_value(&uninspected).unwrap();
        assert!(v["dirty_files"].is_null());
        assert!(v["head_tree"].is_null());
        assert!(v["matches_head"].is_null());
    }

    #[test]
    fn human_summary_states_the_qualifier_that_applies() {
        let s = FreshnessEnvelope::new(
            Some("abc1234def".into()),
            Some("abc1234def".into()),
            Some(0),
        )
        .human_summary();
        assert_eq!(
            s,
            "freshness: current | indexed tree abc1234, working tree clean"
        );

        let s = FreshnessEnvelope::new(
            Some("abc1234def".into()),
            Some("ffff567aaa".into()),
            Some(1),
        )
        .human_summary();
        assert_eq!(
            s,
            "freshness: stale | indexed tree abc1234 (behind HEAD ffff567), 1 dirty file"
        );

        let s = FreshnessEnvelope::new(Some("abc1234def".into()), None, None).human_summary();
        assert_eq!(
            s,
            "freshness: unknown | indexed tree abc1234 (HEAD unknown), working tree not inspected"
        );
    }

    /// The verdict word never says `current` for something nobody looked at. A
    /// `null` that renders as the reassuring word is the same defect class as a
    /// dirty count that reports a `0` it never established.
    #[test]
    fn the_verdict_word_says_current_only_when_both_halves_were_checked() {
        struct Case {
            name: &'static str,
            head: Option<&'static str>,
            dirty: Option<usize>,
            want: &'static str,
        }

        let cases = [
            Case {
                name: "checked both, both clean",
                head: Some("aaaa"),
                dirty: Some(0),
                want: "freshness: current",
            },
            Case {
                name: "checked both, tree dirty",
                head: Some("aaaa"),
                dirty: Some(3),
                want: "freshness: stale",
            },
            Case {
                name: "at HEAD, working tree never inspected",
                head: Some("aaaa"),
                dirty: None,
                want: "freshness: unknown",
            },
            Case {
                name: "clean tree, HEAD unresolvable",
                head: None,
                dirty: Some(0),
                want: "freshness: unknown",
            },
            Case {
                name: "nothing established at all",
                head: None,
                dirty: None,
                want: "freshness: unknown",
            },
        ];

        for c in cases {
            let s = FreshnessEnvelope::new(Some("aaaa".into()), c.head.map(str::to_owned), c.dirty)
                .human_summary();
            assert!(
                s.starts_with(c.want),
                "{}: wanted {:?}, got {s:?}",
                c.name,
                c.want
            );
        }
    }
}
