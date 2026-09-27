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
