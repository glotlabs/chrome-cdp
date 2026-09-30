//! JSON Schemas of each command's `--json` output (plan 08 §2).
//!
//! Each command's `data` is a type here (or a documented `Value`), and
//! [`COMMANDS`] lists every command with the schema of its envelope.
//! `chrome-cdp schema <command>` prints it. The schemas are stable from v1:
//! add fields, never remove or retype them.
//!
//! A new command adds its data type below and one [`CommandSchema`] entry;
//! the CLI's checklist test fails for a command that has none.

use std::collections::BTreeMap;

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
        name: "help",
        summary: "the help text asked for: the overview, a topic or a command",
        schema: || schema_for!(Envelope<HelpData>),
    },
    CommandSchema {
        name: "skill",
        summary: "the skill file's text, or where --install wrote it",
        schema: || schema_for!(Envelope<SkillData>),
    },
    CommandSchema {
        name: "init e2e",
        summary: "the project's E2E layout: the framework found and each file written or kept",
        schema: || schema_for!(Envelope<InitData>),
    },
    CommandSchema {
        name: "completions",
        summary: "the shell completion script",
        schema: || schema_for!(Envelope<CompletionsData>),
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
        name: "doctor",
        summary: "each check of the setup (Chrome, libraries, sandbox, session dir, quarantine, versions) with its fix",
        schema: || schema_for!(Envelope<DoctorData>),
    },
    CommandSchema {
        name: "upgrade",
        summary: "the release looked at, whether it is newer, and whether this binary was replaced",
        schema: || schema_for!(Envelope<UpgradeData>),
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
        summary: "the raw CDP result object; with --listen, the result and the events captured",
        schema: || schema_for!(Envelope<CdpData>),
    },
    CommandSchema {
        name: "tabs",
        summary: "the session's tabs, in the order they opened",
        schema: || schema_for!(Envelope<TabsData>),
    },
    CommandSchema {
        name: "frames",
        summary: "the tab's frames, out-of-process ones too, and its workers",
        schema: || schema_for!(Envelope<FramesData>),
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
        name: "context new",
        summary: "the browser context created and its first tab",
        schema: || schema_for!(Envelope<ContextNewData>),
    },
    CommandSchema {
        name: "context list",
        summary: "the browser contexts made with context new, with their tabs",
        schema: || schema_for!(Envelope<ContextsData>),
    },
    CommandSchema {
        name: "context close",
        summary: "the browser context closed and the tabs that closed with it",
        schema: || schema_for!(Envelope<ContextCloseData>),
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
        name: "expect",
        summary: "the assertion that held: expected, actual, and how long it took",
        schema: || schema_for!(Envelope<ExpectData>),
    },
    CommandSchema {
        name: "expect-snapshot",
        summary: "the snapshot that matched (or was stored as) the baseline",
        schema: || schema_for!(Envelope<ExpectSnapshotData>),
    },
    CommandSchema {
        name: "expect-screenshot",
        summary: "the screenshot that matched (or was stored as) the baseline",
        schema: || schema_for!(Envelope<ExpectScreenshotData>),
    },
    CommandSchema {
        name: "mark",
        summary: "the mark set in the tab's console and network logs",
        schema: || schema_for!(Envelope<MarkData>),
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
        name: "console",
        summary: "the tab's console entries the agent has not seen yet (or --since), oldest first",
        schema: || schema_for!(Envelope<ConsoleData>),
    },
    CommandSchema {
        name: "errors",
        summary: "the tab's unseen errors: uncaught exceptions, console.error and failed requests to its origin",
        schema: || schema_for!(Envelope<ConsoleData>),
    },
    CommandSchema {
        name: "network",
        summary: "the tab's requests the agent has not seen yet (or --since), oldest first",
        schema: || schema_for!(Envelope<NetworkData>),
    },
    CommandSchema {
        name: "network show",
        summary: "one request in full: timing, headers and bodies on request",
        schema: || schema_for!(Envelope<NetworkShowData>),
    },
    CommandSchema {
        name: "network clear",
        summary: "how many requests were cleared from the tab's network log",
        schema: || schema_for!(Envelope<NetworkClearData>),
    },
    CommandSchema {
        name: "screenshot",
        summary: "the image file saved: the viewport, the full page or an element",
        schema: || schema_for!(Envelope<ScreenshotData>),
    },
    CommandSchema {
        name: "eval",
        summary: "the script's value as JSON, and its type",
        schema: || schema_for!(Envelope<EvalData>),
    },
    CommandSchema {
        name: "pdf",
        summary: "the PDF file the page was printed to",
        schema: || schema_for!(Envelope<PdfData>),
    },
    CommandSchema {
        name: "har start",
        summary: "which tabs a HAR recording covers, and whether bodies go in",
        schema: || schema_for!(Envelope<HarStartData>),
    },
    CommandSchema {
        name: "har stop",
        summary: "the HAR 1.2 file saved from the network log",
        schema: || schema_for!(Envelope<HarStopData>),
    },
    CommandSchema {
        name: "route",
        summary: "the routing rules added, and how many are active",
        schema: || schema_for!(Envelope<RouteData>),
    },
    CommandSchema {
        name: "block",
        summary: "the blocking rules added, and how many are active",
        schema: || schema_for!(Envelope<RouteData>),
    },
    CommandSchema {
        name: "routes",
        summary: "the routing rules with their hit counts, or with --log the requests they routed",
        schema: || schema_for!(Envelope<RoutesData>),
    },
    CommandSchema {
        name: "unroute",
        summary: "the routing rules removed, and how many are left",
        schema: || schema_for!(Envelope<UnrouteData>),
    },
    CommandSchema {
        name: "mock use",
        summary: "the mock sets in use after applying some, in the order they are tried",
        schema: || schema_for!(Envelope<MockSetsData>),
    },
    CommandSchema {
        name: "mock off",
        summary: "the mock sets turned off, and the ones still in use",
        schema: || schema_for!(Envelope<MockOffData>),
    },
    CommandSchema {
        name: "mock list",
        summary: "the mock sets in use with their mocks and hits, or with --available the sets found",
        schema: || schema_for!(Envelope<MockSetsData>),
    },
    CommandSchema {
        name: "mock log",
        summary: "the requests the mocks answered or left unmatched, with the mock that answered",
        schema: || schema_for!(Envelope<MockLogData>),
    },
    CommandSchema {
        name: "mock reset",
        summary: "the mock sets whose state (counters, sequences, named values, collections, random values) was reset",
        schema: || schema_for!(Envelope<MockSetsData>),
    },
    CommandSchema {
        name: "mock state",
        summary: "the mocks' named values and collections, or one of them",
        schema: || schema_for!(Envelope<MockStateData>),
    },
    CommandSchema {
        name: "mock seed",
        summary: "the collection seeded, and where its items came from",
        schema: || schema_for!(Envelope<MockSeedData>),
    },
    CommandSchema {
        name: "mock explain",
        summary: "which mock would answer a request, and why each other one would not",
        schema: || schema_for!(Envelope<MockExplainData>),
    },
    CommandSchema {
        name: "mock serve",
        summary: "the mock server's address, its sets and how many requests it answered (a JSON line per request before it)",
        schema: || schema_for!(Envelope<MockServeData>),
    },
    CommandSchema {
        name: "mock ws send",
        summary: "the page's mocked WebSockets that got a message",
        schema: || schema_for!(Envelope<MockWsSendData>),
    },
    CommandSchema {
        name: "mock check",
        summary: "the problems found in the mock files: file:line:col, severity, rule and message",
        schema: || schema_for!(Envelope<MockCheckData>),
    },
    CommandSchema {
        name: "http",
        summary: "the answer to a request sent from the browser context: status, headers, body, and whether a mock, a route or the network answered",
        schema: || schema_for!(Envelope<HttpData>),
    },
    CommandSchema {
        name: "mock record",
        summary: "the mock file written from the session's requests, and its data files",
        schema: || schema_for!(Envelope<MockRecordData>),
    },
    CommandSchema {
        name: "mock import",
        summary: "the mock file written from a HAR file, and its data files",
        schema: || schema_for!(Envelope<MockRecordData>),
    },
    CommandSchema {
        name: "init-script add",
        summary: "the init script added, and how many the session has",
        schema: || schema_for!(Envelope<InitScriptAddData>),
    },
    CommandSchema {
        name: "init-script list",
        summary: "the session's init scripts, in the order they run",
        schema: || schema_for!(Envelope<InitScriptsData>),
    },
    CommandSchema {
        name: "init-script remove",
        summary: "the init scripts removed, and how many are left",
        schema: || schema_for!(Envelope<InitScriptRemoveData>),
    },
    CommandSchema {
        name: "expose",
        summary: "the functions exposed to pages, after adding some",
        schema: || schema_for!(Envelope<ExposeData>),
    },
    CommandSchema {
        name: "unexpose",
        summary: "the functions exposed to pages, after removing some",
        schema: || schema_for!(Envelope<ExposeData>),
    },
    CommandSchema {
        name: "emulate",
        summary: "what the session emulates, after a device or reset; with --list the devices",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "viewport",
        summary: "what the session emulates, after setting the viewport",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "geo",
        summary: "what the session emulates, after setting the geolocation",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "tz",
        summary: "what the session emulates, after setting the time zone",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "locale",
        summary: "what the session emulates, after setting the locale",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "ua",
        summary: "what the session emulates, after setting the user agent",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "media",
        summary: "what the session emulates, after setting CSS media features",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "offline",
        summary: "what the session emulates, after turning the network off or on",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "throttle",
        summary: "what the session emulates, after setting network throttling",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "cpu-throttle",
        summary: "what the session emulates, after setting CPU throttling",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "time freeze",
        summary: "what the session emulates, after freezing the time pages see",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "time advance",
        summary: "what the session emulates, after moving the frozen time on",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "time pause",
        summary: "what the session emulates, after freezing the time and stopping timers",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "time run-for",
        summary: "what the session emulates, after running the paused timers for a while",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "time pause-at",
        summary: "what the session emulates, after running the paused timers up to a time",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "time resume",
        summary: "what the session emulates, after letting paused timers run in real time",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "time reset",
        summary: "what the session emulates, after giving pages the real time back",
        schema: || schema_for!(Envelope<EmulateData>),
    },
    CommandSchema {
        name: "permissions",
        summary: "the permissions the session granted or denied",
        schema: || schema_for!(Envelope<PermissionsData>),
    },
    CommandSchema {
        name: "permissions grant",
        summary: "the permissions the session set, after granting some",
        schema: || schema_for!(Envelope<PermissionsData>),
    },
    CommandSchema {
        name: "permissions deny",
        summary: "the permissions the session set, after denying some",
        schema: || schema_for!(Envelope<PermissionsData>),
    },
    CommandSchema {
        name: "permissions reset",
        summary: "the permissions the session set, after resetting some or all",
        schema: || schema_for!(Envelope<PermissionsData>),
    },
    CommandSchema {
        name: "cookies",
        summary: "the browser's cookies, or those --url or --domain would get",
        schema: || schema_for!(Envelope<CookiesData>),
    },
    CommandSchema {
        name: "cookies set",
        summary: "the cookie set, as the browser stored it",
        schema: || schema_for!(Envelope<CookieSetData>),
    },
    CommandSchema {
        name: "cookies clear",
        summary: "the cookies removed",
        schema: || schema_for!(Envelope<CookiesClearData>),
    },
    CommandSchema {
        name: "storage",
        summary: "a frame's localStorage or sessionStorage: its items, or the item read or changed",
        schema: || schema_for!(Envelope<StorageData>),
    },
    CommandSchema {
        name: "state save",
        summary: "the storage state file written: its cookies and origins",
        schema: || schema_for!(Envelope<StateData>),
    },
    CommandSchema {
        name: "state load",
        summary: "the storage state loaded: its cookies and origins, and how each origin got its storage",
        schema: || schema_for!(Envelope<StateData>),
    },
    CommandSchema {
        name: "trace start",
        summary: "the tab whose performance trace is recording, and its categories",
        schema: || schema_for!(Envelope<TraceStartData>),
    },
    CommandSchema {
        name: "trace stop",
        summary: "the Chrome performance trace file saved",
        schema: || schema_for!(Envelope<TraceStopData>),
    },
    CommandSchema {
        name: "perf",
        summary: "the page's load timing, Web Vitals (LCP, CLS, INP), blocking time and memory",
        schema: || schema_for!(Envelope<PerfData>),
    },
    CommandSchema {
        name: "coverage start",
        summary: "the tab whose JavaScript and CSS coverage is measured",
        schema: || schema_for!(Envelope<CoverageStartData>),
    },
    CommandSchema {
        name: "coverage stop",
        summary: "unused JavaScript and CSS bytes, in total and per file",
        schema: || schema_for!(Envelope<CoverageStopData>),
    },
    CommandSchema {
        name: "highlight",
        summary: "the element highlighted in the page, and for how long",
        schema: || schema_for!(Envelope<HighlightData>),
    },
    CommandSchema {
        name: "inspect",
        summary: "the tab DevTools opened for (headed browsers)",
        schema: || schema_for!(Envelope<InspectData>),
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
        name: "locator",
        summary: "the most robust locator that finds one element alone",
        schema: || schema_for!(Envelope<LocatorData>),
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
        schema: || schema_for!(Envelope<Acted<PointerData>>),
    },
    CommandSchema {
        name: "dblclick",
        summary: "the element double-clicked and the point",
        schema: || schema_for!(Envelope<Acted<PointerData>>),
    },
    CommandSchema {
        name: "hover",
        summary: "the element the mouse is over and the point",
        schema: || schema_for!(Envelope<Acted<PointerData>>),
    },
    CommandSchema {
        name: "tap",
        summary: "the element tapped and the point",
        schema: || schema_for!(Envelope<Acted<PointerData>>),
    },
    CommandSchema {
        name: "mouse move",
        summary: "where the mouse is now and the buttons held",
        schema: || schema_for!(Envelope<Acted<MouseData>>),
    },
    CommandSchema {
        name: "mouse down",
        summary: "the button pressed, where, and the buttons held",
        schema: || schema_for!(Envelope<Acted<MouseData>>),
    },
    CommandSchema {
        name: "mouse up",
        summary: "the button released, where, and the buttons held",
        schema: || schema_for!(Envelope<Acted<MouseData>>),
    },
    CommandSchema {
        name: "mouse wheel",
        summary: "the wheel's deltas and where the mouse is",
        schema: || schema_for!(Envelope<Acted<MouseData>>),
    },
    CommandSchema {
        name: "drag",
        summary: "the elements dragged from and to, the points, and how",
        schema: || schema_for!(Envelope<Acted<DragResult>>),
    },
    CommandSchema {
        name: "scroll",
        summary: "what scrolled, its new scroll position, and which ends it reached",
        schema: || schema_for!(Envelope<Acted<ScrollData>>),
    },
    CommandSchema {
        name: "press",
        summary: "the keys pressed and the element that had focus",
        schema: || schema_for!(Envelope<Acted<KeysData>>),
    },
    CommandSchema {
        name: "type",
        summary: "how many characters were typed and into which element",
        schema: || schema_for!(Envelope<Acted<KeysData>>),
    },
    CommandSchema {
        name: "fill",
        summary: "the field filled and the value it holds now",
        schema: || schema_for!(Envelope<Acted<FillData>>),
    },
    CommandSchema {
        name: "select",
        summary: "the <select> and the options now selected",
        schema: || schema_for!(Envelope<Acted<SelectData>>),
    },
    CommandSchema {
        name: "check",
        summary: "the checkbox, radio or switch, and whether it had to change",
        schema: || schema_for!(Envelope<Acted<CheckData>>),
    },
    CommandSchema {
        name: "uncheck",
        summary: "the checkbox or switch, and whether it had to change",
        schema: || schema_for!(Envelope<Acted<CheckData>>),
    },
    CommandSchema {
        name: "focus",
        summary: "the element that has focus now",
        schema: || schema_for!(Envelope<Acted<FocusData>>),
    },
    CommandSchema {
        name: "blur",
        summary: "the element that lost focus",
        schema: || schema_for!(Envelope<Acted<FocusData>>),
    },
    CommandSchema {
        name: "upload",
        summary: "the file input and the files it holds now",
        schema: || schema_for!(Envelope<Acted<UploadData>>),
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
    CommandSchema {
        name: "batch",
        summary: "one line per step: its place, its command, and that command's envelope",
        schema: || schema_for!(StepLine),
    },
    CommandSchema {
        name: "run",
        summary: "one line per step: its place, its command, and that command's envelope",
        schema: || schema_for!(StepLine),
    },
    CommandSchema {
        name: "test",
        summary: "every test of the run with its result, and the counts; on failure the same data is the error's details",
        schema: || schema_for!(Envelope<TestRunData>),
    },
    CommandSchema {
        name: "history",
        summary: "the commands the session ran, or with --as-scenario the test they make",
        schema: || schema_for!(Envelope<HistoryData>),
    },
    CommandSchema {
        name: "convert",
        summary: "the .cdp script translated from a Playwright test file, and its TODO lines",
        schema: || schema_for!(Envelope<ConvertData>),
    },
    CommandSchema {
        name: "fmt",
        summary: "the .cdp and mock files looked at, and those formatted (or, with --check, not formatted)",
        schema: || schema_for!(Envelope<FmtData>),
    },
    CommandSchema {
        name: "lint",
        summary: "the problems found in scenario and mock files: file:line:col, severity, rule, message and hint",
        schema: || schema_for!(Envelope<LintData>),
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

/// `data` of `chrome-cdp help --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelpData {
    /// What was asked for: `chrome-cdp` (the overview), `commands`, a
    /// topic (`locators`) or a command (`tab close`).
    pub topic: String,
    /// The help text, as printed.
    pub text: String,
}

/// `data` of `chrome-cdp skill --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillData {
    /// `skill`, `agents-md` or `llms-txt`.
    pub format: String,
    /// The file's text (not with `--install`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// With `--install`: the absolute path of the SKILL.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// With `--install`: what happened to the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<SkillWrite>,
}

/// `data` of `chrome-cdp convert --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConvertData {
    /// The Playwright file, as given (`stdin` for `-`).
    pub file: String,
    /// The `.cdp` file: a scenario for `chrome-cdp test`, or a plain
    /// script for `chrome-cdp run` when the Playwright file has no tests.
    pub script: String,
    /// How many tests (`@test` blocks) it has.
    pub tests: usize,
    /// How many command lines it has.
    pub steps: usize,
    /// The statements it could not translate, in the file as TODO lines.
    pub todos: Vec<ConvertTodo>,
    /// With `-o`: the absolute path of the file written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A statement `convert` left as a TODO line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConvertTodo {
    /// Its line in the Playwright file.
    pub line: usize,
    /// Why it was not translated, often with what to use instead.
    pub reason: String,
    /// Its source, on one line.
    pub code: String,
}

/// What `skill --install` did to the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SkillWrite {
    /// It did not exist.
    Created,
    /// It held another version.
    Updated,
    /// It was the same already.
    Unchanged,
}

/// `data` of `chrome-cdp init e2e --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitData {
    /// The project root, absolute: where the files were written.
    pub dir: String,
    /// The framework found in `package.json` (or given with
    /// `--framework`): `vite`, `next`, `cra`, `astro` or `sveltekit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// `npm`, `pnpm`, `yarn` or `bun`, when there is a `package.json` or a
    /// framework.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_manager: Option<String>,
    /// The `[e2e]` `base_url` written; none when the config had an
    /// `[e2e]` section already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The `[e2e.web_server]` written, if a framework was found (and the
    /// section was written).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_server: Option<InitWebServer>,
    /// Every file `init` looked at, in the order written.
    pub files: Vec<InitFile>,
}

/// The web server `init e2e` wrote into the config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitWebServer {
    /// The shell command that serves the app.
    pub command: String,
    /// The URL that answers when it is up.
    pub url: String,
}

/// A file of the layout, and what `init e2e` did to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitFile {
    /// Its absolute path.
    pub path: String,
    pub action: InitAction,
    /// Why it was kept or added to, e.g. `has an [e2e] section`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What `init e2e` did to a file. It never overwrites one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum InitAction {
    /// It did not exist.
    Created,
    /// Lines were added at its end (the `[e2e]` section, a `.gitignore`
    /// entry).
    Appended,
    /// It exists and was left as it is.
    Kept,
}

/// `data` of `chrome-cdp completions --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompletionsData {
    /// The shell: `bash`, `zsh`, `fish`, `elvish` or `powershell`.
    pub shell: String,
    /// The script.
    pub script: String,
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

/// `data` of `chrome-cdp doctor --json`; with a failed check it is the
/// error's `details`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorData {
    /// Every check, in the order they ran.
    pub checks: Vec<DoctorCheck>,
    /// How many checks passed, warned, failed and were skipped.
    pub ok: usize,
    pub warnings: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// One check of `chrome-cdp doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorCheck {
    /// `chrome-cdp`, `chrome`, `libraries`, `sandbox`, `launch`,
    /// `session-dir`, `quarantine` or `sessions`.
    pub name: String,
    pub status: DoctorStatus,
    /// What was found, in one line.
    pub message: String,
    /// The command or step that fixes it, when it is not `ok`.
    pub fix: Option<String>,
}

/// How a `doctor` check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Ok,
    /// Works, but something is off (an old Chrome, a session on an older
    /// build).
    Warn,
    /// Stops chrome-cdp from working.
    Fail,
    /// Not checked here (another OS, or an earlier check failed).
    Skip,
}

impl DoctorStatus {
    /// `ok`, `warn`, `FAIL` or `skip`, as the text output shows it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
            Self::Skip => "skip",
        }
    }
}

/// `data` of `chrome-cdp upgrade --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpgradeData {
    /// The version running the command.
    pub from: String,
    /// The version of the release looked at: the newest, or `--version`'s.
    pub to: String,
    /// Whether `to` is newer than `from`.
    pub newer: bool,
    /// Whether the binary was replaced (never with `--check`).
    pub upgraded: bool,
    /// The binary that was, or would be, replaced.
    pub path: String,
    /// The release archive for this machine.
    pub archive: String,
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
    /// The unpacked extensions `--extension` loaded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<ExtensionInfo>,
    /// The DevTools WebSocket URL (connected browsers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// An unpacked extension `open --extension` loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtensionInfo {
    /// Its id, the host of its pages: `chrome-extension://<id>/…`.
    pub id: String,
    /// Its name, from its manifest.
    pub name: String,
    /// Its version, from its manifest.
    pub version: String,
    /// The directory it was loaded from.
    pub path: String,
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
    /// The device emulated (`--device`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// The storage state loaded (`--state`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StateData>,
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
    /// Console messages and requests of shared and service workers that
    /// no tab had a frame on the origin of, so no tab's log has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_events_dropped: Option<u64>,
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
    /// The browser context it is in (`c1`, from `chrome-cdp context
    /// new`); absent for the session's default context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
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

/// How severe a console entry is, most severe first. `--level warn`
/// shows `error` and `warn` entries.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

/// `data` of `chrome-cdp console --json` and `chrome-cdp errors --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConsoleData {
    /// The tab, e.g. `t1`.
    pub tab: String,
    /// The entries shown, oldest first.
    pub entries: Vec<ConsoleEntry>,
    /// Older entries that matched but `--limit` left out.
    pub skipped: usize,
    /// Entries the tab's buffer dropped before anyone read them (it keeps
    /// the newest `open --console-buffer`, 1,000 by default).
    pub dropped: u64,
    /// The read cursor now: the next `--since last` shows entries after
    /// this sequence number.
    pub cursor: u64,
}

/// One console message, uncaught exception or browser message (a failed
/// load, a CSP violation, an intervention).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConsoleEntry {
    /// Its place in the tab's console; later ones are larger
    /// (`--since <seq>`).
    pub seq: u64,
    /// When the daemon received it, in ms since the Unix epoch.
    pub time_ms: u64,
    pub level: Level,
    /// `console` (a `console.*` call), `exception` (thrown, never
    /// caught), or where Chrome's own message came from: `network`,
    /// `security`, `violation`, `intervention`, `deprecation`, …
    pub source: String,
    /// The `console` method (`log`, `error`, `table`, …), or for an
    /// exception `uncaught` or `uncaught (in promise)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The message, its arguments formatted like DevTools does (objects
    /// from their preview).
    pub text: String,
    /// The script (or, for a failed load, the resource) it is about, with
    /// line and column (1-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// The top of the stack (5 frames at most), innermost first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stack: Vec<StackFrame>,
    /// The worker's script, for a message of a worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    /// What kind of worker `worker` is. A shared or service worker's
    /// message is in each tab of its origin (plan 13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_kind: Option<WorkerKind>,
}

/// One frame of a stack trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StackFrame {
    /// The function; empty at a script's top level.
    pub function: String,
    pub url: String,
    /// 1-based.
    pub line: u32,
    pub column: u32,
    /// Where it is in the original source, when a source map says so
    /// (`--sourcemaps`, on by default for localhost pages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<SourcePosition>,
}

/// A place in an original source file, from a source map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourcePosition {
    /// The source file as the map names it, resolved against the map's
    /// URL (`http://localhost:5173/src/app.ts`) unless it has its own
    /// scheme (`webpack://app/./src/app.ts`).
    pub source: String,
    /// 1-based.
    pub line: u32,
    pub column: u32,
    /// The original name at that place, if the map has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `data` of `chrome-cdp network --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkData {
    /// The tab, e.g. `t1`.
    pub tab: String,
    /// The requests shown, oldest first. A request shown while it was
    /// pending is shown once more when it ends.
    pub requests: Vec<NetworkEntry>,
    /// Older requests that matched but `--limit` left out.
    pub skipped: usize,
    /// Requests the tab's log dropped before anyone read them (it keeps
    /// the newest `open --network-buffer`, 2,000 by default).
    pub dropped: u64,
    /// The read cursor now: the next `--since last` shows requests after
    /// this id.
    pub cursor: u64,
}

/// Where a request is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RequestState {
    /// Sent, not finished yet (a WebSocket: still open).
    Pending,
    /// Finished with a response (whatever its status), or redirected.
    Done,
    /// Ended without a response: `error` says why.
    Failed,
    /// Canceled by the page or a navigation.
    Canceled,
}

/// One request of the tab's network log (each redirect hop is its own).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkEntry {
    /// Its id in the tab's log (`network show <id>`); later ones are
    /// larger (`--since <id>`).
    pub id: u64,
    /// When the daemon saw it start, in ms since the Unix epoch.
    pub time_ms: u64,
    pub method: String,
    pub url: String,
    /// Chrome's resource type in lower case: `document`, `xhr`, `fetch`,
    /// `script`, `stylesheet`, `image`, `websocket`, …
    #[serde(rename = "type")]
    pub kind: String,
    pub state: RequestState,
    /// The HTTP status, once a response came.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    /// The response's MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// `http/1.1`, `h2`, `h3`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Bytes received over the network, headers included (0 from a
    /// cache); for a WebSocket, the frames' payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The body's size once decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_size: Option<u64>,
    /// From start to end (or to the redirect); for a WebSocket, how long
    /// it was open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Which cache served it: `memory`, `disk` or `prefetch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<String>,
    /// A service worker answered it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub service_worker: bool,
    /// What sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<Initiator>,
    /// Where a redirect sent it (the next hop is the next request with
    /// this URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
    /// Chrome's error, for a failed request: `net::ERR_CONNECTION_REFUSED`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why Chrome blocked it: `csp`, `mixed-content`, `inspector`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    /// The CORS check that failed: `MissingAllowOriginHeader`, and the
    /// value at fault if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cors: Option<String>,
    /// A WebSocket's traffic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket: Option<WebSocketSummary>,
    /// The worker that sent it, for a request of a worker (not of a
    /// frame).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<Worker>,
}

/// A worker, as an entry of a tab's log names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Worker {
    pub kind: WorkerKind,
    /// Its script.
    pub url: String,
}

/// What kind of worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WorkerKind {
    /// A page's or iframe's own `Worker`, of one tab.
    Dedicated,
    /// A `SharedWorker`: of every tab of its origin.
    Shared,
    /// A service worker: of every tab of its origin.
    Service,
}

impl WorkerKind {
    /// `dedicated worker`, `shared worker`, `service worker`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dedicated => "dedicated worker",
            Self::Shared => "shared worker",
            Self::Service => "service worker",
        }
    }
}

impl Worker {
    /// `[service worker sw.js]`: its kind and its script's file name.
    pub fn label(&self) -> String {
        let path = self.url.split(['?', '#']).next().unwrap_or_default();
        let file = path.rsplit('/').next().unwrap_or_default();
        let file = if file.is_empty() {
            self.url.as_str()
        } else {
            file
        };
        format!("[{} {file}]", self.kind.as_str())
    }
}

/// What sent a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Initiator {
    /// `parser` (the HTML), `script`, `preload`, `preflight`, `other`, …
    #[serde(rename = "type")]
    pub kind: String,
    /// The document or script, with the line (1-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The function that sent it, for a script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}

/// A WebSocket's frames, summed up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebSocketSummary {
    /// Frames the page sent, and their payload bytes.
    pub sent: u64,
    pub sent_bytes: u64,
    /// Frames it received, and their payload bytes.
    pub received: u64,
    pub received_bytes: u64,
    /// Closed (else still open).
    pub closed: bool,
}

/// `data` of `chrome-cdp network show --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkShowData {
    /// The tab, e.g. `t1`.
    pub tab: String,
    pub request: NetworkEntry,
    /// Where the time went, from Chrome's resource timing (absent from a
    /// cache).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
    /// With `--headers` (else only counted in the text form).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<Vec<Header>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<Vec<Header>>,
    /// With `--request-body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<Body>,
    /// With `--body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<Body>,
    /// A WebSocket's last frames (at most 20), oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<WebSocketFrame>,
}

/// A request's phases, in ms.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Timing {
    /// Waiting before the request could go (queueing, proxy, …).
    pub blocked_ms: u64,
    pub dns_ms: u64,
    /// Opening the connection, TLS included.
    pub connect_ms: u64,
    pub tls_ms: u64,
    pub send_ms: u64,
    /// For the first byte of the response (the server's time).
    pub wait_ms: u64,
    /// Reading the response, if it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_ms: Option<u64>,
}

/// One HTTP header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// A request's or response's body, fetched from Chrome when asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Body {
    /// The body as text (a JSON body pretty-printed only in the text
    /// form); absent for a binary or unavailable body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Its size in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Not text (an image, a font, …): not shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
    /// Why there is no body to show: Chrome no longer has it, the request
    /// failed or is still loading, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// With `-o`: the file the body was saved to (absolute), instead of
    /// `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One WebSocket frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebSocketFrame {
    /// Sent by the page (else received).
    pub sent: bool,
    pub time_ms: u64,
    /// `text`, `binary`, `close`, `ping` or `pong`.
    pub opcode: String,
    /// The payload's size in bytes.
    pub size: u64,
    /// The start of a text payload (at most 200 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// `data` of `chrome-cdp network clear --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkClearData {
    /// The tab, e.g. `t1`.
    pub tab: String,
    /// How many requests the log held.
    pub cleared: usize,
}

/// `data` of `chrome-cdp screenshot --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotData {
    /// The tab captured, e.g. `t1`.
    pub tab: String,
    /// The image file (absolute).
    pub path: String,
    /// `png`, `jpeg` or `webp`.
    pub format: String,
    /// The image's size in pixels.
    pub width: u32,
    pub height: u32,
    /// The file's size in bytes.
    pub bytes: u64,
    /// The element captured; `null` for the viewport or the full page.
    pub locator: Option<String>,
    /// `--full`: the whole page.
    pub full: bool,
    /// `--full` of a page that scrolls inside a container: that
    /// container (`main#feed`), whose whole content is in the image; the
    /// viewport grew for the capture.
    #[serde(default)]
    pub container: Option<String>,
    /// Image pixels per CSS pixel: 1 at `--scale css`, the device pixel
    /// ratio at `--scale device`, less when `--max-size` scaled it down.
    /// A point (x, y) in the image is (x / scale, y / scale) from the
    /// captured area's top-left corner, in CSS pixels.
    pub scale: f64,
    /// The capture was scaled down to fit `--max-size`.
    pub downscaled: bool,
    /// `--annotate`: the refs labelled in the image.
    #[serde(default)]
    pub annotated: Vec<String>,
    /// `--mask`: how many elements were painted over.
    #[serde(default)]
    pub masked: usize,
}

/// `data` of `chrome-cdp pdf --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PdfData {
    /// The tab printed, e.g. `t1`.
    pub tab: String,
    /// The PDF file (absolute).
    pub path: String,
    /// The file's size in bytes.
    pub bytes: u64,
    /// How many pages it has, when that could be read from the file.
    pub pages: Option<u32>,
    /// The paper size: `Letter`, `A4`, …
    pub paper: String,
    pub landscape: bool,
}

/// `data` of `chrome-cdp har start --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarStartData {
    /// The tab recorded (`--tab`); every tab, the ones opened later too,
    /// if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// What happens to response bodies: `embed` or `omit`.
    pub content: String,
    /// Whether a recording was running already (it goes on, with this
    /// `content`).
    pub already: bool,
}

/// `data` of `chrome-cdp har stop --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarStopData {
    /// The HAR file (absolute).
    pub path: String,
    /// The file's size in bytes.
    pub bytes: u64,
    /// How many requests it has.
    pub entries: usize,
    /// The tabs they came from (the HAR's pages), e.g. `t1`.
    pub pages: Vec<String>,
    /// `embed` or `omit`.
    pub content: String,
    /// Response bodies in the file.
    pub bodies: usize,
    /// Response bodies Chrome no longer had, left out.
    pub missing_bodies: usize,
    /// Requests the network log dropped (or `network clear` removed)
    /// before the recording stopped; they are not in the file.
    pub dropped: u64,
    /// How long it recorded, in ms.
    pub duration_ms: u64,
}

/// A routing rule (`route`, `block`; plan 07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RouteInfo {
    /// Its id, e.g. `r1`: counted per session, never reused.
    pub id: String,
    /// The URL pattern as typed: a glob, a `/regex/`, or a part of the URL.
    pub pattern: String,
    /// What the rule is called instead of its pattern (`block --ads`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The methods it matches (upper case); any if empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    /// The resource types it matches (as `network --type`); any if empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_types: Vec<String>,
    /// `fulfill`, `abort`, `continue` or `patch_json`.
    pub action: String,
    /// The action in a few words: `fulfill 200 application/json 17B`.
    pub summary: String,
    /// The status it answers with (`fulfill`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// How long it waits before acting, in ms.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay_ms: u64,
    /// How many requests it matched.
    pub hits: u64,
    /// How many more matches before it is removed (`--times`); no limit
    /// if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<u32>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// `data` of `chrome-cdp route --json` and `chrome-cdp block --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RouteData {
    /// The rules added, in the order they are tried.
    pub added: Vec<RouteInfo>,
    /// How many rules the session has now.
    pub active: usize,
}

/// A request that routing saw (`routes --log`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutedRequest {
    /// Counts every request routing saw in the session.
    pub seq: u64,
    /// When the daemon saw it (ms since the Unix epoch).
    pub time_ms: u64,
    /// The tab, e.g. `t1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    pub method: String,
    pub url: String,
    /// Its resource type, as `network --type` names them.
    pub resource_type: String,
    /// The rule that handled it; none if no rule matched and it went on
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// The mock that answered it (plan 11), when no rule did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock: Option<MockRef>,
    /// The mock set whose `scope` it is in, when no mock answered it:
    /// it was unmatched, and the set's `unmatched` policy handled it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmatched: Option<String>,
    /// With `unmatched`: that set's policy, `fail`, `passthrough` or
    /// `abort`. A `fail` fails the test that sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    /// With `unmatched`: the mocks that come closest to answering it,
    /// best first, each with the condition it fails.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closest: Vec<MockCheck>,
    /// What happened: `fulfill 200`, `abort failed`, `continue`, …
    pub outcome: String,
}

/// `data` of `chrome-cdp routes --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutesData {
    /// The rules, in the order they are tried.
    pub routes: Vec<RouteInfo>,
    /// With `--log`: the newest requests routing saw, oldest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<Vec<RoutedRequest>>,
    /// With `--log`: older requests `--limit` left out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped: u64,
}

/// `data` of `chrome-cdp unroute --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnrouteData {
    /// The ids of the rules removed.
    pub removed: Vec<String>,
    /// How many rules are left.
    pub active: usize,
}

/// An init script (`init-script`, plan 07): it runs in every new document
/// of the session's tabs, popups and frames before the page's own scripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitScriptInfo {
    /// `s1`, `s2`, …; never reused in a session.
    pub id: String,
    /// `main` (the page's own globals) or `isolated` (chrome-cdp's world,
    /// as `eval --world isolated` sees it).
    pub world: String,
    /// The file it was read from (absolute); none for `--code` or stdin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Its first line, cut at 60 characters.
    pub preview: String,
    /// Its size in bytes.
    pub size: u64,
}

/// `data` of `chrome-cdp init-script add --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitScriptAddData {
    pub added: InitScriptInfo,
    /// Whether it also ran in the documents open when it was added
    /// (`--now`); otherwise it runs from their next navigation.
    pub ran_now: bool,
    /// How many init scripts the session has now.
    pub active: usize,
}

/// `data` of `chrome-cdp init-script list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitScriptsData {
    /// In the order they run.
    pub scripts: Vec<InitScriptInfo>,
}

/// `data` of `chrome-cdp init-script remove --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitScriptRemoveData {
    /// The ids of the scripts removed.
    pub removed: Vec<String>,
    /// How many are left.
    pub active: usize,
}

/// `data` of `chrome-cdp expose --json` and `chrome-cdp unexpose --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExposeData {
    /// The names this command added (`expose`) or removed (`unexpose`).
    pub changed: Vec<String>,
    /// Every function the session exposes now, oldest first.
    pub bindings: Vec<String>,
}

/// A cookie (`cookies`, plan 07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    /// `example.com` for a host-only cookie, `.example.com` for one its
    /// subdomains get too.
    pub domain: String,
    pub path: String,
    /// When it expires, in seconds since 1970; none for a session cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    pub http_only: bool,
    pub secure: bool,
    /// `Strict`, `Lax` or `None`; none when the cookie did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
    /// The top-level site of a partitioned (CHIPS) cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_key: Option<String>,
}

/// `data` of `chrome-cdp cookies --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CookiesData {
    /// By domain, then path, then name.
    pub cookies: Vec<CookieInfo>,
}

/// `data` of `chrome-cdp cookies set --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CookieSetData {
    pub cookie: CookieInfo,
}

/// `data` of `chrome-cdp cookies clear --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CookiesClearData {
    /// How many cookies were removed.
    pub removed: usize,
}

/// A name and value of `localStorage` or `sessionStorage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageItem {
    pub name: String,
    pub value: String,
}

/// `data` of `chrome-cdp storage --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageData {
    /// `local` or `session`.
    pub area: String,
    /// The origin whose storage it is.
    pub origin: String,
    /// The tab whose frame was used (`t1`).
    pub tab: String,
    /// `list`, `get`, `set`, `remove` or `clear`.
    pub action: String,
    /// `list`: every item, in key order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<StorageItem>,
    /// The key `get`, `set` or `remove` used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// `get`: the value (none when the key is not there); `set`: the value
    /// set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// `set`, `remove`, `clear`: how many items there are now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
}

/// One origin of a storage state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StateOriginInfo {
    pub origin: String,
    /// How many `localStorage` items.
    pub local_storage: usize,
    /// How many `sessionStorage` items (chrome-cdp's extension).
    pub session_storage: usize,
    /// How many IndexedDB databases (`--indexeddb`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_db: Option<usize>,
    /// `state load`: `now` when a page of the origin was open and got it at
    /// once; `next-load` when the origin's next document gets it, before
    /// its scripts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<String>,
}

/// `data` of `chrome-cdp state save --json` and `state load --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StateData {
    /// The file written or read (absolute); none for stdin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How many cookies.
    pub cookies: usize,
    pub origins: Vec<StateOriginInfo>,
}

/// The page size and kind of screen (`viewport`, or a device's; plan 07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Viewport {
    /// In CSS pixels.
    pub width: u32,
    pub height: u32,
    /// `devicePixelRatio`.
    pub dpr: f64,
    /// A mobile browser: the meta viewport tag counts, scrollbars overlay.
    pub mobile: bool,
    /// Touch events and `navigator.maxTouchPoints` > 0.
    pub touch: bool,
    /// The screen (`screen.width`/`height`) if not the viewport's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_height: Option<u32>,
}

/// Where `navigator.geolocation` says the device is (`geo`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Geolocation {
    pub latitude: f64,
    pub longitude: f64,
    /// In metres.
    pub accuracy: f64,
}

/// The CSS media the page sees (`media`); a feature left out is the
/// browser's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Media {
    /// `screen` or `print`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// `prefers-color-scheme`: `light` or `dark`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<String>,
    /// `prefers-reduced-motion`: `reduce` or `no-preference`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduced_motion: Option<String>,
    /// `forced-colors`: `active` or `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_colors: Option<String>,
}

impl Media {
    /// Whether nothing is emulated.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A slow network (`throttle`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Throttle {
    /// `slow-3g`, `fast-3g` or `4g`; absent for custom values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Added to every request before its response starts, in ms.
    pub latency_ms: f64,
    /// Download bandwidth in kbit/s; absent for no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_kbps: Option<f64>,
    /// Upload bandwidth in kbit/s; absent for no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_kbps: Option<f64>,
}

/// The time pages see while it is frozen (`time freeze`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FrozenTime {
    /// `Date.now()` in the page: ms since the Unix epoch.
    pub epoch_ms: i64,
    /// The same in ISO 8601 (UTC).
    pub iso: String,
    /// Whether timers and animation frames are stopped too (`time
    /// pause`) until `time run-for`, `pause-at` or `resume`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
}

/// What moving a paused clock on fired (`time run-for`, `pause-at`, and
/// `advance` while paused), in every page, frame and worker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClockFired {
    /// `setTimeout` and `setInterval` callbacks run (each interval tick
    /// counts).
    pub timers: u64,
    /// Animation frames run (each runs every `requestAnimationFrame`
    /// callback queued for it).
    pub frames: u64,
}

/// What the session emulates (plan 07). Every field left out (or
/// `false`) is the browser's own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Emulation {
    /// The device `emulate` made the pages look like, as long as its
    /// viewport and user agent stay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// The viewport; absent for the default (`default_viewport`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// A BCP 47 tag: `Intl`, `navigator.language` and `Accept-Language`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// An IANA time zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geolocation: Option<Geolocation>,
    #[serde(default, skip_serializing_if = "Media::is_empty")]
    pub media: Media,
    /// Whether the network is off (`navigator.onLine` is false and
    /// requests fail).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throttle: Option<Throttle>,
    /// How many times slower the CPU runs (`cpu-throttle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_throttle: Option<f64>,
    /// The time `Date` gives while frozen (`time freeze`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<FrozenTime>,
}

/// A device `emulate` can be (`emulate --list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceInfo {
    pub name: String,
    pub viewport: Viewport,
    pub user_agent: String,
}

/// `data` of `chrome-cdp emulate --json` and of `viewport`, `geo`, `tz`,
/// `locale`, `ua`, `media`, `offline`, `throttle`, `cpu-throttle` and
/// `time`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EmulateData {
    /// Everything the session emulates now.
    pub emulation: Emulation,
    /// The viewport pages get without one (1280x800 in headless mode;
    /// absent when the window sets it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_viewport: Option<Viewport>,
    /// With `emulate --list`: the devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<DeviceInfo>>,
    /// After moving a paused clock on: what it fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired: Option<ClockFired>,
}

/// A permission the session set (`permissions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionInfo {
    /// As the Permissions API names it: `geolocation`, `notifications`, …
    pub name: String,
    /// `granted` or `denied`.
    pub setting: String,
    /// The origin it applies to; every origin if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// `data` of `chrome-cdp permissions --json` and its subcommands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionsData {
    /// The permissions the session set, oldest first.
    pub permissions: Vec<PermissionInfo>,
}

/// `data` of `chrome-cdp trace start --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TraceStartData {
    /// The tab whose session records, e.g. `t1`.
    pub tab: String,
    /// The trace categories (`-name`: left out).
    pub categories: Vec<String>,
    /// Whether film strip screenshots are recorded.
    pub screenshots: bool,
    /// Whether a trace was running already (it goes on as it was).
    pub already: bool,
}

/// `data` of `chrome-cdp trace stop --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TraceStopData {
    /// The trace file (absolute), JSON (gzipped if it ends in `.gz`).
    pub path: String,
    /// The file's size in bytes.
    pub bytes: u64,
    /// The tab whose session recorded, e.g. `t1`.
    pub tab: String,
    /// How long it recorded, in ms.
    pub duration_ms: u64,
    /// Chrome's trace buffer filled up and events were lost.
    pub data_loss: bool,
}

/// `data` of `chrome-cdp perf --json`. Times are ms since the navigation
/// started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PerfData {
    /// The tab measured, e.g. `t1`.
    pub tab: String,
    /// The page's URL.
    pub url: String,
    /// The page's load, from Navigation Timing (absent for a page with no
    /// navigation entry, such as `about:blank`).
    pub navigation: Option<PerfNavigation>,
    /// First Contentful Paint.
    pub fcp_ms: Option<u64>,
    /// Largest Contentful Paint, if the page painted one.
    pub lcp: Option<PerfLcp>,
    /// Cumulative Layout Shift: the worst session window's shifts.
    pub cls: PerfCls,
    /// Interaction to Next Paint, once the page had an interaction.
    pub inp: Option<PerfInp>,
    /// Interactions so far (clicks, taps, key presses).
    pub interactions: u64,
    /// Total Blocking Time: long tasks' time over 50ms each, after FCP.
    pub tbt_ms: u64,
    /// Long tasks (50ms or more) after FCP.
    pub long_tasks: u64,
    /// Requests the page made (its document included).
    pub requests: u64,
    /// Bytes they transferred (0 for cached and cross-origin ones without
    /// `Timing-Allow-Origin`).
    pub transfer_bytes: u64,
    /// Elements in the document.
    pub elements: u64,
    /// How long ago the navigation started.
    pub since_load_ms: u64,
    /// Whether chrome-cdp watched the page from its start. If not, INP
    /// counts only interactions of 104ms or more (what the browser keeps).
    pub from_start: bool,
    /// `Performance.getMetrics` of the tab's renderer: `JSHeapUsedSize`,
    /// `JSEventListeners`, `Nodes`, …
    pub metrics: std::collections::BTreeMap<String, f64>,
}

/// Navigation Timing of `perf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PerfNavigation {
    /// `navigate`, `reload`, `back_forward` or `prerender`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Time to First Byte.
    pub ttfb_ms: Option<u64>,
    /// When `DOMContentLoaded` handlers finished (absent until then).
    pub dom_content_loaded_ms: Option<u64>,
    /// When `load` handlers finished (absent until then).
    pub load_ms: Option<u64>,
}

/// Largest Contentful Paint of `perf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PerfLcp {
    pub ms: u64,
    /// `good`, `needs improvement` or `poor` (Web Vitals thresholds).
    pub rating: String,
    /// The element, e.g. `img#hero`, if it is still in the document.
    pub element: Option<String>,
    /// The image's URL, for an image.
    pub url: Option<String>,
}

/// Cumulative Layout Shift of `perf`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PerfCls {
    pub value: f64,
    pub rating: String,
    /// Layout shifts not caused by input.
    pub shifts: u64,
}

/// Interaction to Next Paint of `perf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PerfInp {
    pub ms: u64,
    pub rating: String,
    /// The event of that interaction, e.g. `click` or `keydown`.
    pub event: String,
    /// Its target, e.g. `button#save`.
    pub target: Option<String>,
}

/// `data` of `chrome-cdp coverage start --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoverageStartData {
    /// The tab measured, e.g. `t1`.
    pub tab: String,
    /// Whether coverage was on already (it goes on from its start).
    pub already: bool,
}

/// `data` of `chrome-cdp coverage stop --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoverageStopData {
    /// The tab measured, e.g. `t1`.
    pub tab: String,
    /// JavaScript: all files together.
    pub js: CoverageTotal,
    /// CSS: all stylesheets together.
    pub css: CoverageTotal,
    /// The files with the most unused bytes first (`--limit` of them).
    pub files: Vec<CoverageFile>,
    /// Files `--limit` left out.
    pub skipped: usize,
    /// How long it measured, in ms.
    pub duration_ms: u64,
}

/// Coverage of all files of one kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoverageTotal {
    pub files: usize,
    /// Their size (in characters, as DevTools' Coverage panel counts).
    pub total: u64,
    /// What never ran (JS) or never matched (CSS).
    pub unused: u64,
}

/// Coverage of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoverageFile {
    /// `js` or `css`.
    pub kind: String,
    /// The script's or stylesheet's URL; the document's for inline ones.
    pub url: String,
    /// Inline `<script>`s or `<style>`s of the document, summed.
    pub inline: bool,
    pub total: u64,
    pub unused: u64,
    /// The out-of-process (cross-site) iframe it ran in, by frame id;
    /// `None` for the tab's own process (its page and same-site iframes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// The worker it ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<Worker>,
}

/// `data` of `chrome-cdp highlight --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HighlightData {
    /// The tab, e.g. `t1`.
    pub tab: String,
    /// The element highlighted.
    pub element: ActedElement,
    /// How long it stays highlighted, in ms (0: until the next highlight).
    pub duration_ms: u64,
}

/// `data` of `chrome-cdp inspect --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InspectData {
    /// The tab DevTools opened for, e.g. `t1`.
    pub tab: String,
    /// The panel asked for, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<String>,
    /// The DevTools window's target id.
    pub devtools_target: String,
}

/// `data` of `chrome-cdp cdp --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CdpData {
    /// With `--listen`: the call's result and the events.
    Listened(CdpListenData),
    /// Chrome's result object, as it came.
    Result(Map<String, Value>),
}

/// `data` of `chrome-cdp cdp --listen … --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CdpListenData {
    /// The call's result object; absent when no method was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Map<String, Value>>,
    /// The events captured, in the order they came.
    pub events: Vec<CdpEvent>,
}

/// One raw CDP event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CdpEvent {
    /// `Domain.event`.
    pub method: String,
    pub params: Value,
    /// With `--all-sessions`: what sent it, `main` (the tab's page),
    /// `iframe "pay"` (an out-of-process iframe, by its name or URL), or
    /// `dedicated worker w.js` (`shared …`, `service …`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// With `--all-sessions`: the id of the CDP session it came on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// `data` of `chrome-cdp eval --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvalData {
    /// The tab, e.g. `t1`.
    pub tab: String,
    /// `--in`: the element bound as `el`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    /// `--frame`: the frame the script ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// `main` or `isolated`.
    pub world: String,
    /// What the value is: `undefined`, `null`, `boolean`, `number`,
    /// `string`, `bigint`, `symbol`, `function`, `array`, `node` or
    /// `object`. A promise is awaited and a function called (with `el`)
    /// first, so neither is ever the type.
    #[serde(rename = "type")]
    pub kind: String,
    /// The value as JSON: `undefined` is `null` (see `type`); `NaN`,
    /// `Infinity`, bigints (`12n`) and symbols are strings; a DOM node is
    /// a string such as `button#save.primary`, with its ref when it is an
    /// element of the frame's document (`button#save.primary [e14]`); a
    /// `Date` its ISO string; a `Map` an object (or an array of
    /// `[key, value]` pairs); a `Set` an array; a cycle `"[Circular]"`.
    pub value: Value,
    /// The refs the elements in the value have, in the order they first
    /// appear (at most 50 per call).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    /// How many more elements are in the value, past the 50 that get
    /// refs.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub refs_cut: u64,
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

/// `data` of `chrome-cdp locator --json` (plan 14, milestone 14.1): the
/// locator a test should use for one element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocatorData {
    /// The most robust locator that finds the element alone, e.g.
    /// `role=button[name="Delete"] >> nth=1`; it starts with `frame=`
    /// for an element in an iframe.
    pub locator: String,
    /// How it finds it: `role`, `label`, `testid`, `placeholder`, `text`,
    /// `scoped` (inside a unique container), `has` (inside the container
    /// that has a text) or `nth` (by position; the least robust).
    pub how: String,
    /// `--all`: every candidate tried that finds the element, best first,
    /// with how many elements each finds (`1`: usable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<LocatorCandidate>>,
}

/// One candidate of `locator --all`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocatorCandidate {
    pub locator: String,
    /// How many elements it finds; `1` means the element alone.
    pub count: u64,
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

/// `data` of an action (`click`, `fill`, …): its own fields, plus the
/// snapshot `--snap` took after it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Acted<T> {
    #[serde(flatten)]
    pub data: T,
    /// `--snap`: what changed on the page since the tab's last snapshot
    /// (`snapshot --diff -i`), read after the action's consequences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotData>,
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
    /// Whether the files were dropped on `element` (`--drop`), as a user
    /// drags them from the desktop, instead of going into a file input.
    #[serde(default)]
    pub drop: bool,
}

/// `data` of `chrome-cdp clipboard read --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardReadData {
    /// The tab whose page read it, e.g. `t1`.
    pub tab: String,
    /// The clipboard's text (empty when it holds none); the HTML with
    /// `--type text/html`; empty for an image.
    pub text: String,
    /// The type read: `text/plain`, `text/html` or `image/png`.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// Where an image was saved (absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The image's size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// `data` of `chrome-cdp clipboard write --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardWriteData {
    /// The tab whose page wrote it, e.g. `t1`.
    pub tab: String,
    /// How many characters of text were written (`text/plain`).
    pub chars: usize,
    /// The types of the item written, e.g. `["text/html", "text/plain"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
}

/// `data` of `chrome-cdp tabs --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TabsData {
    /// Every open tab, by id.
    pub tabs: Vec<TabInfo>,
}

/// `data` of `chrome-cdp frames --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FramesData {
    /// The tab, e.g. `t1`.
    pub tab: String,
    /// Its frames: the main frame first, parents before their children,
    /// siblings in the order of their `<iframe>` elements.
    pub frames: Vec<FrameInfo>,
    /// Its workers: the dedicated workers its frames started, and the
    /// shared and service workers of the origins its frames are on.
    pub workers: Vec<WorkerInfo>,
}

/// A frame of a tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FrameInfo {
    /// Chrome's frame id (a tab's main frame has the tab's target id).
    /// `--frame` and `frame=` take it.
    pub id: String,
    /// The parent frame's id; `None` for the main frame.
    pub parent: Option<String>,
    /// The `name` attribute of its `<iframe>`, or `""`.
    pub name: String,
    /// Its document's URL, with the fragment.
    pub url: String,
    /// 0 for the main frame, 1 for its iframes, …
    pub depth: usize,
    /// Whether it runs in another process than the main frame (an
    /// out-of-process iframe, of another site).
    pub oopif: bool,
}

/// A worker of a tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkerInfo {
    pub kind: WorkerKind,
    /// Its script.
    pub url: String,
    /// Chrome's target id.
    pub id: String,
    /// A dedicated worker's frame: the id of the frame that started it
    /// (for a worker a worker started, the frame that started the
    /// first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

/// `data` of `chrome-cdp tab <id> --json` and `chrome-cdp tab new --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TabData {
    /// The tab, now active.
    pub tab: TabInfo,
}

/// A browser context made with `chrome-cdp context new`: its own
/// cookies, storage, cache, routes, mock sets and emulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextInfo {
    /// The context's id, `c1`, `c2`, …: given in the order contexts are
    /// made and never reused within a session. `--context` takes it.
    pub id: String,
    /// Its tabs, by id.
    pub tabs: Vec<String>,
    /// The tab commands with `--context` act on when no `--tab` is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// The mock sets it uses, in the order they were applied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mocks: Vec<String>,
    /// How many routing rules it has.
    #[serde(default)]
    pub routes: usize,
}

/// `data` of `chrome-cdp context new --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextNewData {
    pub context: ContextInfo,
    /// Its first tab.
    pub tab: TabInfo,
}

/// `data` of `chrome-cdp context list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextsData {
    /// Every context made with `context new` and not closed, by id.
    pub contexts: Vec<ContextInfo>,
}

/// `data` of `chrome-cdp context close --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextCloseData {
    /// The ids of the contexts closed.
    pub closed: Vec<String>,
    /// The tabs that closed with them.
    pub tabs: Vec<String>,
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

/// `data` of `chrome-cdp expect --json`: the assertion held.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExpectData {
    /// The tab checked, e.g. `t1`; left out for `expect mock-state`,
    /// which checks the session's mocks.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tab: String,
    /// The assertion as typed after `expect`: `'role=alert' text Saved`.
    pub assertion: String,
    /// What it expected, in words: `text containing "Saved"`.
    pub expected: String,
    /// What it saw when it held: the text, value or attribute compared, a
    /// count, the state in words (`visible`), or the URL. `--no-errors`,
    /// `--no-failed-requests` and `--a11y`: 0 (none found).
    pub actual: Value,
    /// How many elements matched (element matchers), or were found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Milliseconds until it held.
    pub elapsed_ms: u64,
    /// How many times it was checked again before it held.
    pub retries: u64,
}

/// `data` of `chrome-cdp mark --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkData {
    /// The tab marked, e.g. `t1`.
    pub tab: String,
    pub name: String,
    /// The console entry the mark comes after (`console --since` takes
    /// it).
    pub console: u64,
    /// Whether it replaced a mark of the same name.
    pub replaced: bool,
}

/// What `expect-snapshot` and `expect-screenshot` did with the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BaselineResult {
    /// The page matched the baseline.
    Passed,
    /// There was no baseline; the page was stored as one.
    Created,
    /// `--update` replaced a baseline the page no longer matched.
    Updated,
    /// `--update` found the page matching the baseline already.
    Unchanged,
}

/// `data` of `chrome-cdp expect-snapshot --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExpectSnapshotData {
    /// The tab checked, e.g. `t1`.
    pub tab: String,
    /// The baseline's name, as typed.
    pub name: String,
    /// The baseline file.
    pub baseline: String,
    pub result: BaselineResult,
    /// The snapshot compared (or stored): `snapshot -i` lines without
    /// refs.
    pub snapshot: String,
    /// How many lines it has.
    pub lines: u64,
    /// `updated`: the unified diff from the old baseline to the new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// Milliseconds until it matched (or was stored).
    pub elapsed_ms: u64,
    /// How many times the page was read again before that.
    pub retries: u64,
}

/// `data` of `chrome-cdp expect-screenshot --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExpectScreenshotData {
    /// The tab checked, e.g. `t1`.
    pub tab: String,
    /// The baseline's name, as typed.
    pub name: String,
    /// The baseline file (a PNG; one per platform).
    pub baseline: String,
    pub result: BaselineResult,
    /// The image's size in pixels (CSS pixels).
    pub width: u32,
    pub height: u32,
    /// Pixels that differ from the baseline by more than `--threshold`
    /// (allowed up to `--max-diff-pixels`); `updated`: how many had
    /// changed.
    pub diff_pixels: u64,
    /// Milliseconds until it matched (or was stored).
    pub elapsed_ms: u64,
    /// How many screenshots were taken before that, less one.
    pub retries: u64,
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
    /// The action made the tab's main frame navigate (a link, a form, a
    /// script): where it is now, once loaded and settled.
    Navigated {
        /// The tab, e.g. `t1`.
        tab: String,
        /// The URL it is at now.
        url: String,
        title: String,
        /// The main document's HTTP status (absent for same-document
        /// navigations and non-HTTP pages).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        /// It stayed in the document (`pushState`, `#fragment`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        same_document: bool,
        /// It came back from the back/forward cache.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        restored: bool,
        /// Chrome's error when the page could not load
        /// (`net::ERR_CONNECTION_REFUSED`; the tab shows an error page).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Whether the load event fired before the time ran out.
        loaded: bool,
    },
    /// The page logged an error (`console.error`, a failed
    /// `console.assert`) or threw one nothing caught, after the command
    /// began.
    ConsoleError {
        /// The tab, e.g. `t1`.
        tab: String,
        /// The message's first line, e.g. `TypeError: x is undefined`.
        message: String,
        /// The script it came from, with its line and column (1-based).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        column: Option<u32>,
        /// An uncaught exception (else a `console.error` call).
        uncaught: bool,
        /// The worker it came from, if a worker's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker: Option<Worker>,
    },
    /// A request of the page to its own origin failed after the command
    /// began: an HTTP status of 400 or more, or no response.
    RequestFailed {
        /// The tab, e.g. `t1`.
        tab: String,
        /// Its id in the tab's network log (`network show <id>`), if the
        /// log has it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        method: String,
        url: String,
        /// Chrome's resource type: `Document`, `Fetch`, `XHR`, `Script`, …
        resource_type: String,
        /// The HTTP status, if the server answered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        /// Chrome's error if it did not, e.g. `net::ERR_CONNECTION_REFUSED`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Focus moved to an element of another form than before (or into a
    /// form, or out of one, or into another frame) that the action did not
    /// act on: a dialog or the next step of a flow took it.
    FocusMoved {
        /// The tab, e.g. `t1`.
        tab: String,
        element: ActedElement,
        /// The frame it is in, when that is not the tab's main frame: the
        /// frame's id (what `--frame` takes; `frames` lists them).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<String>,
    },
}

/// Where a step of `batch` or `run` is. Each `--json` line of those
/// commands is this, followed by the step command's own envelope fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StepInfo {
    /// The step's number, from 1.
    pub step: usize,
    /// How many steps the script has.
    pub total: usize,
    /// `file:line:column` of the step's line (`<stdin>` for `batch`).
    pub at: String,
    /// The command as written, without `chrome-cdp`.
    pub command: String,
}

/// A line of `batch --json` or `run --json`: a step's place and its
/// command's envelope, or (when the script does not parse) one plain error
/// envelope before any step runs.
#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)] // Only its schema is used.
enum StepLine {
    /// The step succeeded; `data` is its command's `data`.
    Ok(StepOk),
    /// The step failed.
    Err(StepErr),
    /// The script was not run: a syntax error or a bad command line.
    Invalid(ErrEnvelope),
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct StepOk {
    #[serde(flatten)]
    step: StepInfo,
    #[serde(flatten)]
    envelope: OkEnvelope<Value>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct StepErr {
    #[serde(flatten)]
    step: StepInfo,
    #[serde(flatten)]
    envelope: ErrEnvelope,
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
    fn worker_labels() {
        use super::{Worker, WorkerKind};
        let label = |kind, url: &str| {
            Worker {
                kind,
                url: url.to_owned(),
            }
            .label()
        };
        assert_eq!(
            label(WorkerKind::Service, "http://a/targets/sw.js"),
            "[service worker sw.js]"
        );
        assert_eq!(
            label(WorkerKind::Dedicated, "http://a/w.js?who=x#y"),
            "[dedicated worker w.js]"
        );
        assert_eq!(
            label(WorkerKind::Shared, "blob:http://a/"),
            "[shared worker blob:http://a/]"
        );
    }

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

/// A mock of a mock file (plan 11): where it is and what it is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockRef {
    /// Its mock set, e.g. `base` or `errors/api-down`.
    pub set: String,
    /// Its `name`, or `METHOD path` when it has none.
    pub name: String,
    /// The mock file (absolute).
    pub file: String,
    /// The line of its `[[mock]]`.
    pub line: usize,
    /// `base.toml:14`: the file relative to the mocks directory, and the
    /// line.
    pub at: String,
}

/// `data` of `chrome-cdp http --json` (plan 11, milestone 11.11): a
/// request sent from the browser context, with its cookies, outside the
/// page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HttpData {
    pub method: String,
    /// The URL (absolute).
    pub url: String,
    pub status: u16,
    /// The response headers, names in lower case; repeated ones joined
    /// with `, ` (`set-cookie` with a newline).
    pub headers: BTreeMap<String, String>,
    /// The body: its JSON value when it is JSON, else its text (`null`
    /// for an empty or binary body).
    pub body: serde_json::Value,
    /// Whether [`HttpData::body`] is the parsed JSON.
    pub json: bool,
    /// The body's size in bytes.
    pub size: usize,
    /// Who answered: `network`, `mock`, `route` or `unmatched` (a mock
    /// set's `unmatched` policy).
    pub answered_by: String,
    /// The mock that answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock: Option<MockRef>,
    /// The route rule that answered (`r2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// The mock set whose `unmatched` policy answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmatched: Option<String>,
    /// The cookies the answer set in the browser context, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cookies_set: Vec<String>,
    /// How long it took, redirects and a mock's delay included.
    pub elapsed_ms: u64,
}

/// A mock set in use (plan 11): one mock file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockSetInfo {
    /// The set's name: its file's path in the mocks directory, without
    /// `.toml`.
    pub name: String,
    /// The mock file (absolute).
    pub file: String,
    /// How many mocks it has.
    pub count: usize,
    /// How many collections it has.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub collections: usize,
    /// How many `[[websocket]]` mocks it has.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub websockets: usize,
    /// The requests it owns (`[settings] scope`), as written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<String>,
    /// What an in-scope request no mock answers gets: `fail`,
    /// `passthrough` or `abort`.
    pub unmatched: String,
    /// `auto` or `off`.
    pub cors: String,
    /// The delay its answers get, e.g. `0ms` or `50ms..200ms`.
    pub latency: String,
    /// Why the file's latest change was not loaded; the set keeps its
    /// previous version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload_error: Option<String>,
    /// Its mocks, in the order they are tried (`mock list`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mocks: Vec<MockInfo>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

/// A mock, as `mock list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockInfo {
    #[serde(flatten)]
    pub mock: MockRef,
    /// What it matches: `GET /api/products/:id`.
    pub matches: String,
    /// What it answers: `json_file data/products/{{params.id}}.json`.
    pub responds: String,
    /// How many requests it answered.
    pub hits: u64,
}

/// `data` of `chrome-cdp mock use|list|reset --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockSetsData {
    /// The sets in use, in the order they are tried (the last applied
    /// first).
    pub sets: Vec<MockSetInfo>,
    /// With `mock list --available`: the sets in the mocks directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<Vec<String>>,
    /// The mocks directory the sets were looked for in (absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

/// `data` of `chrome-cdp history --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryData {
    /// The commands, oldest first.
    pub entries: Vec<HistoryEntry>,
    /// How many older commands the history no longer has.
    pub dropped: u64,
    /// With `--as-scenario`: the test, as `.cdp` text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    /// With `--as-scenario`: the commands left out of it, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<String>,
}

/// One command a session ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEntry {
    /// Its number in the session's history, from 1.
    pub n: u64,
    /// Its words, as typed (without the program name).
    pub words: Vec<String>,
    /// `--context`, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Whether it succeeded.
    pub ok: bool,
    /// The error's kind, if it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The refs it named, with their stable locators when it ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<RefLocator>,
}

/// A ref and the most robust locator that found its element alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RefLocator {
    /// `e12`.
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    /// How: `role`, `label`, `testid`, `placeholder`, `text`, `scoped` (inside a
    /// unique container), `has` (inside the container that has a text) or
    /// `nth` (by position; the least robust).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
    /// Why there is no locator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `data` of `chrome-cdp fmt --json` (plan 11, milestone 11.12); with
/// `--check`, also `details` of its error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FmtData {
    /// How many files were looked at.
    pub files: usize,
    /// The files formatted (absolute); with `--check`, those that are not
    /// formatted.
    pub changed: Vec<String>,
    /// Whether it ran with `--check` (nothing was written).
    pub check: bool,
    /// With `--check`: the unified diff of the changes it would make.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

/// `data` of `chrome-cdp lint --json` (plan 11, milestone 11.12), and
/// `details` of its error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LintData {
    /// The files checked (absolute): scenarios, step files and mock files.
    pub files: Vec<String>,
    pub errors: usize,
    pub warnings: usize,
    /// By file, line and column.
    pub problems: Vec<LintProblem>,
}

/// One problem `lint` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LintProblem {
    /// The file (absolute).
    pub file: String,
    /// 1-based; 0 when the problem is the whole file's.
    pub line: usize,
    pub col: usize,
    /// `error` or `warning`.
    pub severity: String,
    /// The rule, e.g. `unknown-command`, `ref`, `sleep`, `unreachable`
    /// (`chrome-cdp lint --help` lists them).
    pub rule: String,
    pub message: String,
    /// What to write instead, or the command that helps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// `data` of `chrome-cdp mock check --json` (and `details` of its
/// error).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockCheckData {
    /// The mock files checked (absolute).
    pub files: Vec<String>,
    /// The mocks and collections in them.
    pub mocks: usize,
    /// The OpenAPI spec they were checked against (absolute), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openapi: Option<String>,
    pub errors: usize,
    pub warnings: usize,
    /// Errors first, then warnings, each by file and line.
    pub problems: Vec<MockProblem>,
}

/// One problem `mock check` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockProblem {
    /// The mock file (absolute).
    pub file: String,
    /// 1-based; 0 when the problem is the whole file's.
    pub line: usize,
    pub col: usize,
    /// `error` or `warning`.
    pub severity: String,
    /// `load`, `unreachable`, `collection-shadowed`, `duplicate-collection`,
    /// `undeclared-state`, `openapi-path`, `openapi-method`, `openapi-status`
    /// or `openapi-body`.
    pub rule: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// `data` of `chrome-cdp mock serve --json` (plan 11, milestone 11.9),
/// the last line, once the server stopped; each request it answered is a
/// line before it, as `mock log --json` has them (plus `test`, the
/// request's `x-chrome-cdp-test` header).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockServeData {
    /// Where it listened, e.g. `http://127.0.0.1:4010`.
    pub url: String,
    /// The mock sets it served, in the order they are tried.
    pub sets: Vec<String>,
    /// How many requests it answered.
    pub requests: u64,
}

/// A page's WebSocket that a `[[websocket]]` mock answers (plan 11,
/// milestone 11.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockSocketInfo {
    /// Its tab, e.g. `t1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// Its URL.
    pub url: String,
    /// The mock that answers it.
    pub mock: MockRef,
}

/// `data` of `chrome-cdp mock ws send --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockWsSendData {
    /// The URL glob given.
    pub url: String,
    /// The message's size in bytes.
    pub size: usize,
    /// The sockets that got it.
    pub sockets: Vec<MockSocketInfo>,
}

/// `data` of `chrome-cdp mock record|import --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockRecordData {
    /// The mock file written (absolute).
    pub path: String,
    /// What `mock use` takes: its name in the mocks directory, else its
    /// path.
    pub set: String,
    /// The requests in scope that were recorded.
    pub requests: usize,
    /// The `[[mock]]`s written (one per distinct request).
    pub mocks: usize,
    /// The data files written (absolute).
    pub files: Vec<String>,
    /// The set's `scope` (as given, or guessed; empty if neither).
    pub scope: Vec<String>,
    /// Requests in scope left out, and why.
    pub skipped: Vec<String>,
}

/// `data` of `chrome-cdp mock off --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockOffData {
    /// The sets turned off.
    pub removed: Vec<String>,
    /// The sets still in use, in the order they are tried.
    pub sets: Vec<MockSetInfo>,
}

/// `data` of `chrome-cdp mock log --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockLogData {
    /// The requests, oldest first.
    pub log: Vec<RoutedRequest>,
    /// Older requests `--limit` left out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped: u64,
}

/// One mock's verdict in `mock explain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockCheck {
    #[serde(flatten)]
    pub mock: MockRef,
    /// Whether it matches the request.
    pub matches: bool,
    /// Why it does not match (the first condition that fails), or why a
    /// match does not answer (an earlier mock does).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `data` of `chrome-cdp mock explain --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockExplainData {
    pub method: String,
    /// The request's URL, made absolute.
    pub url: String,
    /// The routing rule that answers it before any mock (`route`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// The mock that answers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<MockRef>,
    /// What the answer is: `fulfill 200 json_file data/products/7.json`,
    /// or for an unmatched request what its set's policy does.
    pub outcome: String,
    /// The values the answering mock's path captured (`:id`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// The set whose scope the request is in; none if it is in no scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Every mock, in the order they are tried.
    pub checked: Vec<MockCheck>,
}

/// A collection of a mock set in use (plan 11.3): a fake REST resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockCollectionInfo {
    /// Its set, name, file and line.
    #[serde(flatten)]
    pub mock: MockRef,
    /// Its list's path, e.g. `/api/todos`.
    pub path: String,
    /// How many items it has now.
    pub count: usize,
}

/// `data` of `chrome-cdp mock state --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockStateData {
    /// With a NAME: the named value or collection shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// With a NAME: its value, or the collection's items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// With a collection's NAME: the collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<MockCollectionInfo>,
    /// Without a NAME: every named value (`[state]` and `set_state`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Value>,
    /// Without a NAME: every collection, in the order they are tried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<MockCollectionInfo>,
    /// The values `--set` changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
}

/// `data` of `chrome-cdp mock seed --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockSeedData {
    /// The collection, with its new count.
    pub collection: MockCollectionInfo,
    /// Where the items came from: a data file (absolute), `stdin`, or
    /// `seed` (its starting items again).
    pub from: String,
}

/// `data` of `chrome-cdp test --json` (and `details` of its error when a
/// test failed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestRunData {
    /// The tests of the run, in the order they were selected.
    pub tests: Vec<TestResult>,
    pub passed: usize,
    pub failed: usize,
    /// Failed, then passed on a retry: they count as passing, and are
    /// always reported.
    pub flaky: usize,
    pub skipped: usize,
    /// Not run: `--bail` stopped the run, or it was interrupted.
    pub not_run: usize,
    /// How long the run took (0 for `--list`).
    pub duration_ms: u64,
    /// `--shard`, as given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shard: Option<String>,
    /// `--list`: the tests were listed, not run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub listed: bool,
    /// `--repeat`: how many times each test ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// The files `--reporter` wrote (absolute), by reporter: `json`,
    /// `junit`, `html`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reporters: BTreeMap<String, String>,
    /// `--debug`: the failed test's browser context, left open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<TestDebug>,
    /// The backend the tests ran against, unless it was the mock one:
    /// a real backend's name, or `all` (plan 11.11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// `--backend all`: each backend's results, in the order they ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backends: Vec<TestBackendRun>,
    /// `--verify-mocks`: how the mocks' answers differ from the real
    /// backend's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyData>,
    /// `--record-mocks`: the mock set written for each passed test.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recorded: Vec<MockRecordData>,
}

/// What `chrome-cdp test --verify-mocks` found: every real answer to a
/// request in a mock set's scope, compared with the answer the mocks
/// would have given (status and JSON shape, not values).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerifyData {
    /// The real backend.
    pub backend: String,
    /// How many real answers were compared.
    pub compared: usize,
    /// How many mocks answered at least one of them.
    pub mocks: usize,
    /// The mocks whose answers differ, in the order first seen.
    pub drifted: Vec<MockDrift>,
    /// Real requests in a mock set's scope that no mock answers.
    pub unmocked: Vec<UnmockedRequest>,
    /// `--update`: the data files rewritten (absolute).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updated: Vec<String>,
}

/// A mock whose answer differs from the real one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MockDrift {
    pub mock: MockRef,
    /// Its JSON data file (absolute), if its body is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_file: Option<String>,
    /// The first request it differed on: `GET http://…/api/products`.
    pub request: String,
    /// What differs: `status: …`, or a JSON path and how.
    pub problems: Vec<String>,
    /// `--update` rewrote its data file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub updated: bool,
}

/// A real request in a mock set's scope that no mock answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnmockedRequest {
    pub method: String,
    pub url: String,
    /// The real answer's status.
    pub status: u16,
    /// The mock set whose scope has it.
    pub set: String,
    /// How many times it was seen.
    pub count: usize,
}

/// One backend's results in `chrome-cdp test --backend all`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TestBackendRun {
    pub name: String,
    /// `mock` or `real`.
    pub mode: String,
    pub passed: usize,
    pub failed: usize,
    pub flaky: usize,
    pub skipped: usize,
    pub not_run: usize,
    pub duration_ms: u64,
}

/// The browser context `chrome-cdp test --debug` left open after the
/// first failure, with its mocks still in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TestDebug {
    /// The session it is in (`--session`), e.g. `test-debug`.
    pub session: String,
    /// The context (`--context`), e.g. `c3`.
    pub context: String,
    /// The test, as `file:line`.
    pub test: String,
}

/// One test of `chrome-cdp test`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestResult {
    /// The scenario file, relative to the current directory.
    pub file: String,
    /// The line of its `@test`; `file:line` runs it alone.
    pub line: u32,
    pub name: String,
    /// `passed`, `failed`, `flaky`, `skipped`, `not-run`, or `listed`
    /// (`--list`).
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// How long its last try took (with `--repeat`, its slowest run).
    #[serde(default)]
    pub duration_ms: u64,
    /// How many times it ran (1 without retries; with `--repeat`, the
    /// tries of all its runs).
    #[serde(default)]
    pub tries: u32,
    /// `--repeat`: the runs that failed. A test fails when one of its runs
    /// does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_runs: Option<u32>,
    /// What its steps warned about, e.g. a screenshot baseline made on
    /// another OS only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Why it was skipped (`@skip`) or not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Its artifacts' directory (absolute): `steps.jsonl`,
    /// `requests.txt`, `console.txt`, and for a failure `report.md`,
    /// `failure.png` and `snapshot.txt`. With retries, its last try's;
    /// with `--repeat`, its first failed (else first) run's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<String>,
    /// Why it failed (its last try; a flaky test's first failure; with
    /// `--repeat`, its first failed run's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TestFailure>,
    /// `--backend all`: the backend of this run of the test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
}

/// The step a test failed at, and its error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestFailure {
    /// `file:line:col` of the step (in a step file for an `@include`d
    /// one).
    pub at: String,
    /// The step as written.
    pub step: String,
    /// `before`, `after`, `before-all` or `after-all` for a hook's step;
    /// `setup` for what the runner does before the steps, `check` for its
    /// checks after them (`expect --no-errors --all-tabs`); absent for
    /// the test's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<String>,
    /// The `@include` lines the step came through, outermost first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<String>,
    /// The step's error.
    pub error: ErrorInfo,
    /// The failure's `report.md` (absolute): the step, the error, the page
    /// around it, console errors and failed requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}
