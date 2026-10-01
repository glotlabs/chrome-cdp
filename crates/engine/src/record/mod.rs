//! The recorder (plan 18, milestone 18.6): `record start|status|stop`
//! turn what happens in the page, whoever does it, into `.cdp` lines.
//!
//! `record start` puts the bundle's `record` (`injected/src/record.ts`)
//! in chrome-cdp's isolated world of every page and iframe of the scope,
//! as an init script with a binding of its own ([`RECORD_BINDING`], see
//! `crate::inject`). It reports clicks, typing, choices, files and keys,
//! each with the element's stable locator taken at event time. The tab
//! manager adds what the page cannot see (`tabs::record`): which frame
//! a report came from (its `frame=` step), navigations the browser
//! started (a typed URL, back, forward, reload; not those the page asked
//! for, which a replay repeats), dialogs someone answered, and the tab.
//!
//! [`Recording`] keeps the raw events, in the order they came; [`steps`]
//! turns them into lines when asked, so the coalescing rules (typing
//! becomes one `fill` when another event comes; a click on a field that
//! is then typed into goes; two clicks and a double click are one
//! `dblclick`) have unit tests.

pub(crate) mod commands;

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use protocol::schema::RecordedStep;
use serde::Deserialize;

/// The binding the recorder reports on, in chrome-cdp's isolated world.
pub(crate) const RECORD_BINDING: &str = "__chromeCdpRecord";

/// The secret a password typed while recording becomes.
pub(crate) const PASSWORD_SECRET: &str = "${secret:PASSWORD}";

/// The session's recording, if one runs.
#[derive(Debug, Default)]
pub(crate) struct Recorder {
    recording: Mutex<Option<Recording>>,
}

/// A recording.
#[derive(Debug, Clone)]
pub(crate) struct Recording {
    /// The test's name.
    pub(crate) name: String,
    /// The browser context it records in (`None`: the session's default).
    pub(crate) scope: Option<String>,
    pub(crate) started_ms: u64,
    /// The tab it started in.
    pub(crate) tab: u32,
    /// The tabs there were at the start.
    pub(crate) existing: HashSet<u32>,
    /// Tabs first seen later, and whether the page opened each (a popup).
    pub(crate) opened: HashMap<u32, bool>,
    /// Tabs whose next main-frame navigation their page asked for.
    pub(crate) asked: HashSet<u32>,
    /// Dialogs the session's policy answered, by tab, whose closing is no
    /// step: the replay's policy answers them too.
    pub(crate) answered: HashMap<u32, u32>,
    pub(crate) events: Vec<Raw>,
}

/// An event, in the order it came.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Raw {
    pub(crate) tab: u32,
    pub(crate) what: What,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum What {
    /// A report of the page's recorder.
    Page { frame: Frame, event: Box<PageEvent> },
    /// A main-frame navigation the browser started.
    Navigate { how: Nav, url: String },
    /// A dialog someone answered.
    Dialog { accept: bool, text: Option<String> },
}

/// The frame a page report came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Main,
    /// An iframe, by the `frame=` step that finds it.
    Step(String),
    /// An iframe no `frame=` step finds alone.
    Unnamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nav {
    Goto,
    Reload,
    /// Back or forward: which, the documents' history indexes tell.
    History,
}

/// What the page's recorder reports (`record.ts`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(crate) struct PageEvent {
    /// `click`, `dblclick`, `check`, `input`, `select`, `upload`, `press`
    /// or `page`.
    pub(crate) k: String,
    pub(crate) loc: Option<String>,
    pub(crate) how: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) checked: Option<bool>,
    pub(crate) value: Option<String>,
    /// Typed into a password field: no value.
    #[serde(default)]
    pub(crate) secret: bool,
    #[serde(default)]
    pub(crate) values: Vec<String>,
    #[serde(default)]
    pub(crate) names: Vec<String>,
    pub(crate) key: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) index: Option<i64>,
}

fn lock(m: &Mutex<Option<Recording>>) -> MutexGuard<'_, Option<Recording>> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Recorder {
    /// Starts `recording`; returns the one it replaces.
    pub(crate) fn start(&self, recording: Recording) -> Option<Recording> {
        lock(&self.recording).replace(recording)
    }

    /// Ends the recording; returns it.
    pub(crate) fn stop(&self) -> Option<Recording> {
        lock(&self.recording).take()
    }

    /// A copy of the recording.
    pub(crate) fn current(&self) -> Option<Recording> {
        lock(&self.recording).clone()
    }

    /// Whether a recording of `scope` runs.
    pub(crate) fn records(&self, scope: Option<&str>) -> bool {
        lock(&self.recording)
            .as_ref()
            .is_some_and(|r| r.scope.as_deref() == scope)
    }

    /// Changes the recording of `scope`, if one runs.
    pub(crate) fn with(&self, scope: Option<&str>, f: impl FnOnce(&mut Recording)) {
        if let Some(r) = lock(&self.recording)
            .as_mut()
            .filter(|r| r.scope.as_deref() == scope)
        {
            f(r);
        }
    }
}

impl Recording {
    /// Adds an event of tab `tab`; `opener` says whether the page opened
    /// it, for a tab not seen yet.
    pub(crate) fn push(&mut self, tab: u32, opener: bool, what: What) {
        if !self.existing.contains(&tab) {
            self.opened.entry(tab).or_insert(opener);
        }
        self.events.push(Raw { tab, what });
    }
}

/// The lines of `recording`, and what a person should look at.
pub(crate) fn steps(recording: &Recording) -> (Vec<RecordedStep>, Vec<String>) {
    let mut b = Builder {
        recording,
        out: Vec::new(),
        warnings: Vec::new(),
        tab: recording.tab,
        seen: HashSet::from([recording.tab]),
        acted: HashSet::new(),
        typing: None,
    };
    let events = &recording.events;
    let mut index: HashMap<u32, i64> = HashMap::new();
    for (i, raw) in events.iter().enumerate() {
        let tab = raw.tab;
        match &raw.what {
            What::Page { event, .. } if event.k == "page" => {
                if let Some(n) = event.index.filter(|&n| n >= 0) {
                    index.insert(tab, n);
                }
            }
            What::Page { frame, event } => {
                b.acted.insert(tab);
                if event.k == "input"
                    && let Some(typing) = &mut b.typing
                    && typing.tab == tab
                    && typing.frame == *frame
                    && typing.event.loc == event.loc
                {
                    typing.event = (**event).clone();
                    continue;
                }
                b.flush();
                b.switch(tab, None);
                b.page(frame, event);
            }
            What::Navigate { how, url } => {
                // A popup loads what its page gave it: no step of its own.
                if recording.opened.get(&tab) == Some(&true) && !b.acted.contains(&tab) {
                    continue;
                }
                if *how == Nav::Goto && (url.is_empty() || url.starts_with("about:")) {
                    continue;
                }
                b.flush();
                let goto = (*how == Nav::Goto).then_some(url.as_str());
                if b.switch(tab, goto) {
                    continue;
                }
                let words = match how {
                    Nav::Goto => vec!["goto".to_owned(), url.clone()],
                    Nav::Reload => vec!["reload".to_owned()],
                    Nav::History => {
                        let before = index.get(&tab).copied();
                        let after = events[i + 1..].iter().find_map(|r| match &r.what {
                            What::Page { event, .. } if r.tab == tab && event.k == "page" => {
                                event.index.filter(|&n| n >= 0)
                            }
                            _ => None,
                        });
                        let forward = matches!((before, after), (Some(b), Some(a)) if a > b);
                        vec![if forward { "forward" } else { "back" }.to_owned()]
                    }
                };
                b.line(Vec::new(), words);
            }
            What::Dialog { accept, text } => {
                b.flush();
                b.switch(tab, None);
                let mut words = vec![
                    "dialog".to_owned(),
                    if *accept { "accept" } else { "dismiss" }.to_owned(),
                ];
                if *accept && let Some(text) = text.as_ref().filter(|t| !t.is_empty()) {
                    words.push(text.clone());
                }
                b.line(Vec::new(), words);
            }
        }
    }
    b.flush();
    (b.out, b.warnings)
}

/// Typing not written yet: the field's last report.
struct Typing {
    tab: u32,
    frame: Frame,
    event: PageEvent,
}

struct Builder<'a> {
    recording: &'a Recording,
    out: Vec<RecordedStep>,
    warnings: Vec<String>,
    /// The tab the steps act in.
    tab: u32,
    seen: HashSet<u32>,
    /// Tabs something happened in.
    acted: HashSet<u32>,
    typing: Option<Typing>,
}

impl Builder<'_> {
    fn line(&mut self, comments: Vec<String>, words: Vec<String>) {
        self.out.push(RecordedStep { comments, words });
    }

    /// Moves the steps to tab `tab`: `tab new [URL]` for a tab opened
    /// since the start (not by a page), else `tab tN`. With `goto`, the
    /// URL a new tab loads; returns whether `tab new URL` took it.
    fn switch(&mut self, tab: u32, goto: Option<&str>) -> bool {
        if tab == self.tab {
            return false;
        }
        self.tab = tab;
        let first = self.seen.insert(tab);
        let new = first && self.recording.opened.get(&tab) == Some(&false);
        if !new {
            self.line(Vec::new(), vec!["tab".to_owned(), format!("t{tab}")]);
            return false;
        }
        let mut words = vec!["tab".to_owned(), "new".to_owned()];
        if let Some(url) = goto {
            words.push(url.to_owned());
        }
        self.line(Vec::new(), words);
        goto.is_some()
    }

    /// The locator word of `event` in `frame`, or `None` (with a comment
    /// in `comments` and a warning) when no locator finds it alone.
    fn locator(
        &mut self,
        frame: &Frame,
        event: &PageEvent,
        comments: &mut Vec<String>,
    ) -> Option<String> {
        let Some(loc) = &event.loc else {
            let reason = event.reason.as_deref().unwrap_or("unknown");
            comments.push(format!(
                "# {} here: no locator finds its element alone ({reason}); add the step by hand",
                event.k
            ));
            self.warnings.push(format!(
                "step {}: a {} whose element no locator finds alone ({reason}) is a comment",
                self.out.len() + 1,
                event.k
            ));
            return None;
        };
        if event.how.as_deref() == Some("nth") {
            self.warnings.push(format!(
                "step {}: {loc} finds its element by position (nothing else found it alone); a \
                 test id on the element would be sturdier",
                self.out.len() + 1
            ));
        }
        Some(match frame {
            Frame::Main => loc.clone(),
            Frame::Step(step) => format!("{step} >> {loc}"),
            Frame::Unnamed => {
                comments.push(
                    "# in an iframe no frame= step finds alone; add one before the locator"
                        .to_owned(),
                );
                self.warnings.push(format!(
                    "step {}: {loc} is in an iframe with no name or URL that only it has",
                    self.out.len() + 1
                ));
                loc.clone()
            }
        })
    }

    /// Writes the typing not written yet as one `fill`.
    fn flush(&mut self) {
        let Some(typing) = self.typing.take() else {
            return;
        };
        let mut comments = Vec::new();
        let Some(loc) = self.locator(&typing.frame, &typing.event, &mut comments) else {
            self.line(comments, Vec::new());
            return;
        };
        // A click that only put the caret in the field adds nothing.
        if let Some(last) = self.out.last()
            && last.comments.is_empty()
            && last.words.len() == 2
            && last.words[0] == "click"
            && last.words[1] == loc
        {
            self.out.pop();
        }
        let value = if typing.event.secret {
            comments.push(
                "# a password field: the test reads it from the PASSWORD secret, never the value"
                    .to_owned(),
            );
            PASSWORD_SECRET.to_owned()
        } else {
            typing.event.value.clone().unwrap_or_default()
        };
        self.line(comments, vec!["fill".to_owned(), loc, value]);
    }

    /// The line of page report `event`.
    fn page(&mut self, frame: &Frame, event: &PageEvent) {
        if event.k == "input" {
            self.typing = Some(Typing {
                tab: self.tab,
                frame: frame.clone(),
                event: event.clone(),
            });
            return;
        }
        if event.k == "press" {
            let Some(key) = &event.key else {
                return;
            };
            let mut words = vec!["press".to_owned(), key.clone()];
            // Keys go where focus is: name the element when the step
            // before acted on another one (a key after a key follows the
            // focus they moved).
            if let Some(loc) = &event.loc {
                let word = match frame {
                    Frame::Step(step) => format!("{step} >> {loc}"),
                    Frame::Main | Frame::Unnamed => loc.clone(),
                };
                let before = self
                    .out
                    .last()
                    .map(|l| l.words.as_slice())
                    .unwrap_or_default();
                let follows =
                    before.first().is_some_and(|w| w == "press") || before.get(1) == Some(&word);
                if !follows {
                    let mut comments = Vec::new();
                    if let Some(target) = self.locator(frame, event, &mut comments) {
                        words.push("--in".to_owned());
                        words.push(target);
                    }
                    self.line(comments, words);
                    return;
                }
            }
            self.line(Vec::new(), words);
            return;
        }
        let mut comments = Vec::new();
        let Some(loc) = self.locator(frame, event, &mut comments) else {
            self.line(comments, Vec::new());
            return;
        };
        let words: Vec<String> = match event.k.as_str() {
            "click" => vec!["click".to_owned(), loc],
            "dblclick" => {
                // The two clicks before it are part of it.
                for _ in 0..2 {
                    if self.out.last().is_some_and(|l| {
                        l.comments.is_empty()
                            && l.words.len() == 2
                            && l.words[0] == "click"
                            && l.words[1] == loc
                    }) {
                        self.out.pop();
                    }
                }
                vec!["dblclick".to_owned(), loc]
            }
            "check" => {
                let verb = if event.checked == Some(false) {
                    "uncheck"
                } else {
                    "check"
                };
                vec![verb.to_owned(), loc]
            }
            "select" if !event.values.is_empty() => {
                let mut words = vec!["select".to_owned(), loc];
                words.extend(event.values.iter().cloned());
                words
            }
            "upload" if !event.names.is_empty() => {
                comments.push(format!(
                    "# TODO: path: the file{} chosen {} {}; give {} path",
                    if event.names.len() == 1 { "" } else { "s" },
                    if event.names.len() == 1 {
                        "was"
                    } else {
                        "were"
                    },
                    event.names.join(", "),
                    if event.names.len() == 1 {
                        "its"
                    } else {
                        "their"
                    },
                ));
                let mut words = vec!["upload".to_owned(), loc];
                words.extend(event.names.iter().cloned());
                words
            }
            _ => return,
        };
        self.line(comments, words);
    }
}

#[cfg(test)]
mod tests;
