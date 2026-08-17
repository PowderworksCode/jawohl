//! A prefix trie over the members of an `enum` (or the single value of a
//! `const`).
//!
//! This is what makes `enum` one of the constraints that *can* be rejected
//! early, exactly and soundly: while a string is still arriving, ask whether
//! any member still has the text so far as a prefix. `"role":"sup"` against
//! `["user","admin"]` is already impossible — no member begins `sup` — and the
//! caller can cancel generation without waiting for the closing quote.
//!
//! Only string members participate. A non-string member (`enum: [1, true]`)
//! cannot be matched against a partial *string*, so it is kept for the final
//! equality check and simply never contributes a live prefix.

/// Members of an `enum`, indexed for prefix queries.
#[derive(Debug, Clone, Default)]
pub struct EnumTrie {
    /// The string members, sorted so a prefix query is a binary search.
    strings: Vec<String>,
    /// True if the enum has any non-string member, in which case a value that
    /// is not a string may still be valid and the trie cannot rule it out.
    has_non_strings: bool,
}

impl EnumTrie {
    pub(crate) fn new(strings: Vec<String>, has_non_strings: bool) -> Self {
        let mut strings = strings;
        strings.sort();
        strings.dedup();
        EnumTrie {
            strings,
            has_non_strings,
        }
    }

    /// Could a string beginning `prefix` still be a member?
    ///
    /// Sound in both directions: `true` means some member has this prefix,
    /// `false` means none can, whatever arrives next.
    pub fn prefix_is_live(&self, prefix: &str) -> bool {
        // The sorted position of `prefix` is where a member starting with it
        // would be; only that member can extend it.
        match self.strings.binary_search_by(|m| m.as_str().cmp(prefix)) {
            Ok(_) => true,
            Err(i) => self
                .strings
                .get(i)
                .is_some_and(|m| m.as_str().starts_with(prefix)),
        }
    }

    /// Is `value` exactly a member?
    pub fn contains(&self, value: &str) -> bool {
        self.strings
            .binary_search_by(|m| m.as_str().cmp(value))
            .is_ok()
    }

    /// True if some member is not a string, so a non-string value cannot be
    /// ruled out by this trie alone.
    pub fn has_non_strings(&self) -> bool {
        self.has_non_strings
    }

    pub fn string_members(&self) -> &[String] {
        &self.strings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trie(members: &[&str]) -> EnumTrie {
        EnumTrie::new(members.iter().map(|s| s.to_string()).collect(), false)
    }

    #[test]
    fn live_prefixes() {
        let t = trie(&["user", "admin"]);
        for live in ["", "u", "us", "user", "a", "ad", "admin"] {
            assert!(t.prefix_is_live(live), "{live:?} should be live");
        }
        // the design's own example
        for dead in ["sup", "x", "userx", "adminx", "b"] {
            assert!(!t.prefix_is_live(dead), "{dead:?} should be dead");
        }
    }

    #[test]
    fn shared_prefixes() {
        let t = trie(&["abc", "abd", "xyz"]);
        assert!(t.prefix_is_live("ab"));
        assert!(t.prefix_is_live("abc"));
        assert!(!t.prefix_is_live("abe"));
        assert!(t.contains("abd"));
        assert!(!t.contains("ab"));
    }

    #[test]
    fn empty_enum_admits_nothing() {
        let t = trie(&[]);
        assert!(!t.prefix_is_live(""));
        assert!(!t.contains("a"));
    }
}
