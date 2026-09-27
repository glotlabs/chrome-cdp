//! Error types for the CDP client.

use std::io;
use std::time::Duration;

/// A failure of the underlying byte transport (pipe or WebSocket).
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// An I/O error on the pipe or socket.
    #[error("transport I/O error: {0}")]
    Io(#[from] io::Error),
    /// A WebSocket protocol or handshake error.
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    /// The transport was already closed.
    #[error("transport closed")]
    Closed,
}

impl From<tokio_tungstenite::tungstenite::Error> for TransportError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(Box::new(e))
    }
}

/// An error from a CDP call or connection.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CdpError {
    /// Chrome answered the call with a protocol error.
    #[error("{method}: {message} (code {code})")]
    Protocol {
        /// The method that failed.
        method: String,
        /// The JSON-RPC error code, e.g. `-32601` for an unknown method.
        code: i64,
        /// Chrome's error message.
        message: String,
        /// Extra detail Chrome sometimes attaches.
        data: Option<String>,
    },
    /// The connection to Chrome is gone (Chrome exited, the pipe or socket
    /// closed, or the transport failed). Every pending and later call fails
    /// with this.
    #[error("disconnected from Chrome: {reason}")]
    Disconnected {
        /// Why the connection closed.
        reason: String,
    },
    /// Chrome did not answer within the call's timeout.
    #[error("{method}: no response from Chrome after {}ms", .after.as_millis())]
    Timeout {
        /// The method that timed out.
        method: String,
        /// The timeout that elapsed.
        after: Duration,
    },
    /// The params could not be serialised to JSON.
    #[error("{method}: cannot serialise params: {message}")]
    Serialize {
        /// The method being called.
        method: String,
        /// The serde error.
        message: String,
    },
    /// Chrome's result did not match the expected type.
    #[error("{method}: unexpected result from Chrome: {message}")]
    Decode {
        /// The method whose result failed to decode.
        method: String,
        /// The serde error.
        message: String,
    },
}

impl CdpError {
    pub(crate) fn disconnected(reason: impl Into<String>) -> Self {
        Self::Disconnected {
            reason: reason.into(),
        }
    }
}

/// Shorthand for results of CDP calls.
pub type Result<T, E = CdpError> = std::result::Result<T, E>;
