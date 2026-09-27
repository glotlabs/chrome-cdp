//! A tab's refs: `e<n>` ↔ the node it names.
//!
//! Milestones 4.1 and 4.3 need only the numbering: a node keeps its ref
//! for as long as its tab lives, a new node gets the next number, and the
//! counter never goes back, so a number is never reused for another
//! node. Milestone 4.2 builds on this: invalidation on navigation, stale
//! and detached errors, and the role+name fallback (hence the role and
//! name kept with each ref).

use std::collections::HashMap;

use super::tree::NodeKey;

/// What a ref was given to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefEntry {
    pub(crate) key: NodeKey,
    pub(crate) role: String,
    pub(crate) name: String,
}

/// One tab's refs.
#[derive(Debug, Default)]
pub(crate) struct Refs {
    /// The last number given.
    last: u32,
    by_key: HashMap<NodeKey, u32>,
    by_number: HashMap<u32, RefEntry>,
}

impl Refs {
    /// The ref of `key`, given now if it has none. Updates the role and
    /// name kept with it.
    pub(crate) fn assign(&mut self, key: &NodeKey, role: &str, name: &str) -> u32 {
        let number = if let Some(&number) = self.by_key.get(key) {
            number
        } else {
            self.last += 1;
            self.by_key.insert(key.clone(), self.last);
            self.last
        };
        let entry = RefEntry {
            key: key.clone(),
            role: role.to_owned(),
            name: name.to_owned(),
        };
        self.by_number.insert(number, entry);
        number
    }

    /// The ref `key` has, if any.
    pub(crate) fn get(&self, key: &NodeKey) -> Option<u32> {
        self.by_key.get(key).copied()
    }

    /// What ref `number` names.
    pub(crate) fn entry(&self, number: u32) -> Option<&RefEntry> {
        self.by_number.get(&number)
    }
}

/// Parses a ref as typed: `e12` or `@e12`.
pub(crate) fn parse_ref(text: &str) -> Option<u32> {
    let digits = text.trim().strip_prefix('@').unwrap_or(text.trim());
    let digits = digits.strip_prefix('e')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::{NodeKey, Refs, parse_ref};

    fn key(backend: i64) -> NodeKey {
        NodeKey {
            frame: "F".into(),
            backend,
        }
    }

    #[test]
    fn a_node_keeps_its_ref_and_numbers_never_repeat() {
        let mut refs = Refs::default();
        assert_eq!(refs.assign(&key(10), "button", "Save"), 1);
        assert_eq!(refs.assign(&key(11), "link", "Home"), 2);
        assert_eq!(refs.assign(&key(10), "button", "Saved"), 1);
        assert_eq!(refs.entry(1).unwrap().name, "Saved");
        assert_eq!(refs.get(&key(11)), Some(2));
        assert_eq!(refs.get(&key(12)), None);
        let other = NodeKey {
            frame: "G".into(),
            backend: 10,
        };
        assert_eq!(refs.assign(&other, "button", "Pay"), 3);
    }

    #[test]
    fn refs_as_typed() {
        assert_eq!(parse_ref("e12"), Some(12));
        assert_eq!(parse_ref("@e3"), Some(3));
        for bad in ["e", "e0", "12", "ee1", "e1x", "#e1", "e-1"] {
            assert_eq!(parse_ref(bad), None, "{bad}");
        }
    }
}
