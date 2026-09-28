//! Golden tests of every command's text, `--json` and `-q` output, errors
//! and hints included (plan 08, milestone 8.1). The wording is part of the
//! UX, so a change shows up in review as a diff of `tests/golden/`.
//!
//! Each case runs the binary once per output mode and compares the result
//! with `tests/golden/<case>.txt`, after replacing machine-specific parts
//! (version, git sha, Chrome path, temp dirs) with placeholders. Every
//! registered JSON Schema is also kept in `tests/golden/schema/`.
//!
//! To accept new output: `UPDATE_GOLDEN=1 cargo test -p chrome-cdp --test
//! golden` (with `CHROME_PATH` set, or the Chrome cases are not written).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Sandbox, chrome_path, cli, fake_chrome};
use fixtures::Fixtures;
use serde_json::Value;

/// What a case needs before it runs.
#[derive(Clone, Copy, PartialEq)]
enum Fixture {
    None,
    /// A session sandbox whose sessions launch the real Chrome, and whose
    /// `default` session is opened before the case runs.
    Session,
    /// A session sandbox with the real Chrome, closed after each mode so
    /// each starts from nothing.
    FreshSession,
    /// Like `FreshSession`, but `default` is opened before each mode.
    SessionPerMode,
    /// Like `Session`, with the fixture site's page at this path loaded.
    SessionAt(&'static str),
    /// A session sandbox with the real Chrome, after these commands
    /// (`{site}` is the fixture site's origin). The first one opens it.
    SessionAfter(&'static [&'static [&'static str]]),
    /// An empty session sandbox, with no Chrome.
    NoSessions,
    /// `{chrome}` is a stand-in that dies like a Chrome without a sandbox.
    SandboxlessChrome,
    /// `{chrome}` is a stand-in that never answers.
    SilentChrome,
    /// `{chrome}` is a stand-in older than the oldest supported Chrome
    /// that dies at startup.
    OldChrome,
    /// `CHROME_CDP_CHROME` names a file that does not exist.
    MissingChromeEnv,
    /// Runs in a directory whose `.chrome-cdp.toml` has a typo.
    BadConfig,
    /// A fake Chrome for Testing index (`<cft>`) and an empty cache in the
    /// temp dir. Linux x86_64 only: it serves `linux64` builds.
    FakeCft,
}

struct Case {
    name: &'static str,
    args: &'static [&'static str],
    /// Extra flags per run: "" is the text form.
    modes: &'static [&'static str],
    fixture: Fixture,
}

const ALL: &[&str] = &["", "--json", "-q"];
const ERR: &[&str] = &["", "--json"];

/// The re-render fixture after a snapshot and a remount: every ref of the
/// snapshot names a node that is gone.
const REMOUNTED: &[&[&str]] = &[
    &["open", "{site}/refs/rerender.html"],
    &["snapshot"],
    &[
        "cdp",
        "--target",
        "page",
        "Runtime.evaluate",
        r#"{"expression": "remount(TODOS)"}"#,
    ],
];

const CASES: &[Case] = &[
    Case {
        name: "version",
        args: &["-V"],
        modes: ALL,
        fixture: Fixture::None,
    },
    Case {
        name: "no-command",
        args: &[],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "unknown-flag",
        args: &["--bogus"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "mistyped-command",
        args: &["scheme"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "mistyped-flag",
        args: &["cdp", "Browser.getVersion", "--jsn"],
        modes: &[""],
        fixture: Fixture::None,
    },
    Case {
        name: "duration-without-unit",
        args: &["--timeout", "5000", "-V"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "config-unknown-key",
        args: &["-V"],
        modes: ERR,
        fixture: Fixture::BadConfig,
    },
    Case {
        name: "schema-list",
        args: &["schema"],
        modes: ALL,
        fixture: Fixture::None,
    },
    Case {
        name: "schema-unknown",
        args: &["schema", "nope"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "install",
        args: &["install"],
        modes: ALL,
        fixture: Fixture::FakeCft,
    },
    Case {
        name: "install-list",
        args: &["install", "--list"],
        modes: ALL,
        fixture: Fixture::FakeCft,
    },
    Case {
        name: "install-unknown-version",
        args: &["install", "--version", "99"],
        modes: ERR,
        fixture: Fixture::FakeCft,
    },
    Case {
        name: "install-list-conflicts",
        args: &["install", "--list", "--force"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "uninstall-missing",
        args: &["uninstall", "stable-1.0.0.0-linux64"],
        modes: ERR,
        fixture: Fixture::FakeCft,
    },
    Case {
        name: "cdp-bad-method",
        args: &["cdp", "getVersion"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "cdp-bad-params",
        args: &["cdp", "Runtime.evaluate", "{expression:1}"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "cdp-params-not-object",
        args: &["cdp", "Runtime.evaluate", "[1]"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "open-chrome-env-missing",
        args: &["open"],
        modes: ERR,
        fixture: Fixture::MissingChromeEnv,
    },
    Case {
        name: "open-old-chrome",
        args: &["open", "--chrome", "{chrome}"],
        modes: ERR,
        fixture: Fixture::OldChrome,
    },
    Case {
        name: "open-chrome-missing",
        args: &["open", "--chrome", "/nonexistent/chrome"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "open-no-sandbox",
        args: &["open", "--chrome", "{chrome}"],
        modes: ERR,
        fixture: Fixture::SandboxlessChrome,
    },
    Case {
        name: "open-startup-timeout",
        args: &["open", "--chrome", "{chrome}", "--timeout", "300ms"],
        modes: ERR,
        fixture: Fixture::SilentChrome,
    },
    Case {
        name: "open",
        args: &["open", "data:text/html,<title>Hello</title>"],
        modes: ALL,
        fixture: Fixture::FreshSession,
    },
    Case {
        name: "open-again",
        args: &["open"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "open-other-options",
        args: &["open", "--headed"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "open-bad-session-name",
        args: &["open", "--session", "a/b"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "connect-bad-endpoint",
        args: &["connect", "wss://example.com/devtools"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "status",
        args: &["status"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "status-stopped",
        args: &["status"],
        modes: ALL,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "sessions",
        // The text table's widths depend on the pid.
        args: &["sessions"],
        modes: &["--json", "-q"],
        fixture: Fixture::Session,
    },
    Case {
        name: "sessions-none",
        args: &["sessions"],
        modes: ALL,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "close",
        args: &["close"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "close-not-running",
        args: &["close"],
        modes: ALL,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "cdp-evaluate",
        args: &[
            "cdp",
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"6*7","returnByValue":true}"#,
        ],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "cdp-truncated",
        args: &[
            "cdp",
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"'ab'.repeat(100)","returnByValue":true}"#,
            "--max-chars",
            "100",
        ],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "cdp-page-method-on-browser",
        args: &["cdp", "Runtime.evaluate", r#"{"expression":"1"}"#],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "tabs",
        args: &["tabs"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "tabs-not-running",
        args: &["tabs"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "tab-select",
        args: &["tab", "t1"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "tab-unknown",
        args: &["tab", "t9"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "tab-bad-id",
        args: &["tab", "first"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "tab-new",
        args: &["tab", "new", "data:text/html,<title>Two</title>"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "tab-close-last",
        args: &["tab", "close"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "tab-close-browser",
        args: &["tab", "close", "--close-browser"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "goto",
        args: &["goto", "{site}/redirect?to=/nav/a.html"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "goto-404",
        args: &["goto", "{site}/nav/missing.html"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "goto-connection-refused",
        // Nothing listens on port 47, and Chrome does not block it.
        args: &["goto", "localhost:47/login"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "goto-relative-on-blank",
        args: &["goto", "/login"],
        modes: ERR,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "goto-load-timeout",
        args: &[
            "goto",
            "{site}/nav/slow-image.html?img_delay=20000",
            "--timeout",
            "1s",
        ],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "back-at-start",
        args: &["back"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "reload",
        args: &["reload"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "forward-not-running",
        args: &["forward"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "wait-url",
        args: &["wait", "--url", "blank"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "wait-fn",
        args: &["wait", "--fn", "6 * 7"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "wait-element-timeout",
        args: &["wait", "#results", "--timeout", "500ms"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "wait-text-timeout",
        args: &["wait", "--text", "Welcome back", "--timeout", "500ms"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "wait-delay",
        args: &["wait", "100"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "wait-bad-locator",
        args: &["wait", "role=button[nme=Save]"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "wait-not-running",
        args: &["wait", "--idle"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "wait-download-none",
        args: &["wait", "--download", "--timeout", "500ms"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "snapshot",
        args: &["snapshot"],
        modes: ALL,
        fixture: Fixture::SessionAt("/snapshot/login.html"),
    },
    Case {
        name: "snapshot-scope-interactive",
        args: &["snapshot", "--scope", "css=form", "-i"],
        modes: ALL,
        fixture: Fixture::SessionAt("/snapshot/login.html"),
    },
    Case {
        name: "snapshot-scope-ambiguous",
        args: &["snapshot", "--scope", "text=Delete"],
        modes: ERR,
        fixture: Fixture::SessionAt("/locators/index.html"),
    },
    Case {
        name: "snapshot-scope-locator",
        args: &["snapshot", "--scope", "role=row >> has-text=Invoice 13"],
        modes: ALL,
        fixture: Fixture::SessionAt("/locators/index.html"),
    },
    Case {
        name: "snapshot-scope-frame",
        args: &["snapshot", "--scope", "frame=payment"],
        modes: ALL,
        fixture: Fixture::SessionAt("/locators/index.html"),
    },
    Case {
        name: "wait-role",
        args: &["wait", "role=button[name=\"Sign in\"]"],
        modes: ALL,
        fixture: Fixture::SessionAt("/locators/index.html"),
    },
    Case {
        name: "snapshot-scope-missing",
        args: &["snapshot", "--scope", "#nothing"],
        modes: ERR,
        fixture: Fixture::SessionAt("/snapshot/login.html"),
    },
    Case {
        name: "snapshot-scope-stale-ref",
        args: &["snapshot", "--scope", "e1"],
        modes: ERR,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/refs/rerender.html"],
            &["snapshot"],
            &["goto", "{site}/snapshot/simple.html"],
        ]),
    },
    Case {
        name: "snapshot-scope-detached-ref",
        args: &["snapshot", "--scope", "e1", "--strict-refs"],
        modes: ERR,
        fixture: Fixture::SessionAfter(REMOUNTED),
    },
    Case {
        name: "snapshot-scope-ambiguous-ref",
        args: &["snapshot", "--scope", "e3"],
        modes: ERR,
        fixture: Fixture::SessionAfter(REMOUNTED),
    },
    Case {
        name: "snapshot-bad-scope",
        args: &["snapshot", "--scope", "Sign in"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "snapshot-not-running",
        args: &["snapshot"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "find",
        args: &["find", "checkout"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "find-nothing",
        args: &["find", "zebra", "--role", "button"],
        modes: ERR,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "text-matches",
        args: &["text", ".item"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "html",
        args: &["html", "#checkout", "--outer"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "html-ambiguous",
        args: &["html", ".item"],
        modes: ERR,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "attr-missing",
        args: &["attr", "#pricing", "title"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "attr-no-name",
        args: &["attr", "#pricing"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "value",
        args: &["value", "#extras"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "value-not-a-field",
        args: &["value", "#intro"],
        modes: ERR,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "count",
        args: &["count", ".item"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "box-in-frame",
        args: &["box", "frame=same >> #target"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "box-not-rendered",
        args: &["box", "#hidden-text"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "markdown",
        args: &["markdown"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/article.html"),
    },
    Case {
        name: "count-no-locator",
        args: &["count"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "goto-dialog-open",
        args: &[
            "goto",
            "{site}/dialogs/on-load.html?kind=confirm&message=Delete%20item%3F",
        ],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "goto-alert",
        args: &[
            "goto",
            "{site}/dialogs/on-load.html?kind=alert&message=Saved",
        ],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "dialog-none-open",
        args: &["dialog", "accept"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "dialog-not-running",
        args: &["dialog", "dismiss"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "goto-download",
        args: &["goto", "{site}/downloads/report.txt?download=report.txt"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "downloads-none",
        args: &["downloads"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "console",
        args: &["console", "--since", "0"],
        modes: ALL,
        fixture: Fixture::SessionAt("/console/index.html"),
    },
    Case {
        name: "console-none",
        args: &["console"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "console-bad-since",
        args: &["console", "--since", "yesterday"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "console-bad-grep",
        args: &["console", "--grep", "fetch("],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "errors",
        args: &["errors", "--since", "0"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/console/index.html"],
            &["click", "#sync"],
        ]),
    },
    Case {
        name: "network-failed",
        args: &["network", "--failed", "--status", "4xx", "--since", "0"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/network/index.html"],
            &["click", "#missing"],
        ]),
    },
    Case {
        name: "network-none",
        args: &["network"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "network-bad-status",
        args: &["network", "--status", "4x"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "network-show-missing",
        args: &["network", "show", "#999"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "screenshot-element",
        args: &["screenshot", "#red"],
        modes: ALL,
        fixture: Fixture::SessionAt("/shots/index.html"),
    },
    Case {
        name: "screenshot-quality-png",
        args: &["screenshot", "--quality", "80"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "eval",
        args: &[
            "eval",
            "({title: document.title, items: document.querySelectorAll('.item').length})",
        ],
        modes: ALL,
        fixture: Fixture::SessionAt("/eval/index.html"),
    },
    Case {
        name: "eval-in",
        args: &["eval", "--in", "#name", "el.value"],
        modes: ALL,
        fixture: Fixture::SessionAt("/eval/index.html"),
    },
    Case {
        name: "eval-threw",
        args: &["eval", "explode()"],
        modes: ERR,
        fixture: Fixture::SessionAt("/eval/index.html"),
    },
    Case {
        name: "eval-syntax",
        args: &["eval", "let x = ;"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "eval-in-and-frame",
        args: &["eval", "--in", "e1", "--frame", "inner", "el"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "cdp-listen-none",
        args: &[
            "cdp",
            "--target",
            "page",
            "--listen",
            "Animation.animationCreated",
            "--timeout",
            "300ms",
            "Runtime.evaluate",
            r#"{"expression":"1"}"#,
        ],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "cdp-listen-bad-event",
        args: &["cdp", "--listen", "loadEventFired"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "pdf-bad-margin",
        args: &["pdf", "--margin", "1cm,2"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "har-start",
        args: &["har", "start", "--content", "omit"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "har-stop-not-recording",
        args: &["har", "stop"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "trace-start",
        args: &["trace", "start", "--categories", "-*,devtools.timeline"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "trace-stop-not-recording",
        args: &["trace", "stop"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "goto-auth-required",
        args: &["goto", "{site}/auth/"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "click",
        args: &["click", "#go"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/mouse.html"),
    },
    Case {
        name: "click-right-with-modifiers",
        args: &[
            "click",
            "#go",
            "--button",
            "right",
            "--modifiers",
            "Shift,Meta",
        ],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/mouse.html"),
    },
    Case {
        name: "dblclick",
        args: &["dblclick", "#go"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/mouse.html"),
    },
    Case {
        name: "hover",
        args: &["hover", "#menu"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/mouse.html"),
    },
    Case {
        name: "click-obscured",
        args: &["click", "#under", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/obscured.html"),
    },
    Case {
        name: "click-disabled",
        args: &["click", "#off", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/mouse.html"),
    },
    Case {
        name: "click-hidden",
        args: &["click", "#hidden", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/states.html"),
    },
    Case {
        name: "click-moving",
        args: &["click", "#forever", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/moving.html"),
    },
    Case {
        name: "click-missing",
        args: &["click", "#nothing", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/mouse.html"),
    },
    Case {
        name: "click-count-zero",
        args: &["click", "#go", "--count", "0"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "click-bad-modifier",
        args: &["click", "#go", "--modifiers", "Hyper"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "press",
        args: &["press", "Enter", "--in", "#field"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/keyboard.html"),
    },
    Case {
        name: "press-unknown-key",
        args: &["press", "Control+Nope"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "press-not-focusable",
        args: &["press", "Enter", "--in", "#plain"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/keyboard.html"),
    },
    Case {
        name: "type",
        args: &["type", "hello", "--in", "#field"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/keyboard.html"),
    },
    Case {
        name: "fill",
        args: &["fill", "#email", "ada@example.com"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "fill-readonly",
        args: &["fill", "#locked", "x", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/keyboard.html"),
    },
    Case {
        name: "fill-not-text",
        args: &["fill", "#plain", "x"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/keyboard.html"),
    },
    Case {
        name: "fill-bad-date",
        args: &["fill", "#birthday", "tomorrow"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "fill-masked",
        args: &["fill", "#code", "123456"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "select",
        args: &["select", "#choice", "Green"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "select-missing",
        args: &["select", "#choice", "purple", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "select-custom-list",
        args: &["select", "#combo", "Apple"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "check-by-label",
        args: &["check", "#offers"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "uncheck-radio",
        args: &["uncheck", "#standard"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "check-ignored",
        args: &["check", "#stubborn", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/form.html"),
    },
    Case {
        name: "focus",
        args: &["focus", "#btn"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/keyboard.html"),
    },
    Case {
        name: "blur-not-focused",
        args: &["blur", "#btn"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/keyboard.html"),
    },
    Case {
        name: "upload",
        args: &["upload", "#avatar", "{files}/hello.txt"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/files.html"),
    },
    Case {
        name: "upload-chooser",
        args: &["upload", "#choose", "{files}/hello.txt", "{files}/notes.md"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/files.html"),
    },
    Case {
        name: "upload-one-file",
        args: &[
            "upload",
            "#choose-one",
            "{files}/hello.txt",
            "{files}/notes.md",
        ],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/files.html"),
    },
    Case {
        name: "upload-no-chooser",
        args: &["upload", "#nothing", "{files}/hello.txt"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/files.html"),
    },
    Case {
        name: "upload-missing-file",
        args: &["upload", "#avatar", "{files}/missing.txt"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "clipboard-write",
        args: &["clipboard", "write", "hello"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/clipboard.html"),
    },
    Case {
        name: "clipboard-read",
        args: &["clipboard", "read"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/actions/clipboard.html"],
            &["click", "#copy"],
        ]),
    },
    Case {
        name: "clipboard-no-origin",
        args: &["clipboard", "read"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "tap-without-touch",
        args: &["tap", "#tap"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/touch.html"),
    },
    Case {
        name: "mouse-move",
        args: &["mouse", "move", "10", "20.5"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "mouse-wheel",
        args: &["mouse", "wheel", "0", "-100"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "drag",
        args: &["drag", "#card1", "#done"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/dnd.html"),
    },
    Case {
        name: "drag-pointer",
        args: &["drag", "#d", "#a"],
        modes: &[""],
        fixture: Fixture::SessionAt("/actions/sortable.html"),
    },
    Case {
        name: "scroll-by",
        args: &["scroll", "--by", "0,300"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/feed.html"),
    },
    Case {
        name: "scroll-into-view",
        args: &["scroll", "text=Mail 12"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/feed.html"),
    },
    Case {
        name: "click-report-failed-request",
        args: &["click", "#save"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/report.html"),
    },
    Case {
        name: "click-report-console-error",
        args: &["click", "#throw"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/report.html"),
    },
    Case {
        name: "click-report-same-document",
        args: &["click", "#push"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/report.html"),
    },
    Case {
        name: "click-report-navigates",
        args: &["click", "#next"],
        modes: &[""],
        fixture: Fixture::SessionAt("/actions/report.html"),
    },
    Case {
        name: "click-snap",
        args: &["click", "#nothing", "--snap"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[&["open", "{site}/actions/report.html"], &["snapshot"]]),
    },
    Case {
        name: "scroll-nothing",
        args: &["scroll"],
        modes: ERR,
        fixture: Fixture::None,
    },
];

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Compares `actual` with the golden file, or rewrites it when updating.
fn check(path: &Path, actual: &str, failures: &mut Vec<String>) {
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, actual).unwrap();
        return;
    }
    match std::fs::read_to_string(path) {
        Ok(expected) if expected == actual => {}
        Ok(expected) => {
            eprintln!(
                "--- {} (expected)\n{expected}\n--- (actual)\n{actual}",
                path.display()
            );
            failures.push(path.display().to_string());
        }
        Err(e) => failures.push(format!("{}: {e}", path.display())),
    }
}

fn finish(failures: &[String]) {
    assert!(
        failures.is_empty(),
        "golden output differs (UPDATE_GOLDEN=1 accepts it): {failures:#?}"
    );
}

/// Machine-specific strings and their placeholders, longest first so a
/// path is replaced before a prefix of it.
struct Normalizer(Vec<(String, &'static str)>);

impl Normalizer {
    fn new() -> Self {
        let out = cli().args(["-V", "--json"]).output().unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let sha = v["data"]["git_sha"].as_str().unwrap().to_owned();
        Self(vec![
            (sha, "<sha>"),
            (env!("CARGO_PKG_VERSION").to_owned(), "<version>"),
        ])
    }

    fn with(&self, from: &Path, to: &'static str) -> Self {
        let mut pairs = self.0.clone();
        pairs.push((from.display().to_string(), to));
        pairs.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        Self(pairs)
    }

    fn apply(&self, text: &str) -> String {
        let text = self
            .0
            .iter()
            .fold(text.to_owned(), |text, (from, to)| text.replace(from, to));
        // What changes from run to run: pids, uptimes, the random part of
        // Chrome's run dir, and the version of the Chrome under test.
        let digits = |c: char| c.is_ascii_digit();
        let uptime = |c: char| c.is_ascii_digit() || matches!(c, 'h' | 'm' | 's');
        let version = |c: char| c.is_ascii_digit() || c == '.';
        [
            ("pid ", &digits as &dyn Fn(char) -> bool, "<pid>"),
            ("\"pid\":", &digits, "<pid>"),
            ("\"daemon_pid\":", &digits, "<pid>"),
            ("\"uptime_ms\":", &digits, "<ms>"),
            ("\"elapsed_ms\":", &digits, "<ms>"),
            ("\"time_ms\":", &digits, "<ms>"),
            // A navigation's or wait's time: `"Title" (412ms)`.
            (
                " (",
                &|c: char| c.is_ascii_digit() || matches!(c, '.' | 'm' | 's'),
                "<ms>",
            ),
            ("up ", &uptime, "<uptime>"),
            (
                "/chrome-cdp-",
                &|c: char| c.is_ascii_alphanumeric(),
                "<rand>",
            ),
            ("Chrome/", &version, "<version>"),
        ]
        .iter()
        .fold(text, |text, (prefix, keep, to)| {
            mask(&text, prefix, keep, to)
        })
    }
}

/// Replaces the run of chars matching `keep` after each `prefix` with
/// `to`, when that run starts with a digit (or letter, for `<rand>`).
fn mask(text: &str, prefix: &str, keep: &dyn Fn(char) -> bool, to: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(prefix) {
        let after = at + prefix.len();
        out.push_str(&rest[..after]);
        rest = &rest[after..];
        let end = rest.find(|c| !keep(c)).unwrap_or(rest.len());
        let starts_right = rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || to == "<rand>");
        // ` (412ms)` is a time, ` (27B)` and `report (1).txt` are not.
        let ends_right = prefix != " (" || rest[..end].ends_with('s');
        if end > 0 && starts_right && ends_right {
            out.push_str(to);
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

/// Runs every mode of `case`; `None` if it needs a Chrome we do not have.
fn run_case(case: &Case, base: &Normalizer) -> Option<String> {
    let dir = tempfile::tempdir().unwrap();
    let mut norm = base.with(dir.path(), "<tmp>");
    let mut cwd = None;
    let mut env: Vec<(&str, String)> = Vec::new();
    let mut _server = None;
    let mut sandbox = None;
    let chrome = match case.fixture {
        Fixture::None => None,
        Fixture::Session
        | Fixture::FreshSession
        | Fixture::SessionPerMode
        | Fixture::SessionAt(_)
        | Fixture::SessionAfter(_) => {
            let chrome = chrome_path()?;
            norm = norm.with(&chrome, "<chrome>");
            let sb = Sandbox::with_chrome(&chrome);
            norm = norm.with(sb.dir.path(), "<sandbox>");
            if case.fixture == Fixture::Session {
                let out = sb.cli().arg("open").output().unwrap();
                assert!(out.status.success(), "{out:?}");
            }
            if let Fixture::SessionAt(path) = case.fixture {
                let url = format!("{}{path}", Fixtures::shared().a());
                let out = sb.cli().args(["open", &url]).output().unwrap();
                assert!(out.status.success(), "{out:?}");
            }
            if let Fixture::SessionAfter(steps) = case.fixture {
                let site = Fixtures::shared().a();
                for step in steps {
                    let args: Vec<String> =
                        step.iter().map(|a| a.replace("{site}", site)).collect();
                    let out = sb.cli().args(&args).output().unwrap();
                    assert!(out.status.success(), "{step:?}: {out:?}");
                }
            }
            sandbox = Some(sb);
            Some(chrome)
        }
        Fixture::NoSessions => {
            let sb = Sandbox::new();
            norm = norm.with(sb.dir.path(), "<sandbox>");
            sandbox = Some(sb);
            None
        }
        Fixture::SandboxlessChrome => Some(fake_chrome(
            &dir,
            "echo '[1:1:0927/101010.1:FATAL:zygote_host_impl_linux.cc(128)] No usable sandbox! \
             If you are running on Ubuntu 23.10+ ...' >&2\nexit 1",
        )),
        Fixture::SilentChrome => Some(fake_chrome(&dir, "exec sleep 30")),
        Fixture::OldChrome => {
            let chrome = fake_chrome(&dir, "exit 1");
            let script = std::fs::read_to_string(&chrome).unwrap();
            std::fs::write(
                &chrome,
                script.replace(common::FAKE_VERSION, "120.0.6099.1"),
            )
            .unwrap();
            Some(chrome)
        }
        Fixture::MissingChromeEnv => {
            env.push(("CHROME_CDP_CHROME", "/nonexistent/chrome".into()));
            let sb = Sandbox::new();
            norm = norm.with(sb.dir.path(), "<sandbox>");
            sandbox = Some(sb);
            None
        }
        Fixture::BadConfig => {
            std::fs::write(dir.path().join(".chrome-cdp.toml"), "sesion = \"shop\"\n").unwrap();
            cwd = Some(dir.path().to_owned());
            None
        }
        Fixture::FakeCft => {
            if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
                return None;
            }
            let server = browser::testing::FakeCft::start("linux64");
            norm = norm.with(Path::new(&server.url), "<cft>");
            env.push(("CHROME_CDP_CFT_URL", server.url.clone()));
            env.push(("CHROME_CDP_CACHE", dir.path().display().to_string()));
            _server = Some(server);
            None
        }
    };

    if matches!(
        case.fixture,
        Fixture::SandboxlessChrome | Fixture::SilentChrome | Fixture::OldChrome
    ) {
        let sb = Sandbox::new();
        norm = norm.with(sb.dir.path(), "<sandbox>");
        sandbox = Some(sb);
    }

    let site = Fixtures::shared();
    norm = norm.with(Path::new(site.a()), "<site>");
    let files = upload_files();
    norm = norm.with(&files, "<files>");
    let mut out = String::new();
    for mode in case.modes {
        let mut args: Vec<String> = case
            .args
            .iter()
            .map(|a| match &chrome {
                Some(chrome) => a.replace("{chrome}", &chrome.display().to_string()),
                None => (*a).to_owned(),
            })
            .map(|a| a.replace("{site}", site.a()))
            .map(|a| a.replace("{files}", &files.display().to_string()))
            .collect();
        if !mode.is_empty() {
            args.push((*mode).to_owned());
        }
        if case.fixture == Fixture::SessionPerMode
            && let Some(sb) = &sandbox
        {
            let out = sb.cli().arg("open").output().unwrap();
            assert!(out.status.success(), "{out:?}");
        }
        let mut cmd: Command = sandbox.as_ref().map_or_else(cli, Sandbox::cli);
        cmd.args(&args);
        if let Some(cwd) = &cwd {
            cmd.current_dir(cwd);
        }
        cmd.envs(env.iter().map(|(k, v)| (k, v)));
        let result = cmd.output().unwrap();
        let shown: Vec<_> = args.iter().map(|a| shell_words(a)).collect();
        let _ = writeln!(out, "$ chrome-cdp {}", shown.join(" ").trim_end());
        let _ = writeln!(out, "exit: {}", result.status.code().unwrap());
        for (name, bytes) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
            if !bytes.is_empty() {
                let _ = write!(out, "--- {name}\n{}", String::from_utf8_lossy(bytes));
                if !bytes.ends_with(b"\n") {
                    out.push_str("\n(no newline at end)\n");
                }
            }
        }
        out.push('\n');
        if matches!(
            case.fixture,
            Fixture::FreshSession | Fixture::SessionPerMode
        ) && let Some(sb) = &sandbox
        {
            let closed = sb.cli().args(["close", "--all"]).output().unwrap();
            assert!(closed.status.success(), "{closed:?}");
        }
    }
    Some(norm.apply(&out))
}

/// The files the upload cases upload (`{files}`).
fn upload_files() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/site/actions/upload")
        .canonicalize()
        .unwrap()
}

/// Quotes an argument for display the way a shell user would type it.
fn shell_words(arg: &str) -> String {
    protocol::format::shell_quote(arg).into_owned()
}

#[test]
fn command_output_matches_golden_files() {
    let base = Normalizer::new();
    let mut failures = Vec::new();
    for case in CASES {
        let Some(actual) = run_case(case, &base) else {
            continue;
        };
        check(
            &golden_dir().join(format!("{}.txt", case.name)),
            &actual,
            &mut failures,
        );
    }
    finish(&failures);
}

#[test]
fn schemas_match_golden_files() {
    let mut failures = Vec::new();
    for entry in protocol::schema::COMMANDS {
        let out = cli()
            .args(["schema"])
            .args(entry.name.split(' '))
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", entry.name);
        let file = format!("{}.json", entry.name.replace(' ', "_"));
        check(
            &golden_dir().join("schema").join(file),
            &String::from_utf8(out.stdout).unwrap(),
            &mut failures,
        );
    }
    finish(&failures);
}

/// Every golden file belongs to a case or schema, so none go stale.
#[test]
fn no_stale_golden_files() {
    let mut expected: Vec<PathBuf> = CASES
        .iter()
        .map(|c| golden_dir().join(format!("{}.txt", c.name)))
        .collect();
    expected.extend(protocol::schema::COMMANDS.iter().map(|c| {
        golden_dir()
            .join("schema")
            .join(format!("{}.json", c.name.replace(' ', "_")))
    }));
    for dir in [golden_dir(), golden_dir().join("schema")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_file() {
                assert!(
                    expected.contains(&path),
                    "stale golden file {}",
                    path.display()
                );
            }
        }
    }
    let mut names: Vec<_> = CASES.iter().map(|c| c.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CASES.len(), "case names must be unique");
}
