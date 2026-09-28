//! Request routing (plan 07, milestone 7.1): `route`, `block`, `routes`
//! and `unroute`.
//!
//! The session's rules live in the daemon ([`Router`], in the tab
//! manager's settings, so they outlive a browser). While at least one
//! exists, every page and iframe session intercepts all requests with the
//! `Fetch` domain (see `tabs::fetch`); without any, `Fetch` is off (unless
//! `open --http-credentials` needs it) and requests are never paused.
//!
//! Each paused request goes through [`Router::decide`]: the rules are
//! tried in the order they were added, and the first that matches (URL
//! pattern, method, resource type, browser context) handles it; no match
//! lets it go on at once. Every request routing sees goes into a bounded
//! journal with the rule that handled it (`routes --log`, and plan 11's
//! request journals). A rule with `--times N` is removed after its N-th
//! match.
//!
//! After the rules come the session's mock sets (plan 11, milestone
//! 11.2; see the `mock` crate): a request no rule handles goes to
//! [`mock::Mocks::decide`], which answers it from a mock, handles it as
//! unmatched (it is in a set's scope), or lets it go on. The journal says
//! which mock answered each request.
//!
//! [`act`] carries a decision out on the request's session: fulfil, fail,
//! continue (changed), or continue and patch the JSON response.

mod act;
mod commands;
mod mocking;

pub(crate) use mocking::unknown_state;

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use cdp::domains::network::ErrorReason;
use mock::{Mocks, Outcome, Response};
use protocol::schema::{MockRef, RouteInfo, RoutedRequest};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, format};
use serde_json::Value;

pub(crate) use act::{PendingPatches, carry_out, continue_paused, patch_response};

use crate::observe::RESOURCE_TYPES;
use crate::pattern::UrlPattern;
use crate::request::{RouteAction, RouteBody, RouteSpec};
use crate::tabs::tab_name;

/// How many routed requests the journal keeps.
const JOURNAL: usize = 1000;

/// The largest `--body-file` a rule takes.
const MAX_BODY: u64 = mock::MAX_FILE;

/// Chrome's network errors, as `--abort` takes them (any case).
const ABORT_REASONS: &[&str] = mock::NETWORK_ERRORS;

/// The session's routing rules, mock sets and journal.
#[derive(Debug)]
pub(crate) struct Router {
    table: Mutex<Table>,
}

impl Default for Router {
    fn default() -> Self {
        Self {
            table: Mutex::new(Table {
                last_id: 0,
                rules: Vec::new(),
                mocks: Mocks::new(0),
                journal: VecDeque::new(),
                seq: 0,
            }),
        }
    }
}

#[derive(Debug)]
struct Table {
    /// The last rule id given.
    last_id: u32,
    /// In the order they are tried.
    rules: Vec<Rule>,
    /// The mock sets in use (session-wide; plan 11.4 adds per-context
    /// sets for parallel tests), tried after the rules.
    mocks: Mocks,
    /// The newest requests routing saw, oldest first.
    journal: VecDeque<RoutedRequest>,
    /// The last journal sequence number given.
    seq: u64,
}

/// A rule, checked and compiled.
#[derive(Debug)]
struct Rule {
    id: u32,
    pattern: UrlPattern,
    label: Option<String>,
    /// Upper case.
    methods: Vec<String>,
    /// Lower case, as `network --type`.
    resource_types: Vec<String>,
    context: Option<String>,
    /// Matches left before it is removed.
    left: Option<u32>,
    hits: u64,
    handler: Arc<Handler>,
}

/// What a rule does with a request it matched.
#[derive(Debug)]
pub(crate) struct Handler {
    /// How long to hold the request first.
    pub(crate) delay: Duration,
    pub(crate) act: Act,
}

#[derive(Debug)]
pub(crate) enum Act {
    Fulfill(Fulfill),
    Abort(ErrorReason),
    Continue {
        set_headers: Vec<(String, String)>,
        /// Lower case.
        remove_headers: Vec<String>,
        url: Option<String>,
    },
    PatchJson(Value),
}

/// A response made up by a rule.
#[derive(Debug)]
pub(crate) struct Fulfill {
    pub(crate) status: u16,
    /// With `Content-Type` (unless there is no body).
    pub(crate) headers: Vec<(String, String)>,
    /// The body in base64, as `Fetch.fulfillRequest` takes it.
    pub(crate) body: String,
    /// The body's size in bytes.
    pub(crate) size: usize,
    /// Whether a cross-origin request's answer is allowed by CORS headers
    /// (rules always; mocks unless their set says `cors = "off"`).
    pub(crate) cors: bool,
}

impl Fulfill {
    /// The value of header `name` (any case).
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A paused request, as routing sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Request<'a> {
    pub(crate) url: &'a str,
    pub(crate) method: &'a str,
    /// Chrome's resource type (`XHR`, `Document`, …).
    pub(crate) resource_type: &'a str,
    /// The request's headers (a JSON object).
    pub(crate) headers: &'a Value,
    /// The browser context of its target.
    pub(crate) context: Option<&'a str>,
    /// Its tab.
    pub(crate) tab: Option<u32>,
    /// Its body, for mocks that match on it.
    pub(crate) body: Option<&'a [u8]>,
    /// `{{now}}` of mock templates (ms since the epoch).
    pub(crate) now_ms: i64,
}

impl Request<'_> {
    /// The value of request header `name` (any case).
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        header(self.headers, name)
    }
}

/// The value of header `name` (any case) in a JSON object of headers.
pub(crate) fn header<'a>(headers: &'a Value, name: &str) -> Option<&'a str> {
    headers
        .as_object()?
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.as_str())
}

/// What to do with a paused request.
#[derive(Debug, Clone)]
pub(crate) enum Decision {
    /// No rule matched: let it go on unchanged.
    Pass,
    /// Rule `rule` handles it; `seq` is its journal entry.
    Route {
        rule: u32,
        handler: Arc<Handler>,
        seq: u64,
    },
    /// A CORS preflight for a request a rule or mock answers itself:
    /// allow it.
    Preflight,
    /// A mock answers it (or its set's `unmatched` policy does); `seq` is
    /// its journal entry.
    Mock { handler: Arc<Handler>, seq: u64 },
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Router {
    /// Whether any rule or mock set exists (and requests must be
    /// intercepted).
    pub(crate) fn is_active(&self) -> bool {
        let table = lock(&self.table);
        !table.rules.is_empty() || !table.mocks.is_empty()
    }

    /// Whether mock sets are in use (and paused requests need their
    /// bodies and the time).
    pub(crate) fn has_mocks(&self) -> bool {
        !lock(&self.table).mocks.is_empty()
    }

    /// How many rules exist.
    pub(crate) fn len(&self) -> usize {
        lock(&self.table).rules.len()
    }

    /// Adds `specs` after the existing rules, all or none: the first bad
    /// one is an error and nothing is added.
    pub(crate) fn add(&self, specs: &[RouteSpec]) -> Result<Vec<RouteInfo>, ErrorInfo> {
        let compiled: Vec<Rule> = specs.iter().map(compile).collect::<Result<_, _>>()?;
        let mut table = lock(&self.table);
        let mut added = Vec::with_capacity(compiled.len());
        for mut rule in compiled {
            table.last_id += 1;
            rule.id = table.last_id;
            added.push(rule.info());
            table.rules.push(rule);
        }
        Ok(added)
    }

    /// Removes rules `ids`; an id that is no rule is a `NotFound` and
    /// nothing is removed.
    pub(crate) fn remove(&self, ids: &[u32]) -> Result<Vec<u32>, u32> {
        let mut table = lock(&self.table);
        if let Some(&missing) = ids
            .iter()
            .find(|&&id| !table.rules.iter().any(|r| r.id == id))
        {
            return Err(missing);
        }
        table.rules.retain(|r| !ids.contains(&r.id));
        Ok(ids.to_vec())
    }

    /// Removes every rule; returns their ids.
    pub(crate) fn clear(&self) -> Vec<u32> {
        let mut table = lock(&self.table);
        table.rules.drain(..).map(|r| r.id).collect()
    }

    /// The rules, in the order they are tried.
    pub(crate) fn list(&self) -> Vec<RouteInfo> {
        lock(&self.table).rules.iter().map(Rule::info).collect()
    }

    /// The newest `limit` journal entries (0 = all), oldest first, and how
    /// many older ones were left out.
    pub(crate) fn journal(&self, limit: usize) -> (Vec<RoutedRequest>, u64) {
        let table = lock(&self.table);
        let skip = if limit == 0 {
            0
        } else {
            table.journal.len().saturating_sub(limit)
        };
        let entries = table.journal.iter().skip(skip).cloned().collect();
        (entries, skip as u64)
    }

    /// Picks what to do with `req`, counts the hit, and logs it. The
    /// `bool` is whether a rule ran out and none is left, so requests need
    /// not be intercepted any more.
    pub(crate) fn decide(&self, req: &Request<'_>) -> (Decision, bool) {
        let mut table = lock(&self.table);
        let preflight = req
            .method
            .eq_ignore_ascii_case("OPTIONS")
            .then(|| req.header("Access-Control-Request-Method"))
            .flatten();
        if let Some(method) = preflight {
            // A preflight for a request a rule fulfils never reaches a
            // server that could allow it, so allow it here.
            let fulfils = table
                .rules
                .iter()
                .find(|r| r.matches(req, method))
                .is_some_and(|r| matches!(r.handler.act, Act::Fulfill(_)));
            if fulfils {
                table.log(req, None, "preflight allowed".into());
                return (Decision::Preflight, false);
            }
        }
        let Some(index) = table.rules.iter().position(|r| r.matches(req, req.method)) else {
            return (table.mock(req), false);
        };
        let rule = &mut table.rules[index];
        rule.hits += 1;
        let id = rule.id;
        let handler = rule.handler.clone();
        let spent = match &mut rule.left {
            Some(left) => {
                *left = left.saturating_sub(1);
                *left == 0
            }
            None => false,
        };
        let outcome = handler.outcome();
        if spent {
            table.rules.remove(index);
        }
        let seq = table.log(req, Some(id), outcome);
        let emptied = spent && table.rules.is_empty();
        (
            Decision::Route {
                rule: id,
                handler,
                seq,
            },
            emptied,
        )
    }

    /// Adds to what the journal says happened to request `seq` (a patch
    /// that could not be applied).
    pub(crate) fn amend(&self, seq: u64, note: &str) {
        let mut table = lock(&self.table);
        if let Some(entry) = table.journal.iter_mut().rev().find(|e| e.seq == seq) {
            let _ = write!(entry.outcome, " ({note})");
        }
    }
}

impl Table {
    /// What the mock sets do with a request no rule handles.
    fn mock(&mut self, req: &Request<'_>) -> Decision {
        if self.mocks.is_empty() {
            self.log(req, None, "continue".into());
            return Decision::Pass;
        }
        self.mocks.refresh();
        let headers: Vec<(String, String)> = req
            .headers
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(n, v)| Some((n.clone(), v.as_str()?.to_owned())))
            .collect();
        let request = mock::Request {
            method: req.method,
            url: req.url,
            headers: &headers,
            body: req.body,
        };
        match self.mocks.decide(&request, req.now_ms) {
            Outcome::Pass => {
                self.log(req, None, "continue".into());
                Decision::Pass
            }
            Outcome::Preflight { mock } => {
                self.log_mock(req, mock, None, "preflight allowed".into());
                Decision::Preflight
            }
            Outcome::Answer(answer) => {
                let handler = mock_handler(answer.response, answer.delay);
                let seq = self.log_mock(req, Some(answer.mock), None, answer.outcome);
                Decision::Mock {
                    handler: Arc::new(handler),
                    seq,
                }
            }
            Outcome::Unmatched {
                set,
                response,
                outcome,
            } => {
                let seq = self.log_mock(req, None, Some(set), outcome);
                if matches!(response, Response::Passthrough { patch: None }) {
                    return Decision::Pass;
                }
                Decision::Mock {
                    handler: Arc::new(mock_handler(response, Duration::ZERO)),
                    seq,
                }
            }
        }
    }

    fn log(&mut self, req: &Request<'_>, rule: Option<u32>, outcome: String) -> u64 {
        let seq = self.log_mock(req, None, None, outcome);
        if let Some(entry) = self.journal.back_mut() {
            entry.route = rule.map(route_name);
        }
        seq
    }

    fn log_mock(
        &mut self,
        req: &Request<'_>,
        mock: Option<MockRef>,
        unmatched: Option<String>,
        outcome: String,
    ) -> u64 {
        self.seq += 1;
        if self.journal.len() == JOURNAL {
            self.journal.pop_front();
        }
        self.journal.push_back(RoutedRequest {
            seq: self.seq,
            time_ms: now_ms(),
            tab: req.tab.map(tab_name),
            method: req.method.to_owned(),
            url: req.url.to_owned(),
            resource_type: req.resource_type.to_ascii_lowercase(),
            route: None,
            mock,
            unmatched,
            outcome,
        });
        self.seq
    }
}

impl Rule {
    fn matches(&self, req: &Request<'_>, method: &str) -> bool {
        (self.methods.is_empty() || self.methods.iter().any(|m| m.eq_ignore_ascii_case(method)))
            && (self.resource_types.is_empty()
                || self
                    .resource_types
                    .iter()
                    .any(|t| same_type(t, req.resource_type)))
            && self
                .context
                .as_deref()
                .is_none_or(|c| req.context == Some(c))
            && self.pattern.matches(req.url)
    }

    fn info(&self) -> RouteInfo {
        let (action, status) = match &self.handler.act {
            Act::Fulfill(f) => ("fulfill", Some(f.status)),
            Act::Abort(_) => ("abort", None),
            Act::Continue { .. } => ("continue", None),
            Act::PatchJson(_) => ("patch_json", None),
        };
        RouteInfo {
            id: route_name(self.id),
            pattern: self.pattern.to_string(),
            label: self.label.clone(),
            methods: self.methods.clone(),
            resource_types: self.resource_types.clone(),
            action: action.into(),
            summary: self.handler.summary(),
            status,
            delay_ms: u64::try_from(self.handler.delay.as_millis()).unwrap_or(u64::MAX),
            hits: self.hits,
            left: self.left,
        }
    }
}

impl Handler {
    /// The action in a few words: `fulfill 200 application/json 17B`.
    fn summary(&self) -> String {
        let mut text = String::new();
        if !self.delay.is_zero() {
            let _ = write!(text, "delay {}, then ", format::duration(self.delay));
        }
        match &self.act {
            Act::Fulfill(f) => {
                let _ = write!(text, "fulfill {}", f.status);
                if let Some(kind) = f.header("Content-Type") {
                    let kind = kind.split(';').next().unwrap_or(kind).trim();
                    let _ = write!(text, " {kind}");
                }
                if f.size > 0 {
                    let _ = write!(text, " {}", format::size(f.size as u64));
                }
            }
            Act::Abort(reason) => {
                let _ = write!(text, "abort {}", reason_name(*reason).to_ascii_lowercase());
            }
            Act::Continue {
                set_headers,
                remove_headers,
                url,
            } => {
                text.push_str("continue");
                let mut changes = Vec::new();
                for (name, value) in set_headers {
                    changes.push(format!("{name}: {value}"));
                }
                for name in remove_headers {
                    changes.push(format!("no {name}"));
                }
                if let Some(url) = url {
                    changes.push(format!("to {url}"));
                }
                if !changes.is_empty() {
                    let _ = write!(text, " ({})", changes.join(", "));
                }
            }
            Act::PatchJson(patch) => {
                let patch = patch.to_string();
                let shown: String = patch.chars().take(60).collect();
                let more = if shown.len() < patch.len() { "…" } else { "" };
                let _ = write!(text, "patch json {shown}{more}");
            }
        }
        text
    }

    /// What the journal says a matched request got.
    fn outcome(&self) -> String {
        match &self.act {
            Act::Fulfill(f) => format!("fulfill {}", f.status),
            Act::Abort(reason) => format!("abort {}", reason_name(*reason).to_ascii_lowercase()),
            Act::Continue {
                set_headers,
                remove_headers,
                url,
            } => {
                if set_headers.is_empty() && remove_headers.is_empty() && url.is_none() {
                    "continue".into()
                } else {
                    "continue changed".into()
                }
            }
            Act::PatchJson(_) => "patch json".into(),
        }
    }
}

/// Whether a rule's resource type `rule` (lower case) is Chrome's `paused`
/// type. `Fetch` reports a `fetch()` as `XHR`, so `xhr` and `fetch` are
/// one type to rules.
fn same_type(rule: &str, paused: &str) -> bool {
    let ajax = |t: &str| t.eq_ignore_ascii_case("xhr") || t.eq_ignore_ascii_case("fetch");
    rule.eq_ignore_ascii_case(paused) || (ajax(rule) && ajax(paused))
}

/// `r3`.
pub(crate) fn route_name(id: u32) -> String {
    format!("r{id}")
}

/// Parses a rule id as typed: `r3` or `3`.
pub(crate) fn parse_route_id(text: &str) -> Result<u32, ErrorInfo> {
    let text = text.trim();
    let digits = text.strip_prefix('r').unwrap_or(text);
    digits
        .parse::<u32>()
        .ok()
        .filter(|&n| n > 0 && !digits.starts_with('+'))
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{text}` is not a route id; route ids look like r1, r2, …"),
            )
            .hint(Hint::run(Cmd::new("routes")).why("lists them"))
        })
}

fn bad(message: impl Into<String>) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadArgs, message)
}

/// Checks and compiles `spec` (its id is set when it is added).
fn compile(spec: &RouteSpec) -> Result<Rule, ErrorInfo> {
    let pattern = UrlPattern::parse(&spec.pattern)?;
    let methods = spec
        .methods
        .iter()
        .map(|m| {
            let m = m.trim();
            if m.is_empty() || !m.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(bad(format!("`{m}` is not an HTTP method"))
                    .hint(Hint::text("e.g. --method GET or --method POST")));
            }
            Ok(m.to_ascii_uppercase())
        })
        .collect::<Result<_, _>>()?;
    let resource_types = spec
        .resource_types
        .iter()
        .map(|t| {
            let t = t.trim().to_ascii_lowercase();
            if RESOURCE_TYPES.contains(&t.as_str()) {
                Ok(t)
            } else {
                Err(bad(format!(
                    "`{t}` is not a resource type; they are {}",
                    RESOURCE_TYPES.join(", ")
                )))
            }
        })
        .collect::<Result<_, _>>()?;
    if spec.times == Some(0) {
        return Err(bad("--times must be at least 1"));
    }
    let act = match &spec.action {
        RouteAction::Fulfill {
            status,
            headers,
            body,
        } => Act::Fulfill(fulfill(*status, headers, body)?),
        RouteAction::Abort { reason } => Act::Abort(parse_reason(reason)?),
        RouteAction::Continue {
            set_headers,
            remove_headers,
            url,
        } => {
            for (name, _) in set_headers {
                check_header_name(name)?;
            }
            for name in remove_headers {
                check_header_name(name)?;
            }
            if let Some(url) = url {
                let parsed = url::Url::parse(url).map_err(|e| {
                    bad(format!("--rewrite-url `{url}` is not a URL: {e}")).hint(Hint::text(
                        "give an absolute URL, e.g. http://localhost:3000/api",
                    ))
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(bad(format!(
                        "--rewrite-url `{url}` must be an http or https URL"
                    )));
                }
            }
            Act::Continue {
                set_headers: set_headers.clone(),
                remove_headers: remove_headers
                    .iter()
                    .map(|h| h.trim().to_ascii_lowercase())
                    .collect(),
                url: url.clone(),
            }
        }
        RouteAction::PatchJson { patch } => Act::PatchJson(patch.clone()),
    };
    Ok(Rule {
        id: 0,
        pattern,
        label: spec.label.clone(),
        methods,
        resource_types,
        context: spec.context.clone(),
        left: spec.times,
        hits: 0,
        handler: Arc::new(Handler {
            delay: Duration::from_millis(spec.delay_ms),
            act,
        }),
    })
}

fn check_header_name(name: &str) -> Result<(), ErrorInfo> {
    if mock::is_header_name(name) {
        Ok(())
    } else {
        Err(
            bad(format!("`{name}` is not a header name")).hint(Hint::text(
                "headers are given as Name:value, e.g. --header 'X-Mock: 1'",
            )),
        )
    }
}

fn fulfill(
    status: u16,
    headers: &[(String, String)],
    body: &RouteBody,
) -> Result<Fulfill, ErrorInfo> {
    if !(100..=599).contains(&status) || (101..200).contains(&status) {
        return Err(bad(format!(
            "--status {status} cannot be answered; use 100 or 200-599"
        )));
    }
    for (name, _) in headers {
        check_header_name(name)?;
    }
    let (bytes, kind): (Vec<u8>, Option<&str>) = match body {
        RouteBody::Empty => (Vec::new(), None),
        RouteBody::Text { text } => (text.clone().into_bytes(), Some("text/plain; charset=utf-8")),
        RouteBody::Json { value } => (
            serde_json::to_vec(value).unwrap_or_default(),
            Some("application/json"),
        ),
        RouteBody::File { path } => (read_body(path)?, Some(content_type(path))),
    };
    let mut headers = headers.to_vec();
    let typed = headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("Content-Type"));
    if let (Some(kind), false) = (kind, typed) {
        headers.push(("Content-Type".into(), kind.into()));
    }
    Ok(Fulfill {
        status,
        headers,
        size: bytes.len(),
        body: base64::engine::general_purpose::STANDARD.encode(&bytes),
        cors: true,
    })
}

fn read_body(path: &Path) -> Result<Vec<u8>, ErrorInfo> {
    let failed = |why: String| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read --body-file {}: {why}", path.display()),
        )
    };
    let size = std::fs::metadata(path)
        .map_err(|e| failed(e.to_string()))?
        .len();
    if size > MAX_BODY {
        return Err(failed(format!(
            "it is {}, over the {} limit",
            format::size(size),
            format::size(MAX_BODY)
        )));
    }
    std::fs::read(path).map_err(|e| failed(e.to_string()))
}

/// The `Content-Type` for a file, by its extension.
fn content_type(path: &Path) -> &'static str {
    mock::content_type(path)
}

fn parse_reason(text: &str) -> Result<ErrorReason, ErrorInfo> {
    let name = mock::network_error(text).ok_or_else(|| {
        let names: Vec<String> = ABORT_REASONS
            .iter()
            .map(|r| r.to_ascii_lowercase())
            .collect();
        bad(format!(
            "`{text}` is not a network error; they are {}",
            names.join(", ")
        ))
        .hint(Hint::text(
            "e.g. --abort blockedbyclient, or --abort alone for `failed`",
        ))
    })?;
    serde_json::from_value(Value::String(name.to_owned()))
        .map_err(|e| bad(format!("`{text}`: {e}")))
}

fn reason_name(reason: ErrorReason) -> &'static str {
    let name = serde_json::to_value(reason).ok();
    ABORT_REASONS
        .iter()
        .find(|r| name.as_ref().and_then(Value::as_str) == Some(**r))
        .copied()
        .unwrap_or("Failed")
}

/// What carries a mock's answer out.
fn mock_handler(response: Response, delay: Duration) -> Handler {
    let act = match response {
        Response::Fulfill {
            status,
            headers,
            body,
            cors,
        } => Act::Fulfill(Fulfill {
            status,
            headers,
            size: body.len(),
            body: base64::engine::general_purpose::STANDARD.encode(&body),
            cors,
        }),
        Response::Abort { reason } => {
            Act::Abort(parse_reason(reason).unwrap_or(ErrorReason::Failed))
        }
        Response::Passthrough { patch: None } => Act::Continue {
            set_headers: Vec::new(),
            remove_headers: Vec::new(),
            url: None,
        },
        Response::Passthrough { patch: Some(patch) } => Act::PatchJson(patch),
    };
    Handler { delay, act }
}

/// Applies a JSON merge patch (RFC 7396) to `target`: objects merge key by
/// key, `null` removes a key, anything else replaces.
pub(crate) fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }
    if let Value::Object(target) = target {
        for (key, value) in patch {
            if value.is_null() {
                target.remove(key);
            } else {
                merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(pattern: &str, action: RouteAction) -> RouteSpec {
        RouteSpec {
            pattern: pattern.into(),
            label: None,
            methods: Vec::new(),
            resource_types: Vec::new(),
            times: None,
            delay_ms: 0,
            context: None,
            action,
        }
    }

    fn json_body(value: Value) -> RouteAction {
        RouteAction::Fulfill {
            status: 200,
            headers: Vec::new(),
            body: RouteBody::Json { value },
        }
    }

    fn abort() -> RouteAction {
        RouteAction::Abort {
            reason: "failed".into(),
        }
    }

    const NO_HEADERS: &Value = &Value::Null;

    fn req<'a>(method: &'a str, url: &'a str, kind: &'a str) -> Request<'a> {
        Request {
            url,
            method,
            resource_type: kind,
            headers: NO_HEADERS,
            context: None,
            tab: Some(1),
            body: None,
            now_ms: 0,
        }
    }

    fn rule_of(decision: &Decision) -> Option<u32> {
        match decision {
            Decision::Route { rule, .. } => Some(*rule),
            Decision::Pass | Decision::Preflight | Decision::Mock { .. } => None,
        }
    }

    #[test]
    fn the_first_matching_rule_wins() {
        let router = Router::default();
        let added = router
            .add(&[
                spec("**/api/items", json_body(json!([1]))),
                spec("/api/", abort()),
            ])
            .unwrap();
        assert_eq!(added[0].id, "r1");
        assert_eq!(added[1].id, "r2");
        let (d, _) = router.decide(&req("GET", "http://x/api/items", "Fetch"));
        assert_eq!(rule_of(&d), Some(1));
        let (d, _) = router.decide(&req("GET", "http://x/api/users", "Fetch"));
        assert_eq!(rule_of(&d), Some(2));
        let (d, _) = router.decide(&req("GET", "http://x/app.js", "Script"));
        assert_eq!(rule_of(&d), None);
        let hits: Vec<u64> = router.list().iter().map(|r| r.hits).collect();
        assert_eq!(hits, [1, 1]);
        // Every request is in the journal, with the rule that handled it.
        let (log, skipped) = router.journal(0);
        assert_eq!(skipped, 0);
        let routes: Vec<_> = log.iter().map(|e| e.route.as_deref()).collect();
        assert_eq!(routes, [Some("r1"), Some("r2"), None]);
        assert_eq!(log[0].outcome, "fulfill 200");
        assert_eq!(log[1].outcome, "abort failed");
        assert_eq!(log[2].outcome, "continue");
        assert_eq!(log[2].resource_type, "script");
        assert_eq!(log[0].tab.as_deref(), Some("t1"));
        let (log, skipped) = router.journal(1);
        assert_eq!((log.len(), skipped), (1, 2));
    }

    #[test]
    fn methods_types_and_contexts_narrow_a_rule() {
        let router = Router::default();
        let mut post = spec("/api/", abort());
        post.methods = vec!["post".into()];
        post.resource_types = vec!["XHR".into()];
        let mut other = spec("/other", abort());
        other.context = Some("C2".into());
        router.add(&[post, other]).unwrap();
        let info = &router.list()[0];
        assert_eq!(info.methods, ["POST"]);
        assert_eq!(info.resource_types, ["xhr"]);
        assert!(rule_of(&router.decide(&req("POST", "http://x/api/a", "XHR")).0).is_some());
        assert!(rule_of(&router.decide(&req("POST", "http://x/api/a", "Fetch")).0).is_some());
        assert!(rule_of(&router.decide(&req("GET", "http://x/api/a", "XHR")).0).is_none());
        assert!(rule_of(&router.decide(&req("POST", "http://x/api/a", "Image")).0).is_none());
        let mut in_c2 = req("GET", "http://x/other", "Fetch");
        assert!(rule_of(&router.decide(&in_c2).0).is_none());
        in_c2.context = Some("C2");
        assert!(rule_of(&router.decide(&in_c2).0).is_some());
    }

    #[test]
    fn times_removes_a_rule_after_its_last_match() {
        let router = Router::default();
        let mut once = spec("/a", abort());
        once.times = Some(2);
        router.add(&[once]).unwrap();
        assert_eq!(router.list()[0].left, Some(2));
        let (d, emptied) = router.decide(&req("GET", "http://x/a", "Fetch"));
        assert!(rule_of(&d).is_some() && !emptied);
        assert_eq!(router.list()[0].left, Some(1));
        let (d, emptied) = router.decide(&req("GET", "http://x/a", "Fetch"));
        assert!(rule_of(&d).is_some() && emptied);
        assert!(!router.is_active());
        let (d, _) = router.decide(&req("GET", "http://x/a", "Fetch"));
        assert!(rule_of(&d).is_none());
    }

    #[test]
    fn preflights_for_fulfilled_requests_are_allowed() {
        let router = Router::default();
        let mut post = spec("/api/", json_body(json!({})));
        post.methods = vec!["POST".into()];
        let mut proxied = spec(
            "/proxied",
            RouteAction::Continue {
                set_headers: Vec::new(),
                remove_headers: Vec::new(),
                url: None,
            },
        );
        proxied.methods = vec!["POST".into()];
        router.add(&[post, proxied]).unwrap();
        let headers = json!({ "access-control-request-method": "POST", "Origin": "http://a" });
        let preflight = |url| Request {
            headers: &headers,
            ..req("OPTIONS", url, "Preflight")
        };
        assert!(matches!(
            router.decide(&preflight("http://b/api/x")).0,
            Decision::Preflight
        ));
        // Not for a rule that sends the request on: the server answers.
        assert!(matches!(
            router.decide(&preflight("http://b/proxied")).0,
            Decision::Pass
        ));
        // Preflights are no hits.
        assert_eq!(router.list()[0].hits, 0);
    }

    #[test]
    fn removing_rules() {
        let router = Router::default();
        router
            .add(&[
                spec("/a", abort()),
                spec("/b", abort()),
                spec("/c", abort()),
            ])
            .unwrap();
        assert_eq!(router.remove(&[2]), Ok(vec![2]));
        assert_eq!(router.remove(&[1, 9]), Err(9));
        assert_eq!(router.len(), 2);
        assert_eq!(router.clear(), [1, 3]);
        // Ids are never reused.
        let added = router.add(&[spec("/d", abort())]).unwrap();
        assert_eq!(added[0].id, "r4");
    }

    #[test]
    fn bad_rules_add_nothing() {
        let router = Router::default();
        let mut bad_type = spec("/a", abort());
        bad_type.resource_types = vec!["pictures".into()];
        let cases = [
            (spec("/(/", abort()), "not a valid regular expression"),
            (
                spec(
                    "/a",
                    RouteAction::Abort {
                        reason: "nope".into(),
                    },
                ),
                "is not a network error; they are failed, aborted, timedout,",
            ),
            (bad_type, "is not a resource type"),
            (
                spec(
                    "/a",
                    RouteAction::Fulfill {
                        status: 42,
                        headers: Vec::new(),
                        body: RouteBody::Empty,
                    },
                ),
                "--status 42",
            ),
            (
                spec(
                    "/a",
                    RouteAction::Continue {
                        set_headers: vec![("Bad Name".into(), "1".into())],
                        remove_headers: Vec::new(),
                        url: None,
                    },
                ),
                "is not a header name",
            ),
            (
                spec(
                    "/a",
                    RouteAction::Continue {
                        set_headers: Vec::new(),
                        remove_headers: Vec::new(),
                        url: Some("/relative".into()),
                    },
                ),
                "is not a URL",
            ),
        ];
        for (bad, message) in cases {
            let err = router.add(&[spec("/ok", abort()), bad]).unwrap_err();
            assert_eq!(err.exit_code(), 2);
            assert!(err.message.contains(message), "{}", err.message);
        }
        assert_eq!(router.len(), 0);
    }

    #[test]
    fn abort_reasons_take_any_case() {
        for (text, name) in [
            ("blockedbyclient", "BlockedByClient"),
            ("TimedOut", "TimedOut"),
            ("connection-refused", "ConnectionRefused"),
        ] {
            assert_eq!(reason_name(parse_reason(text).unwrap()), name);
        }
    }

    #[test]
    fn summaries() {
        let router = Router::default();
        let mut slow = spec(
            "/slow",
            RouteAction::Continue {
                set_headers: vec![("X-Test".into(), "1".into())],
                remove_headers: vec!["Cookie".into()],
                url: None,
            },
        );
        slow.delay_ms = 2000;
        let added = router
            .add(&[
                spec("/j", json_body(json!({"a": 1}))),
                spec(
                    "/t",
                    RouteAction::Fulfill {
                        status: 404,
                        headers: vec![("content-type".into(), "text/html".into())],
                        body: RouteBody::Text { text: "no".into() },
                    },
                ),
                spec(
                    "/b",
                    RouteAction::Abort {
                        reason: "BlockedByClient".into(),
                    },
                ),
                slow,
                spec(
                    "/p",
                    RouteAction::PatchJson {
                        patch: json!({"items": []}),
                    },
                ),
            ])
            .unwrap();
        let summaries: Vec<&str> = added.iter().map(|r| r.summary.as_str()).collect();
        assert_eq!(
            summaries,
            [
                "fulfill 200 application/json 7B",
                "fulfill 404 text/html 2B",
                "abort blockedbyclient",
                "delay 2s, then continue (X-Test: 1, no cookie)",
                r#"patch json {"items":[]}"#,
            ]
        );
    }

    #[test]
    fn merge_patch_follows_rfc_7396() {
        let mut doc = json!({"a": {"b": 1, "c": 2}, "d": [1, 2], "e": 1});
        merge_patch(
            &mut doc,
            &json!({"a": {"b": null, "x": {"y": 1}}, "d": [], "e": null, "f": "new"}),
        );
        assert_eq!(
            doc,
            json!({"a": {"c": 2, "x": {"y": 1}}, "d": [], "f": "new"})
        );
        let mut list = json!([1, 2]);
        merge_patch(&mut list, &json!({"wrapped": true}));
        assert_eq!(list, json!({"wrapped": true}));
        merge_patch(&mut list, &json!([]));
        assert_eq!(list, json!([]));
    }

    #[test]
    fn route_ids() {
        assert_eq!(parse_route_id("r3").unwrap(), 3);
        assert_eq!(parse_route_id("12").unwrap(), 12);
        for bad in ["", "r", "r0", "t3", "r-1"] {
            assert_eq!(parse_route_id(bad).unwrap_err().exit_code(), 2, "{bad}");
        }
    }
}
