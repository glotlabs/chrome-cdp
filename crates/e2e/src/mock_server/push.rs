//! `mock serve`'s WebSockets and pushes (plan 19, milestone 19.2).
//!
//! A request with `Upgrade: websocket` that a `[[websocket]]` of its
//! partition takes is upgraded and served by [`mock::Mocks::ws_open`] and
//! `ws_message`, with delays and `close` as in the browser; one no mock
//! takes is answered as any other request (usually 404).
//!
//! The open sockets, and the streams an `sse` mock answers, are kept so
//! that `mock ws send --serve` and `mock sse send --serve` can push to
//! them: control requests under [`CONTROL`], served to loopback peers
//! only:
//!
//! - `POST /__chrome-cdp/ws/send` and `POST /__chrome-cdp/sse/send`, with
//!   a JSON body `{ "url": glob, "data": "…", "base64"?, "event"?, "id"?,
//!   "retry"?, "test"?, "timeout_ms"? }` ([`Push`]). `test` limits them to
//!   one partition's sockets; they wait up to `timeout_ms` for one to
//!   open. The answer is 200 with `{ "sent": n, "to": [Pushed…] }`, or
//!   404 with `{ "error", "open" }` when none matched in time.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{
    CONNECTION, HeaderValue, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_PROTOCOL, UPGRADE,
};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use mock::{SocketGlob, SseEvent, WsFrame, WsKey, WsMessage, WsOutcome, WsStep};
use protocol::schema::MockRef;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::{
    UnboundedReceiver, UnboundedSender, WeakUnboundedSender, unbounded_channel,
};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::{
    Body, MockServer, Shared, TEST_HEADER, entry, journal, lock, now_ms, partition_of, plain,
};

/// The path prefix of control requests.
pub(super) const CONTROL: &str = "/__chrome-cdp/";

/// How many sockets, and how many streams, are kept; the oldest go first.
const KEPT: usize = 1000;

/// The longest message text a log line shows.
const LOG_TEXT: usize = 200;

/// Called with what happens on a served socket (`mock serve` prints it).
pub type OnSocket = Box<dyn Fn(&SocketLog) + Send + Sync>;

/// Something that happened on a socket or stream `mock serve` serves.
#[derive(Debug, Clone, Serialize)]
pub struct SocketLog {
    /// The `x-chrome-cdp-test` header of the upgrade, if it had one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test: Option<String>,
    /// `websocket` or `eventsource`.
    pub kind: &'static str,
    /// Its path and query.
    pub url: String,
    /// `open`, `in` (a message from the client), `out` (one to it) or
    /// `close`.
    pub event: &'static str,
    /// The message (cut to one line), or the close's code and reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The mock that answers it (with `open`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mock: Option<MockRef>,
}

impl SocketLog {
    /// `WS /ws/prices open (live.toml:6 "prices")`, `WS /ws/prices ←
    /// ping`, `SSE /api/prices → {"price":42}`.
    pub fn line(&self) -> String {
        let kind = if self.kind == "websocket" {
            "WS"
        } else {
            "SSE"
        };
        let what = match self.event {
            "in" => "←",
            "out" => "→",
            other => other,
        };
        let mut line = format!("{kind} {} {what}", self.url);
        if let Some(text) = &self.text {
            line.push(' ');
            line.push_str(text);
        }
        if let Some(mock) = &self.mock {
            let _ = write!(line, " ({} \"{}\")", mock.at, mock.name);
        }
        if let Some(test) = &self.test {
            let _ = write!(line, " [test {test}]");
        }
        line
    }
}

/// `text` on one line, cut to [`LOG_TEXT`] characters.
fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    match flat.char_indices().nth(LOG_TEXT) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat,
    }
}

/// A socket a `[[websocket]]` answers.
pub(super) struct ServedSocket {
    id: u64,
    pub(super) test: Option<String>,
    /// The full URL (`ws://host/path?query`).
    url: String,
    mock: MockRef,
    tx: UnboundedSender<WsFrame>,
}

/// A stream an `sse` mock answers.
pub(super) struct ServedStream {
    pub(super) test: Option<String>,
    /// The full URL.
    url: String,
    mock: MockRef,
    tx: UnboundedSender<SseEvent>,
}

/// What a push sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Push {
    /// To the WebSockets: a message.
    Ws(WsFrame),
    /// To the streams: an event.
    Sse(SseEvent),
}

/// A socket or stream a push reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pushed {
    /// Its URL.
    pub url: String,
    /// The mock that answers it.
    pub mock: MockRef,
    /// Its partition, if it is a test's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<String>,
}

/// Why a push to a mock server reached nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushError {
    pub message: String,
    /// The matching kind's open sockets or streams (URLs), if the server
    /// answered.
    pub open: Vec<String>,
    /// Whether the server was reached and said none matched (rather than
    /// not being there, or refusing).
    pub none_matched: bool,
}

impl MockServer {
    /// Hands `push` to the open sockets (or streams) whose URL `glob`
    /// matches, of partition `test` (every partition when `None`); those
    /// that took it.
    pub fn push(&self, glob: &SocketGlob, push: &Push, test: Option<&str>) -> Vec<Pushed> {
        push_now(&self.shared, glob, push, test)
    }
}

fn push_now(shared: &Shared, glob: &SocketGlob, push: &Push, test: Option<&str>) -> Vec<Pushed> {
    let mut state = lock(&shared.state);
    let ours = |t: &Option<String>| test.is_none_or(|test| t.as_deref() == Some(test));
    let mut sent = Vec::new();
    match push {
        Push::Ws(frame) => {
            state.sockets.retain(|s| !s.tx.is_closed());
            for socket in state
                .sockets
                .iter()
                .filter(|s| ours(&s.test) && glob.matches(&s.url))
            {
                if socket.tx.send(frame.clone()).is_ok() {
                    sent.push(Pushed {
                        url: socket.url.clone(),
                        mock: socket.mock.clone(),
                        test: socket.test.clone(),
                    });
                }
            }
        }
        Push::Sse(event) => {
            state.streams.retain(|s| !s.tx.is_closed());
            for stream in state
                .streams
                .iter()
                .filter(|s| ours(&s.test) && glob.matches(&s.url))
            {
                if stream.tx.send(event.clone()).is_ok() {
                    sent.push(Pushed {
                        url: stream.url.clone(),
                        mock: stream.mock.clone(),
                        test: stream.test.clone(),
                    });
                    log(
                        shared,
                        &SocketLog {
                            test: stream.test.clone(),
                            kind: "eventsource",
                            url: path_of(&stream.url),
                            event: "out",
                            text: event.data.as_deref().map(one_line),
                            mock: None,
                        },
                    );
                }
            }
        }
    }
    sent
}

/// The URLs of the open sockets (or streams) of partition `test`.
fn open_of(shared: &Shared, ws: bool, test: Option<&str>) -> Vec<String> {
    let state = lock(&shared.state);
    let ours = |t: &Option<String>| test.is_none_or(|test| t.as_deref() == Some(test));
    if ws {
        state
            .sockets
            .iter()
            .filter(|s| !s.tx.is_closed() && ours(&s.test))
            .map(|s| s.url.clone())
            .collect()
    } else {
        state
            .streams
            .iter()
            .filter(|s| !s.tx.is_closed() && ours(&s.test))
            .map(|s| s.url.clone())
            .collect()
    }
}

/// `/ws/prices?x=1` of a full URL.
fn path_of(url: &str) -> String {
    let rest = url.find("://").map_or(url, |i| &url[i + 3..]);
    rest.find('/')
        .map_or_else(|| "/".to_owned(), |i| rest[i..].to_owned())
}

fn log(shared: &Shared, entry: &SocketLog) {
    if let Some(on_socket) = shared.on_socket.get() {
        on_socket(entry);
    }
}

/// Keeps a stream an `sse` mock answers (at `url`); the events pushed to
/// it come out of the receiver.
pub(super) fn add_stream(
    shared: &Shared,
    test: Option<String>,
    url: &str,
    mock: MockRef,
) -> UnboundedReceiver<SseEvent> {
    let (tx, rx) = unbounded_channel();
    let mut state = lock(&shared.state);
    state.streams.retain(|s| !s.tx.is_closed());
    if state.streams.len() >= KEPT {
        state.streams.remove(0);
    }
    state.streams.push(ServedStream {
        test,
        url: url.to_owned(),
        mock,
        tx,
    });
    rx
}

/// Whether `req` asks for a WebSocket.
pub(super) fn is_upgrade(req: &Request<Incoming>) -> bool {
    req.headers()
        .get(UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Upgrades `req` if a `[[websocket]]` of its partition takes it; `None`
/// leaves it to the HTTP mocks.
pub(super) fn upgrade(
    shared: &Arc<Shared>,
    req: &mut Request<Incoming>,
    local: Option<SocketAddr>,
) -> Option<Response<Body>> {
    let header = |name: &str| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let key = header("sec-websocket-key")?;
    let host = header("host")
        .or_else(|| local.map(|a| a.to_string()))
        .unwrap_or_else(|| "localhost".into());
    let target = req
        .uri()
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str)
        .to_owned();
    let url = format!("ws://{host}{target}");
    let protocols: Vec<String> = header("sec-websocket-protocol")
        .map(|p| {
            p.split(',')
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let test = header(TEST_HEADER).filter(|t| !t.is_empty());
    let (tx, rx) = unbounded_channel();
    let (key_of, mock, protocol, steps, id) = {
        let mut state = lock(&shared.state);
        let state = &mut *state;
        let partition = partition_of(shared, state, test.as_deref());
        partition.mocks.refresh();
        let now = partition.now_ms.unwrap_or_else(now_ms);
        let WsOutcome::Open {
            key,
            mock,
            protocol,
            steps,
            outcome,
        } = partition.mocks.ws_open(&url, &protocols, now)
        else {
            return None;
        };
        let mut e = entry("GET", &url, "websocket");
        e.mock = Some(mock.clone());
        e.outcome = outcome;
        journal(shared, state, test.clone(), e, false);
        state.last_id += 1;
        let id = state.last_id;
        state.sockets.retain(|s| !s.tx.is_closed());
        if state.sockets.len() >= KEPT {
            state.sockets.remove(0);
        }
        state.sockets.push(ServedSocket {
            id,
            test: test.clone(),
            url: url.clone(),
            mock: mock.clone(),
            tx: tx.clone(),
        });
        (key, mock, protocol, steps, id)
    };
    log(
        shared,
        &SocketLog {
            test: test.clone(),
            kind: "websocket",
            url: target,
            event: "open",
            text: None,
            mock: Some(mock),
        },
    );
    let upgrade = hyper::upgrade::on(req);
    let socket = Socket {
        shared: shared.clone(),
        id,
        key: key_of,
        url,
        test,
        tx: tx.downgrade(),
    };
    tokio::spawn(async move {
        let Ok(upgraded) = upgrade.await else {
            socket.forget();
            return;
        };
        let ws = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
        socket.serve(ws, rx, steps).await;
    });
    let mut response = Response::new(Full::new(Bytes::new()).boxed());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    if let Ok(accept) = HeaderValue::from_str(&derive_accept_key(key.as_bytes())) {
        headers.insert(SEC_WEBSOCKET_ACCEPT, accept);
    }
    if let Some(protocol) = protocol
        && let Ok(value) = HeaderValue::from_str(&protocol)
    {
        headers.insert(SEC_WEBSOCKET_PROTOCOL, value);
    }
    Some(response)
}

/// A served socket's task.
struct Socket {
    shared: Arc<Shared>,
    id: u64,
    key: WsKey,
    url: String,
    test: Option<String>,
    /// Frames to send, in order (the mock's, after their delays, and
    /// pushed ones). Weak: when the kept socket goes (its partition is
    /// closed), the socket closes.
    tx: WeakUnboundedSender<WsFrame>,
}

impl Socket {
    fn log(&self, event: &'static str, text: Option<String>) {
        log(
            &self.shared,
            &SocketLog {
                test: self.test.clone(),
                kind: "websocket",
                url: path_of(&self.url),
                event,
                text,
                mock: None,
            },
        );
    }

    fn forget(&self) {
        lock(&self.shared.state).sockets.retain(|s| s.id != self.id);
    }

    /// Queues `steps`, each after its delay, in a task of their own.
    fn schedule(&self, steps: Vec<WsStep>) {
        if steps.is_empty() {
            return;
        }
        let Some(tx) = self.tx.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            for step in steps {
                if !step.delay.is_zero() {
                    tokio::time::sleep(step.delay).await;
                }
                if tx.send(step.frame).is_err() {
                    return;
                }
            }
        });
    }

    /// The mock's answer to a message from the client.
    fn answer(&self, message: WsMessage<'_>) -> Vec<WsStep> {
        let mut state = lock(&self.shared.state);
        let state = &mut *state;
        let partition = partition_of(&self.shared, state, self.test.as_deref());
        partition.mocks.refresh();
        let now = partition.now_ms.unwrap_or_else(now_ms);
        partition
            .mocks
            .ws_message(&self.key, &self.url, message, now)
    }

    async fn serve<S>(
        self,
        ws: WebSocketStream<S>,
        mut rx: UnboundedReceiver<WsFrame>,
        steps: Vec<WsStep>,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let (mut sink, mut source) = ws.split();
        self.schedule(steps);
        let closed = loop {
            tokio::select! {
                frame = rx.recv() => {
                    // The partition was closed: the socket goes with it.
                    let Some(frame) = frame else {
                        let _ = sink.send(Message::Close(None)).await;
                        break "1001 going away".to_owned();
                    };
                    let (message, close) = match frame {
                        WsFrame::Text(text) => {
                            self.log("out", Some(one_line(&text)));
                            (Message::text(text), None)
                        }
                        WsFrame::Binary(base64) => {
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(base64.as_bytes())
                                .unwrap_or_default();
                            self.log("out", Some(format!("({} bytes, binary)", bytes.len())));
                            (Message::binary(bytes), None)
                        }
                        WsFrame::Close { code, reason } => (
                            Message::Close(Some(CloseFrame {
                                code: CloseCode::from(code),
                                reason: reason.clone().into(),
                            })),
                            Some(format!("{code} {reason}").trim_end().to_owned()),
                        ),
                    };
                    if sink.send(message).await.is_err() {
                        break "1006 gone".to_owned();
                    }
                    if let Some(close) = close {
                        break close;
                    }
                }
                message = source.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        self.log("in", Some(one_line(text.as_str())));
                        let steps = self.answer(WsMessage::Text(text.as_str()));
                        self.schedule(steps);
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        self.log("in", Some(format!("({} bytes, binary)", bytes.len())));
                        let base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let steps = self.answer(WsMessage::Binary(&base64));
                        self.schedule(steps);
                    }
                    Some(Ok(Message::Close(frame))) => {
                        break match frame {
                            Some(f) => format!("{} {}", u16::from(f.code), f.reason).trim_end().to_owned(),
                            None => "1005".to_owned(),
                        };
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break "1006 gone".to_owned(),
                },
            }
        };
        let _ = sink.close().await;
        self.forget();
        self.log("close", Some(closed));
    }
}

/// The JSON body of a control request.
#[derive(Debug, Deserialize)]
struct Control {
    url: String,
    data: String,
    #[serde(default)]
    base64: bool,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    retry: Option<u64>,
    #[serde(default)]
    test: Option<String>,
    #[serde(default)]
    timeout_ms: u64,
}

/// A control request: `POST /__chrome-cdp/ws/send` or `…/sse/send`.
pub(super) async fn control(
    shared: &Arc<Shared>,
    req: Request<Incoming>,
    peer: SocketAddr,
) -> Response<Body> {
    if !peer.ip().is_loopback() {
        return plain(
            403,
            &json!({ "error": format!("chrome-cdp mock serve: {CONTROL} takes requests from this machine only") }),
        );
    }
    let path = req.uri().path().to_owned();
    let ws = match (req.method().as_str(), &path[CONTROL.len()..]) {
        ("POST", "ws/send") => true,
        ("POST", "sse/send") => false,
        _ => {
            return plain(
                404,
                &json!({ "error": format!("chrome-cdp mock serve: no control request {} {path}; there are POST {CONTROL}ws/send and POST {CONTROL}sse/send", req.method()) }),
            );
        }
    };
    let body = match Limited::new(req.into_body(), super::MAX_BODY)
        .collect()
        .await
    {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return plain(
                413,
                &json!({ "error": "chrome-cdp mock serve: the body is too large" }),
            );
        }
    };
    let control: Control = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(e) => {
            return plain(
                400,
                &json!({ "error": format!("chrome-cdp mock serve: the body is not a push: {e}") }),
            );
        }
    };
    let glob = match SocketGlob::parse(&control.url) {
        Ok(g) => g,
        Err(e) => {
            return plain(
                400,
                &json!({ "error": format!("chrome-cdp mock serve: url: {e}") }),
            );
        }
    };
    let push = if ws {
        if control.base64 {
            if base64::engine::general_purpose::STANDARD
                .decode(control.data.trim())
                .is_err()
            {
                return plain(
                    400,
                    &json!({ "error": "chrome-cdp mock serve: data is not base64" }),
                );
            }
            Push::Ws(WsFrame::Binary(control.data.trim().to_owned()))
        } else {
            Push::Ws(WsFrame::Text(control.data))
        }
    } else {
        Push::Sse(SseEvent {
            delay: Duration::ZERO,
            event: control.event.filter(|e| !e.is_empty()),
            data: Some(control.data.replace("\r\n", "\n").replace('\r', "\n")),
            id: control.id,
            retry: control.retry,
            comment: None,
        })
    };
    let test = control.test.filter(|t| !t.is_empty());
    let deadline = tokio::time::Instant::now() + Duration::from_millis(control.timeout_ms);
    loop {
        let sent = push_now(shared, &glob, &push, test.as_deref());
        if !sent.is_empty() {
            return plain(200, &json!({ "sent": sent.len(), "to": sent }));
        }
        if tokio::time::Instant::now() >= deadline {
            let what = if ws {
                "WebSocket"
            } else {
                "EventSource stream"
            };
            return plain(
                404,
                &json!({
                    "error": format!("no {what} matching {} is open", control.url),
                    "open": open_of(shared, ws, test.as_deref()),
                }),
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Posts a push to the mock server at `server` (`http://127.0.0.1:4010`):
/// `kind` is `ws` or `sse`, `body` the control request's JSON. Blocks; it
/// waits up to `timeout_ms` (in the body) plus a margin for an answer.
pub fn post_push(
    server: &str,
    kind: &str,
    body: &serde_json::Value,
) -> Result<Vec<Pushed>, PushError> {
    let failed = |message: String| PushError {
        message,
        open: Vec::new(),
        none_matched: false,
    };
    let wait =
        Duration::from_millis(body["timeout_ms"].as_u64().unwrap_or(0)) + Duration::from_secs(10);
    let url = format!("{}{CONTROL}{kind}/send", server.trim_end_matches('/'));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| failed(format!("no runtime: {e}")))?;
    runtime.block_on(async {
        // reqwest is built without a default provider (no aws-lc).
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(wait)
            .user_agent(concat!("chrome-cdp/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| failed(format!("cannot make an HTTP client: {e}")))?;
        let response = client
            .post(&url)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| failed(format!("cannot reach the mock server at {server}: {e}")))?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(|e| {
            failed(format!(
                "{url} answered {status}, but the body broke off: {e}"
            ))
        })?;
        let answer: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| failed(format!("{url} answered {status}, not with JSON: {e}")))?;
        if status == 200 {
            return serde_json::from_value(answer["to"].clone())
                .map_err(|e| failed(format!("{url} answered with an unknown shape: {e}")));
        }
        let message = answer["error"]
            .as_str()
            .map_or_else(|| format!("{url} answered {status}"), str::to_owned);
        Err(PushError {
            message,
            open: answer["open"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|u| u.as_str().map(str::to_owned))
                .collect(),
            none_matched: status == 404 && answer.get("open").is_some(),
        })
    })
}
