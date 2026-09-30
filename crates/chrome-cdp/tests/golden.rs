//! Golden tests of every command's text, `--json` and `-q` output, errors
//! and hints included (plan 08, milestone 8.1). The wording is part of the
//! UX, so a change shows up in review as a diff of `tests/golden/`.
//!
//! Each case runs the binary once per output mode and compares the result
//! with `tests/golden/<case>.txt`, after replacing machine-specific parts
//! (version, git sha, Chrome path, temp dirs) with placeholders. Every
//! registered JSON Schema is also kept in `tests/golden/schema/`, and the
//! command line's commands and flags in `tests/golden/cli.txt`
//! (`src/surface.rs`).
//!
//! To accept new output: `UPDATE_GOLDEN=1 cargo test -p chrome-cdp --test
//! golden` (with `CHROME_PATH` set, or the Chrome cases are not written).
//! `GOLDEN_CASES=keyboard,status` runs only the cases whose names hold one
//! of those words.

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
    /// (`{site}` is the fixture site's origin, `{sandbox}` the sandbox's
    /// directory). The first one opens it.
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
    /// Runs in `<tmp>/app`, made afresh with these files before each mode.
    Project(&'static [(&'static str, &'static str)]),
    /// A fake Chrome for Testing index (`<cft>`) and an empty cache in the
    /// temp dir. Linux x86_64 only: it serves `linux64` builds.
    FakeCft,
    /// `MissingChromeEnv` on Linux only: `doctor`'s checks differ by OS.
    LinuxMissingChromeEnv,
}

struct Case {
    name: &'static str,
    args: &'static [&'static str],
    /// Extra flags per run: "" is the text form.
    modes: &'static [&'static str],
    fixture: Fixture,
}

/// A project whose scenario and mock file `fmt` changes.
const FMT_PROJECT: &[(&str, &str)] = &[
    (
        "chrome-cdp.toml",
        "[e2e]\nbase_url = \"http://localhost:5173\"\n",
    ),
    (
        "e2e/scenarios/cart.cdp",
        "@tags smoke\n@mocks base\n@test 'adds an item'\n  goto /\n  click \"role=button[name='Add']\"   # the first\n  screenshot -o cart.png\n  expect \"text=1 item\" visible\n",
    ),
    (
        "e2e/mocks/base.toml",
        "[[mock]]\nrespond={status=204}\nmatch = { path = \"/api/cart\" }\nname = \"cart\"\n",
    ),
    ("e2e/mocks/data/items.json", "[]\n"),
];

const ALL: &[&str] = &["", "--json", "-q"];

/// `locators/filters.html` with refs: e5 is row 3's Delete button.
const FILTERS: &[&[&str]] = &[
    &["open", "{site}/locators/filters.html"],
    &["snapshot", "-i"],
];
const ERR: &[&str] = &["", "--json"];

/// The page with a `debugger;` statement, with a logpoint on the line
/// after it.
const DEBUGGING: &[&[&str]] = &[
    &["open", "{site}/debug/debugger.html"],
    &["debug", "log", "debug/debugger.html:10", "runs"],
];

/// The checkout fixture after its Log button logged `checkout done 42`.
const CHECKOUT_LOGGED: &[&[&str]] = &[&["open", "{site}/wait/checkout.html"], &["click", "#log"]];

/// The mock fixture page with the `base` and `sale` sets in use, after a
/// mocked, an unmatched and an unmocked request.
const MOCKED: &[&[&str]] = &[
    &["open", "{site}/mock/index.html"],
    &["mock", "use", "--dir", "{mocks}", "base", "sale"],
    &["eval", "fetch('/mock/api/products').then((r) => r.status)"],
    &[
        "eval",
        "fetch('/mock/api/products/9').then((r) => r.status)",
    ],
    &["eval", "fetch('/mock/api/cart').then((r) => r.status)"],
    &["eval", "fetch('/items?n=1').then((r) => r.status)"],
];

/// The live fixture page on the `live` mock set (plan 11.9): its
/// WebSocket and EventSource got their mocks' messages.
const LIVE: &[&[&str]] = &[
    &["open"],
    &["mock", "use", "--dir", "{mocks}", "live"],
    &["goto", "{site}/mock/live.html"],
    &["wait", "--fn", "window.seen && seen.length >= 4"],
];

/// A session that shopped on the record fixture page with refs (plan
/// 11.8), and made one mistake.
const HISTORY: &[&[&str]] = &[
    &["open", "{site}/record/shop.html"],
    &["wait", "text=Monitor"],
    &["snapshot", "-i"],
    &["click", "e2"],
    &["fill", "e4", "2"],
    &["click", "e5"],
    &[
        "expect",
        "role=status",
        "text",
        "Order o-17: ordered, 2 items",
    ],
];

/// The mock fixture page with the `base` and `cart` sets in use, after
/// it sent an order and asked for the cart, which no mock answers (plan
/// 11.5).
const REQUESTS: &[&[&str]] = &[
    &["open", "{site}/mock/index.html"],
    &["mock", "use", "--dir", "{mocks}", "base", "cart"],
    &[
        "eval",
        "fetch('/mock/api/orders', {method: 'POST', body: JSON.stringify({qty: 2}), \
         headers: {'content-type': 'application/json'}}).then((r) => r.status)",
    ],
    &[
        "eval",
        "fetch('/mock/api/cart?user=7').then((r) => r.status)",
    ],
];

/// The todos page on the `stateful` mock set, after it added a todo and
/// logged in.
const STATEFUL: &[&[&str]] = &[
    &["open"],
    &["mock", "use", "--dir", "{mocks}", "stateful"],
    &["goto", "{site}/mock/todos.html"],
    &["eval", "addTodo('Ship it')"],
    &[
        "eval",
        "fetch('/mock/api/login', {method: 'POST', body: JSON.stringify({email: 'ada@example.com'}), \
         headers: {'content-type': 'application/json'}}).then((r) => r.status)",
    ],
];

/// The baseline fixture after its snapshot and box were stored as
/// baselines (in `{sandbox}/b`) and the page changed: the button reads
/// "Continue" and the box is red.
const BASELINE_CHANGED: &[&[&str]] = &[
    &["open", "{site}/expect/baseline.html"],
    &["expect-snapshot", "login", "--dir", "{sandbox}/b"],
    &["expect-screenshot", "box", "#box", "--dir", "{sandbox}/b"],
    &[
        "eval",
        "document.getElementById('go').textContent = 'Continue'; \
         document.getElementById('box').style.background = '#cc3333'",
    ],
];

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

/// A session with two init scripts, s1 (main world) and s2 (isolated).
/// A page with cookies and storage of its own, for `cookies`, `storage`
/// and `state`.
const LOGGED_IN: &[&[&str]] = &[
    &["open", "{site}/state/app.html"],
    &["cookies", "set", "session=ada-42", "--httponly"],
    &[
        "cookies",
        "set",
        "consent=yes",
        "--domain",
        ".example.com",
        "--secure",
        "--samesite",
        "none",
    ],
    &["storage", "local", "set", "token", "tok-ada-42"],
    &["storage", "local", "set", "theme", "dark"],
    &["storage", "session", "set", "draft", "draft-7"],
];

/// [`LOGGED_IN`], saved to `{sandbox}/auth.json`.
const SAVED: &[&[&str]] = &[
    &["open", "{site}/state/app.html"],
    &["cookies", "set", "session=ada-42", "--httponly"],
    &["storage", "local", "set", "token", "tok-ada-42"],
    &["state", "save", "{sandbox}/auth.json"],
    &["goto", "about:blank"],
];

const TWO_SCRIPTS: &[&[&str]] = &[
    &["open"],
    &["init-script", "add", "--code", "window.__a = 1;"],
    &[
        "init-script",
        "add",
        "--world",
        "isolated",
        "--code",
        "// Marks the page\nwindow.__b = 2;",
    ],
];

/// A session with four routes, r1 to r4.
const FOUR_ROUTES: &[&[&str]] = &[
    &["open"],
    &["route", "/a", "--abort"],
    &["route", "/b", "--abort"],
    &["route", "/c", "--abort"],
    &["route", "/d", "--abort"],
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
        name: "help-topic",
        args: &["help", "refs"],
        modes: ALL,
        fixture: Fixture::None,
    },
    Case {
        name: "help-unknown",
        args: &["help", "locaters"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "convert",
        args: &["convert", "{scripts}/login.spec.ts"],
        modes: ALL,
        fixture: Fixture::None,
    },
    Case {
        name: "convert-missing",
        args: &["convert", "{scripts}/missing.spec.ts"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "init-e2e",
        args: &["init", "e2e"],
        modes: ALL,
        fixture: Fixture::Project(&[(
            "package.json",
            r#"{"scripts": {"dev": "vite"}, "devDependencies": {"vite": "^6"}}"#,
        )]),
    },
    Case {
        name: "init-e2e-kept",
        args: &["init", "e2e", "--framework", "none"],
        modes: &["", "--json"],
        fixture: Fixture::Project(&[
            (
                ".chrome-cdp.toml",
                "[e2e]\nbase_url = \"http://localhost:8080\"\n",
            ),
            (".gitignore", "node_modules/\n"),
            ("e2e/scenarios/example.cdp", "# mine\n"),
        ]),
    },
    Case {
        name: "init-e2e-bad-package",
        args: &["init", "e2e"],
        modes: ERR,
        fixture: Fixture::Project(&[("package.json", "{\"devDependencies\": ")]),
    },
    Case {
        name: "skill-agents-md",
        args: &["skill", "--format", "agents-md"],
        modes: ALL,
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
        name: "doctor-no-chrome",
        args: &["doctor", "--no-launch"],
        modes: ERR,
        fixture: Fixture::LinuxMissingChromeEnv,
    },
    Case {
        name: "upgrade-bad-version",
        args: &["upgrade", "--version", "latest"],
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
        name: "cdp-did-you-mean",
        args: &[
            "cdp",
            "--target",
            "page",
            "Page.navigte",
            r#"{"url":"about:blank"}"#,
        ],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "cdp-did-you-mean-browser",
        args: &["cdp", "Browser.getVersoin"],
        modes: ERR,
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
        name: "test-list",
        args: &["test", "--list"],
        modes: ALL,
        fixture: Fixture::Project(&[
            ("chrome-cdp.toml", "[e2e]\ntests = [\"e2e/**/*.cdp\"]\n"),
            (
                "e2e/cart.cdp",
                "@tags cart\n\n@test \"adds an item\"\n  goto /\n\n@test \"checks out\" @skip \"no payments yet\"\n",
            ),
        ]),
    },
    Case {
        name: "test-bad-scenario",
        args: &["test"],
        modes: ERR,
        fixture: Fixture::Project(&[
            ("chrome-cdp.toml", "[e2e]\ntests = [\"e2e/**/*.cdp\"]\n"),
            (
                "e2e/cart.cdp",
                "@test \"adds an item\"\n  goto /\n  @include missing.cdp\n",
            ),
            ("e2e/login.cdp", "@test \"logs in\"\n  clik text=Login\n"),
        ]),
    },
    Case {
        name: "context-new",
        args: &["context", "new", "data:text/html,<title>Own</title>"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "context-list",
        args: &["context", "list"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open"],
            &["context", "new", "data:text/html,<title>Own</title>"],
            &["context", "new"],
        ]),
    },
    Case {
        name: "context-close-none",
        args: &["context", "close", "--all"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "context-unknown",
        args: &["--context", "c9", "tabs"],
        modes: ERR,
        fixture: Fixture::Session,
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
        name: "wait-console",
        args: &["wait", "--console", "checkout (done|failed)"],
        // Each run would wait for another entry: one mode per case.
        modes: &[""],
        fixture: Fixture::SessionAfter(CHECKOUT_LOGGED),
    },
    Case {
        name: "wait-console-json",
        args: &["wait", "--console", "checkout (done|failed)"],
        modes: &["--json"],
        fixture: Fixture::SessionAfter(CHECKOUT_LOGGED),
    },
    Case {
        name: "wait-console-timeout",
        args: &["wait", "--console", "refunded", "--timeout", "300ms"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "wait-binding",
        args: &["wait", "--binding", "onCheckout"],
        modes: &[""],
        fixture: Fixture::SessionAfter(&[
            &["open"],
            &["expose", "onCheckout"],
            &["goto", "{site}/wait/checkout.html?after=0"],
            &["click", "#pay"],
        ]),
    },
    Case {
        name: "wait-url",
        args: &["wait", "--url", "blank"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "expect-text",
        args: &["expect", "#status", "text", "Loading"],
        modes: ALL,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-text-fails",
        args: &["expect", "#status", "text", "Saved", "--timeout", "300ms"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-ambiguous",
        args: &["expect", ".row", "visible", "--timeout", "300ms"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-state-fails",
        args: &["expect", "#save", "enabled", "--timeout", "300ms"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-count",
        args: &["expect", ".row", "count", "--min", "2"],
        modes: ALL,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-missing",
        args: &["expect", "#nope", "visible", "--timeout", "300ms"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-url-fails",
        args: &["expect", "--url", "/dashboard", "--timeout", "300ms"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-bad-matcher",
        args: &["expect", "e1", "visble"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "expect-no-errors-fails",
        args: &["expect", "--no-errors"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/errors.html"),
    },
    Case {
        name: "expect-no-failed-requests-fails",
        args: &["expect", "--no-failed-requests", "--origin", "self"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/errors.html"),
    },
    Case {
        name: "expect-no-failed-requests-all-tabs",
        args: &["expect", "--no-failed-requests", "--all-tabs"],
        modes: ERR,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/expect/index.html"],
            &["tab", "new", "{site}/expect/missing.json"],
            &["tab", "t1"],
        ]),
    },
    Case {
        name: "expect-no-errors-since-mark",
        args: &["expect", "--no-errors", "--since", "loaded"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/expect/errors.html"],
            &["mark", "loaded"],
        ]),
    },
    Case {
        name: "expect-unknown-mark",
        args: &["expect", "--no-errors", "--since", "nope"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-a11y-fails",
        args: &["expect", "--a11y"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/a11y.html"),
    },
    Case {
        name: "expect-a11y-frames",
        args: &["expect", "--a11y"],
        modes: ERR,
        fixture: Fixture::SessionAt("/expect/a11y-frames.html"),
    },
    Case {
        name: "mark",
        args: &["mark", "before-save"],
        modes: ALL,
        fixture: Fixture::SessionAt("/expect/index.html"),
    },
    Case {
        name: "expect-snapshot",
        args: &["expect-snapshot", "login", "--dir", "{sandbox}/b"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/expect/baseline.html"],
            &["expect-snapshot", "login", "--dir", "{sandbox}/b"],
        ]),
    },
    Case {
        name: "expect-snapshot-fails",
        args: &[
            "expect-snapshot",
            "login",
            "--dir",
            "{sandbox}/b",
            "--timeout",
            "300ms",
        ],
        modes: ERR,
        fixture: Fixture::SessionAfter(BASELINE_CHANGED),
    },
    Case {
        // The text form updates the baseline; the others find it up to date.
        name: "expect-snapshot-update",
        args: &[
            "expect-snapshot",
            "login",
            "--dir",
            "{sandbox}/b",
            "--update",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(BASELINE_CHANGED),
    },
    Case {
        name: "expect-snapshot-bad-name",
        args: &["expect-snapshot", "../login"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "expect-screenshot",
        args: &["expect-screenshot", "box", "#box", "--dir", "{sandbox}/b"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/expect/baseline.html"],
            &["expect-screenshot", "box", "#box", "--dir", "{sandbox}/b"],
        ]),
    },
    Case {
        name: "expect-screenshot-fails",
        args: &[
            "expect-screenshot",
            "box",
            "#box",
            "--dir",
            "{sandbox}/b",
            "--timeout",
            "300ms",
        ],
        modes: ERR,
        fixture: Fixture::SessionAfter(BASELINE_CHANGED),
    },
    Case {
        name: "expect-screenshot-bad-threshold",
        args: &["expect-screenshot", "box", "--threshold", "2"],
        modes: ERR,
        fixture: Fixture::None,
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
        name: "locator",
        args: &["locator", "e5"],
        modes: ALL,
        fixture: Fixture::SessionAfter(FILTERS),
    },
    Case {
        name: "locator-all",
        args: &["locator", "e5", "--all"],
        modes: &["", "--json"],
        fixture: Fixture::SessionAfter(FILTERS),
    },
    Case {
        name: "locator-ambiguous",
        args: &["locator", "role=button[name=Delete]"],
        modes: ERR,
        fixture: Fixture::SessionAt("/locators/filters.html"),
    },
    Case {
        name: "locator-none",
        args: &["locator", "#rule"],
        modes: ERR,
        fixture: Fixture::SessionAt("/locators/filters.html"),
    },
    Case {
        name: "locator-has-not-found",
        args: &[
            "click",
            "role=row >> has=role=button[name=Remove]",
            "--timeout",
            "300ms",
        ],
        modes: ERR,
        fixture: Fixture::SessionAt("/locators/filters.html"),
    },
    Case {
        name: "text-matches",
        args: &["text", ".item"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/index.html"),
    },
    Case {
        name: "text-frames",
        args: &["text"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/framed.html"),
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
        name: "frames",
        args: &["frames"],
        modes: ALL,
        fixture: Fixture::SessionAt("/frames/nested.html"),
    },
    Case {
        name: "frames-workers",
        args: &["frames"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/targets/matrix.html"],
            &["wait", "--fn", "document.title === 'Targets: ready'"],
        ]),
    },
    Case {
        name: "frames-not-found",
        args: &["eval", "--frame", "nope", "1"],
        modes: ERR,
        fixture: Fixture::SessionAt("/frames/nested.html"),
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
        name: "markdown-frames",
        args: &["markdown"],
        modes: ALL,
        fixture: Fixture::SessionAt("/read/framed.html"),
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
        args: &["dialog", "accept", "--timeout", "300ms"],
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
        name: "downloads-clear",
        args: &["downloads", "--clear"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "open-downloads-dir",
        args: &["open", "--downloads-dir", "{sandbox}/dl"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "open-init-script",
        args: &["open", "--init-script-code", "window.__ready = true"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "open-init-script-missing",
        args: &["open", "--init-script", "{sandbox}/missing.js"],
        modes: ERR,
        fixture: Fixture::NoSessions,
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
        args: &["console", "--since", "2 days"],
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
        name: "network-worker",
        args: &[
            "network",
            "--failed",
            "--url",
            "**/missing.json",
            "--since",
            "0",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/workers/worker.html"],
            &["click", "#fetch"],
            &["wait", "--fn", "document.title === 'Worker: fetched 404'"],
        ]),
    },
    Case {
        name: "console-worker",
        args: &["console", "--grep", "from service", "--since", "0"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/targets/matrix.html"],
            &["wait", "--fn", "document.title === 'Targets: ready'"],
            &["eval", "ask('service', 'log')"],
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
        name: "network-clear",
        args: &["network", "clear"],
        modes: &[""],
        fixture: Fixture::SessionAt("/network/index.html"),
    },
    Case {
        name: "network-frames",
        args: &["network", "frames", "2", "--grep", "price", "--limit", "1"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/network/frames.html"],
            &["wait", "--text", "open"],
            &["eval", "send(6)"],
            &["eval", "echoed(6)"],
        ]),
    },
    Case {
        name: "network-frames-not-websocket",
        args: &["network", "frames", "1"],
        modes: ERR,
        fixture: Fixture::SessionAt("/network/frames.html"),
    },
    Case {
        name: "wait-ws",
        args: &["wait", "--ws", "/ws", "--frame", "price"],
        // The --json run finds the frame that came while the first ran.
        modes: &["", "--json"],
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/network/frames.html"],
            &["wait", "--text", "open"],
            &[
                "eval",
                "later(JSON.stringify({ type: 'price', i: 1 }), 200)",
            ],
        ]),
    },
    Case {
        name: "wait-ws-timeout",
        args: &["wait", "--ws", "/nope", "--timeout", "500ms"],
        modes: ERR,
        fixture: Fixture::SessionAt("/network/frames.html"),
    },
    Case {
        name: "network-show-missing",
        args: &["network", "show", "#999"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "route",
        args: &[
            "route",
            "**/api/items",
            "--method",
            "GET",
            "--json-body",
            r#"{"items":[]}"#,
            "--times",
            "2",
        ],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "cookies",
        args: &["cookies"],
        modes: ALL,
        fixture: Fixture::SessionAfter(LOGGED_IN),
    },
    Case {
        name: "cookies-none",
        args: &["cookies", "--domain", "example.org"],
        modes: &[""],
        fixture: Fixture::SessionAfter(LOGGED_IN),
    },
    Case {
        name: "cookies-set",
        args: &["cookies", "set", "theme=dark", "--path", "/state"],
        modes: ALL,
        fixture: Fixture::SessionAt("/state/app.html"),
    },
    Case {
        name: "cookies-set-refused",
        args: &["cookies", "set", "x=1", "--samesite", "none"],
        modes: ERR,
        fixture: Fixture::SessionAt("/state/app.html"),
    },
    Case {
        name: "cookies-set-not-a-pair",
        args: &["cookies", "set", "theme"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "cookies-clear",
        args: &["cookies", "clear", "--domain", "example.com"],
        modes: &[""],
        fixture: Fixture::SessionAfter(LOGGED_IN),
    },
    Case {
        name: "storage",
        args: &["storage", "local"],
        modes: ALL,
        fixture: Fixture::SessionAfter(LOGGED_IN),
    },
    Case {
        name: "storage-get",
        args: &["storage", "session", "get", "draft"],
        modes: ALL,
        fixture: Fixture::SessionAfter(LOGGED_IN),
    },
    Case {
        name: "storage-get-missing",
        args: &["storage", "local", "get", "nope"],
        modes: ERR,
        fixture: Fixture::SessionAt("/state/app.html"),
    },
    Case {
        name: "storage-no-origin",
        args: &["storage", "local", "--origin", "https://example.com"],
        modes: ERR,
        fixture: Fixture::SessionAt("/state/app.html"),
    },
    Case {
        name: "storage-set-no-value",
        args: &["storage", "local", "set", "token"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "state-save",
        args: &["state", "save", "{sandbox}/auth.json"],
        modes: ALL,
        fixture: Fixture::SessionAfter(LOGGED_IN),
    },
    Case {
        name: "state-load",
        args: &["state", "load", "{sandbox}/auth.json"],
        modes: ALL,
        fixture: Fixture::SessionAfter(SAVED),
    },
    Case {
        name: "state-load-missing",
        args: &["state", "load", "{sandbox}/nope.json"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "init-script-add",
        args: &["init-script", "add", "--code", "window.__stub = true"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "init-script-add-now",
        args: &[
            "init-script",
            "add",
            "--now",
            "--world",
            "isolated",
            "--code",
            "window.__stub = true",
        ],
        modes: &[""],
        fixture: Fixture::Session,
    },
    Case {
        name: "init-script-does-not-compile",
        args: &["init-script", "add", "--code", "window.x = {;"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "init-script-list",
        args: &["init-script", "list"],
        modes: ALL,
        fixture: Fixture::SessionAfter(TWO_SCRIPTS),
    },
    Case {
        name: "init-script-list-none",
        args: &["init-script", "list"],
        modes: &[""],
        fixture: Fixture::Session,
    },
    // Each mode would remove what the one before did: one mode a case.
    Case {
        name: "init-script-remove",
        args: &["init-script", "remove", "s1"],
        modes: &[""],
        fixture: Fixture::SessionAfter(TWO_SCRIPTS),
    },
    Case {
        name: "init-script-remove-all",
        args: &["init-script", "remove", "--all"],
        modes: &["--json"],
        fixture: Fixture::SessionAfter(TWO_SCRIPTS),
    },
    Case {
        name: "init-script-remove-missing",
        args: &["init-script", "remove", "s9"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "expose",
        args: &["expose", "onSave", "onLoad"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "expose-returns",
        args: &["expose", "nextToken", "--returns-seq", r#"["a","b",null]"#],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "expose-returns-list",
        args: &["expose"],
        modes: &["", "--json"],
        fixture: Fixture::SessionAfter(&[
            &["open"],
            &["expose", "onSave"],
            &["expose", "getConfig", "--returns", r#"{"beta":true}"#],
        ]),
    },
    Case {
        name: "expose-returns-bad",
        args: &["expose", "nextToken", "--returns-seq", "42"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "expose-bad-name",
        args: &["expose", "on-save"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "unexpose",
        args: &["unexpose", "onSave"],
        modes: &[""],
        fixture: Fixture::SessionAfter(&[&["open"], &["expose", "onSave", "onLoad"]]),
    },
    Case {
        name: "unexpose-missing",
        args: &["unexpose", "nope"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "route-no-action",
        args: &["route", "/api/"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "route-two-actions",
        args: &["route", "/api/", "--abort", "--json-body", "{}"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "route-bad-reason",
        args: &["route", "/api/", "--abort", "nope"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "block",
        args: &["block", "--images", "--ads", "**/*.mp4"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "routes",
        args: &["routes"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/route/index.html"],
            &[
                "route",
                "/route/api/",
                "--body",
                "mocked",
                "--delay",
                "10ms",
            ],
            &["block", "--fonts"],
            &["eval", "fetch('/route/api/items').then((r) => r.text())"],
        ]),
    },
    Case {
        name: "routes-none",
        args: &["routes"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "mock-use",
        args: &["mock", "use", "--dir", "{mocks}", "base", "sale"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "mock-use-unknown",
        args: &["mock", "use", "--dir", "{mocks}", "bsae"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "mock-list-live",
        args: &["mock", "list"],
        modes: ALL,
        fixture: Fixture::SessionAfter(LIVE),
    },
    Case {
        name: "mock-ws-send",
        args: &[
            "mock",
            "ws",
            "send",
            "/mock/ws/**",
            "{\"type\":\"price\",\"value\":42}",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(LIVE),
    },
    Case {
        name: "mock-ws-send-binary",
        args: &["mock", "ws", "send", "/mock/ws/**", "AQID", "--base64"],
        modes: ALL,
        fixture: Fixture::SessionAfter(LIVE),
    },
    Case {
        name: "mock-ws-send-bad-base64",
        args: &[
            "mock",
            "ws",
            "send",
            "/mock/ws/**",
            "not base64!",
            "--base64",
        ],
        modes: ERR,
        fixture: Fixture::SessionAfter(LIVE),
    },
    Case {
        name: "mock-ws-send-none",
        args: &["mock", "ws", "send", "/nope/**", "hi", "--timeout", "200ms"],
        modes: ERR,
        fixture: Fixture::SessionAfter(LIVE),
    },
    Case {
        name: "mock-serve-unknown",
        args: &["mock", "serve", "--dir", "{mocks}", "bsae"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "mock-use-nothing",
        args: &["mock", "use"],
        modes: ERR,
        fixture: Fixture::Project(&[
            ("chrome-cdp.toml", "[e2e]\n"),
            ("e2e/mocks/base.toml", ""),
            ("e2e/mocks/errors/api-down.toml", ""),
        ]),
    },
    Case {
        name: "http-mocked",
        args: &["http", "GET", "{site}/mock/api/products"],
        modes: ALL,
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "http-unexpected-status",
        args: &[
            "http",
            "POST",
            "{site}/mock/api/cart",
            "--json-body",
            "{\"sku\":\"kb\"}",
        ],
        modes: ERR,
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "http-bad-status-flag",
        args: &["http", "GET", "/x", "--expect-status", "2x"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "mock-list",
        args: &["mock", "list"],
        modes: ALL,
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-list-none",
        args: &["mock", "list"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "mock-list-available",
        args: &["mock", "list", "--available", "--dir", "{mocks}"],
        modes: ALL,
        fixture: Fixture::None,
    },
    Case {
        name: "mock-log",
        args: &["mock", "log"],
        modes: ALL,
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-log-unmatched",
        args: &["mock", "log", "--unmatched"],
        modes: &[""],
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-reset",
        args: &["mock", "reset"],
        modes: ALL,
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-state",
        args: &["mock", "state"],
        modes: ALL,
        fixture: Fixture::SessionAfter(STATEFUL),
    },
    Case {
        name: "mock-state-todos",
        args: &["mock", "state", "todos"],
        modes: ALL,
        fixture: Fixture::SessionAfter(STATEFUL),
    },
    Case {
        name: "mock-state-unknown",
        args: &["mock", "state", "todo"],
        modes: ERR,
        fixture: Fixture::SessionAfter(STATEFUL),
    },
    Case {
        name: "mock-seed",
        args: &["mock", "seed", "todos"],
        modes: ALL,
        fixture: Fixture::SessionAfter(STATEFUL),
    },
    Case {
        name: "expect-mock-state",
        args: &["expect", "mock-state", "todos", "count", "3"],
        modes: ALL,
        fixture: Fixture::SessionAfter(STATEFUL),
    },
    Case {
        name: "expect-mock-state-fails",
        args: &[
            "expect",
            "mock-state",
            "todos",
            "contains",
            "{\"title\":\"Milk\"}",
            "--timeout",
            "300ms",
        ],
        modes: ERR,
        fixture: Fixture::SessionAfter(STATEFUL),
    },
    Case {
        name: "expect-request",
        args: &[
            "expect",
            "request",
            "POST",
            "/mock/api/orders",
            "--json-body",
            "{\"qty\":2}",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(REQUESTS),
    },
    Case {
        name: "expect-request-fails",
        args: &[
            "expect",
            "request",
            "POST",
            "/mock/api/orders",
            "--json-body",
            "{\"qty\":3}",
            "--timeout",
            "300ms",
        ],
        modes: ERR,
        fixture: Fixture::SessionAfter(REQUESTS),
    },
    Case {
        name: "expect-no-request-fails",
        args: &["expect", "no-request", "GET", "/mock/api/cart"],
        modes: ERR,
        fixture: Fixture::SessionAfter(REQUESTS),
    },
    Case {
        name: "mock-log-closest",
        args: &["mock", "log", "--unmatched"],
        modes: ERR,
        fixture: Fixture::SessionAfter(REQUESTS),
    },
    // Each mode would turn off what the one before did: one mode a case.
    Case {
        name: "mock-off",
        args: &["mock", "off", "sale"],
        modes: &[""],
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-off-all",
        args: &["mock", "off"],
        modes: &["--json"],
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-off-unknown",
        args: &["mock", "off", "nope"],
        modes: ERR,
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-explain",
        args: &["mock", "explain", "GET", "{site}/mock/api/products/7"],
        modes: ALL,
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-explain-unmatched",
        args: &["mock", "explain", "DELETE", "{site}/mock/api/cart?user=7"],
        modes: &[""],
        fixture: Fixture::SessionAfter(MOCKED),
    },
    Case {
        name: "mock-explain-files",
        args: &[
            "mock",
            "explain",
            "POST",
            "/api/login",
            "--json-body",
            "{\"email\":\"ada@corp.test\"}",
        ],
        modes: ALL,
        fixture: Fixture::Project(&[
            (
                "chrome-cdp.toml",
                "[e2e]\nbase_url = \"http://localhost:5173\"\ndefault_mocks = [\"base\"]\n",
            ),
            (
                "e2e/mocks/base.toml",
                "[settings]\nscope = [\"/api/**\"]\n\n[[mock]]\nname = \"login\"\nmatch = { method = \"POST\", path = \"/api/login\", json = { email = \"*@example.com\" } }\nrespond = { json = { ok = true } }\n",
            ),
        ]),
    },
    Case {
        name: "mock-explain-bad-file",
        args: &["mock", "explain", "GET", "/api/items", "--mocks", "base"],
        modes: ERR,
        fixture: Fixture::Project(&[
            (
                "chrome-cdp.toml",
                "[e2e]\nbase_url = \"http://localhost:5173\"\n",
            ),
            (
                "e2e/mocks/base.toml",
                "[[mock]]\nname = \"items\"\nmatch = { path = \"/api/items\" }\nrespond = { json_file = \"data/items.json\" }\n",
            ),
        ]),
    },
    Case {
        name: "history",
        args: &["history"],
        modes: ALL,
        fixture: Fixture::SessionAfter(HISTORY),
    },
    Case {
        name: "history-as-scenario",
        args: &["history", "--as-scenario", "orders a mouse", "--from", "2"],
        modes: ALL,
        fixture: Fixture::SessionAfter(HISTORY),
    },
    Case {
        name: "mock-check",
        args: &["mock", "check"],
        modes: ALL,
        fixture: Fixture::Project(&[
            (
                "chrome-cdp.toml",
                "[e2e]\nbase_url = \"http://localhost:5173\"\n",
            ),
            (
                "e2e/mocks/base.toml",
                "[settings]\nscope = [\"/api/**\"]\n\n[[mock]]\nname = \"product by id\"\nmatch = { method = \"GET\", path = \"/api/products/:id\" }\nrespond = { json = { id = 1, name = \"Keyboard\" } }\n\n[[mock]]\nname = \"product 7\"\nmatch = { method = \"GET\", path = \"/api/products/7\" }\nrespond = { json = { id = 7, name = \"Seven\" } }\n",
            ),
            (
                "e2e/mocks/errors/api-down.toml",
                "[[mock]]\nname = \"down\"\nmatch = { path = \"/api/**\" }\nrespond = { status = 503 }\n",
            ),
        ]),
    },
    Case {
        name: "mock-check-openapi",
        args: &["mock", "check"],
        modes: ERR,
        fixture: Fixture::Project(&[
            (
                "chrome-cdp.toml",
                "[e2e]\nbase_url = \"http://localhost:5173\"\nopenapi = \"api/openapi.yaml\"\n",
            ),
            (
                "api/openapi.yaml",
                "openapi: 3.0.3\ninfo: { title: shop, version: \"1\" }\nservers:\n  - url: /api\npaths:\n  /products/{id}:\n    get:\n      responses:\n        \"200\":\n          description: one\n          content:\n            application/json:\n              schema:\n                type: object\n                required: [id, name, price_cents]\n                properties:\n                  id: { type: integer }\n                  name: { type: string }\n                  price_cents: { type: integer }\n        \"404\": { description: none }\n",
            ),
            (
                "e2e/mocks/base.toml",
                "[settings]\nscope = [\"/api/**\"]\n\n[[mock]]\nname = \"product by id\"\nmatch = { method = \"GET\", path = \"/api/products/:id\" }\nrespond = { json = { id = 1, name = \"Keyboard\", price = 12 } }\n\n[[mock]]\nname = \"gone\"\nmatch = { method = \"GET\", path = \"/api/products/0\" }\nrespond = { status = 410 }\n\n[[mock]]\nname = \"cart\"\nmatch = { method = \"GET\", path = \"/api/cart\" }\nrespond = { json = [] }\n",
            ),
        ]),
    },
    Case {
        name: "fmt",
        args: &["fmt"],
        modes: ALL,
        fixture: Fixture::Project(FMT_PROJECT),
    },
    Case {
        name: "fmt-check",
        args: &["fmt", "--check"],
        modes: ERR,
        fixture: Fixture::Project(FMT_PROJECT),
    },
    Case {
        name: "lint",
        args: &["lint"],
        modes: ERR,
        fixture: Fixture::Project(&[
            (
                "chrome-cdp.toml",
                "[e2e]\nbase_url = \"http://localhost:5173\"\n",
            ),
            (
                "e2e/scenarios/cart.cdp",
                "@test \"adds an item\"\n  goto /\n  click e12\n  wait 500\n  expect text=1 item visible\n\n@test \"adds an item\"\n  goto /\n  clik text=Add\n",
            ),
            (
                "e2e/mocks/base.toml",
                "[[mock]]\nmatch = { path = \"/api/**\" }\nrespond = { status = 200 }\n\n[[mock]]\nmatch = { path = \"/api/cart\" }\nrespond = { status = 204 }\n",
            ),
        ]),
    },
    Case {
        name: "lint-warnings",
        args: &["lint"],
        modes: ALL,
        fixture: Fixture::Project(&[
            (
                "chrome-cdp.toml",
                "[e2e]\nbase_url = \"http://localhost:5173\"\n",
            ),
            (
                "e2e/scenarios/cart.cdp",
                "@test \"opens the cart\"\n  goto /\n  wait 2s\n  click text=Cart\n",
            ),
        ]),
    },
    Case {
        name: "mock-record",
        args: &[
            "mock",
            "record",
            "--scope",
            "/items",
            "-o",
            "{sandbox}/app/recorded.toml",
            "--force",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/mock/index.html"],
            &["eval", "fetch('/items?n=2').then((r) => r.json())"],
            &["eval", "fetch('/items?n=30').then((r) => r.json())"],
            &["eval", "fetch('/mock/index.html').then((r) => r.status)"],
        ]),
    },
    Case {
        name: "mock-record-nothing",
        args: &[
            "mock",
            "record",
            "--scope",
            "/api/**",
            "-o",
            "{sandbox}/x.toml",
        ],
        modes: ERR,
        fixture: Fixture::SessionAt("/mock/index.html"),
    },
    Case {
        name: "mock-import",
        args: &["mock", "import", "requests.har", "--scope", "/api/**"],
        modes: ALL,
        fixture: Fixture::Project(&[
            (
                "chrome-cdp.toml",
                "[e2e]\nbase_url = \"http://localhost:5173\"\n",
            ),
            (
                "requests.har",
                r#"{"log": {"version": "1.2", "entries": [
  {"_resourceType": "document",
   "request": {"method": "GET", "url": "http://localhost:5173/", "headers": []},
   "response": {"status": 200, "headers": [], "content": {"size": 13, "mimeType": "text/html", "text": "<html></html>"}}},
  {"_resourceType": "fetch",
   "request": {"method": "GET", "url": "http://localhost:5173/api/products/7", "headers": []},
   "response": {"status": 200, "headers": [{"name": "Date", "value": "Mon, 01 Jan 2026 10:00:00 GMT"}],
                "content": {"size": 62, "mimeType": "application/json", "text": "{\"id\": 7, \"name\": \"Mechanical keyboard\", \"updated_at\": \"2026-01-01T10:00:00Z\"}"}}},
  {"_resourceType": "fetch",
   "request": {"method": "OPTIONS", "url": "http://localhost:5173/api/orders", "headers": []},
   "response": {"status": 204, "headers": [], "content": {"size": 0, "mimeType": ""}}},
  {"_resourceType": "fetch",
   "request": {"method": "POST", "url": "http://localhost:5173/api/orders", "headers": [], "postData": {"mimeType": "application/json", "text": "{\"qty\": 1}"}},
   "response": {"status": 503, "headers": [], "content": {"size": 16, "mimeType": "application/json", "text": "{\"error\":\"busy\"}"}}},
  {"_resourceType": "fetch",
   "request": {"method": "POST", "url": "http://localhost:5173/api/orders", "headers": [], "postData": {"mimeType": "application/json", "text": "{\"qty\": 1}"}},
   "response": {"status": 201, "headers": [{"name": "Location", "value": "/api/orders/1"}], "content": {"size": 8, "mimeType": "application/json", "text": "{\"id\":1}"}}}
]}}"#,
            ),
        ]),
    },
    Case {
        name: "mock-import-bad",
        args: &["mock", "import", "requests.har"],
        modes: ERR,
        fixture: Fixture::Project(&[("requests.har", "{\"entries\": []}")]),
    },
    // Each mode would remove what the one before did: one mode a case.
    Case {
        name: "unroute",
        args: &["unroute", "r1", "r3"],
        modes: &[""],
        fixture: Fixture::SessionAfter(FOUR_ROUTES),
    },
    Case {
        name: "unroute-all",
        args: &["unroute", "--all"],
        modes: &["--json"],
        fixture: Fixture::SessionAfter(FOUR_ROUTES),
    },
    Case {
        name: "unroute-missing",
        args: &["unroute", "r7"],
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
        name: "screenshot-full-container",
        args: &["screenshot", "--full", "--max-size", "0"],
        modes: ALL,
        fixture: Fixture::SessionAt("/shots/scroller.html"),
    },
    Case {
        name: "screenshot-full-not-scrolling",
        args: &["screenshot", "--full", "css=header"],
        modes: &[""],
        fixture: Fixture::SessionAt("/shots/scroller.html"),
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
        name: "eval-nodes",
        args: &["eval", "[...document.querySelectorAll('.item, #save')]"],
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
        name: "cdp-listen-unknown-event",
        args: &[
            "cdp",
            "--target",
            "page",
            "--listen",
            "Page.loadEventFird",
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
        name: "pdf-footer",
        args: &[
            "pdf",
            "--pages",
            "1-2",
            "--scale",
            "0.5",
            "--margin",
            "2cm",
            "--footer",
            "<div style='font-size:10px'>Page <span class=pageNumber></span></div>",
        ],
        modes: ALL,
        fixture: Fixture::SessionAt("/shots/tall.html"),
    },
    Case {
        name: "pdf-bad-scale",
        args: &["pdf", "--scale", "3"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "pdf-header-no-file",
        args: &["pdf", "--header", "@no-such-header.html"],
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
        name: "coverage-start",
        args: &["coverage", "start"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "perf-not-running",
        args: &["perf"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "coverage-stop-not-measuring",
        args: &["coverage", "stop"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "highlight",
        args: &["highlight", "#red", "--duration", "500ms"],
        modes: ALL,
        fixture: Fixture::SessionAt("/shots/index.html"),
    },
    Case {
        name: "source",
        args: &["source", "#pay"],
        modes: ALL,
        fixture: Fixture::SessionAt("/source/fake-svelte.html"),
    },
    Case {
        name: "source-production-build",
        args: &["source", "role=button >> nth=0"],
        modes: ERR,
        fixture: Fixture::SessionAt("/apps/react.html"),
    },
    Case {
        name: "debug-log",
        args: &["debug", "log", "debug/debugger.html:10", "runs"],
        modes: ALL,
        fixture: Fixture::SessionAt("/debug/debugger.html"),
    },
    Case {
        name: "debug-log-pending",
        args: &["debug", "log", "src/later.ts:6", "note"],
        modes: ERR,
        fixture: Fixture::SessionAt("/debug/debugger.html"),
    },
    Case {
        name: "debug-log-bad-expression",
        args: &["debug", "log", "debug/debugger.html:10", "runs +"],
        modes: ERR,
        fixture: Fixture::SessionAt("/debug/debugger.html"),
    },
    Case {
        name: "debug-log-bad-place",
        args: &["debug", "log", "debug/debugger.html", "runs"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "debug-hit",
        args: &["click", "#go"],
        modes: ALL,
        fixture: Fixture::SessionAfter(DEBUGGING),
    },
    Case {
        name: "debug-list",
        args: &["debug", "list"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/debug/debugger.html"],
            &["debug", "log", "debug/debugger.html:10", "runs"],
            &["debug", "log", "src/later.ts:6"],
            &["click", "#go"],
        ]),
    },
    Case {
        name: "debug-clear",
        args: &["debug", "clear", "--all"],
        modes: ALL,
        fixture: Fixture::SessionAfter(DEBUGGING),
    },
    Case {
        name: "debug-clear-unknown",
        args: &["debug", "clear", "7"],
        modes: ERR,
        fixture: Fixture::SessionAfter(DEBUGGING),
    },
    Case {
        name: "components",
        args: &["components"],
        modes: ALL,
        fixture: Fixture::SessionAt("/source/deep-svelte.html"),
    },
    Case {
        name: "components-element",
        args: &["components", "#pay"],
        modes: ALL,
        fixture: Fixture::SessionAt("/source/fake-svelte.html"),
    },
    Case {
        name: "components-production-build",
        args: &["components"],
        modes: ERR,
        fixture: Fixture::SessionAt("/apps/react.html"),
    },
    Case {
        name: "snapshot-source",
        args: &["snapshot", "-i", "--source"],
        modes: ALL,
        fixture: Fixture::SessionAt("/source/fake-svelte.html"),
    },
    Case {
        name: "inspect-headless",
        args: &["inspect", "--panel", "console"],
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
        name: "click-missing-in-context",
        args: &["--context", "c2", "click", "#nothing", "--timeout", "1s"],
        modes: ERR,
        fixture: Fixture::SessionAfter(&[&["open"], &["context", "new"], &["context", "new"]]),
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
        name: "upload-directory",
        args: &["upload", "#folder", "{files}/tree"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/folder.html"),
    },
    Case {
        name: "upload-directory-into-files",
        args: &["upload", "#files", "{files}/tree"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/folder.html"),
    },
    Case {
        name: "upload-files-into-directory",
        args: &["upload", "#folder", "{files}/hello.txt"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/folder.html"),
    },
    Case {
        name: "upload-no-chooser",
        args: &["upload", "#nothing", "{files}/hello.txt"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/files.html"),
    },
    Case {
        name: "upload-drop",
        args: &[
            "upload",
            "#zone",
            "{files}/hello.txt",
            "{files}/notes.md",
            "--drop",
        ],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/drop.html"),
    },
    Case {
        name: "upload-drop-refused",
        args: &["upload", "#inert", "{files}/hello.txt", "--drop"],
        modes: ERR,
        fixture: Fixture::SessionAt("/actions/drop.html"),
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
        name: "clipboard-write-html",
        args: &["clipboard", "write", "--html", "<p>Hello <b>world</b></p>"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/clipboard.html"),
    },
    Case {
        name: "clipboard-read-html",
        args: &["clipboard", "read", "--type", "text/html"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/actions/clipboard.html"],
            &["click", "#copy-rich"],
        ]),
    },
    Case {
        name: "clipboard-write-image",
        args: &["clipboard", "write", "--file", "{files}/dot.png"],
        modes: ALL,
        fixture: Fixture::SessionAt("/actions/clipboard.html"),
    },
    Case {
        name: "clipboard-read-image",
        args: &[
            "clipboard",
            "read",
            "--type",
            "image/png",
            "-o",
            "{sandbox}/copied.png",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/actions/clipboard.html"],
            &["click", "#copy-image"],
        ]),
    },
    Case {
        name: "clipboard-read-missing-type",
        args: &[
            "clipboard",
            "read",
            "--type",
            "image/png",
            "-o",
            "{sandbox}/copied.png",
        ],
        modes: ERR,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/actions/clipboard.html"],
            &["click", "#copy-rich"],
        ]),
    },
    Case {
        name: "clipboard-read-image-needs-o",
        args: &["clipboard", "read", "--type", "image/png"],
        modes: ERR,
        fixture: Fixture::None,
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
        name: "tap-on-a-phone",
        args: &["tap", "#tap"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/actions/touch.html"],
            &["emulate", "iPhone 15"],
        ]),
    },
    Case {
        name: "emulate-device",
        args: &["emulate", "iphone", "15"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "emulate-unknown",
        args: &["emulate", "iPhone 99"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "emulate",
        args: &["emulate"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open"],
            &["viewport", "800x600", "--dpr", "2"],
            &["tz", "Europe/Berlin"],
            &["locale", "de-DE"],
            &["media", "--color-scheme", "dark"],
            &["time", "freeze", "2026-01-31T09:30:00Z"],
        ]),
    },
    Case {
        name: "emulate-nothing",
        args: &["emulate"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "emulate-list",
        args: &["emulate", "--list"],
        modes: &[""],
        fixture: Fixture::None,
    },
    Case {
        name: "viewport",
        args: &["viewport", "390x844", "--dpr", "3", "--mobile", "--touch"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "viewport-bad-size",
        args: &["viewport", "390"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "geo",
        args: &["geo", "-33.8688,151.2093", "--accuracy", "50"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "geo-off-the-globe",
        args: &["geo", "91,0"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "tz-unknown",
        args: &["tz", "Mars/Base"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "media",
        args: &[
            "media",
            "--color-scheme",
            "dark",
            "--reduced-motion",
            "reduce",
        ],
        modes: &[""],
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "offline",
        args: &["offline", "on"],
        modes: &[""],
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "throttle",
        args: &["throttle", "slow-3g"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "throttle-nothing",
        args: &["throttle"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "cpu-throttle",
        args: &["cpu-throttle", "4"],
        modes: &[""],
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "time-freeze",
        args: &["time", "freeze", "2026-12-24T18:00+01:00"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "time-advance-unfrozen",
        args: &["time", "advance", "90s"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "time-pause",
        args: &["time", "pause", "2026-01-31T09:30:00Z"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "time-run-for",
        args: &["time", "run-for", "1s"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "{site}/emulate/timers.html"],
            &["time", "pause", "2026-01-31T09:30:00Z"],
            &["goto", "{site}/emulate/timers.html"],
        ]),
    },
    Case {
        name: "time-run-for-unpaused",
        args: &["time", "run-for", "1s"],
        modes: ERR,
        fixture: Fixture::Session,
    },
    Case {
        name: "time-pause-at-past",
        args: &["time", "pause-at", "2026-01-31T09:00:00Z"],
        modes: ERR,
        fixture: Fixture::SessionAfter(&[&["open"], &["time", "pause", "2026-01-31T09:30:00Z"]]),
    },
    Case {
        name: "time-resume",
        args: &["time", "resume"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[&["open"], &["time", "pause", "2026-01-31T09:30:00Z"]]),
    },
    Case {
        name: "permissions",
        args: &["permissions"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open"],
            &["permissions", "grant", "notifications", "camera"],
            &[
                "permissions",
                "deny",
                "geolocation",
                "--origin",
                "https://example.com",
            ],
        ]),
    },
    Case {
        name: "locale",
        args: &["locale", "fr-CA"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "ua",
        args: &["ua", "TestBot/1.0"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "time-reset",
        args: &["time", "reset"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[&["open"], &["time", "freeze", "2026-01-31T09:30:00Z"]]),
    },
    Case {
        name: "permissions-deny",
        args: &[
            "permissions",
            "deny",
            "camera",
            "--origin",
            "https://example.com",
        ],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "permissions-reset",
        args: &["permissions", "reset"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[&["open"], &["permissions", "grant", "notifications"]]),
    },
    Case {
        name: "extensions",
        args: &["extensions"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[&["open", "--extension", "{extensions}/basic"]]),
    },
    Case {
        name: "extensions-none",
        args: &["extensions"],
        modes: &[""],
        fixture: Fixture::Session,
    },
    Case {
        name: "extensions-load",
        args: &["extensions", "load", "{extensions}/basic"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "extensions-load-no-manifest",
        args: &["extensions", "load", "{extensions}"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "extensions-reload",
        args: &["extensions", "reload", "chrome-cdp fixture"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[&["open", "--extension", "{extensions}/basic"]]),
    },
    // One mode per case: the first run uninstalls it.
    Case {
        name: "extensions-uninstall",
        args: &["extensions", "uninstall", "chrome-cdp fixture"],
        modes: &[""],
        fixture: Fixture::SessionAfter(&[&["open", "--extension", "{extensions}/basic"]]),
    },
    Case {
        name: "extensions-uninstall-json",
        args: &["extensions", "uninstall", "chrome-cdp fixture"],
        modes: &["--json"],
        fixture: Fixture::SessionAfter(&[&["open", "--extension", "{extensions}/basic"]]),
    },
    Case {
        name: "extensions-popup",
        args: &["extensions", "popup", "chrome-cdp fixture"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[&["open", "--extension", "{extensions}/basic"]]),
    },
    Case {
        name: "extensions-storage",
        args: &["extensions", "storage", "chrome-cdp fixture"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "--extension", "{extensions}/basic"],
            &[
                "extensions",
                "storage",
                "chrome-cdp fixture",
                "set",
                r#"{"theme": "dark", "count": 3}"#,
            ],
        ]),
    },
    Case {
        name: "extensions-storage-get",
        args: &[
            "extensions",
            "storage",
            "chrome-cdp fixture",
            "get",
            "theme",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "--extension", "{extensions}/basic"],
            &[
                "extensions",
                "storage",
                "chrome-cdp fixture",
                "set",
                r#"{"theme": "dark", "count": 3}"#,
            ],
        ]),
    },
    Case {
        name: "extensions-storage-set",
        args: &[
            "extensions",
            "storage",
            "chrome-cdp fixture",
            "set",
            r#"{"theme": "light"}"#,
            "--area",
            "session",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[&["open", "--extension", "{extensions}/basic"]]),
    },
    Case {
        name: "extensions-storage-remove",
        args: &[
            "extensions",
            "storage",
            "chrome-cdp fixture",
            "remove",
            "count",
        ],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "--extension", "{extensions}/basic"],
            &[
                "extensions",
                "storage",
                "chrome-cdp fixture",
                "set",
                r#"{"theme": "dark", "count": 3}"#,
            ],
        ]),
    },
    Case {
        name: "extensions-storage-clear",
        args: &["extensions", "storage", "chrome-cdp fixture", "clear"],
        modes: ALL,
        fixture: Fixture::SessionAfter(&[
            &["open", "--extension", "{extensions}/basic"],
            &[
                "extensions",
                "storage",
                "chrome-cdp fixture",
                "set",
                r#"{"theme": "dark"}"#,
            ],
        ]),
    },
    Case {
        name: "extensions-storage-managed",
        args: &[
            "extensions",
            "storage",
            "chrome-cdp fixture",
            "set",
            r#"{"theme": "dark"}"#,
            "--area",
            "managed",
        ],
        modes: ERR,
        fixture: Fixture::SessionAfter(&[&["open", "--extension", "{extensions}/basic"]]),
    },
    Case {
        name: "extensions-not-found",
        args: &["extensions", "popup", "My Extension"],
        modes: ERR,
        fixture: Fixture::SessionAfter(&[&["open", "--extension", "{extensions}/basic"]]),
    },
    Case {
        name: "permissions-bad-name",
        args: &["permissions", "grant", "notification"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "mouse-move",
        args: &["mouse", "move", "10", "20.5"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "mouse-down",
        args: &["mouse", "down", "--button", "right"],
        modes: &[""],
        fixture: Fixture::Session,
    },
    Case {
        name: "mouse-up",
        args: &["mouse", "up"],
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
        name: "keyboard-down",
        args: &["keyboard", "down", "Shift"],
        modes: ALL,
        fixture: Fixture::SessionPerMode,
    },
    Case {
        name: "keyboard-up",
        args: &["keyboard", "up", "--all"],
        modes: &[""],
        fixture: Fixture::SessionAfter(&[
            &["open"],
            &["keyboard", "down", "Control"],
            &["keyboard", "down", "Shift"],
        ]),
    },
    Case {
        name: "keyboard-up-not-held",
        args: &["keyboard", "up", "Alt"],
        modes: ALL,
        fixture: Fixture::Session,
    },
    Case {
        name: "keyboard-unknown-key",
        args: &["keyboard", "down", "Shft"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "keyboard-chord",
        args: &["keyboard", "down", "Shift+a"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "status-held-keys",
        args: &["status"],
        modes: &["", "--json"],
        fixture: Fixture::SessionAfter(&[&["open"], &["keyboard", "down", "Shift"]]),
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
    Case {
        name: "run-invalid",
        args: &["run", "{scripts}/bad.cdp"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        // The tests' stdin is empty.
        name: "batch-empty",
        args: &["batch"],
        modes: ERR,
        fixture: Fixture::NoSessions,
    },
    Case {
        name: "completions-unknown-shell",
        args: &["completions", "tcsh"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "run",
        args: &["run", "{scripts}/read.cdp"],
        modes: ALL,
        fixture: Fixture::SessionAt("/script/login.html"),
    },
    Case {
        name: "run-failed-step",
        args: &["run", "{scripts}/fails.cdp", "--continue"],
        modes: ALL,
        fixture: Fixture::SessionAt("/script/login.html"),
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
            // `doctor` says where the binary is.
            (env!("CARGO_BIN_EXE_chrome-cdp").to_owned(), "<bin>"),
            // `expect-screenshot` keeps a baseline per platform.
            (format!("-{}.png", std::env::consts::OS), "-<os>.png"),
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
            // How many times `expect` tried: `(after 300ms, 18 retries)`.
            ("\"retries\":", &digits, "<n>"),
            ("\"tries\":", &digits, "<n>"),
            ("ms, ", &digits, "<n>"),
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
        // A busy machine can leave room for one retry only.
        .replace("<n> retry)", "<n> retries)")
        .pipe(|text| chrome_ids(&text))
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl Pipe for String {}

/// Replaces Chrome's ids of frames and targets (32 upper-case hex
/// digits) with `<id>`.
fn chrome_ids(text: &str) -> String {
    let hex = |c: char| c.is_ascii_digit() || ('A'..='F').contains(&c);
    let mut out = String::new();
    let mut run = String::new();
    for c in text.chars().chain(std::iter::once('\0')) {
        if hex(c) {
            run.push(c);
            continue;
        }
        if run.len() == 32 {
            out.push_str("<id>");
        } else {
            out.push_str(&run);
        }
        run.clear();
        if c != '\0' {
            out.push(c);
        }
    }
    out
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
                    let dir = sb.dir.path().display().to_string();
                    let args: Vec<String> = step
                        .iter()
                        .map(|a| {
                            a.replace("{site}", site)
                                .replace("{sandbox}", &dir)
                                .replace("{mocks}", &mocks_dir().display().to_string())
                                .replace(
                                    "{extensions}",
                                    &testdata("extensions").display().to_string(),
                                )
                        })
                        .collect();
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
        Fixture::LinuxMissingChromeEnv if !cfg!(target_os = "linux") => return None,
        Fixture::MissingChromeEnv | Fixture::LinuxMissingChromeEnv => {
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
        Fixture::Project(_) => {
            cwd = Some(dir.path().join("app"));
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
    norm = norm
        .with(Path::new(site.a()), "<site>")
        .with(Path::new(site.b()), "<site-b>");
    // Sockets' URLs (plan 11.9).
    norm = norm.with(Path::new(&site.a().replacen("http", "ws", 1)), "<ws-site>");
    let files = upload_files();
    norm = norm.with(&files, "<files>");
    let scripts = testdata("scripts");
    norm = norm.with(&scripts, "<scripts>");
    let mocks = mocks_dir();
    norm = norm.with(&mocks, "<mocks>");
    let extensions = testdata("extensions");
    norm = norm.with(&extensions, "<extensions>");
    // Extension ids come from their directories' paths.
    let mut extension_ids: Vec<String> = Vec::new();
    let list_extensions = |ids: &mut Vec<String>| {
        let Some(sb) = sandbox
            .as_ref()
            .filter(|_| case.args.first() == Some(&"extensions"))
        else {
            return;
        };
        let listed = sb.cli().args(["extensions", "-q"]).output().unwrap();
        for id in String::from_utf8_lossy(&listed.stdout).lines() {
            if !id.is_empty() && !ids.iter().any(|i| i == id) {
                ids.push(id.to_owned());
            }
        }
    };
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
            .map(|a| a.replace("{scripts}", &scripts.display().to_string()))
            .map(|a| a.replace("{mocks}", &mocks.display().to_string()))
            .map(|a| a.replace("{extensions}", &extensions.display().to_string()))
            .map(|a| match &sandbox {
                Some(sb) => a.replace("{sandbox}", &sb.dir.path().display().to_string()),
                None => a,
            })
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
        if let (Fixture::Project(files), Some(cwd)) = (case.fixture, &cwd) {
            let _ = std::fs::remove_dir_all(cwd);
            for (path, text) in files {
                let path = cwd.join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
        }
        list_extensions(&mut extension_ids);
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
        list_extensions(&mut extension_ids);
        if matches!(
            case.fixture,
            Fixture::FreshSession | Fixture::SessionPerMode
        ) && let Some(sb) = &sandbox
        {
            let closed = sb.cli().args(["close", "--all"]).output().unwrap();
            assert!(closed.status.success(), "{closed:?}");
        }
    }
    for id in &extension_ids {
        norm = norm.with(Path::new(id), "<ext-id>");
    }
    Some(norm.apply(&out))
}

/// The fixture site's mocks directory (`{mocks}`).
fn mocks_dir() -> PathBuf {
    testdata("site/mock/mocks")
}

/// The files the upload cases upload (`{files}`).
fn upload_files() -> PathBuf {
    testdata("site/actions/upload")
}

/// A directory of `testdata/`, e.g. the `run` cases' scripts (`{scripts}`).
fn testdata(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(dir)
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
    // `GOLDEN_CASES=keyboard,status`: only the cases whose names hold one
    // of these, while working on them.
    let only = std::env::var("GOLDEN_CASES").unwrap_or_default();
    let only: Vec<&str> = only.split(',').filter(|s| !s.is_empty()).collect();
    for case in CASES {
        if !only.is_empty() && !only.iter().any(|o| case.name.contains(o)) {
            continue;
        }
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

/// Every command's output is under review: some case runs it.
#[test]
fn every_command_has_a_golden_case() {
    let missing: Vec<&str> = protocol::schema::COMMANDS
        .iter()
        .map(|c| c.name)
        .filter(|name| {
            let words: Vec<&str> = name.split(' ').collect();
            !CASES.iter().any(|case| {
                // `version` stands for `--version`.
                let typed: Vec<&str> = if case.args.first() == Some(&"-V") {
                    vec!["version"]
                } else {
                    case.args
                        .iter()
                        .copied()
                        .filter(|a| !a.starts_with('-'))
                        .collect()
                };
                // Also with one argument between the words, as in
                // `extensions storage NAME get`.
                typed.starts_with(&words)
                    || (1..typed.len()).any(|i| {
                        let mut skipped = typed.clone();
                        skipped.remove(i);
                        skipped.starts_with(&words)
                    })
            })
        })
        .collect();
    assert!(missing.is_empty(), "no golden case runs {missing:?}");
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
    // The command line's surface, kept by the binary's `surface` test.
    expected.push(golden_dir().join("cli.txt"));
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
