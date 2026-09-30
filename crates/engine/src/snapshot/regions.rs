//! A tab's regions: `r<n>` ↔ what a fold hid (plan 16, milestone 16.3).
//!
//! A region names a node whose children `--budget` folded, or a run of
//! siblings `--fold` folded after the first of them. It is kept the way a
//! ref is ([`super::refs`]): stable across snapshots for the same node (a
//! run's first node), never reused, and dropped when the node's frame
//! loads a new document (all of them for the main frame). Only
//! `snapshot --expand` takes one.

use std::collections::HashMap;

use super::tree::NodeKey;

/// What a region names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Region {
    /// A node, shown with its subtree.
    Node(NodeKey),
    /// Siblings, shown one after the other.
    Run(Vec<NodeKey>),
}

impl Region {
    /// The node that identifies it: the node, or the run's first.
    fn key(&self) -> Option<&NodeKey> {
        match self {
            Self::Node(key) => Some(key),
            Self::Run(keys) => keys.first(),
        }
    }
}

/// What a region number is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lookup {
    Live(Region),
    /// Given before the main frame loaded a new document.
    StalePage,
    /// Given before its (sub)frame loaded a new document.
    StaleFrame,
    /// Never given in this tab.
    Unknown,
}

/// One tab's regions.
#[derive(Debug, Default)]
pub(crate) struct Regions {
    last: u32,
    /// By the node that identifies it, and whether it is a run.
    by_key: HashMap<(NodeKey, bool), u32>,
    by_number: HashMap<u32, Region>,
    /// Regions up to this number are from earlier documents of the main
    /// frame.
    cleared: u32,
}

impl Regions {
    /// The number of `region`, given now if it has none. `None` for an
    /// empty run.
    pub(crate) fn assign(&mut self, region: Region) -> Option<u32> {
        let id = (region.key()?.clone(), matches!(region, Region::Run(_)));
        let number = if let Some(&number) = self.by_key.get(&id) {
            number
        } else {
            self.last += 1;
            self.by_key.insert(id, self.last);
            self.last
        };
        self.by_number.insert(number, region);
        Some(number)
    }

    /// What region `number` names now.
    pub(crate) fn lookup(&self, number: u32) -> Lookup {
        if let Some(region) = self.by_number.get(&number) {
            Lookup::Live(region.clone())
        } else if number == 0 || number > self.last {
            Lookup::Unknown
        } else if number <= self.cleared {
            Lookup::StalePage
        } else {
            Lookup::StaleFrame
        }
    }

    /// Frames `frames` loaded new documents; `main` if one is the main
    /// frame, which drops every region.
    pub(crate) fn navigated(&mut self, frames: &[String], main: bool) {
        if main {
            self.cleared = self.last;
            self.by_key.clear();
            self.by_number.clear();
            return;
        }
        self.by_number
            .retain(|_, region| region.key().is_some_and(|k| !frames.contains(&k.frame)));
        self.by_key
            .retain(|(key, _), _| !frames.contains(&key.frame));
    }
}

/// Parses a region as typed: `r4`.
pub(crate) fn parse_region(text: &str) -> Option<u32> {
    let digits = text.trim().strip_prefix('r')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::{Lookup, NodeKey, Region, Regions, parse_region};

    fn key(frame: &str, backend: i64) -> NodeKey {
        NodeKey {
            frame: frame.into(),
            backend,
        }
    }

    #[test]
    fn a_node_keeps_its_region_and_navigation_drops_it() {
        let mut regions = Regions::default();
        let node = Region::Node(key("F", 1));
        assert_eq!(regions.assign(node.clone()), Some(1));
        let run = Region::Run(vec![key("F", 1), key("F", 2)]);
        assert_eq!(regions.assign(run), Some(2), "a run is not its first node");
        let longer = Region::Run(vec![key("F", 1), key("F", 2), key("F", 3)]);
        assert_eq!(regions.assign(longer.clone()), Some(2));
        assert_eq!(regions.lookup(2), Lookup::Live(longer));
        assert_eq!(regions.assign(Region::Node(key("G", 1))), Some(3));
        assert_eq!(regions.assign(Region::Run(Vec::new())), None);
        assert_eq!(regions.lookup(4), Lookup::Unknown);

        regions.navigated(&["G".into()], false);
        assert_eq!(regions.lookup(3), Lookup::StaleFrame);
        assert_eq!(regions.lookup(1), Lookup::Live(node));
        regions.navigated(&["F".into()], true);
        assert_eq!(regions.lookup(1), Lookup::StalePage);
        assert_eq!(regions.assign(Region::Node(key("F", 1))), Some(4));
    }

    #[test]
    fn regions_as_typed() {
        assert_eq!(parse_region("r4"), Some(4));
        for bad in ["r", "r0", "4", "e4", "r4x", "R4"] {
            assert_eq!(parse_region(bad), None, "{bad}");
        }
    }
}
