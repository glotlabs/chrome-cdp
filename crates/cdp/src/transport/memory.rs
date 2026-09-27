//! An in-process transport for tests.

use bytes::Bytes;
use tokio::sync::mpsc;

use super::{Transport, TransportReceiver, TransportSender};
use crate::error::TransportError;

/// A transport backed by channels. The other end is a [`MemoryPeer`] that
/// plays Chrome: it sees every message the connection sends and can push
/// responses and events back.
#[derive(Debug)]
pub struct MemoryTransport {
    to_peer: mpsc::UnboundedSender<Bytes>,
    from_peer: mpsc::UnboundedReceiver<Bytes>,
}

/// The Chrome side of a [`MemoryTransport`].
#[derive(Debug)]
pub struct MemoryPeer {
    /// Messages the connection sent.
    pub incoming: mpsc::UnboundedReceiver<Bytes>,
    outgoing: Option<mpsc::UnboundedSender<Bytes>>,
}

impl MemoryTransport {
    /// Creates a connected transport/peer pair.
    pub fn pair() -> (Self, MemoryPeer) {
        let (to_peer, incoming) = mpsc::unbounded_channel();
        let (outgoing, from_peer) = mpsc::unbounded_channel();
        (
            Self { to_peer, from_peer },
            MemoryPeer {
                incoming,
                outgoing: Some(outgoing),
            },
        )
    }
}

impl MemoryPeer {
    /// Sends a raw message to the connection. Returns `false` if the
    /// connection is gone or the peer already closed.
    pub fn send(&self, msg: impl Into<Bytes>) -> bool {
        self.outgoing
            .as_ref()
            .is_some_and(|tx| tx.send(msg.into()).is_ok())
    }

    /// Sends a JSON value to the connection.
    pub fn send_json(&self, msg: &serde_json::Value) -> bool {
        self.send(msg.to_string())
    }

    /// Receives the next message the connection sent, parsed as JSON.
    /// Returns `None` when the connection closed its sending half.
    pub async fn recv_json(&mut self) -> Option<serde_json::Value> {
        let msg = self.incoming.recv().await?;
        serde_json::from_slice(&msg).ok()
    }

    /// Closes the peer's sending direction, like Chrome exiting.
    pub fn close(&mut self) {
        self.outgoing = None;
    }
}

/// Sending half of a [`MemoryTransport`].
#[derive(Debug)]
pub struct MemorySender(Option<mpsc::UnboundedSender<Bytes>>);

/// Receiving half of a [`MemoryTransport`].
#[derive(Debug)]
pub struct MemoryReceiver(mpsc::UnboundedReceiver<Bytes>);

impl Transport for MemoryTransport {
    type Sender = MemorySender;
    type Receiver = MemoryReceiver;

    fn split(self) -> (MemorySender, MemoryReceiver) {
        (
            MemorySender(Some(self.to_peer)),
            MemoryReceiver(self.from_peer),
        )
    }
}

impl TransportSender for MemorySender {
    fn send(&mut self, msg: Bytes) -> impl Future<Output = Result<(), TransportError>> + Send {
        let sent = match &self.0 {
            Some(tx) => tx.send(msg).map_err(|_| TransportError::Closed),
            None => Err(TransportError::Closed),
        };
        std::future::ready(sent)
    }

    fn close(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send {
        self.0 = None;
        std::future::ready(Ok(()))
    }
}

impl TransportReceiver for MemoryReceiver {
    async fn recv(&mut self) -> Result<Option<Bytes>, TransportError> {
        Ok(self.0.recv().await)
    }
}
