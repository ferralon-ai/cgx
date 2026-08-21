//! A Thompson NFA whose transitions consume exactly one FQN segment, plus its
//! subset simulation. Compiled once per parse; O(|pattern|) states, flat — no
//! alternation×depth expansion, no regex (dispatch §5).

use crate::grammar::{Pattern, PatternSeg, SegmentMatcher};
use std::collections::BTreeSet;

/// A segment-consuming edge predicate.
#[derive(Debug, Clone)]
enum EdgeMatcher {
    /// Consume any one segment (a globstar self-loop).
    Any,
    /// Consume one segment iff it satisfies the matcher.
    Seg(SegmentMatcher),
}

impl EdgeMatcher {
    fn matches(&self, seg: &str) -> bool {
        match self {
            EdgeMatcher::Any => true,
            EdgeMatcher::Seg(m) => m.matches(seg),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct State {
    /// Forward epsilon transitions.
    eps: Vec<usize>,
    /// Segment-consuming transitions `(predicate, target)`.
    edges: Vec<(EdgeMatcher, usize)>,
}

/// The outcome of simulating one node's segment sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimOutcome {
    /// An accepting state was active after consuming all segments.
    pub matched: bool,
    /// The active-state cap tripped: `matched` may be a false negative.
    pub truncated: bool,
}

/// A compiled selector automaton.
#[derive(Debug, Clone)]
pub struct Nfa {
    states: Vec<State>,
    start: usize,
    accept: usize,
}

impl Nfa {
    /// Compile a [`Pattern`] into a segment-consuming Thompson NFA.
    pub fn compile(pattern: &Pattern) -> Nfa {
        let mut states = vec![State::default()];
        let start = 0usize;
        let mut cur = start;

        for seg in &pattern.segments {
            match seg {
                PatternSeg::Globstar => {
                    // Self-loop on any segment (consume one, stay) + epsilon forward
                    // (consume zero). Classic globstar; no depth expansion.
                    states[cur].edges.push((EdgeMatcher::Any, cur));
                    let next = states.len();
                    states.push(State::default());
                    states[cur].eps.push(next);
                    cur = next;
                }
                PatternSeg::Segment(m) => {
                    let next = states.len();
                    states.push(State::default());
                    states[cur].edges.push((EdgeMatcher::Seg(m.clone()), next));
                    cur = next;
                }
            }
        }
        Nfa {
            states,
            start,
            accept: cur,
        }
    }

    /// Run the NFA over a node's `::`-split segments via subset simulation.
    /// Anchored at both ends: acceptance requires all segments consumed with the
    /// accept state active. Caps the active set at `cap` and reports truncation
    /// honestly rather than silently dropping states.
    pub fn simulate(&self, segments: &[&str], cap: usize) -> SimOutcome {
        let mut truncated = false;
        let mut active: BTreeSet<usize> = BTreeSet::new();
        self.eps_closure(self.start, &mut active);

        for seg in segments {
            let mut next: BTreeSet<usize> = BTreeSet::new();
            for &s in &active {
                for (matcher, target) in &self.states[s].edges {
                    if matcher.matches(seg) {
                        self.eps_closure(*target, &mut next);
                    }
                }
            }
            if next.len() > cap {
                truncated = true;
                // Retain a bounded, deterministic prefix of the active set.
                while next.len() > cap {
                    let last = *next.iter().next_back().expect("non-empty");
                    next.remove(&last);
                }
            }
            active = next;
            if active.is_empty() {
                break;
            }
        }
        SimOutcome {
            matched: active.contains(&self.accept),
            truncated,
        }
    }

    /// Add `state` and everything reachable from it by epsilon transitions.
    fn eps_closure(&self, state: usize, set: &mut BTreeSet<usize>) {
        if set.insert(state) {
            for &e in &self.states[state].eps {
                self.eps_closure(e, set);
            }
        }
    }
}
