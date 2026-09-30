//! Fake dev servers' hot-reload sockets (plan 15, milestone 15.6), so
//! tests of `wait --hmr` need no Node.
//!
//! - `/hmr/vite?channel=<name>` is a WebSocket that speaks Vite's
//!   `vite-hmr` subprotocol: it greets with `{"type":"connected"}`, as
//!   Vite does.
//! - `/hmr/webpack?channel=<name>` is one that greets as
//!   webpack-dev-server does: `hot`, `liveReload`, `hash` and `ok`.
//! - `POST /hmr/send?channel=<name>` sends its body, as one text message,
//!   to every socket of the channel and answers how many there were
//!   (`[`Fixtures::hmr_send`](crate::Fixtures::hmr_send)).
//!
//! `testdata/site/hmr/vite.html` and `hmr/webpack.html` are the clients.
//! Each test uses its own channel, since tests share one server.

use std::collections::HashMap;
use std::sync::Mutex;

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::{Site, param, response, upgrade};

/// The open sockets of each channel.
pub(crate) type Channels = Mutex<HashMap<String, Vec<mpsc::UnboundedSender<String>>>>;

/// Which dev server a socket stands for.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    Vite,
    Webpack,
}

impl Kind {
    /// What the server sends when a client connects.
    fn greeting(self) -> &'static [&'static str] {
        match self {
            Self::Vite => &[r#"{"type":"connected"}"#],
            Self::Webpack => &[
                r#"{"type":"hot"}"#,
                r#"{"type":"liveReload"}"#,
                r#"{"type":"hash","data":"0000000000000000"}"#,
                r#"{"type":"ok"}"#,
            ],
        }
    }
}

/// `/hmr/vite` and `/hmr/webpack`: accepts the upgrade, greets, then
/// sends what `/hmr/send` posts to the channel until the page closes it.
pub(crate) fn socket(
    site: &Site,
    req: &mut Request<Incoming>,
    kind: Kind,
) -> Response<Full<Bytes>> {
    let channel = param(req.uri().query().unwrap_or_default(), "channel")
        .unwrap_or_default()
        .to_owned();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let protocol = match kind {
        Kind::Vite => Some("vite-hmr"),
        Kind::Webpack => None,
    };
    let response = upgrade(req, protocol, move |mut ws| async move {
        for message in kind.greeting() {
            if ws.send(Message::text(*message)).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                message = rx.recv() => {
                    let Some(message) = message else { return };
                    if ws.send(Message::text(message)).await.is_err() {
                        return;
                    }
                }
                incoming = ws.next() => {
                    if !matches!(incoming, Some(Ok(_))) {
                        return;
                    }
                }
            }
        }
    });
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let mut channels = site
            .hmr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        channels.entry(channel).or_default().push(tx);
    }
    response
}

/// `POST /hmr/send?channel=<name>`: sends the body to the channel's open
/// sockets; answers their number.
pub(crate) async fn send(site: &Site, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let channel = param(req.uri().query().unwrap_or_default(), "channel")
        .unwrap_or_default()
        .to_owned();
    let body = req
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let message = String::from_utf8_lossy(&body).into_owned();
    let sent = {
        let mut channels = site
            .hmr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sockets = channels.entry(channel).or_default();
        // A socket whose page went away has dropped its receiver.
        sockets.retain(|tx| tx.send(message.clone()).is_ok());
        sockets.len()
    };
    response(
        StatusCode::OK,
        "text/plain; charset=utf-8",
        Bytes::from(format!("{sent}\n")),
    )
}
