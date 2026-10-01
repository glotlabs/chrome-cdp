//! The fake payment provider of `examples/e2e-shop` (plan 19, milestone
//! 19.3): a third party on an origin of its own, which the shop's
//! checkout calls from the browser, as a Stripe.js call would. It is
//! part of the shop's real backend (`--backend local`); the example's
//! `third-party/payments` mocks stand in for it in mock and hybrid runs.
//!
//! - `POST /v1/charges` (`{"amount", "currency", "card"}`): every card is
//!   accepted, `{"id": "ch_<n>", "object": "charge", "amount",
//!   "currency", "status": "succeeded"}`; a body without an `amount` is
//!   400;
//! - preflights (`OPTIONS`) are allowed, and answers allow any origin.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    HeaderValue,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};

/// Starts the provider on `http://127.0.0.1:<port>` (0 = any free port),
/// on a thread of its own; returns its URL once it accepts connections.
pub(crate) fn start(port: u16) -> std::io::Result<String> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
    listener.set_nonblocking(true)?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let charges = Arc::new(AtomicU64::new(0));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    std::thread::Builder::new()
        .name("payments".into())
        .spawn(move || {
            runtime.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                    return;
                };
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        continue;
                    };
                    let charges = charges.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |req| {
                            let charges = charges.clone();
                            async move { Ok::<_, Infallible>(answer(req, &charges).await) }
                        });
                        let _ = http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            });
        })?;
    Ok(url)
}

async fn answer(req: Request<Incoming>, charges: &AtomicU64) -> Response<Full<Bytes>> {
    let method = req.method().clone();
    let path = req.uri().path().trim_end_matches('/').to_owned();
    let body = req
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let (status, body) = match (&method, path.as_str()) {
        (&Method::OPTIONS, _) => (StatusCode::NO_CONTENT, None),
        (&Method::POST, "/v1/charges") => charge(&body, charges),
        _ => (
            StatusCode::NOT_FOUND,
            Some(json!({ "error": { "message": format!("no route {method} {path}") } })),
        ),
    };
    let mut response = if let Some(body) = body {
        crate::response(status, "application/json", Bytes::from(body.to_string()))
    } else {
        let mut empty = Response::new(Full::new(Bytes::new()));
        *empty.status_mut() = status;
        empty
    };
    let headers = response.headers_mut();
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, OPTIONS"),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    response
}

fn charge(body: &[u8], charges: &AtomicU64) -> (StatusCode, Option<Value>) {
    let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    if !body["amount"].is_number() {
        return (
            StatusCode::BAD_REQUEST,
            Some(json!({ "error": { "code": "invalid_amount", "message": "amount is required" } })),
        );
    }
    let n = charges.fetch_add(1, Ordering::SeqCst) + 1;
    (
        StatusCode::OK,
        Some(json!({
            "id": format!("ch_{n}"),
            "object": "charge",
            "amount": body["amount"],
            "currency": body["currency"],
            "status": "succeeded",
        })),
    )
}
