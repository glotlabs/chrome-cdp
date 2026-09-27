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
    /// `--tab`: load `url` in this tab instead.
    #[serde(default)]
    pub tab: Option<String>,
    /// `--follow-popups`: whether popups become the active tab; `None`
    /// keeps the session's setting (off at first).
    #[serde(default)]
    pub follow_popups: Option<bool>,
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

/// Milliseconds from a request as a duration, at least 1 ms.
pub(crate) fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms.max(1))
}
