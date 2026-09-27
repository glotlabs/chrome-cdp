//! Byte transports that carry CDP JSON messages.
//!
//! A transport is split into a [`TransportSender`] and a
//! [`TransportReceiver`] so the connection's writer task and reader task each
//! own one half without locking.
//!
//! - [`PipeTransport`] (Unix): `--remote-debugging-pipe`, NUL-delimited
//!   frames on fds 3 and 4. Used for browsers we launch.
//! - [`WsTransport`]: a WebSocket to `ws://…/devtools/browser/…`. Used for
//!   `connect` to an already-running browser.
//! - [`MemoryTransport`]: in-process channels, for tests.

use std::future::Future;

use bytes::Bytes;

use crate::error::TransportError;

mod frame;
mod memory;
#[cfg(unix)]
mod pipe;
mod ws;

pub use frame::FrameDecoder;
pub use memory::{MemoryPeer, MemoryTransport};
#[cfg(unix)]
pub use pipe::PipeTransport;
pub use ws::WsTransport;

/// A bidirectional message transport to Chrome.
pub trait Transport: Send + 'static {
    /// The sending half.
    type Sender: TransportSender;
    /// The receiving half.
    type Receiver: TransportReceiver;

    /// Splits the transport into its two halves.
    fn split(self) -> (Self::Sender, Self::Receiver);
}

/// The sending half of a [`Transport`].
pub trait TransportSender: Send + 'static {
    /// Sends one complete JSON message.
    fn send(&mut self, msg: Bytes) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Closes the sending direction. For the pipe this tells Chrome to shut
    /// down; for a WebSocket it sends a close frame.
    fn close(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send;
}

/// The receiving half of a [`Transport`].
pub trait TransportReceiver: Send + 'static {
    /// Receives the next complete JSON message, or `Ok(None)` once the peer
    /// has closed the transport.
    ///
    /// Must be cancel safe: if the future is dropped before it completes, no
    /// message is lost.
    fn recv(&mut self) -> impl Future<Output = Result<Option<Bytes>, TransportError>> + Send;
}
