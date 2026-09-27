//! Session handles and typed event streams.

use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use serde_json::Value;
use tokio_stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::connection::{Connection, RawEvent};
use crate::error::{CdpError, Result};
use crate::{Command, Event};

/// A handle to the browser session or one flat target session.
///
/// Cheap to clone. Calls through it carry its `sessionId`, and its event
/// streams see only its events.
#[derive(Debug, Clone)]
pub struct Session {
    conn: Connection,
    id: Option<Arc<str>>,
    timeout: Option<Duration>,
}

impl Session {
    pub(crate) fn new(conn: Connection, session_id: Option<Arc<str>>) -> Self {
        Self {
            conn,
            id: session_id,
            timeout: None,
        }
    }

    /// The session id, or `None` for the browser session.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The connection this session belongs to.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// A copy of this handle whose calls time out after `timeout` instead of
    /// the connection's default.
    #[must_use]
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
            ..self.clone()
        }
    }

    fn timeout(&self) -> Duration {
        self.timeout.unwrap_or_else(|| self.conn.default_timeout())
    }

    /// Calls a typed command and decodes its result.
    ///
    /// The command is sent when `call` is called, not when the future is
    /// first polled: calls made one after another reach Chrome in that
    /// order even when awaited together (e.g. setting up a paused target
    /// and resuming it, where an early answer would wait for the resume).
    pub fn call<C: Command>(
        &self,
        params: C,
    ) -> impl Future<Output = Result<C::Returns>> + Send + 'static {
        let response = self
            .conn
            .request(self.id(), C::METHOD, &params, self.timeout());
        async move {
            let result = response.await?;
            serde_json::from_value(result).map_err(|e| CdpError::Decode {
                method: C::METHOD.to_owned(),
                message: e.to_string(),
            })
        }
    }

    /// Calls any method with JSON params and returns the raw JSON result.
    /// Sent at once, like [`Session::call`].
    pub fn call_raw(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value>> + Send + 'static {
        self.conn
            .request(self.id(), method, &params, self.timeout())
    }

    /// Streams this session's events of type `E`.
    ///
    /// Only events that arrive after this call are seen, so listen before
    /// sending the command that triggers the event. The stream ends when the
    /// session detaches or the connection closes.
    pub fn listen<E: Event>(&self) -> EventStream<E> {
        EventStream {
            inner: self.events(),
            _event: PhantomData,
        }
    }

    /// Streams all of this session's events, untyped.
    pub fn events(&self) -> RawEventStream {
        RawEventStream {
            inner: BroadcastStream::new(self.conn.subscribe(self.id())),
        }
    }
}

/// A stream of one session's raw events. See [`Session::events`].
///
/// If the consumer falls more than `event_buffer` events behind, the oldest
/// are skipped (and logged); the stream continues.
pub struct RawEventStream {
    inner: BroadcastStream<Arc<RawEvent>>,
}

impl RawEventStream {
    /// The next event, or `None` once the session or connection is gone.
    pub async fn next(&mut self) -> Option<Arc<RawEvent>> {
        std::future::poll_fn(|cx| Pin::new(&mut *self).poll_next(cx)).await
    }
}

impl Stream for RawEventStream {
    type Item = Arc<RawEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => return Poll::Ready(Some(event)),
                Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(skipped)))) => {
                    tracing::warn!(target: "cdp::connection", skipped, "event listener fell behind; events were dropped");
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl std::fmt::Debug for RawEventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawEventStream").finish_non_exhaustive()
    }
}

/// A stream of one session's events of type `E`. See [`Session::listen`].
pub struct EventStream<E> {
    inner: RawEventStream,
    _event: PhantomData<fn() -> E>,
}

impl<E: Event> EventStream<E> {
    /// The next event, or `None` once the session or connection is gone.
    pub async fn next(&mut self) -> Option<E> {
        std::future::poll_fn(|cx| Pin::new(&mut *self).poll_next(cx)).await
    }
}

impl<E: Event> Stream for EventStream<E> {
    type Item = E;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<E>> {
        loop {
            let Some(raw) = std::task::ready!(Pin::new(&mut self.inner).poll_next(cx)) else {
                return Poll::Ready(None);
            };
            if raw.method != E::NAME {
                continue;
            }
            match E::deserialize(&raw.params) {
                Ok(event) => return Poll::Ready(Some(event)),
                Err(e) => {
                    tracing::warn!(target: "cdp::connection", event = E::NAME, error = %e, "cannot decode event; skipping it");
                }
            }
        }
    }
}

impl<E> std::fmt::Debug for EventStream<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventStream")
            .field("event", &std::any::type_name::<E>())
            .finish_non_exhaustive()
    }
}
