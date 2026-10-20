//! What a load run's Chromes tell the run as it goes (plan 23.4): each
//! request that ended (its method, URL, status, cache, error and timing,
//! from the tab's Network events), each uncaught exception, and each page
//! view's Web Vitals. A load run sets a [`LoadSink`] on each engine before
//! its Chrome starts; nothing else has one, and then none of this runs.
//!
//! A tab's requests are followed for the sink on their own ([`Inflight`]),
//! not through its network log, which keeps a load run's last 300: a page
//! with more requests in flight would lose some.
//!
//! The Web Vitals come from `perf`'s observer (`perf.js`) through
//! `vitals.js`, in chrome-cdp's isolated world of every top-level document:
//! a page view is reported once, when the page is left (`pagehide`, through
//! [`VITALS_BINDING`]), or when the run asks at the end of an iteration
//! ([`Engine::page_vitals`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use cdp::domains::runtime;
use protocol::schema::Timing;
use protocol::{ErrorInfo, ErrorKind};
use serde::Deserialize;

use crate::Engine;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::request::millis;

/// The binding `vitals.js` reports a page it leaves through.
pub(crate) const VITALS_BINDING: &str = "__chromeCdpVitals";

/// The page view reporter, after `perf.js`.
pub(crate) const VITALS: &str = include_str!("observe/vitals.js");

/// What [`Engine::page_vitals`] evaluates.
const READ_VITALS: &str = "(globalThis[Symbol.for('chrome-cdp.vitals')] || (() => null))()";

/// Something a load run's Chrome did.
#[derive(Debug, Clone)]
pub enum LoadEvent {
    Request(LoadRequest),
    Vitals(PageVitals),
    /// An uncaught exception, by its message.
    Exception(String),
}

/// A request that ended: done, or failed (not one the page canceled, nor
/// one `[load] block` failed).
#[derive(Debug, Clone)]
pub struct LoadRequest {
    pub method: String,
    pub url: String,
    /// Chrome's resource type, in lower case: `document`, `fetch`, `script`, ….
    pub kind: String,
    /// `None` when it failed without a response.
    pub status: Option<u16>,
    /// Which cache served it: `memory`, `disk` or `prefetch`.
    pub cache: Option<String>,
    /// Chrome's error, for a request that failed: `net::ERR_CONNECTION_REFUSED`.
    pub error: Option<String>,
    /// From its start to its end, in ms.
    pub duration_ms: Option<f64>,
    /// Its phases, when a response came.
    pub timing: Option<Timing>,
}

/// A page view's Web Vitals and navigation timing, in ms (CLS is a
/// score); `None` for what the page did not have.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PageVitals {
    pub url: String,
    pub ttfb: Option<f64>,
    pub fcp: Option<f64>,
    /// When the `load` event ended.
    pub load: Option<f64>,
    pub lcp: Option<f64>,
    pub cls: Option<f64>,
    pub inp: Option<f64>,
}

/// Where a load run's engine sends its [`LoadEvent`]s. It is called on
/// the engine's event tasks, so it must be quick.
pub type LoadSink = Arc<dyn Fn(LoadEvent) + Send + Sync>;

/// An engine's [`LoadSink`], once set.
#[derive(Default)]
pub(crate) struct SinkSlot(OnceLock<LoadSink>);

impl SinkSlot {
    pub(crate) fn get(&self) -> Option<&LoadSink> {
        self.0.get()
    }
}

impl fmt::Debug for SinkSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.get().is_some() {
            "SinkSlot(set)"
        } else {
            "SinkSlot(none)"
        })
    }
}

impl Engine {
    /// Sends what this engine's Chromes do to `sink` (a load run's): the
    /// first sink set stays. Set it before the Chrome starts, so every
    /// page gets the Web Vitals reporter.
    pub fn set_load_sink(&self, sink: LoadSink) {
        let _ = self.settings.load_sink.0.set(sink);
    }

    /// The Web Vitals of the page of the call's context (`in_context`),
    /// unless it was reported already (it was left, or read before):
    /// `None` then, and for a page that is not http(s).
    pub async fn page_vitals(&self, timeout_ms: u64) -> Result<Option<PageVitals>, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(timeout_ms);
        let page = self.log_tab(&running, None, timeout).await?;
        let context = self
            .frame_context(&running, &page, None, World::Isolated, timeout)
            .await?;
        let mut params = runtime::EvaluateParams::new(READ_VITALS.to_owned());
        params.context_id = Some(context.context_id);
        params.return_by_value = Some(true);
        let result = context
            .session
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        if let Some(e) = result.exception_details {
            return Err(ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("reading the page's Web Vitals failed: {}", e.text),
            ));
        }
        match result.result.value {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(value) => serde_json::from_value(value).map(Some).map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    format!("the page's Web Vitals came back unreadable: {e}"),
                )
            }),
        }
    }
}

/// The most requests a tab follows at once for the sink; past it, new
/// ones are not followed (one that never ends must not grow it forever).
const MOST_IN_FLIGHT: usize = 20_000;

/// A tab's requests in flight, for a load run's sink.
#[derive(Debug, Default)]
pub(crate) struct Inflight {
    /// By session and request id.
    requests: HashMap<(String, String), Flying>,
}

/// A request in flight.
#[derive(Debug)]
struct Flying {
    request: LoadRequest,
    /// When it started, and when its headers were in, on Chrome's clock
    /// (seconds).
    started: Option<f64>,
    headers_end: Option<f64>,
}

impl Inflight {
    /// Takes in a `Network` event of session `session`; the request it
    /// ended, if it ended one the run counts (not one the page canceled,
    /// nor one `[load] block` failed).
    pub(crate) fn event(
        &mut self,
        session: &str,
        method: &str,
        params: &serde_json::Value,
    ) -> Option<LoadRequest> {
        let id = params["requestId"].as_str()?;
        let key = (session.to_owned(), id.to_owned());
        match method {
            "Network.requestWillBeSent" => {
                // A redirect ends the hop before it.
                let hop = if params["redirectResponse"].is_object() {
                    self.requests.remove(&key).map(|mut flying| {
                        flying.response(&params["redirectResponse"]);
                        flying.end(params["timestamp"].as_f64())
                    })
                } else {
                    None
                };
                if self.requests.len() < MOST_IN_FLIGHT {
                    let request = &params["request"];
                    self.requests.insert(
                        key,
                        Flying {
                            request: LoadRequest {
                                method: request["method"].as_str().unwrap_or("GET").to_owned(),
                                url: request["url"].as_str().unwrap_or_default().to_owned(),
                                kind: params["type"]
                                    .as_str()
                                    .unwrap_or("other")
                                    .to_ascii_lowercase(),
                                status: None,
                                cache: None,
                                error: None,
                                duration_ms: None,
                                timing: None,
                            },
                            started: params["timestamp"].as_f64(),
                            headers_end: None,
                        },
                    );
                }
                hop
            }
            "Network.requestServedFromCache" => {
                if let Some(flying) = self.requests.get_mut(&key) {
                    flying
                        .request
                        .cache
                        .get_or_insert_with(|| "memory".to_owned());
                }
                None
            }
            "Network.responseReceived" => {
                if let Some(flying) = self.requests.get_mut(&key) {
                    flying.response(&params["response"]);
                    if let Some(kind) = params["type"].as_str() {
                        flying.request.kind = kind.to_ascii_lowercase();
                    }
                }
                None
            }
            "Network.loadingFinished" => self
                .requests
                .remove(&key)
                .map(|f| f.end(params["timestamp"].as_f64())),
            "Network.loadingFailed" => {
                let mut flying = self.requests.remove(&key)?;
                let blocked = params["blockedReason"].as_str() == Some("inspector");
                if params["canceled"].as_bool() == Some(true) || blocked {
                    return None;
                }
                flying.request.error = params["errorText"]
                    .as_str()
                    .filter(|e| !e.is_empty())
                    .map(str::to_owned);
                // A response that came, then failed to load, is a failure.
                flying.request.status = None;
                Some(flying.end(params["timestamp"].as_f64()))
            }
            _ => None,
        }
    }
}

impl Flying {
    fn response(&mut self, response: &serde_json::Value) {
        self.request.status = response["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok());
        if response["fromDiskCache"].as_bool() == Some(true) {
            self.request.cache = Some("disk".to_owned());
        } else if response["fromPrefetchCache"].as_bool() == Some(true) {
            self.request.cache = Some("prefetch".to_owned());
        }
        let timing = &response["timing"];
        if let Some(request_time) = timing["requestTime"].as_f64() {
            self.request.timing = Some(crate::observe::netlog::phases(
                timing,
                request_time,
                self.started,
            ));
            let headers_end = timing["receiveHeadersEnd"].as_f64().unwrap_or(0.0);
            self.headers_end = Some(request_time + headers_end / 1000.0);
        }
    }

    /// The request, ended at `timestamp` (Chrome's clock).
    fn end(mut self, timestamp: Option<f64>) -> LoadRequest {
        if let Some(end) = timestamp {
            if let Some(start) = self.started {
                self.request.duration_ms = Some(((end - start) * 1000.0).max(0.0));
            }
            if let (Some(timing), Some(headers_end)) = (&mut self.request.timing, self.headers_end)
            {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let receive = ((end - headers_end) * 1000.0).max(0.0).round() as u64;
                timing.receive_ms = Some(receive);
            }
        }
        self.request
    }
}

/// The page view a `Runtime.bindingCalled` on [`VITALS_BINDING`] reports.
pub(crate) fn reported(params: &serde_json::Value) -> Option<PageVitals> {
    serde_json::from_str(params["payload"].as_str()?).ok()
}

/// The message of a `Runtime.exceptionThrown`.
pub(crate) fn exception(params: &serde_json::Value) -> String {
    let details = &params["exceptionDetails"];
    let text = details["exception"]["description"]
        .as_str()
        .or_else(|| details["text"].as_str())
        .unwrap_or("exception");
    text.lines().next().unwrap_or(text).to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Inflight, PageVitals, exception, reported};

    #[test]
    fn requests_are_followed_from_start_to_end() {
        let mut flying = Inflight::default();
        let sent = |id: &str, url: &str| {
            json!({"requestId": id, "timestamp": 10.0, "type": "Fetch",
                   "request": {"method": "POST", "url": url}})
        };
        assert!(
            flying
                .event(
                    "s",
                    "Network.requestWillBeSent",
                    &sent("1", "http://a/api/cart")
                )
                .is_none()
        );
        flying.event(
            "s",
            "Network.responseReceived",
            &json!({"requestId": "1", "type": "Fetch", "response": {"status": 201,
                    "timing": {"requestTime": 10.0, "sendStart": 1.0, "sendEnd": 2.0,
                               "receiveHeadersEnd": 200.0}}}),
        );
        let done = flying
            .event(
                "s",
                "Network.loadingFinished",
                &json!({"requestId": "1", "timestamp": 10.25}),
            )
            .unwrap();
        assert_eq!(
            (done.method.as_str(), done.kind.as_str()),
            ("POST", "fetch")
        );
        assert_eq!(done.status, Some(201));
        assert!((done.duration_ms.unwrap() - 250.0).abs() < 1e-6);
        let timing = done.timing.unwrap();
        assert_eq!((timing.wait_ms, timing.receive_ms), (198, Some(50)));
        // Canceled and blocked requests are not counted; failed ones are.
        flying.event("s", "Network.requestWillBeSent", &sent("2", "http://a/x"));
        let canceled = json!({"requestId": "2", "timestamp": 11.0, "canceled": true});
        assert!(
            flying
                .event("s", "Network.loadingFailed", &canceled)
                .is_none()
        );
        flying.event("s", "Network.requestWillBeSent", &sent("3", "http://ads/x"));
        let blocked = json!({"requestId": "3", "blockedReason": "inspector"});
        assert!(
            flying
                .event("s", "Network.loadingFailed", &blocked)
                .is_none()
        );
        flying.event("s", "Network.requestWillBeSent", &sent("4", "http://a/y"));
        let failed = flying
            .event(
                "s",
                "Network.loadingFailed",
                &json!({"requestId": "4", "timestamp": 10.5,
                   "errorText": "net::ERR_CONNECTION_RESET"}),
            )
            .unwrap();
        assert_eq!(failed.error.as_deref(), Some("net::ERR_CONNECTION_RESET"));
        assert_eq!(failed.status, None);
        assert!(flying.requests.is_empty());
    }

    #[test]
    fn a_reported_page_view_is_read() {
        let params = json!({
            "name": "__chromeCdpVitals",
            "payload": r#"{"url":"http://a/","ttfb":12.5,"fcp":null,"load":80,"lcp":90,"cls":0,"inp":null}"#,
        });
        assert_eq!(
            reported(&params),
            Some(PageVitals {
                url: "http://a/".into(),
                ttfb: Some(12.5),
                load: Some(80.0),
                lcp: Some(90.0),
                cls: Some(0.0),
                ..PageVitals::default()
            })
        );
        assert_eq!(reported(&json!({"payload": "nope"})), None);
    }

    #[test]
    fn an_exception_is_its_first_line() {
        let params = json!({"exceptionDetails": {
            "text": "Uncaught",
            "exception": {"description": "TypeError: x is undefined\n    at f (app.js:1:2)"},
        }});
        assert_eq!(exception(&params), "TypeError: x is undefined");
        assert_eq!(
            exception(&json!({"exceptionDetails": {"text": "Uncaught 42"}})),
            "Uncaught 42"
        );
    }
}
