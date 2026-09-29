//! `expect request <METHOD> <path-glob>` and `expect no-request` (plan
//! 11, milestone 11.5): the requests the pages sent, from the network
//! logs of the session's tabs (or the context's, with `--context`, which
//! is each test's own), so they work with mocks, routes or neither.
//!
//! The conditions are a mock's (`mock::RequestFilter`): path and URL
//! globs, query and header globs, and a JSON body subset. Each request
//! counts once (a redirect's hops are one request). A check that more
//! requests can still make hold is looked at again every [`POLL`] until
//! it holds or the timeout passes; one that too many requests broke
//! (`no-request`, `--max`, `--count`) fails at once. A failure lists the
//! matching requests and the ones that come closest, with the condition
//! each fails.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::Duration;

use mock::{RequestFilter, Verdict};
use protocol::schema::ExpectData;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{assertion_text, expected};
use crate::context::Scoped as _;
use crate::observe::netlog::Record;
use crate::request::{ExpectRequest, RequestCheck, millis};
use crate::tabs::tab_name;
use crate::{Engine, Running};

/// How often the logs are looked at again.
const POLL: Duration = Duration::from_millis(25);

/// How many matching and similar requests a failure lists.
const LISTED: usize = 5;

/// How long a body too large for the log is waited for.
const BODY_WAIT: Duration = Duration::from_secs(2);

/// The longest URL a failure shows.
const SHOWN_URL: usize = 200;

/// Checks `check`'s arguments and compiles its conditions.
pub(super) fn filter(check: &RequestCheck) -> Result<RequestFilter, ErrorInfo> {
    let bad = |message: String, example: &str| {
        ErrorInfo::new(ErrorKind::BadArgs, message).hint(Hint::run(
            Cmd::new("expect")
                .raw(if check.none { "no-request" } else { "request" })
                .raw(example),
        ))
    };
    if check.none && (check.count.is_some() || check.min.is_some() || check.max.is_some()) {
        return Err(bad(
            "expect no-request takes no --count, --min or --max".into(),
            "DELETE /api/cart",
        ));
    }
    if check.count.is_some() && (check.min.is_some() || check.max.is_some()) {
        return Err(bad(
            "--count does not go with --min or --max".into(),
            "POST /api/orders --count 2",
        ));
    }
    if let (Some(min), Some(max)) = (check.min, check.max)
        && min > max
    {
        return Err(bad(
            format!("--min {min} is more than --max {max}"),
            "GET /api/items --min 1 --max 3",
        ));
    }
    RequestFilter::new(
        &check.method,
        &check.target,
        &check.query,
        &check.headers,
        check.json.clone(),
    )
    .map_err(|why| bad(format!("expect request: {why}"), "POST /api/orders"))
}

/// What a check makes of how many requests match.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Pass,
    /// Too many: more requests cannot make it hold.
    Fail,
    /// Not enough yet.
    Wait,
}

fn decide(check: &RequestCheck, matched: u64) -> Decision {
    if check.none {
        return if matched == 0 {
            Decision::Pass
        } else {
            Decision::Fail
        };
    }
    if check.max.is_some_and(|max| matched > max) || check.count.is_some_and(|n| matched > n) {
        return Decision::Fail;
    }
    let min = check
        .count
        .or(check.min)
        .unwrap_or(u64::from(check.max.is_none()));
    if matched >= min {
        Decision::Pass
    } else {
        Decision::Wait
    }
}

/// `2 requests`, `1 request`.
fn requests(n: u64) -> String {
    if n == 1 {
        "1 request".into()
    } else {
        format!("{n} requests")
    }
}

/// What `check` expects, in words: `2 requests POST /api/orders with
/// json {"qty":2}`.
pub(super) fn expected_requests(check: &RequestCheck) -> String {
    let how_many = if check.none {
        "no request".to_owned()
    } else {
        match (check.count, check.min, check.max) {
            (Some(n), _, _) => requests(n),
            (None, Some(lo), Some(hi)) => format!("{lo} to {}", requests(hi)),
            (None, Some(lo), None) => format!("at least {}", requests(lo)),
            (None, None, Some(hi)) => format!("at most {}", requests(hi)),
            (None, None, None) => "a request".into(),
        }
    };
    let method = if check.method.trim() == "*" {
        "(any method)".to_owned()
    } else {
        check.method.trim().to_ascii_uppercase()
    };
    let mut text = format!("{how_many} {method} {}", check.target);
    let mut with = Vec::new();
    for (name, value) in &check.query {
        with.push(format!("query {name}={value}"));
    }
    for (name, value) in &check.headers {
        with.push(format!("header {name}: {value}"));
    }
    if let Some(json) = &check.json {
        with.push(format!("json {}", mock::text::compact(json)));
    }
    if !with.is_empty() {
        let _ = write!(text, " with {}", with.join(", "));
    }
    text
}

/// `check` as typed after `expect` (for messages and hints).
pub(super) fn request_command(cmd: Cmd, check: &RequestCheck) -> Cmd {
    let mut cmd = cmd
        .raw(if check.none { "no-request" } else { "request" })
        .arg(&check.method)
        .arg(&check.target);
    for (name, value) in &check.query {
        cmd = cmd.flag("query", format!("{name}={value}"));
    }
    for (name, value) in &check.headers {
        cmd = cmd.flag("header", format!("{name}: {value}"));
    }
    if let Some(json) = &check.json {
        cmd = cmd.flag("json-body", json.to_string());
    }
    for (flag, value) in [
        ("count", check.count),
        ("min", check.min),
        ("max", check.max),
    ] {
        if let Some(value) = value {
            cmd = cmd.flag(flag, value.to_string());
        }
    }
    if let Some(since) = &check.since {
        cmd = cmd.flag("since", since);
    }
    cmd
}

/// A request a page sent, as the check saw it.
#[derive(Debug)]
struct Sent {
    tab: u32,
    /// Its id in the tab's network log.
    id: u64,
    method: String,
    url: String,
    status: Option<u16>,
    verdict: Verdict,
}

impl Sent {
    /// `t1 #12 POST http://localhost/api/orders → 201`, and why it does
    /// not match.
    fn line(&self) -> String {
        let url: String = self.url.chars().take(SHOWN_URL).collect();
        let more = if url.len() < self.url.len() {
            "…"
        } else {
            ""
        };
        let mut line = format!(
            "{} #{} {} {url}{more}",
            tab_name(self.tab),
            self.id,
            self.method
        );
        match self.status {
            Some(status) => {
                let _ = write!(line, " → {status}");
            }
            None => line.push_str(" → no response"),
        }
        if let Some(reason) = &self.verdict.reason {
            let _ = write!(line, ": {reason}");
        }
        line
    }

    fn json(&self) -> Value {
        json!({
            "tab": tab_name(self.tab),
            "id": self.id,
            "method": self.method,
            "url": self.url,
            "status": self.status,
            "reason": self.verdict.reason,
        })
    }
}

/// Where the requests to look at start.
#[derive(Debug, Clone, Copy)]
struct Since {
    /// The mark's tab, and the last request of its log then.
    tab: u32,
    after: u64,
    /// When the mark was set: other tabs' requests from then on count.
    time_ms: u64,
}

/// A request whose body is not in the log, and is needed.
struct Pending {
    tab: u32,
    record: Record,
}

impl Engine {
    /// `expect request` and `expect no-request`.
    pub(super) async fn expect_request(
        &self,
        req: &ExpectRequest,
        check: &RequestCheck,
    ) -> Result<Output, ErrorInfo> {
        let filter = filter(check)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let start = Instant::now();
        let deadline = start + timeout;
        let scope = self.context_key()?;
        let only = match &req.tab {
            Some(tab) => Some(self.page(&running, Some(tab), timeout).await?.id),
            None => None,
        };
        let since = match &check.since {
            Some(name) => {
                let page = self.page(&running, req.tab.as_deref(), timeout).await?;
                let marks = running
                    .tabs
                    .named_mark(page.id, name)
                    .map_err(|names| self.no_mark(page.id, name, &names))?;
                Some(Since {
                    tab: page.id,
                    after: marks.netlog,
                    time_ms: marks.time_ms,
                })
            }
            None => None,
        };
        let mut bodies: HashMap<(String, String), Option<Vec<u8>>> = HashMap::new();
        let mut tries: u64 = 0;
        loop {
            let tabs = match only {
                Some(id) => vec![id],
                None => running.tabs.tab_ids_in(scope.as_deref()),
            };
            let sent = self
                .sent(&running, &filter, &tabs, since, &mut bodies)
                .await;
            let matched = sent.iter().filter(|s| s.verdict.matches()).count() as u64;
            tries += 1;
            let decision = decide(check, matched);
            if decision == Decision::Pass {
                let elapsed = start.elapsed();
                let assertion = assertion_text(req);
                let text = format!(
                    "passed: {assertion}: {} ({})",
                    requests(matched),
                    format::duration(elapsed)
                );
                let data = ExpectData {
                    tab: req.tab.clone().unwrap_or_default(),
                    assertion,
                    expected: expected(req, None),
                    actual: json!(matched),
                    count: Some(matched),
                    elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                    retries: tries - 1,
                };
                return Ok(Output::typed(&data, text)
                    .quiet(matched.to_string())
                    .limit(req.max_chars, "--max-chars 0 for all of it"));
            }
            if decision == Decision::Fail || Instant::now() >= deadline {
                let waited = (decision == Decision::Wait).then_some((timeout, tries - 1));
                return Err(self.request_failure(req, check, &sent, matched, waited, only));
            }
            tokio::time::sleep(POLL.min(deadline.saturating_duration_since(Instant::now()))).await;
        }
    }

    /// The requests `tabs` sent (since `since`), each checked.
    async fn sent(
        &self,
        running: &Running,
        filter: &RequestFilter,
        tabs: &[u32],
        since: Option<Since>,
        bodies: &mut HashMap<(String, String), Option<Vec<u8>>>,
    ) -> Vec<Sent> {
        let mut sent = Vec::new();
        let mut pending = Vec::new();
        // A redirect's hops share the request id: one request. So does a
        // shared or service worker's request in each tab of its origin.
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for &tab in tabs {
            running.tabs.netlog(tab, |log| {
                for record in log.records() {
                    if !seen.insert((record.session.clone(), record.request_id.clone())) {
                        continue;
                    }
                    let after = since.is_none_or(|s| {
                        if s.tab == tab {
                            record.entry.id > s.after
                        } else {
                            record.entry.time_ms >= s.time_ms
                        }
                    });
                    if !after {
                        continue;
                    }
                    let headers = headers(record);
                    let key = (record.session.clone(), record.request_id.clone());
                    let body: Option<Vec<u8>> = match (&record.post_data, bodies.get(&key)) {
                        (Some(data), _) => Some(data.clone().into_bytes()),
                        (None, Some(fetched)) => fetched.clone(),
                        (None, None) => None,
                    };
                    let request = mock::Request {
                        method: &record.entry.method,
                        url: &record.entry.url,
                        headers: &headers,
                        body: body.as_deref(),
                    };
                    let unread = record.post_data.is_none()
                        && record.has_post_data
                        && !bodies.contains_key(&key);
                    if unread && filter.needs_body() && filter.matches_without_body(&request) {
                        pending.push(Pending {
                            tab,
                            record: record.clone(),
                        });
                        continue;
                    }
                    sent.push(Sent {
                        tab,
                        id: record.entry.id,
                        method: record.entry.method.clone(),
                        url: record.entry.url.clone(),
                        status: record.entry.status,
                        verdict: filter.check(&request),
                    });
                }
            });
        }
        for Pending { tab, record } in pending {
            let body = self.request_body(running, &record, BODY_WAIT).await;
            let bytes = body.text.map(String::into_bytes);
            let headers = headers(&record);
            let verdict = filter.check(&mock::Request {
                method: &record.entry.method,
                url: &record.entry.url,
                headers: &headers,
                body: bytes.as_deref(),
            });
            bodies.insert((record.session.clone(), record.request_id.clone()), bytes);
            sent.push(Sent {
                tab,
                id: record.entry.id,
                method: record.entry.method.clone(),
                url: record.entry.url.clone(),
                status: record.entry.status,
                verdict,
            });
        }
        sent.sort_by_key(|s| (s.tab, s.id));
        sent
    }

    /// The failure of `expect request`: how many matched, the matching
    /// requests and the closest others. `waited` is the timeout and the
    /// retries when it gave up waiting for more.
    fn request_failure(
        &self,
        req: &ExpectRequest,
        check: &RequestCheck,
        sent: &[Sent],
        matched: u64,
        waited: Option<(Duration, u64)>,
        only: Option<u32>,
    ) -> ErrorInfo {
        let expected = expected(req, None);
        let mut actual = requests(matched);
        if let Some((timeout, retries)) = waited {
            let _ = write!(
                actual,
                " (after {}, {retries} {})",
                format::duration(timeout),
                if retries == 1 { "retry" } else { "retries" }
            );
        }
        let mut info = ErrorInfo::new(
            ErrorKind::ExpectFailed,
            format!("expect failed: {}", assertion_text(req)),
        )
        .note("expected", &expected)
        .note("actual", &actual);
        let matching: Vec<&Sent> = sent.iter().filter(|s| s.verdict.matches()).collect();
        let mut similar: Vec<&Sent> = sent
            .iter()
            .filter(|s| !s.verdict.matches() && s.verdict.is_close())
            .collect();
        similar.sort_by_key(|s| std::cmp::Reverse(s.verdict.score));
        for s in matching.iter().take(LISTED) {
            info = info.note("matching", s.line());
        }
        if matching.len() > LISTED {
            info = info.note(
                "matching",
                format!("… and {} more", matching.len() - LISTED),
            );
        }
        if !check.none {
            for s in similar.iter().take(LISTED) {
                info = info.note("similar", s.line());
            }
            if similar.is_empty() {
                info = info.note(
                    "similar",
                    format!(
                        "none: no request sent has a path like {} ({} in all)",
                        check.target,
                        requests(sent.len() as u64)
                    ),
                );
            }
        }
        let shown = matching.first().or(similar.first()).copied();
        let hint = match shown {
            Some(s) => {
                let mut cmd = Cmd::new("network").raw("show").arg(s.id.to_string());
                if only.is_some() || self.running().is_ok_and(|r| r.tabs.active() != Some(s.tab)) {
                    cmd = cmd.flag("tab", tab_name(s.tab));
                }
                Hint::run(cmd.scoped(&self.session)).why(if check.none {
                    "shows the request in full"
                } else if matching.is_empty() {
                    "shows the closest request in full, body and headers"
                } else {
                    "shows a matching request in full"
                })
            }
            None => {
                Hint::run(Cmd::new("network").scoped(&self.session)).why("lists what the page sent")
            }
        };
        let mut details = json!({
            "assertion": assertion_text(req),
            "expected": expected,
            "actual": matched,
            "matching": matching.iter().map(|s| s.json()).collect::<Vec<_>>(),
            "similar": similar.iter().take(LISTED).map(|s| s.json()).collect::<Vec<_>>(),
        });
        if let Some((timeout, retries)) = waited {
            details["timeout_ms"] = json!(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX));
            details["retries"] = json!(retries);
        }
        info.hint(hint).details(details)
    }
}

/// A record's request headers, as mocks take them.
fn headers(record: &Record) -> Vec<(String, String)> {
    record
        .request_headers
        .iter()
        .map(|h| (h.name.clone(), h.value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Decision, decide, expected_requests, filter};
    use crate::request::RequestCheck;

    fn check(method: &str, target: &str) -> RequestCheck {
        RequestCheck {
            method: method.into(),
            target: target.into(),
            query: Vec::new(),
            headers: Vec::new(),
            json: None,
            count: None,
            min: None,
            max: None,
            since: None,
            none: false,
        }
    }

    #[test]
    fn how_many_decides() {
        let one = check("POST", "/api/orders");
        assert_eq!(decide(&one, 0), Decision::Wait);
        assert_eq!(decide(&one, 3), Decision::Pass);
        let two = RequestCheck {
            count: Some(2),
            ..one.clone()
        };
        assert_eq!(decide(&two, 1), Decision::Wait);
        assert_eq!(decide(&two, 2), Decision::Pass);
        assert_eq!(decide(&two, 3), Decision::Fail);
        let at_most = RequestCheck {
            max: Some(1),
            ..one.clone()
        };
        assert_eq!(decide(&at_most, 0), Decision::Pass);
        assert_eq!(decide(&at_most, 2), Decision::Fail);
        let none = RequestCheck {
            none: true,
            ..one.clone()
        };
        assert_eq!(decide(&none, 0), Decision::Pass);
        assert_eq!(decide(&none, 1), Decision::Fail);
    }

    #[test]
    fn expected_in_words() {
        let mut c = check("post", "/api/orders");
        assert_eq!(expected_requests(&c), "a request POST /api/orders");
        c.count = Some(2);
        c.json = Some(json!({"qty": 2}));
        c.query = vec![("source".into(), "web".into())];
        assert_eq!(
            expected_requests(&c),
            r#"2 requests POST /api/orders with query source=web, json {"qty":2}"#
        );
        let mut none = check("*", "/api/cart/**");
        none.none = true;
        assert_eq!(
            expected_requests(&none),
            "no request (any method) /api/cart/**"
        );
    }

    #[test]
    fn bad_arguments() {
        let mut c = check("GET", "api/items");
        assert!(filter(&c).is_err());
        c.target = "/api/items".into();
        c.min = Some(3);
        c.max = Some(1);
        assert!(filter(&c).is_err());
        c.min = None;
        c.none = true;
        assert!(filter(&c).is_err());
    }
}
