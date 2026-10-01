//! The commands a session ran (plan 11, milestone 11.8), for `history`
//! and `history --as-scenario`.
//!
//! The CLI sends each command's words with its first call; the daemon
//! keeps them with whether the call succeeded and, for the refs the words
//! name, the stable locators their elements had just before it ran (a
//! ref's element may be gone after a click). The newest [`KEEP`] are
//! kept.

use std::collections::VecDeque;

use protocol::schema::{HistoryData, HistoryEntry, RefLocator};

/// How many commands are kept.
pub(crate) const KEEP: usize = 1000;

#[derive(Debug, Default)]
pub(crate) struct History {
    entries: VecDeque<HistoryEntry>,
    /// The last number given.
    last: u64,
    /// Commands dropped (the oldest, or by `--clear`).
    dropped: u64,
}

impl History {
    /// Keeps a command; `error` is its error's kind and message.
    pub(crate) fn push(
        &mut self,
        words: Vec<String>,
        context: Option<String>,
        error: Option<(String, String)>,
        refs: Vec<RefLocator>,
        password: bool,
    ) {
        self.last += 1;
        if self.entries.len() >= KEEP {
            self.entries.pop_front();
            self.dropped += 1;
        }
        self.entries.push_back(HistoryEntry {
            n: self.last,
            words,
            context,
            ok: error.is_none(),
            message: error.as_ref().map(|(_, m)| m.clone()),
            error: error.map(|(kind, _)| kind),
            refs,
            password,
        });
    }

    /// Forgets every command so far; numbers go on.
    pub(crate) fn clear(&mut self) {
        self.dropped += self.entries.len() as u64;
        self.entries.clear();
    }

    pub(crate) fn data(&self) -> HistoryData {
        HistoryData {
            entries: self.entries.iter().cloned().collect(),
            dropped: self.dropped,
            scenario: None,
            left_out: Vec::new(),
        }
    }
}

/// The value of flag `--name` in `words` (`--name v` or `--name=v`).
pub(crate) fn flag<'a>(words: &'a [String], name: &str) -> Option<&'a str> {
    let long = format!("--{name}");
    let eq = format!("--{name}=");
    let mut iter = words.iter();
    while let Some(w) = iter.next() {
        if *w == long {
            return iter.next().map(String::as_str);
        }
        if let Some(v) = w.strip_prefix(&eq) {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn keeps_the_newest_and_counts_the_rest() {
        let mut h = History::default();
        for i in 0..KEEP + 2 {
            h.push(words(&format!("goto /{i}")), None, None, Vec::new(), false);
        }
        let data = h.data();
        assert_eq!(data.entries.len(), KEEP);
        assert_eq!(data.dropped, 2);
        assert_eq!(data.entries[0].n, 3);
        h.clear();
        h.push(
            words("click e1"),
            Some("c1".into()),
            Some(("NotFound".into(), "no element matches e1".into())),
            Vec::new(),
            false,
        );
        let data = h.data();
        assert_eq!(data.dropped, KEEP as u64 + 2);
        assert_eq!(data.entries[0].n, KEEP as u64 + 3);
        assert!(!data.entries[0].ok);
        assert_eq!(data.entries[0].error.as_deref(), Some("NotFound"));
        assert_eq!(
            data.entries[0].message.as_deref(),
            Some("no element matches e1")
        );
    }

    #[test]
    fn flags_are_found_in_either_form() {
        assert_eq!(flag(&words("click e1 --tab t2"), "tab"), Some("t2"));
        assert_eq!(flag(&words("click e1 --tab=t3"), "tab"), Some("t3"));
        assert_eq!(flag(&words("click e1"), "tab"), None);
    }
}
