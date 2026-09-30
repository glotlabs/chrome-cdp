//! `wait --ws PATTERN [--frame REGEX]` (plan 14, milestone 14.7): the
//! first WebSocket frame of the tab, sent or received since the previous
//! command began (as `--console` counts entries), on a socket whose URL
//! matches, and whose text matches `--frame` if given. Event-driven on
//! the tab's network log, which numbers its changes; it takes no tab
//! lock.

use std::collections::VecDeque;

use protocol::schema::{WaitWebSocket, WebSocketFrame};
use regex::Regex;
use serde_json::json;
use tokio::time::Instant;

use super::{Met, Unmet};
use crate::pattern::UrlPattern;
use crate::tabs::{Page, TabError};
use crate::{Engine, Running};

/// How many of the frames that did not match a timeout lists.
const LISTED: usize = 5;

impl Engine {
    /// Waits until `page`'s network log has a matching frame, up to
    /// `deadline`; `Err` names the sockets and the last frames seen.
    pub(super) async fn websocket_until(
        &self,
        running: &Running,
        page: &Page,
        pattern: &UrlPattern,
        frame: Option<&Regex>,
        deadline: Instant,
    ) -> Result<Result<Met, Unmet>, protocol::ErrorInfo> {
        let wanted = |f: &WebSocketFrame| {
            frame.is_none_or(|re| f.text.as_deref().is_some_and(|t| re.is_match(t)))
        };
        let mut checked = running.tabs.previous_netlog_mark(page.id);
        let mut sockets = 0;
        let mut seen: VecDeque<String> = VecDeque::new();
        loop {
            let (found, changes) = running
                .tabs
                .netlog(page.id, |log| {
                    // The earliest new frame that matches, of any socket.
                    let mut found: Option<(u64, WaitWebSocket)> = None;
                    sockets = 0;
                    for record in log.records() {
                        if record.entry.websocket.is_none() || !pattern.matches(&record.entry.url) {
                            continue;
                        }
                        sockets += 1;
                        for (change, f) in record.frames.iter().filter(|(c, _)| *c > checked) {
                            if found.as_ref().is_some_and(|(c, _)| c < change) {
                                break;
                            }
                            if wanted(f) {
                                let ws = WaitWebSocket {
                                    id: record.entry.id,
                                    url: record.entry.url.clone(),
                                    frame: f.clone(),
                                };
                                found = Some((*change, ws));
                                break;
                            }
                            if seen.len() == LISTED {
                                seen.pop_front();
                            }
                            seen.push_back(format!("#{} {}", record.entry.id, frame_line(f)));
                        }
                    }
                    (found.map(|(_, ws)| ws), log.changes())
                })
                .ok_or_else(|| self.tab_error(running, TabError::NoSuchTab(page.id)))?;
            if let Some(ws) = found {
                return Ok(Ok(Met {
                    text: format!("frame {} on #{} {}", frame_line(&ws.frame), ws.id, ws.url),
                    websocket: Some(ws),
                    ..Met::default()
                }));
            }
            checked = changes;
            if !running
                .tabs
                .netlog_changed(page.id, changes, deadline)
                .await
            {
                return Ok(Err(unmet(sockets, &seen)));
            }
        }
    }
}

/// `3 received text 12B {"type":"hello"}`.
fn frame_line(frame: &WebSocketFrame) -> String {
    let mut line = format!(
        "{} {} {} {}",
        frame.n,
        if frame.sent { "sent" } else { "received" },
        frame.opcode,
        protocol::format::size(frame.size)
    );
    if let Some(text) = &frame.text {
        line.push(' ');
        line.push_str(&text.replace('\n', " "));
    }
    line
}

/// What a timeout saw: the matching sockets, and the last frames on them
/// that did not match.
fn unmet(sockets: usize, seen: &VecDeque<String>) -> Unmet {
    let unmet = Unmet::default()
        .observed("sockets", json!(sockets))
        .observed("frames", json!(seen));
    if sockets == 0 {
        return unmet.note(
            "observed",
            "no WebSocket whose URL matches in the tab's log",
        );
    }
    if seen.is_empty() {
        return unmet.note(
            "observed",
            "no frames on a matching socket since the previous command began",
        );
    }
    seen.iter()
        .fold(unmet, |unmet, line| unmet.note("seen", line.clone()))
}
