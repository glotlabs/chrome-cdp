//! Serde behaviour of the generated domain types.

use serde_json::json;

use crate::domains::{browser, network, page, runtime, security, target};
use crate::{Command, Event};

#[test]
fn commands_and_events_carry_wire_names() {
    assert_eq!(page::NavigateParams::METHOD, "Page.navigate");
    assert_eq!(browser::GetVersionParams::METHOD, "Browser.getVersion");
    assert_eq!(page::LoadEventFired::NAME, "Page.loadEventFired");
    assert_eq!(target::AttachedToTarget::NAME, "Target.attachedToTarget");
}

#[test]
fn optional_params_are_omitted_and_names_are_camel_case() {
    let mut params = page::NavigateParams::new("https://example.com/".into());
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({"url": "https://example.com/"})
    );
    params.transition_type = Some(page::TransitionType::Typed);
    params.frame_id = Some("F1".into());
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({"url": "https://example.com/", "transitionType": "typed", "frameId": "F1"})
    );
}

#[test]
fn all_optional_params_default_to_empty_object() {
    let params = page::EnableParams::default();
    assert_eq!(serde_json::to_value(params).unwrap(), json!({}));
    let params = target::CreateTargetParams::new("about:blank".into());
    assert_eq!(
        serde_json::to_value(params).unwrap(),
        json!({"url": "about:blank"})
    );
}

#[test]
fn returns_deserialise_from_chrome_json() {
    let v: <browser::GetVersionParams as Command>::Returns = serde_json::from_value(json!({
        "protocolVersion": "1.3",
        "product": "HeadlessChrome/154.0.8037.57",
        "revision": "@73c14f6228d7cd537c855007e8f88678969cc0eb",
        "userAgent": "Mozilla/5.0",
        "jsVersion": "15.4.80.11",
        "someFutureField": true
    }))
    .unwrap();
    assert_eq!(v.product, "HeadlessChrome/154.0.8037.57");
    assert_eq!(v.js_version, "15.4.80.11");
}

#[test]
fn unknown_enum_values_do_not_break_deserialisation() {
    let t: page::TransitionType = serde_json::from_str("\"teleport\"").unwrap();
    assert_eq!(t, page::TransitionType::Unknown);
    let t: page::TransitionType = serde_json::from_str("\"auto_bookmark\"").unwrap();
    assert_eq!(t, page::TransitionType::AutoBookmark);
}

#[test]
fn protocol_unknown_value_doubles_as_catch_all() {
    // `Security.SecurityState` has its own "unknown" value.
    let s: security::SecurityState = serde_json::from_str("\"unknown\"").unwrap();
    assert_eq!(s, security::SecurityState::Unknown);
    let s: security::SecurityState = serde_json::from_str("\"quantum-safe\"").unwrap();
    assert_eq!(s, security::SecurityState::Unknown);
    assert_eq!(
        serde_json::to_string(&security::SecurityState::Unknown).unwrap(),
        "\"unknown\""
    );
    assert_eq!(
        serde_json::to_string(&security::SecurityState::InsecureBroken).unwrap(),
        "\"insecure-broken\""
    );
}

#[test]
fn irregular_field_names_keep_their_wire_name() {
    let json = json!({
        "requestId": "1",
        "loaderId": "L",
        "documentURL": "https://example.com/",
        "request": {
            "url": "https://example.com/",
            "method": "GET",
            "headers": {"Accept": "*/*"},
            "initialPriority": "VeryHigh",
            "referrerPolicy": "no-referrer"
        },
        "timestamp": 1.5,
        "wallTime": 2.5,
        "initiator": {"type": "other"},
        "redirectHasExtraInfo": false,
        "type": "Document"
    });
    let ev: network::RequestWillBeSent = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(ev.document_url, "https://example.com/");
    assert_eq!(ev.r#type, Some(network::ResourceType::Document));
    assert_eq!(ev.request.headers["Accept"], "*/*");
    let back = serde_json::to_value(&ev).unwrap();
    assert_eq!(back["documentURL"], json["documentURL"]);
    assert_eq!(back["type"], "Document");
}

#[test]
fn recursive_types_are_boxed() {
    let json = json!({
        "callFrames": [],
        "parent": {"callFrames": [], "description": "await"}
    });
    let st: runtime::StackTrace = serde_json::from_value(json).unwrap();
    let parent: Box<runtime::StackTrace> = st.parent.unwrap();
    assert_eq!(parent.description.as_deref(), Some("await"));
}

#[test]
fn events_with_no_params_deserialise_from_empty_object() {
    let ev: page::DomContentEventFired = serde_json::from_value(json!({"timestamp": 3.0})).unwrap();
    assert!((ev.timestamp - 3.0).abs() < f64::EPSILON);
    let _: page::FrameResized = serde_json::from_value(json!({})).unwrap();
}

#[test]
fn protocol_revision_is_pinned() {
    assert_eq!(crate::domains::PROTOCOL_REVISION, 1_687_809);
}
