//! `pattern`, compiled for prefix queries.
//!
//! # Why early rejection needs an anchor
//!
//! JSON Schema's `pattern` is an **unanchored** search: the value is valid if
//! the regex matches *anywhere* in it. That has a consequence which is easy to
//! get wrong — for an unanchored pattern, **no prefix is ever irrecoverable**.
//! Whatever text has arrived, more can always be appended that contains a
//! match, so a partial value can never be ruled out.
//!
//! Early rejection is therefore sound only when the pattern is anchored at the
//! start (`^…`). Then the match must begin at byte zero, the anchored DFA can
//! enter a dead state, and a dead state genuinely means "no continuation can
//! satisfy this".
//!
//! Patterns without a leading `^` are still compiled and still checked when the
//! value completes; they simply contribute no early verdict, and the lowering
//! report says so rather than leaving the caller to assume otherwise.

use regex_automata::{
    dfa::{dense, Automaton},
    Anchored, Input,
};

/// A compiled `pattern`.
#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    dfa: dense::DFA<Vec<u32>>,
    /// Whether a partial value can be rejected before it finishes — true only
    /// for `^`-anchored patterns (see the module docs).
    supports_early_rejection: bool,
}

impl Pattern {
    /// Compile a pattern. `None` when the regex does not compile, which the
    /// caller records in the lowering report rather than silently treating the
    /// value as valid.
    pub(crate) fn compile(source: &str) -> Option<Pattern> {
        Some(Pattern {
            source: source.to_string(),
            dfa: dense::DFA::new(source).ok()?,
            supports_early_rejection: source.starts_with('^'),
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// True when this pattern can rule a partial value out. See the module
    /// docs: only `^`-anchored patterns can.
    pub fn supports_early_rejection(&self) -> bool {
        self.supports_early_rejection
    }

    /// Could a string beginning `prefix` still satisfy this pattern?
    ///
    /// Conservative by construction: when no sound answer is available — an
    /// unanchored pattern — this returns `true`, so a value is never rejected
    /// on a guess.
    pub fn prefix_is_live(&self, prefix: &str) -> bool {
        if !self.supports_early_rejection {
            return true;
        }
        let input = Input::new(prefix).anchored(Anchored::Yes);
        let Ok(mut state) = self.dfa.start_state_forward(&input) else {
            return true;
        };
        for &b in prefix.as_bytes() {
            state = self.dfa.next_state(state, b);
            if self.dfa.is_dead_state(state) {
                return false;
            }
        }
        true
    }

    /// Does the completed value satisfy the pattern? Unanchored, per JSON
    /// Schema: a match anywhere in the string counts.
    pub fn matches(&self, value: &str) -> bool {
        self.dfa
            .try_search_fwd(&Input::new(value))
            .ok()
            .flatten()
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchored_patterns_reject_early() {
        let p = Pattern::compile("^[0-9]{3}-[0-9]{4}$").unwrap();
        assert!(p.supports_early_rejection());
        for live in ["", "5", "555", "555-", "555-12"] {
            assert!(p.prefix_is_live(live), "{live:?} should be live");
        }
        for dead in ["5a", "abc", "555-12345"] {
            assert!(!p.prefix_is_live(dead), "{dead:?} should be dead");
        }
        assert!(p.matches("555-1234"));
        assert!(!p.matches("555-123"));
    }

    #[test]
    fn unanchored_patterns_never_reject_early() {
        // Sound: "zzz" can still become "zzzfoo", which matches.
        let p = Pattern::compile("foo").unwrap();
        assert!(!p.supports_early_rejection());
        assert!(p.prefix_is_live("zzz"));
        assert!(p.prefix_is_live("anything at all"));
        assert!(p.matches("a foo b"));
        assert!(!p.matches("bar"));
    }

    #[test]
    fn anchored_prefix_of_an_alternation() {
        let p = Pattern::compile("^(cat|car|dog)$").unwrap();
        assert!(p.prefix_is_live("ca"));
        assert!(p.prefix_is_live("car"));
        assert!(!p.prefix_is_live("cab"));
        assert!(!p.prefix_is_live("e"));
        assert!(p.matches("dog"));
    }

    #[test]
    fn an_uncompilable_pattern_is_not_lowered() {
        assert!(Pattern::compile("[unclosed").is_none());
    }
}
