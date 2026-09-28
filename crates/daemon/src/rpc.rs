//! The daemon's RPC methods and their params (the envelope is
//! `protocol::rpc`).
//!
//! `hello`, `status` and `close` must keep working across versions: a new
//! CLI uses them to inspect and replace an old daemon. Add fields with
//! `#[serde(default)]`; never rename or remove one.

use engine::OpenRequest;
use protocol::schema::SessionState;
use serde::{Deserialize, Serialize};

pub const HELLO: &str = "hello";
pub const OPEN: &str = "open";
pub const CONNECT: &str = "connect";
pub const CDP: &str = "cdp";
pub const STATUS: &str = "status";
pub const CLOSE: &str = "close";
pub const TABS: &str = "tabs";
pub const TAB_NEW: &str = "tab_new";
pub const TAB_SELECT: &str = "tab_select";
pub const TAB_CLOSE: &str = "tab_close";
pub const NAVIGATE: &str = "navigate";
pub const WAIT: &str = "wait";
pub const DIALOG: &str = "dialog";
pub const DOWNLOADS: &str = "downloads";
pub const SNAPSHOT: &str = "snapshot";
pub const FIND: &str = "find";
pub const READ: &str = "read";
pub const ACT: &str = "act";
pub const CLIPBOARD: &str = "clipboard";
pub const CONSOLE: &str = "console";
pub const NETWORK: &str = "network";
pub const NETWORK_SHOW: &str = "network_show";
pub const NETWORK_CLEAR: &str = "network_clear";
pub const SCREENSHOT: &str = "screenshot";
pub const PDF: &str = "pdf";
pub const EVAL: &str = "eval";
pub const HAR_START: &str = "har_start";
pub const HAR_STOP: &str = "har_stop";
pub const TRACE_START: &str = "trace_start";
pub const TRACE_STOP: &str = "trace_stop";
pub const PERF: &str = "perf";
pub const COVERAGE_START: &str = "coverage_start";
pub const COVERAGE_STOP: &str = "coverage_stop";
pub const HIGHLIGHT: &str = "highlight";
pub const INSPECT: &str = "inspect";

/// `hello`: the first call on every connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The CLI's build.
    pub build: String,
}

/// The answer to `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    /// The daemon's build. When it differs from the CLI's, the CLI closes
    /// this daemon and starts its own.
    pub build: String,
    pub session: String,
    pub pid: u32,
    /// Whether the daemon has a browser, a crashed one, or none yet.
    pub state: SessionState,
}

/// `open`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenParams {
    #[serde(flatten)]
    pub request: OpenRequest,
    /// `--idle-timeout` in ms (0 = never); `None` keeps the current one.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
}

/// `connect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectParams {
    #[serde(flatten)]
    pub request: engine::ConnectRequest,
    /// As for `open`.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
}

/// `status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusParams {
    /// With `--logs`: how many of `chrome.log`'s last lines to include.
    #[serde(default)]
    pub logs: Option<usize>,
}
