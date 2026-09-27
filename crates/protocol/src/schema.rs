//! JSON Schemas of each command's `--json` output (plan 08 §2).
//!
//! Each command's `data` is a type here (or a documented `Value`), and
//! [`COMMANDS`] lists every command with the schema of its envelope.
//! `chrome-cdp schema <command>` prints it. The schemas are stable from v1:
//! add fields, never remove or retype them.
//!
//! A new command adds its data type below and one [`CommandSchema`] entry;
//! the CLI's checklist test fails for a command that has none.

use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{ErrorInfo, Truncation};

/// One command's entry in the schema registry.
#[derive(Debug, Clone, Copy)]
pub struct CommandSchema {
    /// The command as typed after `chrome-cdp`, e.g. `tab close`.
    pub name: &'static str,
    /// What `data` holds, in a few words.
    pub summary: &'static str,
    schema: fn() -> Schema,
}

impl CommandSchema {
    /// The JSON Schema of this command's `--json` envelope, titled
    /// `chrome-cdp <name> --json`.
    pub fn schema(&self) -> Value {
        let mut schema = (self.schema)().to_value();
        if let Some(root) = schema.as_object_mut() {
            root.insert(
                "title".into(),
                Value::String(format!("chrome-cdp {} --json", self.name)),
            );
        }
        schema
    }
}

/// Every command with a `--json` form, by name.
pub const COMMANDS: &[CommandSchema] = &[
    CommandSchema {
        name: "version",
        summary: "chrome-cdp --version: version, git commit and CDP protocol revision",
        schema: || schema_for!(Envelope<VersionData>),
    },
    CommandSchema {
        name: "schema",
        summary: "a command's JSON Schema, or the list of commands",
        schema: || schema_for!(Envelope<SchemaData>),
    },
    CommandSchema {
        name: "install",
        summary: "the browser installed, or with --list the installed and available ones",
        schema: || schema_for!(Envelope<InstallData>),
    },
    CommandSchema {
        name: "uninstall",
        summary: "the browser removed",
        schema: || schema_for!(Envelope<UninstallData>),
    },
    CommandSchema {
        name: "open",
        summary: "the session and its browser, and the page opened",
        schema: || schema_for!(Envelope<OpenData>),
    },
    CommandSchema {
        name: "connect",
        summary: "the session and the browser it attached to",
        schema: || schema_for!(Envelope<OpenData>),
    },
    CommandSchema {
        name: "close",
        summary: "the sessions closed",
        schema: || schema_for!(Envelope<CloseData>),
    },
    CommandSchema {
        name: "status",
        summary: "the session's state, browser, tabs and uptime",
        schema: || schema_for!(Envelope<StatusData>),
    },
    CommandSchema {
        name: "sessions",
        summary: "every live session",
        schema: || schema_for!(Envelope<SessionsData>),
    },
    CommandSchema {
        name: "cdp",
        summary: "the raw CDP result object",
        schema: || schema_for!(Envelope<Map<String, Value>>),
    },
    CommandSchema {
        name: "tabs",
        summary: "the session's tabs, in the order they opened",
        schema: || schema_for!(Envelope<TabsData>),
    },
    CommandSchema {
        name: "tab",
        summary: "the tab made active",
        schema: || schema_for!(Envelope<TabData>),
    },
    CommandSchema {
        name: "tab new",
        summary: "the tab opened (and made active)",
        schema: || schema_for!(Envelope<TabData>),
    },
    CommandSchema {
        name: "tab close",
        summary: "the tab closed and the tab now active",
        schema: || schema_for!(Envelope<TabCloseData>),
    },
    CommandSchema {
        name: "goto",
        summary: "the page loaded: final URL, title and HTTP status",
        schema: || schema_for!(Envelope<NavigateData>),
    },
    CommandSchema {
        name: "back",
        summary: "the page gone back to, or navigated: false at the start of history",
        schema: || schema_for!(Envelope<NavigateData>),
    },
    CommandSchema {
        name: "forward",
        summary: "the page gone forward to, or navigated: false at the end of history",
        schema: || schema_for!(Envelope<NavigateData>),
    },
    CommandSchema {
        name: "reload",
        summary: "the page reloaded: URL, title and HTTP status",
        schema: || schema_for!(Envelope<NavigateData>),
    },
    CommandSchema {
        name: "wait",
        summary: "the condition that held, the page's URL and the time it took",
        schema: || schema_for!(Envelope<WaitData>),
    },
    CommandSchema {
        name: "dialog accept",
        summary: "the dialog accepted, and the text typed into a prompt",
        schema: || schema_for!(Envelope<DialogData>),
    },
    CommandSchema {
        name: "dialog dismiss",
        summary: "the dialog dismissed",
        schema: || schema_for!(Envelope<DialogData>),
    },
    CommandSchema {
        name: "downloads",
        summary: "the session's downloads, oldest first",
        schema: || schema_for!(Envelope<DownloadsData>),
    },
    CommandSchema {
        name: "snapshot",
        summary: "the page's accessibility tree, with refs on what can be acted on",
        schema: || schema_for!(Envelope<SnapshotData>),
    },
    CommandSchema {
        name: "find",
        summary: "the nodes whose role, name or text match, best first, with refs",
        schema: || schema_for!(Envelope<FindData>),
    },
    CommandSchema {
        name: "text",
        summary: "the visible text of the page or of each match",
        schema: || schema_for!(Envelope<TextData>),
    },
    CommandSchema {
        name: "html",
        summary: "the HTML of the document or of an element, pretty-printed",
        schema: || schema_for!(Envelope<HtmlData>),
    },
    CommandSchema {
        name: "attr",
        summary: "an attribute's value, or null without it",
        schema: || schema_for!(Envelope<AttrData>),
    },
    CommandSchema {
        name: "value",
        summary: "a field's current value",
        schema: || schema_for!(Envelope<ValueData>),
    },
    CommandSchema {
        name: "count",
        summary: "how many elements match, and how many are visible",
        schema: || schema_for!(Envelope<CountData>),
    },
    CommandSchema {
        name: "box",
        summary: "an element's box in the viewport, in CSS pixels",
        schema: || schema_for!(Envelope<BoxData>),
    },
    CommandSchema {
        name: "markdown",
        summary: "the page's readable content (or an element's) as Markdown",
        schema: || schema_for!(Envelope<MarkdownData>),
    },
    CommandSchema {
        name: "click",
        summary: "the element clicked, the point, button, click count and modifiers",
        schema: || schema_for!(Envelope<PointerData>),
    },
    CommandSchema {
        name: "dblclick",
        summary: "the element double-clicked and the point",
        schema: || schema_for!(Envelope<PointerData>),
    },
    CommandSchema {
        name: "hover",
        summary: "the element the mouse is over and the point",
        schema: || schema_for!(Envelope<PointerData>),
    },
    CommandSchema {
        name: "tap",
        summary: "the element tapped and the point",
        schema: || schema_for!(Envelope<PointerData>),
    },
    CommandSchema {
        name: "mouse move",
        summary: "where the mouse is now and the buttons held",
        schema: || schema_for!(Envelope<MouseData>),
    },
    CommandSchema {
        name: "mouse down",
        summary: "the button pressed, where, and the buttons held",
        schema: || schema_for!(Envelope<MouseData>),
    },
    CommandSchema {
        name: "mouse up",
        summary: "the button released, where, and the buttons held",
        schema: || schema_for!(Envelope<MouseData>),
    },
    CommandSchema {
        name: "mouse wheel",
        summary: "the wheel's deltas and where the mouse is",
        schema: || schema_for!(Envelope<MouseData>),
    },
    CommandSchema {
        name: "drag",
        summary: "the elements dragged from and to, the points, and how",
        schema: || schema_for!(Envelope<DragResult>),
    },
    CommandSchema {
        name: "scroll",
        summary: "what scrolled, its new scroll position, and which ends it reached",
        schema: || schema_for!(Envelope<ScrollData>),
    },
    CommandSchema {
        name: "press",
        summary: "the keys pressed and the element that had focus",
        schema: || schema_for!(Envelope<KeysData>),
    },
    CommandSchema {
        name: "type",
        summary: "how many characters were typed and into which element",
        schema: || schema_for!(Envelope<KeysData>),
    },
    CommandSchema {
        name: "fill",
        summary: "the field filled and the value it holds now",
        schema: || schema_for!(Envelope<FillData>),
    },
    CommandSchema {
        name: "select",
        summary: "the <select> and the options now selected",
        schema: || schema_for!(Envelope<SelectData>),
    },
    CommandSchema {
        name: "check",
        summary: "the checkbox, radio or switch, and whether it had to change",
        schema: || schema_for!(Envelope<CheckData>),
    },
    CommandSchema {
        name: "uncheck",
        summary: "the checkbox or switch, and whether it had to change",
        schema: || schema_for!(Envelope<CheckData>),
    },
    CommandSchema {
        name: "focus",
        summary: "the element that has focus now",
        schema: || schema_for!(Envelope<FocusData>),
    },
    CommandSchema {
        name: "blur",
        summary: "the element that lost focus",
        schema: || schema_for!(Envelope<FocusData>),
    },
    CommandSchema {
        name: "upload",
        summary: "the file input and the files it holds now",
        schema: || schema_for!(Envelope<UploadData>),
    },
    CommandSchema {
        name: "clipboard read",
        summary: "the text on the clipboard",
        schema: || schema_for!(Envelope<ClipboardReadData>),
    },
    CommandSchema {
        name: "clipboard write",
        summary: "how much text was put on the clipboard",
        schema: || schema_for!(Envelope<ClipboardWriteData>),
    },
];

/// The registry entry for `name` (`"tab close"`), if any.
pub fn command(name: &str) -> Option<&'static CommandSchema> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// `data` of `chrome-cdp --version --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VersionData {
    /// The chrome-cdp version, e.g. `0.1.0`.
    pub version: String,
    /// The git commit it was built from (12 hex digits), or `unknown`.
    pub git_sha: String,
    /// The Chromium revision of the vendored CDP protocol.
    pub protocol_revision: u32,
}

/// `data` of `chrome-cdp schema --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaData {
    /// `chrome-cdp schema <command>`: that command's JSON Schema.
    Schema(Map<String, Value>),
    /// `chrome-cdp schema`: the commands that have one.
    List {
        /// Every command, in registry order.
        commands: Vec<SchemaListEntry>,
    },
}

/// One line of `chrome-cdp schema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SchemaListEntry {
    /// The command, as passed to `chrome-cdp schema`.
    pub name: String,
    /// What its `data` holds.
    pub summary: String,
}

/// `data` of `chrome-cdp install --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum InstallData {
    /// `chrome-cdp install`: the browser now in the cache.
    Installed(InstalledBrowser),
    /// `chrome-cdp install --list`.
    List(InstallList),
}

/// A browser `chrome-cdp install` put in the cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstalledBrowser {
    /// `<channel>-<version>-<platform>`; `chrome-cdp uninstall` takes it.
    pub id: String,
    /// `stable`, `beta`, `dev`, `canary`, or `pinned` for a `--version` no
    /// channel is on.
    pub channel: String,
    /// E.g. `154.0.8037.57`.
    pub version: String,
    /// The Chrome for Testing platform, e.g. `linux64` or `mac-arm64`.
    pub platform: String,
    /// `chrome` or `chrome-headless-shell`.
    pub product: String,
    /// The binary to launch.
    pub path: String,
    /// Whether this run downloaded it; `false` when it was already there.
    pub downloaded: bool,
    /// The bytes downloaded (0 when nothing was).
    pub download_size: u64,
    /// System packages `--with-deps` installed.
    #[serde(default)]
    pub packages_installed: Vec<String>,
    /// The AppArmor profile `--with-deps` wrote so Chrome's sandbox works
    /// (Ubuntu 23.10+); absent when none was needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apparmor_profile: Option<String>,
}

/// `data` of `chrome-cdp install --list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallList {
    /// This machine's Chrome for Testing platform; `null` where there is no
    /// build.
    pub platform: Option<String>,
    /// Complete installs for this platform, in the order discovery prefers
    /// them.
    pub installed: Vec<InstallListEntry>,
    /// Each channel's current version. Empty when the index cannot be
    /// fetched (a warning says why).
    pub available: Vec<AvailableBrowser>,
}

/// One install in `chrome-cdp install --list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallListEntry {
    /// `<channel>-<version>-<platform>`.
    pub id: String,
    /// `stable`, `beta`, `dev`, `canary` or `pinned`.
    pub channel: String,
    /// E.g. `154.0.8037.57`.
    pub version: String,
    /// `chrome` and/or `chrome-headless-shell`.
    pub products: Vec<String>,
    /// The binary launched from this install (full Chrome if present).
    pub path: String,
}

/// A channel's current version in `chrome-cdp install --list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AvailableBrowser {
    /// `stable`, `beta`, `dev` or `canary`.
    pub channel: String,
    /// E.g. `154.0.8037.57`.
    pub version: String,
    /// The install id when this version is installed.
    pub installed: Option<String>,
}

/// `data` of `chrome-cdp uninstall --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UninstallData {
    /// The install id removed.
    pub id: String,
    /// The directory removed.
    pub dir: String,
}

/// The state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The daemon runs and has a browser.
    Running,
    /// The browser exited or the connection to it dropped; `open` starts a
    /// new one.
    Crashed,
    /// The daemon runs but has no browser yet (it is starting, or its
    /// launch failed).
    Idle,
    /// No daemon runs for this session.
    Stopped,
}

impl SessionState {
    /// The name used in text output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Crashed => "crashed",
            Self::Idle => "idle",
            Self::Stopped => "stopped",
        }
    }
}

/// How a session got its browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserKind {
    /// chrome-cdp launched it (`open`); `close` shuts it down.
    Launched,
    /// An existing browser (`connect`); `close` only detaches.
    Connected,
}

/// The browser a session drives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserInfo {
    /// Launched or connected.
    pub kind: BrowserKind,
    /// `Browser.getVersion`'s product, e.g. `HeadlessChrome/154.0.8037.57`.
    pub product: String,
    /// The binary launched (launched browsers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Why that binary was picked, e.g. `found google-chrome on PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The browser's pid, which is also its process group (launched
    /// browsers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Whether it runs without a window (launched browsers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headless: Option<bool>,
    /// The profile (launched browsers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileInfo>,
    /// Extra flags from `--chrome-arg`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chrome_args: Vec<String>,
    /// The DevTools WebSocket URL (connected browsers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// A launched browser's profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileInfo {
    /// `temporary` (deleted on close), `named` (`--profile`) or `dir`
    /// (`--user-data-dir`).
    pub kind: String,
    /// The `--profile` name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The profile directory.
    pub dir: String,
}

/// A page, after `open <url>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageInfo {
    /// The URL after navigation (redirects followed).
    pub url: String,
    /// The document title.
    pub title: String,
    /// The HTTP status of the main document; absent for pages without one
    /// (`file:`, `data:`, `about:`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

/// `data` of `chrome-cdp open --json` and `chrome-cdp connect --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenData {
    /// The session name.
    pub session: String,
    /// Whether this call started the browser (or attached to it); `false`
    /// when the session already had it.
    pub started: bool,
    /// The browser.
    pub browser: BrowserInfo,
    /// The page opened, when a URL was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<PageInfo>,
}

/// `data` of `chrome-cdp close --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CloseData {
    /// Each session asked to close.
    pub sessions: Vec<ClosedSession>,
}

/// One session in `chrome-cdp close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClosedSession {
    /// The session name.
    pub session: String,
    /// Whether it was running; closing a stopped session does nothing.
    pub was_running: bool,
    /// What happened to its browser: `closed` (launched), `detached`
    /// (connected), or `none`.
    pub browser: String,
}

/// `data` of `chrome-cdp status --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusData {
    /// The session name.
    pub session: String,
    /// Its state.
    pub state: SessionState,
    /// The browser, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<BrowserInfo>,
    /// Why the browser went away (state `crashed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crash: Option<String>,
    /// Open pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tabs: Option<u32>,
    /// Milliseconds since the browser started (or was attached).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_ms: Option<u64>,
    /// The daemon's pid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_pid: Option<u32>,
    /// Milliseconds of inactivity before the daemon closes the browser and
    /// exits; 0 means never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_ms: Option<u64>,
    /// The session directory (socket, logs, `meta.json`).
    pub dir: String,
    /// With `--logs`: the last lines of Chrome's stderr (`chrome.log`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<String>>,
}

/// `data` of `chrome-cdp sessions --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionsData {
    /// Every session with a live daemon, by name.
    pub sessions: Vec<SessionEntry>,
}

/// One line of `chrome-cdp sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionEntry {
    /// The session name.
    pub session: String,
    /// Its state.
    pub state: SessionState,
    /// `Browser.getVersion`'s product, when there is a browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// Launched or connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BrowserKind>,
    /// Open pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tabs: Option<u32>,
    /// Milliseconds since the browser started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_ms: Option<u64>,
    /// The daemon's pid.
    pub daemon_pid: u32,
}

/// One tab of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TabInfo {
    /// The tab's id, `t1`, `t2`, …: given in the order tabs open and never
    /// reused within a session. `--tab` takes it.
    pub id: String,
    /// Whether commands act on this tab when no `--tab` is given.
    pub active: bool,
    /// The document title.
    pub title: String,
    /// The current URL.
    pub url: String,
    /// The tab that opened this one (a popup), if chrome-cdp knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opener: Option<String>,
    /// Whether its page crashed; `chrome-cdp reload` brings it back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crashed: bool,
    /// A dialog that waits for `chrome-cdp dialog accept|dismiss`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<DialogInfo>,
}

/// A JavaScript dialog (`alert`, `confirm`, `prompt`, `beforeunload`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DialogInfo {
    /// `alert`, `confirm`, `prompt` or `beforeunload`.
    pub kind: String,
    /// The text it shows (empty for `beforeunload`).
    pub message: String,
    /// A prompt's default answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_prompt: Option<String>,
    /// The URL of the frame that opened it.
    pub url: String,
}

/// `data` of `chrome-cdp dialog accept|dismiss --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DialogData {
    /// The tab whose dialog it was, e.g. `t1`.
    pub tab: String,
    /// The dialog.
    pub dialog: DialogInfo,
    /// Whether it was accepted (else dismissed).
    pub accepted: bool,
    /// The text typed into a prompt before accepting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_text: Option<String>,
}

/// A file the browser downloaded (or is downloading) into the session's
/// `downloads` directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DownloadInfo {
    /// The tab whose page started it, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// The URL downloaded.
    pub url: String,
    /// The file's name, as the server suggested it (made unique in the
    /// directory).
    pub file: String,
    /// The absolute path of the file, once it completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `in_progress`, `completed` or `canceled`.
    pub state: String,
    /// Bytes received so far.
    pub received_bytes: u64,
    /// The size the server announced, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
}

/// `data` of `chrome-cdp downloads --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DownloadsData {
    /// The session's downloads directory.
    pub dir: String,
    /// Every download of the session, oldest first.
    pub downloads: Vec<DownloadInfo>,
}

/// `data` of `chrome-cdp snapshot --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotData {
    /// The tab, e.g. `t1`.
    pub tab: String,
    /// The main frame's URL.
    pub url: String,
    /// The document title.
    pub title: String,
    /// The engine that read the page: `ax` (Chrome's accessibility tree)
    /// or `dom` (chrome-cdp's own DOM walker).
    pub engine: String,
    /// `--diff`: whether an earlier snapshot of the tab was compared.
    /// Without one, `nodes` is the whole page, all marked `added`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<bool>,
    /// `--diff`: the main frame loaded a new document since the earlier
    /// snapshot, so its nodes were compared by place, not by identity.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_page: bool,
    /// The top-level nodes. With `--diff`, only the changed nodes and
    /// their ancestors (each with `change`).
    pub nodes: Vec<SnapshotNode>,
}

/// One node of a snapshot, as one text line shows it:
/// `role "name" [ref] (hN) value="…" states… → href`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotNode {
    /// The ARIA role (`button`, `link`, `heading`, …), `text` for text,
    /// `clickable` for an element that only looks and acts like a button,
    /// or `iframe`. With `more`, the role of the nodes left out.
    pub role: String,
    /// The accessible name, or the text of a `text` node (or of a node
    /// whose only content is that text).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The ref commands take (`e12`); only on nodes an agent can act on,
    /// unless `--all-refs`.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// A heading's level (1-6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u32>,
    /// The value of a text box, combobox, slider or spin button. Never
    /// set for password fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// A password field (its value is never shown).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub password: bool,
    /// A file input (`upload` sets it; a click opens a file chooser).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub file: bool,
    /// States that carry information, in this order: `checked`,
    /// `unchecked`, `mixed`, `expanded`, `collapsed`, `selected`,
    /// `disabled`, `readonly`, `required`, `invalid`, `focused`, `pressed`,
    /// `scrollable`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    /// A link's target; relative (`/pricing`) when on the frame's origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// How many more nodes of `role` the list has past this point (they
    /// are left out; `--all` shows them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<u32>,
    /// How many nodes below this one `--depth` left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<u32>,
    /// `--diff`: `added`, `removed` or `changed`; absent on the unchanged
    /// ancestors shown for context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    /// `--diff`, `changed`: the parts of the line that changed, as they
    /// were (`unchecked`, `"Old name"`, `value="old"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SnapshotNode>,
}

/// `data` of `chrome-cdp find --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FindData {
    /// The tab searched, e.g. `t1`.
    pub tab: String,
    /// The main frame's URL.
    pub url: String,
    /// The document title.
    pub title: String,
    /// The words searched for.
    pub query: String,
    /// `--role`: the only role searched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// `--scope`: the locator of the only part of the page searched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// How many nodes matched; `matches` holds the best `--limit` of them.
    pub count: u64,
    /// The matches, best first (then in page order), as snapshot nodes
    /// without children.
    pub matches: Vec<FindMatch>,
}

/// One match of `find`: its snapshot node (with a ref unless it is
/// text) and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FindMatch {
    #[serde(flatten)]
    pub node: SnapshotNode,
    /// The nearest named container around it: `form "Login"`,
    /// `row "Invoice 12 $120"`, `iframe "Payment"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// `data` of `chrome-cdp text --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextData {
    /// The tab read, e.g. `t1`.
    pub tab: String,
    /// The locator; absent for the whole page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    /// The visible text of each match, in page order, whitespace tidied:
    /// runs of spaces collapsed, lines trimmed, at most one empty line in
    /// a row. A hidden match has empty text.
    pub texts: Vec<String>,
}

/// `data` of `chrome-cdp html --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HtmlData {
    /// The tab read, e.g. `t1`.
    pub tab: String,
    /// The locator; absent for the whole document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    /// Whether the element itself is included (`--outer`, and always for
    /// the whole document).
    pub outer: bool,
    /// Whether this is the browser's own serialisation (`--raw`) rather
    /// than the pretty-printed form.
    pub raw: bool,
    /// The HTML.
    pub html: String,
}

/// `data` of `chrome-cdp attr --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttrData {
    /// The tab read, e.g. `t1`.
    pub tab: String,
    pub locator: String,
    /// The attribute's name.
    pub name: String,
    /// Its value; `null` when the element has no such attribute.
    pub value: Option<String>,
}

/// `data` of `chrome-cdp value --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ValueData {
    /// The tab read, e.g. `t1`.
    pub tab: String,
    pub locator: String,
    /// What was read: `input`, `password`, `textarea`, `select`, `file`,
    /// `output`, `progress`, `meter`, `contenteditable` (its text) or
    /// `aria` (`aria-valuetext`, else `aria-valuenow`).
    pub kind: String,
    /// The value; for a multiple select or a file input, the values one
    /// per line.
    pub value: String,
    /// A multiple select's selected values or a file input's file names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
}

/// `data` of `chrome-cdp count --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CountData {
    /// The tab read, e.g. `t1`.
    pub tab: String,
    pub locator: String,
    /// How many elements match.
    pub count: u64,
    /// How many of them are visible.
    pub visible: u64,
}

/// `data` of `chrome-cdp box --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BoxData {
    /// The tab read, e.g. `t1`.
    pub tab: String,
    pub locator: String,
    /// The element's border box in CSS pixels, relative to the top-left
    /// corner of the tab's viewport (iframes' offsets added); `null` when
    /// it is not rendered (`display: none`, or not in the layout).
    #[serde(rename = "box")]
    pub rect: Option<Rect>,
    /// Whether any of the box is inside the viewport (scrolled into view).
    pub in_viewport: bool,
}

/// A rectangle in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// `data` of `chrome-cdp markdown --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkdownData {
    /// The tab read, e.g. `t1`.
    pub tab: String,
    /// The main frame's URL.
    pub url: String,
    /// The document title.
    pub title: String,
    /// The locator; absent when the page's main content was picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    /// The element converted: `main`, `article`, `div#content`, `body`,
    /// … (tag, then `#id` or the first class).
    pub source: String,
    /// The content as Markdown. Links on the page's origin are relative.
    pub markdown: String,
}

/// An element an action acted on, as the output names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActedElement {
    /// Its ref, e.g. `e6` (absent if the page changed before one could
    /// be given).
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Its role (`generic` for none).
    pub role: String,
    /// Its accessible name (or text), cut to 60 characters.
    pub name: String,
    /// Its tag name, lower case.
    pub tag: String,
}

/// A point in the tab's viewport, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// `data` of `chrome-cdp click`, `dblclick`, `hover` and `tap --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PointerData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// `click`, `dblclick`, `hover` or `tap`.
    pub action: String,
    /// The locator as given.
    pub locator: String,
    pub element: ActedElement,
    /// Where the events went, in the tab's viewport.
    pub point: Point,
    /// The button (clicks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
    /// How many clicks (clicks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// The modifier keys held: `Alt`, `Control`, `Meta`, `Shift`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<String>,
}

/// `data` of `chrome-cdp mouse move|down|up|wheel --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MouseData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// `move`, `down`, `up` or `wheel`.
    pub action: String,
    /// Where the mouse is now, in the tab's viewport.
    pub point: Point,
    /// The button pressed or released (`down`, `up`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
    /// The buttons held now: left 1, right 2, middle 4 (added).
    pub buttons: u32,
    /// The wheel's deltas (`wheel`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<Point>,
}

/// `data` of `chrome-cdp drag --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DragResult {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    pub from: ActedElement,
    pub to: ActedElement,
    /// Where the drag started and ended, in the tab's viewport.
    pub from_point: Point,
    pub to_point: Point,
    /// `pointer` (mouse events only) or `html5` (the page started an
    /// HTML5 drag, which was carried out with drag events).
    pub method: String,
}

/// `data` of `chrome-cdp scroll --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScrollData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// The element the locator named, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<ActedElement>,
    /// The scroll container reported on; absent for the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<ActedElement>,
    /// Its scroll offset now, in CSS pixels.
    pub x: f64,
    pub y: f64,
    /// The largest scroll offsets it has now.
    pub max_x: f64,
    pub max_y: f64,
    /// Which ends it is at.
    pub at_top: bool,
    pub at_bottom: bool,
    pub at_left: bool,
    pub at_right: bool,
}

/// `data` of `chrome-cdp press` and `type --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeysData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// `press` or `type`.
    pub action: String,
    /// The chords pressed, `Mod` resolved (`Control+A`, `Backspace`)
    /// (press).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// How many characters were typed (type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<usize>,
    /// The element that had focus when the keys went (after `--in`
    /// focused it); absent when nothing had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<ActedElement>,
}

/// `data` of `chrome-cdp fill --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FillData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// The locator as given.
    pub locator: String,
    pub element: ActedElement,
    /// What the field holds now (absent for passwords).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// An option of a `<select>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SelectOption {
    pub value: String,
    /// Its text, whitespace collapsed.
    pub label: String,
}

/// `data` of `chrome-cdp select --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SelectData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// The locator as given.
    pub locator: String,
    pub element: ActedElement,
    /// The options selected now, in the order asked for.
    pub selected: Vec<SelectOption>,
}

/// `data` of `chrome-cdp check` and `uncheck --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// The locator as given.
    pub locator: String,
    /// The checkbox, radio button or switch (the control a label is
    /// for).
    pub element: ActedElement,
    /// Whether it is checked now.
    pub checked: bool,
    /// Whether it was clicked: false when it already was as asked.
    pub changed: bool,
    /// The label clicked instead of a visually hidden control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<ActedElement>,
}

/// `data` of `chrome-cdp focus` and `blur --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FocusData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// `focus` or `blur`.
    pub action: String,
    /// The locator as given.
    pub locator: String,
    pub element: ActedElement,
}

/// A file `upload` gave a file input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadedFile {
    /// Its absolute path.
    pub path: String,
    /// Its name, as the page sees it.
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
}

/// `data` of `chrome-cdp upload --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadData {
    /// The tab acted in, e.g. `t1`.
    pub tab: String,
    /// The locator as given.
    pub locator: String,
    /// The file input, or what was clicked to open a file chooser.
    pub element: ActedElement,
    /// The files the input holds now (none after `--clear`).
    pub files: Vec<UploadedFile>,
    /// Whether the files went through a file chooser that clicking
    /// `element` opened (else straight into the input).
    pub chooser: bool,
}

/// `data` of `chrome-cdp clipboard read --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardReadData {
    /// The tab whose page read it, e.g. `t1`.
    pub tab: String,
    /// The clipboard's text (empty when it holds none).
    pub text: String,
}

/// `data` of `chrome-cdp clipboard write --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardWriteData {
    /// The tab whose page wrote it, e.g. `t1`.
    pub tab: String,
    /// How many characters were written.
    pub chars: usize,
}

/// `data` of `chrome-cdp tabs --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TabsData {
    /// Every open tab, by id.
    pub tabs: Vec<TabInfo>,
}

/// `data` of `chrome-cdp tab <id> --json` and `chrome-cdp tab new --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TabData {
    /// The tab, now active.
    pub tab: TabInfo,
}

/// `data` of `chrome-cdp tab close --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TabCloseData {
    /// The id of the tab closed.
    pub closed: String,
    /// The tab active now; absent when the session closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<TabInfo>,
    /// Whether a blank tab was opened because the last one closed (so the
    /// session keeps its browser).
    #[serde(default)]
    pub opened_blank: bool,
    /// Whether the session and its browser closed (`--close-browser` on the
    /// last tab).
    #[serde(default)]
    pub session_closed: bool,
}

/// `data` of `chrome-cdp goto`, `back`, `forward` and `reload --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NavigateData {
    /// The tab that navigated, e.g. `t1`.
    pub tab: String,
    /// `goto`: the URL asked for, with its scheme added or resolved against
    /// `base_url` or the current page; before redirects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_url: Option<String>,
    /// The page's URL now (redirects followed).
    pub url: String,
    /// The document title.
    pub title: String,
    /// The HTTP status of the main document. A 4xx or 5xx is a warning, not
    /// an error. Absent for pages without one (`file:`, `data:`, `about:`)
    /// and for same-document navigations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// `false` when `back` or `forward` found no history entry to go to, so
    /// nothing happened.
    pub navigated: bool,
    /// Whether the document stayed: a `#fragment` or a history entry made
    /// by `pushState`.
    pub same_document: bool,
    /// Whether the page came back from the back/forward cache (`back`,
    /// `forward`), with its state as it was.
    pub restored: bool,
    /// What the command waited for: `settle` (the load event, then the
    /// page settled), `load`, `domcontentloaded`, `networkidle`, `commit`
    /// or `none`; `download` when `goto`'s URL was a download.
    pub wait: String,
    /// Milliseconds from the start of the navigation until that.
    pub elapsed_ms: u64,
    /// `goto` to a URL the browser downloads instead of showing: the file
    /// (the page stays where it was, and `navigated` is `false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<DownloadInfo>,
}

/// `data` of `chrome-cdp wait --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WaitData {
    /// The tab watched, e.g. `t1`.
    pub tab: String,
    /// What was waited for: `load`, `domcontentloaded`, `idle`, `settle`,
    /// `element`, `text`, `text_gone`, `url`, `fn`, `request`, `response`
    /// or `delay`.
    pub condition: String,
    /// What held, in words: `text "Welcome" is visible`.
    pub met: String,
    /// The main frame's URL when it held.
    pub url: String,
    /// Milliseconds the wait took.
    pub elapsed_ms: u64,
    /// `wait <locator>`: how many elements matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// `--fn`: the expression's value (as JSON; other values as a string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// `--request` and `--response`: the one that matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestInfo>,
    /// `--download`: the file downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<DownloadInfo>,
}

/// A request or response a wait matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestInfo {
    /// `GET`, `POST`, … (empty if not known).
    pub method: String,
    pub url: String,
    /// The HTTP status, for a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

/// Something that happened in the browser while a command ran. The
/// envelope lists them in `events`; the command's text says what matters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A tab opened that no command asked for: a popup (`window.open`,
    /// `target=_blank`) or one the user opened. It is not active unless the
    /// session follows popups (`open --follow-popups`).
    TabOpened {
        /// The new tab's id.
        tab: String,
        /// Its URL when the command ended (it may still be loading).
        url: String,
        /// The tab that opened it, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opener: Option<String>,
        /// Whether it became the active tab.
        active: bool,
    },
    /// A JavaScript dialog closed without a `chrome-cdp dialog` command:
    /// the session's `--dialogs` policy answered it (an `alert` or
    /// `beforeunload` by default), or a navigation dismissed it.
    Dialog {
        /// The tab it opened in.
        tab: String,
        /// `alert`, `confirm`, `prompt` or `beforeunload`.
        kind: String,
        /// The text it showed.
        message: String,
        /// Whether it was accepted (else dismissed).
        accepted: bool,
        /// `policy` (the session's `--dialogs` answered it) or `other`
        /// (it closed without `chrome-cdp dialog`: a navigation dismissed
        /// it, or a raw CDP call answered it).
        by: String,
    },
    /// A download started, or finished, while the command ran.
    Download(DownloadInfo),
}

/// The `--json` envelope (see [`crate::Envelope`]) with `data: T`.
#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)] // Only its schema is used.
enum Envelope<T> {
    /// The command succeeded.
    Ok(OkEnvelope<T>),
    /// The command failed.
    Err(ErrEnvelope),
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct OkEnvelope<T> {
    /// Always `true`.
    #[schemars(extend("const" = true))]
    ok: bool,
    /// The command's result.
    data: T,
    /// Non-fatal notes.
    #[serde(default)]
    warnings: Vec<String>,
    /// Page events observed while the command ran (tabs opened, dialogs,
    /// navigations, …).
    #[serde(default)]
    events: Vec<Event>,
    /// Values in `data` shortened to keep the output bounded; `--max-chars`
    /// (or the command's own flag, named in the warning) shows more.
    #[serde(default)]
    truncated: Vec<Truncation>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct ErrEnvelope {
    /// Always `false`.
    #[schemars(extend("const" = false))]
    ok: bool,
    /// What went wrong and what to run next.
    error: ErrorInfo,
    /// Non-fatal notes.
    #[serde(default)]
    warnings: Vec<String>,
    /// Page events observed while the command ran.
    #[serde(default)]
    events: Vec<Event>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{COMMANDS, VersionData, command};
    use crate::{Envelope, ErrorInfo, ErrorKind, Output};

    #[test]
    fn names_are_unique() {
        for (i, c) in COMMANDS.iter().enumerate() {
            assert!(
                COMMANDS[i + 1..].iter().all(|d| d.name != c.name),
                "{}",
                c.name
            );
        }
    }

    #[test]
    fn envelope_schema_describes_both_outcomes() {
        let schema = command("version").unwrap().schema();
        assert_eq!(schema["title"], json!("chrome-cdp version --json"));
        let text = schema.to_string();
        for needle in [
            r#""const":true"#,
            r#""const":false"#,
            r#""protocol_revision""#,
            r#""DialogOpen""#,
            r#""truncated""#,
        ] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
        assert!(command("nope").is_none());
    }

    /// A tiny structural check: every `required` property of the matching
    /// branch is present. Enough to catch a renamed field.
    #[test]
    fn real_envelopes_have_the_required_fields() {
        let schema = command("version").unwrap().schema();
        let data = VersionData {
            version: "0.1.0".into(),
            git_sha: "abc".into(),
            protocol_revision: 1,
        };
        let ok = Envelope::from_output(Output::new(serde_json::to_value(&data).unwrap(), ""));
        let ok = serde_json::to_value(ok).unwrap();
        let err =
            serde_json::to_value(Envelope::err(ErrorInfo::new(ErrorKind::Timeout, "x"))).unwrap();
        let defs = &schema["$defs"];
        for (value, def) in [(&ok, "OkEnvelope"), (&err, "ErrEnvelope")] {
            let (_, branch) = defs
                .as_object()
                .unwrap()
                .iter()
                .find(|(k, _)| k.starts_with(def))
                .unwrap();
            for field in branch["required"].as_array().unwrap() {
                let field = field.as_str().unwrap();
                assert!(value.get(field).is_some(), "{def} needs {field}: {value}");
            }
        }
        for field in ["version", "git_sha", "protocol_revision"] {
            assert!(ok["data"].get(field).is_some());
        }
    }
}
