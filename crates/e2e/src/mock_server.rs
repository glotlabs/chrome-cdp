//! `mock serve` (plan 11, milestone 11.9): the mock sets as a real HTTP
//! server, for the requests that never pass through the browser, such as
//! an SSR framework's server-side fetches.
//!
//! [`MockServer`] answers every request with the same engine the daemon's
//! router uses ([`mock::Mocks`]), on a thread of its own, so the CLI and
//! the test runner can both hold one without an async runtime.
//!
//! State (hit counters, sequences, named values, collections) is kept per
//! **partition**. A request with an `x-chrome-cdp-test: <id>` header
//! ([`TEST_HEADER`]) uses partition `<id>`, made from the served sets when
//! first seen, and a request without it the shared one. `chrome-cdp test`
//! sends the header with its browser requests, and makes each test's
//! partition from the test's own sets ([`MockServer::partition`]); an app
//! that forwards the header to its API calls gets per-test state, and one
//! that does not shares the state across tests.
//!
//! Answers are as in the browser, except for what only a browser can do:
//! `abort` closes the connection without an answer, `passthrough` (with no
//! real server behind) answers 502, and a request no set owns answers 404.
//! An `sse` answer streams its events, each after its delay, and then
//! keeps the stream open.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt as _, Full, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use mock::{MockSet, Mocks, Outcome, SseEvent};
use protocol::schema::RoutedRequest;
use serde::Serialize;
use serde_json::json;

/// The request header that picks a request's partition.
pub const TEST_HEADER: &str = "x-chrome-cdp-test";

/// How many requests the journal keeps.
const JOURNAL: usize = 1000;

/// How many header-made partitions are kept; the oldest go first.
const PARTITIONS: usize = 1000;

/// The largest request body read.
const MAX_BODY: usize = 50 * 1024 * 1024;

/// A request the server answered, with its partition.
#[derive(Debug, Clone, Serialize)]
pub struct Served {
    /// The `x-chrome-cdp-test` header, if the request had one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test: Option<String>,
    #[serde(flatten)]
    pub request: RoutedRequest,
}

/// Called with each request answered (`mock serve` prints them).
pub type OnRequest = Box<dyn Fn(&Served) + Send + Sync>;

/// A running mock server; dropping it stops it.
pub struct MockServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for MockServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockServer")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

struct Shared {
    /// What a partition made from a header starts with.
    sets: Vec<MockSet>,
    seed: u64,
    state: Mutex<State>,
    on_request: Option<OnRequest>,
}

struct State {
    shared: Partition,
    partitions: HashMap<String, Partition>,
    /// Header-made partitions, oldest first.
    made: VecDeque<String>,
    journal: VecDeque<Served>,
    seq: u64,
}

struct Partition {
    mocks: Mocks,
    /// `{{now}}`: a frozen time, else the clock.
    now_ms: Option<i64>,
}

impl Partition {
    fn new(sets: Vec<MockSet>, seed: u64, now_ms: Option<i64>) -> Self {
        let mut mocks = Mocks::new(seed);
        mocks.apply(sets, false);
        Self { mocks, now_ms }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MockServer {
    /// Starts serving `sets` (in layering order) on `addr` (port 0 picks
    /// a free one); random values follow `seed`. An address that cannot
    /// be bound is the error.
    pub fn start(
        addr: SocketAddr,
        sets: Vec<MockSet>,
        seed: u64,
        on_request: Option<OnRequest>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                shared: Partition::new(sets.clone(), seed, None),
                partitions: HashMap::new(),
                made: VecDeque::new(),
                journal: VecDeque::new(),
                seq: 0,
            }),
            sets,
            seed,
            on_request,
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let served = shared.clone();
        let thread = std::thread::Builder::new()
            .name("mock-server".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    tokio::select! {
                        () = accept(listener, served) => {}
                        _ = stopped => {}
                    }
                });
                // Dropping the runtime ends the connections still open.
            })?;
        Ok(Self {
            addr,
            shared,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// Where it listens.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:4010`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Makes partition `id` afresh from `sets` (a test's own), with
    /// `{{now}}` frozen at `now_ms` if given.
    pub fn partition(&self, id: &str, sets: Vec<MockSet>, now_ms: Option<i64>) {
        let partition = Partition::new(sets, self.shared.seed, now_ms);
        let mut state = lock(&self.shared.state);
        state.made.retain(|m| m != id);
        state.partitions.insert(id.to_owned(), partition);
    }

    /// Forgets partition `id` and its journal entries.
    pub fn close_partition(&self, id: &str) {
        let mut state = lock(&self.shared.state);
        state.partitions.remove(id);
        state.made.retain(|m| m != id);
        state.journal.retain(|s| s.test.as_deref() != Some(id));
    }

    /// The requests of partition `test` (`None`: those without the
    /// header), oldest first.
    pub fn journal(&self, test: Option<&str>) -> Vec<RoutedRequest> {
        lock(&self.shared.state)
            .journal
            .iter()
            .filter(|s| s.test.as_deref() == test)
            .map(|s| s.request.clone())
            .collect()
    }

    /// Stops it and waits for its thread.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.halt();
    }
}

async fn accept(listener: tokio::net::TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let local = stream.local_addr().ok();
        let shared = shared.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let shared = shared.clone();
                async move { handle(&shared, req, local).await }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

type Body = BoxBody<Bytes, Infallible>;

/// `abort`: the service fails, so hyper closes the connection unanswered.
#[derive(Debug)]
struct Aborted;

impl std::fmt::Display for Aborted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("aborted by a mock")
    }
}

impl std::error::Error for Aborted {}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// What a request gets, worked out under the lock.
enum Reply {
    Fulfill {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        events: Option<Vec<SseEvent>>,
        delay: Duration,
    },
    Abort(Duration),
}

async fn handle(
    shared: &Shared,
    req: Request<Incoming>,
    local: Option<SocketAddr>,
) -> Result<Response<Body>, Aborted> {
    let (parts, body) = req.into_parts();
    let body = match Limited::new(body, MAX_BODY).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return Ok(plain(
                413,
                &json!({"error": "chrome-cdp mock serve: the request body is too large"}),
            ));
        }
    };
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .filter_map(|(n, v)| Some((n.as_str().to_owned(), v.to_str().ok()?.to_owned())))
        .collect();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let host = header("host")
        .or_else(|| local.map(|a| a.to_string()))
        .unwrap_or_else(|| "localhost".into());
    let target = parts
        .uri
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str);
    let url = format!("http://{host}{target}");
    let method = parts.method.as_str().to_owned();
    let origin = header("origin");
    let test = header(TEST_HEADER).filter(|t| !t.is_empty());
    let request = mock::Request {
        method: &method,
        url: &url,
        headers: &headers,
        body: (!body.is_empty()).then_some(&body[..]),
    };
    let reply = decide(shared, &request, test, origin.as_deref());
    match reply {
        Reply::Abort(delay) => {
            tokio::time::sleep(delay).await;
            Err(Aborted)
        }
        Reply::Fulfill {
            status,
            headers,
            body,
            events,
            delay,
        } => {
            tokio::time::sleep(delay).await;
            let mut response = Response::builder().status(status);
            for (name, value) in &headers {
                response = response.header(name, value);
            }
            let body = match events {
                Some(events) => stream(events),
                None => Full::new(Bytes::from(body)).boxed(),
            };
            Ok(response
                .body(body)
                .unwrap_or_else(|_| plain(500, &json!({"error": "bad mock headers"}))))
        }
    }
}

/// An `sse` answer's events, each after its delay; then the stream stays
/// open until the client goes.
fn stream(events: Vec<SseEvent>) -> Body {
    let frames = futures_util::stream::unfold(events.into_iter(), |mut events| async move {
        match events.next() {
            Some(event) => {
                tokio::time::sleep(event.delay).await;
                let frame = Frame::data(Bytes::from(event.text()));
                Some((Ok::<_, Infallible>(frame), events))
            }
            None => futures_util::future::pending().await,
        }
    });
    StreamBody::new(frames).boxed()
}

fn plain(status: u16, value: &serde_json::Value) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::from(value.to_string())).boxed());
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    response
}

fn decide(
    shared: &Shared,
    request: &mock::Request<'_>,
    test: Option<String>,
    origin: Option<&str>,
) -> Reply {
    let mut state = lock(&shared.state);
    let state = &mut *state;
    let partition = match &test {
        None => &mut state.shared,
        Some(id) => {
            if !state.partitions.contains_key(id) {
                if state.made.len() >= PARTITIONS
                    && let Some(old) = state.made.pop_front()
                {
                    state.partitions.remove(&old);
                }
                state.made.push_back(id.clone());
                state.partitions.insert(
                    id.clone(),
                    Partition::new(shared.sets.clone(), shared.seed, None),
                );
            }
            state
                .partitions
                .get_mut(id)
                .unwrap_or_else(|| unreachable!("just made"))
        }
    };
    partition.mocks.refresh();
    let now = partition.now_ms.unwrap_or_else(now_ms);
    let mut entry = RoutedRequest {
        seq: 0,
        time_ms: u64::try_from(now_ms()).unwrap_or(0),
        tab: None,
        method: request.method.to_owned(),
        url: request.url.to_owned(),
        resource_type: "server".into(),
        route: None,
        mock: None,
        unmatched: None,
        policy: None,
        closest: Vec::new(),
        outcome: String::new(),
    };
    let (reply, outcome) = match partition.mocks.decide(request, now) {
        Outcome::Answer(answer) => {
            entry.mock = Some(answer.mock);
            let (reply, note) = reply(answer.response, answer.events, answer.delay, origin);
            let outcome = match note {
                Some(note) => format!("{} ({note})", answer.outcome),
                None => answer.outcome,
            };
            (reply, outcome)
        }
        Outcome::Preflight { mock } => {
            entry.mock = mock;
            (preflight(request, origin), "preflight allowed".to_owned())
        }
        Outcome::Unmatched {
            set,
            policy,
            response,
            outcome,
            closest,
        } => {
            entry.unmatched = Some(set);
            entry.policy = Some(policy.as_str().to_owned());
            entry.closest = closest;
            let (reply, note) = reply(response, None, Duration::ZERO, origin);
            let outcome = match note {
                Some(note) => format!("{outcome} ({note})"),
                None => outcome,
            };
            (reply, outcome)
        }
        Outcome::Pass => {
            let body = json!({
                "error": format!(
                    "chrome-cdp mock serve: no mock set has {} {}",
                    request.method, request.url
                ),
            });
            (
                Reply::Fulfill {
                    status: 404,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: body.to_string().into_bytes(),
                    events: None,
                    delay: Duration::ZERO,
                },
                "no mock, 404".to_owned(),
            )
        }
    };
    entry.outcome = outcome;
    state.seq += 1;
    entry.seq = state.seq;
    let served = Served {
        test,
        request: entry,
    };
    if let Some(on_request) = &shared.on_request {
        on_request(&served);
    }
    if state.journal.len() == JOURNAL {
        state.journal.pop_front();
    }
    state.journal.push_back(served);
    reply
}

/// A mock's response as an HTTP answer; the note says what the browser
/// would have done instead.
fn reply(
    response: mock::Response,
    events: Option<Vec<SseEvent>>,
    delay: Duration,
    origin: Option<&str>,
) -> (Reply, Option<&'static str>) {
    match response {
        mock::Response::Fulfill {
            status,
            mut headers,
            body,
            cors,
        } => {
            let has = |h: &[(String, String)], name: &str| {
                h.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
            };
            if let Some(origin) = origin
                && cors
                && !has(&headers, "Access-Control-Allow-Origin")
            {
                let exposed: Vec<String> = headers
                    .iter()
                    .map(|(n, _)| n.clone())
                    .filter(|n| {
                        !n.eq_ignore_ascii_case("Content-Type")
                            && !n.eq_ignore_ascii_case("Set-Cookie")
                    })
                    .collect();
                headers.extend(allow_origin(origin));
                if !exposed.is_empty() && !has(&headers, "Access-Control-Expose-Headers") {
                    headers.push(("Access-Control-Expose-Headers".into(), exposed.join(", ")));
                }
            }
            (
                Reply::Fulfill {
                    status,
                    headers,
                    body,
                    events,
                    delay,
                },
                None,
            )
        }
        mock::Response::Abort { .. } => (Reply::Abort(delay), Some("connection closed")),
        mock::Response::Passthrough { .. } => {
            let body = json!({
                "error": "chrome-cdp mock serve: `passthrough` sends a request to the real server, and mock serve has none",
            });
            (
                Reply::Fulfill {
                    status: 502,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: body.to_string().into_bytes(),
                    events: None,
                    delay,
                },
                Some("502: no real server behind mock serve"),
            )
        }
    }
}

/// The headers that allow `origin`'s requests, with credentials.
fn allow_origin(origin: &str) -> [(String, String); 3] {
    [
        ("Access-Control-Allow-Origin".into(), origin.to_owned()),
        ("Access-Control-Allow-Credentials".into(), "true".into()),
        ("Vary".into(), "Origin".into()),
    ]
}

/// The answer to a CORS preflight: whatever it asks for is allowed.
fn preflight(request: &mock::Request<'_>, origin: Option<&str>) -> Reply {
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let mut headers: Vec<(String, String)> = match origin {
        Some(origin) => allow_origin(origin).into(),
        None => vec![("Access-Control-Allow-Origin".into(), "*".into())],
    };
    if let Some(method) = header("access-control-request-method") {
        headers.push(("Access-Control-Allow-Methods".into(), method));
    }
    if let Some(asked) = header("access-control-request-headers") {
        headers.push(("Access-Control-Allow-Headers".into(), asked));
    }
    Reply::Fulfill {
        status: 204,
        headers,
        body: Vec::new(),
        events: None,
        delay: Duration::ZERO,
    }
}
