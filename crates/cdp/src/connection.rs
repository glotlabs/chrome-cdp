//! A CDP connection: request/response correlation and event routing over a
//! [`Transport`].
//!
//! One writer task owns the transport's sending half and one reader task owns
//! its receiving half. Calls allocate an id from an atomic counter, register a
//! oneshot in the pending map, and hand the serialised request to the writer.
//! The reader completes pending calls by id and routes events by `sessionId`
//! to per-session broadcast channels.
//!
//! When the transport closes or fails, every pending call fails with
//! [`CdpError::Disconnected`], every event stream ends, and later calls fail
//! immediately. Nothing waits forever.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use crate::Event as _;
use crate::domains::target;
use crate::error::{CdpError, Result};
use crate::log::Truncated;
use crate::session::Session;
use crate::transport::{Transport, TransportReceiver, TransportSender};

/// Tuning for a [`Connection`].
#[derive(Debug, Clone)]
pub struct ConnectionOptions {
    /// Timeout for calls that do not set their own
    /// (see [`Session::with_timeout`]). Default: 30 s.
    pub call_timeout: Duration,
    /// Events buffered per session before slow listeners start missing them.
    /// Default: 1024.
    pub event_buffer: usize,
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            call_timeout: Duration::from_secs(30),
            event_buffer: 1024,
        }
    }
}

/// An event as it came off the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct RawEvent {
    /// The event name, e.g. `"Page.loadEventFired"`.
    pub method: String,
    /// The event's params (an empty object if Chrome sent none).
    pub params: Value,
    /// The flat session the event belongs to; `None` for the browser session.
    pub session_id: Option<String>,
}

/// A connection to Chrome. Cheap to clone; all clones share one transport.
///
/// Dropping the last clone (and every [`Session`] made from it) stops the
/// reader task and closes the transport, which makes a pipe-launched Chrome
/// exit.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

struct Inner {
    shared: Arc<Shared>,
    outgoing: mpsc::UnboundedSender<Bytes>,
    next_id: AtomicU64,
    options: ConnectionOptions,
    reader: AbortHandle,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The writer stops by itself once `outgoing` is dropped, after
        // flushing queued messages and closing the transport. The reader may
        // be blocked on a peer that never closes (a WebSocket), so stop it.
        self.reader.abort();
    }
}

/// State shared by the connection handles and the two I/O tasks.
struct Shared {
    /// Calls waiting for a response, by request id.
    pending: DashMap<u64, oneshot::Sender<Response>>,
    /// Event channels by session id (`""` for the browser session). Created
    /// when someone listens; removed when the session detaches.
    sessions: DashMap<String, broadcast::Sender<Arc<RawEvent>>>,
    /// Sessions that detached lately, oldest first, so a listener that
    /// comes too late gets a stream that has already ended instead of one
    /// no event will ever reach.
    detached: Mutex<VecDeque<String>>,
    /// `Some(reason)` once the connection has closed.
    closed: watch::Sender<Option<Arc<str>>>,
    event_buffer: usize,
}

/// How many detached session ids [`Shared::detached`] remembers.
const DETACHED_MEMORY: usize = 512;

/// What the reader hands a pending call.
type Response = std::result::Result<Value, ResponseError>;

enum ResponseError {
    Protocol {
        code: i64,
        message: String,
        data: Option<String>,
    },
    Disconnected(Arc<str>),
}

impl Connection {
    /// Starts a connection over `transport` with default options.
    ///
    /// Must be called inside a Tokio runtime: it spawns the reader and writer
    /// tasks.
    pub fn new<T: Transport>(transport: T) -> Self {
        Self::with_options(transport, ConnectionOptions::default())
    }

    /// Starts a connection over `transport`.
    pub fn with_options<T: Transport>(transport: T, options: ConnectionOptions) -> Self {
        let (sender, receiver) = transport.split();
        let shared = Arc::new(Shared {
            pending: DashMap::new(),
            sessions: DashMap::new(),
            detached: Mutex::default(),
            closed: watch::Sender::new(None),
            event_buffer: options.event_buffer.max(1),
        });
        let (outgoing, queue) = mpsc::unbounded_channel();
        tokio::spawn(write_loop(sender, queue, shared.clone()));
        let reader = tokio::spawn(read_loop(receiver, shared.clone())).abort_handle();
        Self {
            inner: Arc::new(Inner {
                shared,
                outgoing,
                next_id: AtomicU64::new(1),
                options,
                reader,
            }),
        }
    }

    /// The browser-level session (no `sessionId`).
    pub fn browser(&self) -> Session {
        Session::new(self.clone(), None)
    }

    /// A handle to an existing flat session.
    pub fn session(&self, session_id: impl Into<Arc<str>>) -> Session {
        Session::new(self.clone(), Some(session_id.into()))
    }

    /// Attaches to a target with `flatten: true` and returns its session.
    pub async fn attach(&self, target_id: impl Into<String>) -> Result<Session> {
        let mut params = target::AttachToTargetParams::new(target_id.into());
        params.flatten = Some(true);
        let attached = self.browser().call(params).await?;
        Ok(self.session(attached.session_id))
    }

    /// Whether the connection has closed.
    pub fn is_closed(&self) -> bool {
        self.inner.shared.closed_reason().is_some()
    }

    /// Waits until the connection closes and returns why.
    pub async fn closed(&self) -> Arc<str> {
        let mut rx = self.inner.shared.closed.subscribe();
        match rx.wait_for(Option::is_some).await {
            Ok(reason) => reason.clone().unwrap_or_else(|| "closed".into()),
            // Unreachable while `self` holds the sender, but not worth a panic.
            Err(_) => "closed".into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_calls(&self) -> usize {
        self.inner.shared.pending.len()
    }

    pub(crate) fn default_timeout(&self) -> Duration {
        self.inner.options.call_timeout
    }

    /// Subscribes to the events of one session (`None` = browser).
    pub(crate) fn subscribe(&self, session_id: Option<&str>) -> broadcast::Receiver<Arc<RawEvent>> {
        self.inner.shared.subscribe(session_id.unwrap_or_default())
    }

    /// Sends one request now and returns a future for its response.
    ///
    /// The request is queued before this returns, not when the future is
    /// first polled, so requests made one after another reach Chrome in
    /// that order even when their futures are awaited together.
    ///
    /// Cancel safe: dropping the future removes the pending entry, and a late
    /// response is ignored.
    pub(crate) fn request<P: Serialize + ?Sized>(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: &P,
        timeout: Duration,
    ) -> impl Future<Output = Result<Value>> + Send + 'static + use<P> {
        let sent = self.send(session_id, method, params);
        response(sent, method.to_owned(), timeout)
    }

    /// Registers a request as pending and queues it for the writer.
    fn send<P: Serialize + ?Sized>(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: &P,
    ) -> Result<(oneshot::Receiver<Response>, PendingGuard)> {
        let shared = &self.inner.shared;
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = serde_json::to_vec(&Request {
            id,
            method,
            params,
            session_id,
        })
        .map_err(|e| CdpError::Serialize {
            method: method.to_owned(),
            message: e.to_string(),
        })?;

        let (tx, rx) = oneshot::channel();
        shared.pending.insert(id, tx);
        let guard = PendingGuard {
            shared: shared.clone(),
            id,
        };
        // Checked after inserting: `Shared::close` sets the reason before it
        // drains `pending`, so either we see the reason here or the drain sees
        // our entry.
        if let Some(reason) = shared.closed_reason() {
            return Err(CdpError::disconnected(&*reason));
        }
        if self.inner.outgoing.send(Bytes::from(msg)).is_err() {
            return Err(CdpError::disconnected("the writer task stopped"));
        }
        Ok((rx, guard))
    }
}

/// Waits for the response to a request [`Connection::send`] queued.
fn response(
    sent: Result<(oneshot::Receiver<Response>, PendingGuard)>,
    method: String,
    timeout: Duration,
) -> impl Future<Output = Result<Value>> + Send + 'static {
    async move {
        let (rx, guard) = sent?;
        let response = tokio::time::timeout(timeout, rx).await;
        let shared = &guard.shared;
        match response {
            Err(_) => Err(CdpError::Timeout {
                method,
                after: timeout,
            }),
            Ok(Err(_)) => Err(CdpError::disconnected(
                shared
                    .closed_reason()
                    .as_deref()
                    .unwrap_or("connection dropped"),
            )),
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(ResponseError::Protocol {
                code,
                message,
                data,
            }))) => Err(CdpError::Protocol {
                method,
                code,
                message,
                data,
            }),
            Ok(Ok(Err(ResponseError::Disconnected(reason)))) => {
                Err(CdpError::disconnected(&*reason))
            }
        }
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("pending", &self.inner.shared.pending.len())
            .field("closed", &self.inner.shared.closed_reason())
            .finish_non_exhaustive()
    }
}

/// Removes a call's pending entry when the call finishes or is dropped.
struct PendingGuard {
    shared: Arc<Shared>,
    id: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.shared.pending.remove(&self.id);
    }
}

#[derive(Serialize)]
struct Request<'a, P: ?Sized> {
    id: u64,
    method: &'a str,
    params: &'a P,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
}

/// Any message from Chrome: a response (`id`) or an event (`method`).
#[derive(Deserialize)]
struct Incoming {
    id: Option<u64>,
    method: Option<String>,
    params: Option<Value>,
    result: Option<Value>,
    error: Option<IncomingError>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct IncomingError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl Shared {
    fn closed_reason(&self) -> Option<Arc<str>> {
        self.closed.borrow().clone()
    }

    /// Marks the connection closed, fails every pending call and ends every
    /// event stream. Only the first reason is kept.
    fn close(&self, reason: &str) {
        let first = self.closed.send_if_modified(|closed| {
            if closed.is_some() {
                return false;
            }
            *closed = Some(reason.into());
            true
        });
        if first {
            tracing::debug!(target: "cdp::connection", reason, "connection closed");
        }
        let reason = self.closed_reason().unwrap_or_else(|| reason.into());
        let ids: Vec<u64> = self.pending.iter().map(|e| *e.key()).collect();
        for id in ids {
            if let Some((_, tx)) = self.pending.remove(&id) {
                let _ = tx.send(Err(ResponseError::Disconnected(reason.clone())));
            }
        }
        // Dropping the senders ends every listener's stream.
        self.sessions.clear();
    }

    fn subscribe(&self, key: &str) -> broadcast::Receiver<Arc<RawEvent>> {
        let rx = self
            .sessions
            .entry(key.to_owned())
            .or_insert_with(|| broadcast::channel(self.event_buffer).0)
            .subscribe();
        // Same ordering argument as for pending calls: if the connection
        // closed or the session detached meanwhile, `close` or `dispatch`
        // may have missed our channel. Removing it drops the sender, which
        // ends the stream.
        if self.closed_reason().is_some() || (!key.is_empty() && self.was_detached(key)) {
            self.sessions.remove(key);
        }
        rx
    }

    fn was_detached(&self, session_id: &str) -> bool {
        let detached = self.detached.lock().unwrap_or_else(PoisonError::into_inner);
        detached.iter().any(|id| id == session_id)
    }

    fn remember_detached(&self, session_id: &str) {
        let mut detached = self.detached.lock().unwrap_or_else(PoisonError::into_inner);
        if detached.len() == DETACHED_MEMORY {
            detached.pop_front();
        }
        detached.push_back(session_id.to_owned());
    }

    fn dispatch(&self, frame: &[u8]) {
        let msg: Incoming = match serde_json::from_slice(frame) {
            Ok(msg) => msg,
            Err(e) => {
                tracing::warn!(target: "cdp::connection", error = %e, frame = %Truncated(frame), "ignoring malformed message from Chrome");
                return;
            }
        };
        if let Some(id) = msg.id {
            let response = match msg.error {
                Some(e) => Err(ResponseError::Protocol {
                    code: e.code,
                    message: e.message,
                    data: e.data.map(|d| match d {
                        Value::String(s) => s,
                        other => other.to_string(),
                    }),
                }),
                None => Ok(msg.result.unwrap_or_else(|| Value::Object(Map::default()))),
            };
            match self.pending.remove(&id) {
                Some((_, tx)) => {
                    let _ = tx.send(response);
                }
                None => {
                    tracing::debug!(target: "cdp::connection", id, "response for a call that is gone (timed out or cancelled)");
                }
            }
        } else if let Some(method) = msg.method {
            let key = msg.session_id.as_deref().unwrap_or_default();
            let detached = (method == target::DetachedFromTarget::NAME)
                .then(|| {
                    msg.params
                        .as_ref()?
                        .get("sessionId")?
                        .as_str()
                        .map(str::to_owned)
                })
                .flatten();
            if let Some(tx) = self.sessions.get(key)
                && tx.receiver_count() > 0
            {
                let _ = tx.send(Arc::new(RawEvent {
                    method,
                    params: msg.params.unwrap_or_else(|| Value::Object(Map::default())),
                    session_id: msg.session_id,
                }));
            }
            // The session is gone: end its listeners' streams.
            if let Some(session_id) = detached {
                self.remember_detached(&session_id);
                self.sessions.remove(&session_id);
            }
        } else {
            tracing::warn!(target: "cdp::connection", frame = %Truncated(frame), "ignoring message with neither id nor method");
        }
    }
}

/// Resolves once the connection is marked closed. Does not hold the watch
/// guard across the await, so the I/O tasks stay `Send`.
async fn wait_closed(closed: &mut watch::Receiver<Option<Arc<str>>>) {
    let _ = closed.wait_for(Option::is_some).await;
}

async fn write_loop<S: TransportSender>(
    mut sender: S,
    mut queue: mpsc::UnboundedReceiver<Bytes>,
    shared: Arc<Shared>,
) {
    let mut closed = shared.closed.subscribe();
    loop {
        tokio::select! {
            msg = queue.recv() => {
                let Some(msg) = msg else { break };
                tracing::trace!(target: "cdp::wire", "send {}", Truncated(&msg));
                if let Err(e) = sender.send(msg).await {
                    shared.close(&format!("sending to Chrome failed: {e}"));
                    break;
                }
            }
            () = wait_closed(&mut closed) => break,
        }
    }
    if let Err(e) = sender.close().await {
        tracing::debug!(target: "cdp::connection", error = %e, "closing the transport failed");
    }
}

async fn read_loop<R: TransportReceiver>(mut receiver: R, shared: Arc<Shared>) {
    let mut closed = shared.closed.subscribe();
    let reason = loop {
        tokio::select! {
            frame = receiver.recv() => match frame {
                Ok(Some(frame)) => {
                    tracing::trace!(target: "cdp::wire", "recv {}", Truncated(&frame));
                    shared.dispatch(&frame);
                }
                Ok(None) => break "Chrome closed the connection".to_owned(),
                Err(e) => break format!("receiving from Chrome failed: {e}"),
            },
            () = wait_closed(&mut closed) => return,
        }
    };
    shared.close(&reason);
}

#[cfg(test)]
mod tests;
