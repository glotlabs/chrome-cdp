//! What the CLI asks the engine for. These are the params of the daemon's
//! RPC methods, so they are serde types.

use std::path::PathBuf;
use std::time::Duration;

use browser::{Chrome, ProfileChoice};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
    /// Bounds the launch and the page load.
    pub timeout_ms: u64,
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

/// `cdp`: one raw CDP call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CdpRequest {
    /// `Domain.method`.
    pub method: String,
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
    /// Bounds the call.
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
    /// Bounds the wait.
    pub timeout_ms: u64,
    /// The output budget (0 = none), for `--fn`'s value.
    pub max_chars: usize,
}

/// Milliseconds from a request as a duration, at least 1 ms.
pub(crate) fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms.max(1))
}
