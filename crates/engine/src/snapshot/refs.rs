//! A tab's refs: `e<n>` ↔ the node it names (plan 04, milestone 4.2).
//!
//! A node keeps its ref for as long as its document lives, a new node gets
//! the next number, and the counter never goes back, so a number is never
//! reused for another node. When a frame loads a new document its refs
//! are dropped (all of them for the main frame) and become *stale*: using
//! one is `RefStale`, never a hit on whatever took its place.
//!
//! Each ref keeps the role and name it was shown with, for the role+name
//! fallback when its node is gone ([`super::resolve`]). A same-document
//! navigation (`pushState`, `#fragment`) keeps refs, since the nodes
//! may well be the same, but turns the fallback off for refs last shown
//! before it: after a route change, a node with the same role and name is
//! likely another page's.

use std::collections::{HashMap, HashSet};

use super::tree::NodeKey;

/// What a ref was given to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefEntry {
    pub(crate) key: NodeKey,
    pub(crate) role: String,
    pub(crate) name: String,
    /// Its frame's same-document navigations when the ref was last shown.
    route: u32,
}

/// What a ref number is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lookup {
    /// Given to a node of a document that is still loaded. The flag says
    /// whether the role+name fallback may be used.
    Live(RefEntry, bool),
    /// Given before the main frame loaded a new document.
    StalePage,
    /// Given before its (sub)frame loaded a new document.
    StaleFrame,
    /// Never given in this tab.
    Unknown,
}

/// One tab's refs.
#[derive(Debug, Default)]
pub(crate) struct Refs {
    /// The last number given.
    last: u32,
    by_key: HashMap<NodeKey, u32>,
    by_number: HashMap<u32, RefEntry>,
    /// Refs up to this number are from earlier documents of the main
    /// frame.
    cleared: u32,
    /// Refs above `cleared` dropped because their frame navigated.
    stale: HashSet<u32>,
    /// Same-document navigations per frame, for [`RefEntry::route`].
    routes: HashMap<String, u32>,
    /// Counts the documents loaded, in any frame.
    generation: u64,
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
            route: self.route(&key.frame),
        };
        self.by_number.insert(number, entry);
        number
    }

    /// The ref `key` has, if any.
    pub(crate) fn get(&self, key: &NodeKey) -> Option<u32> {
        self.by_key.get(key).copied()
    }

    /// What ref `number` names now.
    pub(crate) fn lookup(&self, number: u32) -> Lookup {
        if let Some(entry) = self.by_number.get(&number) {
            let fallback = entry.route == self.route(&entry.key.frame);
            Lookup::Live(entry.clone(), fallback)
        } else if number == 0 || number > self.last {
            Lookup::Unknown
        } else if number <= self.cleared {
            Lookup::StalePage
        } else {
            Lookup::StaleFrame
        }
    }

    /// Points ref `number` at `key`, the node the fallback found in place
    /// of its own. `key` gets the ref too unless it already has one.
    pub(crate) fn repoint(&mut self, number: u32, key: &NodeKey) {
        let Some(entry) = self.by_number.get_mut(&number) else {
            return;
        };
        if self.by_key.get(&entry.key) == Some(&number) {
            self.by_key.remove(&entry.key);
        }
        entry.key = key.clone();
        self.by_key.entry(key.clone()).or_insert(number);
    }

    /// Frames `frames` loaded new documents; `main` if one is the main
    /// frame, which drops every ref. Their refs become stale.
    pub(crate) fn navigated(&mut self, frames: &[String], main: bool) {
        self.generation += 1;
        if main {
            self.cleared = self.last;
            self.by_key.clear();
            self.by_number.clear();
            self.stale.clear();
            self.routes.clear();
            return;
        }
        let gone: Vec<u32> = self
            .by_number
            .iter()
            .filter(|(_, e)| frames.contains(&e.key.frame))
            .map(|(&number, _)| number)
            .collect();
        for number in gone {
            if let Some(entry) = self.by_number.remove(&number) {
                self.by_key.remove(&entry.key);
                self.stale.insert(number);
            }
        }
        for frame in frames {
            self.routes.remove(frame);
        }
    }

    /// Changes whenever a frame loads a new document: a snapshot read
    /// across a change may hold nodes of a document that is gone, which
    /// must not get refs.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Frame `frame` navigated within its document.
    pub(crate) fn route_changed(&mut self, frame: &str) {
        *self.routes.entry(frame.to_owned()).or_default() += 1;
    }

    fn route(&self, frame: &str) -> u32 {
        self.routes.get(frame).copied().unwrap_or_default()
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
    use super::{Lookup, NodeKey, Refs, parse_ref};

    fn key(backend: i64) -> NodeKey {
        in_frame("F", backend)
    }

    fn in_frame(frame: &str, backend: i64) -> NodeKey {
        NodeKey {
            frame: frame.into(),
            backend,
        }
    }

    fn live(refs: &Refs, number: u32) -> (NodeKey, bool) {
        match refs.lookup(number) {
            Lookup::Live(entry, fallback) => (entry.key, fallback),
            other => panic!("e{number} is {other:?}"),
        }
    }

    #[test]
    fn a_node_keeps_its_ref_and_numbers_never_repeat() {
        let mut refs = Refs::default();
        assert_eq!(refs.assign(&key(10), "button", "Save"), 1);
        assert_eq!(refs.assign(&key(11), "link", "Home"), 2);
        assert_eq!(refs.assign(&key(10), "button", "Saved"), 1);
        let Lookup::Live(entry, _) = refs.lookup(1) else {
            panic!()
        };
        assert_eq!(entry.name, "Saved");
        assert_eq!(refs.get(&key(11)), Some(2));
        assert_eq!(refs.get(&key(12)), None);
        assert_eq!(refs.assign(&in_frame("G", 10), "button", "Pay"), 3);
        assert_eq!(refs.lookup(4), Lookup::Unknown);
        assert_eq!(refs.lookup(0), Lookup::Unknown);
    }

    #[test]
    fn a_new_page_makes_every_ref_stale_and_numbering_goes_on() {
        let mut refs = Refs::default();
        refs.assign(&key(10), "button", "Save");
        refs.assign(&in_frame("G", 3), "link", "Pay");
        let generation = refs.generation();
        refs.navigated(&["F".into()], true);
        assert_ne!(refs.generation(), generation);
        assert_eq!(refs.lookup(1), Lookup::StalePage);
        assert_eq!(refs.lookup(2), Lookup::StalePage);
        assert_eq!(refs.get(&key(10)), None);
        // The same backend id in the new document is a new node.
        assert_eq!(refs.assign(&key(10), "button", "Save"), 3);
        assert_eq!(live(&refs, 3).0, key(10));
    }

    #[test]
    fn a_frame_that_navigates_loses_only_its_refs() {
        let mut refs = Refs::default();
        refs.assign(&key(10), "button", "Save");
        refs.assign(&in_frame("G", 3), "link", "Pay");
        refs.assign(&in_frame("H", 3), "link", "Nested");
        refs.navigated(&["G".into(), "H".into()], false);
        assert_eq!(live(&refs, 1).0, key(10));
        assert_eq!(refs.lookup(2), Lookup::StaleFrame);
        assert_eq!(refs.lookup(3), Lookup::StaleFrame);
        assert_eq!(refs.assign(&in_frame("G", 3), "link", "Pay"), 4);
        // A later page load makes them all page-stale.
        refs.navigated(&["F".into()], true);
        assert_eq!(refs.lookup(3), Lookup::StalePage);
        assert_eq!(refs.lookup(4), Lookup::StalePage);
    }

    #[test]
    fn a_route_change_turns_the_fallback_off_until_shown_again() {
        let mut refs = Refs::default();
        refs.assign(&key(10), "button", "Save");
        refs.assign(&in_frame("G", 3), "link", "Pay");
        assert!(live(&refs, 1).1);
        refs.route_changed("F");
        assert_eq!(live(&refs, 1), (key(10), false));
        assert!(live(&refs, 2).1, "another frame's route is its own");
        refs.assign(&key(10), "button", "Save");
        assert!(live(&refs, 1).1);
    }

    #[test]
    fn repointing_moves_a_ref_to_the_node_found_instead() {
        let mut refs = Refs::default();
        refs.assign(&key(10), "button", "Save");
        refs.assign(&key(20), "button", "Other");
        refs.repoint(1, &key(11));
        assert_eq!(live(&refs, 1).0, key(11));
        assert_eq!(refs.get(&key(11)), Some(1));
        assert_eq!(refs.get(&key(10)), None);
        // A node that has a ref keeps it.
        refs.repoint(1, &key(20));
        assert_eq!(live(&refs, 1).0, key(20));
        assert_eq!(refs.get(&key(20)), Some(2));
        assert_eq!(refs.get(&key(11)), None);
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
