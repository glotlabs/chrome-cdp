//! WebSocket mocks and Server-Sent Events (plan 11.9).

use std::time::Duration;

use serde_json::json;

use crate::tests::{dir, mocks, req};
use crate::{
    Outcome, Response, SseEvent, Unmatched, WsFrame, WsMessage, WsOutcome, WsStep, parse_sse,
};

fn load_err(text: &str) -> String {
    crate::tests::load_err(&[("base.toml", text)])
}

const SOCKETS: &str = r#"
[settings]
scope = ["/api/**", "/ws/**"]

[[websocket]]
name = "prices"
url = "/ws/prices/:symbol"
on_open = [
  { send = { type = "hello", symbol = "{{params.symbol}}", n = "{{seq}}" } },
  { delay = "1s", send = "tick" },
]
on_message = [
  { match = { type = "ping" }, send = { type = "pong", id = "{{json.id}}" } },
  { match = "bye*", close = 4000, reason = "done" },
  { match = "echo *", delay = "50ms", send = "{{body}}" },
]

[[websocket]]
url = "wss://push.example.com/**"
protocol = "graphql-ws"
"#;

fn open(m: &mut crate::Mocks, url: &str) -> (crate::WsKey, Option<String>, Vec<WsStep>) {
    match m.ws_open(url, &["v2".into(), "v1".into()], 0) {
        WsOutcome::Open {
            key,
            protocol,
            steps,
            mock,
            outcome,
        } => {
            assert!(outcome.starts_with("websocket open"), "{outcome}");
            assert!(mock.at.starts_with("base.toml:"), "{}", mock.at);
            (key, protocol, steps)
        }
        other => panic!("{url}: {other:?}"),
    }
}

fn text(s: &str) -> WsFrame {
    WsFrame::Text(s.into())
}

#[test]
fn websocket_mocks_open_and_answer() {
    let d = dir(&[("base.toml", SOCKETS)]);
    let mut m = mocks(d.path(), &["base"]);
    assert!(m.has_websockets());
    let (key, protocol, steps) = open(&mut m, "ws://app/ws/prices/ACME?x=1");
    assert_eq!(protocol.as_deref(), Some("v2"));
    assert_eq!(steps.len(), 2);
    let WsFrame::Text(hello) = &steps[0].frame else {
        panic!("{steps:?}")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(hello).unwrap(),
        json!({"type": "hello", "symbol": "ACME", "n": 1})
    );
    assert_eq!(steps[1].delay, Duration::from_secs(1));
    assert_eq!(steps[1].frame, text("tick"));

    let url = "ws://app/ws/prices/ACME";
    let pong = m.ws_message(&key, url, WsMessage::Text(r#"{"type":"ping","id":7}"#), 0);
    assert_eq!(pong.len(), 1);
    let WsFrame::Text(pong) = &pong[0].frame else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(pong).unwrap(),
        json!({"type": "pong", "id": 7})
    );
    let echo = m.ws_message(&key, url, WsMessage::Text("echo hi"), 0);
    assert_eq!(
        echo,
        vec![WsStep {
            delay: Duration::from_millis(50),
            frame: text("echo hi")
        }]
    );
    assert_eq!(
        m.ws_message(&key, url, WsMessage::Text("bye now"), 0)[0].frame,
        WsFrame::Close {
            code: 4000,
            reason: "done".into()
        }
    );
    // No rule fits: no answer.
    assert!(
        m.ws_message(&key, url, WsMessage::Text("what?"), 0)
            .is_empty()
    );
    assert!(
        m.ws_message(&key, url, WsMessage::Text(r#"{"type":"other"}"#), 0)
            .is_empty()
    );

    // A second socket is the mock's second hit; `mock reset` starts over.
    let (_, _, steps) = open(&mut m, url);
    assert!(matches!(&steps[0].frame, WsFrame::Text(t) if t.contains(r#""n":2"#)));
    m.reset();
    let (_, _, steps) = open(&mut m, url);
    assert!(matches!(&steps[0].frame, WsFrame::Text(t) if t.contains(r#""n":1"#)));

    // A full-URL glob, with its own protocol and nothing to say.
    let (_, protocol, steps) = open(&mut m, "wss://push.example.com/graphql");
    assert_eq!(protocol.as_deref(), Some("graphql-ws"));
    assert!(steps.is_empty());

    // `mock list` shows them.
    let info = m.info(true);
    let listed: Vec<&str> = info[0].mocks.iter().map(|i| i.matches.as_str()).collect();
    assert!(listed.contains(&"WS /ws/prices/:symbol"), "{listed:?}");
    assert!(
        listed.contains(&"WS wss://push.example.com/**"),
        "{listed:?}"
    );
}

#[test]
fn websocket_mocks_send_and_take_binary_messages() {
    let d = dir(&[(
        "base.toml",
        r#"
[[websocket]]
url = "/ws"
on_open = [{ send_base64 = "AQID" }, { send = "text" }]
on_message = [
  { match = "ping", send = "pong" },
  { send_base64 = "{{body}}" },
]

[[websocket]]
url = "/text"
on_message = [{ match = "*", send = "text only" }]
"#,
    )]);
    let mut m = mocks(d.path(), &["base"]);
    let (key, _, steps) = open(&mut m, "ws://app/ws");
    assert_eq!(steps[0].frame, WsFrame::Binary("AQID".into()));
    assert_eq!(steps[1].frame, text("text"));
    let url = "ws://app/ws";
    // A binary message fits only a rule without `match`; its `body` is
    // its base64 (normalised on the way out).
    let echo = m.ws_message(&key, url, WsMessage::Binary("BAUG"), 0);
    assert_eq!(echo[0].frame, WsFrame::Binary("BAUG".into()));
    let echo = m.ws_message(&key, url, WsMessage::Binary("_-8"), 0);
    assert_eq!(echo[0].frame, WsFrame::Binary("/+8=".into()));
    assert_eq!(
        m.ws_message(&key, url, WsMessage::Binary("cGluZw=="), 0)[0].frame,
        WsFrame::Binary("cGluZw==".into()),
        "binary \"ping\" is not the text rule's"
    );
    assert_eq!(
        m.ws_message(&key, url, WsMessage::Text("ping"), 0)[0].frame,
        text("pong")
    );
    // A glob on the text takes no binary message.
    let (key, _, _) = open(&mut m, "ws://app/text");
    assert!(
        m.ws_message(&key, "ws://app/text", WsMessage::Binary("AQID"), 0)
            .is_empty()
    );
    // What a template makes must be base64.
    let (key, _, _) = open(&mut m, url);
    let steps = m.ws_message(&key, url, WsMessage::Text("no base64!"), 0);
    let WsFrame::Close { code, reason } = &steps[0].frame else {
        panic!("{steps:?}")
    };
    assert_eq!(*code, 1011);
    assert!(reason.contains("is not base64"), "{reason}");
}

#[test]
fn unmatched_websockets_follow_the_sets_policy() {
    let d = dir(&[
        ("base.toml", SOCKETS),
        (
            "open.toml",
            "[settings]\nscope = [\"/live/**\"]\nunmatched = \"passthrough\"\n",
        ),
    ]);
    let mut m = mocks(d.path(), &["base", "open"]);
    match m.ws_open("ws://app/ws/other", &[], 0) {
        WsOutcome::Unmatched {
            set,
            policy,
            outcome,
        } => {
            assert_eq!((set.as_str(), policy), ("base", Unmatched::Fail));
            assert_eq!(outcome, "unmatched, socket fails");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        m.ws_open("ws://app/live/feed", &[], 0),
        WsOutcome::Unmatched {
            policy: Unmatched::Passthrough,
            ..
        }
    ));
    assert!(matches!(
        m.ws_open("ws://app/elsewhere", &[], 0),
        WsOutcome::Pass
    ));
}

#[test]
fn websocket_template_errors_close_the_socket() {
    let d = dir(&[(
        "base.toml",
        "[[websocket]]\nurl = \"/ws\"\non_open = [{ send = \"{{json.missing}}\" }, { send = \"never\" }]\n",
    )]);
    let mut m = mocks(d.path(), &["base"]);
    let (_, _, steps) = open(&mut m, "ws://app/ws");
    assert_eq!(steps.len(), 1);
    let WsFrame::Close { code, reason } = &steps[0].frame else {
        panic!("{steps:?}")
    };
    assert_eq!(*code, 1011);
    assert!(
        reason.starts_with("chrome-cdp mock error in base.toml:1"),
        "{reason}"
    );
    assert!(reason.len() <= 123);
}

#[test]
fn websocket_mocks_are_checked() {
    for (text, want) in [
        (
            "[[websocket]]\nurl = \"ws\"\n",
            "websocket.url: `ws` is neither",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\non_open = [{ delay = \"1s\" }]\n",
            "websocket.on_open[0]: give `send` or `close`",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\non_open = [{ send = 1, close = 1000 }]\n",
            "not both",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\non_message = [{ close = 1006 }]\n",
            "websocket.on_message[0].close: a page cannot be sent code 1006",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\non_message = [{ match = 3, send = \"x\" }]\n",
            "websocket.on_message[0].match: `3` is neither",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\non_open = [{ send = \"{{ oops\" }]\n",
            "websocket.on_open[0].send",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\non_open = [{ send = 1, reason = \"x\" }]\n",
            "`reason` needs `close`",
        ),
        (
            "[[websocket]]\nname = \"a\"\nurl = \"/ws\"\n[[websocket]]\nname = \"a\"\nurl = \"/ws2\"\n",
            "websocket: the name `a` is used twice",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\nsend = 1\n",
            "unknown field `send`",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\non_open = [{ send_base64 = \"a*b\" }]\n",
            "websocket.on_open[0].send_base64: `a*b` is not base64",
        ),
        (
            "[[websocket]]\nurl = \"/ws\"\non_message = [{ send = \"x\", send_base64 = \"AQID\" }]\n",
            "give `send` or `send_base64`, not both",
        ),
    ] {
        let err = load_err(text);
        assert!(err.contains(want), "{text}\n→ {err}");
    }
}

const EVENTS: &str = r#"
[[mock]]
name = "prices"
match = { path = "/api/prices" }
respond = { sse = [
  { data = { price = 41 } },
  { delay = "1s", event = "price", id = "{{seq}}", data = { price = 42, q = "{{query.q}}" } },
  { data = "two\nlines", retry = 5000 },
  { comment = "keep-alive" },
] }
"#;

#[test]
fn sse_answers_carry_their_events() {
    let d = dir(&[("base.toml", EVENTS)]);
    let mut m = mocks(d.path(), &["base"]);
    assert!(m.has_sse());
    let Outcome::Answer(answer) = m.decide(&req("GET", "http://app/api/prices?q=x", None), 0)
    else {
        panic!()
    };
    let events = answer.events.expect("sse events");
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].data.as_deref(), Some(r#"{"price":41}"#));
    assert_eq!(events[1].delay, Duration::from_secs(1));
    assert_eq!(events[1].event.as_deref(), Some("price"));
    assert_eq!(events[1].id.as_deref(), Some("1"));
    assert_eq!(events[1].data.as_deref(), Some(r#"{"price":42,"q":"x"}"#));
    assert!(!events[3].dispatched());
    let Response::Fulfill { headers, body, .. } = answer.response else {
        panic!()
    };
    let body = String::from_utf8(body).unwrap();
    assert_eq!(
        body,
        "data: {\"price\":41}\n\nevent: price\nid: 1\ndata: {\"price\":42,\"q\":\"x\"}\n\nretry: 5000\ndata: two\ndata: lines\n\n: keep-alive\n\n"
    );
    assert!(headers.contains(&("Content-Type".into(), "text/event-stream".into())));
    assert!(headers.contains(&("Cache-Control".into(), "no-cache".into())));

    // A browser reads the same events back from the body.
    let parsed = parse_sse(&body);
    let expected: Vec<SseEvent> = events
        .into_iter()
        .map(|e| SseEvent {
            delay: Duration::ZERO,
            ..e
        })
        .collect();
    assert_eq!(parsed, expected);
    assert!(
        load_err("[[mock]]\nmatch = { path = \"/x\" }\nrespond = { sse = [] }\n")
            .contains("respond.sse: it has no events")
    );
    assert!(
        load_err("[[mock]]\nmatch = { path = \"/x\" }\nrespond = { sse = [{ event = \"x\" }] }\n")
            .contains("respond.sse[0]: give `data`")
    );
    assert!(
        load_err(
            "[[mock]]\nmatch = { path = \"/x\" }\nrespond = { json = 1, sse = [{ data = 1 }] }\n"
        )
        .contains("give one body, not json and sse")
    );
}
