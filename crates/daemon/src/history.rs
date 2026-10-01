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
        redacted: Option<Vec<String>>,
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
            redacted,
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

/// What a password typed into a page becomes in a repro bundle.
pub(crate) const PASSWORD: &str = "${secret:PASSWORD}";

/// `words` with each of `passwords` (the values of the page's password
/// fields) as [`PASSWORD`]: a word that is one, or a string of a JSON
/// object word (`fill-form`'s values) that is one. `None` when none is
/// there.
pub(crate) fn redact(words: &[String], passwords: &[String]) -> Option<Vec<String>> {
    fn json(value: &mut serde_json::Value, passwords: &[String]) -> bool {
        match value {
            serde_json::Value::String(s) if passwords.contains(s) => {
                PASSWORD.clone_into(s);
                true
            }
            // Every value, not up to the first: each password goes.
            serde_json::Value::Object(map) => {
                let mut changed = false;
                for v in map.values_mut() {
                    changed |= json(v, passwords);
                }
                changed
            }
            serde_json::Value::Array(list) => {
                let mut changed = false;
                for v in list {
                    changed |= json(v, passwords);
                }
                changed
            }
            _ => false,
        }
    }
    let passwords: Vec<String> = passwords
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect();
    if passwords.is_empty() {
        return None;
    }
    let mut changed = false;
    let out = words
        .iter()
        .map(|w| {
            if passwords.contains(w) {
                changed = true;
                return PASSWORD.to_owned();
            }
            if w.trim_start().starts_with('{')
                && let Ok(mut value) = serde_json::from_str::<serde_json::Value>(w)
                && json(&mut value, &passwords)
            {
                changed = true;
                return value.to_string();
            }
            w.clone()
        })
        .collect();
    changed.then_some(out)
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
            h.push(words(&format!("goto /{i}")), None, None, Vec::new(), None);
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
            None,
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
    fn passwords_are_redacted_as_words_and_json_values() {
        let pw = vec!["hunter2".to_owned()];
        assert_eq!(
            redact(&words("fill label=Password hunter2"), &pw),
            Some(words("fill label=Password ${secret:PASSWORD}"))
        );
        let form = vec![
            "fill-form".to_owned(),
            "--values".to_owned(),
            r#"{"Email":"a@b.test","Password":"hunter2"}"#.to_owned(),
        ];
        assert_eq!(
            redact(&form, &pw).unwrap()[2],
            r#"{"Email":"a@b.test","Password":"${secret:PASSWORD}"}"#
        );
        // A part of a word is not the password: `hunter2x` stays.
        assert_eq!(redact(&words("fill e1 hunter2x"), &pw), None);
        assert_eq!(redact(&words("fill e1 x"), &[String::new()]), None);
    }

    #[test]
    fn flags_are_found_in_either_form() {
        assert_eq!(flag(&words("click e1 --tab t2"), "tab"), Some("t2"));
        assert_eq!(flag(&words("click e1 --tab=t3"), "tab"), Some("t3"));
        assert_eq!(flag(&words("click e1"), "tab"), None);
    }
}
