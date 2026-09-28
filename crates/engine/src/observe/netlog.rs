//! A tab's network log (plan 06, milestone 6.2): a ring buffer of its
//! requests, for `network` and `network show` (and plan 06's HAR export
//! later).
//!
//! Every page and iframe session of the tab has `Network` enabled from
//! before its first request (see `tabs::setup`), and feeds its events here
//! (see `tabs::watch_host`), whether or not anyone asks. Each request, and
//! each hop of a redirect, is one record: what was asked, the response
//! (status, MIME type, sizes, timing, cache, protocol), how it failed
//! (`errorText`, `blockedReason`, CORS), and for a WebSocket its frames,
//! summed up. Records keep headers and a small request body; response
//! bodies stay in Chrome until `network show --body` asks for them.
//!
//! Records are keyed by session and request id while they are live, since
//! each session numbers its own requests.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use protocol::schema::{
    Header, Initiator, NetworkEntry, RequestState, Timing, WebSocketFrame, WebSocketSummary,
};
use serde_json::Value;

use super::format::{self, one_based};

/// How many requests a tab keeps unless `open --network-buffer` says
/// otherwise.
pub(crate) const DEFAULT_CAPACITY: usize = 2000;

/// How much of a URL is kept (a `data:` URL can be megabytes).
const URL_LEN: usize = 4000;

/// How many headers of each side a record keeps, and how much of each
/// value.
const HEADERS: usize = 100;
const HEADER_LEN: usize = 2000;

/// The longest request body a record keeps; longer ones are fetched from
/// Chrome (`Network.getRequestPostData`) when asked for.
const POST_DATA_LEN: usize = 8 * 1024;

/// How many WebSocket frames a record keeps, and how much of each text.
const FRAMES: usize = 20;
const FRAME_TEXT: usize = 200;

/// One request as the log keeps it.
#[derive(Debug, Clone)]
pub(crate) struct Record {
    pub(crate) entry: NetworkEntry,
    /// The session that sent it, and its id there (for its bodies).
    pub(crate) session: String,
    pub(crate) request_id: String,
    pub(crate) timing: Option<Timing>,
    pub(crate) request_headers: Vec<Header>,
    pub(crate) response_headers: Vec<Header>,
    /// The request body if it is short, and whether it has one at all.
    pub(crate) post_data: Option<String>,
    pub(crate) has_post_data: bool,
    pub(crate) frames: VecDeque<WebSocketFrame>,
    /// Chrome's monotonic clock (seconds) when it started, and when its
    /// response headers were in.
    started: Option<f64>,
    headers_end: Option<f64>,
}

/// The network log of one tab.
#[derive(Debug, Default)]
pub(crate) struct NetLog {
    records: VecDeque<Record>,
    /// The live records by session and request id.
    live: HashMap<(String, String), u64>,
    seq: u64,
    /// The newest record dropped to make room.
    evicted: u64,
    /// The newest record `network clear` removed.
    cleared: u64,
    /// The last record `network` showed.
    pub(crate) cursor: u64,
    /// Records `network` showed while they were pending: it shows them
    /// once more when they end.
    pub(crate) reshow: BTreeSet<u64>,
}

impl NetLog {
    /// The id of the last record.
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// The newest record dropped to make room (0 if none was).
    pub(crate) fn evicted(&self) -> u64 {
        self.evicted
    }

    /// The newest record `network clear` removed (0 if none).
    pub(crate) fn cleared(&self) -> u64 {
        self.cleared
    }

    /// The records kept, oldest first.
    pub(crate) fn records(&self) -> impl DoubleEndedIterator<Item = &Record> {
        self.records.iter()
    }

    /// Record `id`, if the log still has it.
    pub(crate) fn get(&self, id: u64) -> Option<&Record> {
        let i = self.records.partition_point(|r| r.entry.id < id);
        self.records.get(i).filter(|r| r.entry.id == id)
    }

    /// Forgets every record (ids go on from where they were).
    pub(crate) fn clear(&mut self) -> usize {
        let cleared = self.records.len();
        self.records.clear();
        self.live.clear();
        self.reshow.clear();
        self.cleared = self.seq;
        self.cursor = self.seq;
        cleared
    }

    /// Applies one `Network` event of session `session`, keeping at most
    /// `capacity` records; `false` if it changed nothing.
    pub(crate) fn event(
        &mut self,
        session: &str,
        method: &str,
        params: &Value,
        capacity: usize,
    ) -> bool {
        let Some(id) = params["requestId"].as_str() else {
            return false;
        };
        let key = (session.to_owned(), id.to_owned());
        match method {
            "Network.requestWillBeSent" => {
                if params["redirectResponse"].is_object()
                    && let Some(hop) = self.live_record(&key)
                {
                    let response = &params["redirectResponse"];
                    hop.response(response);
                    hop.entry.size = whole(&response["encodedDataLength"]);
                    hop.entry.redirect = Some(cut(text(&params["request"]["url"]), URL_LEN));
                    hop.end(params["timestamp"].as_f64());
                    hop.entry.state = RequestState::Done;
                }
                let record = request(session, id, params);
                self.push(key, record, capacity);
            }
            "Network.webSocketCreated" => {
                let mut record = Record::new(session, id, "GET", text(&params["url"]));
                "websocket".clone_into(&mut record.entry.kind);
                record.entry.initiator = initiator(&params["initiator"]);
                record.entry.websocket = Some(WebSocketSummary::default());
                self.push(key, record, capacity);
            }
            _ => {
                let Some(record) = self.live_record(&key) else {
                    return false;
                };
                if !record.apply(method, params) {
                    return false;
                }
                if record.entry.state != RequestState::Pending {
                    self.live.remove(&key);
                }
            }
        }
        true
    }

    fn live_record(&mut self, key: &(String, String)) -> Option<&mut Record> {
        let id = *self.live.get(key)?;
        let i = self.records.partition_point(|r| r.entry.id < id);
        self.records.get_mut(i).filter(|r| r.entry.id == id)
    }

    fn push(&mut self, key: (String, String), mut record: Record, capacity: usize) {
        self.seq += 1;
        record.entry.id = self.seq;
        while self.records.len() >= capacity.max(1) {
            let Some(dropped) = self.records.pop_front() else {
                break;
            };
            self.evicted = dropped.entry.id;
            let dropped_key = (dropped.session, dropped.request_id);
            if self.live.get(&dropped_key) == Some(&dropped.entry.id) {
                self.live.remove(&dropped_key);
            }
            self.reshow.remove(&dropped.entry.id);
        }
        self.live.insert(key, self.seq);
        self.records.push_back(record);
    }
}

impl Record {
    fn new(session: &str, request_id: &str, method: &str, url: String) -> Self {
        Self {
            entry: NetworkEntry {
                id: 0,
                time_ms: now_ms(),
                method: method.to_owned(),
                url: cut(url, URL_LEN),
                kind: "other".to_owned(),
                state: RequestState::Pending,
                status: None,
                status_text: None,
                mime: None,
                protocol: None,
                size: None,
                body_size: None,
                duration_ms: None,
                cache: None,
                service_worker: false,
                initiator: None,
                redirect: None,
                error: None,
                blocked: None,
                cors: None,
                websocket: None,
            },
            session: session.to_owned(),
            request_id: request_id.to_owned(),
            timing: None,
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            post_data: None,
            has_post_data: false,
            frames: VecDeque::new(),
            started: None,
            headers_end: None,
        }
    }

    /// Applies an event about this record; `false` if it is not one the
    /// log follows.
    fn apply(&mut self, method: &str, params: &Value) -> bool {
        match method {
            "Network.requestServedFromCache" => {
                self.entry.cache.get_or_insert_with(|| "memory".to_owned());
            }
            "Network.responseReceived" => {
                self.response(&params["response"]);
                if let Some(kind) = params["type"].as_str() {
                    self.entry.kind = kind.to_ascii_lowercase();
                }
            }
            "Network.dataReceived" => {
                let data = params["dataLength"].as_u64().unwrap_or(0);
                *self.entry.body_size.get_or_insert(0) += data;
            }
            "Network.loadingFinished" => {
                self.entry.size = whole(&params["encodedDataLength"]);
                self.end(params["timestamp"].as_f64());
                self.entry.state = RequestState::Done;
            }
            "Network.loadingFailed" => {
                self.end(params["timestamp"].as_f64());
                let canceled = params["canceled"].as_bool() == Some(true);
                self.entry.state = if canceled {
                    RequestState::Canceled
                } else {
                    RequestState::Failed
                };
                self.entry.error = params["errorText"]
                    .as_str()
                    .filter(|e| !e.is_empty())
                    .map(str::to_owned);
                self.entry.blocked = params["blockedReason"].as_str().map(str::to_owned);
                let cors = &params["corsErrorStatus"];
                self.entry.cors = cors["corsError"].as_str().map(|error| {
                    match cors["failedParameter"].as_str().filter(|p| !p.is_empty()) {
                        Some(parameter) => format!("{error} ({parameter})"),
                        None => error.to_owned(),
                    }
                });
            }
            "Network.webSocketWillSendHandshakeRequest" => {
                self.started = params["timestamp"].as_f64();
                self.request_headers = headers(&params["request"]["headers"]);
            }
            "Network.webSocketHandshakeResponseReceived" => {
                let response = &params["response"];
                self.entry.status = status(&response["status"]);
                self.entry.status_text = non_empty(&response["statusText"]);
                self.response_headers = headers(&response["headers"]);
            }
            "Network.webSocketFrameSent" | "Network.webSocketFrameReceived" => {
                self.frame(method == "Network.webSocketFrameSent", &params["response"]);
            }
            "Network.webSocketFrameError" => {
                self.entry.error = non_empty(&params["errorMessage"]);
            }
            "Network.webSocketClosed" => {
                self.end(params["timestamp"].as_f64());
                if let Some(ws) = &mut self.entry.websocket {
                    ws.closed = true;
                }
                self.entry.state = if self.entry.status.is_none() && self.entry.error.is_some() {
                    RequestState::Failed
                } else {
                    RequestState::Done
                };
            }
            _ => return false,
        }
        true
    }

    /// Takes in a `Network.Response`.
    fn response(&mut self, response: &Value) {
        self.entry.status = status(&response["status"]);
        self.entry.status_text = non_empty(&response["statusText"]);
        self.entry.mime = non_empty(&response["mimeType"]);
        self.entry.protocol = non_empty(&response["protocol"]);
        self.entry.service_worker = response["fromServiceWorker"].as_bool() == Some(true);
        if response["fromDiskCache"].as_bool() == Some(true) {
            self.entry.cache = Some("disk".to_owned());
        } else if response["fromPrefetchCache"].as_bool() == Some(true) {
            self.entry.cache = Some("prefetch".to_owned());
        }
        self.response_headers = headers(&response["headers"]);
        // The headers Chrome actually sent (with cookies), if it says.
        if let Some(sent) = response["requestHeaders"].as_object()
            && !sent.is_empty()
        {
            self.request_headers = headers(&response["requestHeaders"]);
        }
        let timing = &response["timing"];
        if let Some(request_time) = timing["requestTime"].as_f64() {
            self.timing = Some(phases(timing, request_time, self.started));
            let headers_end = timing["receiveHeadersEnd"].as_f64().unwrap_or(0.0);
            self.headers_end = Some(request_time + headers_end / 1000.0);
        }
    }

    /// It ended (or was redirected) at `timestamp` (Chrome's clock).
    fn end(&mut self, timestamp: Option<f64>) {
        let Some(end) = timestamp else {
            return;
        };
        if let Some(start) = self.started {
            self.entry.duration_ms = Some(ms(end - start));
        }
        if let (Some(timing), Some(headers_end)) = (&mut self.timing, self.headers_end) {
            timing.receive_ms = Some(ms(end - headers_end));
        }
    }

    fn frame(&mut self, sent: bool, frame: &Value) {
        let payload = frame["payloadData"].as_str().unwrap_or_default();
        let opcode = whole(&frame["opcode"]).unwrap_or(1);
        let (opcode, size, text) = match opcode {
            1 => ("text", payload.len(), Some(payload)),
            2 => ("binary", base64_len(payload), None),
            8 => ("close", payload.len(), None),
            9 => ("ping", payload.len(), None),
            10 => ("pong", payload.len(), None),
            _ => ("other", payload.len(), None),
        };
        let size = size as u64;
        if let Some(ws) = &mut self.entry.websocket {
            if sent {
                ws.sent += 1;
                ws.sent_bytes += size;
            } else {
                ws.received += 1;
                ws.received_bytes += size;
            }
            self.entry.size = Some(ws.sent_bytes + ws.received_bytes);
        }
        if self.frames.len() >= FRAMES {
            self.frames.pop_front();
        }
        self.frames.push_back(WebSocketFrame {
            sent,
            time_ms: now_ms(),
            opcode: opcode.to_owned(),
            size,
            text: text.map(|t| cut(t.to_owned(), FRAME_TEXT)),
        });
    }
}

/// A record from `Network.requestWillBeSent`.
fn request(session: &str, id: &str, params: &Value) -> Record {
    let request = &params["request"];
    let mut url = text(&request["url"]);
    if let Some(fragment) = request["urlFragment"].as_str() {
        url.push_str(fragment);
    }
    let mut record = Record::new(
        session,
        id,
        request["method"].as_str().unwrap_or("GET"),
        url,
    );
    record.entry.kind = params["type"]
        .as_str()
        .unwrap_or("Other")
        .to_ascii_lowercase();
    record.entry.initiator = initiator(&params["initiator"]);
    record.request_headers = headers(&request["headers"]);
    record.has_post_data = request["hasPostData"].as_bool() == Some(true);
    record.post_data = request["postData"]
        .as_str()
        .filter(|d| d.len() <= POST_DATA_LEN)
        .map(str::to_owned);
    record.started = params["timestamp"].as_f64();
    record
}

/// What sent a request, from its `Network.Initiator`.
fn initiator(initiator: &Value) -> Option<Initiator> {
    let kind = initiator["type"].as_str()?;
    // The innermost frame, of the stack or of the async stack it
    // continues.
    let mut stack = &initiator["stack"];
    let mut frame = None;
    while stack.is_object() && frame.is_none() {
        frame = stack["callFrames"].as_array().and_then(|f| f.first());
        stack = &stack["parent"];
    }
    let (url, line, function) = match frame {
        Some(frame) => (
            non_empty(&frame["url"]),
            one_based(&frame["lineNumber"]),
            non_empty(&frame["functionName"]),
        ),
        None => (
            non_empty(&initiator["url"]),
            one_based(&initiator["lineNumber"]),
            None,
        ),
    };
    Some(Initiator {
        kind: kind.to_owned(),
        url,
        line,
        function,
    })
}

/// A request's phases from its `Network.ResourceTiming` (ms after
/// `request_time`; -1 for a phase that did not happen).
fn phases(timing: &Value, request_time: f64, started: Option<f64>) -> Timing {
    let at = |name: &str| timing[name].as_f64().filter(|v| *v >= 0.0);
    let span = |start: &str, end: &str| match (at(start), at(end)) {
        (Some(s), Some(e)) => ms((e - s) / 1000.0),
        _ => 0,
    };
    let first = ["dnsStart", "connectStart", "sendStart"]
        .into_iter()
        .find_map(at)
        .unwrap_or(0.0);
    let queued = started.map_or(0.0, |s| (request_time - s).max(0.0));
    Timing {
        blocked_ms: ms(queued + first / 1000.0),
        dns_ms: span("dnsStart", "dnsEnd"),
        connect_ms: span("connectStart", "connectEnd"),
        tls_ms: span("sslStart", "sslEnd"),
        send_ms: span("sendStart", "sendEnd"),
        wait_ms: span("sendEnd", "receiveHeadersEnd"),
        receive_ms: None,
    }
}

/// Headers from a `Network.Headers` object, capped.
fn headers(headers: &Value) -> Vec<Header> {
    let Some(headers) = headers.as_object() else {
        return Vec::new();
    };
    headers
        .iter()
        .take(HEADERS)
        .map(|(name, value)| Header {
            name: name.clone(),
            value: cut(
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned),
                HEADER_LEN,
            ),
        })
        .collect()
}

/// Seconds to whole milliseconds (0 if negative).
fn ms(seconds: f64) -> u64 {
    round(seconds * 1000.0)
}

/// A CDP number (a JSON float at times) as a count (0 if negative).
fn whole(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_f64().map(round))
}

// Durations and sizes fit well within u64.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round(n: f64) -> u64 {
    n.max(0.0).round() as u64
}

fn status(value: &Value) -> Option<u16> {
    whole(value).and_then(|s| u16::try_from(s).ok())
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn non_empty(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn cut(mut text: String, max: usize) -> String {
    format::cut(&mut text, max);
    text
}

/// The size of the data base64 text `b64` encodes.
fn base64_len(b64: &str) -> usize {
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_or(b64.len() * 3 / 4, |d| d.len())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use protocol::schema::RequestState;
    use serde_json::{Value, json};

    use super::NetLog;

    const CAP: usize = 100;

    fn sent(log: &mut NetLog, id: &str, method: &str, url: &str, kind: &str, at: f64) {
        let params = json!({
            "requestId": id,
            "type": kind,
            "timestamp": at,
            "request": { "method": method, "url": url, "headers": { "Accept": "*/*" } },
            "initiator": { "type": "script", "stack": { "callFrames": [
                { "functionName": "save", "url": "http://x/app.js", "lineNumber": 11, "columnNumber": 2 },
            ] } },
        });
        assert!(log.event("s1", "Network.requestWillBeSent", &params, CAP));
    }

    fn event(log: &mut NetLog, method: &str, params: &Value) -> bool {
        log.event("s1", method, params, CAP)
    }

    #[test]
    fn a_request_from_start_to_end() {
        let mut log = NetLog::default();
        sent(&mut log, "1", "POST", "http://x/api/login", "XHR", 10.0);
        assert!(event(
            &mut log,
            "Network.responseReceived",
            &json!({
                "requestId": "1",
                "type": "XHR",
                "response": {
                    "url": "http://x/api/login", "status": 500, "statusText": "Internal Server Error",
                    "mimeType": "application/json", "protocol": "http/1.1",
                    "headers": { "Content-Type": "application/json" },
                    "timing": {
                        "requestTime": 10.01, "dnsStart": -1, "dnsEnd": -1,
                        "connectStart": 1.0, "connectEnd": 3.0, "sslStart": -1, "sslEnd": -1,
                        "sendStart": 3.0, "sendEnd": 4.0, "receiveHeadersEnd": 304.0,
                    },
                },
            }),
        ));
        event(
            &mut log,
            "Network.dataReceived",
            &json!({ "requestId": "1", "dataLength": 3000 }),
        );
        assert!(event(
            &mut log,
            "Network.loadingFinished",
            &json!({ "requestId": "1", "timestamp": 10.34, "encodedDataLength": 12288 }),
        ));
        let record = log.get(1).unwrap();
        let entry = &record.entry;
        assert_eq!(entry.state, RequestState::Done);
        assert_eq!(
            (entry.method.as_str(), entry.status, entry.kind.as_str()),
            ("POST", Some(500), "xhr")
        );
        assert_eq!(entry.size, Some(12288));
        assert_eq!(entry.body_size, Some(3000));
        assert_eq!(entry.duration_ms, Some(340));
        let initiator = entry.initiator.as_ref().unwrap();
        assert_eq!(initiator.function.as_deref(), Some("save"));
        assert_eq!(initiator.line, Some(12));
        let timing = record.timing.as_ref().unwrap();
        assert_eq!((timing.blocked_ms, timing.connect_ms), (11, 2));
        assert_eq!((timing.wait_ms, timing.receive_ms), (300, Some(26)));
        assert_eq!(record.request_headers[0].name, "Accept");
        // Its end made it no longer live: a late event changes nothing.
        assert!(!event(
            &mut log,
            "Network.dataReceived",
            &json!({ "requestId": "1", "dataLength": 1 }),
        ));
    }

    #[test]
    fn redirects_failures_and_cancels() {
        let mut log = NetLog::default();
        sent(&mut log, "1", "GET", "http://x/old", "Document", 1.0);
        let hop = json!({
            "requestId": "1",
            "type": "Document",
            "timestamp": 1.05,
            "request": { "method": "GET", "url": "http://x/new" },
            "redirectResponse": { "url": "http://x/old", "status": 302, "encodedDataLength": 120 },
        });
        assert!(event(&mut log, "Network.requestWillBeSent", &hop));
        sent(&mut log, "2", "GET", "http://127.0.0.1:1/x", "Fetch", 2.0);
        event(
            &mut log,
            "Network.loadingFailed",
            &json!({
                "requestId": "2", "timestamp": 2.003, "errorText": "net::ERR_FAILED",
                "corsErrorStatus": { "corsError": "MissingAllowOriginHeader", "failedParameter": "" },
            }),
        );
        sent(&mut log, "3", "GET", "http://x/slow", "Fetch", 3.0);
        event(
            &mut log,
            "Network.loadingFailed",
            &json!({ "requestId": "3", "timestamp": 3.1, "errorText": "net::ERR_ABORTED", "canceled": true }),
        );
        let entries: Vec<_> = log.records().map(|r| &r.entry).collect();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].status, Some(302));
        assert_eq!(entries[0].redirect.as_deref(), Some("http://x/new"));
        assert_eq!(entries[0].duration_ms, Some(50));
        assert_eq!(entries[1].url, "http://x/new");
        assert_eq!(entries[1].state, RequestState::Pending);
        assert_eq!(entries[2].state, RequestState::Failed);
        assert_eq!(entries[2].cors.as_deref(), Some("MissingAllowOriginHeader"));
        assert_eq!(entries[3].state, RequestState::Canceled);
    }

    #[test]
    fn websockets_are_summed_up() {
        let mut log = NetLog::default();
        event(
            &mut log,
            "Network.webSocketCreated",
            &json!({ "requestId": "9", "url": "ws://x/ws", "initiator": { "type": "script" } }),
        );
        event(
            &mut log,
            "Network.webSocketWillSendHandshakeRequest",
            &json!({ "requestId": "9", "timestamp": 5.0, "request": { "headers": {} } }),
        );
        event(
            &mut log,
            "Network.webSocketHandshakeResponseReceived",
            &json!({ "requestId": "9", "timestamp": 5.01, "response": { "status": 101, "statusText": "Switching Protocols", "headers": {} } }),
        );
        for (method, payload) in [
            ("Network.webSocketFrameSent", "hello"),
            ("Network.webSocketFrameReceived", "hello"),
        ] {
            event(
                &mut log,
                method,
                &json!({ "requestId": "9", "timestamp": 5.1, "response": { "opcode": 1, "mask": true, "payloadData": payload } }),
            );
        }
        event(
            &mut log,
            "Network.webSocketFrameReceived",
            &json!({ "requestId": "9", "timestamp": 5.2, "response": { "opcode": 2, "mask": false, "payloadData": "AAEC" } }),
        );
        let record = log.get(1).unwrap();
        assert_eq!(record.entry.state, RequestState::Pending);
        let ws = record.entry.websocket.as_ref().unwrap();
        assert_eq!((ws.sent, ws.received, ws.received_bytes), (1, 2, 8));
        assert_eq!(record.frames[2].opcode, "binary");
        assert_eq!(record.frames[2].size, 3);
        event(
            &mut log,
            "Network.webSocketClosed",
            &json!({ "requestId": "9", "timestamp": 6.0 }),
        );
        let entry = &log.get(1).unwrap().entry;
        assert_eq!(entry.state, RequestState::Done);
        assert_eq!(entry.duration_ms, Some(1000));
        assert!(entry.websocket.as_ref().unwrap().closed);
    }

    #[test]
    fn the_log_is_bounded_and_clears() {
        let mut log = NetLog::default();
        for i in 0..5 {
            let params = json!({
                "requestId": i.to_string(),
                "type": "Image",
                "timestamp": 1.0,
                "request": { "method": "GET", "url": format!("http://x/{i}.png") },
            });
            log.event("s1", "Network.requestWillBeSent", &params, 3);
        }
        let ids: Vec<u64> = log.records().map(|r| r.entry.id).collect();
        assert_eq!(ids, [3, 4, 5]);
        assert_eq!(log.evicted(), 2);
        assert!(log.get(2).is_none());
        // The evicted request's events are ignored.
        assert!(!event(
            &mut log,
            "Network.loadingFinished",
            &json!({ "requestId": "0", "timestamp": 2.0, "encodedDataLength": 1 }),
        ));
        assert_eq!(log.clear(), 3);
        assert_eq!((log.cleared(), log.cursor, log.seq()), (5, 5, 5));
        assert!(log.records().next().is_none());
    }
}
