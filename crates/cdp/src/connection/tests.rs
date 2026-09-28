//! Connection behaviour against a scripted in-memory Chrome.

use std::time::Duration;

use serde_json::{Value, json};

use crate::domains::{browser, page, target};
use crate::transport::{MemoryPeer, MemoryTransport};
use crate::{CdpError, Connection, ConnectionOptions};

fn connect() -> (Connection, MemoryPeer) {
    let (transport, peer) = MemoryTransport::pair();
    (Connection::new(transport), peer)
}

/// Receives the next request and returns its id.
async fn next_request(peer: &mut MemoryPeer) -> (u64, Value) {
    let req = peer.recv_json().await.expect("a request");
    (req["id"].as_u64().expect("numeric id"), req)
}

#[tokio::test]
async fn typed_call_round_trip() {
    let (conn, mut peer) = connect();
    let call = tokio::spawn({
        let conn = conn.clone();
        async move {
            conn.browser()
                .call(browser::GetVersionParams::default())
                .await
        }
    });
    let (id, req) = next_request(&mut peer).await;
    assert_eq!(
        req,
        json!({"id": id, "method": "Browser.getVersion", "params": {}})
    );
    peer.send_json(&json!({"id": id, "result": {
        "protocolVersion": "1.3", "product": "Chrome/154", "revision": "r",
        "userAgent": "UA", "jsVersion": "15"
    }}));
    let version = call.await.unwrap().unwrap();
    assert_eq!(version.product, "Chrome/154");
    assert_eq!(conn.pending_calls(), 0);
}

#[tokio::test]
async fn responses_are_correlated_by_id() {
    let (conn, mut peer) = connect();
    let a = tokio::spawn({
        let s = conn.browser();
        async move { s.call_raw("A.a", json!({"n": 1})).await }
    });
    let (id_a, _) = next_request(&mut peer).await;
    let b = tokio::spawn({
        let s = conn.browser();
        async move { s.call_raw("B.b", json!({"n": 2})).await }
    });
    let (id_b, req_b) = next_request(&mut peer).await;
    assert_ne!(id_a, id_b);
    assert_eq!(req_b["params"], json!({"n": 2}));
    // Answer out of order.
    peer.send_json(&json!({"id": id_b, "result": {"who": "b"}}));
    peer.send_json(&json!({"id": id_a, "result": {"who": "a"}}));
    assert_eq!(a.await.unwrap().unwrap(), json!({"who": "a"}));
    assert_eq!(b.await.unwrap().unwrap(), json!({"who": "b"}));
}

#[tokio::test]
async fn session_calls_carry_session_id() {
    let (conn, mut peer) = connect();
    let s = conn.session("S1");
    let call = tokio::spawn(async move { s.call(page::EnableParams::default()).await });
    let (id, req) = next_request(&mut peer).await;
    assert_eq!(req["sessionId"], "S1");
    assert_eq!(req["method"], "Page.enable");
    // A result-less response still decodes into the empty returns struct.
    peer.send_json(&json!({"id": id, "sessionId": "S1"}));
    call.await.unwrap().unwrap();
}

#[tokio::test]
async fn protocol_errors_are_mapped() {
    let (conn, mut peer) = connect();
    let call = tokio::spawn({
        let s = conn.browser();
        async move { s.call_raw("Nope.nope", json!({})).await }
    });
    let (id, _) = next_request(&mut peer).await;
    peer.send_json(&json!({"id": id, "error": {
        "code": -32601, "message": "'Nope.nope' wasn't found", "data": "extra"
    }}));
    let err = call.await.unwrap().unwrap_err();
    let CdpError::Protocol {
        method,
        code,
        message,
        data,
    } = &err
    else {
        panic!("expected a protocol error, got {err:?}");
    };
    assert_eq!(method, "Nope.nope");
    assert_eq!(*code, -32601);
    assert_eq!(message, "'Nope.nope' wasn't found");
    assert_eq!(data.as_deref(), Some("extra"));
    assert_eq!(
        err.to_string(),
        "Nope.nope: 'Nope.nope' wasn't found (code -32601)"
    );
}

#[tokio::test]
async fn undecodable_results_are_decode_errors() {
    let (conn, mut peer) = connect();
    let call = tokio::spawn({
        let s = conn.browser();
        async move { s.call(browser::GetVersionParams::default()).await }
    });
    let (id, _) = next_request(&mut peer).await;
    peer.send_json(&json!({"id": id, "result": {"product": 5}}));
    let err = call.await.unwrap().unwrap_err();
    assert!(
        matches!(&err, CdpError::Decode { method, .. } if method == "Browser.getVersion"),
        "{err:?}"
    );
}

#[tokio::test]
async fn events_are_routed_by_session() {
    let (conn, peer) = connect();
    let mut browser_events = conn.browser().events();
    let mut s1_loads = conn.session("S1").listen::<page::LoadEventFired>();
    let mut s2_events = conn.session("S2").events();

    peer.send_json(
        &json!({"method": "Page.loadEventFired", "params": {"timestamp": 1.0}, "sessionId": "S2"}),
    );
    peer.send_json(&json!({"method": "Page.domContentEventFired", "params": {"timestamp": 2.0}, "sessionId": "S1"}));
    peer.send_json(
        &json!({"method": "Page.loadEventFired", "params": {"timestamp": 3.0}, "sessionId": "S1"}),
    );
    peer.send_json(&json!({"method": "Target.targetCreated", "params": {"targetInfo": {}}}));

    // S1's typed stream skips the other event name.
    let load = s1_loads.next().await.unwrap();
    assert!((load.timestamp - 3.0).abs() < f64::EPSILON);

    let ev = s2_events.next().await.unwrap();
    assert_eq!(ev.method, "Page.loadEventFired");
    assert_eq!(ev.session_id.as_deref(), Some("S2"));

    let ev = browser_events.next().await.unwrap();
    assert_eq!(ev.method, "Target.targetCreated");
    assert_eq!(ev.session_id, None);
}

#[tokio::test]
async fn events_without_params_get_an_empty_object() {
    let (conn, peer) = connect();
    let mut resized = conn.session("S").listen::<page::FrameResized>();
    peer.send_json(&json!({"method": "Page.frameResized", "sessionId": "S"}));
    resized.next().await.unwrap();
}

#[tokio::test]
async fn detaching_ends_the_session_streams() {
    let (conn, peer) = connect();
    let mut events = conn.session("S1").events();
    let mut detached = conn.browser().listen::<target::DetachedFromTarget>();
    peer.send_json(&json!({"method": "Target.detachedFromTarget", "params": {"sessionId": "S1", "targetId": "T1"}}));
    assert_eq!(detached.next().await.unwrap().session_id, "S1");
    assert!(events.next().await.is_none());
    // Listening after the detach gets a stream that has already ended.
    let mut late = conn.session("S1").events();
    assert!(late.next().await.is_none());
    // Other sessions are not affected.
    let mut other = conn.session("S2").events();
    peer.send_json(&json!({"method": "Page.loadEventFired", "params": {}, "sessionId": "S2"}));
    assert_eq!(other.next().await.unwrap().method, "Page.loadEventFired");
}

#[tokio::test]
async fn lossless_listeners_miss_nothing() {
    let (transport, peer) = MemoryTransport::pair();
    let conn = Connection::with_options(
        transport,
        ConnectionOptions {
            event_buffer: 4,
            ..ConnectionOptions::default()
        },
    );
    let session = conn.session("S1");
    let mut fetch = session.events_lossless("Fetch.");
    let mut all = session.events();
    for n in 0..20 {
        peer.send_json(
            &json!({"method": "Fetch.requestPaused", "params": {"n": n}, "sessionId": "S1"}),
        );
        peer.send_json(&json!({"method": "Page.frameResized", "params": {}, "sessionId": "S1"}));
    }
    peer.send_json(&json!({"method": "Target.detachedFromTarget", "params": {"sessionId": "S1", "targetId": "T1"}}));
    // Every Fetch event, in order, and nothing else.
    for n in 0..20 {
        let event = fetch.next().await.unwrap();
        assert_eq!(event.method, "Fetch.requestPaused");
        assert_eq!(event.params["n"], n);
    }
    // The detach ends it, as it does the other streams.
    assert!(fetch.next().await.is_none());
    // The broadcast stream fell behind and skipped some.
    let mut seen = 0;
    while all.next().await.is_some() {
        seen += 1;
    }
    assert!(seen < 40, "{seen}");
    // Listening after the detach gets a stream that has already ended.
    assert!(session.events_lossless("Fetch.").next().await.is_none());
}

#[tokio::test]
async fn calls_are_sent_when_made_not_when_polled() {
    let (conn, mut peer) = connect();
    let session = conn.session("S1");
    // Made in this order, polled in the reverse one.
    let first = session.call_raw("A.first", json!({}));
    let second = session.call_raw("A.second", json!({}));
    let (id_a, a) = next_request(&mut peer).await;
    let (id_b, b) = next_request(&mut peer).await;
    assert_eq!(
        (a["method"].as_str(), b["method"].as_str()),
        (Some("A.first"), Some("A.second"))
    );
    peer.send_json(&json!({"id": id_b, "result": {"n": 2}}));
    peer.send_json(&json!({"id": id_a, "result": {"n": 1}}));
    assert_eq!(second.await.unwrap(), json!({"n": 2}));
    assert_eq!(first.await.unwrap(), json!({"n": 1}));
    // A call dropped unpolled leaves nothing pending.
    drop(session.call_raw("A.dropped", json!({})));
    assert_eq!(conn.pending_calls(), 0);
}

#[tokio::test]
async fn attach_uses_flat_mode() {
    let (conn, mut peer) = connect();
    let attach = tokio::spawn({
        let conn = conn.clone();
        async move { conn.attach("T1").await }
    });
    let (id, req) = next_request(&mut peer).await;
    assert_eq!(req["method"], "Target.attachToTarget");
    assert_eq!(req["params"], json!({"targetId": "T1", "flatten": true}));
    peer.send_json(&json!({"id": id, "result": {"sessionId": "S9"}}));
    let session = attach.await.unwrap().unwrap();
    assert_eq!(session.id(), Some("S9"));
}

#[tokio::test(start_paused = true)]
async fn calls_time_out_and_clean_up() {
    let (transport, mut peer) = MemoryTransport::pair();
    let conn = Connection::with_options(
        transport,
        ConnectionOptions {
            call_timeout: Duration::from_secs(5),
            ..ConnectionOptions::default()
        },
    );
    let err = conn
        .browser()
        .call_raw("Slow.call", json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, CdpError::Timeout { method, after } if method == "Slow.call" && *after == Duration::from_secs(5)),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "Slow.call: no response from Chrome after 5000ms"
    );
    assert_eq!(conn.pending_calls(), 0);

    // Per-handle override.
    let err = conn
        .browser()
        .with_timeout(Duration::from_millis(10))
        .call_raw("Slow.call", json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, CdpError::Timeout { after, .. } if after == Duration::from_millis(10)));

    // A late response for a timed-out call is ignored.
    let (id, _) = next_request(&mut peer).await;
    peer.send_json(&json!({"id": id, "result": {}}));
    let call = tokio::spawn({
        let s = conn.browser();
        async move { s.call_raw("Next.call", json!({})).await }
    });
    let _ = next_request(&mut peer).await;
    let (id, _) = next_request(&mut peer).await;
    peer.send_json(&json!({"id": id, "result": {"ok": true}}));
    assert_eq!(call.await.unwrap().unwrap(), json!({"ok": true}));
}

#[tokio::test]
async fn dropping_a_call_removes_it() {
    let (conn, mut peer) = connect();
    let call = tokio::spawn({
        let s = conn.browser();
        async move { s.call_raw("Never.answered", json!({})).await }
    });
    let (id, _) = next_request(&mut peer).await;
    assert_eq!(conn.pending_calls(), 1);
    call.abort();
    let _ = call.await;
    assert_eq!(conn.pending_calls(), 0);
    // The late answer is harmless.
    peer.send_json(&json!({"id": id, "result": {}}));
    tokio::task::yield_now().await;
    assert!(!conn.is_closed());
}

#[tokio::test]
async fn disconnect_fails_pending_calls_and_ends_streams() {
    let (conn, mut peer) = connect();
    let mut events = conn.session("S1").events();
    let calls: Vec<_> = (0..3)
        .map(|i| {
            let s = conn.browser();
            tokio::spawn(async move { s.call_raw(&format!("Pending.call{i}"), json!({})).await })
        })
        .collect();
    for _ in 0..3 {
        next_request(&mut peer).await;
    }
    let closed_at = std::time::Instant::now();
    peer.close();
    for call in calls {
        let err = call.await.unwrap().unwrap_err();
        assert!(matches!(err, CdpError::Disconnected { .. }), "{err:?}");
    }
    assert!(closed_at.elapsed() < Duration::from_millis(100));
    assert!(events.next().await.is_none());
    assert_eq!(&*conn.closed().await, "Chrome closed the connection");
    assert!(conn.is_closed());
    assert_eq!(conn.pending_calls(), 0);

    // Later calls fail at once, and later listeners end at once.
    let err = conn
        .browser()
        .call_raw("After.close", json!({}))
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "disconnected from Chrome: Chrome closed the connection"
    );
    assert!(conn.browser().events().next().await.is_none());
}

#[tokio::test]
async fn malformed_messages_are_ignored() {
    let (conn, mut peer) = connect();
    let call = tokio::spawn({
        let s = conn.browser();
        async move { s.call_raw("X.y", json!({})).await }
    });
    let (id, _) = next_request(&mut peer).await;
    peer.send("not json");
    peer.send_json(&json!({"neither": "id nor method"}));
    peer.send_json(&json!({"id": id, "result": {"fine": 1}}));
    assert_eq!(call.await.unwrap().unwrap(), json!({"fine": 1}));
}

#[tokio::test]
async fn dropping_the_connection_closes_the_transport() {
    let (conn, mut peer) = connect();
    let session = conn.browser();
    drop(conn);
    // A session keeps the connection alive.
    let call = tokio::spawn(async move {
        let _ = session.call_raw("Keep.alive", json!({})).await;
    });
    let (id, _) = next_request(&mut peer).await;
    peer.send_json(&json!({"id": id, "result": {}}));
    call.await.unwrap();
    // The last handle is gone: the writer closes its half.
    assert!(peer.incoming.recv().await.is_none());
}
