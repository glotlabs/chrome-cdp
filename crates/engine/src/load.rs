//! What a load run's Chromes tell the run as it goes (plan 23.4): each
//! request that ended (its method, URL, status, cache, error and timing,
//! from the tab's Network events), each uncaught exception, and each page
//! view's Web Vitals. A load run sets a [`LoadSink`] on each engine before
//! its Chrome starts; nothing else has one, and then none of this runs.
//!
//! The Web Vitals come from `perf`'s observer (`perf.js`) through
//! `vitals.js`, in chrome-cdp's isolated world of every top-level document:
//! a page view is reported once, when the page is left (`pagehide`, through
//! [`VITALS_BINDING`]), or when the run asks at the end of an iteration
//! ([`Engine::page_vitals`]).

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

    use super::{PageVitals, exception, reported};

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
