//! `har start` and `har stop` (plan 06, milestone 6.5): a HAR 1.2 file
//! built from the network log.
//!
//! Every tab records its requests anyway (see [`super::netlog`]), so
//! `har start` only notes where each tab's log is, and `har stop` writes
//! the records after that mark: of every tab (tabs opened meanwhile from
//! their first request), or of the one `--tab` named. Response bodies are
//! asked of Chrome at `har stop` (`--content embed`, the default); Chrome
//! keeps them while its buffer has room and their page is open, and the
//! ones it no longer has are left out and counted. The log is a ring
//! buffer, so a recording longer than it (`open --network-buffer`) loses
//! its oldest requests; `har stop` says how many.
//!
//! The file follows <http://www.softwareishard.com/blog/har-12-spec/>,
//! with DevTools' `_resourceType`, `_transferSize`, `_error` and
//! `_webSocketMessages` extensions. Each tab is a page.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::network as cdp_network;
use protocol::schema::{HarStartData, HarStopData, Header, RequestState};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde::Serialize;

use super::netlog::Record;
use super::save::save;
use crate::request::{HarContent, HarStartRequest, HarStopRequest, millis};
use crate::tabs::{parse_tab_id, tab_name};
use crate::{Engine, Running};

/// How many bodies are asked of Chrome at a time.
const BODY_CALLS: usize = 8;

/// A running `har start`.
#[derive(Debug, Clone)]
pub(crate) struct HarRecording {
    content: HarContent,
    /// `--tab`: only this tab.
    tab: Option<u32>,
    started_ms: u64,
    /// Each tab's newest request id at the start; tabs not here opened
    /// later and count from their first.
    marks: HashMap<u32, u64>,
}

impl Engine {
    /// `har start`: notes where each tab's network log is.
    pub async fn har_start(&self, req: HarStartRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let tab = match req.tab.as_deref() {
            Some(tab) => {
                let page = running
                    .tabs
                    .page(Some(parse_tab_id(tab)?), millis(req.timeout_ms))
                    .await
                    .map_err(|e| self.tab_error(&running, e))?;
                Some(page.id)
            }
            None => None,
        };
        let mut recording = lock(&running.har);
        let already = match &mut *recording {
            Some(current) if current.tab == tab => {
                current.content = req.content;
                true
            }
            _ => {
                let marks = running
                    .tabs
                    .tab_ids()
                    .into_iter()
                    .filter_map(|id| Some((id, running.tabs.netlog(id, |log| log.seq())?)))
                    .collect();
                *recording = Some(HarRecording {
                    content: req.content,
                    tab,
                    started_ms: now_ms(),
                    marks,
                });
                false
            }
        };
        drop(recording);
        let which = tab.map_or_else(
            || "every tab (and tabs opened from now on)".to_owned(),
            tab_name,
        );
        let bodies = match req.content {
            HarContent::Embed => "with response bodies",
            HarContent::Omit => "without response bodies",
        };
        let stop = Cmd::new("har stop")
            .raw("-o")
            .arg("requests.har")
            .session(&self.session);
        let text = if already {
            format!("still recording a HAR of {which}, {bodies}; {stop} saves it")
        } else {
            format!("recording a HAR of {which}, {bodies}; {stop} saves it")
        };
        let data = HarStartData {
            tab: tab.map(tab_name),
            content: req.content.as_str().to_owned(),
            already,
        };
        Ok(Output::typed(&data, text).quiet(req.content.as_str()))
    }

    /// `har stop`: writes what the network log has recorded since `har
    /// start`, and ends the recording.
    pub async fn har_stop(&self, req: HarStopRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let Some(recording) = lock(&running.har).clone() else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("session {} is not recording a HAR", self.session),
            )
            .hint(
                Hint::run(Cmd::new("har").arg("start").session(&self.session))
                    .why("then act, then har stop"),
            ));
        };
        let timeout = millis(req.timeout_ms);
        let mut pages = Vec::new();
        let mut records = Vec::new();
        let mut dropped = 0;
        let ids = match recording.tab {
            Some(tab) => vec![tab],
            None => running.tabs.tab_ids(),
        };
        for id in ids {
            let mark = recording.marks.get(&id).copied().unwrap_or(0);
            let Some((mut taken, lost)) = running.tabs.netlog(id, |log| {
                let taken: Vec<Record> = log
                    .records()
                    .filter(|r| r.entry.id > mark)
                    .cloned()
                    .collect();
                (taken, log.evicted().max(log.cleared()).saturating_sub(mark))
            }) else {
                continue;
            };
            dropped += lost;
            if taken.is_empty() {
                continue;
            }
            let title = running
                .tabs
                .info(id)
                .map(|info| {
                    if info.title.is_empty() {
                        info.url
                    } else {
                        info.title
                    }
                })
                .unwrap_or_default();
            let first = taken.iter().map(|r| r.entry.time_ms).min().unwrap_or(0);
            pages.push(HarPage {
                started_date_time: iso8601(first),
                id: tab_name(id),
                title,
                page_timings: PageTimings {},
            });
            records.extend(taken.drain(..).map(|r| (id, r)));
        }
        records.sort_by_key(|(_, r)| (r.entry.time_ms, r.entry.id));
        let embed = recording.content == HarContent::Embed;
        let mut fetched: Vec<Fetched> = Vec::with_capacity(records.len());
        for chunk in records.chunks(BODY_CALLS) {
            let mut calls = Vec::with_capacity(chunk.len());
            for (_, record) in chunk {
                calls.push(self.har_bodies(&running, record, embed, timeout));
            }
            fetched.extend(futures_util::future::join_all(calls).await);
        }
        let mut bodies = 0;
        let mut missing = 0;
        let mut pending = 0;
        let entries: Vec<HarEntry> = records
            .iter()
            .zip(fetched)
            .map(|((id, record), fetched)| {
                match &fetched.response {
                    Some(Ok(_)) => bodies += 1,
                    Some(Err(())) => missing += 1,
                    None => {}
                }
                if record.entry.state == RequestState::Pending {
                    pending += 1;
                }
                entry(*id, record, fetched)
            })
            .collect();
        let entry_count = entries.len();
        let product = &running.info.product;
        let (browser, version) = product.split_once('/').unwrap_or((product, ""));
        let har = Har {
            log: HarLog {
                version: "1.2",
                creator: Creator {
                    name: "chrome-cdp".to_owned(),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                },
                browser: Creator {
                    name: browser.to_owned(),
                    version: version.to_owned(),
                },
                pages,
                entries,
            },
        };
        let json = serde_json::to_vec_pretty(&har).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("cannot write the HAR: {e}"),
            )
        })?;
        let path = save(
            &json,
            req.path.as_deref(),
            &self.output_dir().join("hars"),
            "har",
        )?;
        let page_names: Vec<String> = har.log.pages.iter().map(|p| p.id.clone()).collect();
        // Saved: the recording is over (a failed save leaves it running,
        // so it can be saved elsewhere).
        *lock(&running.har) = None;
        let path_text = path.display().to_string();
        let mut text = format!(
            "{path_text}: {} {}",
            format::thousands(entry_count),
            if entry_count == 1 {
                "request"
            } else {
                "requests"
            }
        );
        if !page_names.is_empty() {
            let _ = write!(text, " from {}", page_names.join(", "));
        }
        if embed {
            let _ = write!(
                text,
                ", {} {} embedded",
                format::thousands(bodies),
                if bodies == 1 { "body" } else { "bodies" }
            );
        }
        let _ = write!(text, ", {}", format::size(json.len() as u64));
        let duration_ms = now_ms().saturating_sub(recording.started_ms);
        let data = HarStopData {
            path: path_text.clone(),
            bytes: json.len() as u64,
            entries: entry_count,
            pages: page_names,
            content: recording.content.as_str().to_owned(),
            bodies,
            missing_bodies: missing,
            dropped,
            duration_ms,
        };
        let mut output = Output::typed(&data, text).quiet(path_text);
        if dropped > 0 {
            output = output.warning(format!(
                "{dropped} {} dropped from the network log (or cleared) before har stop; a tab \
                 keeps its newest {} (open --network-buffer N keeps more)",
                if dropped == 1 {
                    "request was"
                } else {
                    "requests were"
                },
                self.settings.network_buffer(),
            ));
        }
        if missing > 0 {
            output = output.warning(format!(
                "{missing} response {} left out: Chrome keeps bodies only while its buffer has \
                 room and the page that loaded them is open",
                if missing == 1 {
                    "body is"
                } else {
                    "bodies are"
                },
            ));
        }
        if pending > 0 {
            output = output.warning(format!(
                "{pending} {} still loading, and in the file without an end (a fetch ends once \
                 the page reads its body)",
                if pending == 1 {
                    "request was"
                } else {
                    "requests were"
                },
            ));
        }
        Ok(output)
    }

    /// The bodies of `record` the HAR gets: the request body if it has
    /// one, and with `embed` the response body.
    async fn har_bodies(
        &self,
        running: &Running,
        record: &Record,
        embed: bool,
        timeout: Duration,
    ) -> Fetched {
        let session = running.tabs.host_session(&record.session);
        let mut request = record.post_data.clone();
        if request.is_none()
            && record.has_post_data
            && embed
            && let Some(session) = &session
        {
            let params = cdp_network::GetRequestPostDataParams::new(record.request_id.clone());
            if let Ok(data) = session.with_timeout(timeout).call(params).await {
                request = Some(data.post_data);
            }
        }
        let entry = &record.entry;
        let has_body = embed
            && entry.state == RequestState::Done
            && entry.websocket.is_none()
            && entry.redirect.is_none();
        let response = if has_body {
            Some(match &session {
                Some(session) => {
                    let params = cdp_network::GetResponseBodyParams::new(record.request_id.clone());
                    match session.with_timeout(timeout).call(params).await {
                        Ok(body) => Ok((body.body, body.base64_encoded)),
                        Err(_) => Err(()),
                    }
                }
                None => Err(()),
            })
        } else {
            None
        };
        Fetched { request, response }
    }
}

/// A record's bodies as fetched for the HAR: the response body is `None`
/// when none was asked for, `Err` when Chrome no longer had it, and
/// `Ok((text, base64))`.
struct Fetched {
    request: Option<String>,
    response: Option<Result<(String, bool), ()>>,
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One HAR entry from `record` of tab `tab`.
// Times in ms and s fit well within f64.
#[allow(clippy::cast_precision_loss)]
fn entry(tab: u32, record: &Record, fetched: Fetched) -> HarEntry {
    let e = &record.entry;
    let http_version = e.protocol.as_deref().map(http_version).unwrap_or_default();
    let request_mime = header(&record.request_headers, "content-type").unwrap_or_default();
    let post_data = fetched.request.map(|text| PostData {
        mime_type: request_mime.to_owned(),
        text,
    });
    let request_body_size = match &post_data {
        Some(p) => signed(p.text.len() as u64),
        None if record.has_post_data => -1,
        None => 0,
    };
    let mut content = Content {
        size: e.body_size.map_or(0, signed),
        mime_type: e.mime.clone().unwrap_or_default(),
        text: None,
        encoding: None,
    };
    if let Some(Ok((text, base64))) = fetched.response {
        if !base64 && e.body_size.is_none() {
            content.size = signed(text.len() as u64);
        }
        content.text = Some(text);
        content.encoding = base64.then(|| "base64".to_owned());
    }
    let timings = record.timing.as_ref().map_or(
        Timings {
            blocked: -1,
            dns: -1,
            connect: -1,
            ssl: -1,
            send: 0,
            wait: 0,
            receive: 0,
        },
        |t| Timings {
            blocked: signed(t.blocked_ms),
            dns: signed(t.dns_ms),
            connect: signed(t.connect_ms),
            ssl: signed(t.tls_ms),
            send: signed(t.send_ms),
            wait: signed(t.wait_ms),
            receive: signed(t.receive_ms.unwrap_or(0)),
        },
    );
    let error = match (&e.error, &e.blocked, &e.cors) {
        (_, _, Some(cors)) => Some(format!("CORS: {cors}")),
        (_, Some(blocked), _) => Some(format!("blocked: {blocked}")),
        (Some(error), _, _) => Some(error.clone()),
        _ if e.state == RequestState::Canceled => Some("canceled".to_owned()),
        _ => None,
    };
    let messages = e.websocket.as_ref().map(|_| {
        record
            .frames
            .iter()
            .map(|f| WebSocketMessage {
                kind: if f.sent { "send" } else { "receive" },
                time: f.time_ms as f64 / 1000.0,
                opcode: match f.opcode.as_str() {
                    "text" => 1,
                    "binary" => 2,
                    "close" => 8,
                    "ping" => 9,
                    "pong" => 10,
                    _ => 0,
                },
                data: f.text.clone().unwrap_or_default(),
            })
            .collect()
    });
    HarEntry {
        pageref: tab_name(tab),
        started_date_time: iso8601(e.time_ms),
        time: e.duration_ms.unwrap_or(0) as f64,
        request: HarRequest {
            method: e.method.clone(),
            url: e.url.clone(),
            http_version: http_version.clone(),
            cookies: request_cookies(&record.request_headers),
            headers: headers(&record.request_headers),
            query_string: query(&e.url),
            post_data,
            headers_size: -1,
            body_size: request_body_size,
        },
        response: HarResponse {
            status: e.status.unwrap_or(0),
            status_text: e.status_text.clone().unwrap_or_default(),
            http_version,
            cookies: response_cookies(&record.response_headers),
            headers: headers(&record.response_headers),
            content,
            redirect_url: e.redirect.clone().unwrap_or_default(),
            headers_size: -1,
            body_size: -1,
            transfer_size: e.size,
            error,
        },
        cache: Cache {},
        timings,
        resource_type: e.kind.clone(),
        web_socket_messages: messages,
    }
}

/// `h2` → `HTTP/2`, `http/1.1` → `HTTP/1.1`, as HAR viewers expect.
fn http_version(protocol: &str) -> String {
    match protocol.to_ascii_lowercase().as_str() {
        "h2" | "http/2" | "http/2.0" => "HTTP/2".to_owned(),
        "h3" | "http/3" => "HTTP/3".to_owned(),
        p if p.starts_with("http/") => p.to_ascii_uppercase(),
        _ => protocol.to_owned(),
    }
}

fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

fn headers(headers: &[Header]) -> Vec<NameValue> {
    // Chrome joins repeated headers (Set-Cookie) with newlines.
    headers
        .iter()
        .flat_map(|h| {
            h.value.split('\n').map(|value| NameValue {
                name: h.name.clone(),
                value: value.to_owned(),
            })
        })
        .collect()
}

fn query(url: &str) -> Vec<NameValue> {
    url::Url::parse(url).map_or_else(
        |_| Vec::new(),
        |url| {
            url.query_pairs()
                .map(|(name, value)| NameValue {
                    name: name.into_owned(),
                    value: value.into_owned(),
                })
                .collect()
        },
    )
}

fn request_cookies(headers: &[Header]) -> Vec<Cookie> {
    header(headers, "cookie")
        .into_iter()
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            Some(Cookie {
                name: name.to_owned(),
                value: value.to_owned(),
                ..Cookie::default()
            })
        })
        .collect()
}

fn response_cookies(headers: &[Header]) -> Vec<Cookie> {
    header(headers, "set-cookie")
        .into_iter()
        .flat_map(|value| value.split('\n'))
        .filter_map(|line| {
            let mut parts = line.split(';');
            let (name, value) = parts.next()?.trim().split_once('=')?;
            let mut cookie = Cookie {
                name: name.to_owned(),
                value: value.to_owned(),
                ..Cookie::default()
            };
            for attribute in parts {
                let (key, value) = attribute
                    .trim()
                    .split_once('=')
                    .unwrap_or((attribute.trim(), ""));
                match key.to_ascii_lowercase().as_str() {
                    "path" => cookie.path = Some(value.to_owned()),
                    "domain" => cookie.domain = Some(value.to_owned()),
                    "expires" => cookie.expires = Some(value.to_owned()),
                    "httponly" => cookie.http_only = Some(true),
                    "secure" => cookie.secure = Some(true),
                    _ => {}
                }
            }
            Some(cookie)
        })
        .collect()
}

/// Unix ms as ISO 8601 in UTC: `2026-09-28T10:04:05.123Z`.
fn iso8601(ms: u64) -> String {
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rest = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60,
        ms % 1000
    )
}

/// A size or duration as HAR's signed number.
fn signed(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

// The HAR 1.2 document, in the spec's field order.

#[derive(Serialize)]
struct Har {
    log: HarLog,
}

#[derive(Serialize)]
struct HarLog {
    version: &'static str,
    creator: Creator,
    browser: Creator,
    pages: Vec<HarPage>,
    entries: Vec<HarEntry>,
}

#[derive(Serialize)]
struct Creator {
    name: String,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HarPage {
    started_date_time: String,
    id: String,
    title: String,
    page_timings: PageTimings,
}

/// No `onContentLoad`/`onLoad`: they are per navigation, and a page here
/// is a tab.
#[derive(Serialize)]
struct PageTimings {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HarEntry {
    pageref: String,
    started_date_time: String,
    time: f64,
    request: HarRequest,
    response: HarResponse,
    cache: Cache,
    timings: Timings,
    #[serde(rename = "_resourceType")]
    resource_type: String,
    #[serde(rename = "_webSocketMessages", skip_serializing_if = "Option::is_none")]
    web_socket_messages: Option<Vec<WebSocketMessage>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HarRequest {
    method: String,
    url: String,
    http_version: String,
    cookies: Vec<Cookie>,
    headers: Vec<NameValue>,
    query_string: Vec<NameValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_data: Option<PostData>,
    headers_size: i64,
    body_size: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HarResponse {
    status: u16,
    status_text: String,
    http_version: String,
    cookies: Vec<Cookie>,
    headers: Vec<NameValue>,
    content: Content,
    #[serde(rename = "redirectURL")]
    redirect_url: String,
    headers_size: i64,
    body_size: i64,
    #[serde(rename = "_transferSize", skip_serializing_if = "Option::is_none")]
    transfer_size: Option<u64>,
    #[serde(rename = "_error", skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Cookie {
    name: String,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secure: Option<bool>,
}

#[derive(Serialize)]
struct NameValue {
    name: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PostData {
    mime_type: String,
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Content {
    size: i64,
    mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding: Option<String>,
}

#[derive(Serialize)]
struct Cache {}

#[derive(Serialize)]
struct Timings {
    blocked: i64,
    dns: i64,
    connect: i64,
    send: i64,
    wait: i64,
    receive: i64,
    ssl: i64,
}

#[derive(Serialize)]
struct WebSocketMessage {
    #[serde(rename = "type")]
    kind: &'static str,
    time: f64,
    opcode: u8,
    data: String,
}

#[cfg(test)]
mod tests {
    use protocol::schema::Header;

    use super::{http_version, iso8601, query, response_cookies};

    #[test]
    fn dates_are_iso_8601_utc() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(951_782_400_123), "2000-02-29T00:00:00.123Z");
        assert_eq!(iso8601(1_790_586_245_007), "2026-09-28T09:04:05.007Z");
    }

    #[test]
    fn versions_queries_and_cookies() {
        assert_eq!(http_version("h2"), "HTTP/2");
        assert_eq!(http_version("http/1.1"), "HTTP/1.1");
        let q = query("http://x/items?n=2&q=a%20b");
        assert_eq!((q[1].name.as_str(), q[1].value.as_str()), ("q", "a b"));
        let cookies = response_cookies(&[Header {
            name: "Set-Cookie".to_owned(),
            value: "sid=1; Path=/; HttpOnly\ntheme=dark".to_owned(),
        }]);
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].path.as_deref(), Some("/"));
        assert_eq!(cookies[0].http_only, Some(true));
        assert_eq!(cookies[1].value, "dark");
    }
}
