//! The wait engine (plan 03, milestone 3.4): `wait` and the settle that
//! navigations run after the load event.
//!
//! One primitive, [`Engine::wait_until`], waits for a [`Condition`]. It is
//! event-driven where Chrome reports what matters: network idle and
//! `--request`/`--response` come from each tab's [`crate::network`]
//! tracker, the URL from its frame tree (both through `Tabs::watch`).
//! What only the page knows (elements, text, `--fn`, the load state) is a
//! probe that polls in the page on every animation frame (see [`probe`]).
//! The settle is network idle plus a quiet DOM; plan 05's actions reuse
//! [`Engine::settle`] after they trigger a navigation.
//!
//! A timeout (exit 3) says what was observed: the element count, the
//! requests in flight, the URL, the last value, and the page's URL and
//! title.

mod probe;

use std::time::Duration;

use protocol::schema::{DownloadInfo, RequestInfo, WaitData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use crate::downloads;
use crate::locator::{LocateOptions, Locator, parse_locator};
use crate::navigate::describe;
use crate::network::{Exchange, IDLE_QUIET, InFlight};
use crate::pattern::UrlPattern;
use crate::request::{ElementState, WaitFor, WaitRequest, millis};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

/// How long the DOM must go without changes to count as quiet.
const DOM_QUIET: Duration = Duration::from_millis(300);

/// The longest the settle waits for a quiet DOM; a page that keeps
/// changing (an animation, a clock) counts as settled after that.
const DOM_QUIET_MAX: Duration = Duration::from_secs(3);

/// How long a navigation's settle may take after the load event. Past it
/// the navigation still succeeds, with a warning.
pub(crate) const NAVIGATION_SETTLE: Duration = Duration::from_secs(5);

/// How long the network and the DOM must stay quiet after an action
/// that did not navigate (see [`Engine::action_settle`]).
const ACTION_QUIET: Duration = Duration::from_millis(150);

/// How many in-flight requests an error or warning lists.
const LISTED_REQUESTS: usize = 3;

/// How long describing the page may take in a timeout error.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(2);

/// What `wait` waits for, checked and compiled.
#[derive(Debug)]
pub(crate) enum Condition {
    /// The main document's load event, or `DOMContentLoaded`.
    Load {
        dom_content_loaded: bool,
    },
    Idle,
    Settle,
    Element {
        locator: Locator,
        state: ElementState,
    },
    Text {
        text: String,
        within: Option<Locator>,
        gone: bool,
    },
    Url(UrlPattern),
    Function(String),
    Exchange {
        pattern: UrlPattern,
        response: bool,
        status: Option<u16>,
    },
    Delay(Duration),
    Download,
}

/// Checks a condition's arguments (locators, URL patterns, empty text)
/// without a browser, so the CLI can fail before it starts one.
pub fn check_condition(condition: &WaitFor) -> Result<(), ErrorInfo> {
    Condition::parse(condition).map(drop)
}

impl Condition {
    /// Reads `testid=` locators from `attribute`.
    fn with_testid_attribute(self, attribute: Option<&str>) -> Self {
        match self {
            Self::Element { locator, state } => Self::Element {
                locator: locator.with_testid_attribute(attribute),
                state,
            },
            Self::Text { text, within, gone } => Self::Text {
                text,
                within: within.map(|l| l.with_testid_attribute(attribute)),
                gone,
            },
            other => other,
        }
    }

    pub(crate) fn parse(req: &WaitFor) -> Result<Self, ErrorInfo> {
        let not_empty = |value: &str, what: &str, example: &str| {
            if value.trim().is_empty() {
                Err(
                    ErrorInfo::new(ErrorKind::BadArgs, format!("{what} is empty"))
                        .hint(Hint::run(Cmd::new("wait").raw(example))),
                )
            } else {
                Ok(())
            }
        };
        Ok(match req {
            WaitFor::Load => Self::Load {
                dom_content_loaded: false,
            },
            WaitFor::DomContentLoaded => Self::Load {
                dom_content_loaded: true,
            },
            WaitFor::Idle => Self::Idle,
            WaitFor::Settle => Self::Settle,
            WaitFor::Element { locator, state } => Self::Element {
                locator: parse_locator(locator)?,
                state: *state,
            },
            WaitFor::Text { text, within, gone } => {
                let flag = if *gone { "--text-gone" } else { "--text" };
                not_empty(
                    text,
                    &format!("the {flag} text"),
                    &format!("{flag} Welcome"),
                )?;
                Self::Text {
                    text: text.clone(),
                    within: within.as_deref().map(parse_locator).transpose()?,
                    gone: *gone,
                }
            }
            WaitFor::Url { pattern } => Self::Url(UrlPattern::parse(pattern)?),
            WaitFor::Function { expression } => {
                not_empty(expression, "the --fn expression", "--fn 'window.appReady'")?;
                Self::Function(expression.clone())
            }
            WaitFor::Request { pattern } => Self::Exchange {
                pattern: UrlPattern::parse(pattern)?,
                response: false,
                status: None,
            },
            WaitFor::Response { pattern, status } => Self::Exchange {
                pattern: UrlPattern::parse(pattern)?,
                response: true,
                status: *status,
            },
            WaitFor::Delay { ms } => Self::Delay(Duration::from_millis(*ms)),
            WaitFor::Download => Self::Download,
        })
    }

    /// Whether it needs the page's scripts to run (so an open dialog or a
    /// crashed page fails it at once).
    fn runs_script(&self) -> bool {
        matches!(
            self,
            Self::Load { .. }
                | Self::Settle
                | Self::Element { .. }
                | Self::Text { .. }
                | Self::Function(_)
        )
    }

    /// `WaitData::condition`.
    fn name(&self) -> &'static str {
        match self {
            Self::Load {
                dom_content_loaded: false,
            } => "load",
            Self::Load {
                dom_content_loaded: true,
            } => "domcontentloaded",
            Self::Idle => "idle",
            Self::Settle => "settle",
            Self::Element { .. } => "element",
            Self::Text { gone: false, .. } => "text",
            Self::Text { gone: true, .. } => "text_gone",
            Self::Url(_) => "url",
            Self::Function(_) => "fn",
            Self::Exchange {
                response: false, ..
            } => "request",
            Self::Exchange { response: true, .. } => "response",
            Self::Delay(_) => "delay",
            Self::Download => "download",
        }
    }

    /// What is waited for, after "waited 10s for".
    fn describe(&self) -> String {
        match self {
            Self::Load {
                dom_content_loaded: false,
            } => "the load event".into(),
            Self::Load {
                dom_content_loaded: true,
            } => "DOMContentLoaded".into(),
            Self::Idle => "network idle".into(),
            Self::Settle => "the page to settle".into(),
            Self::Element { locator, state } => format!("{locator} to be {}", state.as_str()),
            Self::Text { text, within, gone } => {
                let scope = within
                    .as_ref()
                    .map(|w| format!(" in {w}"))
                    .unwrap_or_default();
                if *gone {
                    format!("text {text:?}{scope} to go away")
                } else {
                    format!("text {text:?}{scope}")
                }
            }
            Self::Url(pattern) => format!("a URL matching {:?}", pattern.to_string()),
            Self::Function(_) => "--fn to be truthy".into(),
            Self::Exchange {
                pattern,
                response: false,
                ..
            } => format!("a request to {:?}", pattern.to_string()),
            Self::Exchange {
                pattern,
                response: true,
                status,
            } => {
                let status = status
                    .map(|s| format!(" with status {s}"))
                    .unwrap_or_default();
                format!("a response from {:?}{status}", pattern.to_string())
            }
            Self::Delay(delay) => format::duration(*delay),
            Self::Download => "a download to finish".into(),
        }
    }
}

/// A condition that held.
#[derive(Debug, Default)]
pub(crate) struct Met {
    /// What held, in words: `text "Welcome" is visible`.
    pub(crate) text: String,
    pub(crate) count: Option<u64>,
    pub(crate) value: Option<Value>,
    pub(crate) exchange: Option<Exchange>,
    pub(crate) download: Option<DownloadInfo>,
    /// `e6 re-resolved by role+name` when a ref's element was found again.
    pub(crate) note: Option<String>,
}

impl Met {
    fn says(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

/// What a wait observed when its time ran out.
#[derive(Debug, Default)]
pub(crate) struct Unmet {
    /// Labelled lines for the error text.
    pub(crate) notes: Vec<(&'static str, String)>,
    /// The same for `details.observed`.
    pub(crate) observed: Map<String, Value>,
}

impl Unmet {
    fn note(mut self, label: &'static str, text: impl Into<String>) -> Self {
        self.notes.push((label, text.into()));
        self
    }

    fn observed(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.observed.insert(key.to_owned(), value.into());
        self
    }
}

/// The requests that kept the network busy, as notes.
pub(crate) fn busy_unmet(busy: &[InFlight]) -> Unmet {
    let now = Instant::now();
    let listed: Vec<Value> = busy
        .iter()
        .map(|r| {
            json!({
                "method": r.method,
                "url": r.url,
                "type": r.kind,
                "elapsed_ms": u64::try_from(now.duration_since(r.started).as_millis()).unwrap_or(u64::MAX),
            })
        })
        .collect();
    let unmet = Unmet::default().observed("in_flight", listed);
    if busy.is_empty() {
        return unmet.note(
            "network",
            format!(
                "requests kept starting; it was never quiet for {}",
                format::duration(IDLE_QUIET)
            ),
        );
    }
    let mut unmet = busy
        .iter()
        .take(LISTED_REQUESTS)
        .fold(unmet, |unmet, r| unmet.note("in flight", in_flight(r, now)));
    if busy.len() > LISTED_REQUESTS {
        unmet = unmet.note(
            "in flight",
            format!("… and {} more", busy.len() - LISTED_REQUESTS),
        );
    }
    unmet
}

/// `GET https://x/api/slow (4.1s)`.
fn in_flight(request: &InFlight, now: Instant) -> String {
    format!(
        "{} {} ({})",
        request.method,
        request.url,
        format::duration(now.duration_since(request.started))
    )
}

/// How the settle ended.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Settled {
    /// Whether the DOM went quiet too (else it kept changing for
    /// [`DOM_QUIET_MAX`]).
    pub(crate) dom_quiet: bool,
}

impl Engine {
    /// `wait`: one condition in the active tab (or `--tab`).
    pub async fn wait(&self, req: WaitRequest) -> Result<Output, ErrorInfo> {
        let condition = Condition::parse(&req.condition)?
            .with_testid_attribute(req.testid_attribute.as_deref());
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        if condition.runs_script() {
            self.check_not_blocked(&running, &page)?;
        }
        let mark = running.tabs.mark();
        let start = Instant::now();
        let met = {
            let _tab = self.tabs.lock(&page.target_id).await;
            let wait = self.wait_until(&running, &page, &condition, start, timeout, &req);
            self.unless_blocked(&running, &page, mark, wait).await?
        };
        let elapsed = start.elapsed();
        let url = running
            .tabs
            .frames(page.id)
            .into_iter()
            .next()
            .map(|f| f.url)
            .unwrap_or_default();
        let request = met.exchange.as_ref().map(|e| RequestInfo {
            method: e.method.clone(),
            url: e.url.clone(),
            status: e.status,
        });
        let quiet = match (&condition, &met) {
            (Condition::Url(_), _) => url.clone(),
            (
                Condition::Download,
                Met {
                    download: Some(d), ..
                },
            ) => d.path.clone().unwrap_or_default(),
            (
                Condition::Exchange { .. },
                Met {
                    exchange: Some(e), ..
                },
            ) => e.url.clone(),
            (_, Met { value: Some(v), .. }) => v.to_string(),
            (_, Met { count: Some(n), .. }) => n.to_string(),
            _ => String::new(),
        };
        let text = if matches!(condition, Condition::Delay(_)) {
            met.text.clone()
        } else {
            format!("{} ({})", met.text, format::duration(elapsed))
        };
        let data = WaitData {
            tab: tab_name(page.id),
            condition: condition.name().to_owned(),
            met: met.text,
            url,
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            count: met.count,
            value: met.value,
            request,
            download: met.download.clone(),
        };
        let mut output = Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it");
        if let Some(note) = met.note {
            output = output.warning(note);
        }
        if matches!(condition, Condition::Delay(_)) {
            output = output.warning(
                "a fixed wait is slow and flaky; wait for what you need instead: \
                 chrome-cdp wait <locator>, --text, --url, --idle or --fn",
            );
        }
        Ok(self
            .report_events(&running, mark, output, met.download.as_ref())
            .await)
    }

    /// Waits up to `timeout` for `condition` in `page`: the one primitive
    /// every wait uses. `since` is when the network's quiet period may
    /// start (idle and settle). `req` is the command, for the hint.
    pub(crate) async fn wait_until(
        &self,
        running: &Running,
        page: &Page,
        condition: &Condition,
        since: Instant,
        timeout: Duration,
        req: &WaitRequest,
    ) -> Result<Met, ErrorInfo> {
        let deadline = Instant::now() + timeout;
        let outcome = match condition {
            Condition::Delay(delay) => {
                tokio::time::sleep(*delay).await;
                Ok(Met::says(format!("waited {}", format::duration(*delay))))
            }
            Condition::Idle => running
                .tabs
                .network_idle(page.id, since, deadline)
                .await
                .map(|()| Met::says("network idle"))
                .map_err(|busy| busy_unmet(&busy)),
            Condition::Settle => {
                self.settle(running, page, since, deadline)
                    .await?
                    .map(|settled| {
                        Met::says(if settled.dom_quiet {
                            "page settled: network idle, DOM quiet"
                        } else {
                            "page settled: network idle; the DOM kept changing"
                        })
                    })
            }
            Condition::Url(pattern) => running
                .tabs
                .main_url(page.id, |url| pattern.matches(url), deadline)
                .await
                .map(|url| Met::says(format!("URL {url} matches {:?}", pattern.to_string())))
                .map_err(|url| {
                    Unmet::default()
                        .note("url", url.clone())
                        .observed("url", url)
                }),
            Condition::Exchange {
                pattern,
                response,
                status,
            } => {
                let after = running.tabs.previous_command_mark(page.id);
                let wanted = |e: &Exchange| {
                    pattern.matches(&e.url) && status.is_none_or(|s| e.status == Some(s))
                };
                let found = running
                    .tabs
                    .exchange(page.id, *response, after, wanted, deadline)
                    .await;
                if let Some(found) = found {
                    Ok(Met {
                        text: exchange_text(&found, *response),
                        exchange: Some(found),
                        ..Met::default()
                    })
                } else {
                    // A response with another status is worth naming.
                    let other = running
                        .tabs
                        .exchange(
                            page.id,
                            *response,
                            after,
                            |e| pattern.matches(&e.url),
                            Instant::now(),
                        )
                        .await;
                    Err(match other {
                        Some(other) => Unmet::default()
                            .note("got", exchange_text(&other, true))
                            .observed("status", other.status),
                        None => Unmet::default().note(
                            "observed",
                            if *response {
                                "no matching response"
                            } else {
                                "no matching request"
                            },
                        ),
                    })
                }
            }
            Condition::Download => {
                let after = running.tabs.previous_event_mark(page.id);
                match running.tabs.download_done(after, None, deadline).await {
                    Ok(download) => Ok(Met {
                        text: downloads::line(&download, &self.session),
                        download: Some(download),
                        ..Met::default()
                    }),
                    Err(Some(partial)) => Err(Unmet::default()
                        .note("downloading", downloads::line(&partial, &self.session))
                        .observed("download", json!(partial))),
                    Err(None) => Err(Unmet::default()
                        .note("observed", "no download started")
                        .observed("download", Value::Null)),
                }
            }
            Condition::Load { .. }
            | Condition::Element { .. }
            | Condition::Text { .. }
            | Condition::Function(_) => {
                let options = LocateOptions {
                    strict_refs: req.strict_refs,
                    testid_attribute: req.testid_attribute.clone(),
                };
                self.probe_until(running, page, condition, &options, deadline)
                    .await?
            }
        };
        match outcome {
            Ok(met) => Ok(met),
            Err(unmet) => Err(self.wait_error(page, condition, timeout, unmet, req).await),
        }
    }

    /// Waits until `page`'s network is idle since `since` and its DOM is
    /// quiet (at most [`DOM_QUIET_MAX`] for that part), up to `deadline`.
    /// `Err` holds what kept the network busy.
    pub(crate) async fn settle(
        &self,
        running: &Running,
        page: &Page,
        since: Instant,
        deadline: Instant,
    ) -> Result<Result<Settled, Unmet>, ErrorInfo> {
        loop {
            if let Err(busy) = running.tabs.network_idle(page.id, since, deadline).await {
                return Ok(Err(busy_unmet(&busy)));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let dom_quiet = self
                .dom_quiet(running, page, DOM_QUIET, DOM_QUIET_MAX.min(left))
                .await?;
            // A request the DOM change started: wait for it too.
            let now = Instant::now();
            match running.tabs.network_idle(page.id, since, now).await {
                Ok(()) => return Ok(Ok(Settled { dom_quiet })),
                Err(busy) if now >= deadline => return Ok(Err(busy_unmet(&busy))),
                Err(_) => {}
            }
        }
    }

    /// The brief settle after an action that did not navigate (plan 05,
    /// 5.5): waits until the requests `page` started since `since` are
    /// done and its DOM went [`ACTION_QUIET`] without a change, up to
    /// `deadline`; `false` if it did not. Shorter than [`Engine::settle`]:
    /// most actions change nothing, and requests from before (a page's
    /// polling) are not the action's.
    pub(crate) async fn action_settle(
        &self,
        running: &Running,
        page: &Page,
        since: Instant,
        deadline: Instant,
    ) -> Result<bool, ErrorInfo> {
        loop {
            if !running
                .tabs
                .own_requests_done(page.id, since, ACTION_QUIET, deadline)
                .await
            {
                return Ok(false);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let dom_quiet = self.dom_quiet(running, page, ACTION_QUIET, left).await?;
            // A request the DOM change started: wait for it too.
            let done = running
                .tabs
                .own_requests_done(page.id, since, ACTION_QUIET, Instant::now())
                .await;
            if done {
                return Ok(dom_quiet);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
        }
    }

    /// The timeout error of a wait: what was observed and the page.
    async fn wait_error(
        &self,
        page: &Page,
        condition: &Condition,
        timeout: Duration,
        unmet: Unmet,
        req: &WaitRequest,
    ) -> ErrorInfo {
        let shown = describe(&page.session.with_timeout(DESCRIBE_TIMEOUT))
            .await
            .ok();
        let mut info = ErrorInfo::new(
            ErrorKind::Timeout,
            format!(
                "waited {} for {}",
                format::duration(timeout),
                condition.describe()
            ),
        );
        for (label, text) in &unmet.notes {
            info = info.note(*label, text.clone());
        }
        let mut details = json!({
            "condition": condition.name(),
            "timeout_ms": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            "observed": unmet.observed,
        });
        if let Some(shown) = shown {
            info = info.note("page", format!("{} {:?}", shown.url, shown.title));
            details["page"] = json!({ "url": shown.url, "title": shown.title });
        }
        let longer = format::duration(timeout * 2);
        let again = retry(&req.condition, req.tab.as_deref())
            .flag("timeout", &longer)
            .session(&self.session);
        let hint = match condition {
            Condition::Idle | Condition::Settle => {
                Hint::run(retry(&WaitFor::Load, req.tab.as_deref()).session(&self.session))
                    .why("does not wait for the requests still open")
                    .or(format!("a longer --timeout, e.g. --timeout {longer}"))
            }
            Condition::Text { gone: false, .. } => Hint::run(again)
                .why("the text must match exactly, spaces collapsed; only visible text counts"),
            Condition::Element {
                state: ElementState::Visible | ElementState::Attached | ElementState::Enabled,
                ..
            } if unmet.observed.get("count") == Some(&json!(0)) => {
                Hint::run(again).why("no element matches the locator yet; check it")
            }
            Condition::Exchange { .. } => Hint::run(again).why(
                "only requests since the previous command count; run the action that sends it first",
            ),
            Condition::Download if unmet.observed.get("download") == Some(&Value::Null) => {
                Hint::run(again).why(
                    "only downloads since the previous command count; run the action that starts it first",
                )
            }
            _ => Hint::run(again),
        };
        let info = self.note_unread_errors(page.id, info, &mut details);
        info.hint(hint).details(details)
    }
}

/// `response 200 GET https://x/api` or `request GET https://x/api`.
fn exchange_text(exchange: &Exchange, response: bool) -> String {
    let status = exchange.status.filter(|_| response).map(|s| s.to_string());
    let words = [
        Some(if response { "response" } else { "request" }),
        status.as_deref(),
        Some(exchange.method.as_str()).filter(|m| !m.is_empty()),
        Some(exchange.url.as_str()),
    ];
    words.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

/// The `wait` command that waits for `condition` again (in tab `tab`).
fn retry(condition: &WaitFor, tab: Option<&str>) -> Cmd {
    let cmd = Cmd::new("wait");
    let cmd = match condition {
        WaitFor::Load => cmd.switch("load"),
        WaitFor::DomContentLoaded => cmd.switch("domcontentloaded"),
        WaitFor::Idle => cmd.switch("idle"),
        WaitFor::Settle => cmd.switch("settle"),
        WaitFor::Element { locator, state } => {
            let cmd = cmd.arg(locator);
            if *state == ElementState::Visible {
                cmd
            } else {
                cmd.flag("state", state.as_str())
            }
        }
        WaitFor::Text { text, within, gone } => {
            let cmd = cmd.flag(if *gone { "text-gone" } else { "text" }, text);
            match within {
                Some(within) => cmd.flag("in", within),
                None => cmd,
            }
        }
        WaitFor::Url { pattern } => cmd.flag("url", pattern),
        WaitFor::Function { expression } => cmd.flag("fn", expression),
        WaitFor::Request { pattern } => cmd.flag("request", pattern),
        WaitFor::Response { pattern, status } => {
            let cmd = cmd.flag("response", pattern);
            match status {
                Some(status) => cmd.flag("status", status.to_string()),
                None => cmd,
            }
        }
        WaitFor::Delay { ms } => cmd.arg(ms.to_string()),
        WaitFor::Download => cmd.switch("download"),
    };
    match tab {
        Some(tab) => cmd.flag("tab", tab),
        None => cmd,
    }
}

#[cfg(test)]
mod tests {
    use protocol::ErrorKind;

    use super::{Condition, retry};
    use crate::request::{ElementState, WaitFor};

    #[test]
    fn conditions_are_checked_before_waiting() {
        let bad = [
            WaitFor::Element {
                locator: "role=".into(),
                state: ElementState::Visible,
            },
            WaitFor::Text {
                text: "  ".into(),
                within: None,
                gone: false,
            },
            WaitFor::Text {
                text: "Hi".into(),
                within: Some("main".into()),
                gone: false,
            },
            WaitFor::Url {
                pattern: "/(/".into(),
            },
            WaitFor::Function {
                expression: String::new(),
            },
        ];
        for condition in bad {
            let err = Condition::parse(&condition).unwrap_err();
            assert_eq!(err.exit_code(), 2, "{condition:?}");
        }
        let err = Condition::parse(&WaitFor::Function {
            expression: " ".into(),
        })
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs);
        assert_eq!(
            err.hint.as_deref(),
            Some("chrome-cdp wait --fn 'window.appReady'")
        );
    }

    #[test]
    fn descriptions() {
        let describe = |c: WaitFor| Condition::parse(&c).unwrap().describe();
        assert_eq!(
            describe(WaitFor::Text {
                text: "Welcome".into(),
                within: Some("#main".into()),
                gone: false,
            }),
            "text \"Welcome\" in #main"
        );
        assert_eq!(
            describe(WaitFor::Element {
                locator: "css=button".into(),
                state: ElementState::Enabled,
            }),
            "css=button to be enabled"
        );
        assert_eq!(
            describe(WaitFor::Response {
                pattern: "/api/save".into(),
                status: Some(200),
            }),
            "a response from \"/api/save\" with status 200"
        );
    }

    #[test]
    fn retry_commands() {
        let cmd = retry(
            &WaitFor::Text {
                text: "Sign in".into(),
                within: Some("#nav".into()),
                gone: true,
            },
            Some("t2"),
        );
        assert_eq!(
            cmd.to_string(),
            "chrome-cdp wait --text-gone 'Sign in' --in '#nav' --tab t2"
        );
        let cmd = retry(
            &WaitFor::Element {
                locator: ".item".into(),
                state: ElementState::Hidden,
            },
            None,
        );
        assert_eq!(cmd.to_string(), "chrome-cdp wait .item --state hidden");
    }
}
