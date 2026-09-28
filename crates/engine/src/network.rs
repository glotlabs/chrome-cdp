//! A tab's network activity (plan 03, milestone 3.4): the requests in
//! flight, for network idle (`wait --idle`, the settle after navigations
//! and actions), a short log of recent requests and responses, for
//! `wait --request` and `wait --response`, and of failed requests, for
//! the post-action report (5.5).
//!
//! The tab's page session and its OOPIF sessions feed it their `Network`
//! events (see `tabs::watch_host`). Requests are keyed by session and
//! request id, since each session numbers its own. Plan 06's network
//! buffer can grow from the same events.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

/// How long the network must stay quiet to count as idle.
pub(crate) const IDLE_QUIET: Duration = Duration::from_millis(500);

/// Requests open longer than this (long-polls, streams) no longer keep
/// the network busy.
pub(crate) const LONG_LIVED: Duration = Duration::from_secs(10);

/// Resource types that never keep the network busy: they stay open by
/// design (`EventSource`) or nobody waits for them (beacons, CSP reports).
/// WebSockets are not requests at all in CDP.
const IGNORED: &[&str] = &["EventSource", "Ping", "CSPViolationReport", "WebSocket"];

/// How many requests and responses the log keeps.
const LOG_LEN: usize = 256;

/// A bound on the requests followed, in case Chrome never ends some.
const MAX_IN_FLIGHT: usize = 1000;

/// One request that has not finished yet.
#[derive(Debug, Clone)]
pub(crate) struct InFlight {
    pub(crate) method: String,
    pub(crate) url: String,
    /// Chrome's resource type (`Document`, `Fetch`, `Ping`, …).
    pub(crate) kind: String,
    pub(crate) started: Instant,
    /// The frame and document (loader) that sent it.
    frame: String,
    loader: String,
}

impl InFlight {
    /// Whether it keeps the network busy at `now`.
    fn counts(&self, now: Instant) -> bool {
        !IGNORED.contains(&self.kind.as_str()) && now.duration_since(self.started) < LONG_LIVED
    }
}

/// A request sent or a response received, in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Exchange {
    /// Its place in the tab's log; later ones are larger.
    pub(crate) seq: u64,
    pub(crate) method: String,
    pub(crate) url: String,
    /// The HTTP status, for a response.
    pub(crate) status: Option<u16>,
}

/// A request that failed: an HTTP error status (400 or more), or no
/// response at all (`net::ERR_…`). Requests the page or a navigation
/// canceled are not failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    /// Its place in the tab's log (shared with [`Exchange::seq`]).
    pub(crate) seq: u64,
    pub(crate) method: String,
    pub(crate) url: String,
    /// Chrome's resource type (`Document`, `Fetch`, `Script`, …).
    pub(crate) kind: String,
    /// The HTTP status, if it answered.
    pub(crate) status: Option<u16>,
    /// Chrome's error, if it did not: `net::ERR_CONNECTION_REFUSED`.
    pub(crate) error: Option<String>,
    /// The session that sent it and its id there, to find it in the
    /// tab's network log.
    pub(crate) session: String,
    pub(crate) request_id: String,
}

/// The network activity of one tab.
#[derive(Debug)]
pub(crate) struct Network {
    in_flight: HashMap<(String, String), InFlight>,
    /// When a request that counts last started or ended.
    last_activity: Instant,
    requests: VecDeque<Exchange>,
    responses: VecDeque<Exchange>,
    failures: VecDeque<Failure>,
    seq: u64,
}

impl Default for Network {
    fn default() -> Self {
        Self {
            in_flight: HashMap::new(),
            last_activity: Instant::now(),
            requests: VecDeque::new(),
            responses: VecDeque::new(),
            failures: VecDeque::new(),
            seq: 0,
        }
    }
}

/// Whether the network is idle, and if not, when to look again (besides
/// on the next change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Idle {
    Now,
    /// Quiet now, but not for long enough: idle at this instant unless a
    /// request starts.
    At(Instant),
    /// Requests are in flight; the oldest stops counting at this instant.
    Busy(Instant),
}

impl Network {
    /// The number of the last request or response logged: a mark for
    /// [`Network::request_since`] and [`Network::response_since`].
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// Applies one `Network` event of session `session`; `false` if it is
    /// not one this tracker follows.
    pub(crate) fn event(&mut self, session: &str, method: &str, params: &Value) -> bool {
        let now = Instant::now();
        let Some(id) = params["requestId"].as_str() else {
            return false;
        };
        let key = (session.to_owned(), id.to_owned());
        let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        match method {
            "Network.requestWillBeSent" => {
                let request = &params["request"];
                let entry = InFlight {
                    method: text(&request["method"]),
                    url: text(&request["url"]),
                    kind: params["type"].as_str().unwrap_or("Other").to_owned(),
                    started: now,
                    frame: text(&params["frameId"]),
                    loader: text(&params["loaderId"]),
                };
                self.log_request(&entry);
                // A redirect goes on under the same id: keep its start.
                let started = self.in_flight.get(&key).map(|r| r.started);
                let entry = InFlight {
                    started: started.unwrap_or(now),
                    ..entry
                };
                if entry.counts(now) {
                    self.last_activity = now;
                }
                if self.in_flight.len() >= MAX_IN_FLIGHT
                    && let Some(oldest) = self.oldest()
                {
                    self.in_flight.remove(&oldest);
                }
                self.in_flight.insert(key, entry);
            }
            "Network.responseReceived" => {
                let response = &params["response"];
                let method = self
                    .in_flight
                    .get(&key)
                    .map(|r| r.method.clone())
                    .unwrap_or_default();
                let status = response["status"]
                    .as_u64()
                    .and_then(|s| u16::try_from(s).ok());
                self.seq += 1;
                let exchange = Exchange {
                    seq: self.seq,
                    method,
                    url: text(&response["url"]),
                    status,
                };
                if status.is_some_and(|s| s >= 400) {
                    push(
                        &mut self.failures,
                        Failure {
                            seq: self.seq,
                            method: exchange.method.clone(),
                            url: exchange.url.clone(),
                            kind: params["type"].as_str().unwrap_or("Other").to_owned(),
                            status,
                            error: None,
                            session: key.0.clone(),
                            request_id: key.1.clone(),
                        },
                    );
                }
                push(&mut self.responses, exchange);
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                let Some(done) = self.in_flight.remove(&key) else {
                    return true;
                };
                if done.counts(now) {
                    self.last_activity = now;
                }
                let canceled = params["canceled"].as_bool() == Some(true);
                if method == "Network.loadingFailed" && !canceled {
                    self.seq += 1;
                    push(
                        &mut self.failures,
                        Failure {
                            seq: self.seq,
                            method: done.method,
                            url: done.url,
                            kind: done.kind,
                            status: None,
                            error: Some(text(&params["errorText"])),
                            session: key.0,
                            request_id: key.1,
                        },
                    );
                }
            }
            _ => return false,
        }
        true
    }

    fn log_request(&mut self, request: &InFlight) {
        self.seq += 1;
        push(
            &mut self.requests,
            Exchange {
                seq: self.seq,
                method: request.method.clone(),
                url: request.url.clone(),
                status: None,
            },
        );
    }

    fn oldest(&self) -> Option<(String, String)> {
        self.in_flight
            .iter()
            .min_by_key(|(_, r)| r.started)
            .map(|(k, _)| k.clone())
    }

    /// Frame `frame` of `session` committed document `loader`: the
    /// requests of the documents it replaced are over, though Chrome does
    /// not always say so. When the main frame navigates, that is every
    /// other request of the session (its subframes went too). Requests
    /// other sessions sent for the frame are over too: a cross-site
    /// iframe's document is requested on its parent's session, but ends
    /// on the iframe's own, where the parent never hears of it.
    pub(crate) fn navigated(&mut self, session: &str, frame: &str, loader: &str, main: bool) {
        self.in_flight.retain(|(s, _), r| {
            if s == session {
                r.loader == loader || !(main || r.frame == frame)
            } else {
                r.frame != frame
            }
        });
    }

    /// Forgets the requests of a session that went away (an OOPIF).
    pub(crate) fn session_gone(&mut self, session: &str) {
        self.in_flight.retain(|(s, _), _| s != session);
    }

    /// Whether the network has been idle since `since` at `now`: no
    /// request that counts in flight, and none started or ended for
    /// [`IDLE_QUIET`].
    pub(crate) fn idle(&self, since: Instant, now: Instant) -> Idle {
        self.idle_for(since, now, IDLE_QUIET, false)
    }

    /// Like [`Network::idle`], for the brief settle after an action: the
    /// network must be quiet for `quiet`, and with `own`, only requests
    /// that started since `since` count (a page's long-poll from before
    /// is not the action's doing).
    pub(crate) fn idle_for(
        &self,
        since: Instant,
        now: Instant,
        quiet: Duration,
        own: bool,
    ) -> Idle {
        let mut quiet_since = since.max(self.last_activity);
        let mut busy_until: Option<Instant> = None;
        for request in self.in_flight.values() {
            if IGNORED.contains(&request.kind.as_str()) || (own && request.started < since) {
                continue;
            }
            let long_lived = request.started + LONG_LIVED;
            if now < long_lived {
                busy_until = Some(busy_until.map_or(long_lived, |b| b.min(long_lived)));
            } else {
                // It stopped counting then: quiet from that moment.
                quiet_since = quiet_since.max(long_lived);
            }
        }
        if let Some(until) = busy_until {
            return Idle::Busy(until);
        }
        let at = quiet_since + quiet;
        if now >= at { Idle::Now } else { Idle::At(at) }
    }

    /// The requests that keep the network busy at `now`, oldest first.
    pub(crate) fn busy(&self, now: Instant) -> Vec<InFlight> {
        let mut busy: Vec<InFlight> = self
            .in_flight
            .values()
            .filter(|r| r.counts(now))
            .cloned()
            .collect();
        busy.sort_by_key(|r| r.started);
        busy
    }

    /// The first request logged after mark `after` that `matches` its URL.
    pub(crate) fn request_since(
        &self,
        after: u64,
        matches: impl Fn(&Exchange) -> bool,
    ) -> Option<Exchange> {
        self.requests
            .iter()
            .find(|e| e.seq > after && matches(e))
            .cloned()
    }

    /// The first response logged after mark `after` that `matches`.
    pub(crate) fn response_since(
        &self,
        after: u64,
        matches: impl Fn(&Exchange) -> bool,
    ) -> Option<Exchange> {
        self.responses
            .iter()
            .find(|e| e.seq > after && matches(e))
            .cloned()
    }

    /// The failed requests logged after mark `after` (a [`Network::seq`]),
    /// oldest first.
    pub(crate) fn failures_since(&self, after: u64) -> Vec<Failure> {
        self.failures
            .iter()
            .filter(|f| f.seq > after)
            .cloned()
            .collect()
    }
}

fn push<T>(log: &mut VecDeque<T>, entry: T) {
    if log.len() >= LOG_LEN {
        log.pop_front();
    }
    log.push_back(entry);
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::time::Instant;

    use super::{IDLE_QUIET, Idle, LONG_LIVED, Network};

    fn sent(net: &mut Network, id: &str, kind: &str, url: &str) {
        let params = json!({
            "requestId": id,
            "type": kind,
            "frameId": "main",
            "loaderId": "doc1",
            "request": { "method": "GET", "url": url },
        });
        assert!(net.event("s1", "Network.requestWillBeSent", &params));
    }

    fn finished(net: &mut Network, id: &str) {
        assert!(net.event("s1", "Network.loadingFinished", &json!({ "requestId": id })));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_after_quiet() {
        let mut net = Network::default();
        let start = Instant::now();
        assert_eq!(net.idle(start, start), Idle::At(start + IDLE_QUIET));
        sent(&mut net, "1", "Fetch", "http://x/api");
        assert!(matches!(net.idle(start, Instant::now()), Idle::Busy(_)));
        tokio::time::advance(IDLE_QUIET).await;
        finished(&mut net, "1");
        let end = Instant::now();
        assert_eq!(net.idle(start, end), Idle::At(end + IDLE_QUIET));
        assert_eq!(net.idle(start, end + IDLE_QUIET), Idle::Now);
    }

    #[tokio::test(start_paused = true)]
    async fn beacons_and_event_streams_do_not_count() {
        let mut net = Network::default();
        let start = Instant::now();
        sent(&mut net, "1", "Ping", "http://x/beacon");
        sent(&mut net, "2", "EventSource", "http://x/events");
        assert_eq!(net.idle(start, start + IDLE_QUIET), Idle::Now);
        assert!(net.busy(start).is_empty());
        // They are still logged for `wait --request`.
        assert_eq!(
            net.request_since(0, |_| true).unwrap().url,
            "http://x/beacon"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn long_lived_requests_stop_counting() {
        let mut net = Network::default();
        let start = Instant::now();
        sent(&mut net, "1", "XHR", "http://x/poll");
        assert_eq!(net.idle(start, start), Idle::Busy(start + LONG_LIVED));
        let aged = start + LONG_LIVED;
        assert_eq!(net.idle(start, aged), Idle::At(aged + IDLE_QUIET));
        assert_eq!(net.idle(start, aged + IDLE_QUIET), Idle::Now);
        // Its end is not activity either.
        tokio::time::advance(LONG_LIVED * 2).await;
        finished(&mut net, "1");
        assert_eq!(net.idle(start, Instant::now()), Idle::Now);
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_document_ends_the_old_one_s_requests() {
        let mut net = Network::default();
        sent(&mut net, "1", "Fetch", "http://x/slow");
        net.navigated("s1", "child", "doc2", false);
        assert_eq!(net.busy(Instant::now()).len(), 1);
        net.navigated("s1", "main", "doc2", true);
        assert!(net.busy(Instant::now()).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_iframe_s_own_session_ends_its_document_request() {
        let mut net = Network::default();
        let params = json!({
            "requestId": "9",
            "type": "Document",
            "frameId": "child",
            "loaderId": "doc3",
            "request": { "method": "GET", "url": "http://y/child.html" },
        });
        net.event("s1", "Network.requestWillBeSent", &params);
        sent(&mut net, "1", "Fetch", "http://x/slow");
        // The out-of-process iframe commits it on its session, s2.
        net.navigated("s2", "child", "doc3", false);
        let busy = net.busy(Instant::now());
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].url, "http://x/slow");
    }

    #[tokio::test(start_paused = true)]
    async fn failures_are_error_statuses_and_errors_not_cancels() {
        let mut net = Network::default();
        let mark = net.seq();
        sent(&mut net, "1", "Fetch", "http://x/api/save");
        let response = json!({
            "requestId": "1",
            "type": "Fetch",
            "response": { "url": "http://x/api/save", "status": 500 },
        });
        net.event("s1", "Network.responseReceived", &response);
        finished(&mut net, "1");
        sent(&mut net, "2", "Script", "http://x/app.js");
        let failed = json!({ "requestId": "2", "errorText": "net::ERR_CONNECTION_REFUSED" });
        net.event("s1", "Network.loadingFailed", &failed);
        sent(&mut net, "3", "Fetch", "http://x/aborted");
        let canceled =
            json!({ "requestId": "3", "errorText": "net::ERR_ABORTED", "canceled": true });
        net.event("s1", "Network.loadingFailed", &canceled);
        let failures = net.failures_since(mark);
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert_eq!(failures[0].status, Some(500));
        assert_eq!(failures[0].kind, "Fetch");
        assert_eq!(failures[1].url, "http://x/app.js");
        assert_eq!(
            failures[1].error.as_deref(),
            Some("net::ERR_CONNECTION_REFUSED")
        );
        assert!(net.failures_since(failures[1].seq).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn own_requests_are_those_since_the_action() {
        let mut net = Network::default();
        sent(&mut net, "1", "XHR", "http://x/poll");
        tokio::time::advance(IDLE_QUIET).await;
        let since = Instant::now();
        let quiet = std::time::Duration::from_millis(150);
        assert_eq!(
            net.idle_for(since, since, quiet, true),
            Idle::At(since + quiet)
        );
        assert!(matches!(
            net.idle_for(since, since, quiet, false),
            Idle::Busy(_)
        ));
        sent(&mut net, "2", "Fetch", "http://x/api");
        assert!(matches!(
            net.idle_for(since, since, quiet, true),
            Idle::Busy(_)
        ));
    }

    #[test]
    fn responses_are_logged_with_their_request_method() {
        let mut net = Network::default();
        let mark = net.seq();
        let params = json!({
            "requestId": "7",
            "type": "Fetch",
            "request": { "method": "POST", "url": "http://x/api/save" },
        });
        net.event("s1", "Network.requestWillBeSent", &params);
        let response = json!({
            "requestId": "7",
            "type": "Fetch",
            "response": { "url": "http://x/api/save", "status": 201 },
        });
        net.event("s1", "Network.responseReceived", &response);
        let found = net
            .response_since(mark, |e| e.url.ends_with("/save"))
            .unwrap();
        assert_eq!(found.method, "POST");
        assert_eq!(found.status, Some(201));
        assert!(net.response_since(found.seq, |_| true).is_none());
        net.session_gone("s1");
        assert!(net.busy(Instant::now()).is_empty());
    }
}
