//! Mocked WebSockets and EventSources (plan 11, milestone 11.9), as the
//! router sees them.
//!
//! Neither goes through `Fetch` the way a mock could answer it, so a
//! shim in the page (`shim.js`, given to documents of a scope with mock
//! sets; see `crate::inject`) asks here about each socket and stream it
//! opens: [`Router::ws_open`] and [`Router::sse_open`] decide with the
//! scope's mocks and journal it, as a paused request would be. Sockets a
//! `[[websocket]]` answers are kept ([`Socket`]) so the page's messages
//! find their mock and `mock ws send` finds them; streams an `sse` mock
//! answers are kept ([`Stream`]) so `mock sse send` finds them.
//!
//! Also here: the header a test run tags its browser requests with
//! (`x-chrome-cdp-test`, for `mock serve`'s per-test state), given to the
//! requests to the run's own origins that routing lets go on.

use std::time::Duration;

use mock::{Outcome, Response, SseEvent, WsKey, WsMessage, WsOutcome, WsStep};
use protocol::schema::MockRef;
use serde_json::Value;

use super::{Act, Decision, Handler, Request, Router, Table, lock};

/// A page's socket that a `[[websocket]]` answers.
#[derive(Debug, Clone)]
pub(crate) struct Socket {
    pub(crate) scope: Option<String>,
    /// The page or iframe session, and the execution context (document)
    /// the socket lives in.
    pub(crate) session: String,
    pub(crate) context: i64,
    /// The shim's id of it, within its document.
    pub(crate) id: u64,
    pub(crate) url: String,
    pub(crate) tab: Option<u32>,
    pub(crate) key: WsKey,
    pub(crate) mock: MockRef,
}

/// A page's `EventSource` that a mock's `sse` answers.
#[derive(Debug, Clone)]
pub(crate) struct Stream {
    pub(crate) scope: Option<String>,
    /// The page or iframe session, and the execution context (document)
    /// the stream lives in.
    pub(crate) session: String,
    pub(crate) context: i64,
    /// The shim's id of it, within its document.
    pub(crate) id: u64,
    pub(crate) url: String,
    pub(crate) tab: Option<u32>,
    pub(crate) mock: MockRef,
    /// Its mock's own events are all handed over: pushed events go after
    /// them.
    pub(crate) ready: bool,
}

/// What a page's socket gets.
#[derive(Debug)]
pub(crate) enum WsReply {
    /// A mock: it opens (with `protocol`), then gets `steps`.
    Mock {
        protocol: Option<String>,
        steps: Vec<WsStep>,
    },
    /// The real socket.
    Real,
    /// It fails (unmatched in a set's scope).
    Fail,
}

/// What a page's `EventSource` gets.
#[derive(Debug)]
pub(crate) enum SseReply {
    /// A mock's events, after `delay`.
    Mock {
        delay: Duration,
        events: Vec<SseEvent>,
        mock: MockRef,
    },
    Real,
    Fail,
}

/// The header of a test run's browser requests, and where it goes.
#[derive(Debug, Clone)]
pub(crate) struct Tag {
    pub(crate) name: String,
    pub(crate) value: String,
    /// `http://localhost:5173`: origins, as `url` serialises them.
    pub(crate) origins: Vec<String>,
}

/// `http://localhost:5173` of a URL, if it has one.
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let origin = parsed.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

impl Router {
    /// Whether `scope`'s documents need the shim: it has mock sets.
    pub(crate) fn wants_shim(&self, scope: Option<&str>) -> bool {
        self.has_mocks(scope)
    }

    /// Decides what the socket to `url` (with the page's `protocols`)
    /// gets, and journals it.
    pub(crate) fn ws_open(
        &self,
        req: &Request<'_>,
        protocols: &[String],
    ) -> (WsReply, Option<(WsKey, MockRef)>) {
        let mut table = lock(&self.table);
        if table
            .mocks_of(req.context)
            .is_none_or(mock::Mocks::is_empty)
        {
            table.log(req, None, "continue".into());
            return (WsReply::Real, None);
        }
        let mocks = table.mocks_in(req.context);
        mocks.refresh();
        match mocks.ws_open(req.url, protocols, req.now_ms) {
            WsOutcome::Open {
                key,
                mock,
                protocol,
                steps,
                outcome,
            } => {
                table.log_mock(req, Some(mock.clone()), None, outcome);
                (WsReply::Mock { protocol, steps }, Some((key, mock)))
            }
            WsOutcome::Unmatched {
                set,
                policy,
                outcome,
            } => {
                table.log_mock(req, None, Some(set), outcome);
                if let Some((_, entry)) = table.journal.back_mut() {
                    entry.policy = Some(policy.as_str().to_owned());
                }
                match policy {
                    mock::Unmatched::Passthrough => (WsReply::Real, None),
                    mock::Unmatched::Fail | mock::Unmatched::Abort => (WsReply::Fail, None),
                }
            }
            WsOutcome::Pass => {
                table.log(req, None, "continue".into());
                (WsReply::Real, None)
            }
        }
    }

    /// The mock's answer to a message the page sent on `socket`.
    pub(crate) fn ws_message(
        &self,
        socket: &Socket,
        message: WsMessage<'_>,
        now_ms: i64,
    ) -> Vec<WsStep> {
        let mut table = lock(&self.table);
        let mocks = table.mocks_in(socket.scope.as_deref());
        mocks.refresh();
        mocks.ws_message(&socket.key, &socket.url, message, now_ms)
    }

    /// Decides what the page's `EventSource` to `url` gets, and journals
    /// it. A mock's answer is streamed if it is `sse` (or a 200
    /// `text/event-stream` body, read as events); other answers fail it,
    /// as they would a real one. Requests that go to the network are
    /// journalled when `Fetch` sees them.
    pub(crate) fn sse_open(&self, req: &Request<'_>) -> SseReply {
        let mut table = lock(&self.table);
        if table
            .mocks_of(req.context)
            .is_none_or(mock::Mocks::is_empty)
        {
            return SseReply::Real;
        }
        let headers = vec![("Accept".to_owned(), "text/event-stream".to_owned())];
        let mocks = table.mocks_in(req.context);
        mocks.refresh();
        let request = mock::Request {
            method: "GET",
            url: req.url,
            headers: &headers,
            body: None,
        };
        match mocks.decide(&request, req.now_ms) {
            Outcome::Answer(answer) => {
                let (reply, note) = match answer.response {
                    Response::Fulfill {
                        status,
                        headers,
                        body,
                        ..
                    } => {
                        let stream = headers.iter().any(|(n, v)| {
                            n.eq_ignore_ascii_case("Content-Type")
                                && v.to_ascii_lowercase().starts_with("text/event-stream")
                        });
                        if status == 200 && stream {
                            let events = answer.events.unwrap_or_else(|| {
                                mock::parse_sse(&String::from_utf8_lossy(&body))
                            });
                            let note = format!(
                                "{} {}",
                                events.len(),
                                if events.len() == 1 { "event" } else { "events" }
                            );
                            (
                                SseReply::Mock {
                                    delay: answer.delay,
                                    events,
                                    mock: answer.mock.clone(),
                                },
                                Some(note),
                            )
                        } else if status == 200 {
                            (
                                SseReply::Fail,
                                Some("EventSource fails: not text/event-stream".into()),
                            )
                        } else {
                            (SseReply::Fail, Some(format!("EventSource fails: {status}")))
                        }
                    }
                    Response::Abort { .. } => (SseReply::Fail, None),
                    // The real stream, unpatched (a stream has no JSON).
                    Response::Passthrough { .. } => return SseReply::Real,
                };
                let outcome = match note {
                    Some(note) => format!("{} ({note})", answer.outcome),
                    None => answer.outcome,
                };
                table.log_mock(req, Some(answer.mock), None, outcome);
                reply
            }
            Outcome::Unmatched {
                set,
                policy,
                outcome,
                closest,
                ..
            } => {
                if policy == mock::Unmatched::Passthrough {
                    return SseReply::Real;
                }
                table.log_mock(req, None, Some(set), outcome);
                if let Some((_, entry)) = table.journal.back_mut() {
                    entry.policy = Some(policy.as_str().to_owned());
                    entry.closest = closest;
                }
                SseReply::Fail
            }
            Outcome::Preflight { .. } | Outcome::Pass => SseReply::Real,
        }
    }

    /// Keeps a socket a mock answers.
    pub(crate) fn add_socket(&self, socket: Socket) {
        let mut table = lock(&self.table);
        if table.sockets.len() >= 1000 {
            table.sockets.remove(0);
        }
        table.sockets.push(socket);
    }

    /// Socket `id` of document `context` of `session`.
    pub(crate) fn socket(&self, session: &str, context: i64, id: u64) -> Option<Socket> {
        lock(&self.table)
            .sockets
            .iter()
            .find(|s| s.session == session && s.context == context && s.id == id)
            .cloned()
    }

    /// Forgets socket `id` of document `context` of `session` (closed).
    pub(crate) fn remove_socket(&self, session: &str, context: i64, id: u64) {
        lock(&self.table)
            .sockets
            .retain(|s| !(s.session == session && s.context == context && s.id == id));
    }

    /// The open sockets of `scope`, oldest first.
    pub(crate) fn sockets(&self, scope: Option<&str>) -> Vec<Socket> {
        lock(&self.table)
            .sockets
            .iter()
            .filter(|s| s.scope.as_deref() == scope)
            .cloned()
            .collect()
    }

    /// Keeps a stream a mock answers.
    pub(crate) fn add_stream(&self, stream: Stream) {
        let mut table = lock(&self.table);
        if table.streams.len() >= 1000 {
            table.streams.remove(0);
        }
        table.streams.push(stream);
    }

    /// Stream `id` of document `context` of `session` has had its mock's
    /// own events.
    pub(crate) fn stream_ready(&self, session: &str, context: i64, id: u64) {
        let mut table = lock(&self.table);
        if let Some(s) = table
            .streams
            .iter_mut()
            .find(|s| s.session == session && s.context == context && s.id == id)
        {
            s.ready = true;
        }
    }

    /// Forgets stream `id` of document `context` of `session` (closed).
    pub(crate) fn remove_stream(&self, session: &str, context: i64, id: u64) {
        lock(&self.table)
            .streams
            .retain(|s| !(s.session == session && s.context == context && s.id == id));
    }

    /// The open streams of `scope`, oldest first.
    pub(crate) fn streams(&self, scope: Option<&str>) -> Vec<Stream> {
        lock(&self.table)
            .streams
            .iter()
            .filter(|s| s.scope.as_deref() == scope)
            .cloned()
            .collect()
    }

    /// Tags `scope`'s requests to `tag`'s origins with its header, or
    /// (`None`) stops.
    pub(crate) fn set_tag(&self, scope: Option<&str>, tag: Option<Tag>) {
        let mut table = lock(&self.table);
        let key = scope.unwrap_or_default().to_owned();
        match tag {
            Some(tag) => {
                table.tags.insert(key, tag);
            }
            None => {
                table.tags.remove(&key);
            }
        }
    }
}

impl Table {
    /// `decision` for `req`, with the scope's tag header added if the
    /// request goes on to one of its origins.
    pub(super) fn tagged(&self, req: &Request<'_>, decision: Decision) -> Decision {
        let (Decision::Pass, Some(tag)) =
            (&decision, self.tags.get(req.context.unwrap_or_default()))
        else {
            return decision;
        };
        let to_origin = origin_of(req.url).is_some_and(|o| tag.origins.contains(&o));
        if !to_origin {
            return decision;
        }
        Decision::Mock {
            handler: std::sync::Arc::new(Handler {
                delay: Duration::ZERO,
                act: Act::Continue {
                    set_headers: vec![(tag.name.clone(), tag.value.clone())],
                    remove_headers: Vec::new(),
                    url: None,
                },
            }),
            seq: self.seq,
        }
    }
}

/// The JSON a delivery hands the shim.
pub(crate) fn frame_message(id: u64, step: &WsStep) -> Value {
    frame_json(id, &step.frame)
}

/// The JSON that hands `frame` to socket `id`.
pub(crate) fn frame_json(id: u64, frame: &mock::WsFrame) -> Value {
    match frame {
        mock::WsFrame::Text(text) => serde_json::json!({ "id": id, "text": text }),
        mock::WsFrame::Binary(base64) => serde_json::json!({ "id": id, "base64": base64 }),
        mock::WsFrame::Close { code, reason } => {
            serde_json::json!({ "id": id, "close": { "code": code, "reason": reason } })
        }
    }
}
