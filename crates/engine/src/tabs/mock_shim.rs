//! The mock shim's calls (plan 11, milestone 11.9): a page's WebSocket
//! or EventSource asks what it gets, and the answers go back to it.
//!
//! The shim (`route/shim.js`) calls [`MOCK_BINDING`] with a JSON message:
//! `ws-open`, `ws-send` (`data`, or a binary message's bytes as `base64`),
//! `ws-close`, `sse-open` or `sse-close`, each with the shim's id of the
//! socket. The router decides (see
//! `route::sockets`); the answers are handed to the document the call
//! came from with `Runtime.evaluate`, each after its delay, in a task of
//! their own.

use std::sync::Arc;
use std::time::Duration;

use cdp::domains::runtime;
use cdp::{RawEvent, Session};
use serde_json::{Value, json};

use super::{Shared, Tabs};
use crate::route::{self, Socket, SseReply, WsReply, frame_message};

/// How long a delivery may take.
const DELIVER_TIMEOUT: Duration = Duration::from_secs(5);

/// Hands `message` to the shim of execution context `context`; whether
/// it knew the socket.
async fn deliver(session: &Session, context: i64, message: &Value) -> bool {
    let mut params = runtime::EvaluateParams::new(format!(
        "globalThis[Symbol.for('chrome-cdp.mock')]?.({message}) === true"
    ));
    params.context_id = Some(context);
    params.return_by_value = Some(true);
    params.silent = Some(true);
    match session.with_timeout(DELIVER_TIMEOUT).call(params).await {
        Ok(result) => result.result.value == Some(Value::Bool(true)),
        Err(e) => {
            tracing::debug!("cannot hand a mock's message to the page: {e}");
            false
        }
    }
}

/// A `Runtime.bindingCalled` on [`MOCK_BINDING`] from session
/// `session_id` (tab `tab`).
pub(super) fn called(shared: &Arc<Shared>, session_id: &str, tab: Option<u32>, event: &RawEvent) {
    let Some(message) = event.params["payload"]
        .as_str()
        .and_then(|p| serde_json::from_str::<Value>(p).ok())
    else {
        return;
    };
    let Some(context) = event.params["executionContextId"].as_i64() else {
        return;
    };
    let Some((session, scope)) = shared
        .lock()
        .targets
        .get(session_id)
        .map(|t| (t.session.clone(), t.scope.clone()))
    else {
        return;
    };
    let id = message["id"].as_u64().unwrap_or_default();
    let url = message["url"].as_str().unwrap_or_default().to_owned();
    let routes = &shared.settings.routes;
    let now_ms = super::fetch::now_ms(shared, scope.as_deref());
    let headers = Value::Null;
    let request = |resource_type: &'static str| route::Request {
        url: &url,
        method: "GET",
        resource_type,
        headers: &headers,
        context: scope.as_deref(),
        tab,
        body: None,
        now_ms,
    };
    match message["op"].as_str().unwrap_or_default() {
        "ws-open" => {
            let protocols: Vec<String> = message["protocols"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect();
            let (reply, mocked) = routes.ws_open(&request("WebSocket"), &protocols);
            let (first, steps) = match reply {
                WsReply::Real => (json!({ "id": id, "decision": "real" }), Vec::new()),
                WsReply::Fail => (json!({ "id": id, "decision": "fail" }), Vec::new()),
                WsReply::Mock { protocol, steps } => (
                    json!({ "id": id, "decision": "mock", "protocol": protocol }),
                    steps,
                ),
            };
            let socket = mocked.map(|(key, mock)| Socket {
                scope: scope.clone(),
                session: session_id.to_owned(),
                context,
                id,
                url: url.clone(),
                tab,
                key,
                mock,
            });
            if let Some(socket) = &socket {
                routes.add_socket(socket.clone());
            }
            let settings = shared.settings.clone();
            shared.spawn(async move {
                if !deliver(&session, context, &first).await {
                    return;
                }
                if let Some(socket) = socket {
                    send_steps(&settings.routes, &session, &socket, steps).await;
                }
            });
        }
        "ws-send" => {
            let Some(socket) = routes.socket(session_id, context, id) else {
                return;
            };
            // A binary message comes as base64.
            let message = match (message["data"].as_str(), message["base64"].as_str()) {
                (Some(text), _) => mock::WsMessage::Text(text),
                (None, Some(base64)) => mock::WsMessage::Binary(base64),
                (None, None) => return,
            };
            let steps = routes.ws_message(&socket, message, now_ms);
            if steps.is_empty() {
                return;
            }
            let settings = shared.settings.clone();
            shared.spawn(async move {
                send_steps(&settings.routes, &session, &socket, steps).await;
            });
        }
        "ws-close" => routes.remove_socket(session_id, context, id),
        "sse-open" => {
            let reply = routes.sse_open(&request("EventSource"));
            shared.spawn(async move {
                match reply {
                    SseReply::Real => {
                        deliver(&session, context, &json!({ "id": id, "decision": "real" })).await;
                    }
                    SseReply::Fail => {
                        deliver(&session, context, &json!({ "id": id, "decision": "fail" })).await;
                    }
                    SseReply::Mock { delay, events } => {
                        tokio::time::sleep(delay).await;
                        let open = json!({ "id": id, "decision": "mock" });
                        if !deliver(&session, context, &open).await {
                            return;
                        }
                        for event in events {
                            tokio::time::sleep(event.delay).await;
                            let message = json!({
                                "id": id,
                                "event": {
                                    "event": event.event,
                                    "data": event.data,
                                    "id": event.id,
                                },
                            });
                            // A closed stream (or document) takes no more.
                            if !deliver(&session, context, &message).await {
                                return;
                            }
                        }
                    }
                }
            });
        }
        // The shim stops taking its events; nothing to keep.
        _ => {}
    }
}

/// Hands `steps` to `socket`, each after its delay; a socket that is
/// gone, or that the mock closes, is forgotten.
async fn send_steps(
    routes: &route::Router,
    session: &Session,
    socket: &Socket,
    steps: Vec<mock::WsStep>,
) {
    for step in steps {
        if !step.delay.is_zero() {
            tokio::time::sleep(step.delay).await;
        }
        let closes = matches!(step.frame, mock::WsFrame::Close { .. });
        let known = deliver(session, socket.context, &frame_message(socket.id, &step)).await;
        if !known || closes {
            routes.remove_socket(&socket.session, socket.context, socket.id);
            return;
        }
    }
}

impl Tabs {
    /// Hands text `message` to `socket`, as its mock would; whether it
    /// was still there to take it (if not, it is forgotten).
    pub(crate) async fn ws_deliver(&self, socket: &Socket, message: &str) -> bool {
        let session = self
            .shared
            .lock()
            .targets
            .get(&socket.session)
            .map(|t| t.session.clone());
        let known = match session {
            Some(session) => {
                let text = json!({ "id": socket.id, "text": message });
                deliver(&session, socket.context, &text).await
            }
            None => false,
        };
        if !known {
            self.shared
                .settings
                .routes
                .remove_socket(&socket.session, socket.context, socket.id);
        }
        known
    }
}
