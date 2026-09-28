//! WebSocket mocks (plan 11, milestone 11.9): `[[websocket]]`.
//!
//! Fetch cannot intercept WebSockets, so the daemon replaces the page's
//! `WebSocket` with a stand-in that asks [`Mocks::ws_open`] about each
//! socket it opens, and [`Mocks::ws_message`] about each message the page
//! sends. The answers are [`WsStep`]s: messages to hand the page, or a
//! close, each after its delay. Templates see the socket's URL (`params`,
//! `query`), the page's message (`body`, `json`), the named values and
//! the usual `uuid`, `now`, `seq` and `faker`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use protocol::schema::{MockInfo, MockRef};
use serde::Serialize;
use serde_json::Value;

use crate::engine::{Mocks, vars};
use crate::file::Unmatched;
use crate::glob::{Glob, PathGlob};
use crate::load::MockSet;
use crate::matcher::{Parsed, Request, json_subset};
use crate::respond::text_of;
use crate::template;

/// A `[[websocket]]`, checked and compiled.
#[derive(Debug, Clone)]
pub(crate) struct WsMock {
    pub(crate) name: String,
    pub(crate) line: usize,
    pub(crate) url: PathGlob,
    pub(crate) protocol: Option<String>,
    pub(crate) on_open: Vec<WsAction>,
    pub(crate) on_message: Vec<WsRule>,
    /// Sockets opened.
    pub(crate) hits: u64,
    /// Messages answered.
    pub(crate) answered: u64,
}

/// An action as written (strings are templates).
#[derive(Debug, Clone)]
pub(crate) struct WsAction {
    pub(crate) send: Option<Value>,
    pub(crate) close: Option<(u16, String)>,
    pub(crate) delay: Duration,
}

/// An `on_message` rule.
#[derive(Debug, Clone)]
pub(crate) struct WsRule {
    pub(crate) matcher: WsMatch,
    pub(crate) action: WsAction,
}

/// Which messages a rule answers.
#[derive(Debug, Clone)]
pub(crate) enum WsMatch {
    Any,
    /// JSON messages containing this, as `match.json`.
    Json(Value),
    /// A glob on the text.
    Text(Glob),
}

/// What the fake server does to a socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WsFrame {
    /// Hand the page a message.
    Text(String),
    /// Close the socket.
    Close { code: u16, reason: String },
}

/// A frame, after `delay` (counted from the step before it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsStep {
    pub delay: Duration,
    pub frame: WsFrame,
}

/// Which `[[websocket]]` a socket talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsKey {
    pub set: String,
    pub index: usize,
}

/// What the mocks do with a socket the page opens.
#[derive(Debug)]
pub enum WsOutcome {
    /// A `[[websocket]]` takes it: it opens with `protocol`, then gets
    /// `steps` (its `on_open`).
    Open {
        key: WsKey,
        mock: MockRef,
        protocol: Option<String>,
        steps: Vec<WsStep>,
        /// What the journal says: `websocket open`.
        outcome: String,
    },
    /// No `[[websocket]]` takes it, and set `set` owns its URL: with
    /// `fail` or `abort` it fails, with `passthrough` it goes to the
    /// network.
    Unmatched {
        set: String,
        policy: Unmatched,
        outcome: String,
    },
    /// Nothing to do with the mocks: a real socket.
    Pass,
}

impl WsMock {
    /// The socket URL's parameters if it takes it.
    fn captures(&self, parsed: &Parsed) -> Option<BTreeMap<String, String>> {
        if self.url.is_path() {
            self.url.captures(&parsed.path)
        } else {
            self.url.captures(&parsed.base)
        }
    }

    pub(crate) fn info(&self, set: &MockSet) -> MockInfo {
        let mut responds = format!(
            "websocket: on_open {}, on_message {}",
            self.on_open.len(),
            self.on_message.len()
        );
        if let Some(protocol) = &self.protocol {
            let _ = write!(responds, ", protocol {protocol}");
        }
        MockInfo {
            mock: ws_ref(set, self),
            matches: format!("WS {}", self.url),
            responds,
            hits: self.hits,
        }
    }
}

impl WsMatch {
    fn fits(&self, parsed: &Parsed) -> bool {
        match self {
            Self::Any => true,
            Self::Json(want) => parsed
                .json
                .as_ref()
                .is_some_and(|got| json_subset(want, got, false, &mut String::new()).is_ok()),
            Self::Text(glob) => glob.matches(&parsed.body),
        }
    }
}

pub(crate) fn ws_ref(set: &MockSet, ws: &WsMock) -> MockRef {
    MockRef {
        set: set.name().to_owned(),
        name: ws.name.clone(),
        file: set.source.file.display().to_string(),
        line: ws.line,
        at: format!("{}:{}", set.file_name(), ws.line),
    }
}

/// The steps of `actions` for one socket or message; a template error
/// closes the socket with 1011, the reason naming the mock.
fn steps(actions: &[&WsAction], ctx: &minijinja::Value, mock: &MockRef) -> Vec<WsStep> {
    let mut steps = Vec::with_capacity(actions.len());
    for action in actions {
        let frame = match (&action.send, &action.close) {
            (Some(send), _) => match template::render_json(send, ctx) {
                Ok(value) => WsFrame::Text(text_of(&value)),
                Err(e) => {
                    steps.push(WsStep {
                        delay: Duration::ZERO,
                        frame: WsFrame::Close {
                            code: 1011,
                            reason: cut_reason(&format!(
                                "chrome-cdp mock error in {} \"{}\": {e}",
                                mock.at, mock.name
                            )),
                        },
                    });
                    return steps;
                }
            },
            (None, Some((code, reason))) => WsFrame::Close {
                code: *code,
                reason: reason.clone(),
            },
            (None, None) => continue,
        };
        steps.push(WsStep {
            delay: action.delay,
            frame,
        });
    }
    steps
}

/// A close reason fits in 123 bytes.
fn cut_reason(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if out.len() + c.len_utf8() > 123 {
            break;
        }
        out.push(c);
    }
    out
}

fn parsed(url: &str, message: Option<&str>) -> Parsed {
    Parsed::new(&Request {
        method: "GET",
        url,
        headers: &[],
        body: message.map(str::as_bytes),
    })
}

impl Mocks {
    /// Whether a set in use has a `[[websocket]]`.
    pub fn has_websockets(&self) -> bool {
        self.sets.iter().any(|s| !s.websockets.is_empty())
    }

    /// Whether a set in use answers with `sse` somewhere.
    pub fn has_sse(&self) -> bool {
        self.sets.iter().any(|s| {
            s.mocks
                .iter()
                .any(|m| m.steps.iter().any(|s| s.respond.is_sse()))
        })
    }

    /// What socket `url` (absolute, `ws:` or `wss:`) gets; `protocols` are
    /// the page's. Opening counts a hit.
    pub fn ws_open(&mut self, url: &str, protocols: &[String], now_ms: i64) -> WsOutcome {
        let parsed = parsed(url, None);
        let found = self.sets.iter().enumerate().rev().find_map(|(s, set)| {
            set.websockets
                .iter()
                .enumerate()
                .find_map(|(i, ws)| Some((s, i, ws.captures(&parsed)?)))
        });
        let Some((s, i, params)) = found else {
            let Some(set) = self
                .sets
                .iter()
                .rev()
                .find(|s| s.owns(&parsed.path, &parsed.base))
            else {
                return WsOutcome::Pass;
            };
            let outcome = match set.unmatched {
                Unmatched::Passthrough => "unmatched, passthrough",
                Unmatched::Fail | Unmatched::Abort => "unmatched, socket fails",
            };
            return WsOutcome::Unmatched {
                set: set.name().to_owned(),
                policy: set.unmatched,
                outcome: outcome.into(),
            };
        };
        let seed = self.rng.next_u64();
        let state = self.state.clone();
        let set = &mut self.sets[s];
        let ws = &mut set.websockets[i];
        ws.hits += 1;
        let ctx = vars(&parsed, params, ws.hits, now_ms, seed, state).context();
        let ws = &set.websockets[i];
        let mock = ws_ref(set, ws);
        let actions: Vec<&WsAction> = ws.on_open.iter().collect();
        let steps = steps(&actions, &ctx, &mock);
        let protocol = ws.protocol.clone().or_else(|| protocols.first().cloned());
        let sends = steps
            .iter()
            .filter(|s| matches!(s.frame, WsFrame::Text(_)))
            .count();
        let outcome = match sends {
            0 => "websocket open".to_owned(),
            1 => "websocket open, sends 1 message".to_owned(),
            n => format!("websocket open, sends {n} messages"),
        };
        WsOutcome::Open {
            key: WsKey {
                set: set.name().to_owned(),
                index: i,
            },
            mock,
            protocol,
            steps,
            outcome,
        }
    }

    /// The answer of socket `key` (its URL `url`) to the page's message
    /// `text`: the first `on_message` rule that fits, else nothing. A set
    /// no longer in use answers nothing.
    pub fn ws_message(&mut self, key: &WsKey, url: &str, text: &str, now_ms: i64) -> Vec<WsStep> {
        let parsed = parsed(url, Some(text));
        let seed = self.rng.next_u64();
        let state = self.state.clone();
        let Some(set) = self.sets.iter_mut().find(|s| s.name() == key.set) else {
            return Vec::new();
        };
        let Some(ws) = set.websockets.get_mut(key.index) else {
            return Vec::new();
        };
        let Some(rule) = ws.on_message.iter().position(|r| r.matcher.fits(&parsed)) else {
            return Vec::new();
        };
        ws.answered += 1;
        let params = ws.captures(&parsed).unwrap_or_default();
        let ctx = vars(&parsed, params, ws.answered, now_ms, seed, state).context();
        let ws = &set.websockets[key.index];
        let mock = ws_ref(set, ws);
        steps(&[&ws.on_message[rule].action], &ctx, &mock)
    }
}
