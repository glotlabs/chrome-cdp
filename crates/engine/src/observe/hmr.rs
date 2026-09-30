//! A tab's dev-server hot updates (plan 15, milestone 15.6): did the
//! page get the agent's last edit?
//!
//! Nothing is injected. Each tab's [`HotUpdates`] reads what the tab
//! already sees: its WebSockets (fed with the network log's events, see
//! `tabs::network_event`), its console (`tabs::console_event`) and its
//! main frame's documents (`tabs::frame_event`). It knows three clients:
//!
//! - **Vite**: a socket with the `vite-hmr` subprotocol. `update` starts
//!   a round (its `updates[].acceptedPath` are pending), the client logs
//!   `[vite] hot updated: <path>` (or `css hot updated`) as it applies
//!   each, and `[vite] Failed to reload <path>. …` when it cannot;
//!   `error` is a compile error (Vite shows its overlay and logs
//!   nothing), `full-reload` reloads the page.
//! - **webpack-dev-server**: a socket whose first messages are its
//!   `hot`, `liveReload` or `hash`. `invalid` starts a round, `errors`
//!   fails it, `static-changed` reloads; webpack's HMR runtime logs
//!   `[HMR] Updated modules:`, one `[HMR]  - <module>` each, and `[HMR]
//!   App is up to date.`, or `[HMR] … Need to do a full reload!`.
//! - **Next.js**: the `/_next/webpack-hmr` socket. `building` starts a
//!   round, `built`/`sync` with `errors` fails it, `reloadPage` reloads;
//!   the page logs `[Fast Refresh] done` when it applied one. Best
//!   effort: its messages are not a public API.
//!
//! Each round ends in one [`Outcome`]: applied (the files), failed (the
//! message and place) or a full reload, which counts as done once the
//! new document's `load` fired. Outcomes of one round merge (a Vite
//! update of three modules is one applied outcome; its compile error and
//! the client's "failed to reload" are one failure). They are numbered,
//! so commands can ask what came since a mark.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use protocol::schema::HotOutcome;
use serde_json::Value;

/// How many outcomes a tab keeps.
const OUTCOMES: usize = 32;

/// How many frames of a socket are read before it counts as not a
/// hot-reload socket (webpack-dev-server's greeting comes first).
const SNIFF_FRAMES: u32 = 4;

/// How long a failure's message may be.
const MESSAGE_LEN: usize = 300;

/// A dev server's client in the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Client {
    Vite,
    Webpack,
    Next,
}

impl Client {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Vite => "vite",
            Self::Webpack => "webpack",
            Self::Next => "next",
        }
    }
}

/// How a round of hot updates ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_field_names)] // `outcome` is the schema's name.
pub(crate) struct Outcome {
    /// Its number in the tab (from 1).
    pub(crate) seq: u64,
    pub(crate) client: Client,
    pub(crate) outcome: HotOutcome,
    /// As the dev server names them: applied, the files updated; failed,
    /// the file when `place` has none; full reload, the file that asked.
    pub(crate) files: Vec<String>,
    /// Failed: the error's first line.
    pub(crate) message: Option<String>,
    /// Failed: the file, 1-based line and column, when the error says.
    pub(crate) place: Option<(String, u32, u32)>,
    /// A full reload: whether the new document's `load` fired.
    pub(crate) loaded: bool,
    /// A full reload: the main frame's document (loader id) when it
    /// came; a `load` of another is the reload's.
    loader: Option<String>,
    /// The round it ended.
    round: u64,
    /// Whether more of its round may merge into it: an applied outcome
    /// while files are pending, a failure until the next round, a full
    /// reload until its document loads.
    open: bool,
}

impl Outcome {
    /// Whether the page is where the outcome leaves it: a full reload
    /// once its document loaded, anything else at once.
    pub(crate) fn settled(&self) -> bool {
        self.outcome != HotOutcome::FullReload || self.loaded
    }
}

/// A round under way: an update started and has not ended.
#[derive(Debug, Clone)]
pub(crate) struct Building {
    pub(crate) client: Client,
    round: u64,
    /// The files not applied yet, when the dev server named them.
    pub(crate) pending: Vec<String>,
}

/// A WebSocket of the tab that is, or may be, a dev server's.
#[derive(Debug, Clone)]
struct Socket {
    client: Option<Client>,
    /// Frames read while it was not known.
    sniffed: u32,
}

/// The hot updates one tab saw.
#[derive(Debug, Default)]
pub(crate) struct HotUpdates {
    /// Open WebSockets, by session and request id.
    sockets: HashMap<(String, String), Socket>,
    /// A client the current document's console showed (`[vite]
    /// connected.`), for pages whose socket opened before chrome-cdp
    /// watched.
    logged_client: Option<Client>,
    outcomes: VecDeque<Outcome>,
    seq: u64,
    round: u64,
    building: Option<Building>,
    /// webpack's `[HMR]  - <module>` lines since `[HMR] Updated modules:`.
    modules: Vec<String>,
    /// The main frame's document (loader id) and its URL's path.
    loader: Option<String>,
    path: String,
    /// Counts every change, for waiters.
    changes: u64,
    /// The last outcome a command reported (see `Tabs::report_hot_updates`).
    pub(crate) reported: u64,
    /// The last outcome `wait --hmr` returned: the next one waits for a
    /// newer one.
    pub(crate) waited: u64,
}

impl HotUpdates {
    /// The last outcome's number (0 before the first).
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    pub(crate) fn changes(&self) -> u64 {
        self.changes
    }

    /// The client the page has, if it has one: an open hot-reload socket,
    /// or the current document logged one.
    pub(crate) fn client(&self) -> Option<Client> {
        self.sockets
            .values()
            .find_map(|s| s.client)
            .or(self.logged_client)
    }

    /// The round under way, if any.
    pub(crate) fn building(&self) -> Option<&Building> {
        self.building.as_ref()
    }

    /// Whether the round under way (if any) began after `outcome`'s.
    pub(crate) fn building_after(&self, outcome: &Outcome) -> bool {
        self.building
            .as_ref()
            .is_some_and(|b| b.round > outcome.round)
    }

    /// The outcomes after number `after`, oldest first.
    pub(crate) fn since(&self, after: u64) -> impl Iterator<Item = &Outcome> {
        self.outcomes.iter().filter(move |o| o.seq > after)
    }

    /// Notes a `Network` event of `session`; `true` if anything changed.
    pub(crate) fn network(&mut self, session: &str, method: &str, params: &Value) -> bool {
        let Some(id) = params["requestId"].as_str() else {
            return false;
        };
        let key = (session.to_owned(), id.to_owned());
        match method {
            "Network.webSocketCreated" => {
                let url = params["url"].as_str().unwrap_or_default();
                let client = url.contains("/_next/webpack-hmr").then_some(Client::Next);
                self.sockets.insert(key, Socket { client, sniffed: 0 });
                self.bump()
            }
            "Network.webSocketWillSendHandshakeRequest"
            | "Network.webSocketHandshakeResponseReceived" => {
                let headers = &params[if method.ends_with("Request") {
                    "request"
                } else {
                    "response"
                }]["headers"];
                let vite = headers.as_object().is_some_and(|h| {
                    h.iter().any(|(name, value)| {
                        name.eq_ignore_ascii_case("sec-websocket-protocol")
                            && value
                                .as_str()
                                .is_some_and(|v| v.split(',').any(|p| p.trim() == "vite-hmr"))
                    })
                });
                match self.sockets.get_mut(&key) {
                    Some(socket) if vite && socket.client.is_none() => {
                        socket.client = Some(Client::Vite);
                        self.bump()
                    }
                    _ => false,
                }
            }
            "Network.webSocketFrameReceived" => {
                let frame = &params["response"];
                if frame["opcode"].as_u64().unwrap_or(1) != 1 {
                    return false;
                }
                let Some(socket) = self.sockets.get_mut(&key) else {
                    return false;
                };
                if socket.client.is_none() && socket.sniffed >= SNIFF_FRAMES {
                    return false;
                }
                let payload = frame["payloadData"].as_str().unwrap_or_default();
                let mut changed = false;
                for message in messages(payload) {
                    let Some(socket) = self.sockets.get_mut(&key) else {
                        break;
                    };
                    let client = if let Some(client) = socket.client {
                        client
                    } else {
                        socket.sniffed += 1;
                        if !is_webpack_greeting(&message) {
                            continue;
                        }
                        socket.client = Some(Client::Webpack);
                        changed = true;
                        Client::Webpack
                    };
                    changed |= match client {
                        Client::Vite => self.vite(&message),
                        Client::Webpack => self.webpack(&message),
                        Client::Next => self.next(&message),
                    };
                }
                if changed {
                    self.bump();
                }
                changed
            }
            "Network.webSocketClosed" => {
                let known = self
                    .sockets
                    .remove(&key)
                    .is_some_and(|s| s.client.is_some());
                known && self.bump()
            }
            _ => false,
        }
    }

    /// Notes a console entry's text; `true` if anything changed.
    pub(crate) fn console(&mut self, text: &str) -> bool {
        let changed = if let Some(rest) = text.strip_prefix("[vite] ") {
            self.vite_log(rest)
        } else if let Some(rest) = text.strip_prefix("[hmr] ") {
            // Vite before 6.
            self.vite_log(rest)
        } else if let Some(rest) = text.strip_prefix("[HMR] ") {
            self.webpack_log(rest)
        } else if let Some(rest) = text.strip_prefix("[webpack-dev-server] ") {
            self.logged(Client::Webpack);
            if rest.starts_with("App updated. Recompiling") {
                self.start(Client::Webpack, Vec::new());
            } else if rest.ends_with("Reloading...") {
                self.full_reload(Client::Webpack, None);
            }
            true
        } else if let Some(rest) = text.strip_prefix("[Fast Refresh] ") {
            self.logged(Client::Next);
            if rest.starts_with("rebuilding") {
                if self.building.is_none() {
                    self.start(Client::Next, Vec::new());
                }
            } else if rest.starts_with("done") {
                self.applied(Client::Next, None);
            } else if rest.starts_with("performing full reload") {
                self.full_reload(Client::Next, None);
            }
            true
        } else {
            false
        };
        if changed {
            self.bump();
        }
        changed
    }

    /// Notes that the main frame (of `session`) committed a new document
    /// `loader` at `url`: the old document's client and sockets are gone,
    /// and a round under way ended with it.
    pub(crate) fn navigated(&mut self, session: &str, loader: &str, url: &str) {
        self.sockets.retain(|(s, _), _| s != session);
        self.loader = Some(loader.to_owned());
        url_path(url).clone_into(&mut self.path);
        self.logged_client = None;
        self.building = None;
        self.modules.clear();
        self.bump();
    }

    /// Notes the main frame's `Page.lifecycleEvent` `name` of document
    /// `loader`: a `load` of a new document ends a full reload.
    pub(crate) fn lifecycle(&mut self, loader: &str, name: &str) -> bool {
        if name != "load" {
            return false;
        }
        let mut changed = false;
        for outcome in &mut self.outcomes {
            if outcome.outcome == HotOutcome::FullReload
                && !outcome.loaded
                && outcome.loader.as_deref() != Some(loader)
            {
                outcome.loaded = true;
                changed = true;
            }
        }
        if changed {
            self.bump();
        }
        changed
    }

    fn bump(&mut self) -> bool {
        self.changes += 1;
        true
    }

    fn logged(&mut self, client: Client) {
        self.logged_client.get_or_insert(client);
    }

    /// A message of Vite's socket.
    fn vite(&mut self, message: &Value) -> bool {
        match message["type"].as_str().unwrap_or_default() {
            "update" => {
                let pending = message["updates"]
                    .as_array()
                    .map(|updates| {
                        updates
                            .iter()
                            .filter_map(|u| u["acceptedPath"].as_str().or(u["path"].as_str()))
                            .map(|p| p.split('?').next().unwrap_or(p).to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                self.start(Client::Vite, pending);
                true
            }
            "full-reload" => {
                if message["ifFallback"].as_bool() == Some(true) {
                    return false;
                }
                let path = message["path"].as_str().filter(|p| *p != "*");
                // Vite reloads for an HTML file only the page it is.
                if let Some(path) = path
                    && Path::new(path)
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("html"))
                    && path != "/index.html"
                    && path != self.path
                {
                    return false;
                }
                // `*` names no file; `triggeredBy` is the edited one.
                let file = path.or(message["triggeredBy"].as_str());
                self.full_reload(Client::Vite, file.map(str::to_owned));
                true
            }
            "error" => {
                let err = &message["err"];
                let loc = &err["loc"];
                let text = err["message"].as_str().unwrap_or_default();
                let file = loc["file"]
                    .as_str()
                    .or(err["id"].as_str())
                    .map(|f| f.split('?').next().unwrap_or(f).to_owned());
                let place = match (&file, loc["line"].as_u64()) {
                    (Some(file), Some(line)) => Some((
                        file.clone(),
                        u32::try_from(line).unwrap_or(u32::MAX),
                        // Rollup's columns are 0-based.
                        loc["column"]
                            .as_u64()
                            .and_then(|c| u32::try_from(c + 1).ok())
                            .unwrap_or(1),
                    )),
                    // Vite 8 (oxc) has the place in the message only.
                    (Some(file), None) => {
                        diagnostic_place(text).map(|(line, column)| (file.clone(), line, column))
                    }
                    _ => None,
                };
                let files = if place.is_none() {
                    file.into_iter().collect()
                } else {
                    Vec::new()
                };
                self.failed(Client::Vite, first_line(text), place, files);
                true
            }
            "connected" => {
                self.logged(Client::Vite);
                false
            }
            _ => false,
        }
    }

    /// A `[vite] ` console line (without the prefix).
    fn vite_log(&mut self, rest: &str) -> bool {
        self.logged(Client::Vite);
        let updated = rest
            .strip_prefix("hot updated: ")
            .or_else(|| rest.strip_prefix("css hot updated: "));
        if let Some(updated) = updated {
            // `<changed> via <boundary>`: the changed file comes first.
            let file = updated.split(" via ").next().unwrap_or(updated);
            let file = file.split('?').next().unwrap_or(file);
            self.applied(Client::Vite, Some(file.to_owned()));
        } else if let Some(failed) = rest.strip_prefix("Failed to reload ") {
            let file = failed.split(". ").next().unwrap_or(failed);
            let file = file.strip_suffix('.').unwrap_or(file);
            self.failed(
                Client::Vite,
                Some(format!(
                    "failed to reload {file}; the browser console has why"
                )),
                None,
                vec![file.to_owned()],
            );
        }
        true
    }

    /// A message of webpack-dev-server's socket.
    fn webpack(&mut self, message: &Value) -> bool {
        match message["type"].as_str().unwrap_or_default() {
            "invalid" => {
                self.start(Client::Webpack, Vec::new());
                true
            }
            "errors" => {
                let Some(error) = message["data"].as_array().and_then(|e| e.first()) else {
                    return false;
                };
                let (text, place) = match error {
                    Value::String(text) => (text.as_str(), None),
                    error => {
                        let module = error["moduleName"].as_str().or(error["file"].as_str());
                        let place = module.and_then(|m| {
                            let (line, column) = line_column(error["loc"].as_str()?)?;
                            Some((m.to_owned(), line, column))
                        });
                        (error["message"].as_str().unwrap_or_default(), place)
                    }
                };
                let files = match (&place, error["moduleName"].as_str()) {
                    (None, Some(module)) => vec![module.to_owned()],
                    _ => Vec::new(),
                };
                self.failed(Client::Webpack, first_line(text), place, files);
                true
            }
            "static-changed" => {
                let file = message["data"].as_str().map(str::to_owned);
                self.full_reload(Client::Webpack, file);
                true
            }
            _ => false,
        }
    }

    /// An `[HMR] ` console line of webpack's runtime (without the prefix).
    fn webpack_log(&mut self, rest: &str) -> bool {
        self.logged(Client::Webpack);
        if rest.starts_with("Updated modules:") {
            self.modules.clear();
        } else if let Some(module) = rest.strip_prefix(" - ") {
            self.modules.push(module.trim().to_owned());
        } else if rest.starts_with("App is up to date") {
            let modules = std::mem::take(&mut self.modules);
            self.applied_all(Client::Webpack, modules);
        } else if rest.contains("Need to do a full reload") {
            self.full_reload(Client::Webpack, None);
        }
        true
    }

    /// A message of Next.js's socket.
    fn next(&mut self, message: &Value) -> bool {
        match message["action"].as_str().unwrap_or_default() {
            "building" => {
                self.start(Client::Next, Vec::new());
                true
            }
            "built" | "sync" => {
                let Some(error) = message["errors"].as_array().and_then(|e| e.first()) else {
                    return false;
                };
                let text = error
                    .as_str()
                    .or(error["message"].as_str())
                    .unwrap_or_default();
                self.failed(Client::Next, first_line(text), None, Vec::new());
                true
            }
            "reloadPage" => {
                self.full_reload(Client::Next, None);
                true
            }
            "serverComponentChanges" => {
                self.applied(Client::Next, None);
                true
            }
            _ => false,
        }
    }

    /// A round begins: the files it will apply, if known.
    fn start(&mut self, client: Client, pending: Vec<String>) {
        self.round += 1;
        self.modules.clear();
        self.building = Some(Building {
            client,
            round: self.round,
            pending,
        });
    }

    /// The page applied `file` (or the round, without one).
    fn applied(&mut self, client: Client, file: Option<String>) {
        self.applied_all(client, file.into_iter().collect());
    }

    fn applied_all(&mut self, client: Client, files: Vec<String>) {
        if let Some(building) = &mut self.building {
            building.pending.retain(|p| !files.contains(p));
            if building.pending.is_empty() || building.client != client {
                self.building = None;
            }
        }
        // Vite's client logs `hot updated` even after it failed to
        // reload the module: the round failed.
        if self.outcomes.back().is_some_and(|last| {
            last.open
                && last.round == self.round
                && last.client == client
                && last.outcome == HotOutcome::Failed
        }) {
            return;
        }
        self.push(client, HotOutcome::Applied, files, None, None);
    }

    fn failed(
        &mut self,
        client: Client,
        message: Option<String>,
        place: Option<(String, u32, u32)>,
        files: Vec<String>,
    ) {
        self.building = None;
        self.push(client, HotOutcome::Failed, files, message, place);
    }

    fn full_reload(&mut self, client: Client, file: Option<String>) {
        self.building = None;
        self.push(
            client,
            HotOutcome::FullReload,
            file.into_iter().collect(),
            None,
            None,
        );
    }

    /// Adds an outcome, or merges it into the last one when that ended
    /// the same round the same way.
    fn push(
        &mut self,
        client: Client,
        outcome: HotOutcome,
        files: Vec<String>,
        message: Option<String>,
        place: Option<(String, u32, u32)>,
    ) {
        let open = match outcome {
            HotOutcome::Applied => self.building.is_some(),
            HotOutcome::Failed | HotOutcome::FullReload => true,
        };
        if let Some(last) = self.outcomes.back_mut()
            && last.open
            && last.client == client
            && last.outcome == outcome
            && match outcome {
                HotOutcome::FullReload => !last.loaded && last.loader == self.loader,
                _ => last.round == self.round,
            }
        {
            last.open = open;
            if place.is_some() && last.place.is_none() {
                // A compile error says more than the client's "failed to
                // reload" before it.
                last.place = place;
                last.message = message.or(last.message.take());
                last.files.clear();
            } else if last.place.is_none() {
                for file in files {
                    if !last.files.contains(&file) {
                        last.files.push(file);
                    }
                }
                if last.message.is_none() {
                    last.message = message;
                }
            }
            return;
        }
        if self.outcomes.len() == OUTCOMES {
            self.outcomes.pop_front();
        }
        self.seq += 1;
        self.outcomes.push_back(Outcome {
            seq: self.seq,
            client,
            outcome,
            files,
            message,
            place,
            loaded: false,
            loader: self.loader.clone(),
            round: self.round,
            open,
        });
    }
}

/// The JSON messages of a frame: one, or SockJS's `a["…", …]` batch.
fn messages(payload: &str) -> Vec<Value> {
    if let Some(batch) = payload.strip_prefix('a') {
        return serde_json::from_str::<Vec<String>>(batch)
            .map(|texts| {
                texts
                    .iter()
                    .filter_map(|t| serde_json::from_str(t).ok())
                    .collect()
            })
            .unwrap_or_default();
    }
    if !payload.starts_with('{') {
        return Vec::new();
    }
    serde_json::from_str(payload).map_or_else(|_| Vec::new(), |m| vec![m])
}

/// Whether `message` is one webpack-dev-server greets its client with.
fn is_webpack_greeting(message: &Value) -> bool {
    match message["type"].as_str() {
        Some("hot" | "liveReload") => true,
        Some("hash") => message["data"].is_string(),
        _ => false,
    }
}

/// The line of an error message that says what went wrong: the first,
/// unless it only introduces the next (`Transform failed with 1
/// error:`, `Module build failed (from …):`). Terminal colours are
/// dropped.
fn first_line(text: &str) -> Option<String> {
    let text = strip_ansi(text);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let line = match lines.as_slice() {
        [] => return None,
        [first, next, ..] if first.ends_with(':') => next,
        [first, ..] => first,
    };
    let mut line = (*line).to_owned();
    if line.len() > MESSAGE_LEN {
        let mut end = MESSAGE_LEN;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    Some(line)
}

/// `text` without ANSI escape sequences.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The line and (1-based) column of an oxc/Rolldown diagnostic's header,
/// `╭─[ src/cart.ts?t=1:23:8 ]`, in `text`.
fn diagnostic_place(text: &str) -> Option<(u32, u32)> {
    let header = text.split("╭─[").nth(1)?.split(']').next()?.trim();
    let mut parts = header.rsplitn(3, ':');
    let column = parts.next()?.trim().parse().ok()?;
    let line = parts.next()?.trim().parse().ok()?;
    Some((line, column))
}

/// webpack's `loc`: `12:5`, `12:5-9` or `12:5-14:2`.
fn line_column(loc: &str) -> Option<(u32, u32)> {
    let start = loc.split('-').next()?;
    let (line, column) = start.split_once(':').unwrap_or((start, "1"));
    Some((line.trim().parse().ok()?, column.trim().parse().ok()?))
}

/// A URL's path: `/app/index.html` of `http://localhost:5173/app/index.html?x`.
fn url_path(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = rest.find('/').map_or("/", |i| &rest[i..]);
    path.split(['?', '#']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    fn created(h: &mut HotUpdates, id: &str, url: &str) {
        h.network(
            "s",
            "Network.webSocketCreated",
            &json!({"requestId": id, "url": url}),
        );
    }

    fn vite_socket(h: &mut HotUpdates, id: &str) {
        created(h, id, "ws://localhost:5173/?token=x");
        h.network(
            "s",
            "Network.webSocketHandshakeResponseReceived",
            &json!({"requestId": id, "response": {"headers": {"Sec-WebSocket-Protocol": "vite-hmr"}}}),
        );
    }

    fn received(h: &mut HotUpdates, id: &str, message: &Value) -> bool {
        h.network(
            "s",
            "Network.webSocketFrameReceived",
            &json!({"requestId": id, "response": {"opcode": 1, "payloadData": message.to_string()}}),
        )
    }

    fn update(paths: &[&str]) -> Value {
        let updates: Vec<Value> = paths
            .iter()
            .map(|p| json!({"type": "js-update", "timestamp": 1, "path": p, "acceptedPath": p}))
            .collect();
        json!({"type": "update", "updates": updates})
    }

    fn last(h: &HotUpdates) -> &Outcome {
        h.since(0).last().unwrap()
    }

    #[test]
    fn vite_updates_apply_as_the_client_logs_them() {
        let mut h = HotUpdates::default();
        h.navigated("s", "L1", "http://localhost:5173/");
        assert_eq!(h.client(), None);
        vite_socket(&mut h, "1");
        assert_eq!(h.client(), Some(Client::Vite));
        received(&mut h, "1", &json!({"type": "connected"}));
        received(&mut h, "1", &update(&["/src/a.ts", "/src/b.ts"]));
        assert_eq!(h.building().unwrap().pending, ["/src/a.ts", "/src/b.ts"]);
        assert_eq!(h.seq(), 0);
        h.console("[vite] hot updated: /src/a.ts");
        assert_eq!(last(&h).outcome, HotOutcome::Applied);
        assert!(h.building().is_some(), "b.ts is still pending");
        h.console("[vite] hot updated: /src/b.ts via /src/App.tsx");
        assert!(h.building().is_none());
        // One round, one outcome.
        assert_eq!(h.seq(), 1);
        assert_eq!(last(&h).files, ["/src/a.ts", "/src/b.ts"]);
        assert!(last(&h).settled());
        // The next edit is another outcome.
        received(&mut h, "1", &update(&["/src/a.ts"]));
        h.console("[vite] css hot updated: /src/a.css");
        assert_eq!(h.seq(), 2);
        assert_eq!(last(&h).files, ["/src/a.css"]);
    }

    #[test]
    fn a_vite_compile_error_fails_with_its_place() {
        let mut h = HotUpdates::default();
        vite_socket(&mut h, "1");
        received(&mut h, "1", &update(&["/src/cart.ts"]));
        received(
            &mut h,
            "1",
            &json!({"type": "error", "err": {
                "message": "Transform failed with 1 error:\n/app/src/cart.ts:12:4: ERROR: Expected \";\"",
                "id": "/app/src/cart.ts",
                "loc": {"file": "/app/src/cart.ts", "line": 12, "column": 4},
            }}),
        );
        // The client's own lines merge into it: its failure, and the
        // "hot updated" it logs after a failure too.
        h.console("[vite] Failed to reload /src/cart.ts. This could be due to syntax errors or importing non-existent modules. (see errors above)");
        h.console("[vite] hot updated: /src/cart.ts");
        assert_eq!(h.seq(), 1);
        let o = last(&h);
        assert_eq!(o.outcome, HotOutcome::Failed);
        assert_eq!(
            o.message.as_deref(),
            Some("/app/src/cart.ts:12:4: ERROR: Expected \";\"")
        );
        assert_eq!(o.place, Some(("/app/src/cart.ts".into(), 12, 5)));
        assert!(o.files.is_empty());
        assert!(h.building().is_none());
    }

    #[test]
    fn a_vite_8_error_has_its_place_in_the_message() {
        let mut h = HotUpdates::default();
        vite_socket(&mut h, "1");
        received(
            &mut h,
            "1",
            &json!({"type": "error", "err": {
                "message": "Transform failed with 1 error:\n\n[PARSE_ERROR] Unexpected token\n    ╭─[ src/components/CartItem.tsx?t=1:23:8 ]\n    │\n 23 │       </button>\n",
                "id": "/app/src/components/CartItem.tsx?t=1",
                "frame": "",
                "plugin": "vite:oxc",
            }}),
        );
        let o = last(&h);
        assert_eq!(o.message.as_deref(), Some("[PARSE_ERROR] Unexpected token"));
        assert_eq!(
            o.place,
            Some(("/app/src/components/CartItem.tsx".into(), 23, 8))
        );
    }

    #[test]
    fn a_client_failure_alone_names_the_file() {
        let mut h = HotUpdates::default();
        h.console("[vite] Failed to reload /src/cart.ts. This could be due to syntax errors or importing non-existent modules. (see errors above)");
        let o = last(&h);
        assert_eq!(o.outcome, HotOutcome::Failed);
        assert_eq!(o.files, ["/src/cart.ts"]);
        assert_eq!(h.client(), Some(Client::Vite));
    }

    #[test]
    fn a_full_reload_is_done_when_the_new_document_loads() {
        let mut h = HotUpdates::default();
        h.navigated("s", "L1", "http://localhost:5173/");
        vite_socket(&mut h, "1");
        received(
            &mut h,
            "1",
            &json!({"type": "full-reload", "path": "/src/main.ts"}),
        );
        let o = last(&h);
        assert_eq!(o.outcome, HotOutcome::FullReload);
        assert_eq!(o.files, ["/src/main.ts"]);
        assert!(!o.settled());
        // The old document's own load does not count.
        h.lifecycle("L1", "load");
        assert!(!last(&h).settled());
        h.network("s", "Network.webSocketClosed", &json!({"requestId": "1"}));
        h.navigated("x", "L2", "http://localhost:5173/");
        assert_eq!(h.client(), None);
        h.lifecycle("L2", "DOMContentLoaded");
        assert!(!last(&h).settled());
        h.lifecycle("L2", "load");
        assert!(last(&h).settled());
        // `*` names no file (unless `triggeredBy` does); another page's
        // HTML does not reload this one.
        received(&mut h, "2", &json!({"type": "full-reload", "path": "*"}));
        vite_socket(&mut h, "2");
        received(
            &mut h,
            "2",
            &json!({"type": "full-reload", "path": "/other.html"}),
        );
        assert_eq!(h.seq(), 1);
        received(&mut h, "2", &json!({"type": "full-reload", "path": "*"}));
        assert_eq!(h.seq(), 2);
        assert!(last(&h).files.is_empty());
        h.lifecycle("L3", "load");
        received(
            &mut h,
            "2",
            &json!({"type": "full-reload", "path": "*", "triggeredBy": "/app/src/main.ts"}),
        );
        assert_eq!(last(&h).files, ["/app/src/main.ts"]);
    }

    #[test]
    fn webpack_is_known_by_its_greeting_and_logs() {
        let mut h = HotUpdates::default();
        created(&mut h, "7", "ws://localhost:8080/ws");
        assert_eq!(h.client(), None);
        received(&mut h, "7", &json!({"type": "hot"}));
        assert_eq!(h.client(), Some(Client::Webpack));
        received(&mut h, "7", &json!({"type": "invalid"}));
        assert!(h.building().is_some());
        received(&mut h, "7", &json!({"type": "hash", "data": "abc"}));
        received(&mut h, "7", &json!({"type": "ok"}));
        for line in [
            "[HMR] Checking for updates on the server...",
            "[HMR] Updated modules:",
            "[HMR]  - ./src/cart.ts",
            "[HMR]  - ./src/api.ts",
            "[HMR] App is up to date.",
        ] {
            h.console(line);
        }
        let o = last(&h);
        assert_eq!(o.outcome, HotOutcome::Applied);
        assert_eq!(o.files, ["./src/cart.ts", "./src/api.ts"]);
        assert!(h.building().is_none());
        received(&mut h, "7", &json!({"type": "invalid"}));
        received(
            &mut h,
            "7",
            &json!({"type": "errors", "data": [{
                "message": "Module build failed (from ./node_modules/babel-loader/lib/index.js):\nSyntaxError: /app/src/cart.ts: Unexpected token (12:5)",
                "moduleName": "./src/cart.ts",
                "loc": "12:5-9",
            }]}),
        );
        let o = last(&h);
        assert_eq!(o.outcome, HotOutcome::Failed);
        assert_eq!(
            o.message.as_deref(),
            Some("SyntaxError: /app/src/cart.ts: Unexpected token (12:5)")
        );
        assert_eq!(o.place, Some(("./src/cart.ts".into(), 12, 5)));
        h.console("[HMR] Cannot apply update. Need to do a full reload!");
        assert_eq!(last(&h).outcome, HotOutcome::FullReload);
    }

    #[test]
    fn an_app_socket_is_not_a_dev_server() {
        let mut h = HotUpdates::default();
        created(&mut h, "3", "ws://localhost:3000/live");
        for i in 0..10 {
            assert!(!received(&mut h, "3", &json!({"type": "price", "i": i})));
        }
        // Past the first frames, even a greeting-like one is the app's.
        assert!(!received(&mut h, "3", &json!({"type": "hot"})));
        assert_eq!(h.client(), None);
        assert_eq!(h.seq(), 0);
    }

    #[test]
    fn next_builds_and_fast_refresh() {
        let mut h = HotUpdates::default();
        created(&mut h, "9", "ws://localhost:3000/_next/webpack-hmr");
        assert_eq!(h.client(), Some(Client::Next));
        received(
            &mut h,
            "9",
            &json!({"action": "sync", "hash": "a", "errors": [], "warnings": []}),
        );
        received(&mut h, "9", &json!({"action": "building"}));
        h.console("[Fast Refresh] rebuilding");
        received(
            &mut h,
            "9",
            &json!({"action": "built", "hash": "b", "errors": [], "warnings": []}),
        );
        assert_eq!(h.seq(), 0);
        h.console("[Fast Refresh] done in 104ms");
        assert_eq!(last(&h).outcome, HotOutcome::Applied);
        received(&mut h, "9", &json!({"action": "building"}));
        received(
            &mut h,
            "9",
            &json!({"action": "built", "errors": [{"message": "\u{1b}[31m./app/page.tsx\u{1b}[39m\nError: x"}]}),
        );
        assert_eq!(last(&h).outcome, HotOutcome::Failed);
        assert_eq!(last(&h).message.as_deref(), Some("./app/page.tsx"));
        received(&mut h, "9", &json!({"action": "reloadPage"}));
        assert_eq!(last(&h).outcome, HotOutcome::FullReload);
    }

    #[test]
    fn sockjs_batches_and_bounds() {
        let mut h = HotUpdates::default();
        created(
            &mut h,
            "4",
            "http://localhost:8080/sockjs-node/1/x/websocket",
        );
        let batch = json!([r#"{"type":"hot"}"#, r#"{"type":"liveReload"}"#]);
        received_raw(&mut h, "4", &format!("a{batch}"));
        assert_eq!(h.client(), Some(Client::Webpack));
        for _ in 0..(OUTCOMES + 5) {
            received(&mut h, "4", &json!({"type": "invalid"}));
            received(&mut h, "4", &json!({"type": "errors", "data": ["x"]}));
        }
        assert_eq!(h.since(0).count(), OUTCOMES);
        assert_eq!(h.seq(), (OUTCOMES + 5) as u64);
        // Reloads merge until the page loads.
        received(
            &mut h,
            "4",
            &json!({"type": "static-changed", "data": "index.html"}),
        );
        h.console(
            "[webpack-dev-server] \"index.html\" from static directory was changed. Reloading...",
        );
        assert_eq!(h.seq(), (OUTCOMES + 6) as u64);
        assert_eq!(last(&h).files, ["index.html"]);
    }

    fn received_raw(h: &mut HotUpdates, id: &str, payload: &str) {
        h.network(
            "s",
            "Network.webSocketFrameReceived",
            &json!({"requestId": id, "response": {"opcode": 1, "payloadData": payload}}),
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(line_column("12:5-14:2"), Some((12, 5)));
        assert_eq!(line_column("7"), Some((7, 1)));
        assert_eq!(url_path("http://h:1/a/b.html?x#y"), "/a/b.html");
        assert_eq!(url_path("http://h:1"), "/");
        assert_eq!(first_line("\n  one\ntwo"), Some("one".into()));
        assert_eq!(first_line(""), None);
        assert_eq!(
            first_line(&"x".repeat(400)).unwrap().chars().count(),
            MESSAGE_LEN + 1
        );
    }
}
