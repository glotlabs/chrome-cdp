//! WebSocket transport, for connecting to an already-running browser.

use bytes::Bytes;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::{Transport, TransportReceiver, TransportSender};
use crate::error::TransportError;

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A CDP transport over a WebSocket, e.g.
/// `ws://127.0.0.1:9222/devtools/browser/<id>`.
#[derive(Debug)]
pub struct WsTransport {
    stream: Stream,
}

impl WsTransport {
    /// Connects to a DevTools WebSocket URL.
    ///
    /// Message and frame size limits are off: CDP screenshots and traces can
    /// be larger than 100 MB.
    pub async fn connect(url: &str) -> Result<Self, TransportError> {
        let config = WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        let (stream, _response) =
            tokio_tungstenite::connect_async_with_config(url, Some(config), true).await?;
        Ok(Self { stream })
    }
}

/// Sending half of a [`WsTransport`].
#[derive(Debug)]
pub struct WsSender(SplitSink<Stream, Message>);

/// Receiving half of a [`WsTransport`].
#[derive(Debug)]
pub struct WsReceiver(SplitStream<Stream>);

impl Transport for WsTransport {
    type Sender = WsSender;
    type Receiver = WsReceiver;

    fn split(self) -> (WsSender, WsReceiver) {
        let (sink, stream) = self.stream.split();
        (WsSender(sink), WsReceiver(stream))
    }
}

impl TransportSender for WsSender {
    async fn send(&mut self, msg: Bytes) -> Result<(), TransportError> {
        // CDP messages are JSON, so always valid UTF-8; fall back to a binary
        // frame rather than failing if a caller sends something else.
        let msg = match Utf8Bytes::try_from(msg.clone()) {
            Ok(text) => Message::Text(text),
            Err(_) => Message::Binary(msg),
        };
        self.0.send(msg).await?;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.0.close().await?;
        Ok(())
    }
}

impl TransportReceiver for WsReceiver {
    async fn recv(&mut self) -> Result<Option<Bytes>, TransportError> {
        // `StreamExt::next` is cancel safe.
        while let Some(msg) = self.0.next().await {
            match msg? {
                Message::Text(text) => return Ok(Some(text.into())),
                Message::Binary(data) => return Ok(Some(data)),
                Message::Close(_) => return Ok(None),
                // Pongs are answered by tungstenite; nothing to deliver.
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

    use super::WsTransport;
    use crate::transport::{Transport, TransportReceiver, TransportSender};

    /// Serves one WebSocket that echoes text frames, pings once, and answers the
    /// client's close.
    async fn echo_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            // The server must not cap sizes either: the test sends > 64 MB,
            // tungstenite's default limit.
            let config = WebSocketConfig::default()
                .max_message_size(None)
                .max_frame_size(None);
            let mut ws = tokio_tungstenite::accept_async_with_config(tcp, Some(config))
                .await
                .unwrap();
            ws.send(Message::Ping(Bytes::from_static(b"hi")))
                .await
                .unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                match msg {
                    Message::Text(_) | Message::Binary(_) => ws.send(msg).await.unwrap(),
                    // Keep polling: the next poll flushes tungstenite's close
                    // reply and then ends the stream.
                    _ => {}
                }
            }
        });
        format!("ws://{addr}/devtools/browser/test")
    }

    #[tokio::test]
    async fn round_trip_and_close() {
        let url = echo_server().await;
        let (mut tx, mut rx) = WsTransport::connect(&url).await.unwrap().split();
        tx.send(Bytes::from_static(b"{\"id\":1}")).await.unwrap();
        // The ping is skipped; the echo comes through.
        assert_eq!(rx.recv().await.unwrap().unwrap(), &b"{\"id\":1}"[..]);
        let big = Bytes::from(vec![b'y'; 70 * 1024 * 1024]);
        tx.send(big).await.unwrap();
        assert_eq!(rx.recv().await.unwrap().unwrap().len(), 70 * 1024 * 1024);
        tx.close().await.unwrap();
        assert!(rx.recv().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn connect_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        assert!(
            WsTransport::connect(&format!("ws://{addr}/"))
                .await
                .is_err()
        );
    }
}
