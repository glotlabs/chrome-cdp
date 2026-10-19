//! The load server's faults, measured from outside: the latency an
//! endpoint is given is the latency its clients see, and an error rate is
//! the share of injected 500s.

#![allow(clippy::unwrap_used)]

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use load_server::{Fault, LoadServer, Options};

/// Sends one request and returns its status and body.
fn request(url: &str, method: &str, path: &str, body: &str) -> (u16, String) {
    let answer = exchange(url, method, path, body);
    let status = answer[9..12].parse().unwrap();
    let body = answer
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    (status, body)
}

/// Sends one request and returns the whole answer, headers and all.
fn exchange(url: &str, method: &str, path: &str, body: &str) -> String {
    let addr = url
        .trim_start_matches("http://")
        .replace("localhost", "127.0.0.1");
    let mut stream = TcpStream::connect(addr).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

#[test]
fn latency_is_what_the_endpoint_is_given() {
    let server = LoadServer::start(Options {
        faults: vec![Fault::latency("POST /api/cart=200ms±20").unwrap()],
        ..Options::default()
    })
    .unwrap();
    let mut times: Vec<Duration> = (0..10)
        .map(|_| {
            let started = Instant::now();
            let (status, _) = request(server.url(), "POST", "/api/cart", r#"{"product_id":1}"#);
            assert_eq!(status, 201);
            started.elapsed()
        })
        .collect();
    times.sort();
    let median = times[times.len() / 2];
    assert!(
        median >= Duration::from_millis(180) && median <= Duration::from_millis(250),
        "median {median:?}"
    );
    // Other endpoints are not slowed.
    let started = Instant::now();
    assert_eq!(request(server.url(), "GET", "/api/cart", "").0, 200);
    assert!(started.elapsed() < Duration::from_millis(150));
    let stats = server.stats();
    assert_eq!(stats["POST /api/cart"].requests, 10);
    assert_eq!(stats["GET /api/cart"].requests, 1);
}

#[test]
fn errors_are_the_share_asked_for() {
    let server = LoadServer::start(Options {
        faults: vec![Fault::errors("/api/products/*=2%").unwrap()],
        seed: 7,
        ..Options::default()
    })
    .unwrap();
    let url = server.url().to_owned();
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let url = url.clone();
            std::thread::spawn(move || {
                (0..1250)
                    .filter(|_| request(&url, "GET", "/api/products/1", "").0 == 500)
                    .count()
            })
        })
        .collect();
    let errors: usize = workers.into_iter().map(|w| w.join().unwrap()).sum();
    assert!((150..=250).contains(&errors), "{errors} errors in 10,000");
    let seen = server.stats()["GET /api/products/1"];
    assert_eq!(seen.requests, 10_000);
    assert_eq!(seen.errors, errors as u64);
    let (status, body) = request(&url, "GET", "/__load/stats", "");
    assert_eq!(status, 200);
    assert!(body.contains("\"GET /api/products/1\""), "{body}");
}

#[test]
fn serves_the_shop() {
    let server = LoadServer::start(Options::default()).unwrap();
    let (status, body) = request(server.url(), "GET", "/", "");
    assert_eq!(status, 200);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "{body}"
    );
    let (status, body) = request(server.url(), "GET", "/api/products", "");
    assert_eq!(status, 200);
    assert!(body.contains("Laptop stand"), "{body}");
    assert_eq!(request(server.url(), "POST", "/__load/reset", "").0, 200);
    assert!(server.stats().is_empty());
}

#[test]
fn hashed_assets_may_be_cached() {
    let server = LoadServer::start(Options::default()).unwrap();
    let page = exchange(server.url(), "GET", "/", "").to_ascii_lowercase();
    assert!(page.contains("cache-control: no-store"), "{page}");
    let script = page
        .split('"')
        .find(|part| {
            part.starts_with("/assets/")
                && std::path::Path::new(part)
                    .extension()
                    .is_some_and(|e| e == "js")
        })
        .unwrap_or_else(|| panic!("no script in {page}"))
        .to_owned();
    let asset = exchange(server.url(), "GET", &script, "").to_ascii_lowercase();
    assert!(asset.starts_with("http/1.1 200"), "{asset}");
    assert!(
        asset.contains("cache-control: public, max-age=31536000, immutable"),
        "{asset}"
    );
}
