//! `network`, `network show` and `network clear` (plan 06, milestone 6.2).
//!
//! All three read the tab's network log (see [`super::netlog`]). Like
//! `console`, none takes the tab's lock or counts as a page command, so
//! they answer while an action runs or a dialog is open.
//!
//! `network` has a read cursor per tab: by default (`--since last`) it
//! shows only the requests it has not shown before. A request it showed
//! while pending is shown once more when it ends, so the agent sees how
//! it went. `network show` fetches bodies from Chrome only when asked
//! (`--body`, `--request-body`); Chrome keeps them for a while, not
//! forever.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use cdp::domains::network as cdp_network;
use protocol::schema::{
    Body, NetworkClearData, NetworkData, NetworkEntry, NetworkShowData, RequestState,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format as units};
use tokio::time::Instant;

use super::commands::shown_url;
use super::netlog::{NetLog, Record};
use super::save;
use crate::Engine;
use crate::Running;
use crate::input::origin;
use crate::pattern::UrlPattern;
use crate::request::{NetworkClearRequest, NetworkRequest, NetworkShowRequest, Since, millis};
use crate::tabs::{Page, TabError, parse_tab_id, tab_name};

/// The resource types `network --type` takes: Chrome's, in lower case.
pub const RESOURCE_TYPES: &[&str] = &[
    "document",
    "stylesheet",
    "image",
    "media",
    "font",
    "script",
    "texttrack",
    "xhr",
    "fetch",
    "prefetch",
    "eventsource",
    "websocket",
    "manifest",
    "signedexchange",
    "ping",
    "cspviolationreport",
    "preflight",
    "fedcm",
    "other",
];

/// How much of a URL a `network` line shows.
const SHOWN_URL: usize = 150;

/// How many WebSocket frames `network show` lists in the text form.
const SHOWN_FRAMES: usize = 10;

/// What one `network` call picked from the log.
#[derive(Debug)]
struct Selection {
    requests: Vec<NetworkEntry>,
    /// Older matches `--limit` left out.
    skipped: usize,
    /// Requests after the starting point the log dropped.
    dropped: u64,
    /// Where the selection started (an id).
    base: u64,
    /// The cursor after this call.
    cursor: u64,
}

/// What `network` filters on.
struct Filter<'a> {
    req: &'a NetworkRequest,
    url: Option<UrlPattern>,
}

impl Filter<'_> {
    fn matches(&self, e: &NetworkEntry) -> bool {
        let req = self.req;
        (!req.failed || failed(e))
            && (req.types.is_empty() || req.types.iter().any(|t| t.eq_ignore_ascii_case(&e.kind)))
            && self.url.as_ref().is_none_or(|p| p.matches(&e.url))
            && req
                .method
                .as_ref()
                .is_none_or(|m| m.eq_ignore_ascii_case(&e.method))
            && (req.status.is_empty()
                || e.status
                    .is_some_and(|s| req.status.iter().any(|r| r.contains(s))))
    }
}

/// Whether `--failed` shows `entry`: it ended without a response, or got
/// a status of 400 or more (its body may still be loading: Chrome ends a
/// `fetch` only once the page reads the body). Requests canceled before a
/// response are the page's (or a navigation's) own doing.
fn failed(entry: &NetworkEntry) -> bool {
    entry.state == RequestState::Failed || entry.status.is_some_and(|s| s >= 400)
}

impl Engine {
    /// `network`: the tab's requests after `--since`, filtered, the newest
    /// `--limit`. With `wait_ms` and nothing to show, waits that long for
    /// one (`--follow`).
    pub async fn network(&self, mut req: NetworkRequest) -> Result<Output, ErrorInfo> {
        let url = req.url.as_deref().map(UrlPattern::parse).transpose()?;
        let running = self.running()?;
        let page = self
            .log_page(&running, req.tab.as_deref(), req.timeout_ms)
            .await?;
        if let Since::Mark { name } = &req.since {
            let marks = running
                .tabs
                .named_mark(page.id, name)
                .map_err(|names| self.no_mark(page.id, name, &names))?;
            req.since = Since::Seq { seq: marks.netlog };
        }
        let page_origin = page_origin(&running, page.id);
        let filter = Filter { req: &req, url };
        let deadline = Instant::now() + Duration::from_millis(req.wait_ms);
        let selection = loop {
            let (selection, changes) = running
                .tabs
                .netlog(page.id, |log| {
                    (select(log, &filter, now_ms()), log.changes())
                })
                .ok_or_else(|| self.tab_error(&running, TabError::NoSuchTab(page.id)))?;
            if !selection.requests.is_empty() || Instant::now() >= deadline {
                break selection;
            }
            if !running
                .tabs
                .netlog_changed(page.id, changes, deadline)
                .await
            {
                break selection;
            }
        };
        Ok(self.network_output(&page, &req, selection, page_origin.as_deref()))
    }

    fn network_output(
        &self,
        page: &Page,
        req: &NetworkRequest,
        selection: Selection,
        page_origin: Option<&str>,
    ) -> Output {
        let tab = tab_name(page.id);
        let mut text = String::new();
        for entry in &selection.requests {
            text.push_str(&entry_line(entry, page_origin));
            text.push('\n');
        }
        if selection.skipped > 0 {
            let since = match req.since {
                Since::Ago { ms } => format!("{ms}ms"),
                _ => selection.base.to_string(),
            };
            let cmd = self
                .network_cmd(req)
                .flag("since", since)
                .flag("limit", (req.limit + selection.skipped).to_string());
            let _ = writeln!(
                text,
                "… {} earlier {} not shown; {cmd} shows them",
                selection.skipped,
                if selection.skipped == 1 {
                    "request"
                } else {
                    "requests"
                },
            );
        }
        if selection.requests.is_empty() {
            let new = if req.since == Since::Last { "new " } else { "" };
            let _ = write!(text, "no {new}requests in {tab}");
            if req.since == Since::Last && selection.cursor > 0 {
                let cmd = self.network_cmd(req).flag("since", "0");
                let _ = write!(text, "; {cmd} shows the earlier ones");
            }
            text.push('\n');
        } else if let Some(last) = selection.requests.last()
            // `--follow`'s later batches: said once is enough.
            && req.wait_ms == 0
        {
            let cmd = self.network_show_cmd(page, last.id);
            let _ = writeln!(text, "{cmd} shows one in full");
        }
        let quiet: Vec<String> = selection
            .requests
            .iter()
            .map(|e| format!("#{}", e.id))
            .collect();
        let mut output = Output::typed(
            &NetworkData {
                tab: tab.clone(),
                requests: selection.requests,
                skipped: selection.skipped,
                dropped: selection.dropped,
                cursor: selection.cursor,
            },
            text.trim_end().to_owned(),
        )
        .quiet(quiet.join("\n"));
        if selection.dropped > 0 {
            output = output.warning(format!(
                "{} {} dropped from {tab}'s network log before they were read; it keeps the \
                 newest {} (open --network-buffer N keeps more)",
                selection.dropped,
                if selection.dropped == 1 {
                    "request was"
                } else {
                    "requests were"
                },
                self.settings.network_buffer(),
            ));
        }
        output.limit(
            req.max_chars,
            "--max-chars 0 for all of it, or --limit N for fewer",
        )
    }

    /// `network` with the flags of `req` that pick requests (and the
    /// session), for hints.
    fn network_cmd(&self, req: &NetworkRequest) -> Cmd {
        let mut cmd = Cmd::new("network");
        if let Some(tab) = &req.tab {
            cmd = cmd.flag("tab", tab);
        }
        if req.failed {
            cmd = cmd.switch("failed");
        }
        if !req.types.is_empty() {
            cmd = cmd.flag("type", req.types.join(","));
        }
        if let Some(url) = &req.url {
            cmd = cmd.flag("url", url);
        }
        if let Some(method) = &req.method {
            cmd = cmd.flag("method", method);
        }
        if !req.status.is_empty() {
            let ranges: Vec<String> = req
                .status
                .iter()
                .map(|r| {
                    if r.min == r.max {
                        r.min.to_string()
                    } else if r.min % 100 == 0 && r.max == r.min + 99 {
                        format!("{}xx", r.min / 100)
                    } else {
                        format!("{}-{}", r.min, r.max)
                    }
                })
                .collect();
            cmd = cmd.flag("status", ranges.join(","));
        }
        cmd.session(&self.session)
    }

    /// `network show <id>` on `page` (naming it unless it is active).
    fn network_show_cmd(&self, page: &Page, id: u64) -> Cmd {
        let mut cmd = Cmd::new("network").arg("show").arg(id.to_string());
        if let Ok(running) = self.running()
            && running.tabs.active() != Some(page.id)
        {
            cmd = cmd.flag("tab", tab_name(page.id));
        }
        cmd.session(&self.session)
    }

    /// `network show`: one request in full, its bodies fetched from Chrome
    /// if asked for.
    pub async fn network_show(&self, req: NetworkShowRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self
            .log_page(&running, req.tab.as_deref(), req.timeout_ms)
            .await?;
        let page_origin = page_origin(&running, page.id);
        let found = running
            .tabs
            .netlog(page.id, |log| {
                (
                    log.get(req.id).cloned(),
                    log.records().next().map(|r| r.entry.id),
                    log.seq(),
                )
            })
            .ok_or_else(|| self.tab_error(&running, TabError::NoSuchTab(page.id)))?;
        let record = match found {
            (Some(record), _, _) => record,
            (None, oldest, latest) => {
                return Err(self.no_such_request(&page, req.id, oldest, latest));
            }
        };
        let mut data = NetworkShowData {
            tab: tab_name(page.id),
            request: record.entry.clone(),
            timing: record.timing.clone(),
            request_headers: None,
            response_headers: None,
            request_body: None,
            response_body: None,
            frames: record.frames.iter().cloned().collect(),
        };
        if req.headers {
            data.request_headers = Some(record.request_headers.clone());
            data.response_headers = Some(record.response_headers.clone());
        }
        if req.request_body {
            data.request_body = Some(self.request_body(&running, &record, timeout).await);
        }
        if let Some(path) = &req.output {
            data.response_body = Some(
                self.save_response_body(&running, &record, path, timeout)
                    .await?,
            );
        } else if req.body {
            data.response_body = Some(self.response_body(&running, &record, timeout).await);
        }
        let text = show_text(&data, &record, &req, page_origin.as_deref());
        let quiet = data
            .response_body
            .as_ref()
            .and_then(|b| b.path.clone())
            .or_else(|| {
                data.response_body
                    .as_ref()
                    .or(data.request_body.as_ref())
                    .and_then(|b| b.text.clone())
            })
            .unwrap_or_else(|| entry_line(&data.request, page_origin.as_deref()));
        Ok(Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it"))
    }

    /// `network clear`: empties the tab's network log.
    pub async fn network_clear(&self, req: NetworkClearRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let page = self
            .log_page(&running, req.tab.as_deref(), req.timeout_ms)
            .await?;
        let cleared = running
            .tabs
            .netlog(page.id, NetLog::clear)
            .ok_or_else(|| self.tab_error(&running, TabError::NoSuchTab(page.id)))?;
        let tab = tab_name(page.id);
        let text = format!(
            "cleared {} from {tab}'s network log",
            match cleared {
                1 => "1 request".to_owned(),
                n => format!("{} requests", units::thousands(n)),
            }
        );
        Ok(Output::typed(&NetworkClearData { tab, cleared }, text).quiet(cleared.to_string()))
    }

    /// The tab `tab` names (or the active one). Not `Engine::page`:
    /// reading the log is no page command, so it must not move `wait
    /// --request`'s "previous command" mark.
    async fn log_page(
        &self,
        running: &Running,
        tab: Option<&str>,
        timeout_ms: u64,
    ) -> Result<Page, ErrorInfo> {
        let id = tab.map(parse_tab_id).transpose()?;
        running
            .tabs
            .page(id, millis(timeout_ms))
            .await
            .map_err(|e| self.tab_error(running, e))
    }

    fn no_such_request(&self, page: &Page, id: u64, oldest: Option<u64>, latest: u64) -> ErrorInfo {
        let tab = tab_name(page.id);
        let why = if id > latest || id == 0 {
            format!("{tab}'s network log has no request #{id}")
        } else {
            format!(
                "request #{id} is no longer in {tab}'s network log (cleared, or dropped to make room)"
            )
        };
        let mut list = Cmd::new("network").flag("since", "0");
        if self
            .running()
            .is_ok_and(|r| r.tabs.active() != Some(page.id))
        {
            list = list.flag("tab", &tab);
        }
        let mut info = ErrorInfo::new(ErrorKind::NotFound, why)
            .hint(Hint::run(list.session(&self.session)).why("lists the requests it has"));
        info.details = Some(serde_json::json!({ "id": id, "oldest": oldest, "latest": latest }));
        info
    }

    /// The request body: kept in the log if short, else asked of Chrome.
    async fn request_body(&self, running: &Running, record: &Record, timeout: Duration) -> Body {
        if let Some(data) = &record.post_data {
            return Body {
                text: Some(data.clone()),
                size: Some(data.len() as u64),
                ..Body::default()
            };
        }
        if !record.has_post_data {
            return unavailable("the request has no body");
        }
        let Some(session) = running.tabs.host_session(&record.session) else {
            return unavailable("its frame is gone, and Chrome with it");
        };
        let params = cdp_network::GetRequestPostDataParams::new(record.request_id.clone());
        match session.with_timeout(timeout).call(params).await {
            Ok(data) => body(&data.post_data, data.base64_encoded, None),
            Err(e) => unavailable(&gone(&e)),
        }
    }

    /// The response body, asked of Chrome.
    async fn response_body(&self, running: &Running, record: &Record, timeout: Duration) -> Body {
        match self.response_data(running, record, timeout).await {
            Ok((data, base64)) => body(&data, base64, record.entry.mime.as_deref()),
            Err(unavailable) => unavailable,
        }
    }

    /// `network show -o`: the response body saved to `path`, decoded (an
    /// image as its bytes). A body Chrome cannot give is an error: the
    /// caller wants a file.
    async fn save_response_body(
        &self,
        running: &Running,
        record: &Record,
        path: &Path,
        timeout: Duration,
    ) -> Result<Body, ErrorInfo> {
        let id = record.entry.id;
        let (data, base64) = match self.response_data(running, record, timeout).await {
            Ok(found) => found,
            Err(body) => {
                let why = body.unavailable.unwrap_or_default();
                return Err(ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("request #{id} has no response body to save: {why}"),
                ));
            }
        };
        let bytes = if base64 {
            base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(|_| {
                    ErrorInfo::new(
                        ErrorKind::ProtocolError,
                        format!(
                            "Chrome sent the body of request #{id} in a form chrome-cdp cannot read"
                        ),
                    )
                })?
        } else {
            data.into_bytes()
        };
        let dir = path.parent().unwrap_or(path);
        let saved = save::save(&bytes, Some(path), dir, "")?;
        let binary = std::str::from_utf8(&bytes).is_err()
            || !record.entry.mime.as_deref().is_none_or(textual);
        Ok(Body {
            size: Some(bytes.len() as u64),
            binary,
            path: Some(saved.display().to_string()),
            ..Body::default()
        })
    }

    /// The response body as Chrome gives it (and whether it is base64), or
    /// an unavailable [`Body`] saying why there is none.
    async fn response_data(
        &self,
        running: &Running,
        record: &Record,
        timeout: Duration,
    ) -> Result<(String, bool), Body> {
        let entry = &record.entry;
        let why = match entry.state {
            _ if entry.websocket.is_some() => Some("a WebSocket has frames, not a body"),
            _ if entry.redirect.is_some() => Some("a redirect has no body"),
            RequestState::Pending => Some(
                "the response is still loading (Chrome ends a fetch once the page reads its body)",
            ),
            RequestState::Failed => Some("the request failed"),
            RequestState::Canceled => Some("the request was canceled"),
            RequestState::Done => None,
        };
        if let Some(why) = why {
            return Err(unavailable(why));
        }
        let Some(session) = running.tabs.host_session(&record.session) else {
            return Err(unavailable("its frame is gone, and the body with it"));
        };
        let params = cdp_network::GetResponseBodyParams::new(record.request_id.clone());
        match session.with_timeout(timeout).call(params).await {
            Ok(data) => Ok((data.body, data.base64_encoded)),
            Err(e) => Err(unavailable(&gone(&e))),
        }
    }
}

/// Why Chrome could not give a body.
fn gone(error: &impl std::fmt::Display) -> String {
    let message = error.to_string();
    if message.contains("No resource with given identifier")
        || message.contains("No data found")
        || message.contains("No post data")
    {
        "Chrome no longer has it: it keeps bodies only while its buffer has room and the page \
         that loaded them is open"
            .to_owned()
    } else {
        message
    }
}

fn unavailable(why: &str) -> Body {
    Body {
        unavailable: Some(why.to_owned()),
        ..Body::default()
    }
}

/// A body as Chrome gave it: text, unless it is binary.
fn body(data: &str, base64: bool, mime: Option<&str>) -> Body {
    if !base64 {
        return Body {
            text: Some(data.to_owned()),
            size: Some(data.len() as u64),
            ..Body::default()
        };
    }
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
        return unavailable("Chrome sent it in a form chrome-cdp cannot read");
    };
    let size = Some(bytes.len() as u64);
    match String::from_utf8(bytes) {
        Ok(text) if mime.is_none_or(textual) => Body {
            text: Some(text),
            size,
            ..Body::default()
        },
        _ => Body {
            size,
            binary: true,
            ..Body::default()
        },
    }
}

/// Whether a MIME type is text.
fn textual(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    mime.starts_with("text/")
        || [
            "json",
            "javascript",
            "ecmascript",
            "xml",
            "x-www-form-urlencoded",
            "graphql",
        ]
        .iter()
        .any(|t| mime.contains(t))
}

/// The picked requests of `log`, and moves its cursor past them.
fn select(log: &mut NetLog, filter: &Filter<'_>, now_ms: u64) -> Selection {
    let req = filter.req;
    let cursor = log.cursor;
    let (base, since_ms) = match req.since {
        // A mark is a `Seq` by now.
        Since::Last | Since::Mark { .. } => (cursor, 0),
        Since::Seq { seq } => (seq, 0),
        Since::Ago { ms } => (0, now_ms.saturating_sub(ms)),
    };
    let base = base.max(log.cleared());
    let mut requests: Vec<NetworkEntry> = log
        .records()
        .map(|r| &r.entry)
        .filter(|e| {
            let after = e.id > base && e.time_ms >= since_ms;
            // Shown pending before, and over now.
            let ended = req.since == Since::Last
                && e.state != RequestState::Pending
                && log.reshow.contains(&e.id);
            (after || ended) && filter.matches(e)
        })
        .cloned()
        .collect();
    let skipped = match req.limit {
        0 => 0,
        limit => requests.len().saturating_sub(limit),
    };
    requests.drain(..skipped);
    let dropped = match req.since {
        Since::Ago { .. } => 0,
        _ => log.evicted().saturating_sub(base),
    };
    for entry in &requests {
        if entry.state == RequestState::Pending {
            log.reshow.insert(entry.id);
        } else {
            log.reshow.remove(&entry.id);
        }
    }
    let shown = requests.iter().map(|e| e.id).max().unwrap_or(0);
    log.cursor = cursor.max(shown);
    Selection {
        requests,
        skipped,
        dropped,
        base,
        cursor: log.cursor,
    }
}

/// The origin of tab `id`'s main frame, to shorten URLs by.
fn page_origin(running: &Running, id: u32) -> Option<String> {
    let frames = running.tabs.frames(id);
    let url = frames.first().map(|f| f.url.as_str()).unwrap_or_default();
    origin(url).map(str::to_owned)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A URL as a line shows it: without the page's origin, and cut.
fn short_url(url: &str, page_origin: Option<&str>) -> String {
    let mut url = shown_url(url, page_origin).to_owned();
    super::format::cut(&mut url, SHOWN_URL);
    url
}

/// `#42 POST 500 /api/login xhr 12KB 340ms`, and what went wrong.
fn entry_line(entry: &NetworkEntry, page_origin: Option<&str>) -> String {
    let status = match (entry.state, entry.status) {
        (RequestState::Canceled, _) => "canceled".to_owned(),
        // Pending too: its body still loading, or an open WebSocket.
        (_, Some(status)) => status.to_string(),
        (RequestState::Pending, None) => "pending".to_owned(),
        (RequestState::Failed, None) => "failed".to_owned(),
        (RequestState::Done, None) => "done".to_owned(),
    };
    let mut line = format!(
        "#{} {} {status} {} {}",
        entry.id,
        entry.method,
        short_url(&entry.url, page_origin),
        entry.kind,
    );
    if let Some(cache) = &entry.cache {
        let _ = write!(line, " ({cache} cache)");
    } else if entry.service_worker {
        line.push_str(" (service worker)");
    } else if let Some(size) = entry
        .size
        .filter(|_| entry.state != RequestState::Pending || entry.websocket.is_some())
    {
        let _ = write!(line, " {}", units::size(size));
    }
    if let Some(ms) = entry.duration_ms {
        let _ = write!(line, " {}", units::duration(Duration::from_millis(ms)));
    }
    if let Some(to) = &entry.redirect {
        let _ = write!(line, " -> {}", short_url(to, page_origin));
    }
    if let Some(ws) = &entry.websocket {
        let _ = write!(line, " {} sent, {} received", ws.sent, ws.received);
        if !ws.closed && entry.state == RequestState::Pending {
            line.push_str(", open");
        }
    }
    if let Some(error) = &entry.error {
        let _ = write!(line, " {error}");
    }
    if let Some(blocked) = &entry.blocked {
        let _ = write!(line, " (blocked: {blocked})");
    }
    if let Some(cors) = &entry.cors {
        let _ = write!(line, " (CORS: {cors})");
    }
    line
}

/// `network show`'s text form.
fn show_text(
    data: &NetworkShowData,
    record: &Record,
    req: &NetworkShowRequest,
    page_origin: Option<&str>,
) -> String {
    let entry = &data.request;
    let mut text = String::new();
    let status = match (entry.state, entry.status) {
        (RequestState::Canceled, _) => "canceled".to_owned(),
        (_, Some(status)) => match &entry.status_text {
            Some(why) => format!("{status} {why}"),
            None => status.to_string(),
        },
        (RequestState::Pending, None) => "pending".to_owned(),
        (_, None) => "failed".to_owned(),
    };
    let _ = writeln!(
        text,
        "#{} {} {status} {}",
        entry.id, entry.method, entry.url
    );
    let mut about = vec![format!("type: {}", entry.kind)];
    if let Some(mime) = &entry.mime {
        about.push(format!("mime: {mime}"));
    }
    if let Some(protocol) = &entry.protocol {
        about.push(format!("protocol: {protocol}"));
    }
    if let Some(cache) = &entry.cache {
        about.push(format!("from the {cache} cache"));
    }
    if entry.service_worker {
        about.push("from a service worker".to_owned());
    }
    if entry.state == RequestState::Pending && entry.status.is_some() && entry.websocket.is_none() {
        about.push("body still loading".to_owned());
    }
    let _ = writeln!(text, "{}", about.join(", "));
    let mut sizes = Vec::new();
    if let Some(size) = entry.size {
        let mut s = format!("size: {}", units::size(size));
        if let Some(body) = entry
            .body_size
            .filter(|b| units::size(*b) != units::size(size))
        {
            let _ = write!(s, " ({} decoded)", units::size(body));
        }
        sizes.push(s);
    }
    if let Some(ms) = entry.duration_ms {
        let mut s = format!("time: {}", units::duration(Duration::from_millis(ms)));
        if let Some(t) = &data.timing {
            let mut phases = Vec::new();
            for (name, ms) in [
                ("blocked", t.blocked_ms),
                ("dns", t.dns_ms),
                ("connect", t.connect_ms),
                ("tls", t.tls_ms),
                ("send", t.send_ms),
            ] {
                if ms > 0 {
                    phases.push(format!("{name} {ms}ms"));
                }
            }
            phases.push(format!("wait {}ms", t.wait_ms));
            if let Some(receive) = t.receive_ms {
                phases.push(format!("receive {receive}ms"));
            }
            let _ = write!(s, " ({})", phases.join(", "));
        }
        sizes.push(s);
    }
    if !sizes.is_empty() {
        let _ = writeln!(text, "{}", sizes.join(", "));
    }
    if let Some(initiator) = &entry.initiator {
        let mut s = format!("initiator: {}", initiator.kind);
        if let Some(url) = &initiator.url {
            let _ = write!(s, " {}", shown_url(url, page_origin));
            if let Some(line) = initiator.line {
                let _ = write!(s, ":{line}");
            }
        }
        if let Some(function) = &initiator.function {
            let _ = write!(s, " ({function})");
        }
        let _ = writeln!(text, "{s}");
    }
    if let Some(to) = &entry.redirect {
        let _ = writeln!(text, "redirect: {to}");
    }
    let mut problems = Vec::new();
    if let Some(error) = &entry.error {
        problems.push(error.clone());
    }
    if let Some(blocked) = &entry.blocked {
        problems.push(format!("blocked: {blocked}"));
    }
    if let Some(cors) = &entry.cors {
        problems.push(format!("CORS: {cors}"));
    }
    if !problems.is_empty() {
        let _ = writeln!(text, "error: {}", problems.join(", "));
    }
    if let Some(ws) = &entry.websocket {
        let _ = writeln!(
            text,
            "frames: {} sent ({}), {} received ({}){}",
            ws.sent,
            units::size(ws.sent_bytes),
            ws.received,
            units::size(ws.received_bytes),
            if ws.closed { ", closed" } else { ", open" },
        );
        let skip = data.frames.len().saturating_sub(SHOWN_FRAMES);
        for frame in &data.frames[skip..] {
            let _ = write!(
                text,
                "  {} {} {}",
                if frame.sent { "sent" } else { "received" },
                frame.opcode,
                units::size(frame.size),
            );
            if let Some(payload) = &frame.text {
                let _ = write!(text, " {}", payload.replace('\n', " "));
            }
            text.push('\n');
        }
    }
    match (&data.request_headers, &data.response_headers) {
        (Some(request), Some(response)) => {
            for (side, headers) in [("request", request), ("response", response)] {
                if headers.is_empty() {
                    let _ = writeln!(text, "{side} headers: none");
                    continue;
                }
                let _ = writeln!(text, "{side} headers:");
                for h in headers {
                    let _ = writeln!(text, "  {}: {}", h.name, h.value);
                }
            }
        }
        _ => {
            let _ = writeln!(
                text,
                "headers: {} request, {} response",
                record.request_headers.len(),
                record.response_headers.len(),
            );
        }
    }
    for (side, body) in [
        ("request body", &data.request_body),
        ("response body", &data.response_body),
    ] {
        if let Some(body) = body {
            body_text(
                &mut text,
                side,
                body,
                entry.mime.as_deref().filter(|_| side == "response body"),
            );
        }
    }
    // What more there is to see.
    let mut more = Vec::new();
    if !req.headers {
        more.push("--headers");
    }
    if !req.body
        && req.output.is_none()
        && entry.state == RequestState::Done
        && entry.websocket.is_none()
        && entry.redirect.is_none()
    {
        more.push("--body");
    }
    if !req.request_body && record.has_post_data {
        more.push("--request-body");
    }
    if !more.is_empty() {
        let _ = writeln!(text, "({} show more)", more.join(", "));
    }
    text.trim_end().to_owned()
}

/// Adds `body`'s lines: pretty-printed if it is JSON.
fn body_text(text: &mut String, side: &str, body: &Body, mime: Option<&str>) {
    if let Some(why) = &body.unavailable {
        let _ = writeln!(text, "{side}: not available: {why}");
        return;
    }
    let size = body.size.map(units::size).unwrap_or_default();
    if let Some(path) = &body.path {
        let _ = writeln!(text, "{side}: saved to {path} ({size})");
        return;
    }
    if body.binary {
        let _ = writeln!(
            text,
            "{side}: binary, {size}{}; not shown",
            mime.map(|m| format!(" of {m}")).unwrap_or_default()
        );
        return;
    }
    let content = body.text.as_deref().unwrap_or_default();
    if content.is_empty() {
        let _ = writeln!(text, "{side}: empty");
        return;
    }
    let _ = writeln!(text, "{side} ({size}):");
    let json = serde_json::from_str::<serde_json::Value>(content)
        .is_ok_and(|v| v.is_object() || v.is_array());
    let pretty = json.then(|| indent_json(content));
    let _ = writeln!(text, "{}", pretty.as_deref().unwrap_or(content));
}

/// Valid JSON `json` indented by two spaces a level, its keys in their
/// order (parsing it into a `Value` would sort them).
fn indent_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len() * 2);
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = json.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                // `{}` and `[]` stay as they are.
                if chars.peek().is_some_and(|n| matches!(n, '}' | ']')) {
                    out.extend(chars.next());
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use protocol::schema::{NetworkEntry, RequestState, WebSocketSummary};
    use serde_json::json;

    use super::{Filter, NetLog, entry_line, select};
    use crate::request::{NetworkRequest, Since, StatusRange};

    fn request() -> NetworkRequest {
        NetworkRequest {
            tab: None,
            failed: false,
            types: Vec::new(),
            url: None,
            method: None,
            status: Vec::new(),
            since: Since::Last,
            limit: 50,
            wait_ms: 0,
            timeout_ms: 1000,
            max_chars: 0,
        }
    }

    fn sent(log: &mut NetLog, id: &str, method: &str, url: &str, kind: &str) {
        let params = json!({
            "requestId": id, "type": kind, "timestamp": 1.0,
            "request": { "method": method, "url": url },
        });
        log.event("s1", "Network.requestWillBeSent", &params, 100);
    }

    fn done(log: &mut NetLog, id: &str, status: u16) {
        let response = json!({ "requestId": id, "response": { "url": "", "status": status } });
        log.event("s1", "Network.responseReceived", &response, 100);
        let end = json!({ "requestId": id, "timestamp": 1.2, "encodedDataLength": 2048 });
        log.event("s1", "Network.loadingFinished", &end, 100);
    }

    fn ids(log: &mut NetLog, req: &NetworkRequest) -> Vec<u64> {
        let filter = Filter {
            req,
            url: req
                .url
                .as_deref()
                .map(|u| crate::pattern::UrlPattern::parse(u).unwrap()),
        };
        select(log, &filter, u64::MAX)
            .requests
            .iter()
            .map(|e| e.id)
            .collect()
    }

    #[test]
    fn the_cursor_shows_each_request_once_and_ends_again() {
        let mut log = NetLog::default();
        sent(&mut log, "1", "GET", "http://x/", "Document");
        done(&mut log, "1", 200);
        sent(&mut log, "2", "GET", "http://x/api/slow", "Fetch");
        let req = request();
        assert_eq!(ids(&mut log, &req), [1, 2]);
        assert!(ids(&mut log, &req).is_empty());
        // The pending one ends: shown once more, with a new one.
        done(&mut log, "2", 200);
        sent(&mut log, "3", "GET", "http://x/app.js", "Script");
        assert_eq!(ids(&mut log, &req), [2, 3]);
        // Still pending: nothing new.
        assert!(ids(&mut log, &req).is_empty());
        done(&mut log, "3", 200);
        assert_eq!(ids(&mut log, &req), [3]);
        assert!(ids(&mut log, &req).is_empty());
        let all = NetworkRequest {
            since: Since::Seq { seq: 0 },
            ..request()
        };
        assert_eq!(ids(&mut log, &all), [1, 2, 3]);
    }

    #[test]
    fn filters() {
        let mut log = NetLog::default();
        sent(&mut log, "1", "GET", "http://x/", "Document");
        done(&mut log, "1", 200);
        sent(&mut log, "2", "POST", "http://x/api/login", "XHR");
        done(&mut log, "2", 500);
        sent(&mut log, "3", "GET", "http://x/api/items", "Fetch");
        done(&mut log, "3", 404);
        sent(&mut log, "4", "GET", "http://127.0.0.1:1/x", "Fetch");
        log.event(
            "s1",
            "Network.loadingFailed",
            &json!({ "requestId": "4", "timestamp": 1.1, "errorText": "net::ERR_CONNECTION_REFUSED" }),
            100,
        );
        sent(&mut log, "5", "GET", "http://x/gone", "Fetch");
        log.event(
            "s1",
            "Network.loadingFailed",
            &json!({ "requestId": "5", "timestamp": 1.1, "errorText": "net::ERR_ABORTED", "canceled": true }),
            100,
        );
        let all = |r: NetworkRequest| NetworkRequest {
            since: Since::Seq { seq: 0 },
            ..r
        };
        let failed = all(NetworkRequest {
            failed: true,
            ..request()
        });
        assert_eq!(ids(&mut log, &failed), [2, 3, 4]);
        let types = all(NetworkRequest {
            types: vec!["xhr".into(), "document".into()],
            ..request()
        });
        assert_eq!(ids(&mut log, &types), [1, 2]);
        let url = all(NetworkRequest {
            url: Some("**/api/*".into()),
            ..request()
        });
        assert_eq!(ids(&mut log, &url), [2, 3]);
        let method = all(NetworkRequest {
            method: Some("post".into()),
            ..request()
        });
        assert_eq!(ids(&mut log, &method), [2]);
        let status = all(NetworkRequest {
            status: vec![StatusRange::parse("4xx").unwrap()],
            ..request()
        });
        assert_eq!(ids(&mut log, &status), [3]);
        let limited = all(NetworkRequest {
            limit: 2,
            ..request()
        });
        let filter = Filter {
            req: &limited,
            url: None,
        };
        let picked = select(&mut log, &filter, u64::MAX);
        assert_eq!(picked.skipped, 3);
        assert_eq!(picked.requests[0].id, 4);
    }

    #[test]
    fn json_keeps_its_key_order() {
        assert_eq!(
            super::indent_json(r#"{"user":"ada", "tags":[],"o":{},"n":[1,{"a":"x,\"}"}]}"#),
            "{\n  \"user\": \"ada\",\n  \"tags\": [],\n  \"o\": {},\n  \"n\": [\n    1,\n    {\n      \"a\": \"x,\\\"}\"\n    }\n  ]\n}"
        );
    }

    #[test]
    fn status_ranges() {
        let r = StatusRange::parse("4xx").unwrap();
        assert!(r.contains(404) && !r.contains(500));
        assert_eq!(StatusRange::parse("404").unwrap().max, 404);
        assert_eq!(StatusRange::parse("500-599").unwrap().min, 500);
        for bad in ["", "abc", "0xx", "1000", "600-500", "99"] {
            assert!(StatusRange::parse(bad).is_none(), "{bad}");
        }
    }

    fn entry(id: u64) -> NetworkEntry {
        NetworkEntry {
            id,
            time_ms: 0,
            method: "POST".into(),
            url: "http://x/api/login".into(),
            kind: "xhr".into(),
            state: RequestState::Done,
            status: Some(500),
            status_text: None,
            mime: None,
            protocol: None,
            size: Some(12 * 1024),
            body_size: None,
            duration_ms: Some(340),
            cache: None,
            service_worker: false,
            initiator: None,
            redirect: None,
            error: None,
            blocked: None,
            cors: None,
            websocket: None,
        }
    }

    #[test]
    fn lines() {
        let origin = Some("http://x");
        assert_eq!(
            entry_line(&entry(42), origin),
            "#42 POST 500 /api/login xhr 12KB 340ms"
        );
        let failed = NetworkEntry {
            method: "GET".into(),
            url: "http://y/api".into(),
            kind: "fetch".into(),
            state: RequestState::Failed,
            status: None,
            size: None,
            duration_ms: Some(4),
            error: Some("net::ERR_FAILED".into()),
            cors: Some("MissingAllowOriginHeader".into()),
            ..entry(43)
        };
        assert_eq!(
            entry_line(&failed, origin),
            "#43 GET failed http://y/api fetch 4ms net::ERR_FAILED (CORS: MissingAllowOriginHeader)"
        );
        let pending = NetworkEntry {
            method: "GET".into(),
            state: RequestState::Pending,
            status: None,
            duration_ms: None,
            ..entry(44)
        };
        assert_eq!(
            entry_line(&pending, origin),
            "#44 GET pending /api/login xhr"
        );
        let ws = NetworkEntry {
            method: "GET".into(),
            url: "ws://x/ws".into(),
            kind: "websocket".into(),
            state: RequestState::Pending,
            status: Some(101),
            size: Some(10),
            duration_ms: None,
            websocket: Some(WebSocketSummary {
                sent: 1,
                sent_bytes: 5,
                received: 1,
                received_bytes: 5,
                closed: false,
            }),
            ..entry(45)
        };
        assert_eq!(
            entry_line(&ws, origin),
            "#45 GET 101 ws://x/ws websocket 10B 1 sent, 1 received, open"
        );
        let redirect = NetworkEntry {
            method: "GET".into(),
            status: Some(302),
            redirect: Some("http://x/b".into()),
            cache: Some("disk".into()),
            ..entry(46)
        };
        assert_eq!(
            entry_line(&redirect, origin),
            "#46 GET 302 /api/login xhr (disk cache) 340ms -> /b"
        );
    }
}
