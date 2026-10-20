//! `load --metrics-listen ADDR` (plan 23.4): serves the run's live numbers
//! in Prometheus' text format, for teams that already graph their servers
//! there. It listens on the address given only, and only while the run
//! lasts; any path answers the numbers (`/metrics` is the usual one).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use protocol::{ErrorInfo, ErrorKind, Hint};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use super::user::Shared;

/// How long a scrape may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest request read.
const MOST: usize = 8 * 1024;

/// Listens on `addr`, before the run starts.
pub(super) async fn bind(addr: SocketAddr) -> Result<TcpListener, ErrorInfo> {
    TcpListener::bind(addr).await.map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--metrics-listen: cannot listen on {addr}: {e}"),
        )
        .hint(Hint::text(
            "pick a free port, e.g. --metrics-listen 127.0.0.1:9464",
        ))
    })
}

/// Answers scrapes until aborted.
pub(super) async fn serve(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(READ_TIMEOUT, answer(stream, &shared)).await;
        });
    }
}

async fn answer(mut stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < MOST {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        request.extend_from_slice(&buf[..n]);
    }
    let get = request.starts_with(b"GET ") || request.starts_with(b"HEAD ");
    let (status, body) = if get {
        ("200 OK", shared.records.metrics.prometheus(shared.gauges()))
    } else {
        ("405 Method Not Allowed", "GET the numbers\n".to_owned())
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    if !request.starts_with(b"HEAD ") {
        stream.write_all(body.as_bytes()).await?;
    }
    stream.shutdown().await
}
