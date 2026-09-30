//! What the CLI asks the engine for. These are the params of the daemon's
//! RPC methods, so they are serde types.

use std::path::PathBuf;
use std::time::Duration;

use browser::{Chrome, ProfileChoice};
use protocol::schema::{Geolocation, Media, Throttle, Viewport};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::state::StorageState;

/// `open`: start the session's browser, or check that the running one fits,
/// then optionally load a URL.
///
/// Options left `None` (or empty) mean "whatever the session runs with", so
/// a plain `open <url>` works on any session. Given ones must match the
/// running browser unless `restart` is set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRequest {
    /// The Chrome the CLI found for a launch (it may have installed it
    /// first). `None` when the CLI expects the browser to run already; the
    /// engine then finds one itself if it has to launch after all.
    #[serde(default)]
    pub chrome: Option<Chrome>,
    /// `--chrome`, as given.
    #[serde(default)]
    pub chrome_flag: Option<PathBuf>,
    /// `Some(true)` for `--headed`.
    #[serde(default)]
    pub headed: Option<bool>,
    /// `--profile` or `--user-data-dir`.
    #[serde(default)]
    pub profile: Option<ProfileChoice>,
    /// `--chrome-arg`s.
    #[serde(default)]
    pub chrome_args: Vec<String>,
    /// `--extension`s: unpacked extensions' directories (absolute), loaded
    /// at launch.
    #[serde(default)]
    pub extensions: Vec<PathBuf>,
    /// Close a running browser first, so the given options apply.
    #[serde(default)]
    pub restart: bool,
    /// A URL to load in the active tab (see [`crate::normalize_url`]).
    #[serde(default)]
    pub url: Option<String>,
    /// `base_url` from the config, for a relative `url`.
    #[serde(default)]
    pub base_url: Option<String>,
    /// `--tab`: load `url` in this tab instead.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--follow-popups`: whether popups become the active tab; `None`
    /// keeps the session's setting (off at first).
    #[serde(default)]
    pub follow_popups: Option<bool>,
    /// `--ignore-https-errors`: whether pages load despite certificate
    /// errors; `None` keeps the session's setting (off at first).
    #[serde(default)]
    pub ignore_https_errors: Option<bool>,
    /// `--dialogs`: how JavaScript dialogs are answered; `None` keeps the
    /// session's policy ([`DialogPolicy::Auto`] at first).
    #[serde(default)]
    pub dialogs: Option<DialogPolicy>,
    /// `--http-credentials`: what to answer HTTP authentication challenges
    /// with; `None` keeps the session's (none at first).
    #[serde(default)]
    pub http_credentials: Option<HttpCredentials>,
    /// `--console-buffer`: how many console entries each tab keeps; `None`
    /// keeps the session's (1,000 at first).
    #[serde(default)]
    pub console_buffer: Option<usize>,
    /// `--network-buffer`: how many requests each tab's network log keeps;
    /// `None` keeps the session's (2,000 at first).
    #[serde(default)]
    pub network_buffer: Option<usize>,
    /// `--bypass-service-worker`: whether requests skip service workers
    /// even without routes (they always do while routes exist); `None`
    /// keeps the session's setting (off at first).
    #[serde(default)]
    pub bypass_service_worker: Option<bool>,
    /// `--device`: emulate this device (as `emulate <device>`) before
    /// loading `url`.
    #[serde(default)]
    pub device: Option<String>,
    /// `--downloads-dir`: where downloads go (absolute; created); `None`
    /// keeps the session's (`<session dir>/downloads` at first).
    #[serde(default)]
    pub downloads_dir: Option<PathBuf>,
    /// `--init-script` and `--init-script-code`: init scripts added (as
    /// `init-script add`) before `url` loads, in order. One the session
    /// already has (same source and file) is not added again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub init_scripts: Vec<OpenInitScript>,
    /// Sent by a CLI whose daemon an upgrade replaced: `downloads` says
    /// its list starts now.
    #[serde(default)]
    pub replaced_daemon: bool,
    /// `--state`: load this storage state (as `state load`) before
    /// loading `url`. Never kept for a later daemon: it holds secrets.
    #[serde(default)]
    pub state: Option<StateLoadRequest>,
    /// Bounds the launch and the page load.
    pub timeout_ms: u64,
}

/// An init script `open` adds: `--init-script FILE` (with the file, read
/// by the CLI) or `--init-script-code JS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenInitScript {
    pub source: String,
    /// The file it was read from (absolute), for `init-script list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

impl OpenInitScript {
    /// `earlier` followed by those of `later` it does not have: the init
    /// scripts of an `open` after the scripts of the ones before it.
    pub fn merged(earlier: &[Self], later: &[Self]) -> Vec<Self> {
        let mut all = earlier.to_vec();
        for script in later {
            if !all.contains(script) {
                all.push(script.clone());
            }
        }
        all
    }
}

/// How the session answers JavaScript dialogs (`open --dialogs`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogPolicy {
    /// Accept `alert` and `beforeunload`; `confirm` and `prompt` wait for
    /// `chrome-cdp dialog`.
    #[default]
    Auto,
    /// Accept every dialog (a prompt with its default answer).
    Accept,
    /// Dismiss every dialog.
    Dismiss,
    /// Every dialog waits for `chrome-cdp dialog`.
    Manual,
}

impl DialogPolicy {
    /// The `--dialogs` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Accept => "accept",
            Self::Dismiss => "dismiss",
            Self::Manual => "manual",
        }
    }
}

/// A user name and password for HTTP authentication (basic, digest, …).
/// `Debug` does not show the password.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpCredentials {
    pub username: String,
    pub password: String,
    /// The only origin (`https://app.test`) whose challenges get them;
    /// `None` for every origin, as Playwright's `httpCredentials`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl HttpCredentials {
    /// Whether a challenge from `url` (a URL or an origin) gets these
    /// credentials. An origin that does not parse gets none.
    pub fn applies_to(&self, url: &str) -> bool {
        match &self.origin {
            None => true,
            Some(origin) => {
                let of = |s: &str| url::Url::parse(s).ok().map(|u| u.origin());
                of(url).is_some_and(|o| o.is_tuple() && Some(o) == of(origin))
            }
        }
    }
}

impl std::fmt::Debug for HttpCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpCredentials")
            .field("username", &self.username)
            .field("password", &"…")
            .field("origin", &self.origin)
            .finish()
    }
}

/// `connect`: attach the session to a running browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectRequest {
    /// A port, `host:port`, `http://host:port` or a `ws://` DevTools URL.
    pub endpoint: String,
    /// Bounds the connection and each later call's default.
    pub timeout_ms: u64,
}

/// Where a raw CDP call goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// The browser session (`Browser.*`, `Target.*`, …).
    Browser,
    /// The active tab, or `--tab` (`Page.*`, `Runtime.*`, `DOM.*`, …).
    Page,
}

/// `cdp`: one raw CDP call, and the events that follow it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CdpRequest {
    /// `Domain.method`; `None` only listens (`--listen`).
    #[serde(default)]
    pub method: Option<String>,
    /// The params object.
    #[serde(default)]
    pub params: Map<String, Value>,
    /// The browser or a tab.
    pub target: Target,
    /// `--tab`: the tab for [`Target::Page`]; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--frame`: send a [`Target::Page`] call to this frame's session (an
    /// OOPIF has its own), and run `Runtime` evaluations in its main
    /// world. Its id, `name` or a part of its URL.
    #[serde(default)]
    pub frame: Option<String>,
    /// `--listen`: the events to capture from the call's session, each
    /// `Domain.event` or `Domain.*`.
    #[serde(default)]
    pub listen: Vec<String>,
    /// `--count`: how many events to wait for; 0 captures them until the
    /// timeout.
    #[serde(default)]
    pub count: usize,
    /// `--all-sessions`: listen on every session of the tab (its page's,
    /// its out-of-process iframes', its workers'), not only the call's.
    #[serde(default)]
    pub all_sessions: bool,
    /// Bounds the call, and the wait for events.
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// Where `eval` runs a script (`--world`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvalWorld {
    /// The page's own world, with its globals.
    #[default]
    Main,
    /// chrome-cdp's isolated world: the same DOM, none of the page's
    /// globals.
    Isolated,
}

impl EvalWorld {
    /// The `--world` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isolated => "isolated",
        }
    }
}

/// `eval <js>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalRequest {
    /// The script: an expression or statements; the last one's value is
    /// the result.
    pub script: String,
    /// `--in`: the one element bound as `el`.
    #[serde(default)]
    pub locator: Option<String>,
    /// `--frame`: run in this frame (its id, `name` or a part of its URL).
    #[serde(default)]
    pub frame: Option<String>,
    #[serde(default)]
    pub world: EvalWorld,
    /// `--tab`: the tab to run in; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--strict-refs`: a ref whose element is gone is an error, never
    /// re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    /// `--sourcemaps`: resolve an exception's stack with source maps
    /// (always on for localhost pages).
    #[serde(default)]
    pub sourcemaps: bool,
    /// Bounds finding the element and running the script (a promise's
    /// wait included).
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `tabs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabsRequest {
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `frames`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FramesRequest {
    /// The tab (`t2`); the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `tab new [url]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabNewRequest {
    /// A URL to load in the new tab (see [`crate::normalize_url`]).
    #[serde(default)]
    pub url: Option<String>,
    /// `base_url` from the config, for a relative `url`.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Bounds opening the tab and loading the page.
    pub timeout_ms: u64,
}

/// `context new [url]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextNewRequest {
    /// A URL to load in the context's first tab.
    #[serde(default)]
    pub url: Option<String>,
    /// `base_url` from the config, for a relative `url`.
    #[serde(default)]
    pub base_url: Option<String>,
    pub timeout_ms: u64,
}

/// `context list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextListRequest {
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `context close [id…] [--all]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCloseRequest {
    /// `c1`, …; none closes `--context`'s.
    #[serde(default)]
    pub ids: Vec<String>,
    /// Every context of the session.
    #[serde(default)]
    pub all: bool,
    pub timeout_ms: u64,
}

/// `tab <id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSelectRequest {
    /// `t2` or `2`.
    pub tab: String,
    pub timeout_ms: u64,
}

/// `tab close [id]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabCloseRequest {
    /// The tab to close; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// On the last tab, close the session instead of leaving a blank tab.
    #[serde(default)]
    pub close_browser: bool,
    pub timeout_ms: u64,
}

/// What a navigation waits for (`--wait`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitUntil {
    /// The load event, then the page settles (network idle and a quiet
    /// DOM), for a few seconds at most.
    #[default]
    Settle,
    /// The load event: the document and its images, scripts and styles.
    Load,
    /// `DOMContentLoaded`: the document is parsed.
    DomContentLoaded,
    /// No requests in flight for 500 ms, not counting WebSockets,
    /// beacons, event streams and requests open longer than 10 s.
    NetworkIdle,
    /// The server answered and the new document replaced the old one.
    Commit,
    /// Nothing: return once the navigation is sent.
    None,
}

impl WaitUntil {
    /// The `--wait` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Settle => "settle",
            Self::Load => "load",
            Self::DomContentLoaded => "domcontentloaded",
            Self::NetworkIdle => "networkidle",
            Self::Commit => "commit",
            Self::None => "none",
        }
    }
}

/// The navigations of `goto`, `back`, `forward` and `reload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Navigation {
    /// Load a URL (see [`crate::normalize_url`]); a relative one resolves
    /// against `base_url`, else the page's URL.
    Goto {
        url: String,
        #[serde(default)]
        base_url: Option<String>,
    },
    /// One history entry back.
    Back,
    /// One history entry forward.
    Forward,
    /// Load the page again; `hard` bypasses the cache.
    Reload {
        #[serde(default)]
        hard: bool,
    },
}

/// `goto`, `back`, `forward` and `reload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigateRequest {
    pub navigation: Navigation,
    #[serde(default)]
    pub wait: WaitUntil,
    /// `--tab`: the tab to navigate; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// Bounds the whole navigation, the wait included.
    pub timeout_ms: u64,
}

/// What `wait` waits for: one condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitFor {
    /// `--load`: the main document's load event.
    Load,
    /// `--domcontentloaded`: the main document is parsed.
    DomContentLoaded,
    /// `--idle`: network idle (see [`WaitUntil::NetworkIdle`]).
    Idle,
    /// `--settle`: network idle and no DOM changes for 300 ms (3 s at
    /// most for the DOM part).
    Settle,
    /// `<locator> [--state]`: an element in a state.
    Element {
        locator: String,
        #[serde(default)]
        state: ElementState,
    },
    /// `--text` (or `--text-gone`): visible text in the page, or in the
    /// first element `within` names.
    Text {
        text: String,
        #[serde(default)]
        within: Option<String>,
        #[serde(default)]
        gone: bool,
    },
    /// `--url`: the main frame's URL matches a pattern.
    Url { pattern: String },
    /// `--fn`: a JavaScript expression is truthy (polled in the page).
    Function { expression: String },
    /// `--request`: a request whose URL matches.
    Request { pattern: String },
    /// `--response [--status]`: a response whose URL (and status) match.
    Response {
        pattern: String,
        #[serde(default)]
        status: Option<u16>,
    },
    /// `wait <ms>`: a fixed delay.
    Delay { ms: u64 },
    /// `--download`: a download that started since the previous command
    /// began has finished.
    Download,
    /// `--console`: a console entry (any level or source) logged after
    /// the wait began whose text or URL matches a regular expression.
    Console { pattern: String },
    /// `--binding`: a call of a function `expose` put in the page (its
    /// `binding NAME(…)` console entry), after the wait began.
    Binding { name: String },
}

/// The state `wait <locator>` waits for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElementState {
    /// A match has a non-empty box and is not `visibility: hidden`.
    #[default]
    Visible,
    /// No match is visible (or there is none).
    Hidden,
    /// A match is in the document.
    Attached,
    /// No match is in the document.
    Detached,
    /// A match is not disabled (nor inside `aria-disabled="true"`).
    Enabled,
}

impl ElementState {
    /// The `--state` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
            Self::Attached => "attached",
            Self::Detached => "detached",
            Self::Enabled => "enabled",
        }
    }
}

/// `wait`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitRequest {
    pub condition: WaitFor,
    /// `--tab`: the tab to watch; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--strict-refs`: a ref whose element is gone fails instead of
    /// being re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    /// Bounds the wait.
    pub timeout_ms: u64,
    /// The output budget (0 = none), for `--fn`'s value.
    pub max_chars: usize,
}

/// What `expect <locator> <matcher>` checks of the element(s).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "matcher", rename_all = "kebab-case")]
pub enum Matcher {
    Visible,
    /// Not visible, or no element matches.
    Hidden,
    /// At least one element matches.
    Attached,
    /// No element matches.
    Detached,
    Enabled,
    Disabled,
    /// Enabled and not read-only.
    Editable,
    Checked,
    Unchecked,
    Focused,
    /// Some of the element is in the viewport.
    InViewport,
    /// Its text contains `text` (all of it with `exact`); `/re/` is a
    /// regex.
    Text {
        text: String,
        #[serde(default)]
        exact: bool,
    },
    /// A field's value is `value` (or matches `/re/`).
    Value {
        value: String,
    },
    /// A multiple `<select>`'s values are these, in any order.
    Values {
        values: Vec<String>,
    },
    /// The attribute is there (with this value, or matching `/re/`).
    Attr {
        name: String,
        #[serde(default)]
        value: Option<String>,
    },
    /// It has these classes (space-separated), or its `class` matches
    /// `/re/`.
    Class {
        name: String,
    },
    /// A computed style property has this value (colours compare by
    /// colour: `red` is `rgb(255, 0, 0)`), or matches `/re/`.
    Css {
        property: String,
        value: String,
    },
    /// How many elements match: exactly `count`, and at least `min`, at
    /// most `max`.
    Count {
        #[serde(default)]
        count: Option<u64>,
        #[serde(default)]
        min: Option<u64>,
        #[serde(default)]
        max: Option<u64>,
    },
}

/// The accessibility level `expect --a11y` checks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum A11yLevel {
    A,
    #[default]
    AA,
}

impl A11yLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::AA => "AA",
        }
    }
}

/// What `expect` asserts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expectation {
    /// `expect <locator> <matcher> [args]`.
    Element { locator: String, matcher: Matcher },
    /// `--url`: the main frame's URL matches (a part, a glob or `/re/`).
    Url { pattern: String },
    /// `--title`: the page's title contains `text` (all of it with
    /// `exact`), or matches `/re/`.
    Title {
        text: String,
        #[serde(default)]
        exact: bool,
    },
    /// `--no-errors`: no uncaught exception or console error since
    /// `since` (a mark; the current document if `None`); with `all_tabs`,
    /// in every tab of the session (or context) since it opened.
    NoErrors {
        #[serde(default)]
        since: Option<String>,
        #[serde(default)]
        all_tabs: bool,
    },
    /// `--no-failed-requests`: no request failed or got a status of 400
    /// or more since `since`; only those to `origin` (`self`: the page's)
    /// if given; with `all_tabs`, in every tab of the session (or
    /// context) since it opened.
    NoFailedRequests {
        #[serde(default)]
        since: Option<String>,
        #[serde(default)]
        origin: Option<String>,
        #[serde(default)]
        all_tabs: bool,
    },
    /// `--a11y`: the built-in accessibility rules of `level`, in the
    /// page or the first element `scope` matches.
    A11y {
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        level: A11yLevel,
    },
    /// `expect mock-state <name> <check>`: a named value or collection of
    /// the mock sets in use (plan 11, milestone 11.3).
    MockState { name: String, check: MockStateCheck },
    /// `expect request <METHOD> <path-glob>` and `expect no-request`: the
    /// requests the pages of the session (or context) sent (plan 11,
    /// milestone 11.5).
    Request(RequestCheck),
}

/// `expect request` and `expect no-request`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestCheck {
    /// An HTTP method, or `*` for any.
    pub method: String,
    /// A path glob (`/api/orders/*`), `**/…`, a full-URL glob, or `*`;
    /// `?k=v` after it adds query conditions.
    pub target: String,
    /// `--query k=v`: query parameters it has (values are globs).
    #[serde(default)]
    pub query: Vec<(String, String)>,
    /// `--header 'Name: value'`: headers it has (values are globs).
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// `--json-body`: its JSON body contains this (as a mock's
    /// `match.json`).
    #[serde(default)]
    pub json: Option<serde_json::Value>,
    /// `--count N`: exactly N such requests.
    #[serde(default)]
    pub count: Option<u64>,
    /// `--min N` (1 unless `count` or `max` is given).
    #[serde(default)]
    pub min: Option<u64>,
    /// `--max N`.
    #[serde(default)]
    pub max: Option<u64>,
    /// `--since <mark>`: only requests sent after the mark.
    #[serde(default)]
    pub since: Option<String>,
    /// `expect no-request`: none was sent.
    #[serde(default)]
    pub none: bool,
}

/// What `expect mock-state <name>` checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum MockStateCheck {
    /// `count N` (or `--min`/`--max`): how many items a collection (or a
    /// list value) has.
    Count {
        #[serde(default)]
        count: Option<u64>,
        #[serde(default)]
        min: Option<u64>,
        #[serde(default)]
        max: Option<u64>,
    },
    /// `value V`: the value (a collection's items) is V.
    Value { value: serde_json::Value },
    /// `contains V`: some item of a collection (or a list value) has V,
    /// as a mock's `match.json` does; an object value has V's keys.
    Contains { value: serde_json::Value },
}

/// `expect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectRequest {
    pub expectation: Expectation,
    /// `--not`: the matcher must not hold.
    #[serde(default)]
    pub not: bool,
    /// `--tab`: the tab to check; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--strict-refs`: a ref whose element is gone fails instead of
    /// being re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    /// How long it retries until the assertion holds.
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `mark <name>`: a named point in a tab's console and network logs, for
/// `expect --no-errors --since <name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkRequest {
    pub name: String,
    /// `--tab`: the tab to mark; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
}

/// `expect-snapshot <name>`: the page's `snapshot -i` text (refs left
/// out) against a baseline file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectSnapshotRequest {
    /// The baseline's name, for messages.
    pub name: String,
    /// The baseline file, absolute.
    pub path: PathBuf,
    /// `--scope`: only this element's subtree.
    #[serde(default)]
    pub scope: Option<String>,
    /// `--full`: every node, not only what `snapshot -i` shows.
    #[serde(default)]
    pub full: bool,
    /// `--update`: store the page as the baseline instead of comparing.
    #[serde(default)]
    pub update: bool,
    /// Store the page when there is no baseline yet (else that fails).
    #[serde(default)]
    pub create: bool,
    /// `--dir` as typed, for hints that repeat the command.
    #[serde(default)]
    pub dir: Option<String>,
    /// `--tab`: the tab to check; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--strict-refs`, for a ref in `--scope`.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    /// How long it reads the page again until it matches.
    pub timeout_ms: u64,
    /// The output budget (0 = none); also bounds a failure's diff.
    pub max_chars: usize,
}

/// `expect-screenshot <name>`: a screenshot against a baseline PNG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectScreenshotRequest {
    /// The baseline's name, for messages.
    pub name: String,
    /// The baseline file, absolute.
    pub path: PathBuf,
    /// Capture only this element.
    #[serde(default)]
    pub locator: Option<String>,
    /// `--full`: the whole page, not only the viewport.
    #[serde(default)]
    pub full: bool,
    /// `--full <loc>`: the container the page scrolls in.
    #[serde(default)]
    pub container: Option<String>,
    /// `--mask`: locators whose matches are painted over.
    #[serde(default)]
    pub mask: Vec<String>,
    /// `--threshold`: how different (0 to 1, in YIQ colour distance) a
    /// pixel may be before it counts as changed.
    pub threshold: f64,
    /// `--max-diff-pixels`: how many changed pixels still pass.
    #[serde(default)]
    pub max_diff_pixels: u64,
    /// `--update`: store the page as the baseline instead of comparing.
    #[serde(default)]
    pub update: bool,
    /// Store the page when there is no baseline yet (else that fails).
    #[serde(default)]
    pub create: bool,
    /// `--dir` as typed, for hints that repeat the command.
    #[serde(default)]
    pub dir: Option<String>,
    /// `--tab`: the tab to capture; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--strict-refs`, for refs in the locators.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    /// How long it takes screenshots again until one matches.
    pub timeout_ms: u64,
}

/// `dialog accept [text]` and `dialog dismiss`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogRequest {
    /// Accept (else dismiss) the dialog.
    pub accept: bool,
    /// The text to type into a prompt before accepting it.
    #[serde(default)]
    pub text: Option<String>,
    /// `--tab`: the tab whose dialog to answer; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
}

/// `downloads`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadsRequest {
    /// `--clear`: delete the files of finished downloads and forget them.
    #[serde(default)]
    pub clear: bool,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// Where `console`, `errors` and `network` start (`--since`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Since {
    /// After the entries this command showed last time (per tab).
    #[default]
    Last,
    /// After entry `seq` (`0`: all of them).
    Seq { seq: u64 },
    /// Logged in the last `ms` milliseconds.
    Ago { ms: u64 },
    /// After the tab's `mark <name>`.
    Mark { name: String },
}

/// `console` and `errors`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleRequest {
    /// `errors`: only uncaught exceptions, `console.error`s and other
    /// errors, and failed loads from the page's own origin.
    #[serde(default)]
    pub errors: bool,
    /// `--tab`: the tab to read; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--level`: the least severe level shown (`console` only).
    pub level: protocol::schema::Level,
    #[serde(default)]
    pub since: Since,
    /// `--grep`: a regular expression the text (or URL) must match.
    #[serde(default)]
    pub grep: Option<String>,
    /// `--limit`: the most entries shown, the newest (0 = all).
    pub limit: usize,
    /// For `--follow`: with nothing new, wait up to this long for an
    /// entry before answering.
    #[serde(default)]
    pub wait_ms: u64,
    /// `--sourcemaps`: resolve stack frames with source maps (always on
    /// for localhost pages).
    #[serde(default)]
    pub sourcemaps: bool,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// A range of HTTP statuses for `network --status` (`404`: 404 to 404,
/// `4xx`: 400 to 499).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusRange {
    pub min: u16,
    pub max: u16,
}

impl StatusRange {
    /// Parses `404`, `4xx` or `400-499`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        if let Some((min, max)) = text.split_once('-') {
            let (min, max) = (min.trim().parse().ok()?, max.trim().parse().ok()?);
            return ((100..=999).contains(&min) && (min..=999).contains(&max))
                .then_some(Self { min, max });
        }
        if let Some(class) = text.strip_suffix("xx") {
            let class: u16 = class.parse().ok()?;
            return (1..=9).contains(&class).then_some(Self {
                min: class * 100,
                max: class * 100 + 99,
            });
        }
        let status: u16 = text.parse().ok()?;
        (100..=999).contains(&status).then_some(Self {
            min: status,
            max: status,
        })
    }

    pub fn contains(self, status: u16) -> bool {
        (self.min..=self.max).contains(&status)
    }
}

/// `network`: the tab's requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRequest {
    /// `--tab`: the tab to read; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--failed`: only requests that failed or got a status of 400 or
    /// more.
    #[serde(default)]
    pub failed: bool,
    /// `--type`: only these resource types (lower case: `xhr`, `fetch`,
    /// …).
    #[serde(default)]
    pub types: Vec<String>,
    /// `--url`: a URL pattern (a glob, `/regex/` or a part of the URL).
    #[serde(default)]
    pub url: Option<String>,
    /// `--method`: only this HTTP method.
    #[serde(default)]
    pub method: Option<String>,
    /// `--status`: only statuses in one of these ranges.
    #[serde(default)]
    pub status: Vec<StatusRange>,
    #[serde(default)]
    pub since: Since,
    /// `--limit`: the most requests shown, the newest (0 = all).
    pub limit: usize,
    /// For `--follow`: with nothing new, wait up to this long for a
    /// request (or the end of one shown pending) before answering.
    #[serde(default)]
    pub wait_ms: u64,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `network show`: one request of the tab's log in full.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkShowRequest {
    #[serde(default)]
    pub tab: Option<String>,
    /// The request's id in the log (`#42`).
    pub id: u64,
    /// `--headers`: list its headers.
    #[serde(default)]
    pub headers: bool,
    /// `--body`: fetch the response body from Chrome.
    #[serde(default)]
    pub body: bool,
    /// `--request-body`: the request body.
    #[serde(default)]
    pub request_body: bool,
    /// `-o`: save the response body (decoded, text or binary) to this
    /// absolute path instead of showing it.
    #[serde(default)]
    pub output: Option<PathBuf>,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `network clear`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkClearRequest {
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
}

/// What `har` does with response bodies (`har start --content`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarContent {
    /// In the file: text as it is, binary bodies in base64.
    #[default]
    Embed,
    /// Left out (sizes and MIME types stay).
    Omit,
}

impl HarContent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Embed => "embed",
            Self::Omit => "omit",
        }
    }
}

/// `har start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarStartRequest {
    /// `--tab`: record only this tab; every tab if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    #[serde(default)]
    pub content: HarContent,
    pub timeout_ms: u64,
}

/// `har stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarStopRequest {
    /// `-o`: an absolute path; `<session dir>/hars/<n>.har` if `None`.
    #[serde(default)]
    pub path: Option<PathBuf>,
    pub timeout_ms: u64,
}

/// `mock record`: the HAR of the requests in `scope`, returned instead
/// of written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarExportRequest {
    /// `--tab`: only this tab's requests; else every tab of the context.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--scope` globs (`/api/**`, `https://api.example.com/**`); the
    /// `fetch` and XHR requests if empty.
    #[serde(default)]
    pub scope: Vec<String>,
    pub timeout_ms: u64,
}

/// `trace start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceStartRequest {
    /// `--tab`: the tab whose session records; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--screenshots`: film strip frames in the trace.
    #[serde(default)]
    pub screenshots: bool,
    /// `--categories`: trace categories (`-name` leaves one out); the
    /// Performance panel's if empty.
    #[serde(default)]
    pub categories: Vec<String>,
    pub timeout_ms: u64,
}

/// `trace stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceStopRequest {
    /// `-o`: an absolute path; `<session dir>/traces/<n>.json` if `None`.
    /// A path ending in `.gz` gets a gzipped trace.
    #[serde(default)]
    pub path: Option<PathBuf>,
    pub timeout_ms: u64,
}

/// `perf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerfRequest {
    /// `--tab`: the tab to measure; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
}

/// `coverage start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageStartRequest {
    /// `--tab`: the tab to measure; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
}

/// `coverage stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageStopRequest {
    /// `--limit`: the files with the most unused bytes (0 = all).
    pub limit: usize,
    pub timeout_ms: u64,
    #[serde(default)]
    pub max_chars: usize,
}

/// One routing rule, as `route` and `block` give it (plan 07, milestone
/// 7.1). The engine checks and compiles it when it is added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteSpec {
    /// The URL pattern: a glob (`**/api/*`), a `/regex/`, or a part of the
    /// URL, as for `wait --url`.
    pub pattern: String,
    /// What to call the rule instead of its pattern (`block --ads`).
    #[serde(default)]
    pub label: Option<String>,
    /// `--method`: only requests with one of these methods; any if empty.
    #[serde(default)]
    pub methods: Vec<String>,
    /// `--resource-type`: only requests of these types (as `network
    /// --type` names them); any if empty.
    #[serde(default)]
    pub resource_types: Vec<String>,
    /// `--times`: the rule is removed after this many matches.
    #[serde(default)]
    pub times: Option<u32>,
    /// `--delay`: how long to hold a matched request before acting.
    #[serde(default)]
    pub delay_ms: u64,
    /// The browser context whose requests the rule applies to; every
    /// context's if `None`. For per-context mocks (plan 11).
    #[serde(default)]
    pub context: Option<String>,
    pub action: RouteAction,
}

/// What a routing rule does with a request it matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteAction {
    /// Answer it without the network (`--status`, `--body`,
    /// `--json-body`, `--body-file`, `--header`).
    Fulfill {
        status: u16,
        #[serde(default)]
        headers: Vec<(String, String)>,
        #[serde(default)]
        body: RouteBody,
    },
    /// Fail it with a network error (`--abort [reason]`): `failed`,
    /// `blockedbyclient`, `timedout`, … (Chrome's `Network.ErrorReason`,
    /// any case).
    Abort { reason: String },
    /// Send it on, changed (`--set-header`, `--remove-header`,
    /// `--rewrite-url`), or as it is (`--delay` alone).
    Continue {
        #[serde(default)]
        set_headers: Vec<(String, String)>,
        #[serde(default)]
        remove_headers: Vec<String>,
        #[serde(default)]
        url: Option<String>,
    },
    /// Send it on, then deep-merge `patch` into its JSON response (JSON
    /// merge patch: `null` removes a key).
    PatchJson { patch: Value },
}

/// The body of a fulfilled request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteBody {
    #[default]
    Empty,
    /// `--body`: text, `text/plain` unless a header says otherwise.
    Text { text: String },
    /// `--json-body`: `application/json`.
    Json { value: Value },
    /// `--body-file`: read when the rule is added; its type follows the
    /// extension. An absolute path.
    File { path: PathBuf },
}

/// `route` and `block`: add routing rules, tried after the existing ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRequest {
    pub rules: Vec<RouteSpec>,
    pub timeout_ms: u64,
}

/// `routes`: the rules, or with `log` the requests routing saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutesRequest {
    /// `--log`: list the requests routing saw instead.
    #[serde(default)]
    pub log: bool,
    /// `--limit`: with `log`, the newest this many (0 = all).
    pub limit: usize,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `unroute`: remove rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnrouteRequest {
    /// The rules' ids (`r1` or `1`).
    #[serde(default)]
    pub ids: Vec<String>,
    /// `--all`: every rule.
    #[serde(default)]
    pub all: bool,
    pub timeout_ms: u64,
}

/// `mock use`: apply mock sets (plan 11, milestone 11.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockUseRequest {
    /// The sets, in layering order (later ones are tried first).
    pub sets: Vec<mock::Source>,
    /// `--add`: layer them on the sets in use instead of replacing them.
    #[serde(default)]
    pub add: bool,
    /// Seeds `{{uuid}}`, `{{faker.*}}` and latency ranges.
    #[serde(default)]
    pub seed: u64,
    /// A test run's tag for the context's requests (`--test-id`).
    #[serde(default)]
    pub test: Option<MockTestTag>,
    pub timeout_ms: u64,
}

/// `mock use --test-id ID --test-origin URL…` (plan 11.9): requests to
/// these origins that go on to the network get `x-chrome-cdp-test: ID`,
/// so `mock serve` gives the test state of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockTestTag {
    pub id: String,
    /// `http://localhost:5173`.
    pub origins: Vec<String>,
}

/// `mock ws send`: hand a message to the page's mocked sockets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockWsSendRequest {
    /// `*`, a path glob or a full-URL glob.
    pub url: String,
    pub message: String,
    /// `--base64`: `message` is base64, sent as a binary message.
    #[serde(default)]
    pub base64: bool,
    pub max_chars: usize,
    pub timeout_ms: u64,
}

/// `mock off`: turn mock sets off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockOffRequest {
    /// The sets' names; none turns every set off.
    #[serde(default)]
    pub sets: Vec<String>,
    pub timeout_ms: u64,
}

/// `mock list`: the sets in use and their mocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockListRequest {
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `mock log`: the requests the mocks saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockLogRequest {
    /// `--unmatched`: only unmatched requests.
    #[serde(default)]
    pub unmatched: bool,
    /// `--all`: requests no mock set had anything to do with too.
    #[serde(default)]
    pub all: bool,
    /// The newest this many (0 = all).
    pub limit: usize,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `mock explain`: which mock answers a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockExplainRequest {
    pub method: String,
    /// An absolute URL.
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
    /// Show at most this many mocks (0 = all).
    pub limit: usize,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `http`: a request sent from the browser context, outside the page
/// (plan 11, milestone 11.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    /// Upper case.
    pub method: String,
    /// An absolute http(s) URL.
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `mock state`: the mocks' named values and collections (plan 11,
/// milestone 11.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockStateRequest {
    /// One named value or collection to show; all of them if `None`.
    #[serde(default)]
    pub name: Option<String>,
    /// `--set name=value`: named values to change first (`null` removes
    /// one).
    #[serde(default)]
    pub set: Vec<(String, serde_json::Value)>,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `mock seed`: fill a collection with other items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockSeedRequest {
    /// The collection's name.
    pub name: String,
    /// A data file: absolute, or relative to the mocks directory, else to
    /// `cwd`.
    #[serde(default)]
    pub file: Option<String>,
    /// The items themselves (read from stdin).
    #[serde(default)]
    pub items: Option<serde_json::Value>,
    /// The CLI's directory.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// `init-script add`: a script for every new document (plan 07,
/// milestone 7.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitScriptAddRequest {
    /// The script's source.
    pub source: String,
    /// The file it was read from (absolute), for `init-script list`.
    #[serde(default)]
    pub file: Option<PathBuf>,
    /// `--world`: the page's own world, or chrome-cdp's isolated one.
    #[serde(default)]
    pub world: EvalWorld,
    /// `--now`: run it in the documents there are too.
    #[serde(default)]
    pub now: bool,
    pub timeout_ms: u64,
}

/// `init-script list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitScriptListRequest {
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `init-script remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitScriptRemoveRequest {
    /// The scripts' ids (`s1` or `1`).
    #[serde(default)]
    pub ids: Vec<String>,
    /// `--all`: every script.
    #[serde(default)]
    pub all: bool,
    pub timeout_ms: u64,
}

/// `expose` (with names) and `unexpose`: functions pages can call, whose
/// calls go to the tab's console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposeRequest {
    /// Global function names; `expose` without any lists them.
    #[serde(default)]
    pub names: Vec<String>,
    /// Remove the names instead (`unexpose`).
    #[serde(default)]
    pub remove: bool,
    /// With `remove`: every one.
    #[serde(default)]
    pub all: bool,
    pub timeout_ms: u64,
}

/// `cookies`, `cookies set` and `cookies clear` (plan 07, milestone
/// 7.4): the browser's cookies, of every tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CookiesRequest {
    pub op: CookiesOp,
    /// The output budget of the list (0 = none).
    #[serde(default)]
    pub max_chars: usize,
    pub timeout_ms: u64,
}

/// What `cookies` does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CookiesOp {
    /// List them (all, or those the filter keeps).
    List {
        #[serde(default)]
        filter: CookieFilter,
    },
    /// Set one.
    Set { cookie: CookieSpec },
    /// Remove them (all, or those the filter keeps).
    Clear {
        #[serde(default)]
        filter: CookieFilter,
    },
}

/// Which cookies `cookies` and `cookies clear` take; all when empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieFilter {
    /// `--url`: the cookies a request to this URL would carry.
    #[serde(default)]
    pub url: Option<String>,
    /// `--domain`: cookies of this domain or its subdomains.
    #[serde(default)]
    pub domain: Option<String>,
    /// `--name`: only cookies with this name.
    #[serde(default)]
    pub name: Option<String>,
}

impl CookieFilter {
    pub fn is_empty(&self) -> bool {
        self.url.is_none() && self.domain.is_none() && self.name.is_none()
    }
}

/// `cookies set <name>=<value>`'s cookie.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CookieSpec {
    pub name: String,
    pub value: String,
    /// `--url`: the cookie's URL (its host, path and scheme). Without it
    /// or `domain`, the active tab's URL.
    #[serde(default)]
    pub url: Option<String>,
    /// `--domain`: `example.com` for that host only, `.example.com` for
    /// its subdomains too (as Playwright's `addCookies` has it).
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    /// `Strict`, `Lax` or `None`.
    #[serde(default)]
    pub same_site: Option<String>,
    /// Seconds since 1970; a session cookie without.
    #[serde(default)]
    pub expires: Option<f64>,
}

/// `storage local|session [list|get|set|remove|clear]` (plan 07,
/// milestone 7.4): a frame's Web Storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageRequest {
    pub area: StorageArea,
    pub op: StorageOp,
    /// `--origin`: a frame of this origin; the main frame without.
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub tab: Option<String>,
    /// The output budget (0 = none).
    #[serde(default)]
    pub max_chars: usize,
    pub timeout_ms: u64,
}

/// `localStorage` or `sessionStorage`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageArea {
    #[default]
    Local,
    Session,
}

impl StorageArea {
    /// `local` or `session`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Session => "session",
        }
    }

    /// The global's name: `localStorage` or `sessionStorage`.
    pub fn global(self) -> &'static str {
        match self {
            Self::Local => "localStorage",
            Self::Session => "sessionStorage",
        }
    }
}

/// What `storage` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StorageOp {
    List,
    Get { key: String },
    Set { key: String, value: String },
    Remove { key: String },
    Clear,
}

impl StorageOp {
    /// The action's name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Get { .. } => "get",
            Self::Set { .. } => "set",
            Self::Remove { .. } => "remove",
            Self::Clear => "clear",
        }
    }
}

/// `state save <file>`: writes the session's cookies and storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateSaveRequest {
    /// Where to write it (absolute).
    pub path: PathBuf,
    /// `--indexeddb`: also each origin's IndexedDB databases.
    #[serde(default)]
    pub indexed_db: bool,
    pub timeout_ms: u64,
}

/// `state load <file>`: gives the session a storage state's cookies and
/// storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateLoadRequest {
    pub state: StorageState,
    /// The file it was read from (absolute), for the output.
    #[serde(default)]
    pub path: Option<PathBuf>,
    pub timeout_ms: u64,
}

/// `emulate`, `viewport`, `geo`, `tz`, `locale`, `ua`, `media`,
/// `offline`, `throttle`, `cpu-throttle` and `time`: change what the
/// session emulates (or only show it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmulateRequest {
    pub change: EmulationChange,
    pub timeout_ms: u64,
}

/// One change to the session's emulation. `None` puts a setting back to
/// the browser's own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EmulationChange {
    /// Change nothing (`emulate` alone).
    Show,
    /// Every setting back to the browser's own (`emulate reset`).
    Reset,
    /// A device of the table: viewport, touch, mobile and user agent.
    Device {
        name: String,
    },
    Viewport {
        viewport: Option<Viewport>,
    },
    /// Also grants the geolocation permission to every origin.
    Geolocation {
        geolocation: Option<Geolocation>,
    },
    Timezone {
        timezone: Option<String>,
    },
    Locale {
        locale: Option<String>,
    },
    UserAgent {
        user_agent: Option<String>,
    },
    /// The features given change; `None` resets them all.
    Media {
        media: Option<Media>,
    },
    Offline {
        offline: bool,
    },
    Throttle {
        throttle: Option<Throttle>,
    },
    /// 1 or `None` for full speed.
    CpuThrottle {
        rate: Option<f64>,
    },
    /// Pages see this time (ms since the Unix epoch) until it changes.
    TimeFreeze {
        epoch_ms: i64,
    },
    /// Moves the frozen time on.
    TimeAdvance {
        ms: u64,
    },
    /// Freezes the time at `epoch_ms` (the frozen time, else now) and
    /// stops timers and animation frames (plan 14.2).
    TimePause {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        epoch_ms: Option<i64>,
    },
    /// Moves the paused time on by `ms`, firing every callback due on the
    /// way.
    TimeRunFor {
        ms: u64,
    },
    /// Moves the paused time on to `epoch_ms`, as `TimeRunFor`.
    TimePauseAt {
        epoch_ms: i64,
    },
    /// Lets paused timers run in real time again; the time stays frozen.
    TimeResume,
    TimeReset,
}

/// `permissions`: list, grant, deny or reset permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsRequest {
    pub op: PermissionsOp,
    /// Permission names as the Permissions API has them (`geolocation`).
    #[serde(default)]
    pub names: Vec<String>,
    /// `--origin`: only this origin; every origin if `None`.
    #[serde(default)]
    pub origin: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionsOp {
    List,
    Grant,
    Deny,
    /// Back to asking (all of them without names).
    Reset,
}

impl PermissionsOp {
    /// Its subcommand of `permissions` (`list` is `permissions` alone).
    pub fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Grant => "grant",
            Self::Deny => "deny",
            Self::Reset => "reset",
        }
    }
}

/// `highlight`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightRequest {
    /// `--tab`: the tab to highlight in; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub locator: String,
    /// How long the highlight stays; 0 keeps it until the next one.
    pub duration_ms: u64,
    #[serde(default)]
    pub strict_refs: bool,
    #[serde(default)]
    pub testid_attribute: Option<String>,
    pub timeout_ms: u64,
}

/// `inspect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectRequest {
    /// `--tab`: the tab to open DevTools for; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--panel`: DevTools' id of the panel to open (`elements`,
    /// `console`, `timeline`, …); DevTools' choice if `None`.
    #[serde(default)]
    pub panel: Option<String>,
    pub timeout_ms: u64,
}

/// An image format (`screenshot --format`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ImageFormat {
    /// Its name, as typed.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
        }
    }

    /// The file extension it is saved with.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    /// The format a file name's extension names (`.jpg`, `.JPEG`, …).
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}

/// How many image pixels a CSS pixel takes (`screenshot --scale`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShotScale {
    /// One: the image's pixels are CSS pixels, whatever the screen.
    #[default]
    Css,
    /// The device pixel ratio: as sharp as the screen shows it.
    Device,
}

/// `screenshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotRequest {
    /// `--tab`: the tab to capture; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// Capture only this element (scrolled into view first).
    #[serde(default)]
    pub locator: Option<String>,
    /// `--full`: the whole page, not only the viewport.
    #[serde(default)]
    pub full: bool,
    /// `--full <loc>`: the container the page scrolls in, whose whole
    /// content is captured; found on its own if `None`.
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub format: ImageFormat,
    /// `--quality`: 0 to 100, for jpeg and webp.
    #[serde(default)]
    pub quality: Option<u8>,
    /// `-o`: an absolute path; `<session dir>/shots/<n>.<ext>` if `None`.
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub scale: ShotScale,
    /// `--max-size`: the longest side in image pixels; a larger capture is
    /// scaled down to fit (0 = no limit).
    pub max_size: u32,
    /// `--omit-background`: a transparent background where the page has
    /// none (png and webp).
    #[serde(default)]
    pub omit_background: bool,
    /// `--mask`: locators whose matches are painted over.
    #[serde(default)]
    pub mask: Vec<String>,
    /// `--annotate`: label the elements `snapshot -i` gives refs with
    /// those refs.
    #[serde(default)]
    pub annotate: bool,
    /// `--strict-refs`: a ref whose element is gone is an error, never
    /// re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    pub timeout_ms: u64,
}

/// A paper size for `pdf --format`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaperFormat {
    #[default]
    Letter,
    Legal,
    Tabloid,
    A3,
    A4,
    A5,
}

impl PaperFormat {
    /// Its name, as typed.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Letter => "Letter",
            Self::Legal => "Legal",
            Self::Tabloid => "Tabloid",
            Self::A3 => "A3",
            Self::A4 => "A4",
            Self::A5 => "A5",
        }
    }

    /// Width and height in inches, portrait.
    pub fn inches(self) -> (f64, f64) {
        match self {
            Self::Letter => (8.5, 11.0),
            Self::Legal => (8.5, 14.0),
            Self::Tabloid => (11.0, 17.0),
            Self::A3 => (11.69, 16.54),
            Self::A4 => (8.27, 11.69),
            Self::A5 => (5.83, 8.27),
        }
    }
}

/// Page margins in inches (`pdf --margin`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Margins {
    /// Parses one to four lengths, comma-separated, in CSS order (`1cm`;
    /// `10mm,20mm`: top and bottom, then left and right; `top,right,
    /// bottom,left`). A length is a number with `in`, `cm`, `mm` or `px`
    /// (1/96 in); a bare `0` needs no unit.
    pub fn parse(text: &str) -> Result<Self, String> {
        let lengths = text
            .split(',')
            .map(|part| inches(part.trim()))
            .collect::<Result<Vec<f64>, String>>()?;
        let [top, right, bottom, left] = match lengths.as_slice() {
            [all] => [*all; 4],
            [y, x] => [*y, *x, *y, *x],
            [top, x, bottom] => [*top, *x, *bottom, *x],
            [top, right, bottom, left] => [*top, *right, *bottom, *left],
            _ => return Err("give 1 to 4 lengths, comma-separated".to_owned()),
        };
        Ok(Self {
            top,
            right,
            bottom,
            left,
        })
    }
}

/// A length such as `1cm`, `0.5in`, `10mm`, `20px` or `0`, in inches.
fn inches(text: &str) -> Result<f64, String> {
    let bad = || format!("`{text}` is not a length; use e.g. 1cm, 10mm, 0.5in or 20px");
    let split = text
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number.trim().parse().map_err(|_| bad())?;
    if !number.is_finite() || number < 0.0 {
        return Err(bad());
    }
    let per_inch = match unit.to_ascii_lowercase().as_str() {
        "" if number == 0.0 => 1.0,
        "in" => 1.0,
        "cm" => 2.54,
        "mm" => 25.4,
        "px" => 96.0,
        _ => return Err(bad()),
    };
    Ok(number / per_inch)
}

/// `pdf`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfRequest {
    /// `--tab`: the tab to print; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `-o`: an absolute path; `<session dir>/pdfs/<n>.pdf` if `None`.
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub format: PaperFormat,
    #[serde(default)]
    pub landscape: bool,
    /// `--background`: print background colours and images.
    #[serde(default)]
    pub background: bool,
    /// `--margin`; Chrome's (1cm) if `None`.
    #[serde(default)]
    pub margin: Option<Margins>,
    /// `--pages`: the pages to print, e.g. `1-3,5`; all if `None`.
    #[serde(default)]
    pub pages: Option<String>,
    /// `--css-page-size`: the page's CSS `@page` size wins over `format`.
    #[serde(default)]
    pub css_page_size: bool,
    pub timeout_ms: u64,
}

/// Which engine reads the page for `snapshot` (`--engine`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotEngine {
    /// Chrome's accessibility tree (`Accessibility.getFullAXTree`).
    Ax,
    /// chrome-cdp's DOM walker, injected into its isolated world.
    Dom,
}

/// `snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRequest {
    /// `--tab`: the tab to read; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--interactive`: only nodes with refs and their landmarks.
    #[serde(default)]
    pub interactive: bool,
    /// `--scope`: only the subtree of the element this locator names.
    #[serde(default)]
    pub scope: Option<String>,
    /// `--depth`: how many levels to show.
    #[serde(default)]
    pub depth: Option<u32>,
    /// `--diff`: only what changed since the tab's last snapshot.
    #[serde(default)]
    pub diff: bool,
    /// `--all-refs`: a ref on every node, not only actionable ones.
    #[serde(default)]
    pub all_refs: bool,
    /// `--all`: do not collapse long lists.
    #[serde(default)]
    pub all: bool,
    /// `--text-limit`: longest text or name shown (0 = no limit).
    pub text_limit: usize,
    /// `--no-text`: leave text out (a skeleton).
    #[serde(default)]
    pub no_text: bool,
    /// `--engine`; the default engine if `None`.
    #[serde(default)]
    pub engine: Option<SnapshotEngine>,
    /// `--strict-refs`: a `--scope` ref whose element is gone is an
    /// error, never re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read (`testid_attribute` in the
    /// config); `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// What a read command reads of an element, or of the page without a
/// locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Read {
    /// `text [loc]`: the visible text of every match (the page's without
    /// a locator).
    Text,
    /// `html [loc]`: the element's content (`outer`: the element too),
    /// pretty-printed unless `raw`.
    Html {
        #[serde(default)]
        outer: bool,
        #[serde(default)]
        raw: bool,
    },
    /// `attr <loc> <name>`: an attribute's value.
    Attr { name: String },
    /// `value <loc>`: a field's current value.
    Value,
    /// `count <loc>`: how many elements match.
    Count,
    /// `box <loc>`: the element's box in the viewport.
    Box,
    /// `markdown [loc]`: the readable content as Markdown.
    Markdown,
}

impl Read {
    /// The command's name.
    pub fn command(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Html { .. } => "html",
            Self::Attr { .. } => "attr",
            Self::Value => "value",
            Self::Count => "count",
            Self::Box => "box",
            Self::Markdown => "markdown",
        }
    }
}

/// `text`, `html`, `attr`, `value`, `count`, `box` and `markdown`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRequest {
    pub read: Read,
    /// The element(s) to read; the page if `None` (`text`, `html`,
    /// `markdown`).
    #[serde(default)]
    pub locator: Option<String>,
    /// `--tab`: the tab to read; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--strict-refs`: a ref whose element is gone is an error, never
    /// re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// What `clipboard` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipboardOp {
    /// `clipboard read [--type T] [-o FILE]`: what the clipboard holds of
    /// one type.
    Read {
        /// `--type`: `text/plain` (the default), `text/html` or
        /// `image/png`.
        #[serde(default)]
        mime: Option<String>,
        /// `-o`: where an image is saved (an absolute path); a numbered
        /// file in the session's `clipboard/` directory if `None`.
        #[serde(default)]
        path: Option<PathBuf>,
    },
    /// `clipboard write [TEXT] [--html HTML] [--file IMAGE.png]`: puts
    /// one item on the clipboard with every type given.
    Write {
        /// The text (`text/plain`); with `html` and no text, the HTML's
        /// text.
        #[serde(default)]
        text: Option<String>,
        /// `--html`: the `text/html` part.
        #[serde(default)]
        html: Option<String>,
        /// `--file`: a PNG image (an absolute path), the `image/png` part.
        #[serde(default)]
        image: Option<PathBuf>,
    },
}

impl ClipboardOp {
    /// `clipboard read`: the clipboard's text.
    #[must_use]
    pub fn read_text() -> Self {
        ClipboardOp::Read {
            mime: None,
            path: None,
        }
    }

    /// `clipboard write <text>`: puts text on the clipboard.
    #[must_use]
    pub fn write_text(text: impl Into<String>) -> Self {
        ClipboardOp::Write {
            text: Some(text.into()),
            html: None,
            image: None,
        }
    }
}

/// `clipboard read` and `clipboard write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardRequest {
    pub op: ClipboardOp,
    /// `--tab`: the tab whose page uses the clipboard; the active one if
    /// `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
    /// The output budget (0 = none), for what `read` prints.
    pub max_chars: usize,
}

/// `find <query>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindRequest {
    /// Words to look for in roles, names, values and text.
    pub query: String,
    /// `--role`: only nodes with this role.
    #[serde(default)]
    pub role: Option<String>,
    /// `--scope`: only the subtree of the element this locator names.
    #[serde(default)]
    pub scope: Option<String>,
    /// `--strict-refs`: a `--scope` ref whose element is gone is an
    /// error, never re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    /// `--limit`: how many matches to show (0 = all).
    pub limit: usize,
    /// `--tab`: the tab to search; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    pub max_chars: usize,
}

/// `locator <LOCATOR>`: the stable locator of one element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocatorRequest {
    /// The element: a ref (e12) or any locator that matches one.
    pub locator: String,
    /// `--all`: every candidate tried, with how many elements each finds.
    #[serde(default)]
    pub all: bool,
    /// `--strict-refs`: a ref whose element is gone is an error, never
    /// re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    /// `--tab`: the tab to look in; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
}

/// Milliseconds from a request as a duration, at least 1 ms.
pub(crate) fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms.max(1))
}

/// A mouse button (`--button`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

impl Button {
    /// Its name, as typed.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

/// A modifier key held during a mouse action (`--modifiers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Modifier {
    Alt,
    Control,
    Meta,
    Shift,
    /// Meta when the browser runs on macOS, Control elsewhere.
    Mod,
}

impl Modifier {
    /// Its name as the output shows it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alt => "Alt",
            Self::Control => "Control",
            Self::Meta => "Meta",
            Self::Shift => "Shift",
            Self::Mod => "Mod",
        }
    }
}

/// Where `scroll --to` goes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScrollTo {
    Top,
    Bottom,
    Left,
    Right,
    /// A scroll offset, in CSS pixels.
    Point {
        x: f64,
        y: f64,
    },
}

/// What a low-level `mouse` command does.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MouseOp {
    /// `mouse move x y`: in `steps` moves from where the mouse is.
    Move { x: f64, y: f64, steps: u32 },
    /// `mouse down [x y]`: press `button` (after moving to the point).
    Down {
        #[serde(default)]
        at: Option<(f64, f64)>,
        #[serde(default)]
        button: Button,
        count: u32,
    },
    /// `mouse up [x y]`: release `button` (after moving to the point);
    /// without one, the button held (left if none or several are).
    Up {
        #[serde(default)]
        at: Option<(f64, f64)>,
        #[serde(default)]
        button: Option<Button>,
        count: u32,
    },
    /// `mouse wheel dx dy`: a wheel event where the mouse is.
    Wheel { dx: f64, dy: f64 },
}

impl MouseOp {
    /// The subcommand's name.
    pub fn command(&self) -> &'static str {
        match self {
            Self::Move { .. } => "move",
            Self::Down { .. } => "down",
            Self::Up { .. } => "up",
            Self::Wheel { .. } => "wheel",
        }
    }
}

/// What `keyboard` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum KeyboardOp {
    /// `keyboard down <key>`: presses the key and holds it.
    Down { key: String },
    /// `keyboard up <key>`, or `keyboard up --all` (`key` is `None`):
    /// releases held keys.
    Up {
        #[serde(default)]
        key: Option<String>,
    },
}

impl KeyboardOp {
    /// The subcommand's name.
    pub fn command(&self) -> &'static str {
        match self {
            Self::Down { .. } => "down",
            Self::Up { .. } => "up",
        }
    }
}

/// What an input action does (plan 05). Each names its element(s) with a
/// locator, except `mouse`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// `click <loc>` (and `dblclick`, which is `count: 2`).
    Click {
        locator: String,
        #[serde(default)]
        button: Button,
        count: u32,
        #[serde(default)]
        modifiers: Vec<Modifier>,
        /// `--position x,y`: from the top-left corner of the element's
        /// border box instead of its centre.
        #[serde(default)]
        position: Option<(f64, f64)>,
        #[serde(default)]
        force: bool,
        /// Typed as `dblclick`: for hints.
        #[serde(default)]
        dblclick: bool,
    },
    /// `hover <loc>`.
    Hover {
        locator: String,
        #[serde(default)]
        modifiers: Vec<Modifier>,
        #[serde(default)]
        position: Option<(f64, f64)>,
        #[serde(default)]
        force: bool,
    },
    /// `tap <loc>`: a touch, when touch emulation is on.
    Tap {
        locator: String,
        #[serde(default)]
        modifiers: Vec<Modifier>,
        #[serde(default)]
        position: Option<(f64, f64)>,
        #[serde(default)]
        force: bool,
    },
    /// `mouse move|down|up|wheel`: raw mouse events at viewport points.
    Mouse(MouseOp),
    /// `keyboard down|up`: holds keys across commands (plan 14).
    Keyboard(KeyboardOp),
    /// `drag <from> <to>`.
    Drag {
        from: String,
        to: String,
        steps: u32,
        #[serde(default)]
        force: bool,
    },
    /// `scroll [loc] [--by dx,dy | --to where]`: wheel events at the
    /// element (the page's centre without one). Neither `by` nor `to`
    /// scrolls the element into view.
    Scroll {
        #[serde(default)]
        locator: Option<String>,
        #[serde(default)]
        by: Option<(f64, f64)>,
        #[serde(default)]
        to: Option<ScrollTo>,
        #[serde(default)]
        force: bool,
    },
    /// `press <keys> [--in loc]`: key chords (`Control+A Backspace`) to
    /// what has focus (`locator`, focused first, if given).
    Press {
        keys: String,
        #[serde(default)]
        locator: Option<String>,
    },
    /// `type <text> [--in loc] [--delay ms]`: key presses for what the
    /// layout types, inserted text for the rest.
    Type {
        text: String,
        #[serde(default)]
        locator: Option<String>,
        /// Between keys.
        #[serde(default)]
        delay_ms: u64,
        #[serde(default)]
        force: bool,
    },
    /// `fill <loc> <text>`: replaces the field's content in one input.
    Fill {
        locator: String,
        text: String,
        #[serde(default)]
        force: bool,
    },
    /// `select <loc> <value|label>...`: options of a native `<select>`.
    Select {
        locator: String,
        values: Vec<String>,
        #[serde(default)]
        force: bool,
    },
    /// `check <loc>` (`checked: true`) and `uncheck <loc>`.
    Check {
        locator: String,
        checked: bool,
        #[serde(default)]
        force: bool,
    },
    /// `focus <loc>`.
    Focus { locator: String },
    /// `blur <loc>`.
    Blur { locator: String },
    /// `upload <loc> <file>...` (or `--clear`): files into a file input,
    /// or into the file chooser that clicking `locator` opens; with
    /// `drop`, dropped on `locator` with drag events.
    Upload {
        locator: String,
        /// Absolute paths; empty with `clear`.
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        clear: bool,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        drop: bool,
    },
}

impl Action {
    /// The command's name, as typed.
    pub fn command(&self) -> &'static str {
        match self {
            Self::Click { dblclick: true, .. } => "dblclick",
            Self::Click { .. } => "click",
            Self::Hover { .. } => "hover",
            Self::Tap { .. } => "tap",
            Self::Mouse(_) => "mouse",
            Self::Keyboard(_) => "keyboard",
            Self::Drag { .. } => "drag",
            Self::Scroll { .. } => "scroll",
            Self::Press { .. } => "press",
            Self::Type { .. } => "type",
            Self::Fill { .. } => "fill",
            Self::Select { .. } => "select",
            Self::Check { checked: true, .. } => "check",
            Self::Check { .. } => "uncheck",
            Self::Focus { .. } => "focus",
            Self::Blur { .. } => "blur",
            Self::Upload { .. } => "upload",
        }
    }
}

/// The input actions: `click`, `dblclick`, `hover`, `tap`, `mouse`,
/// `drag`, `scroll`, `press`, `type`, `fill`, `select`, `check`,
/// `uncheck`, `focus`, `blur` and `upload`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action: Action,
    /// `--tab`: the tab to act in; the active one if `None`.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--strict-refs`: a ref whose element is gone is an error, never
    /// re-resolved by role and name.
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` locators read; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
    /// How long to wait for the element to be actionable.
    pub timeout_ms: u64,
    /// `--no-wait-after`: return once the input is sent, without waiting
    /// for what it causes (the report still lists what already happened).
    #[serde(default)]
    pub no_wait_after: bool,
    /// `--snap`: add `snapshot --diff -i` after the report.
    #[serde(default)]
    pub snap: bool,
    /// The output budget (0 = none); only `--snap`'s snapshot grows with
    /// the page.
    #[serde(default)]
    pub max_chars: usize,
}

#[cfg(test)]
mod tests {
    use super::Margins;

    #[test]
    fn margins_take_one_to_four_lengths_in_css_order() {
        let m = Margins::parse("1in").unwrap();
        assert_eq!((m.top, m.right, m.bottom, m.left), (1.0, 1.0, 1.0, 1.0));
        let m = Margins::parse("0, 96px").unwrap();
        assert_eq!((m.top, m.right, m.bottom, m.left), (0.0, 1.0, 0.0, 1.0));
        let m = Margins::parse("2.54cm,25.4mm,0,1in").unwrap();
        assert_eq!((m.top, m.right, m.bottom, m.left), (1.0, 1.0, 0.0, 1.0));
        assert!(
            Margins::parse("1")
                .unwrap_err()
                .contains("`1` is not a length")
        );
        assert!(Margins::parse("-1cm").is_err());
        assert!(Margins::parse("1pt").is_err());
        assert!(Margins::parse("1cm,1cm,1cm,1cm,1cm").is_err());
    }
}

/// What `extensions` does. An extension is named by its id or its name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtensionsOp {
    /// `extensions`: the unpacked extensions loaded.
    List,
    /// `extensions load <DIR>`: loads an unpacked extension (an absolute
    /// path).
    Load { dir: PathBuf },
    /// `extensions reload <ID|NAME>`: loads it again from its directory.
    Reload { extension: String },
    /// `extensions uninstall <ID|NAME>`.
    Uninstall { extension: String },
    /// `extensions popup <ID|NAME>`: runs its action on the tab (`--tab`,
    /// else the active one), which opens its popup if it has one.
    Popup { extension: String },
    /// `extensions storage <ID|NAME> [get|set|remove|clear]`: its
    /// `chrome.storage`.
    Storage {
        extension: String,
        area: ExtensionStorageArea,
        op: ExtensionStorageOp,
    },
}

/// A `chrome.storage` area (`extensions storage --area`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionStorageArea {
    #[default]
    Local,
    Sync,
    Session,
    /// Set by policy; read-only.
    Managed,
}

impl ExtensionStorageArea {
    pub fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Sync => "sync",
            Self::Session => "session",
            Self::Managed => "managed",
        }
    }
}

/// What `extensions storage` does to the area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtensionStorageOp {
    /// `get [KEY...]`: the items of `keys`, or every item without.
    Get {
        #[serde(default)]
        keys: Vec<String>,
    },
    /// `set <JSON>`: merges the object's items into the area.
    Set { values: Map<String, Value> },
    /// `remove <KEY...>`.
    Remove { keys: Vec<String> },
    /// `clear`: removes every item.
    Clear,
}

/// `extensions` and its subcommands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionsRequest {
    pub op: ExtensionsOp,
    /// `--tab`: the tab `popup` runs the action on; the active one if
    /// `None`.
    #[serde(default)]
    pub tab: Option<String>,
    pub timeout_ms: u64,
    /// The output budget (0 = none).
    #[serde(default)]
    pub max_chars: usize,
}
