//! `perf` (plan 06, milestone 6.5): how fast the page is, as one compact
//! block: Navigation Timing, the Web Vitals (LCP, CLS, INP), total
//! blocking time, and the renderer's memory and counts.
//!
//! LCP, layout shifts, interactions and long tasks can only be seen with a
//! `PerformanceObserver` watching as they happen, so every top-level
//! document gets one (`perf.js`) in chrome-cdp's isolated world before its
//! own scripts run (see `tabs::setup`); `perf` reads it (`perf_read.js`).
//! A page chrome-cdp attached to after it loaded gets the observer on
//! demand, and then has what the browser buffered: everything but the
//! faster interactions.
//!
//! `Performance.getMetrics` adds the heap and counts. Its durations
//! (script, layout, style) only count while the `Performance` domain is
//! on, which costs every page; the long tasks stand in for them.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use cdp::domains::{performance as cdp_performance, runtime};
use protocol::schema::{PerfCls, PerfData, PerfInp, PerfLcp, PerfNavigation};
use protocol::{ErrorInfo, ErrorKind, Output, format};
use serde::Deserialize;

use crate::Engine;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::request::{PerfRequest, millis};
use crate::tabs::tab_name;

/// The observer every top-level document gets.
pub(crate) const OBSERVER: &str = include_str!("perf.js");

/// How often `Performance.getMetrics` is asked until it has the heap.
const METRICS_ATTEMPTS: u32 = 5;

/// What `perf` evaluates after [`OBSERVER`].
const READ: &str = include_str!("perf_read.js");

/// What `perf_read.js` returns.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Read {
    url: String,
    from_start: bool,
    navigation: Option<Navigation>,
    fcp: Option<u64>,
    requests: u64,
    transfer: f64,
    lcp: Option<Lcp>,
    cls: f64,
    shifts: u64,
    inp: Option<Inp>,
    interactions: u64,
    blocking: Option<u64>,
    long_tasks: u64,
    elements: u64,
    since_load: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Navigation {
    #[serde(rename = "type")]
    kind: String,
    ttfb: Option<u64>,
    dom_content_loaded: Option<u64>,
    load: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Lcp {
    time: Option<u64>,
    element: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Inp {
    duration: Option<u64>,
    name: String,
    target: Option<String>,
}

/// The Web Vitals thresholds of "good" and "poor".
fn rating(value: f64, good: f64, poor: f64) -> String {
    if value <= good {
        "good"
    } else if value <= poor {
        "needs improvement"
    } else {
        "poor"
    }
    .to_owned()
}

impl Engine {
    /// `perf`: the page's speed now.
    pub async fn perf(&self, req: PerfRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let work = async {
            let context = self
                .frame_context(&running, &page, None, World::Isolated, timeout)
                .await?;
            let mut params = runtime::EvaluateParams::new(format!("{OBSERVER}\n{READ}"));
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
                    format!("reading the page's timing failed: {}", e.text),
                ));
            }
            let read: Read = serde_json::from_value(result.result.value.unwrap_or_default())
                .map_err(|e| {
                    ErrorInfo::new(
                        ErrorKind::ProtocolError,
                        format!("the page's timing came back unreadable: {e}"),
                    )
                })?;
            let metrics = self.metrics(&page.session, timeout).await;
            Ok((read, metrics))
        };
        let (read, metrics) = Box::pin(self.unless_blocked(&running, &page, mark, work)).await?;
        let data = perf_data(tab_name(page.id), read, metrics);
        let text = perf_text(&data);
        let quiet = data
            .lcp
            .as_ref()
            .map_or_else(|| "-".to_owned(), |l| format!("{}ms", l.ms));
        let mut output = Output::typed(&data, text).quiet(quiet);
        if !data.from_start {
            output = output.warning(
                "chrome-cdp attached after this page loaded, so INP counts only interactions of \
                 104ms or more; reload for all of them",
            );
        }
        Ok(output)
    }

    /// `Performance.getMetrics`, with the domain on just for the call;
    /// empty if Chrome would not say.
    async fn metrics(
        &self,
        session: &cdp::Session,
        timeout: std::time::Duration,
    ) -> BTreeMap<String, f64> {
        let session = session.with_timeout(timeout);
        if session
            .call(cdp_performance::EnableParams::default())
            .await
            .is_err()
        {
            return BTreeMap::new();
        }
        let mut metrics = BTreeMap::new();
        // Right after `enable`, Chrome at times leaves out the V8 heap and
        // listener counts; they come a moment later.
        for attempt in 0..METRICS_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            match session.call(cdp_performance::GetMetricsParams {}).await {
                Ok(got) => {
                    metrics = got.metrics.into_iter().map(|m| (m.name, m.value)).collect();
                    if metrics.contains_key("JSHeapUsedSize") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = session.call(cdp_performance::DisableParams {}).await;
        metrics
    }
}

// Times and byte counts fit well within f64 and back.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn perf_data(tab: String, read: Read, metrics: BTreeMap<String, f64>) -> PerfData {
    let lcp = read.lcp.and_then(|l| {
        let ms = l.time?;
        Some(PerfLcp {
            ms,
            rating: rating(ms as f64, 2500.0, 4000.0),
            element: l.element,
            url: l.url.filter(|u| !u.is_empty()),
        })
    });
    let inp = read.inp.and_then(|i| {
        let ms = i.duration?;
        Some(PerfInp {
            ms,
            rating: rating(ms as f64, 200.0, 500.0),
            event: i.name,
            target: i.target,
        })
    });
    PerfData {
        tab,
        url: read.url,
        navigation: read.navigation.map(|n| PerfNavigation {
            kind: n.kind.replace('-', "_"),
            ttfb_ms: n.ttfb,
            dom_content_loaded_ms: n.dom_content_loaded,
            load_ms: n.load,
        }),
        fcp_ms: read.fcp,
        lcp,
        cls: PerfCls {
            value: read.cls,
            rating: rating(read.cls, 0.1, 0.25),
            shifts: read.shifts,
        },
        inp,
        interactions: read.interactions,
        tbt_ms: read.blocking.unwrap_or(0),
        long_tasks: read.long_tasks,
        requests: read.requests,
        transfer_bytes: read.transfer.max(0.0) as u64,
        elements: read.elements,
        since_load_ms: read.since_load.unwrap_or(0),
        from_start: read.from_start,
        metrics,
    }
}

/// The block `perf` prints.
// Heap sizes and counts fit well within u64.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn perf_text(data: &PerfData) -> String {
    let ms = |ms: u64| format::duration(std::time::Duration::from_millis(ms));
    let mut text = format!(
        "{} {} (loaded {} ago)\n",
        data.tab,
        data.url,
        ms(data.since_load_ms)
    );
    let mut load = Vec::new();
    if let Some(nav) = &data.navigation {
        if let Some(ttfb) = nav.ttfb_ms {
            load.push(format!("TTFB {}", ms(ttfb)));
        }
        if let Some(fcp) = data.fcp_ms {
            load.push(format!("FCP {}", ms(fcp)));
        }
        if let Some(dcl) = nav.dom_content_loaded_ms {
            load.push(format!("DOMContentLoaded {}", ms(dcl)));
        }
        load.push(match nav.load_ms {
            Some(load) => format!("load {}", ms(load)),
            None => "load not yet".to_owned(),
        });
    }
    load.push(format!(
        "{} {}, {}",
        format::thousands(data.requests as usize),
        if data.requests == 1 {
            "request"
        } else {
            "requests"
        },
        format::size(data.transfer_bytes)
    ));
    let _ = writeln!(text, "load: {}", load.join(" · "));
    match &data.lcp {
        Some(lcp) => {
            let _ = write!(text, "LCP {} {}", ms(lcp.ms), lcp.rating);
            if let Some(element) = &lcp.element {
                let _ = write!(text, " ({element})");
            }
            text.push('\n');
        }
        None => text.push_str("LCP none yet (nothing contentful painted)\n"),
    }
    let _ = writeln!(
        text,
        "CLS {:.3} {} ({} {})",
        data.cls.value,
        data.cls.rating,
        data.cls.shifts,
        if data.cls.shifts == 1 {
            "shift"
        } else {
            "shifts"
        }
    );
    match &data.inp {
        Some(inp) => {
            let _ = write!(
                text,
                "INP {} {} ({} {}; slowest: {}",
                ms(inp.ms),
                inp.rating,
                data.interactions,
                if data.interactions == 1 {
                    "interaction"
                } else {
                    "interactions"
                },
                inp.event
            );
            if let Some(target) = &inp.target {
                let _ = write!(text, " on {target}");
            }
            text.push_str(")\n");
        }
        None if data.interactions > 0 => {
            let _ = writeln!(
                text,
                "INP under 16ms ({} {})",
                data.interactions,
                if data.interactions == 1 {
                    "interaction"
                } else {
                    "interactions"
                }
            );
        }
        None => text.push_str("INP none yet (no interactions)\n"),
    }
    let _ = writeln!(
        text,
        "TBT {} ({} long {})",
        ms(data.tbt_ms),
        data.long_tasks,
        if data.long_tasks == 1 {
            "task"
        } else {
            "tasks"
        }
    );
    let mut page = vec![format!(
        "{} elements",
        format::thousands(data.elements as usize)
    )];
    if let Some(listeners) = data.metrics.get("JSEventListeners") {
        page.push(format!(
            "{} listeners",
            format::thousands(*listeners as usize)
        ));
    }
    if let (Some(used), Some(total)) = (
        data.metrics.get("JSHeapUsedSize"),
        data.metrics.get("JSHeapTotalSize"),
    ) {
        page.push(format!(
            "JS heap {} of {}",
            format::size(*used as u64),
            format::size(*total as u64)
        ));
    }
    let _ = write!(text, "page: {}", page.join(" · "));
    text
}
