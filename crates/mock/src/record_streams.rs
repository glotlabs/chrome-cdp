//! `mock record` and `mock import` of streaming traffic (plan 19,
//! milestone 19.2): a page's WebSockets become `[[websocket]]` mocks, and
//! an EventSource's messages an `sse` answer.
//!
//! One socket's messages, in order, become:
//!
//! - `on_open`: the server's messages before the page's first, each with
//!   the delay since the one before (rounded to 10 ms; none under 50 ms);
//! - an `on_message` rule for each page message that got an answer: it
//!   matches the message (a JSON object by its stable fields, as
//!   `--match-bodies` picks them; any other text exactly) and sends the
//!   first server message after it, before the page's next. The others
//!   are left out (a rule sends one message), with the reason.
//!
//! Binary messages are left out. Sockets to one path are merged: rules
//! once, a conflicting answer (or `on_open`) from the first socket only.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{Map, Value};

use crate::record::{Exchange, toml_string, toml_value};
use crate::stable::{self, Fields, Kind};
use crate::template::is_template;

/// A WebSocket message of a recording (HAR `_webSocketMessages`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WsRecorded {
    /// Sent by the page (else received).
    pub sent: bool,
    /// When (ms since the epoch).
    pub time_ms: i64,
    /// 1 text, 2 binary, 8 close, 9 ping, 10 pong.
    pub opcode: u8,
    /// The text, or a binary message's base64.
    pub data: String,
}

/// An EventSource message of a recording (`_eventSourceMessages`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseRecorded {
    /// When (ms since the epoch).
    pub time_ms: i64,
    /// Its type; `message` for most.
    pub event: String,
    pub id: String,
    pub data: String,
}

/// An `sse` event as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseRec {
    pub(crate) delay_ms: u64,
    pub(crate) event: Option<String>,
    pub(crate) id: Option<String>,
    pub(crate) data: Option<String>,
}

/// `"120ms"` or `"2s"`: `ms` rounded to 10 ms, `None` under 50 ms.
pub(crate) fn delay(ms: i64) -> Option<String> {
    if ms < 50 {
        return None;
    }
    let ms = (ms + 5) / 10 * 10;
    Some(if ms % 1000 == 0 {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    })
}

/// The events of an EventSource's messages, each with the delay since
/// the one before (the first: since `start_ms`).
pub(crate) fn sse_events(messages: &[SseRecorded], start_ms: Option<i64>) -> Vec<SseRec> {
    let mut last = start_ms.or_else(|| messages.first().map(|m| m.time_ms));
    messages
        .iter()
        .map(|m| {
            let since = last.map_or(0, |l| m.time_ms - l);
            last = Some(m.time_ms);
            SseRec {
                delay_ms: u64::try_from(since).unwrap_or(0),
                event: Some(m.event.clone()).filter(|e| !e.is_empty() && e != "message"),
                id: Some(m.id.clone()).filter(|i| !i.is_empty()),
                data: Some(m.data.clone()),
            }
        })
        .collect()
}

/// The events of a `text/event-stream` body (no delays: the times are
/// not known).
pub(crate) fn sse_body(text: &str) -> Vec<SseRec> {
    crate::parse_sse(text)
        .into_iter()
        .filter(|e| e.data.is_some() || e.event.is_some() || e.id.is_some())
        .map(|e| SseRec {
            delay_ms: 0,
            event: e.event.filter(|e| !e.is_empty() && e != "message"),
            id: e.id,
            data: e.data,
        })
        .collect()
}

/// The `sse = [ … ]` value of `events`; `notes` get what was left out.
pub(crate) fn sse_toml(events: &[SseRec], notes: &mut Vec<String>) -> Option<String> {
    let mut items = Vec::new();
    let mut templated = 0;
    for e in events {
        let mut parts = Vec::new();
        if let Some(d) = delay(i64::try_from(e.delay_ms).unwrap_or(i64::MAX)) {
            parts.push(format!("delay = {}", toml_string(&d)));
        }
        if let Some(event) = &e.event {
            parts.push(format!("event = {}", toml_string(event)));
        }
        if let Some(id) = &e.id {
            parts.push(format!("id = {}", toml_string(id)));
        }
        if let Some(data) = &e.data {
            let Some(value) = send_value(data) else {
                templated += 1;
                continue;
            };
            parts.push(format!("data = {value}"));
        }
        items.push(format!("{{ {} }}", parts.join(", ")));
    }
    if templated > 0 {
        notes.push(format!(
            "{templated} event(s) left out: their data looks like a template ({{{{ … }}}})"
        ));
    }
    if items.is_empty() {
        return None;
    }
    Some(format!("[\n  {},\n]", items.join(",\n  ")))
}

/// A message as a mock sends it: a JSON object or array as a table or
/// list (the mock sends it back as JSON), else a string; `None` if it
/// would be read as a template.
fn send_value(text: &str) -> Option<String> {
    if let Ok(value @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(text)
        && let Some(toml) = toml_value(&value)
    {
        return Some(toml);
    }
    (!is_template(text) && !text.contains("{#")).then(|| toml_string(text))
}

/// How a rule matches the page's message.
#[derive(Debug, Clone, PartialEq)]
enum Match {
    /// A JSON object's stable fields.
    Json(Map<String, Value>),
    /// The text, exactly.
    Text(String),
}

impl Match {
    fn toml(&self) -> String {
        match self {
            Self::Json(map) => toml_value(&Value::Object(map.clone()))
                .unwrap_or_else(|| toml_string(&Value::Object(map.clone()).to_string())),
            Self::Text(text) => toml_string(text),
        }
    }

    /// Fields matched: a text match is the most specific.
    fn weight(&self) -> usize {
        match self {
            Self::Json(map) => stable::count(map),
            Self::Text(_) => usize::MAX,
        }
    }
}

/// An `on_open` send or an `on_message` rule.
#[derive(Debug, Clone, PartialEq)]
struct Action {
    delay: Option<String>,
    /// The `send` value, as TOML.
    send: String,
}

impl Action {
    fn toml(&self, matcher: Option<&Match>) -> String {
        let mut parts = Vec::new();
        if let Some(m) = matcher {
            parts.push(format!("match = {}", m.toml()));
        }
        if let Some(d) = &self.delay {
            parts.push(format!("delay = {}", toml_string(d)));
        }
        parts.push(format!("send = {}", self.send));
        format!("{{ {} }}", parts.join(", "))
    }
}

/// `[[websocket]]`s of the recorded sockets.
#[derive(Debug, Default)]
pub(crate) struct Sockets {
    /// The TOML of each, in the order first seen.
    pub(crate) toml: Vec<String>,
    /// What was left out, and why.
    pub(crate) skipped: Vec<String>,
}

/// The `[[websocket]]`s of `sockets` (WebSocket exchanges in scope).
pub(crate) fn websockets(sockets: &[&Exchange], now_ms: i64) -> Sockets {
    let mut out = Sockets::default();
    // By path, in the order first seen.
    let mut paths: Vec<String> = Vec::new();
    for ex in sockets {
        let path = path_of(&ex.url);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    for path in paths {
        let group: Vec<&Exchange> = sockets
            .iter()
            .copied()
            .filter(|ex| path_of(&ex.url) == path)
            .collect();
        if let Some(toml) = websocket(&path, &group, now_ms, &mut out.skipped) {
            out.toml.push(toml);
        }
    }
    out
}

/// `/ws/prices` of `ws://host/ws/prices?x=1`.
fn path_of(url: &str) -> String {
    url::Url::parse(url).map_or_else(|_| "/".to_owned(), |u| u.path().to_owned())
}

/// One `[[websocket]]` of the sockets to `path`.
fn websocket(
    path: &str,
    group: &[&Exchange],
    now_ms: i64,
    skipped: &mut Vec<String>,
) -> Option<String> {
    let shown = format!("WS {path}");
    // The page's JSON messages, for their stable fields.
    let sent: Vec<Fields> = group
        .iter()
        .flat_map(|ex| ex.ws_messages.iter())
        .filter(|m| m.sent && m.opcode == 1)
        .filter_map(|m| match serde_json::from_str::<Value>(&m.data) {
            Ok(Value::Object(map)) => Some(Fields {
                kind: Kind::Json,
                value: map,
                at_ms: m.time_ms,
            }),
            _ => None,
        })
        .collect();
    let refs: Vec<&Fields> = sent.iter().collect();
    let drop = stable::volatile(&refs);
    let matcher = |text: &str| -> Match {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => {
                let map = stable::stable(&map, &drop);
                if toml_value(&Value::Object(map.clone())).is_some() {
                    Match::Json(map)
                } else {
                    Match::Text(text.to_owned())
                }
            }
            _ => Match::Text(text.to_owned()),
        }
    };

    let mut on_open: Option<Vec<Action>> = None;
    let mut rules: Vec<(Match, Action)> = Vec::new();
    let mut binary = 0;
    let mut more = 0;
    let mut conflicts = 0;
    for (n, ex) in group.iter().enumerate() {
        let messages: Vec<&WsRecorded> = ex
            .ws_messages
            .iter()
            .filter(|m| matches!(m.opcode, 1 | 2))
            .collect();
        let start = ex
            .started_ms
            .or_else(|| messages.first().map(|m| m.time_ms))
            .unwrap_or(now_ms);
        // The server's messages before the page's first.
        let mut opens = Vec::new();
        let mut last = start;
        let mut i = 0;
        while let Some(m) = messages.get(i).filter(|m| !m.sent) {
            i += 1;
            let since = m.time_ms - last;
            last = m.time_ms;
            if m.opcode != 1 {
                binary += 1;
                continue;
            }
            match send_value(&m.data) {
                Some(send) => opens.push(Action {
                    delay: delay(since),
                    send,
                }),
                None => more += 1,
            }
        }
        match &on_open {
            None => on_open = Some(opens),
            // One that sent fewer (it closed sooner) agrees; delays vary.
            Some(first)
                if {
                    let sends = |a: &[Action]| a.iter().map(|a| a.send.clone()).collect::<Vec<_>>();
                    let (a, b) = (sends(first), sends(&opens));
                    !a.starts_with(&b) && !b.starts_with(&a)
                } =>
            {
                skipped.push(format!(
                    "{shown}: socket {}'s first messages differ from socket 1's; on_open has socket 1's",
                    n + 1
                ));
            }
            Some(_) => {}
        }
        // Each page message, and the server's answers up to the next.
        while let Some(m) = messages.get(i) {
            i += 1;
            let mut answers = Vec::new();
            while let Some(a) = messages.get(i).filter(|a| !a.sent) {
                answers.push(*a);
                i += 1;
            }
            if answers.is_empty() {
                continue;
            }
            if m.opcode != 1 {
                binary += 1 + answers.len();
                continue;
            }
            let first = answers[0];
            more += answers.len() - 1;
            if first.opcode != 1 {
                binary += 1;
                continue;
            }
            let Some(send) = send_value(&first.data) else {
                more += 1;
                continue;
            };
            let rule = (
                matcher(&m.data),
                Action {
                    delay: delay(first.time_ms - m.time_ms),
                    send,
                },
            );
            match rules.iter().find(|(k, _)| *k == rule.0) {
                None => rules.push(rule),
                Some((_, action)) if action.send != rule.1.send => conflicts += 1,
                Some(_) => {}
            }
        }
    }
    if binary > 0 {
        skipped.push(format!(
            "{shown}: {binary} binary message(s) left out (send_base64 is not recorded)"
        ));
    }
    if more > 0 {
        skipped.push(format!(
            "{shown}: {more} more answer(s) left out: a rule sends one message (the first after \
             the page's), and text like a template ({{{{ … }}}}) is not sent"
        ));
    }
    if conflicts > 0 {
        skipped.push(format!(
            "{shown}: {conflicts} answer(s) to a message answered differently before; the first \
             is kept"
        ));
    }
    let on_open = on_open.unwrap_or_default();
    if on_open.is_empty() && rules.is_empty() {
        skipped.push(format!(
            "{shown}: the server sent no text message to record"
        ));
        return None;
    }
    // The more a rule matches, the earlier: a subset never shadows it.
    rules.sort_by_key(|(m, _)| std::cmp::Reverse(m.weight()));
    let mut text = String::new();
    let ids: BTreeSet<&str> = path
        .split('/')
        .filter(|s| crate::record::is_id(s))
        .collect();
    if !ids.is_empty() {
        let _ = writeln!(
            text,
            "# ids in the path ({}): url = \"{}\" takes every one, and {{{{params.id}}}} says which",
            ids.into_iter().collect::<Vec<_>>().join(", "),
            crate::record::param_path(path)
        );
    }
    let _ = writeln!(
        text,
        "# {} recorded {}",
        group.len(),
        if group.len() == 1 {
            "socket"
        } else {
            "sockets"
        }
    );
    text.push_str("[[websocket]]\n");
    let _ = writeln!(text, "url = {}", toml_string(path));
    let protocol = group.iter().find_map(|ex| {
        ex.response_headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("sec-websocket-protocol"))
            .map(|(_, v)| v.clone())
    });
    if let Some(protocol) = protocol {
        let _ = writeln!(text, "protocol = {}", toml_string(&protocol));
    }
    if !on_open.is_empty() {
        text.push_str("on_open = [\n");
        for a in &on_open {
            let _ = writeln!(text, "  {},", a.toml(None));
        }
        text.push_str("]\n");
    }
    if !rules.is_empty() {
        text.push_str("on_message = [\n");
        for (m, a) in &rules {
            let _ = writeln!(text, "  {},", a.toml(Some(m)));
        }
        text.push_str("]\n");
    }
    Some(text)
}
