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
pub const FRAMES: &str = "frames";
pub const CONTEXT_NEW: &str = "context_new";
pub const CONTEXT_LIST: &str = "context_list";
pub const CONTEXT_CLOSE: &str = "context_close";
pub const TAB_NEW: &str = "tab_new";
pub const TAB_SELECT: &str = "tab_select";
pub const TAB_CLOSE: &str = "tab_close";
pub const NAVIGATE: &str = "navigate";
pub const WAIT: &str = "wait";
pub const EXPECT: &str = "expect";
pub const MARK: &str = "mark";
pub const EXPECT_SNAPSHOT: &str = "expect_snapshot";
pub const EXPECT_SCREENSHOT: &str = "expect_screenshot";
pub const DIALOG: &str = "dialog";
pub const DOWNLOADS: &str = "downloads";
pub const SNAPSHOT: &str = "snapshot";
pub const FIND: &str = "find";
pub const LOCATOR: &str = "locator";
pub const READ: &str = "read";
pub const ACT: &str = "act";
pub const CLIPBOARD: &str = "clipboard";
pub const CONSOLE: &str = "console";
pub const ISSUES: &str = "issues";
pub const NETWORK: &str = "network";
pub const NETWORK_SHOW: &str = "network_show";
pub const NETWORK_FRAMES: &str = "network_frames";
pub const NETWORK_CLEAR: &str = "network_clear";
pub const ROUTE: &str = "route";
pub const ROUTES: &str = "routes";
pub const UNROUTE: &str = "unroute";
pub const MOCK_USE: &str = "mock_use";
pub const MOCK_OFF: &str = "mock_off";
pub const MOCK_LIST: &str = "mock_list";
pub const MOCK_LOG: &str = "mock_log";
pub const MOCK_RESET: &str = "mock_reset";
pub const MOCK_EXPLAIN: &str = "mock_explain";
pub const MOCK_STATE: &str = "mock_state";
pub const MOCK_SEED: &str = "mock_seed";
pub const MOCK_WS_SEND: &str = "mock_ws_send";
pub const HTTP: &str = "http";
pub const INIT_SCRIPT_ADD: &str = "init_script_add";
pub const INIT_SCRIPT_LIST: &str = "init_script_list";
pub const INIT_SCRIPT_REMOVE: &str = "init_script_remove";
pub const EXPOSE: &str = "expose";
pub const EMULATE: &str = "emulate";
pub const PERMISSIONS: &str = "permissions";
pub const EXTENSIONS: &str = "extensions";
pub const COOKIES: &str = "cookies";
pub const STORAGE: &str = "storage";
pub const STATE_SAVE: &str = "state_save";
pub const STATE_LOAD: &str = "state_load";
pub const SCREENSHOT: &str = "screenshot";
pub const PDF: &str = "pdf";
pub const EVAL: &str = "eval";
pub const HAR_START: &str = "har_start";
pub const HAR_STOP: &str = "har_stop";
pub const HAR_EXPORT: &str = "har_export";
pub const REPRO_START: &str = "repro_start";
pub const REPRO_INFO: &str = "repro_info";
pub const RECORD_START: &str = "record_start";
pub const RECORD_STATUS: &str = "record_status";
pub const RECORD_STOP: &str = "record_stop";
/// `test`'s heal (plan 18.2): the elements that best match a fingerprint.
pub const HEAL_MATCH: &str = "heal_match";
pub const HISTORY: &str = "history";
pub const TRACE_START: &str = "trace_start";
pub const TRACE_STOP: &str = "trace_stop";
pub const PERF: &str = "perf";
pub const COVERAGE_START: &str = "coverage_start";
pub const COVERAGE_STOP: &str = "coverage_stop";
pub const DEBUG_LOG: &str = "debug_log";
pub const DEBUG_LIST: &str = "debug_list";
pub const DEBUG_CLEAR: &str = "debug_clear";
pub const DEBUG_LOCALS: &str = "debug_locals";
pub const HIGHLIGHT: &str = "highlight";
pub const SOURCE: &str = "source";
pub const WHY: &str = "why";
pub const FORMS: &str = "forms";
pub const FILL_FORM: &str = "fill_form";
pub const LAYOUT: &str = "layout";
pub const AUDIT: &str = "audit";
pub const COMPARE: &str = "compare";
pub const COMPARE_CAPTURE: &str = "compare_capture";
pub const COMPARE_NAME: &str = "compare_name";
pub const EXPLORE: &str = "explore";
pub const COMPONENTS: &str = "components";
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
