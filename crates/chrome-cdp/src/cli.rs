//! Command-line definitions (clap derive).
//!
//! Global flags follow plan 08 §4: each has a `CHROME_CDP_*` env var, may
//! appear before or after the subcommand, and (for `--session` and
//! `--timeout`) may default from the config files (see `config`). Plan 02 onwards add the real
//! commands to [`Command`].

use std::path::PathBuf;
use std::time::Duration;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand, ValueEnum};

use protocol::Mode;

/// Drive Chrome over the DevTools Protocol from a shell. Built for AI agents.
#[derive(Debug, Parser)]
#[command(
    name = "chrome-cdp",
    disable_version_flag = true,
    // `help` is ours (topics, `help commands`), and `--help` is short: the
    // top commands and the locator grammar (plan 08.3).
    disable_help_subcommand = true,
    override_help = crate::help::SHORT
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) global: GlobalArgs,

    /// Print the version, git commit and CDP protocol revision.
    #[arg(short = 'V', long)]
    pub(crate) version: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

/// Flags every command accepts.
#[derive(Debug, Clone, Args)]
pub(crate) struct GlobalArgs {
    /// Print one JSON object on stdout: {"ok":true,"data":…} or {"ok":false,"error":…}.
    #[arg(
        long,
        global = true,
        env = "CHROME_CDP_JSON",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) json: bool,

    /// Print only the essential result, for `$(…)` capture.
    #[arg(
        short,
        long,
        global = true,
        env = "CHROME_CDP_QUIET",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true,
        conflicts_with = "json"
    )]
    pub(crate) quiet: bool,

    /// Browser session to use; each session has its own daemon and Chrome.
    #[arg(
        long,
        global = true,
        env = "CHROME_CDP_SESSION",
        default_value = "default",
        value_name = "NAME"
    )]
    pub(crate) session: String,

    /// Give up after this long, e.g. 500ms, 30s, 2m. Default: 5s for
    /// `expect`; 10s for `wait`, actions, the read commands (`text`,
    /// `attr`, …) and `screenshot LOC`; 30s for navigation and everything
    /// else.
    #[arg(
        long,
        global = true,
        env = "CHROME_CDP_TIMEOUT",
        value_parser = parse_duration,
        value_name = "DURATION"
    )]
    pub(crate) timeout: Option<Duration>,

    /// The tab page commands act on, e.g. t2 (see `chrome-cdp tabs`).
    /// Default: the active tab.
    #[arg(long, global = true, env = "CHROME_CDP_TAB", value_name = "ID")]
    pub(crate) tab: Option<String>,

    /// The browser context to act in, e.g. c1 (see `chrome-cdp context`):
    /// its tabs, cookies, routes and mocks. Default: the session's own.
    #[arg(long, global = true, env = "CHROME_CDP_CONTEXT", value_name = "ID")]
    pub(crate) context: Option<String>,

    /// Log to stderr: -v info, -vv debug, -vvv trace. CHROME_CDP_LOG
    /// (e.g. cdp=trace) overrides it.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub(crate) verbose: u8,

    /// `base_url` from the config files, for relative URLs (`goto /login`).
    #[arg(skip)]
    pub(crate) base_url: Option<String>,

    /// The attribute `testid=` locators read (`testid_attribute` in the
    /// config files, or `CHROME_CDP_TESTID_ATTRIBUTE`); `data-testid` if
    /// `None`.
    #[arg(skip)]
    pub(crate) testid_attribute: Option<String>,

    /// `snap_after_action = "diff"` in the config files: actions add
    /// `snapshot --diff -i` unless `--no-snap`.
    #[arg(skip)]
    pub(crate) snap_after_action: bool,

    /// `node_stacks` in the config files: `open --node-stacks`' default.
    #[arg(skip)]
    pub(crate) node_stacks: Option<engine::NodeStacks>,

    /// `snapshot_budget` in the config files: `snapshot --budget`'s
    /// default.
    #[arg(skip)]
    pub(crate) snapshot_budget: Option<usize>,

    /// The project root places in the page's code print relative to:
    /// `source_root` in the config files, else the nearest directory up
    /// with a project config or `.git`, else the current one.
    #[arg(skip)]
    pub(crate) source_root: Option<PathBuf>,
}

/// The timeout of navigation and of commands without their own default.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The timeout of `wait` (and plan 05's actions).
pub(crate) const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

impl GlobalArgs {
    /// `--timeout` (or its env var or config key), else `default`.
    pub(crate) fn timeout_or(&self, default: Duration) -> Duration {
        self.timeout.unwrap_or(default)
    }

    /// How to render results.
    pub(crate) fn mode(&self) -> Mode {
        if self.json {
            Mode::Json
        } else if self.quiet {
            Mode::Quiet
        } else {
            Mode::Text
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Print the JSON Schema of a command's --json output.
    #[command(after_help = "\
Examples:
  chrome-cdp schema                 # list the commands that have one
  chrome-cdp schema version         # the schema of chrome-cdp --version --json
  chrome-cdp schema scenario        # .cdp directives, commands, flags, variables
  chrome-cdp schema mocks           # the mock file format
  chrome-cdp schema audit-baseline  # an audit baseline file")]
    Schema(SchemaArgs),

    /// Download Chrome for Testing into chrome-cdp's cache.
    #[command(after_help = "\
The cache is $CHROME_CDP_CACHE, else ~/.cache/chrome-cdp (Linux) or
~/Library/Caches/chrome-cdp (macOS). --timeout bounds connecting and each
wait for data, not the whole download. CHROME_CDP_CFT_URL points at a
mirror of the Chrome for Testing index.

Examples:
  chrome-cdp install                       # the current stable Chrome
  chrome-cdp install --headless-shell      # smaller, headless only
  chrome-cdp install --channel beta
  chrome-cdp install --version 154         # the newest 154.x
  chrome-cdp install --with-deps           # Linux: also the system libraries
  chrome-cdp install --list")]
    Install(InstallArgs),

    /// Remove a browser installed by `chrome-cdp install`.
    #[command(after_help = "\
Examples:
  chrome-cdp install --list                # shows the ids
  chrome-cdp uninstall stable-154.0.8037.57-linux64")]
    Uninstall(UninstallArgs),

    /// Check the setup: Chrome, its libraries and sandbox, the session dir,
    /// macOS quarantine and version skew; prints a fix for each problem.
    #[command(after_help = "\
Each check prints ok, warn, FAIL or skip, and a fix for anything not ok.
Unless --no-launch is given, doctor starts the Chrome it finds (headless,
a temporary profile) and closes it again: the one sure test of its
libraries and sandbox. It exits 4 when a check fails, 0 otherwise.

Examples:
  chrome-cdp doctor
  chrome-cdp doctor --no-launch                 # only look, start nothing
  chrome-cdp doctor --chrome /usr/bin/chromium
  chrome-cdp doctor --json                      # every check, with its fix")]
    Doctor(DoctorArgs),

    /// Replace this binary with the newest release (or --version's).
    #[command(after_help = "\
upgrade downloads the release archive for this OS and CPU from GitHub,
checks its SHA-256 against the release's manifest.json, checks that the new
binary runs, then renames it over this one. Sessions still running the old
build restart with the new one on their next command.

A binary installed by Homebrew, npm or cargo is left alone: upgrade names
the command of that package manager instead. CHROME_CDP_RELEASES_URL
points at a mirror of https://github.com/glotlabs/chrome-cdp/releases.

Examples:
  chrome-cdp upgrade
  chrome-cdp upgrade --check          # is there a newer release?
  chrome-cdp upgrade --version 1.2.0  # that release, even an older one")]
    Upgrade(UpgradeArgs),

    /// Start the session's browser (or check the running one) and
    /// optionally open a URL.
    #[command(after_help = "\
Each session (--session, default `default`) has its own background daemon
and browser; later commands reuse them. Running open again is a no-op,
unless it asks for other launch options: add --restart for those.
The daemon closes the browser after --idle-timeout without commands.
An --extension is loaded before the URL; open prints its id, so its pages
(popup, options) open with goto chrome-extension://<id>/popup.html.

Examples:
  chrome-cdp open https://example.com
  chrome-cdp open --headed                          # with a window
  chrome-cdp open --profile work https://app.test   # keeps logins
  chrome-cdp open --session shop http://localhost:3000
  chrome-cdp open --device 'iPhone 15' http://localhost:3000
  chrome-cdp open --init-script ./stub-analytics.js http://localhost:3000
  chrome-cdp open --restart --chrome-arg=--window-size=1920,1080
  chrome-cdp open --follow-popups                   # popups become the active tab
  chrome-cdp open --dialogs dismiss                 # answer every dialog with Cancel
  chrome-cdp open --downloads-dir ./downloads       # downloads land there
  chrome-cdp open --http-credentials ada:s3cret http://localhost:8080/admin
  chrome-cdp open --node-stacks on https://staging.app.test   # record who made each node
  chrome-cdp open --extension ./my-extension         # prints its chrome-extension:// origin")]
    Open(OpenArgs),

    /// Attach the session to a browser that is already running.
    #[command(after_help = "\
The browser must run with --remote-debugging-port. `close` then only
detaches; the browser keeps running. The port has no authentication or
encryption: keep it on 127.0.0.1. connect warns when the browser is on
another host.

Examples:
  chrome-cdp connect 9222
  chrome-cdp connect http://127.0.0.1:9222 --session mine
  chrome-cdp connect ws://127.0.0.1:9222/devtools/browser/<id>")]
    Connect(ConnectArgs),

    /// Close the session: its daemon, and the browser it launched.
    #[command(after_help = "\
Examples:
  chrome-cdp close
  chrome-cdp close --session shop
  chrome-cdp close --all")]
    Close(CloseArgs),

    /// Show the session's state, browser, tabs and uptime.
    #[command(after_help = "\
Examples:
  chrome-cdp status
  chrome-cdp status -q            # just the state: running, crashed, idle or stopped
  chrome-cdp status --logs        # also the end of Chrome's log")]
    Status(StatusArgs),

    /// List the running sessions.
    #[command(after_help = "\
Examples:
  chrome-cdp sessions
  chrome-cdp sessions -q          # one name per line")]
    Sessions(SessionsArgs),

    /// List the session's tabs; `*` marks the active one.
    #[command(after_help = "\
Tabs have short ids (t1, t2, …) in the order they opened, never reused.
Commands act on the active tab unless --tab names another. A popup opens a
new tab but does not become active (unless `open --follow-popups`). The
active tab stays in front, visible and not throttled; the others are hidden.

Examples:
  chrome-cdp tabs
  chrome-cdp tabs -q              # one id per line")]
    Tabs(TabsArgs),

    /// Switch to a tab, or open or close one.
    #[command(
        args_conflicts_with_subcommands = true,
        arg_required_else_help = true,
        after_help = "\
Examples:
  chrome-cdp tab t2                        # make t2 the active tab
  chrome-cdp tab new https://example.com   # open a tab and switch to it
  chrome-cdp tab close                     # close the active tab
  chrome-cdp tab close t3"
    )]
    Tab(TabArgs),

    /// List the tab's frames and workers as a tree.
    #[command(after_help = "\
One line per frame, indented under its parent, in the order of the <iframe>
elements: `frame main URL` for the page, then `iframe \"name\" URL`, marked
`(out of process)` for a cross-site iframe that runs in its own process.
Each frame's dedicated workers follow its iframes; the shared and service
workers of the origins the tab's frames are on come last. --frame and
frame= take a frame's name, a part of its URL, or its id (in --json, or
one per line with -q).

Examples:
  chrome-cdp frames
  chrome-cdp frames --json | jq -r '.data.frames[].url'
  chrome-cdp eval --frame \"$(chrome-cdp frames -q | sed -n 2p)\" 'document.title'")]
    Frames(FramesArgs),

    /// Make, list or close browser contexts: isolated cookies, storage,
    /// routes and mocks in one browser.
    #[command(after_help = "\
A context is like an incognito window: its own cookies, storage and
cache, plus its own routes, mock sets, init scripts, emulation and
permissions, none of which it shares with the session's default context
or other contexts.
Contexts have ids (c1, c2, …) in the order they were made, never reused.
Any command takes --context <id> (or CHROME_CDP_CONTEXT) to act in one; a
page command then acts on the context's active tab. `chrome-cdp test`
runs each test in a fresh context.

Examples:
  chrome-cdp context new /login                 # c1, with a tab at base_url/login
  chrome-cdp --context c1 fill label=Email a@example.com
  chrome-cdp context new -q                     # prints just the id
  chrome-cdp context list
  chrome-cdp context close c1
  chrome-cdp context close --all")]
    Context(ContextArgs),

    /// Run the project's E2E scenarios, each test in a fresh browser
    /// context with its mocks.
    #[command(after_help = "\
Finds the scenarios by the `tests` globs of [e2e] in chrome-cdp.toml
(default e2e/scenarios/**/*.cdp), starts the [e2e.web_server]s unless
something already answers on their URLs, and opens a browser session of
its own. Each command the config runs (a web server's, a backend's setup
and teardown) is first shown on stderr: `running: <command> (from
<config path>)`. Every test gets a fresh browser context: its own cookies,
storage, routes and mock sets (default_mocks, then the file's and the
test's @mocks), so tests cannot leak into each other. Each also sees the
same time (2026-01-01T10:00:00Z), random values, locale (en-US), time
zone (UTC) and no animations; [e2e.determinism] changes that, and `time
advance 5m` moves the time on and runs the timers due by then. `--jobs N`
runs N tests at once in one Chrome; a test that fails, then passes on a
retry, is reported as flaky; `--repeat 20` runs each test 20 times to
find flaky ones. A flaky test's line names the likely cause (acted
before the page settled, requests raced, a moving target, different
requests, time or randomness, shared state), with the request or element
and a fix, from comparing its failing and passing runs. A request no mock answers, in a set's scope with
unmatched = \"fail\", fails the test at the step after which it was
sent, naming the closest mocks; so does a console error in any of its
tabs (unless @allow-console-errors, or allow_console_errors = true in
[e2e]). Exit code 0: every test passed; 1: a test failed.
Each test leaves steps.jsonl, requests.txt and console.txt in
e2e/.results/<file>/<test>/; a failed one also failure.png, snapshot.txt
and report.md (the step, the error, the page around it, console errors,
failed requests), whose path is printed under its line. --trace adds a
screenshot after every step; --reporter json,junit,html writes those
files too (docs/e2e/debugging.md).
A scenario that does not check (bad syntax, unknown command, missing
@include) fails before any test runs, with exit code 2.

Backends (docs/e2e/backends.md): --backend NAME runs the same scenarios
against a real backend of [e2e.backends.NAME] (its base_url, web_server,
setup commands and vars); the mock sets are off but for the backend's
own `mocks`, and time is not frozen. `--backend all` runs every backend
in turn. @backend mock (or real, or a name) limits a test or file to
those backends; the others skip it, with the reason. ${var:name} comes
from the backend's vars file, ${secret:NAME} from the environment
(masked in all output), ${test.id} is unique per test, and `http POST
/api/items --save item` arranges data (${saved.item.id}). --verify-mocks
compares every real answer with the mock that would have given it, and
reports mocks whose JSON shape drifted (--update fixes their data
files); --record-mocks writes each passed test's requests as a mock set.
A protected backend runs only @safe tests unless --allow-writes.

Healing (docs/e2e/healing.md): a passed test keeps what the element each
step's locator found looks like in e2e/fingerprints/ (commit it;
[e2e] fingerprints = \"auto\" writes them unless CI is set). When a
locator later finds nothing, the failure names the element that
fingerprint matches best now, with its confidence. --heal runs the step
with it when the match is high (--heal=medium: medium too), marks the
step healed, and writes the passed tests' changes to heal.patch in the
artifacts folder; --heal --write applies it to the scenario files.

UI coverage (docs/e2e/coverage.md): --ui-coverage reads the controls of
each page the tests visit (the elements `snapshot -i` gives refs) and
prints, per page, how many a step acted on and those none did, and the
mocks no request matched; ui-coverage.json in the artifacts folder has
it all. [e2e] pages = [\"/products/:id\", …] groups by those patterns
and names the listed pages no test visited.

Repro bundles (docs/e2e/repro.md): `test DIR`, where DIR has a
chrome-cdp.toml of its own (as `repro save` writes), runs DIR's project
as if started there: its tests, mocks and state, offline.

Scenario syntax (docs/e2e/scenarios.md):
  @mocks base checkout          # file settings: @mocks @tags @skip @only
  @before                       #   @timeout @viewport @device @retries
    goto /                      #   @allow-console-errors
  @test \"adds an item\" @tags smoke
    click text=Add
    expect text=1 item visible
    expect request POST /api/cart --json-body '{\"qty\":1}'
  @test \"logs in\"
    @include ../steps/login.cdp email=a@example.com

Examples:
  chrome-cdp test
  chrome-cdp test e2e/scenarios/checkout.cdp:12     # the test at line 12
  chrome-cdp test --grep cart --tags smoke,!slow
  chrome-cdp test --jobs 4 --shard 2/4 --retries 1
  chrome-cdp test e2e/scenarios/cart.cdp --repeat 20   # flaky before a commit?
  chrome-cdp test --debug      # keep the first failed test's page open
  chrome-cdp test --watch      # run the affected tests again on each change
  chrome-cdp test --reporter text,junit,html --trace
  chrome-cdp test --backend local              # against the real API
  chrome-cdp test --backend staging --verify-mocks
  chrome-cdp test --heal       # after a redesign: heal broken locators
  chrome-cdp test --heal --write   # and fix them in the files
  chrome-cdp test --ui-coverage    # what the suite never clicks
  chrome-cdp test bugs/cart-total  # a repro bundle, offline
  chrome-cdp test --list")]
    Test(TestArgs),

    /// The commands the session ran; --as-scenario writes them as a test.
    #[command(after_help = "\
The session keeps its last 1,000 commands (from any shell, `batch` and
`run`), each with whether it succeeded. --as-scenario NAME prints them as
a test for a scenario file, the core authoring workflow: explore with the
usual commands, then save what worked.

In the test, refs (e12) become the most robust locator that found their
element alone when the command ran: role and name, then label, test id
(testid=…, read with the config's testid_attribute, as the project's
`test` reads it), placeholder or text, then one of those inside
a named container (a list item, a row, a dialog), and `>> nth=` only as
a last resort. An element in an iframe gets frame=NAME (or a part of its URL only it has) first:
frame=pay >> role=button[name=\"Pay\"]. Commands that only read (snapshot, console, text, network, …) and failed ones are left
out; `expect` commands stay; `open URL` becomes `goto`; URLs under
base_url become paths; `mock use SETS` becomes `@mocks SETS`. --from N
starts at command N (see `history`), and --clear starts the history over
(after printing it).

Examples:
  chrome-cdp history
  chrome-cdp history --as-scenario \"adds item to cart\" >> e2e/scenarios/cart.cdp
  chrome-cdp history --as-scenario checkout --from 12
  chrome-cdp history --clear -q")]
    History(HistoryCmdArgs),

    /// Pack what the session did into a bundle that replays it offline.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
A repro bundle is a small project that replays a bug found in a session,
with no server: `repro start` marks the start (it keeps the cookies and
storage, the time, and from then on the response bodies of the session's
requests, documents and scripts included); reproduce the bug; `repro save
DIR` writes the bundle. `chrome-cdp test DIR` replays it from its own mocks
and fails the way the session did; once the bug is fixed it passes, and it
is a regression test. docs/e2e/repro.md has the layout.

Examples:
  chrome-cdp repro start
  chrome-cdp click 'role=button[name=\"Pay\"]' && chrome-cdp expect role=status text Paid
  chrome-cdp repro save bugs/pay-twice --name \"pays twice\"
  chrome-cdp test bugs/pay-twice"
    )]
    Repro(ReproArgs),

    /// Record what happens in the page as test steps.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
`record start` listens to the page itself, in every tab and iframe of the
session (or --context), so it records whoever acts: a person in a headed
window, an agent's chrome-cdp commands, or raw `cdp Input.*` events.
`record stop` prints the recorded test (or appends it to -o FILE): clicks,
checks, typing (one `fill` per field), choices, files, keys, browser
navigations, dialogs answered and tab switches, each element by the same
stable locators `history --as-scenario` writes. It guesses no assertions:
add `expect` steps. docs/e2e/recording.md has the details.

Examples:
  chrome-cdp record start --name \"adds a mouse to the cart\"
  chrome-cdp record status
  chrome-cdp record stop -o e2e/scenarios/cart.cdp"
    )]
    Record(RecordArgs),

    /// Load a URL in the current tab and wait for it.
    #[command(after_help = "\
Starts the session's browser first if it has none. Without a scheme,
https:// is added (http:// for localhost and IPs). A relative URL (/login,
./next, ?q=1, #top) resolves against base_url from the config
(.chrome-cdp.toml), else against the current page. Prints the HTTP status,
final URL (after redirects), title and time taken; a 4xx/5xx status is a
warning. A page that cannot load (DNS, connection refused, certificate …)
fails with exit code 1 and says why.

Examples:
  chrome-cdp goto https://example.com
  chrome-cdp goto localhost:3000/login
  chrome-cdp goto /settings                      # with base_url, or on the same site
  chrome-cdp goto example.com --wait domcontentloaded
  chrome-cdp goto file:///tmp/report.html
  chrome-cdp goto https://example.com --tab t2 -q   # prints the final URL")]
    Goto(GotoArgs),

    /// Go back one page in the tab's history.
    #[command(after_help = "\
At the start of the history it says so and exits 0. Going back to a
pushState entry of a single-page app is a same-document navigation.

Examples:
  chrome-cdp back
  chrome-cdp back --tab t2 --wait commit")]
    Back(HistoryArgs),

    /// Go forward one page in the tab's history.
    #[command(after_help = "\
At the end of the history it says so and exits 0.

Examples:
  chrome-cdp forward
  chrome-cdp forward --json")]
    Forward(HistoryArgs),

    /// Load the current page again.
    #[command(after_help = "\
Examples:
  chrome-cdp reload
  chrome-cdp reload --hard          # bypass the cache
  chrome-cdp reload --wait domcontentloaded")]
    Reload(ReloadArgs),

    /// Wait for the page: an element, text, the URL, network idle, a request, …
    #[command(after_help = "\
Give exactly one condition. It is checked against the active tab (or
--tab) until it holds or --timeout (default 10s) passes; a timeout
(exit 3) says what the page showed instead. Needs a running session.

  <LOCATOR>           an element: a ref (e12), #id, css=…, text=…,
                      role=button[name=\"…\"], label=…, frame=… >> …;
                      --state visible (default), hidden, attached,
                      detached or enabled
  --text, --text-gone visible text anywhere, iframes included (or only
                      --in <LOCATOR>); spaces are collapsed, case counts
  --url               the page URL: a part of it, a glob (* and **) or
                      /regex/
  --load, --domcontentloaded, --idle (no requests for 500ms, not counting
                      WebSockets, beacons and requests open over 10s),
  --settle            (network idle and no DOM changes for 300ms)
  --fn                a JavaScript expression that becomes truthy
  --request, --response [--status]  since the previous command began
  --download          a download started since the previous command began
                      has finished (prints its path with -q)
  --console <REGEX>   a console entry (any level or source) whose text or
                      URL matches, logged since the previous command
                      began; prints it as `console` does
  --binding <NAME>    a call of a function `expose NAME` put in the page,
                      since the previous command began
  --ws <PATTERN> [--frame <REGEX>]  a frame sent or received on a WebSocket
                      whose URL matches, since the previous command began
  --hmr               the dev server's hot update (Vite, webpack, Next.js)
                      since the previous command began, or the next one:
                      after an edit, whether the page has it. Applied or a
                      full reload (once loaded) succeed; a compile error
                      fails with HotUpdateFailed and its place; a page with
                      no hot-reload client fails at once
  <MS>                a fixed sleep, e.g. 500 or 2s (prefer a condition)

Examples:
  chrome-cdp wait '#results'
  chrome-cdp wait 'role=button[name=\"Save\"]'
  chrome-cdp wait e12 --state detached
  chrome-cdp wait .spinner --state hidden
  chrome-cdp wait --text 'Welcome back'
  chrome-cdp wait --text-gone Loading --in '#main'
  chrome-cdp wait --url /dashboard
  chrome-cdp wait --idle
  chrome-cdp wait --fn 'window.app && window.app.ready'
  chrome-cdp wait --response '**/api/save' --status 200
  chrome-cdp wait --download
  chrome-cdp wait --console 'checkout (done|failed)'
  chrome-cdp wait --binding onCheckout      # after expose onCheckout
  chrome-cdp wait --ws /live --frame '\"type\":\"price\"'
  chrome-cdp wait --hmr                     # after saving src/cart.ts")]
    Wait(WaitCmdArgs),

    /// Assert something about the page, retrying until it holds: an element's state, text, value, count, …
    #[command(after_help = "\
Checks the active tab (or --tab) again on every animation frame until the
assertion holds or --timeout (default 5s) passes, like Playwright's
expect(). A failure exits 1 (ExpectFailed) and says what it expected, what
it saw, the element (with a ref) and what to run next. --not inverts
element matchers, --url and --title. Needs a running session.

Element matchers (expect <LOCATOR> <MATCHER> [VALUE…]):
  visible, hidden, attached, detached, enabled, disabled, editable,
  checked, unchecked, focused, in-viewport
  text <TEXT>          its text contains TEXT (all of it with --exact);
                       case counts, spaces are collapsed
  value <V>, values <V…>   a field's value; a multiple select's values
  attr <NAME> [V], class <NAME>, css <PROPERTY> <V>
  count <N> (or --min/--max)   how many elements match
  A TEXT or V written /regex/flags is a regex. Matchers that look at one
  element fail when several match (add >> nth=0, or use a ref).

Page checks:
  --url, --title       retried like the matchers
  --no-errors          no uncaught exception or console.error since the
                       page loaded (or --since <mark>; see `mark`; or
                       --all-tabs: in any tab since it opened)
  --no-issues          no issue of Chrome's (`issues`) of severity warning
                       or error since then (--min-severity error|info;
                       --since, --all-tabs as for --no-errors)
  --no-failed-requests no request failed or got a 4xx/5xx since then
                       (--origin self: only to the page's origin; the
                       browser's own /favicon.ico does not count;
                       --all-tabs: in any tab since it opened)
  --a11y [--level A]   26 built-in accessibility rules (WCAG A and AA:
                       names, labels, ARIA roles and attributes, headings,
                       lists, tables, contrast, target size, focus), in
                       the page and its iframes, or the element LOCATOR
                       names. focus-visible (AA) focuses each element and
                       gives focus back; docs/accessibility.md lists them
  --perf BUDGETS       `perf`'s numbers within every budget, e.g.
                       'lcp<2.5s cls<0.1 tbt<200': <metric><op><value>,
                       op <, <=, > or >=; ttfb, fcp, dcl, load, lcp, inp,
                       tbt in ms (or 2.5s), cls a score, requests and
                       elements counts, transfer and heap in bytes (or kb,
                       mb, gb); `good` is lcp<=2500 cls<=0.1 inp<=200. Waits
                       for load, then reads once; the failure lists every
                       budget and the throttling. A metric not measured
                       yet (inp before an interaction) fails, unless
                       --perf-missing ok
From --no-errors to --perf, they look once: what they find does not go
away by waiting.
  --layout --baseline NAME   no error finding of `layout` (in the page, or
                       LOCATOR's element) that .chrome-cdp/layout/NAME.txt
                       does not list; retried like the matchers. --update
                       writes the findings to it; layout's --viewports,
                       --only, --full, --min-target and --stretch-text apply
  --audit [--baseline NAME|FILE]   no finding of `audit` (Chrome's issues,
                       accessibility, layout, errors, failed requests,
                       security, metadata) at --min-severity (warning) or
                       above that the baseline does not accept; without
                       one, none at all. --category picks, LOCATOR scopes.
                       Waits for load, then looks once; the failure is the
                       report of the new findings. NAME is
                       .chrome-cdp/audit/NAME.json (e2e/audit/ under
                       `test`); a missing one is written, except in CI

Mock state (expect mock-state <NAME> <CHECK>), retried like the matchers:
  count <N> (or --min/--max)   items of a collection (or a list value)
  value <V>            the named value is V (JSON, else text)
  contains <V>         some item has V's fields (`{\"title\":\"Milk\"}`)

Requests sent (expect request <METHOD> <PATH>), from the network logs of
every tab of the session (or --context; or --tab), mocked or not:
  PATH                 a path glob (/api/orders/*, **/orders), a full-URL
                       glob, or *; METHOD may be * (any)
  --query k=v, --header 'Name: value', --json-body '{\"qty\":2}'
                       conditions, as a mock's match (values are globs)
  --count N | --min N | --max N   how many (default: at least 1)
  --since <mark>       only those sent after the mark
Retried until enough were sent; too many fails at once. `expect
no-request <METHOD> <PATH>` checks none was sent. A failure lists the
matching requests and the closest others, with the condition each fails.

Examples:
  chrome-cdp expect e6 visible
  chrome-cdp expect 'role=alert' text Saved
  chrome-cdp expect '#email' value alice@example.com
  chrome-cdp expect '.row' count 3
  chrome-cdp expect '.spinner' visible --not
  chrome-cdp expect 'role=button[name=\"Save\"]' enabled --timeout 10s
  chrome-cdp expect css=h1 text '/^Welcome, \\w+$/'
  chrome-cdp expect --url '**/dashboard'
  chrome-cdp expect --no-errors --since before-save
  chrome-cdp expect --no-failed-requests --all-tabs
  chrome-cdp expect --no-issues --min-severity error
  chrome-cdp expect --a11y --level A
  chrome-cdp expect --perf 'lcp<2.5s cls<0.1 tbt<200'
  chrome-cdp expect --layout --baseline cart --viewports mobile,desktop
  chrome-cdp expect --audit --baseline checkout
  chrome-cdp expect mock-state todos count 4
  chrome-cdp expect request POST /api/orders --json-body '{\"qty\":2}' --count 1
  chrome-cdp expect no-request DELETE '/api/cart/**'")]
    Expect(ExpectArgs),

    /// Name this point in the tab's console, issues and network logs, for expect --no-errors --since.
    #[command(after_help = "\
A mark belongs to the tab (the active one, or --tab). Setting a name again
moves it. `expect --no-errors --since <name>`, `expect --no-issues --since
<name>` and `expect --no-failed-requests --since <name>` look only at what
happened after it, as do `console`, `errors`, `issues` and `network` with
`--since <name>`. A name starts with a letter: letters, digits, -, _ and .
follow.

Examples:
  chrome-cdp mark before-save
  chrome-cdp click 'role=button[name=\"Save\"]'
  chrome-cdp expect --no-errors --since before-save
  chrome-cdp network --since before-save")]
    Mark(MarkArgs),

    /// Compare the page's snapshot with a stored text baseline; a diff shows what changed.
    #[command(after_help = "\
Reads the active tab (or --tab) as `snapshot -i` shows it, without refs
(and long lists in full), and compares it with the baseline
.chrome-cdp/snapshots/<NAME>.txt (in `test`, and in `run` of a file of an
E2E project, e2e/snapshots/<NAME>.txt), reading again until they match or
--timeout (default 5s) passes. A failure exits 1 (ExpectFailed) with a
unified diff: - lines are the baseline's, + lines the page's.

--update stores the page as the baseline instead, once two reads in a row
are alike. A first run with no baseline stores it too, unless the CI
environment variable is set: then a missing baseline fails. Commit the
files. They live in .chrome-cdp/ of the nearest directory up with
.chrome-cdp.toml, .chrome-cdp/ or .git (else the current one); --dir puts
them elsewhere. Needs a running session.

Edit a baseline into a template for what changes from run to run:
  button \"*\"                 any name (or text, for a text line)
  heading /Order #\\d+/ (h1)  a name the regex finds a match in (/…/i:
                             any case); the rest of the line as it is
  textbox \"At\" value=\"*\"     a field's value= takes \"*\" and /…/ too
  link \"Invoice\" → \"*\"       any href
  ...                        any number of lines at its depth, with the
                             lines below them
A failure's diff shows the template's lines where they match. --update
keeps the template lines that still match and rewrites the rest; it
never writes a template itself.

Examples:
  chrome-cdp expect-snapshot login
  chrome-cdp expect-snapshot checkout/cart --scope 'role=main'
  chrome-cdp expect-snapshot login --update
  chrome-cdp expect-snapshot home --full --timeout 10s
  chrome-cdp expect-snapshot orders    # a baseline with /…/, \"*\" and ...")]
    ExpectSnapshot(ExpectSnapshotArgs),

    /// Compare a screenshot with a stored PNG baseline, pixel by pixel (experimental).
    #[command(after_help = "\
Takes a screenshot of the active tab's viewport (or the --full page, or the
element LOCATOR names) in CSS pixels and compares it with the baseline
.chrome-cdp/screenshots/<NAME>-<os>.png (in `test`, and in `run` of a file
of an E2E project, e2e/snapshots/<NAME>-<os>.png), taking it again until it
matches or --timeout (default 5s) passes. A pixel has changed when its colour moved
more than --threshold (0 to 1); up to --max-diff-pixels may change. A
failure exits 1 (ExpectFailed) and saves the screenshot and a diff image
(changes in red) where `screenshot` saves its files.

Experimental: fonts render differently on each OS (so each OS has its own
baseline) and may change with Chrome's version. expect-snapshot is the
sturdier check. For the capture the caret is hidden, animations and
transitions are finished and endless ones stopped at their start (as
Playwright does). --mask paints over what changes on every load (clocks,
ads). --update, a first run, CI and --dir work as in expect-snapshot.
Needs a running session.

Examples:
  chrome-cdp expect-screenshot home
  chrome-cdp expect-screenshot logo 'role=img[name=\"Acme\"]'
  chrome-cdp expect-screenshot dashboard --full --mask '.clock'
  chrome-cdp expect-screenshot home --update")]
    ExpectScreenshot(ExpectScreenshotArgs),

    /// Show the page as an accessibility tree, with refs (e12) to act on.
    #[command(after_help = "\
One node per line, two spaces per level:
  - role \"name\" [ref] (hN) (password|file) value=\"…\" states… → href
Refs go on what can be acted on (controls, links, clickable elements,
scrollable boxes, iframes); a node keeps its ref while it stays in the
page. Hidden content is left out, lists longer than 30 collapse, and text
is cut at --text-limit. Needs a running session.

A big page: --budget 2000 folds parts of it (runs of cards or rows, then
big lists, tables and sections) until it is about 2,000 tokens; --fold
folds every run of four or more alike. A folded part is one line with a
region (r4); its refs still work, and --expand r4 shows it.

Examples:
  chrome-cdp snapshot
  chrome-cdp snapshot -i                  # only what can be acted on
  chrome-cdp snapshot --scope e12         # one part of the page
  chrome-cdp snapshot --scope '#results' --depth 3
  chrome-cdp snapshot --scope 'frame=payment'
  chrome-cdp snapshot --diff              # what changed since the last one
  chrome-cdp snapshot -i --source         # and where each came from
  chrome-cdp snapshot --budget 2000       # a big page in ~2,000 tokens
  chrome-cdp snapshot --fold              # one card, then \"… 59 more\"
  chrome-cdp snapshot --expand r4 r7      # parts it folded")]
    Snapshot(SnapshotArgs),

    /// Search the page for elements by role, name or text; shows refs.
    #[command(after_help = "\
Every word must match, in any order: a part of a node's name, value or
text (any case), its role (button, link, textbox, …), or, for words of
five letters or more, a word of the name one typo away. The best matches
come first, as snapshot lines with refs and the named container each is
in. Cheaper than a whole snapshot when you know what you want. Needs a
running session.

Examples:
  chrome-cdp find checkout
  chrome-cdp find 'sign in' --role button
  chrome-cdp find email textbox
  chrome-cdp find delete --scope e40        # only inside e40
  chrome-cdp find delete --limit 0          # every match
  chrome-cdp find checkout -q               # only the refs")]
    Find(FindArgs),

    /// Print the locator a test should use for one element (a ref's stable locator).
    #[command(after_help = "\
Refs last only as long as the session; a committed script or test needs a
locator. This prints the most robust one that finds the element and
nothing else, as `history --as-scenario` writes it: role and name, then
label, test id, placeholder or text; then one of those inside a unique
container (a row, a list item, a dialog); then `>> nth=` as a last
resort. An element in an iframe gets a frame= step first. --all lists
every candidate tried, best first, with how many elements each finds (1
is usable). testid= reads --testid-attribute. An element not there yet is
waited for (up to --timeout, 10s by default). Needs a running session.

Examples:
  chrome-cdp locator e12
  chrome-cdp locator e12 --all              # every candidate, with counts
  chrome-cdp locator 'text=Delete >> nth=1'
  chrome-cdp locator e12 -q                 # only the locator")]
    Locator(LocatorArgs),

    /// Print the visible text of the page, or of every element a locator matches.
    #[command(after_help = "\
The rendered text (innerText): hidden content left out, runs of spaces
collapsed, lines trimmed, at most one empty line in a row. With a locator
that matches several elements, one text per match, in page order. Without
a locator, each visible iframe's text (cross-site ones too) is where its
<iframe> sits, between [iframe \"name\"] (or its URL) and [/iframe] lines. A
locator that names a frame (frame=payment, or an iframe's ref) reads that
frame's document only. A locator that matches nothing yet is waited for
(up to --timeout, 10s by default). Needs a running session.

Examples:
  chrome-cdp text
  chrome-cdp text e12
  chrome-cdp text '#results'
  chrome-cdp text 'role=listitem'           # one line per item
  chrome-cdp text frame=payment")]
    Text(ReadTextArgs),

    /// Print the HTML of the document or of an element, pretty-printed.
    #[command(after_help = "\
An element's content (its innerHTML) unless --outer; without a locator,
the whole document. One element per line, two spaces per level, text
collapsed, and long attribute values, comments and script or style bodies
cut; open shadow roots show as #shadow-root. --raw prints the browser's
own serialisation instead. An element not there yet is waited for (up to
--timeout, 10s by default). Needs a running session.

Examples:
  chrome-cdp html e12
  chrome-cdp html '#cart' --outer
  chrome-cdp html                           # the whole document
  chrome-cdp html css=form --raw")]
    Html(HtmlArgs),

    /// Print an attribute of an element.
    #[command(after_help = "\
Prints nothing (and a warning naming the attributes it has) when the
element has no such attribute; --json has \"value\": null. For a
field's current value use `value`. An element not there yet is waited
for (up to --timeout, 10s by default). Needs a running session.

Examples:
  chrome-cdp attr e12 href
  chrome-cdp attr '#avatar' src
  chrome-cdp attr 'role=button[name=\"Save\"]' aria-disabled")]
    Attr(AttrArgs),

    /// Print a field's current value: input, textarea, select, editable text.
    #[command(after_help = "\
A select prints the chosen option's value (a multiple select, and a file
input's file names, one per line); an editable element its text; a slider,
spin button, progress bar or meter its value. An element not there yet
is waited for (up to --timeout, 10s by default). Needs a running session.

Examples:
  chrome-cdp value e3
  chrome-cdp value 'label=Email'
  chrome-cdp value '#country'")]
    Value(ValueArgs),

    /// Count the elements a locator matches (0 is not an error).
    #[command(after_help = "\
Prints the count, and how many are visible when that differs:
`4 (3 visible)`. -q prints only the count. It does not wait: 0 is an
answer (`wait LOCATOR` or `expect LOCATOR count` wait). Needs a running
session.

Examples:
  chrome-cdp count '.cart-item'
  chrome-cdp count 'role=row'
  chrome-cdp count 'text=Out of stock' -q")]
    Count(CountArgs),

    /// Print an element's box in the viewport: x, y, width and height.
    #[command(
        name = "box",
        after_help = "\
CSS pixels from the top-left corner of the tab's viewport (iframes'
offsets added), rounded to hundredths: `x=24 y=310 w=120 h=36`, with
`(outside the 1280x720 viewport)` when scrolled out of view. Prints
nothing and a warning when the element is not rendered. -q prints the
four numbers. An element not there yet is waited for (up to --timeout,
10s by default). Needs a running session.

Examples:
  chrome-cdp box e12
  chrome-cdp box '#checkout' -q"
    )]
    BoundingBox(BoxArgs),

    /// Explain why an action on an element would wait or fail, check by
    /// check, without acting.
    #[command(after_help = "\
Runs the checks the action needs (--for, default click) once, now, and
prints each with its cause: resolve (no match, or which elements an
ambiguous locator matches), visible (display: none on which ancestor,
visibility, opacity: 0, an empty box), stable (how far it moved between
two frames, and the animations on it), enabled (disabled, a disabled
fieldset, aria-disabled, inert, a modal dialog open elsewhere), editable
(fill, type), in view (which container scrolled, or what clips it) and
hit test (what covers it and the button that likely dismisses it, or
pointer-events: none). The last line says what the action would do:
act now, wait until the timeout, or fail at once. Elements are named as
the snapshot names them, with refs. It scrolls the element into view as
the action would; --no-scroll leaves the page where it is and skips the
hit test when the element is out of view. It does not act, wait or
retry, and exits 0 whatever it finds. -q prints the last line. Needs a
running session.

Examples:
  chrome-cdp why e12
  chrome-cdp why '#email' --for fill
  chrome-cdp why 'role=button[name=\"Pay\"]' --no-scroll")]
    Why(WhyArgs),

    /// List the page's forms: each field's name, kind, state, value and
    /// validation message.
    #[command(after_help = "\
Lists every <form>, role=form and role=search, in every frame, and last
the fields outside any form; with a LOCATOR, only the form it is or is
inside (else the forms in it, else the fields in it). Each field is named
as the snapshot names it, with its ref, then its kind (an input's type:
email, password, date, submit), required, disabled, readonly, its value
(never a password's; a file input's files), the first 10 options of a
select, listbox or combobox, and why it is invalid: its validation
message (not for a required field that is only empty), else the text
aria-errormessage or aria-describedby points to. Radios are one line per
group. A form's header says where it submits and how many required
fields are empty; -q prints that count. Needs a running session.

Examples:
  chrome-cdp forms
  chrome-cdp forms e10
  chrome-cdp forms 'role=search' --json")]
    Forms(FormsArgs),

    /// Lint what the page looks like: covered, overlapping, clipped,
    /// off-screen and too small elements, sideways scrolling, low
    /// contrast.
    #[command(after_help = "\
Reads geometry and computed styles in the page and its iframes (or
inside the element LOCATOR names) and prints one line per finding,
naming elements as the snapshot does, with refs:
  covered       an interactive element whose centre something else
                covers (in view; --full scrolls down the whole page)
  overlap       two interactive elements whose boxes overlap
  clipped       text cut off by overflow: hidden or clip
  truncated     text cut on purpose, by an ellipsis or a line clamp
                (info: listed with --all)
  overflow-x    the page scrolls sideways, and what sticks out
  offscreen     an interactive element no scroll can reach
  small-target  an interactive element under 24x24 px (WCAG 2.5.8; a
                link in a line of text, or one with room around it, is
                fine; --min-target changes 24)
  contrast      text below 4.5:1 (3:1 when large) against the pixels
                behind it, so text on images and gradients counts
--only takes some of these. --viewports mobile,tablet,desktop (or sizes,
390x844) checks at each size (mobile and tablet with touch), waiting for
the page to settle each time, and gives the tab its viewport back after.
--stretch-text 40 checks again with every text 40% longer, as a
translation might make it, and reports what only that finds. The page,
its scroll and its text are left as they were (should the session die
mid-run, a reload brings the text back). It exits 0 whatever it
finds; expect --layout fails on new findings. -q prints the number of
errors. Needs a running session.

Examples:
  chrome-cdp layout
  chrome-cdp layout --only covered,clipped --all
  chrome-cdp layout --viewports mobile,tablet,desktop
  chrome-cdp layout 'role=main' --stretch-text 40
  chrome-cdp layout --full --min-target 44")]
    Layout(LayoutArgs),

    /// Compare two pages: what their snapshots differ in, and with
    /// --visual their pixels.
    #[command(after_help = "\
A and B are each a URL (opened in a new tab of the current context,
compared, then closed), tab:<id> (a tab of this session, as it is),
session:<name> (the active tab of another session), or a .json file
`snapshot --json` wrote (--visual needs a live page). Nodes are matched
by structure, not by ref: siblings line up by role and name, and a node
left over with the same role where another was counts as changed. The
diff reads as snapshot --diff's: + only in B, - only in A, ~ changed
(was …), with the nodes around each change; refs are left out. When the
origins differ (local against production), B's links to its own origin
are read as links to A's, so only real differences show. A block that
moved shows as removed and added.

--visual shoots both viewports at one size (--viewport, else A's; a URL
side gets that size; tabs of different sizes compare the area they
share, with a warning) and groups the pixels that differ past
--threshold (0 to 1, default 0.1) into regions: the 10 largest are
listed, each named by the smallest element of B at its centre (with a
ref when B is a tab or session). The diff image (B faded, changes red,
regions boxed) is saved in the session's shots directory. It exits 0
whatever differs; --exit-code exits 1 when anything does (in run, batch
and test, the step fails). -q prints how many differences (changes and
regions) there are. Needs a running session.

Examples:
  chrome-cdp compare http://localhost:5173/cart https://shop.example.com/cart
  chrome-cdp compare tab:t1 tab:t2 -i
  chrome-cdp compare before.json tab:t1 --scope 'role=main'
  chrome-cdp compare session:local session:prod --visual --exit-code")]
    Compare(CompareArgs),

    /// Map an app: follow its links (and buttons) from a start page, and
    /// list every page and state found, how to reach each, and what was
    /// wrong on each.
    #[command(after_help = "\
Starts at URL (else the active tab's page) and walks breadth first:
on each state it clicks, in page order, the links to the start's origin
(and --origin's) that --include and --exclude (URL patterns, as wait
--url takes) allow, and with --click-buttons the buttons, tabs, menu
items and controls with aria-expanded or aria-haspopup. A state is the
URL (no fragment, query keys sorted, utm_* dropped) plus the page's
skeleton (its controls' roles and its headings', landmarks' and
dialogs' names), so an open modal is a state and a list with other data
is not. Clicks run the normal action code, so client-side routing runs;
to get back to a state it loads its URL when that shows the same
skeleton, else replays its path. A link to where another led already
is not clicked again.

It never submits a form (--submit-forms clicks submit buttons), types,
uploads, clicks a control whose name looks destructive (delete, remove,
log out, sign out, unsubscribe, pay, buy, checkout, confirm, reset, …;
--allow-destructive), leaves the allowed origins (other links are
listed as external), or follows mailto:, tel:, javascript:, download or
file links. alert and beforeunload are accepted, confirm and prompt
dismissed; a new tab a click opens is closed and its URL loaded
instead.

It runs in a context of its own with a copy of the current one's
cookies, storage, routes, mocks, init scripts and emulation, and closes
it at the end (also on Ctrl-C): your tabs are left alone. It stops at
--max-states (30), --max-actions (200, replayed steps included),
--depth (3 steps from the start) or --time (2m), and says which
stopped it and what was left. Each state lists its console errors,
failed requests and, with --layout, layout's errors.

-o DIR (empty or new) writes map.json (the --json data) and
states/<id>.cdp, a scenario that gets to each state (chrome-cdp run
DIR/states/s4.cdp), and with --screenshots states/<id>.png. It exits 0
whatever it finds; --exit-code exits 1 when a state had an error, a
failed request, a broken link or a layout error (in run, batch and test,
the step fails). -q prints how many problems. Needs a running session.

Examples:
  chrome-cdp explore http://localhost:5173/
  chrome-cdp explore --click-buttons --layout --exit-code
  chrome-cdp explore /admin --include '**/admin/**' --max-states 50
  chrome-cdp explore http://localhost:5173/ -o map --screenshots")]
    Explore(ExploreArgs),

    /// Print the file and line an element came from, and its components.
    #[command(after_help = "\
For the app's own dev build: the element's place (`at`) and the
components around it, innermost first (`in`), each at the line of its
file that renders the next step towards the element; then what answered.
Places are files relative to the project root (the directory of
chrome-cdp.toml or .git, or source_root) when chrome-cdp finds them on
disk, else URLs. It asks, in order: the framework's debug info in the
page (React 18 and 19, Vue 3, Svelte 5 dev builds; the page could fake
it), the stack Chrome recorded when a script made the element (node
stacks: on for localhost pages, see open --node-stacks), and the
document's HTML. When nothing answers it fails with NoSource and says
why. -q prints the place alone. Needs a running session.

Examples:
  chrome-cdp source e12
  chrome-cdp source 'role=button[name=\"Add to cart\"]' --depth 3
  $EDITOR \"$(chrome-cdp source e12 -q)\"")]
    Source(SourceArgs),

    /// Print the page's component tree, or the component that rendered an
    /// element, with its props and state.
    #[command(after_help = "\
Without a locator: the component tree from each root of the app's dev
build, --depth levels (default 4), each component at its own file and
the line of the first element it renders, with its props one level deep
(`items=[3 items]`; functions left out). Siblings with the same name,
place, props and components below are folded (`<CartItem> ×3`).

With a locator: the component that rendered the element, its props and
state in full (within the limits), and its owners, innermost first, at
most --depth (default 8). State is a React function component's
useState/useReducer hooks by index (`#0`), a class component's state,
Vue's setupState (refs unwrapped) and data; Svelte 5 has no public API
for it. Values are previewed in the page: 2 levels deep, strings cut at
80 characters, collections over 5 entries counted (`[12 items]`,
`{…8 keys}`), functions as `ƒ`.

Reads React 18 and 19, Vue 3 and Svelte 5 dev builds (the page's debug
info, which it could fake); a production build fails with NoSource.
Places are files relative to the project root, as source prints them.
-q prints the tree without the last line, or the component's place.
Needs a running session.

Examples:
  chrome-cdp components
  chrome-cdp components --depth 2
  chrome-cdp components e12               # the component behind e12
  chrome-cdp components 'role=button[name=\"Add\"] >> nth=0' --json")]
    Components(ComponentsArgs),

    /// Print the page's readable content (or an element's) as Markdown.
    #[command(after_help = "\
Without a locator, the main content: a single <main> (or role=main), else
a single <article>, else the element that holds the most paragraph text;
navigation, sidebars and footers inside it are left out. Headings, lists,
links (relative on the page's origin), images, code, quotes and tables are
kept; controls are not. An iframe in the main content shows its frame's
main content where it sits, between [iframe \"name\"] (or its URL) and
[/iframe] lines. A locator that names a frame reads that frame's main
content; one that matches nothing yet is waited for (up to --timeout,
10s by default). Needs a running session.

Examples:
  chrome-cdp markdown
  chrome-cdp markdown e40
  chrome-cdp markdown 'css=article' --max-chars 0")]
    Markdown(MarkdownArgs),

    /// Click an element: waits until it can be clicked, then a real mouse click.
    #[command(after_help = "\
Waits (up to --timeout, 10s by default) until the element is visible,
stable, enabled and not covered, scrolls it into view, then moves the
mouse to its centre and presses and releases the button there: trusted
events, as a user makes them. When something covers it, the error names
what, with a ref, and a click that likely dismisses it. Then it waits
for what the click caused (a new page to load, the requests it sent) and
adds a line for each thing that happened: a navigation, a new tab, a
dialog, a download, a console error, a failed request, focus moving to
another form or frame. Every action does. Needs a running session.

Examples:
  chrome-cdp click e6
  chrome-cdp click e6 --snap                  # and what changed on the page
  chrome-cdp click 'role=button[name=\"Sign in\"]'
  chrome-cdp click e6 --button right          # the context menu
  chrome-cdp click e12 --modifiers Shift      # select a range
  chrome-cdp click '#canvas' --position 10,20  # from its top-left corner
  chrome-cdp click e6 --force                 # skip the checks")]
    Click(ClickArgs),

    /// Double-click an element (click --count 2).
    #[command(after_help = "\
Two clicks in a row, as a user makes them: the page gets mousedown,
mouseup and click twice, then dblclick. Waits like click. Needs a running
session.

Examples:
  chrome-cdp dblclick e6
  chrome-cdp dblclick 'text=Rename me'")]
    Dblclick(DblclickArgs),

    /// Move the mouse over an element (menus, tooltips).
    #[command(after_help = "\
Waits like click (except that a disabled element is fine), then moves the
mouse to the element's centre, so the page gets mouseover, mouseenter and
mousemove, and :hover styles apply. Needs a running session.

Examples:
  chrome-cdp hover e8
  chrome-cdp hover 'role=menuitem[name=\"File\"]'")]
    Hover(HoverArgs),

    /// Tap an element with a touch (needs touch emulation).
    #[command(after_help = "\
Waits like click, then a touch starts and ends at the element's centre.
The tab must emulate a touch screen (navigator.maxTouchPoints > 0), or
the page would get no touch events; use click otherwise. Needs a running
session.

Examples:
  chrome-cdp tap e6
  chrome-cdp tap 'role=link[name=\"Menu\"]'")]
    Tap(TapArgs),

    /// Drag an element onto another: mouse events, or HTML5 drag and drop.
    #[command(after_help = "\
Presses the left button on the first element, moves to the second in
--steps moves and releases there, which is what sortable lists built on
mouse or pointer events need. When the page starts an HTML5 drag
(draggable elements), it is finished with real drag events (dragenter,
dragover, drop) carrying the page's own drag data. Both elements are
waited for like hover. Needs a running session.

Examples:
  chrome-cdp drag e4 e9
  chrome-cdp drag 'text=Card 1' 'role=list[name=\"Done\"]'
  chrome-cdp drag e4 e9 --steps 30          # slower, for fussy libraries")]
    Drag(DragArgs),

    /// Scroll the page or an element with the mouse wheel.
    #[command(after_help = "\
Real wheel events over the element (the middle of the viewport without
one), so scroll listeners and infinite scroll run. Prints where the
scroll container (the element or its nearest scrollable ancestor, else
the page) is now, and which end it reached. With a locator and neither
--by nor --to, scrolls the element into view. Needs a running session.

Examples:
  chrome-cdp scroll --by 0,600               # the page, one screen down
  chrome-cdp scroll --to bottom              # load more of a feed
  chrome-cdp scroll e30 --by 0,-200          # inside a list
  chrome-cdp scroll e30 --to top
  chrome-cdp scroll e99                      # into view")]
    Scroll(ScrollArgs),

    /// Low-level mouse events at viewport coordinates.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
An escape hatch for what the element commands cannot express: canvas
drawing, drags in hops, a press held across commands. Coordinates are
CSS pixels from the top-left corner of the tab's viewport. The session
keeps where the mouse is and which buttons it holds between commands.

Examples:
  chrome-cdp mouse move 100 200
  chrome-cdp mouse down
  chrome-cdp mouse move 300 200 --steps 10
  chrome-cdp mouse up
  chrome-cdp mouse wheel 0 400"
    )]
    Mouse(MouseArgs),

    /// Hold keys down across commands (Shift while clicking, …).
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
`keyboard down` presses a key and holds it; `keyboard up` releases it.
Held modifiers (Shift, Control, Alt, Meta, Mod) go with the tab's later
click, dblclick, press, type, mouse and drag events, as a user's held key
does. Keys stay held across navigations and are dropped when the tab
closes; `status` lists them. For a shortcut, `press Control+A` is simpler.

Examples:
  chrome-cdp keyboard down Shift
  chrome-cdp click e7                      # a Shift+click
  chrome-cdp click e9                      # extends the selection
  chrome-cdp keyboard up Shift
  chrome-cdp keyboard up --all"
    )]
    Keyboard(KeyboardArgs),

    /// Press keys or shortcuts: Enter, Control+A, "Mod+A Backspace".
    #[command(after_help = "\
Keys go where focus is, as a user's do; --in focuses an element first.
Name keys like Enter, Tab, Escape, ArrowDown, PageUp, F1, Numpad0, a or A,
and hold them together with + (Control+Shift+Z). Mod is Meta when the
browser runs on macOS and Control elsewhere, so Mod+A selects all on both.
Several chords, separated by spaces, are pressed in turn. Needs a running
session.

Examples:
  chrome-cdp press Enter
  chrome-cdp press Tab --in e5
  chrome-cdp press 'Mod+A Backspace' --in 'label=Search'
  chrome-cdp press Shift+Tab")]
    Press(PressArgs),

    /// Type text key by key, as a user does (fill is faster).
    #[command(after_help = "\
Presses a key for each character the US layout has (keydown, keypress,
input, keyup: every key listener runs) and inserts the rest (emoji, CJK)
as an input method does. Types where focus is; --in focuses an element
first (the caret goes after its text) once it takes text. Use fill to
replace a field's content in one go. Needs a running session.

Examples:
  chrome-cdp type 'hello world' --in e5
  chrome-cdp type 'rust' --in 'role=searchbox' --delay 50ms
  chrome-cdp type '👍 ok'")]
    Type(TypeArgs),

    /// Replace a field's content with text, in one input event.
    #[command(after_help = "\
Waits until the input, textarea or contenteditable element is visible,
enabled and editable, focuses it, selects what it holds and inserts the
text as one trusted edit, so frameworks (React, Vue, Svelte) see it; the
page gets change when the field loses focus. Date, time, colour and range
inputs get the value set (2026-09-27, 13:45, #ff0000, 42). An empty text
clears the field. Needs a running session.

Examples:
  chrome-cdp fill e5 'ada@example.com'
  chrome-cdp fill 'label=Password' - < secret.txt
  chrome-cdp fill '#birthday' 1990-05-17
  chrome-cdp fill e5 ''                      # clear")]
    Fill(FillArgs),

    /// Fill a form from a JSON object in one step: keys name fields,
    /// values pick the action.
    #[command(after_help = "\
Each key names a field by its label (ignoring case, spaces at the ends
and a trailing * or :), else its name, id or placeholder, else a part of
its label only it has. The value picks the action: a string or number
fills a text field (or a date, colour, range), selects a <select>'s
option (an array for several), or checks the radio of a group with that
label or value; true or false checks or unchecks a checkbox or switch; a
path, or an array of paths, uploads to a file input (relative to this
directory); a custom combobox is filled, then the option with that name
is clicked when its listbox shows one; a listbox's options with those
names are clicked. Fields are filled in the page's order, each as its
own command does (waiting until it can act), and a field that appears
after an earlier choice (a region after a country) is matched too.
A value of the wrong type fails with BadArgs before anything is filled;
keys that match no field, or several, fail with NotFound after the others
are filled, naming the fields. Without a LOCATOR the page must have one
form (else Ambiguous, with the forms' refs). --submit then clicks the
form's first submit button, or presses Enter in the last field filled.
One action report covers the whole form. `forms` shows the names to use.
Needs a running session.

Examples:
  chrome-cdp fill-form --values '{\"Email\": \"ada@example.com\", \"Password\": \"s3cret\"}' --submit
  chrome-cdp fill-form e10 --file order.json
  chrome-cdp fill-form 'role=form[name=\"Checkout\"]' --values - < order.json")]
    FillForm(FillFormArgs),

    /// Choose options of a <select> by value or label.
    #[command(after_help = "\
Each value matches an option's value, else its label (then in any case);
several values need a <select multiple>. Waits until the options are
there, then selects them and fires input and change. For a custom
dropdown (role=combobox or listbox without a <select>), click it, then
click the option. Needs a running session.

Examples:
  chrome-cdp select e7 blue
  chrome-cdp select 'label=Country' 'Norway'
  chrome-cdp select '#toppings' cheese olives")]
    Select(SelectArgs),

    /// Check a checkbox, radio button or switch (no-op if it is already).
    #[command(after_help = "\
Clicks it only when it is unchecked, then makes sure it became checked.
Custom checkboxes (role=checkbox, switch) are clicked like native ones; a
native input styled away behind its label is checked by clicking the
label. A label locator checks the control it labels. Needs a running
session.

Examples:
  chrome-cdp check e9
  chrome-cdp check 'label=Remember me'
  chrome-cdp check 'role=radio[name=\"Express\"]'")]
    Check(CheckArgs),

    /// Uncheck a checkbox or switch (no-op if it is not checked).
    #[command(after_help = "\
Like check, the other way. A radio button cannot be unchecked: check
another one of its group. Needs a running session.

Examples:
  chrome-cdp uncheck e9
  chrome-cdp uncheck 'label=Send me offers'")]
    Uncheck(CheckArgs),

    /// Give an element focus (without clicking it).
    #[command(after_help = "\
Waits until the element is visible, then focuses it, so focus styles,
focus listeners and later keys apply to it. Fails if it cannot take
focus. Needs a running session.

Examples:
  chrome-cdp focus e5
  chrome-cdp focus 'label=Email'")]
    Focus(FocusArgs),

    /// Take focus away from an element (blur and change fire).
    #[command(after_help = "\
If the element (or something in it) has focus, it loses it: the page gets
blur, and change for a field whose value was edited. Needs a running
session.

Examples:
  chrome-cdp blur e5")]
    Blur(FocusArgs),

    /// Put files into a file input, the file chooser a button opens, or a
    /// drop zone.
    #[command(after_help = "\
A file input gets the files at once, even a hidden one (most upload
widgets hide theirs behind a button), and the page gets input and
change as after a user's choice; a <label> of one works too. Anything
else is clicked like click does, and the files go to the file chooser
that opens. --drop drops the files on the element instead, as a user
drags them in from the desktop (dragenter, dragover and drop, with the
files in dataTransfer.files), for drop zones with no file input; it
fails if the page takes no drop there. A folder input (webkitdirectory)
takes one directory instead: its files, the page seeing each one's path
in it (webkitRelativePath, e.g. photos/2024/a.jpg). Paths are relative
to the current directory. --clear leaves the input with no files. Needs
a running session.

Examples:
  chrome-cdp upload e5 ./avatar.png
  chrome-cdp upload 'label=Attachments' a.pdf b.pdf
  chrome-cdp upload 'label=Folder' ./photos        # a webkitdirectory input
  chrome-cdp upload 'role=button[name=\"Choose files\"]' report.csv
  chrome-cdp upload 'text=Drop files here' photo.jpg --drop
  chrome-cdp upload e5 --clear")]
    Upload(UploadArgs),

    /// Read or write the clipboard (text, HTML or a PNG), through the page.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
The page's clipboard API does it, as a page's own copy and paste buttons
would: the tab's page must be on https (or http on localhost), and gets
the clipboard permissions for the rest of the session. The clipboard is
the browser's (the system's when headed): what a page copied can be read,
and what is written pastes with press Mod+V.

Examples:
  chrome-cdp clipboard read
  chrome-cdp clipboard write 'hello'
  chrome-cdp clipboard write --html '<b>Bold</b> move'
  chrome-cdp clipboard read --type image/png -o copied.png
  chrome-cdp press Mod+V --in e5"
    )]
    Clipboard(ClipboardArgs),

    /// Answer the JavaScript dialog (confirm, prompt, …) the page waits on.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
By default an alert, and a beforeunload (\"leave the page?\"), are accepted
at once; a confirm or prompt waits for this command, and the command that
ran into it fails with exit code 5 (see `open --dialogs`). With no dialog
open in the tab yet, this waits for one (up to --timeout), so a prompt the
page opens right after a confirm needs no `wait` in between.

Examples:
  chrome-cdp dialog accept
  chrome-cdp dialog dismiss
  chrome-cdp dialog accept 'Ada Lovelace'     # a prompt's answer"
    )]
    Dialog(DialogArgs),

    /// List the files the session's pages downloaded.
    #[command(after_help = "\
Downloads go to the session's downloads directory (`open --downloads-dir`
picks another), named as the server suggests (made unique). A command that
starts one says so; `wait --download` waits for it to finish. --clear
deletes the files of finished downloads and empties the list; downloads
still in progress are kept, and no other file is touched. The list is the
daemon's: after an upgrade replaced it, it starts again (and says so).

Examples:
  chrome-cdp downloads
  chrome-cdp downloads -q         # one path per line
  chrome-cdp downloads --clear")]
    Downloads(DownloadsArgs),

    /// Show what the page logged: console calls, uncaught exceptions and
    /// the browser's messages.
    #[command(after_help = "\
Every tab records its console from the moment it opens (the newest 1,000
entries; see `open --console-buffer`). By default only entries this command
has not shown before are printed, so running it again shows what is new.
Stack frames of localhost pages are resolved with source maps, and shown
as files of your project when they are there (src/cart.ts:12, relative to
the directory with chrome-cdp.toml or .git, or `source_root` in the
config; `chrome-cdp help config`). Workers log here too, marked `[dedicated worker w.js]`; a shared or service worker's
messages are in every tab with a frame on its origin. `--locals` shows the
variables of each uncaught error's top frames, as `errors --locals` does.

Examples:
  chrome-cdp console
  chrome-cdp console --level warn --since 0      # every warning and error so far
  chrome-cdp console --since 30s --grep 'fetch|api'
  chrome-cdp console --follow --timeout 1m       # keep printing new entries")]
    Console(ConsoleArgs),

    /// Show the page's errors: uncaught exceptions, console.error and
    /// failed loads from its own origin.
    #[command(after_help = "\
The first thing to run when an action did not do what you expected. Like
`console`, it shows only what it has not shown before; `--since 0` shows
every error the tab kept. Stack frames of localhost pages are resolved with
source maps, so they point at your original source files, shown relative
to your project (src/cart.ts:18:22) when they are there.

`--locals` shows, under each of an uncaught error's top 3 frames, its local
variables when the error was thrown (`item = undefined, qty = 2`): the
page pauses at uncaught errors for up to 200ms to read them, on localhost
pages by default (`debug locals on|off` changes that). An error without
them says why.

Examples:
  chrome-cdp errors
  chrome-cdp errors --locals                     # and each frame's variables
  chrome-cdp errors --since 0 --limit 5
  chrome-cdp errors --sourcemaps --tab t2")]
    Errors(ErrorsArgs),

    /// Show Chrome's own findings about the page, as in DevTools' Issues
    /// panel: mixed content, CORS and CSP blocks, cookie problems,
    /// deprecations, form problems.
    #[command(after_help = "\
Every tab records Chrome's issues from the moment it opens, from its
frames (iframes of other sites too) and its workers. Chrome raises one per
occurrence; the same rule about the same thing is one issue with a count
(×4), within one page load. Like `console`, only issues this command has
not shown before are printed, and one raised again is shown again. Form
issues (labels, autocomplete) are checked when this runs.

Each line has a severity (E error, W warning, I info), a stable rule id
(`mixed-content-blocked`, `cookie-exclude-same-site-unspecified-treated-as-lax`),
a message, and what it is about: an element's ref, a script's line, or the
request's id in the network log (`network show <id>`). --json has each
issue's `key`, which stays the same across runs, ports and reloads.

Examples:
  chrome-cdp issues
  chrome-cdp issues --since 0 --min-severity info   # all of them, info too
  chrome-cdp issues --grep cookie --all-tabs
  chrome-cdp issues --follow --timeout 1m           # keep printing new ones")]
    Issues(IssuesArgs),

    /// Audit the page in one report: Chrome's issues, accessibility,
    /// layout, errors, failed requests, security and metadata.
    #[command(after_help = "\
Runs every check chrome-cdp has over the active tab (or --tab) and prints
one report, most severe first, then by category:
  issues    Chrome's own findings (see `issues`) since the page loaded
  a11y      the accessibility rules of expect --a11y, level AA (A is an
            error, AA a warning)
  layout    `layout`'s findings at the tab's viewport
  console   uncaught exceptions (error) and console errors (warning)
  network   failed requests and 4xx/5xx responses (error from the page's
            origin, else warning)
  security  http: pages (not localhost), password fields and forms on
            http:, no Content-Security-Policy, no HSTS on https:, cookies
            without Secure, HttpOnly (session-like names) or SameSite
  meta      no <title>, no lang, no or zoom-blocking viewport meta,
            no description (info), noindex (info)
A LOCATOR checks inside that element what can be scoped: a11y, layout and
the forms of security (--category adds the others, page-wide). It waits
for the page's load event first.

Each finding has a key (`a11y/image-alt/#logo`, `security/no-csp//cart`)
that stays the same across runs, ports and reloads. --baseline accepts
findings by key: a JSON file (NAME is <dir>/audit/NAME.json, <dir> being
--dir or the project's .chrome-cdp; `chrome-cdp schema audit-baseline`
has its format). Accepted findings are hidden (--show-accepted lists
them), entries that no longer match are listed as stale, and --update
writes the current findings into the file, keeping notes. It exits 0
whatever it finds; expect --audit fails on findings the baseline does
not accept. -q prints the number of findings shown. docs/audits.md has
the details, and docs/commands/audit.md every rule with its severity.

Examples:
  chrome-cdp audit
  chrome-cdp audit --min-severity error
  chrome-cdp audit --category security,meta --min-severity info
  chrome-cdp audit 'role=main' --category a11y
  chrome-cdp audit --baseline checkout --update     # accept what it finds
  chrome-cdp audit --baseline e2e/audit/checkout.json --show-accepted")]
    Audit(AuditArgs),

    /// Show the page's requests, or one in full, or clear the log.
    #[command(
        args_conflicts_with_subcommands = true,
        after_help = "\
Every tab records its requests from the moment it opens (the newest 2,000;
see `open --network-buffer`). By default only requests this command has not
shown before are printed; one shown while pending is shown again when it
ends. Each line reads: #id METHOD STATUS URL TYPE SIZE TIME, and on
localhost pages where in your source a script sent it (`from
src/api.ts:12`). Requests of the tab's workers are in it too, marked with
the worker, e.g. `[service worker sw.js]`; a shared or service worker's go
to every tab of its origin.

Examples:
  chrome-cdp network
  chrome-cdp network --failed --since 0            # every failed request so far
  chrome-cdp network --type xhr,fetch --url '**/api/**'
  chrome-cdp network --status 4xx --method POST
  chrome-cdp network --follow --type fetch         # watch requests as they come
  chrome-cdp network show 42 --body                # the response body of #42"
    )]
    Network(NetworkArgs),

    /// Mock, block or change the page's requests.
    #[command(after_help = "\
Adds a rule for requests whose URL matches PATTERN: a glob (`**/api/*`,
where * stops at / and ** does not), a /regex/, or a part of the URL. Rules
belong to the browser context (the session's own, unless --context): they
apply to every tab, popup, frame and worker in it, the ones opened later
too, before the page's first script runs (Chrome lets a shared worker be
intercepted only as it starts, and under load may start it first). The
first matching rule, in the order they were added, handles a request; the
rest go on untouched. `routes` lists them, `unroute` removes them.

One action per rule:
  answer it:      --json-body, --body, --body-file, --status, --header
  fail it:        --abort [REASON]  (failed, blockedbyclient, timedout, …)
  change it:      --set-header, --remove-header, --rewrite-url
  patch it:       --patch-json (the real response, with the patch merged in)
--delay holds a matched request first (alone: then sends it on).

While routes exist, requests skip service workers and the HTTP cache.
Mocked responses to another origin allow the page's origin (CORS).

Examples:
  chrome-cdp route '**/api/items' --json-body '[]'
  chrome-cdp route /api/login --method POST --status 401 --json-body '{\"error\":\"bad password\"}'
  chrome-cdp route '**/api/cart' --patch-json '{\"items\": [], \"total\": 0}'
  chrome-cdp route /api/ --status 500 --body 'down' --times 1
  chrome-cdp route '**/*.js' --delay 2s
  chrome-cdp route analytics.example.com --abort
  chrome-cdp route /api/ --set-header 'Authorization: Bearer test' --remove-header cookie
  chrome-cdp route 'https://prod.example.com/api/**' --rewrite-url http://localhost:3000/api
  chrome-cdp route /report.pdf --body-file ./fixtures/report.pdf")]
    Route(RouteArgs),

    /// Block requests: by URL, or ads, images, fonts or media.
    #[command(after_help = "\
Each PATTERN (as for `route`) and each preset becomes a rule that fails its
requests as blocked by the client. Presets: --images, --fonts, --media
(by resource type) and --ads (well-known ad and tracking hosts). Blocking
images, fonts and media makes heavy pages load faster. `routes` lists the
rules, `unroute` removes them.

Examples:
  chrome-cdp block --images --fonts
  chrome-cdp block --ads
  chrome-cdp block '**/*.mp4' cdn.example.com/tracker.js")]
    Block(BlockArgs),

    /// List the routing rules and their hits, or the requests they saw.
    #[command(after_help = "\
Each line reads: id, what it matches, what it does, and how many requests
it handled. --log lists the requests routing saw instead (while routes
exist, every request is seen), newest last, with the rule that handled
each.

Examples:
  chrome-cdp routes
  chrome-cdp routes --log
  chrome-cdp routes --log --limit 100")]
    Routes(RoutesArgs),

    /// Remove routing rules.
    #[command(after_help = "\
Without routes, requests are no longer intercepted at all.

Examples:
  chrome-cdp unroute r2
  chrome-cdp unroute r1 r3
  chrome-cdp unroute --all")]
    Unroute(UnrouteArgs),

    /// Answer the page's requests from mock files: use, list, log, explain.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
A mock set is a TOML file in the project's mocks directory (`mocks` of
[e2e] in chrome-cdp.toml, else ./e2e/mocks): `mock use base` reads
e2e/mocks/base.toml. Each [[mock]] matches requests and answers them. Data
files are relative to the mocks directory:

  [settings]
  scope = [\"/api/**\"]      # requests the set owns; unmatched ones get a 501
  [[mock]]
  name = \"product by id\"
  match = { method = \"GET\", path = \"/api/products/:id\" }
  respond = { json_file = \"data/products/{{params.id}}.json\" }

Match on: method, path (:param, *, **), url, regex, query, headers, json
(a subset; \"*\" matches anything), form (urlencoded or multipart; a
file field's value is its file name), body, graphql. Answer with
status, headers and one of json, json_file, body, body_file or file; or
with abort, or passthrough (plus patch); delay holds the answer. Templates:
{{params.x}} {{query.x}} {{headers.x}} {{json.x}} {{form.x}}
{{form_files.x.filename}} {{uuid}} {{now}} {{seq}} {{faker.name}}
{{state.x}}. A json_file that does not exist
answers 404. `chrome-cdp schema mocks` prints the file format's JSON
Schema.

State: `sequence = [{…}, {…}]` answers in turn (after_sequence =
repeat-last, cycle or unmatched). [state] declares named values that
mocks require (`when = { logged_in = true }`) and change (`set_state`,
`unset_state`). A [[collection]] is a fake REST resource (name, path,
seed = \"data/todos.json\"): GET lists, POST adds, GET/PUT/PATCH/DELETE
path/:id; a path with parameters (/api/users/:user/todos) keeps each
user's items apart by their `user` field. `mock state` shows it all,
`mock reset` starts it over.

Beyond fetches: a [[websocket]] (url, on_open, on_message) answers the
page's WebSockets (`mock ws send` pushes more), `respond = { sse = [...] }`
streams Server-Sent Events to its EventSources, and `mock serve` answers
server-side requests as a real HTTP server.

Sets applied later are tried first, and `route` rules come before any
mock. Mock files are read again when they change. The commands need a
running session, except `mock explain` and `mock list --available`.

Examples:
  chrome-cdp mock use base checkout
  chrome-cdp mock list
  chrome-cdp mock log --unmatched
  chrome-cdp mock explain GET /api/products/7
  chrome-cdp mock state todos
  chrome-cdp mock serve --port 4010
  chrome-cdp mock off"
    )]
    Mock(MockArgs),

    /// Send an HTTP request from the browser context, outside the page.
    #[command(after_help = "\
Sends the request the way the context's pages would: the context's route
rules and mock sets answer it first (so with mocks, a collection takes the
POST), else it goes to the server with the context's cookies (HttpOnly
too). Cookies the answer sets go into the context, so `http POST
/api/login` logs its pages in. A URL starting with / is relative to
base_url. The page never sees the request, and `expect request` does not
count it.

In a test scenario (`chrome-cdp test`), it arranges data through the API
in @before and cleans up in @after; --save NAME keeps the JSON answer for
later steps as ${saved.NAME.field}. It fails unless the status is
--expect-status (default 2xx).

Examples:
  chrome-cdp http GET /api/products
  chrome-cdp http POST /api/orders --json-body '{\"sku\":\"kb-1\",\"qty\":1}' --save order
  chrome-cdp http DELETE /api/orders/42 --expect-status 2xx,404
  chrome-cdp http POST /api/login --json-body '{\"email\":\"ada@example.com\"}' -q")]
    Http(HttpArgs),

    /// Run scripts in every page before its own: add, list or remove.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
An init script runs in every new document of the browser context (tabs,
popups and frames, those opened later too) before the page's own scripts, in the
order they were added. Stub APIs, set feature flags, or record what the
page does. Init scripts belong to the browser context (the session's own,
unless --context); the session's own stay when the browser is restarted.
For the very first page, give them to open: open --init-script FILE URL.

Examples:
  chrome-cdp init-script add ./stubs.js
  chrome-cdp init-script add --code 'window.FEATURE_X = true' --now
  chrome-cdp init-script list
  chrome-cdp init-script remove s1"
    )]
    InitScript(InitScriptArgs),

    /// Give pages a function whose calls show up in the console.
    #[command(after_help = "\
Each NAME becomes a global function in every page and frame of the session
(the open ones at once, new ones before their first script). It takes any
arguments and returns a promise; each call is logged in the tab's console
as `binding NAME(args)`, the arguments as JSON, so `console` shows what
the page reported. Without NAME, lists the exposed functions. `unexpose`
takes them away.

The promise resolves to undefined, or with --returns to that JSON value.
--returns-seq gives the calls its values in call order (counted across
the session's tabs), the last one for every later call. The answer is
data, not code: for logic in the page, use eval or init-script. Exposing
a NAME again with other answers starts its sequence over.

Examples:
  chrome-cdp expose onCheckout
  chrome-cdp console --grep 'binding onCheckout'
  chrome-cdp expose getConfig --returns '{\"flags\":[\"beta\"]}'
  chrome-cdp expose nextToken --returns-seq '[\"a\",\"b\",null]'
  chrome-cdp wait --binding onCheckout
  chrome-cdp expose")]
    Expose(ExposeArgs),

    /// Take exposed functions away from pages.
    #[command(after_help = "\
Examples:
  chrome-cdp unexpose onCheckout
  chrome-cdp unexpose --all")]
    Unexpose(UnexposeArgs),

    /// Make pages look like a phone, tablet or desktop; list or reset.
    #[command(after_help = "\
A device sets the viewport, pixel ratio, mobile mode, touch and user agent
(the table comes from Playwright's device descriptors). Without DEVICE,
prints everything the session emulates; `emulate reset` turns all of it
off (viewport, user agent, locale, time zone, geolocation, media,
network, CPU and time). Emulation belongs to the browser context (the session's own,
unless --context): it applies to every tab, popup and frame in it, those
opened later too, before the page's first script runs.

Examples:
  chrome-cdp emulate 'iPhone 15'
  chrome-cdp emulate Pixel 7 landscape
  chrome-cdp emulate --list
  chrome-cdp emulate                      # what the session emulates
  chrome-cdp emulate reset")]
    Emulate(EmulateArgs),

    /// Set the viewport: WIDTHxHEIGHT, pixel ratio, mobile, touch.
    #[command(after_help = "\
In headless mode pages get 1280x800 until a viewport is set (unless
--chrome-arg=--window-size sets the window); headed, the window decides.
`viewport reset` goes back to that. Applies to every tab of the session.

Examples:
  chrome-cdp viewport 1440x900
  chrome-cdp viewport 390x844 --dpr 3 --mobile --touch
  chrome-cdp viewport reset")]
    Viewport(ViewportArgs),

    /// Set where navigator.geolocation says the device is.
    #[command(after_help = "\
Also grants the geolocation permission to every origin, so pages get the
position without asking. `geo reset` stops emulating the position.

Examples:
  chrome-cdp geo 59.9139,10.7522
  chrome-cdp geo -33.8688,151.2093 --accuracy 50
  chrome-cdp geo reset")]
    Geo(GeoArgs),

    /// Set the time zone pages see (an IANA name).
    #[command(after_help = "\
Frames and workers see it too; a worker that kept another is a warning.

Examples:
  chrome-cdp tz Europe/Berlin
  chrome-cdp tz America/New_York
  chrome-cdp tz reset")]
    Tz(NameArgs),

    /// Set the locale: Intl formats, navigator.language, Accept-Language.
    #[command(after_help = "\
Frames and workers see it too; a worker that kept another is a warning.

Examples:
  chrome-cdp locale de-DE
  chrome-cdp locale ja-JP
  chrome-cdp locale reset")]
    Locale(NameArgs),

    /// Set the user agent pages and servers see.
    #[command(after_help = "\
`emulate <device>` sets a device's user agent with its viewport. Workers
get it too: their navigator.userAgent and their requests.

Examples:
  chrome-cdp ua 'Mozilla/5.0 (compatible; MyBot/1.0)'
  chrome-cdp ua reset")]
    Ua(UaArgs),

    /// Emulate CSS media: print, dark mode, reduced motion, forced colors.
    #[command(after_help = "\
Each flag sets one media feature and keeps the others; `media reset` gives
all of them back to the browser.

Examples:
  chrome-cdp media --color-scheme dark
  chrome-cdp media --reduced-motion reduce --forced-colors active
  chrome-cdp media --type print
  chrome-cdp media reset")]
    Media(MediaArgs),

    /// Take the network away (requests fail, navigator.onLine is false).
    #[command(after_help = "\
Examples:
  chrome-cdp offline on
  chrome-cdp offline off")]
    Offline(OfflineArgs),

    /// Slow the network: a preset, or latency and bandwidth.
    #[command(after_help = "\
Presets (as in Chrome DevTools): slow-3g (2s latency, 400kbit/s both
ways), fast-3g (563ms, 1440kbit/s down, 675kbit/s up), 4g (165ms,
8100kbit/s down, 1350kbit/s up). Flags change a preset's values, or set
their own.

Examples:
  chrome-cdp throttle slow-3g
  chrome-cdp throttle --latency 300ms --down 1000 --up 500
  chrome-cdp throttle off")]
    Throttle(ThrottleArgs),

    /// Slow the CPU down by a factor (4 = four times slower).
    #[command(
        name = "cpu-throttle",
        after_help = "\
Examples:
  chrome-cdp cpu-throttle 4
  chrome-cdp cpu-throttle off"
    )]
    CpuThrottle(CpuThrottleArgs),

    /// List, grant, deny or reset permissions (geolocation, camera, …).
    #[command(after_help = "\
Without a subcommand, lists the permissions the session set (`geo` and
`clipboard` grant some too). Permissions belong to a browser context, so
they apply to every tab of the session, or with --context to every tab of
that context only; without --origin, to every origin. For camera and
microphone streams, start the browser with `open --fake-media`.

Examples:
  chrome-cdp permissions
  chrome-cdp permissions grant notifications
  chrome-cdp permissions grant camera microphone --origin http://localhost:3000
  chrome-cdp permissions deny geolocation
  chrome-cdp permissions reset")]
    Permissions(PermissionsArgs),

    /// List, load, reload or uninstall unpacked extensions; open an
    /// extension's popup; read or write its chrome.storage.
    #[command(after_help = "\
Without a subcommand, lists the unpacked extensions the browser has
loaded: id, name, version and directory. Start a browser with one with
`open --extension DIR`. Name an extension by its id or its name.

Examples:
  chrome-cdp extensions
  chrome-cdp extensions load ./my-extension
  chrome-cdp extensions reload 'My Extension'
  chrome-cdp extensions popup 'My Extension'
  chrome-cdp extensions storage 'My Extension' get")]
    Extensions(ExtensionsArgs),

    /// List, set or clear the browser's cookies.
    #[command(
        args_conflicts_with_subcommands = true,
        after_help = "\
Without a subcommand, lists the cookies of every tab (they are the
browser's): name=value (cut at 60 characters; --json has all of it),
domain and path, and flags. A domain with a leading dot (.example.com)
is sent to subdomains too; without one, to that host only.

Examples:
  chrome-cdp cookies
  chrome-cdp cookies --domain example.com
  chrome-cdp cookies --url https://app.example.com/api
  chrome-cdp cookies set theme=dark
  chrome-cdp cookies clear --name session"
    )]
    Cookies(CookiesArgs),

    /// Read or change a page's localStorage or sessionStorage.
    #[command(after_help = "\
AREA is local (localStorage) or session (sessionStorage); ACTION is list
(the default), get KEY, set KEY VALUE (VALUE `-` reads stdin), remove KEY
or clear. It works in the tab's main frame, or with --origin in its first
frame of that origin (an iframe's). sessionStorage belongs to the tab.

Examples:
  chrome-cdp storage local
  chrome-cdp storage local get token
  chrome-cdp storage local set feature-x on
  chrome-cdp storage session clear
  chrome-cdp storage local --origin https://auth.example.com")]
    Storage(StorageArgs),

    /// Save or load cookies and storage, e.g. a login (Playwright's format).
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
A state file is Playwright's storageState JSON: the cookies, and each
origin's localStorage. chrome-cdp adds sessionStorage, and IndexedDB with
`state save --indexeddb` (as Playwright's indexedDB option has it), so
Playwright's auth files load here and ours load in Playwright. The file
holds secrets (session cookies, tokens): it is written readable by you
only; keep it out of version control. For a whole persistent browser
profile instead, use `open --profile <name>`.

Examples:
  chrome-cdp state save auth.json
  chrome-cdp state load auth.json
  chrome-cdp open --state auth.json https://app.example.com"
    )]
    State(StateArgs),

    /// Freeze or pause the time pages see, move it on, or give the real one back.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
While frozen, `Date` (and `Intl.DateTimeFormat` without a date) gives the
frozen time in every tab, frame and worker, from each page's (or worker's)
first script on; a warning says how many workers already running got it
late.
Timers and animations keep running in real time. `time advance` also moves
performance.now() on and runs the setTimeout and setInterval callbacks due
by then (each once), so expiry, polling and debounce can be tested without
waiting. It is an init script that replaces `Date`, not Chrome's virtual
time, which stalls page loads.
`time pause` stops timers and animation frames as well, and `time run-for`
fires them in order, each interval tick and a frame every 16ms, like
Playwright's clock.runFor.

Examples:
  chrome-cdp time freeze 2026-01-31T09:30:00Z
  chrome-cdp time advance 90s
  chrome-cdp time pause
  chrome-cdp time run-for 1s
  chrome-cdp time reset"
    )]
    Time(TimeArgs),

    /// Save an image of the page, the whole page or one element.
    #[command(after_help = "\
Prints the file's path (-q: only the path); the image goes to the
session's shots/ directory (1.png, 2.png, …) unless -o names a file, or
-o - for stdout. The format follows -o's extension unless --format says.

Images are in CSS pixels (--scale css) and at most --max-size pixels on
their longest side (scaled down to fit; 0 for no limit), so a point in the
image is a point on the page. An element is scrolled into view (and waited
for, up to --timeout, 10s by default) first, and the image is its box.
--full captures the whole page; when the page does not scroll itself but
inside a container (html, body { height: 100% } and a scrolling <main>),
the viewport grows for the capture until that container's content fits,
and is put back afterwards. --full LOC names the container.

--annotate labels the elements `snapshot -i` gives refs with those refs;
--mask paints over elements. Both are drawn only for the screenshot, above
an open modal dialog or popover too. Needs a running session.

Examples:
  chrome-cdp screenshot
  chrome-cdp screenshot --annotate            # label the refs in the image
  chrome-cdp screenshot e12                   # one element
  chrome-cdp screenshot --full -o page.png    # the whole page
  chrome-cdp screenshot --full '#feed'        # all of what #feed scrolls
  chrome-cdp screenshot --mask '.price' --mask e7
  chrome-cdp screenshot --format jpeg --quality 70 -o - > shot.jpg")]
    Screenshot(ScreenshotArgs),

    /// Print the page to a PDF file (headless browsers only).
    #[command(after_help = "\
Prints the file's path (-q: only the path); the PDF goes to the session's
pdfs/ directory (1.pdf, 2.pdf, …) unless -o names a file, or -o - for
stdout. Chrome prints the page as its print dialog would: with the print
media styles, on pages of --format paper. Chrome prints only when headless.
Needs a running session.

--header and --footer print an HTML template on each page, in the margin
(make it wide enough). Chrome fills elements of class pageNumber,
totalPages, date, title and url, and prints the templates in a tiny font
unless they set one.

Examples:
  chrome-cdp pdf
  chrome-cdp pdf -o invoice.pdf --format A4 --background
  chrome-cdp pdf --landscape --margin 1cm
  chrome-cdp pdf --margin 10mm,15mm           # top and bottom, left and right
  chrome-cdp pdf --pages 1-3,5 --css-page-size
  chrome-cdp pdf --scale 0.8
  chrome-cdp pdf --margin 2cm --footer '<div style=\"font-size:10px;width:100%;text-align:center\">Page <span class=pageNumber></span> of <span class=totalPages></span></div>'
  chrome-cdp pdf --margin 2cm --header @header.html")]
    Pdf(PdfArgs),

    /// Run JavaScript in the page and print its value.
    #[command(after_help = "\
The script runs like a line typed into DevTools' console: statements are
fine and the last one's value is printed, `await` works at the top level,
and it counts as a user gesture. A promise is awaited; a function
(`() => …`, `el => …`) is called, with the --in element. Strings print as
they are; other values print as JSON. DOM nodes print as
`button#id.class`, and elements of the page with a ref to act on
(`button#save.primary [e14]`): the first 50 of a value; a warning says how
many more got none. The script's `let`s and `const`s do not stay behind in
the page. An exception exits 1 with its message and stack. The --in
element is waited for (within --timeout) while nothing matches it. Needs a
running session.

Examples:
  chrome-cdp eval document.title
  chrome-cdp eval 'document.querySelectorAll(\"a\").length'
  chrome-cdp eval '[...document.querySelectorAll(\".row button\")]'
  chrome-cdp eval --in e12 'el.value'
  chrome-cdp eval --in '#price' 'getComputedStyle(el).color'
  chrome-cdp eval 'await fetch(\"/api/me\").then((r) => r.json())'
  chrome-cdp eval --frame checkout 'location.href'
  chrome-cdp eval - <<'EOF'
    const rows = [...document.querySelectorAll('tr')];
    rows.map((r) => r.innerText)
  EOF")]
    Eval(EvalArgs),

    /// Record the page's requests to a HAR file.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
`har start` marks where each tab's network log is; `har stop` writes the
requests since then, of every tab (or of --tab), as a HAR 1.2 file that
DevTools, Charles or a HAR viewer can open. Each tab is a page, or each
document for a tab that navigated meanwhile (t1_1, t1_2, …), with its
DOMContentLoaded and load times. Response bodies go in unless `--content
omit`. Needs a running session.

Examples:
  chrome-cdp har start
  chrome-cdp click e5 && chrome-cdp har stop -o checkout.har
  chrome-cdp har start --content omit --tab t2"
    )]
    Har(HarArgs),

    /// Record a Chrome performance trace, for DevTools or Perfetto.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
`trace start` starts Chrome's tracing on the tab (the Performance panel's
categories by default); `trace stop` saves the trace as JSON, which
DevTools' Performance panel (Load profile) and https://ui.perfetto.dev
open. For a quick look at a page's speed, `chrome-cdp perf` is cheaper.
Needs a running session.

Examples:
  chrome-cdp trace start --screenshots
  chrome-cdp reload && chrome-cdp trace stop -o load.json
  chrome-cdp trace stop -o trace.json.gz        # gzipped"
    )]
    Trace(TraceArgs),

    /// Show how fast the page is: load timing, Web Vitals, blocking time,
    /// memory.
    #[command(after_help = "\
One compact block: TTFB, FCP, DOMContentLoaded and load (ms after the
navigation started), the requests and bytes, LCP (and its element), CLS,
INP (once the page had an interaction, and which one was slowest), total
blocking time from long tasks, and the element count, listeners and JS
heap. LCP, CLS and INP are rated good, needs improvement or poor by the
Web Vitals thresholds. Interact first to measure INP. -q prints the LCP.
`expect --perf 'lcp<2.5s tbt<200'` fails when a number is over budget.
Needs a running session.

Examples:
  chrome-cdp perf
  chrome-cdp click e5 && chrome-cdp perf       # INP of that click
  chrome-cdp perf --json | jq .lcp")]
    Perf(PerfArgs),

    /// Measure how much of the page's JavaScript and CSS is unused.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
`coverage start` turns on V8's block coverage and CSS rule tracking in the
tab, its cross-site iframes and its workers (and those that start while it
runs); `coverage stop` lists the scripts and stylesheets by unused bytes
(code that never ran, rules that never matched), as DevTools' Coverage
panel does. Inline scripts and styles are summed per document; a file that
ran in a cross-site iframe or a worker is marked [iframe \"pay\"] or
[dedicated worker w.js]. Needs a running session.

Examples:
  chrome-cdp coverage start
  chrome-cdp reload && chrome-cdp click e4 && chrome-cdp coverage stop
  chrome-cdp coverage stop --limit 5 --json"
    )]
    Coverage(CoverageArgs),

    /// Log values at a source line each time it runs (logpoints), without
    /// stopping the page or editing its code.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
`debug log FILE:LINE EXPR…` sets a logpoint in the tab's scripts: the line
of an original source file, found through the scripts' source maps (a
script without one by its own URL). Each time the line runs, the page
logs the expressions' values without stopping: in `console`, and in the
report of the action that ran it (`! logpoint 1 src/cart.ts:42: total=12`).
It is set again after a reload, and in scripts loaded later. `debug list`
shows the logpoints and their hits; `debug clear` removes them and turns
the debugger off when nothing else needs it. While the debugger is on, a
`debugger;` statement does not stop the page: it is resumed and noted in
the console.

`debug locals on|off|localhost` says where uncaught errors keep their
frames' local variables for `errors --locals` (localhost pages by default;
the debugger is on there for them). Needs a running session.

Examples:
  chrome-cdp debug log src/cart.ts:42 total items.length
  chrome-cdp click e12                   # ! logpoint 1 src/cart.ts:42: total=12, items.length=3
  chrome-cdp debug list
  chrome-cdp debug clear --all
  chrome-cdp debug locals off"
    )]
    Debug(DebugArgs),

    /// Point out an element in the browser window, for a human watching.
    #[command(after_help = "\
Scrolls the element into view and paints DevTools' element highlight over
it (content, padding, border and margin boxes, with a tooltip) for
--duration. Meant for a headed browser (open --headed); headless, only
screenshots show it. Needs a running session.

Examples:
  chrome-cdp highlight e12
  chrome-cdp highlight 'role=button[name=\"Save\"]' --duration 5s
  chrome-cdp highlight '#cart' --duration 0      # until the next highlight")]
    Highlight(HighlightArgs),

    /// Open DevTools for the tab, in its own window (headed browsers).
    #[command(after_help = "\
For a human pairing with the agent: DevTools opens next to the browser
window, on --panel if given. A headless browser has no window to open it
in: start one with open --headed --restart. Needs a running session.

Examples:
  chrome-cdp inspect
  chrome-cdp inspect --panel console
  chrome-cdp inspect --tab t2 --panel network")]
    Inspect(InspectArgs),

    /// Make one raw CDP call on the session's browser or current page.
    #[command(after_help = "\
Starts the session's browser first if it has none. --tab and --frame send
the call to a page. --listen also prints the events of the call's session
that follow it, one per line, until --count of them came (1 by default; 0
keeps them all until --timeout); with no method, it only listens.
--all-sessions listens on every session of the tab: its page, its
cross-site iframes and its workers, each line marked with which. The page
domains chrome-cdp uses (Page, Runtime, Network, Log, …) are on; send
another's enable call to hear its events. A method Chrome does not know
gets the closest name of the protocol as a hint; it is never sent for you.
Methods: https://chromedevtools.github.io/devtools-protocol/

Examples:
  chrome-cdp cdp Browser.getVersion
  chrome-cdp cdp --target page Runtime.evaluate '{\"expression\":\"1+1\"}'
  echo '{\"expression\":\"1+1\"}' | chrome-cdp cdp --target page Runtime.evaluate -
  chrome-cdp cdp Target.getTargets --session shop
  chrome-cdp cdp --tab t2 Page.reload
  chrome-cdp cdp --frame checkout.example Runtime.evaluate '{\"expression\":\"location.href\"}'
  chrome-cdp cdp --target page --listen Page.loadEventFired Page.reload
  chrome-cdp cdp --target page --listen 'Network.*' --count 0 --timeout 5s
  chrome-cdp cdp --all-sessions --listen Network.requestWillBeSent --count 0 Page.reload")]
    Cdp(CdpArgs),

    /// Run commands from stdin, one per line, in one call.
    #[command(after_help = "\
Each line is a command as you would type it after `chrome-cdp`, with the
same quoting; `#` starts a comment. Every line is checked before the first
runs, then they run in order and stop at the first failure (--continue
runs the rest). Each step prints `[3/7] click e6 ✓` and its usual output;
with --json, one JSON line per step. Lines inherit --session, --timeout,
--tab, --json and -q. ${name} takes a --var, ${env:NAME} an environment
variable. The full syntax: docs/scenario-syntax.md.

Examples:
  chrome-cdp batch <<'EOF'
    goto localhost:3000/login
    fill label=Email ada@example.com
    fill label=Password \"${env:APP_PASSWORD}\"
    click 'role=button[name=\"Sign in\"]'
    wait --url /dashboard
  EOF
  printf 'goto example.com\\ntext css=h1\\n' | chrome-cdp batch --json
  chrome-cdp batch --continue --var user=ada < steps.cdp")]
    Batch(BatchArgs),

    /// Run a .cdp script: commands one per line, with ${var} substitution.
    #[command(after_help = "\
The same syntax as `batch` (docs/scenario-syntax.md), read from a file.
${name} takes a --var, ${env:NAME} an environment variable, and
${data:file.json#/a/0} a value from a JSON file next to the script.
Errors name the place: flow.cdp:12:5.

Examples:
  chrome-cdp run login.cdp
  chrome-cdp run login.cdp --var email=ada@example.com --var plan=pro
  chrome-cdp run smoke.cdp --continue --session ci
  chrome-cdp run login.cdp --json    # one JSON line per step")]
    Run(RunArgs),

    /// Translate a Playwright test file into a .cdp scenario (best effort).
    #[command(after_help = "\
Reads a Playwright test (TypeScript or JavaScript) and writes the .cdp
scenario that does the same, following docs/migrating-from-playwright.md:
goto, getBy… locators, click, fill, check, selectOption, press, waitFor…,
route with fulfill or abort, and the expect matchers. Each test(…) becomes
a @test \"describe > title\" block (docs/e2e/scenarios.md), beforeEach and
afterEach become @before and @after, beforeAll and afterAll @before-all
and @after-all, test.skip/fixme/only @skip and @only, and test.use a
@viewport or @device setting plus emulation commands, and { tag } @tags.
chrome-cdp test runs it, each test in a fresh browser context. A file
without tests (a script that launches the browser itself) becomes a plain
script for run.

Values known now are read: the file's constants (texts, numbers, lists and
objects), for…of over such a list, and its helper functions, inlined
where they are called. What it cannot translate (conditions, helpers from
other files, values only known at run time) becomes a `# TODO line N
(why): code` line, and a warning counts them: rewrite those before running
it. A test left with no steps gets @skip, so the file still loads. Nothing
is run and no browser starts.

Examples:
  chrome-cdp convert tests/login.spec.ts
  chrome-cdp convert tests/login.spec.ts -o login.cdp && chrome-cdp test login.cdp
  chrome-cdp convert - < checkout.spec.ts > checkout.cdp
  chrome-cdp convert tests/login.spec.ts --json    # the scenario and each TODO")]
    Convert(ConvertArgs),

    /// Format .cdp scenario files and mock files in place; --check only reports.
    #[command(after_help = "\
Without paths, formats the project's E2E files: every .cdp file below the
project root (chrome-cdp.toml's folder) and the mock files in the mocks
directory ([e2e] mocks, else e2e/mocks). A folder means the .cdp files
below it, and its .toml files when they are mock files.

.cdp files: 2-space bodies, one blank line before each @test and hook,
single quotes unless a word needs ${…} or a ' (directive arguments such
as test names take double quotes), long flags instead of short ones,
settings in a stable order (@skip @only @mocks @tags @backend @safe
@timeout @retries @viewport/@device @allow-console-errors), trailing
comments lined up. Mock files: the keys of each [[mock]] in the order
name, match, respond, sequence, …, `key = value` spacing, inline tables
kept inline. Comments are kept, and the meaning never changes: a file
whose formatted form would read differently is left alone, with an error.

--check changes nothing: it exits 1 and prints a diff when a file is not
formatted, for CI.

Examples:
  chrome-cdp fmt
  chrome-cdp fmt e2e/scenarios/checkout.cdp
  chrome-cdp fmt --check        # in CI, before chrome-cdp lint and test")]
    Fmt(FmtArgs),

    /// Check scenario, mock and audit baseline files for mistakes without running them.
    #[command(after_help = "\
Without paths, checks the project's E2E files, as fmt finds them. Every
problem has a place (file:line:col), a severity, a rule and a hint.

Scenario rules: syntax (and directive misuse), unknown-command,
unknown-flag, bad-args, unknown-directive, unknown-device,
unknown-backend, locator (an invalid locator), include-missing,
include-cycle, include-param (a ${name} the @include line does not
pass), undefined-var (a ${var:x} missing from a backend's vars, an
unknown ${saved.x}, a missing data file), ref (e12 refs are
session-only; write a stable locator), duplicate-test, and the warnings
mock-only (mock state/seed/ws send or expect mock-state in a test that
real backends run), mock-literal (a value from the mock data, in a test
real backends run), sleep (wait <ms>) and no-expect (a test that checks
nothing). Mock files get the `mock check` rules (unreachable,
collection-shadowed, openapi-…), and audit baselines (the .json files of
e2e/audit and .chrome-cdp/audit) the rule audit-baseline (not valid JSON,
a field `schema audit-baseline` does not have, a bad key or version).
Environment variables are not needed: ${env:…} and ${secret:…} are not
read.

Errors exit 1; warnings alone exit 0. --format github prints workflow
annotations, so problems show inline on pull requests; --format json is
--json. `chrome-cdp test` runs these checks first and refuses to start
on errors.

Examples:
  chrome-cdp lint
  chrome-cdp lint e2e/scenarios/checkout.cdp
  chrome-cdp lint --format github    # in a GitHub workflow
  chrome-cdp lint --json")]
    Lint(LintArgs),

    /// Set up a project for chrome-cdp: `init e2e` scaffolds E2E tests with mocks.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
Examples:
  chrome-cdp init e2e
  chrome-cdp init e2e --framework vite --dir web"
    )]
    Init(InitArgs),

    /// Show help: an overview, every command by task, a command in full, or a topic.
    #[command(after_help = "\
Topics: locators, refs, waiting, errors, config, scripts. `help commands`
lists every command by task; `help <command>` is the command's --help.

Examples:
  chrome-cdp help                  # the overview, as --help
  chrome-cdp help commands
  chrome-cdp help locators
  chrome-cdp help tab close")]
    Help(HelpArgs),

    /// Print a SKILL.md that teaches an agent to use chrome-cdp, or install it.
    #[command(after_help = "\
The skill is an Agent Skills file (Claude Code and others): frontmatter
with a name and description, then the workflow (open, snapshot -i, act by
ref, read the report, errors and console when stuck), the common pitfalls
and a cheat sheet, in under 2k tokens.

--install writes it to .claude/skills/chrome-cdp/SKILL.md in the project
(the nearest directory up with .git or .claude, else this one); with
--user to $CLAUDE_CONFIG_DIR/skills/… or ~/.claude/skills/…. Running it
again updates the file. --format agents-md prints a section for AGENTS.md
and --format llms-txt an llms.txt with every command.

Examples:
  chrome-cdp skill                          # print SKILL.md
  chrome-cdp skill --install                # into this project's .claude/skills
  chrome-cdp skill --install --user         # for every project
  chrome-cdp skill --format agents-md >> AGENTS.md
  chrome-cdp skill --format llms-txt > llms.txt")]
    Skill(SkillArgs),

    /// Print a shell completion script: bash, zsh, fish, elvish or powershell.
    #[command(after_help = "\
Examples:
  chrome-cdp completions bash > ~/.local/share/bash-completion/completions/chrome-cdp
  chrome-cdp completions zsh > ~/.zfunc/_chrome-cdp       # with fpath+=~/.zfunc
  chrome-cdp completions fish > ~/.config/fish/completions/chrome-cdp.fish")]
    Completions(CompletionsArgs),

    /// The session daemon; other commands start it.
    #[command(name = "__daemon", hide = true)]
    Daemon(DaemonArgs),
}

#[derive(Debug, Args)]
pub(crate) struct FmtArgs {
    /// Files or folders; the project's E2E files if none.
    #[arg(value_name = "PATH")]
    pub(crate) paths: Vec<PathBuf>,

    /// Change nothing: exit 1 and print a diff if a file is not formatted.
    #[arg(long)]
    pub(crate) check: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct LintArgs {
    /// Files or folders; the project's E2E files if none.
    #[arg(value_name = "PATH")]
    pub(crate) paths: Vec<PathBuf>,

    /// How to print the problems: text, json (as --json) or github
    /// (workflow annotations).
    #[arg(long, value_enum, default_value_t = LintFormat::Text, value_name = "FORMAT")]
    pub(crate) format: LintFormat,

    /// Check the mocks against this OpenAPI spec (JSON or YAML) instead of
    /// the config's `openapi`.
    #[arg(long, value_name = "FILE")]
    pub(crate) openapi: Option<PathBuf>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `lint --format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LintFormat {
    Text,
    Json,
    Github,
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    #[command(subcommand)]
    pub(crate) command: InitCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum InitCommand {
    /// Scaffold E2E tests: the [e2e] config, a scenario, a mock set and a CI workflow.
    #[command(after_help = "\
Writes, in the project directory (the current one, or --dir):

  chrome-cdp.toml               the [e2e] section (added to an existing
                                chrome-cdp.toml or .chrome-cdp.toml)
  e2e/scenarios/example.cdp     a scenario: steps are chrome-cdp commands
  e2e/mocks/base.toml           the mock set every test gets
  e2e/mocks/data/items.json     data the mocks answer with
  .github/workflows/e2e.yml     runs the tests on GitHub Actions
  .gitignore                    ignores e2e/.results/ (test artifacts)

The web server command and base_url come from package.json: Vite, Next.js,
Create React App, Astro or SvelteKit, run with npm, pnpm, yarn or bun (from
packageManager or the lock file). An existing file is never overwritten;
running it again changes nothing. `chrome-cdp help config` lists the [e2e]
keys.

Examples:
  chrome-cdp init e2e
  chrome-cdp init e2e --dir frontend
  chrome-cdp init e2e --framework next      # instead of reading package.json
  chrome-cdp init e2e --json                # each file and what was done")]
    E2e(InitE2eArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InitE2eArgs {
    /// The project's directory (default: the current one).
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,

    /// The app's framework, instead of the one package.json names.
    #[arg(long, value_enum, value_name = "NAME")]
    pub(crate) framework: Option<FrameworkArg>,
}

/// `init e2e --framework`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FrameworkArg {
    Vite,
    Next,
    Cra,
    Astro,
    Sveltekit,
    /// No framework: the web server is left for you to fill in.
    None,
}

#[derive(Debug, Args)]
pub(crate) struct HelpArgs {
    /// A topic (locators, refs, waiting, errors, config, scripts),
    /// `commands`, or a command (`click`, `tab close`).
    #[arg(value_name = "TOPIC")]
    pub(crate) topic: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct SkillArgs {
    /// What to print: the SKILL.md, an AGENTS.md section, or an llms.txt.
    #[arg(long, value_enum, default_value_t = SkillFormat::Skill)]
    pub(crate) format: SkillFormat,

    /// Write SKILL.md where Claude Code finds it (see --user) instead of
    /// printing it.
    #[arg(long, conflicts_with = "format")]
    pub(crate) install: bool,

    /// With --install: into this project's .claude/skills (the default).
    #[arg(long, requires = "install", conflicts_with = "user")]
    pub(crate) project: bool,

    /// With --install: into ~/.claude/skills, for every project.
    #[arg(long, requires = "install")]
    pub(crate) user: bool,
}

/// `skill --format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SkillFormat {
    /// SKILL.md, for Claude Code and other Agent Skills clients.
    Skill,
    /// A section to add to AGENTS.md.
    AgentsMd,
    /// llms.txt: a summary, the help topics and every command.
    LlmsTxt,
}

impl SkillFormat {
    /// The name `--format` takes.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::AgentsMd => "agents-md",
            Self::LlmsTxt => "llms-txt",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct CompletionsArgs {
    /// The shell to complete in.
    #[arg(value_enum, value_name = "SHELL")]
    pub(crate) shell: clap_complete::Shell,
}

#[derive(Debug, Args)]
pub(crate) struct SchemaArgs {
    /// The command, e.g. `version` or `tab close`; omit it to list them.
    #[arg(value_name = "COMMAND")]
    pub(crate) command: Vec<String>,
}

/// What `batch` and `run` share.
#[derive(Debug, Args)]
pub(crate) struct ScriptArgs {
    /// Run every step even after one fails; the exit code is the first
    /// failure's.
    #[arg(long = "continue")]
    pub(crate) keep_going: bool,

    /// Set ${KEY} in the script; repeat for more.
    #[arg(long = "var", value_name = "KEY=VALUE", value_parser = parse_var)]
    pub(crate) vars: Vec<(String, String)>,
}

#[derive(Debug, Args)]
pub(crate) struct BatchArgs {
    #[command(flatten)]
    pub(crate) script: ScriptArgs,
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// The script, e.g. login.cdp.
    #[arg(value_name = "FILE")]
    pub(crate) file: PathBuf,

    #[command(flatten)]
    pub(crate) script: ScriptArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ConvertArgs {
    /// The Playwright test file, e.g. tests/login.spec.ts; `-` reads stdin.
    #[arg(value_name = "SPEC", allow_hyphen_values = true)]
    pub(crate) file: PathBuf,

    /// Write the scenario to this file instead of printing it.
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,

    /// Bound the printed scenario to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `--var KEY=VALUE`.
fn parse_var(arg: &str) -> Result<(String, String), String> {
    let (key, value) = arg
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got `{arg}`"))?;
    if !protocol::script::is_name(key) {
        return Err(format!(
            "`{key}` is not a variable name: use letters, digits, _ and -, starting with a letter or _"
        ));
    }
    Ok((key.to_owned(), value.to_owned()))
}

#[derive(Debug, Args)]
pub(crate) struct InstallArgs {
    /// The release channel to install.
    #[arg(long, value_enum, default_value_t = Channel::Stable, conflicts_with = "version")]
    pub(crate) channel: Channel,

    /// A version, or the start of one: 154, 154.0.8037.57.
    #[arg(long, value_name = "VERSION")]
    pub(crate) version: Option<String>,

    /// Install chrome-headless-shell (smaller, headless only) instead of
    /// full Chrome.
    #[arg(long)]
    pub(crate) headless_shell: bool,

    /// Download again even if it is installed.
    #[arg(long)]
    pub(crate) force: bool,

    /// Linux: also install the system libraries Chrome needs (sudo apt-get
    /// or dnf) and, on Ubuntu 23.10+, the AppArmor profile its sandbox
    /// needs. Asks first on a terminal.
    #[arg(long)]
    pub(crate) with_deps: bool,

    /// List the installed browsers and each channel's current version.
    #[arg(
        long,
        conflicts_with_all = ["channel", "version", "headless_shell", "force", "with_deps"]
    )]
    pub(crate) list: bool,
}

/// A Chrome release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Channel {
    Stable,
    Beta,
    Dev,
    Canary,
}

impl Channel {
    pub(crate) fn to_browser(self) -> browser::Channel {
        match self {
            Self::Stable => browser::Channel::Stable,
            Self::Beta => browser::Channel::Beta,
            Self::Dev => browser::Channel::Dev,
            Self::Canary => browser::Channel::Canary,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct UninstallArgs {
    /// The install id, as `chrome-cdp install --list` shows it.
    #[arg(value_name = "ID")]
    pub(crate) id: String,
}

#[derive(Debug, Args)]
pub(crate) struct DoctorArgs {
    /// The Chrome or Chromium binary to check. Default: the one `open`
    /// would launch.
    #[arg(long, value_name = "PATH")]
    pub(crate) chrome: Option<PathBuf>,

    /// Do not start Chrome; only look at the files and the system.
    #[arg(long)]
    pub(crate) no_launch: bool,
}

#[derive(Debug, Args)]
pub(crate) struct UpgradeArgs {
    /// Only report whether a newer release exists.
    #[arg(long)]
    pub(crate) check: bool,

    /// Install this release (e.g. 1.2.0) instead of the newest, even an
    /// older one.
    #[arg(long, value_name = "VERSION")]
    pub(crate) version: Option<String>,

    /// Install even when this version is already running.
    #[arg(long, conflicts_with = "check")]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct OpenArgs {
    /// A URL to open in the current page; without a scheme, https:// (or
    /// http:// for localhost) is added.
    #[arg(value_name = "URL")]
    pub(crate) url: Option<String>,

    #[command(flatten)]
    pub(crate) launch: LaunchArgs,

    /// Close a running browser first and start one with these options.
    #[arg(long)]
    pub(crate) restart: bool,

    /// Emulate this device (as `emulate <device>`) before loading URL.
    /// The session keeps it, like `emulate`.
    #[arg(long, value_name = "DEVICE")]
    pub(crate) device: Option<String>,

    /// Run this file's script in every document before its own scripts,
    /// from URL on (as `init-script add FILE`). Repeatable; the session
    /// keeps them, and `init-script list` shows them.
    #[arg(long, value_name = "FILE")]
    pub(crate) init_script: Vec<PathBuf>,

    /// Like --init-script, with the script itself. Repeatable; these run
    /// after the --init-script files.
    #[arg(long, value_name = "JS")]
    pub(crate) init_script_code: Vec<String>,

    /// Load cookies and storage from a state file (as `state load`)
    /// before loading URL.
    #[arg(long, value_name = "FILE")]
    pub(crate) state: Option<PathBuf>,

    /// Make popups (window.open, target=_blank) the active tab as they
    /// open. The session keeps this setting.
    #[arg(long)]
    pub(crate) follow_popups: bool,

    /// Load pages despite certificate errors (self-signed test servers).
    /// The session keeps this setting; it applies to a running browser
    /// too.
    #[arg(long)]
    pub(crate) ignore_https_errors: bool,

    /// Send requests past service workers to the network, even without
    /// routes (routes always do, since a service worker would answer
    /// before them). The session keeps this setting; it applies to a
    /// running browser too.
    #[arg(long)]
    pub(crate) bypass_service_worker: bool,

    /// How to answer JavaScript dialogs. The session keeps this setting.
    #[arg(long, value_enum, value_name = "POLICY")]
    pub(crate) dialogs: Option<Dialogs>,

    /// Save downloads in this directory (created; relative to the current
    /// one) instead of the session's. The session keeps this setting; it
    /// applies to a running browser too.
    #[arg(long, value_name = "DIR")]
    pub(crate) downloads_dir: Option<PathBuf>,

    /// Answer HTTP authentication (basic, digest) with USER:PASSWORD; `-`
    /// reads it from stdin. The session keeps it; it applies to a running
    /// browser too. Every origin that asks gets it, unless
    /// --http-credentials-origin names one.
    #[arg(
        long,
        env = "CHROME_CDP_HTTP_CREDENTIALS",
        hide_env_values = true,
        value_name = "USER:PASSWORD"
    )]
    pub(crate) http_credentials: Option<String>,

    /// Send --http-credentials only to this origin (e.g.
    /// https://app.test); other origins' challenges are cancelled, so a
    /// third-party frame or a redirect elsewhere never gets them.
    #[arg(
        long,
        env = "CHROME_CDP_HTTP_CREDENTIALS_ORIGIN",
        value_name = "ORIGIN",
        requires = "http_credentials"
    )]
    pub(crate) http_credentials_origin: Option<String>,

    /// How many console entries each tab keeps (default 1,000). The
    /// session keeps this setting.
    #[arg(
        long,
        env = "CHROME_CDP_CONSOLE_BUFFER",
        value_parser = clap::value_parser!(u64).range(1..=1_000_000),
        value_name = "N"
    )]
    pub(crate) console_buffer: Option<u64>,

    /// How many requests each tab's network log keeps (default 2,000).
    /// The session keeps this setting.
    #[arg(
        long,
        env = "CHROME_CDP_NETWORK_BUFFER",
        value_parser = clap::value_parser!(u64).range(1..=1_000_000),
        value_name = "N"
    )]
    pub(crate) network_buffer: Option<u64>,

    /// Where Chrome records the stack that creates each DOM node, to tell
    /// which line of the page's code made an element. Recording costs
    /// page time, so by default only pages on this machine get it
    /// (`node_stacks` in the config sets the default). The session keeps
    /// this setting; it applies to a running browser too, for the nodes
    /// made from then on.
    #[arg(long, env = "CHROME_CDP_NODE_STACKS", value_enum, value_name = "WHERE")]
    pub(crate) node_stacks: Option<NodeStacks>,

    #[command(flatten)]
    pub(crate) idle: IdleArgs,
}

/// Where node creation stacks are recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum NodeStacks {
    /// Every page.
    On,
    /// No page.
    Off,
    /// Pages on localhost, *.localhost and loopback addresses (the
    /// default).
    Localhost,
}

impl NodeStacks {
    pub(crate) fn to_engine(self) -> engine::NodeStacks {
        match self {
            Self::On => engine::NodeStacks::On,
            Self::Off => engine::NodeStacks::Off,
            Self::Localhost => engine::NodeStacks::Localhost,
        }
    }
}

/// How the session answers JavaScript dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Dialogs {
    /// Accept alerts and beforeunload; confirm and prompt wait for
    /// `chrome-cdp dialog` (the default).
    Auto,
    /// Accept every dialog (a prompt with its default answer).
    Accept,
    /// Dismiss every dialog.
    Dismiss,
    /// Every dialog waits for `chrome-cdp dialog`.
    Manual,
}

impl Dialogs {
    pub(crate) fn to_engine(self) -> engine::DialogPolicy {
        match self {
            Self::Auto => engine::DialogPolicy::Auto,
            Self::Accept => engine::DialogPolicy::Accept,
            Self::Dismiss => engine::DialogPolicy::Dismiss,
            Self::Manual => engine::DialogPolicy::Manual,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct UploadArgs {
    /// The file input, its label, a button that opens a file chooser, or
    /// (with --drop) a drop zone: a ref (e12) or a locator that matches
    /// one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// The files to upload, or one directory for a folder input.
    #[arg(
        value_name = "FILE",
        num_args = 1..,
        required_unless_present = "clear",
        conflicts_with = "clear"
    )]
    pub(crate) files: Vec<PathBuf>,

    /// Leave the input with no files.
    #[arg(long)]
    pub(crate) clear: bool,

    /// Drop the files on the element with drag events, for a drop zone
    /// with no file input.
    #[arg(long, conflicts_with = "clear")]
    pub(crate) drop: bool,

    /// Skip the checks (enabled; for a button or drop zone also visible,
    /// stable and not covered).
    #[arg(long)]
    pub(crate) force: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ClipboardArgs {
    #[command(subcommand)]
    pub(crate) command: ClipboardCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClipboardCommand {
    /// Print the text on the clipboard, its HTML, or save its image.
    #[command(after_help = "\
A type the clipboard does not hold is an error that lists the types it
has. An image needs -o: a file, or - for stdout.

Examples:
  chrome-cdp clipboard read
  chrome-cdp click e7 && chrome-cdp clipboard read -q   # what a Copy button copied
  chrome-cdp clipboard read --type text/html
  chrome-cdp clipboard read --type image/png -o copied.png")]
    Read(ClipboardReadArgs),

    /// Put text, HTML or a PNG image on the clipboard.
    #[command(after_help = "\
With --html or --file, the clipboard gets one item with every part given;
--html without TEXT adds the HTML's text as text/plain, so a plain field
pastes it too.

Examples:
  chrome-cdp clipboard write 'hello'
  chrome-cdp clipboard write - < notes.txt
  chrome-cdp clipboard write --html '<p>Hello <b>world</b></p>'
  chrome-cdp clipboard write --file chart.png")]
    Write(ClipboardWriteArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ClipboardReadArgs {
    /// The type to read.
    #[arg(
        long = "type",
        value_name = "TYPE",
        default_value = "text/plain",
        value_parser = ["text/plain", "text/html", "image/png"]
    )]
    pub(crate) mime: String,

    /// Where to save an image (`-` for stdout); needed with
    /// `--type image/png`.
    #[arg(short, long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct ClipboardWriteArgs {
    /// The text; `-` reads it from stdin.
    #[arg(
        value_name = "TEXT",
        allow_hyphen_values = true,
        required_unless_present_any = ["html", "file"]
    )]
    pub(crate) text: Option<String>,

    /// HTML to put on the clipboard as text/html; `-` reads it from stdin.
    #[arg(long, value_name = "HTML", allow_hyphen_values = true)]
    pub(crate) html: Option<String>,

    /// A PNG image to put on the clipboard as image/png.
    #[arg(long, value_name = "IMAGE.png")]
    pub(crate) file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct DialogArgs {
    #[command(subcommand)]
    pub(crate) command: DialogCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DialogCommand {
    /// Accept the dialog (OK); a prompt gets TEXT, else its default.
    #[command(after_help = "\
Examples:
  chrome-cdp dialog accept
  chrome-cdp dialog accept 'Ada Lovelace'     # a prompt's answer
  chrome-cdp dialog accept --tab t2")]
    Accept(DialogAcceptArgs),

    /// Dismiss the dialog (Cancel; stay on the page for beforeunload).
    #[command(after_help = "\
Examples:
  chrome-cdp dialog dismiss
  chrome-cdp dialog dismiss --tab t2")]
    Dismiss(DialogDismissArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DialogAcceptArgs {
    /// The answer to a prompt; `-` reads it from stdin.
    #[arg(value_name = "TEXT")]
    pub(crate) text: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct DialogDismissArgs {}

#[derive(Debug, Args)]
pub(crate) struct SnapshotArgs {
    /// Only nodes with refs and the landmarks around them (much smaller).
    #[arg(short, long)]
    pub(crate) interactive: bool,

    /// Only this part of the page: a ref (e12) or any locator that
    /// matches one element.
    #[arg(long, value_name = "LOCATOR")]
    pub(crate) scope: Option<String>,

    /// Show this many levels; deeper nodes are counted, not shown.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) depth: Option<u32>,

    /// Only what changed since this tab's last snapshot: + added,
    /// - removed, ~ changed, with the nodes around them for context.
    #[arg(long)]
    pub(crate) diff: bool,

    /// A ref on every node, not only on those an agent acts on.
    #[arg(long)]
    pub(crate) all_refs: bool,

    /// Do not collapse long lists.
    #[arg(long)]
    pub(crate) all: bool,

    /// Cut text and names after this many characters (0 = no limit).
    #[arg(long, default_value_t = 200, value_name = "N")]
    pub(crate) text_limit: usize,

    /// Leave text out: the page's skeleton.
    #[arg(long)]
    pub(crate) no_text: bool,

    /// After each node with a ref, the component it is in and its place
    /// in the source (`<CartItem src/components/CartItem.tsx:42>`), as
    /// `source` finds them.
    #[arg(long, conflicts_with = "diff")]
    pub(crate) source: bool,

    /// Fold runs of four or more siblings with the same shape (cards,
    /// rows): the first is shown, then one `… N more … [region r5]` line.
    #[arg(long)]
    pub(crate) fold: bool,

    /// Fold parts of the page until the output is at most this many
    /// tokens (characters ÷ 3): runs of alike siblings, then regions (big
    /// lists, tables, sections) as one line each; 0 = none.
    /// `snapshot_budget` in the config sets a default.
    #[arg(long, value_name = "TOKENS")]
    pub(crate) budget: Option<usize>,

    /// Show these regions (r4) of the last folded snapshot, with the same
    /// --fold and --budget inside them.
    #[arg(long, value_name = "REGION", num_args = 1.., conflicts_with_all = ["scope", "diff"])]
    pub(crate) expand: Vec<String>,

    /// How to read the page: ax (Chrome's accessibility tree, the
    /// default) or dom (chrome-cdp's DOM walker).
    #[arg(long, value_enum, value_name = "ENGINE")]
    pub(crate) engine: Option<SnapshotEngineArg>,

    /// A --scope ref whose element is gone fails, instead of being
    /// re-resolved to the one element with its role and name.
    #[arg(
        long,
        env = "CHROME_CDP_STRICT_REFS",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) strict_refs: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = SNAPSHOT_MAX_CHARS, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `snapshot --max-chars`' default.
pub(crate) const SNAPSHOT_MAX_CHARS: usize = 20_000;

/// `snapshot --engine`.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum SnapshotEngineArg {
    /// Chrome's accessibility tree.
    Ax,
    /// chrome-cdp's DOM walker.
    Dom,
}

impl SnapshotEngineArg {
    pub(crate) fn to_engine(self) -> engine::SnapshotEngine {
        match self {
            Self::Ax => engine::SnapshotEngine::Ax,
            Self::Dom => engine::SnapshotEngine::Dom,
        }
    }
}

/// What an action does after its input (plan 05, milestone 5.5): wait
/// for what it caused and report it, and `--snap`. Global, so `mouse`'s
/// subcommands take them too.
#[derive(Debug, Args)]
pub(crate) struct AfterArgs {
    /// Return once the input is sent, without waiting for the page to
    /// react (a navigation to load, the requests it sent).
    #[arg(
        long,
        global = true,
        env = "CHROME_CDP_NO_WAIT_AFTER",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) no_wait_after: bool,

    /// Then print what changed on the page: snapshot --diff -i
    /// (snap_after_action = "diff" in the config files makes it the
    /// default).
    #[arg(
        long,
        global = true,
        env = "CHROME_CDP_SNAP",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true,
        overrides_with = "no_snap"
    )]
    pub(crate) snap: bool,

    /// No snapshot after it, whatever the config files say.
    #[arg(long, global = true, overrides_with = "snap")]
    pub(crate) no_snap: bool,

    /// Bound the output to this many characters (0 = no limit); only
    /// --snap's snapshot grows with the page.
    #[arg(long, global = true, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

impl AfterArgs {
    /// Whether to add the snapshot: `--snap`, else `snap_after_action`
    /// from the config files, unless `--no-snap`.
    pub(crate) fn snap(&self, global: &GlobalArgs) -> bool {
        !self.no_snap && (self.snap || global.snap_after_action)
    }
}

/// `--strict-refs` for commands that take a locator.
#[derive(Debug, Args)]
pub(crate) struct StrictRefsArgs {
    /// A ref whose element is gone fails, instead of being re-resolved to
    /// the one element with its role and name.
    #[arg(
        long,
        env = "CHROME_CDP_STRICT_REFS",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) strict_refs: bool,
}

/// `--position` and `--by`: `x,y`.
pub(crate) fn parse_pair(text: &str) -> Result<(f64, f64), String> {
    let (x, y) = text
        .split_once(',')
        .ok_or_else(|| format!("`{text}` is not x,y (e.g. 10,20)"))?;
    let number = |part: &str| {
        part.trim()
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("`{text}` is not x,y (e.g. 10,20)"))
    };
    Ok((number(x)?, number(y)?))
}

/// `scroll --to`: top, bottom, left, right or x,y.
fn parse_scroll_to(text: &str) -> Result<engine::ScrollTo, String> {
    Ok(match text.trim().to_ascii_lowercase().as_str() {
        "top" => engine::ScrollTo::Top,
        "bottom" => engine::ScrollTo::Bottom,
        "left" => engine::ScrollTo::Left,
        "right" => engine::ScrollTo::Right,
        _ => {
            let (x, y) = parse_pair(text)
                .map_err(|_| format!("`{text}` is not top, bottom, left, right or x,y"))?;
            engine::ScrollTo::Point { x, y }
        }
    })
}

/// `--modifiers`: one value holding them all (clap would take a `Vec`
/// for a repeated flag).
#[derive(Debug, Clone, Default)]
pub(crate) struct Modifiers(pub(crate) Vec<engine::Modifier>);

fn parse_modifiers(text: &str) -> Result<Modifiers, String> {
    engine::parse_modifiers(text).map(Modifiers)
}

/// `--button`.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub(crate) enum ButtonArg {
    #[default]
    Left,
    Right,
    Middle,
}

impl ButtonArg {
    pub(crate) fn to_button(self) -> engine::Button {
        match self {
            Self::Left => engine::Button::Left,
            Self::Right => engine::Button::Right,
            Self::Middle => engine::Button::Middle,
        }
    }
}

/// What `click`, `dblclick`, `hover` and `tap` share.
#[derive(Debug, Args)]
pub(crate) struct PointerArgs {
    /// The element: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// Keys held during it: Alt, Control, Meta, Shift, or Mod (Meta on
    /// macOS, Control elsewhere); comma-separated.
    #[arg(long, value_name = "KEYS", value_parser = parse_modifiers)]
    pub(crate) modifiers: Option<Modifiers>,

    /// Where on the element, in CSS pixels from the top-left corner of
    /// its box, instead of its centre.
    #[arg(long, value_name = "X,Y", value_parser = parse_pair, allow_hyphen_values = true)]
    pub(crate) position: Option<(f64, f64)>,

    /// Skip the checks (visible, stable, enabled, not covered); still
    /// scroll the element into view.
    #[arg(long)]
    pub(crate) force: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ClickArgs {
    #[command(flatten)]
    pub(crate) pointer: PointerArgs,

    /// The mouse button.
    #[arg(long, value_enum, default_value_t = ButtonArg::Left, value_name = "BUTTON")]
    pub(crate) button: ButtonArg,

    /// How many clicks in a row (2 is a double click).
    #[arg(long, default_value_t = 1, value_name = "N")]
    pub(crate) count: u32,
}

#[derive(Debug, Args)]
pub(crate) struct DblclickArgs {
    #[command(flatten)]
    pub(crate) pointer: PointerArgs,

    /// The mouse button.
    #[arg(long, value_enum, default_value_t = ButtonArg::Left, value_name = "BUTTON")]
    pub(crate) button: ButtonArg,
}

#[derive(Debug, Args)]
pub(crate) struct HoverArgs {
    #[command(flatten)]
    pub(crate) pointer: PointerArgs,
}

#[derive(Debug, Args)]
pub(crate) struct TapArgs {
    #[command(flatten)]
    pub(crate) pointer: PointerArgs,
}

#[derive(Debug, Args)]
pub(crate) struct DragArgs {
    /// The element to drag: a ref (e12) or a locator that matches one.
    #[arg(value_name = "FROM")]
    pub(crate) from: String,

    /// The element to drop it on.
    #[arg(value_name = "TO")]
    pub(crate) to: String,

    /// How many mouse moves from one to the other.
    #[arg(long, default_value_t = 10, value_name = "N")]
    pub(crate) steps: u32,

    /// Skip the checks (visible, stable, not covered) on both.
    #[arg(long)]
    pub(crate) force: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("amount").args(["by", "to"])))]
pub(crate) struct ScrollArgs {
    /// The element to scroll in (or into view); the page if omitted.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    /// Wheel by this much, in CSS pixels (positive is right and down).
    #[arg(long, value_name = "DX,DY", value_parser = parse_pair, allow_hyphen_values = true)]
    pub(crate) by: Option<(f64, f64)>,

    /// Wheel until this end: top, bottom, left, right, or a scroll
    /// offset x,y.
    #[arg(long, value_name = "WHERE", value_parser = parse_scroll_to)]
    pub(crate) to: Option<engine::ScrollTo>,

    /// Skip the checks on the element (visible, stable, not covered).
    #[arg(long)]
    pub(crate) force: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct MouseArgs {
    #[command(subcommand)]
    pub(crate) command: MouseCommand,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MouseCommand {
    /// Move the mouse to a point.
    #[command(
        allow_negative_numbers = true,
        after_help = "\
From where the mouse is, in --steps even moves; buttons held stay held
(a drag).

Examples:
  chrome-cdp mouse move 100 200
  chrome-cdp mouse move 400 200 --steps 20"
    )]
    Move(MouseMoveArgs),
    /// Press a mouse button (where the mouse is, or at x y).
    #[command(
        allow_negative_numbers = true,
        after_help = "\
The button stays held until `mouse up`, across commands.

Examples:
  chrome-cdp mouse down
  chrome-cdp mouse down 100 200 --button right"
    )]
    Down(MouseButtonArgs),
    /// Release a mouse button (where the mouse is, or at x y).
    #[command(
        allow_negative_numbers = true,
        after_help = "\
Examples:
  chrome-cdp mouse up
  chrome-cdp mouse up 300 200"
    )]
    Up(MouseButtonArgs),
    /// Turn the mouse wheel where the mouse is.
    #[command(
        allow_negative_numbers = true,
        after_help = "\
DX and DY are CSS pixels: positive scrolls right and down. Use `scroll`
to scroll an element and see where it ended.

Examples:
  chrome-cdp mouse wheel 0 400
  chrome-cdp mouse wheel 0 -400"
    )]
    Wheel(MouseWheelArgs),
}

#[derive(Debug, Args)]
pub(crate) struct KeyboardArgs {
    #[command(subcommand)]
    pub(crate) command: KeyboardCommand,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum KeyboardCommand {
    /// Press a key and hold it until `keyboard up`.
    #[command(after_help = "\
Name the key as `press` does: Shift, Control, Alt, Meta, Mod (Meta on
macOS, Control elsewhere), Enter, a, A, … A held key is not pressed again.

Examples:
  chrome-cdp keyboard down Shift
  chrome-cdp keyboard down Mod
  chrome-cdp keyboard down a               # a keydown the page sees, held")]
    Down(KeyboardDownArgs),

    /// Release a held key, or all of them.
    #[command(after_help = "\
A key that is not held is a warning, not an error.

Examples:
  chrome-cdp keyboard up Shift
  chrome-cdp keyboard up --all")]
    Up(KeyboardUpArgs),
}

#[derive(Debug, Args)]
pub(crate) struct KeyboardDownArgs {
    /// The key: Shift, Control, Mod, Enter, a, …
    #[arg(value_name = "KEY")]
    pub(crate) key: String,
}

#[derive(Debug, Args)]
pub(crate) struct KeyboardUpArgs {
    /// The key to release.
    #[arg(
        value_name = "KEY",
        required_unless_present = "all",
        conflicts_with = "all"
    )]
    pub(crate) key: Option<String>,

    /// Release every held key, the last held first.
    #[arg(long)]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct MouseMoveArgs {
    /// CSS pixels from the viewport's left edge.
    #[arg(value_name = "X")]
    pub(crate) x: f64,

    /// CSS pixels from the viewport's top edge.
    #[arg(value_name = "Y")]
    pub(crate) y: f64,

    /// How many moves to get there.
    #[arg(long, default_value_t = 1, value_name = "N")]
    pub(crate) steps: u32,
}

#[derive(Debug, Args)]
pub(crate) struct MouseButtonArgs {
    /// Move here first: CSS pixels from the viewport's left edge.
    #[arg(value_name = "X", requires = "y")]
    pub(crate) x: Option<f64>,

    /// And from its top edge.
    #[arg(value_name = "Y")]
    pub(crate) y: Option<f64>,

    /// The mouse button: left for down; for up, the one held.
    #[arg(long, value_enum, value_name = "BUTTON")]
    pub(crate) button: Option<ButtonArg>,

    /// The click count the event carries (2 for the second press of a
    /// double click).
    #[arg(long, default_value_t = 1, value_name = "N")]
    pub(crate) count: u32,
}

#[derive(Debug, Args)]
pub(crate) struct MouseWheelArgs {
    /// Horizontal delta in CSS pixels.
    #[arg(value_name = "DX")]
    pub(crate) dx: f64,

    /// Vertical delta in CSS pixels.
    #[arg(value_name = "DY")]
    pub(crate) dy: f64,
}

#[derive(Debug, Args)]
pub(crate) struct PressArgs {
    /// Keys or chords, space-separated: Enter, Control+A, "Tab Tab Enter".
    #[arg(value_name = "KEYS")]
    pub(crate) keys: String,

    /// Focus this element first: a ref (e12) or a locator.
    #[arg(long = "in", value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct TypeArgs {
    /// The text; `-` reads it from stdin.
    #[arg(value_name = "TEXT", allow_hyphen_values = true)]
    pub(crate) text: String,

    /// Focus this element first: a ref (e12) or a locator.
    #[arg(long = "in", value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    /// Wait this long between keys (e.g. 50ms), for pages that react to
    /// each one.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub(crate) delay: Option<Duration>,

    /// Skip the checks on the --in element (visible, stable, enabled,
    /// editable).
    #[arg(long, requires = "locator")]
    pub(crate) force: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct FillArgs {
    /// The field: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// The text; `-` reads it from stdin; "" clears the field.
    #[arg(value_name = "TEXT", allow_hyphen_values = true)]
    pub(crate) text: String,

    /// Skip the checks (visible, stable, enabled, editable).
    #[arg(long)]
    pub(crate) force: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct SelectArgs {
    /// The <select>: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// Options' values or labels.
    #[arg(value_name = "VALUE", required = true, num_args = 1..)]
    pub(crate) values: Vec<String>,

    /// Skip the checks (visible, stable, enabled).
    #[arg(long)]
    pub(crate) force: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CheckArgs {
    /// The checkbox, radio button, switch, or its label: a ref (e12) or
    /// a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// Skip the checks before the click (visible, stable, enabled, not
    /// covered).
    #[arg(long)]
    pub(crate) force: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct FocusArgs {
    /// The element: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct FindArgs {
    /// Words to look for; `-` reads them from stdin.
    #[arg(value_name = "QUERY", required = true, num_args = 1..)]
    pub(crate) query: Vec<String>,

    /// Only nodes with this role (button, link, textbox, heading, …).
    #[arg(long, value_name = "ROLE")]
    pub(crate) role: Option<String>,

    /// Only search this part of the page: a ref (e12) or any locator that
    /// matches one element.
    #[arg(long, value_name = "LOCATOR")]
    pub(crate) scope: Option<String>,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Show at most this many matches, best first (0 = all).
    #[arg(long, default_value_t = 10, value_name = "N")]
    pub(crate) limit: usize,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct LocatorArgs {
    /// The element: a ref (e12) or any locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// List every candidate tried, with how many elements each finds.
    #[arg(long)]
    pub(crate) all: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ReadTextArgs {
    /// The element(s): a ref (e12) or any locator; the page if omitted.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct FormsArgs {
    /// The form: a ref (e12) or a locator that matches it or an element
    /// in it; every form if left out.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct FillFormArgs {
    /// The form: a ref (e12) or a locator that matches it or an element
    /// in it; the page's only form if left out.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    /// The values, a JSON object of field names to values; `-` reads it
    /// from stdin.
    #[arg(
        long,
        value_name = "JSON",
        allow_hyphen_values = true,
        required_unless_present = "file",
        conflicts_with = "file"
    )]
    pub(crate) values: Option<String>,

    /// Read the values from this JSON file.
    #[arg(long, value_name = "FILE")]
    pub(crate) file: Option<std::path::PathBuf>,

    /// Then submit the form: click its first submit button, or press
    /// Enter in the last field filled.
    #[arg(long)]
    pub(crate) submit: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    #[command(flatten)]
    pub(crate) after: AfterArgs,
}

#[derive(Debug, Args)]
pub(crate) struct WhyArgs {
    /// The element: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// The action whose checks run.
    #[arg(long = "for", value_enum, default_value_t = WhyForArg::Click, value_name = "ACTION")]
    pub(crate) action: WhyForArg,

    /// Leave the page where it is: an element out of view is not
    /// hit-tested.
    #[arg(long)]
    pub(crate) no_scroll: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,
}

/// `why --for`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum WhyForArg {
    Click,
    Hover,
    Fill,
    Type,
    Check,
    Select,
    Upload,
    Drag,
}

impl WhyForArg {
    pub(crate) fn to_engine(self) -> engine::WhyFor {
        match self {
            Self::Click => engine::WhyFor::Click,
            Self::Hover => engine::WhyFor::Hover,
            Self::Fill => engine::WhyFor::Fill,
            Self::Type => engine::WhyFor::Type,
            Self::Check => engine::WhyFor::Check,
            Self::Select => engine::WhyFor::Select,
            Self::Upload => engine::WhyFor::Upload,
            Self::Drag => engine::WhyFor::Drag,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct LayoutArgs {
    /// Check only inside this element: a ref (e12) or a locator that
    /// matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    #[command(flatten)]
    pub(crate) flags: LayoutFlags,

    /// List info findings too (text truncated on purpose).
    #[arg(long)]
    pub(crate) all: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct CompareArgs {
    /// The first page: a URL, tab:<id>, session:<name> or a snapshot
    /// --json file.
    #[arg(value_name = "A")]
    pub(crate) a: String,

    /// The second page, the same kinds as A: what it has that A has not
    /// is marked +.
    #[arg(value_name = "B")]
    pub(crate) b: String,

    /// Compare only nodes with refs and the landmarks around them.
    #[arg(short, long)]
    pub(crate) interactive: bool,

    /// Compare only this part of each page: a locator that matches one
    /// element on both.
    #[arg(long, value_name = "LOCATOR")]
    pub(crate) scope: Option<String>,

    /// Compare screenshots of the two viewports too.
    #[arg(long)]
    pub(crate) visual: bool,

    /// With --visual: how far a pixel's colour may move before it
    /// differs (0 to 1).
    #[arg(long, default_value_t = 0.1, value_name = "T")]
    pub(crate) threshold: f64,

    /// With --visual: the viewport both pages are shot at, WIDTHxHEIGHT
    /// (A's if left out).
    #[arg(long, value_name = "WxH", value_parser = parse_size)]
    pub(crate) viewport: Option<(u32, u32)>,

    /// Exit 1 when anything differs.
    #[arg(long)]
    pub(crate) exit_code: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct ExploreArgs {
    /// Where to start (relative to base_url); the active tab's page if
    /// left out.
    #[arg(value_name = "URL")]
    pub(crate) url: Option<String>,

    /// Stop after finding this many states.
    #[arg(long, default_value_t = 30, value_name = "N")]
    pub(crate) max_states: u64,

    /// Stop after this many clicks and loads, replayed ones included.
    #[arg(long, default_value_t = 200, value_name = "N")]
    pub(crate) max_actions: u64,

    /// Follow nothing more than this many steps from the start.
    #[arg(long, default_value_t = 3, value_name = "N")]
    pub(crate) depth: u64,

    /// Stop after this long, e.g. 30s, 5m.
    #[arg(long, default_value = "2m", value_parser = parse_duration, value_name = "DURATION")]
    pub(crate) time: Duration,

    /// Click buttons, tabs, menu items and disclosure controls too, not
    /// only links.
    #[arg(long)]
    pub(crate) click_buttons: bool,

    /// Click buttons that submit a form.
    #[arg(long)]
    pub(crate) submit_forms: bool,

    /// Click controls whose name looks destructive (delete, log out,
    /// pay, …).
    #[arg(long)]
    pub(crate) allow_destructive: bool,

    /// Follow links to this origin too (repeatable).
    #[arg(long = "origin", value_name = "ORIGIN")]
    pub(crate) origins: Vec<String>,

    /// Follow only links to URLs this pattern matches (repeatable).
    #[arg(long, value_name = "PATTERN")]
    pub(crate) include: Vec<String>,

    /// Never follow links to URLs this pattern matches (repeatable).
    #[arg(long, value_name = "PATTERN")]
    pub(crate) exclude: Vec<String>,

    /// Record layout's error findings in each state.
    #[arg(long)]
    pub(crate) layout: bool,

    /// With -o: save a screenshot of each state.
    #[arg(long)]
    pub(crate) screenshots: bool,

    /// Write map.json and states/<id>.cdp here (an empty or new
    /// directory).
    #[arg(short = 'o', long = "output", value_name = "DIR")]
    pub(crate) output: Option<PathBuf>,

    /// Exit 1 when a state had an error, a failed request, a broken link
    /// or a layout error.
    #[arg(long)]
    pub(crate) exit_code: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `WIDTHxHEIGHT`, in CSS pixels.
fn parse_size(text: &str) -> Result<(u32, u32), String> {
    text.split_once(['x', 'X'])
        .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
        .filter(|&(w, h): &(u32, u32)| w > 0 && h > 0)
        .ok_or_else(|| format!("expected WIDTHxHEIGHT, e.g. 1280x800, got {text:?}"))
}

/// The flags `layout` and `expect --layout` share.
#[derive(Debug, Default, Args)]
pub(crate) struct LayoutFlags {
    /// Check at each of these viewports: mobile, tablet, desktop or WxH,
    /// comma-separated.
    #[arg(long, value_name = "LIST", value_delimiter = ',')]
    pub(crate) viewports: Vec<String>,

    /// Check only these kinds, comma-separated: covered, overlap,
    /// clipped, truncated, overflow-x, offscreen, small-target, contrast.
    #[arg(long, value_name = "KINDS", value_delimiter = ',')]
    pub(crate) only: Vec<String>,

    /// Look for covered elements and low contrast down the whole page, a
    /// viewport at a time, not only in view.
    #[arg(long)]
    pub(crate) full: bool,

    /// The least width and height of a target, in CSS pixels (default
    /// 24).
    #[arg(long, value_name = "PX")]
    pub(crate) min_target: Option<u32>,

    /// Check again with every text this many percent longer, and report
    /// what only that finds.
    #[arg(long, value_name = "PCT")]
    pub(crate) stretch_text: Option<u32>,
}

impl LayoutFlags {
    /// The first flag given, for an error that says it needs --layout.
    pub(crate) fn given(&self) -> Option<&'static str> {
        [
            (!self.viewports.is_empty(), "--viewports"),
            (!self.only.is_empty(), "--only"),
            (self.full, "--full"),
            (self.min_target.is_some(), "--min-target"),
            (self.stretch_text.is_some(), "--stretch-text"),
        ]
        .into_iter()
        .find_map(|(given, flag)| given.then_some(flag))
    }
}

#[derive(Debug, Args)]
pub(crate) struct HtmlArgs {
    /// The element: a ref (e12) or a locator that matches one; the whole
    /// document if omitted.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    /// Include the element itself, not only its content.
    #[arg(long)]
    pub(crate) outer: bool,

    /// The browser's own serialisation, not pretty-printed.
    #[arg(long)]
    pub(crate) raw: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct AttrArgs {
    /// The element: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// The attribute, e.g. href.
    #[arg(value_name = "NAME")]
    pub(crate) name: String,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct ValueArgs {
    /// The field: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct CountArgs {
    /// What to count: any locator.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,
}

#[derive(Debug, Args)]
pub(crate) struct BoxArgs {
    /// The element: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,
}

#[derive(Debug, Args)]
pub(crate) struct SourceArgs {
    /// The element: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// List at most this many components around it; the rest are
    /// counted.
    #[arg(long, default_value_t = 8, value_name = "N")]
    pub(crate) depth: u32,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ComponentsArgs {
    /// The element whose component is shown: a ref (e12) or a locator
    /// that matches one; the whole tree if omitted.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    /// The tree's levels (default 4), or with a locator the owners
    /// listed (default 8; the rest are counted).
    #[arg(long, value_name = "N")]
    pub(crate) depth: Option<u32>,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MarkdownArgs {
    /// The element: a ref (e12) or a locator that matches one; the page's
    /// main content if omitted.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct DownloadsArgs {
    /// Delete the files of finished downloads and forget them.
    #[arg(long)]
    pub(crate) clear: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct ConsoleArgs {
    /// The least severe level shown.
    #[arg(long, value_enum, default_value_t = LevelArg::Info, value_name = "LEVEL")]
    pub(crate) level: LevelArg,

    #[command(flatten)]
    pub(crate) read: LogReadArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ErrorsArgs {
    #[command(flatten)]
    pub(crate) read: LogReadArgs,
}

#[derive(Debug, Args)]
pub(crate) struct IssuesArgs {
    /// Where to start: `last` (what this command has not shown yet), an
    /// issue number such as 12 (0 for all), a duration such as 30s
    /// (raised in the last 30 seconds), or a mark (see `mark`).
    #[arg(long, default_value = "last", value_parser = parse_since, value_name = "WHEN")]
    pub(crate) since: engine::Since,

    /// Only issues whose rule, message or URL matches this regular
    /// expression.
    #[arg(long, value_name = "REGEX")]
    pub(crate) grep: Option<String>,

    /// Show at most this many issues, the newest (0 = all); with
    /// --all-tabs, per tab.
    #[arg(long, default_value_t = 50, value_name = "N")]
    pub(crate) limit: usize,

    /// Keep printing new issues as they come, until Ctrl-C (or
    /// --timeout, if given).
    #[arg(short, long, conflicts_with = "all_tabs")]
    pub(crate) follow: bool,

    /// The least severe issues shown.
    #[arg(long, value_enum, default_value_t = SeverityArg::Warning, value_name = "SEVERITY")]
    pub(crate) min_severity: SeverityArg,

    /// Every tab of the session (or context), each from its own cursor.
    #[arg(long)]
    pub(crate) all_tabs: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct AuditArgs {
    /// Check inside this element what can be scoped (a11y, layout, the
    /// forms of security): a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: Option<String>,

    #[command(flatten)]
    pub(crate) flags: AuditFlags,

    /// The least severe findings shown.
    #[arg(long, value_enum, default_value_t = SeverityArg::Warning, value_name = "SEVERITY")]
    pub(crate) min_severity: SeverityArg,

    /// Also list the findings the baseline accepts, marked (accepted).
    #[arg(long, requires = "baseline")]
    pub(crate) show_accepted: bool,

    /// Every tab of the session (or context).
    #[arg(long)]
    pub(crate) all_tabs: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// What `audit` and `expect --audit` share.
#[derive(Debug, Args)]
pub(crate) struct AuditFlags {
    /// The categories to check (comma-separated or repeated); all by
    /// default, or with a LOCATOR those it can scope.
    #[arg(
        long = "category",
        value_enum,
        value_delimiter = ',',
        value_name = "CATEGORY"
    )]
    pub(crate) categories: Vec<AuditCategoryArg>,

    /// The accepted findings: a .json file, or NAME for <dir>/audit/NAME.json.
    #[arg(long, value_name = "FILE|NAME")]
    pub(crate) baseline: Option<String>,

    /// Write the findings into the baseline (keeping its notes; dropping
    /// stale entries).
    #[arg(long, requires = "baseline")]
    pub(crate) update: bool,

    /// Where a baseline NAME is (default: .chrome-cdp in the project;
    /// e2e under `chrome-cdp test`).
    #[arg(long, value_name = "DIR", requires = "baseline")]
    pub(crate) dir: Option<PathBuf>,
}

impl AuditFlags {
    /// The categories as the engine takes them.
    pub(crate) fn category_names(&self) -> Vec<String> {
        self.categories
            .iter()
            .filter_map(|c| c.to_possible_value().map(|v| v.get_name().to_owned()))
            .collect()
    }
}

/// `audit --category`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum AuditCategoryArg {
    /// Chrome's issues (see `issues`).
    Issues,
    /// The accessibility rules of expect --a11y, level AA.
    A11y,
    /// `layout`'s findings.
    Layout,
    /// Uncaught exceptions and console errors.
    Console,
    /// Failed requests and 4xx/5xx responses.
    Network,
    /// http:, CSP, HSTS, cookies and forms.
    Security,
    /// Title, lang, viewport, description and robots.
    Meta,
}

/// `--min-severity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SeverityArg {
    /// Errors only: things that break for some users or leak data.
    Error,
    /// Warnings and errors: things that will break or degrade too.
    Warning,
    /// Everything.
    Info,
}

impl SeverityArg {
    pub(crate) fn to_severity(self) -> protocol::schema::Severity {
        match self {
            Self::Error => protocol::schema::Severity::Error,
            Self::Warning => protocol::schema::Severity::Warning,
            Self::Info => protocol::schema::Severity::Info,
        }
    }
}

/// What `console` and `errors` share.
#[derive(Debug, Args)]
pub(crate) struct LogReadArgs {
    /// Where to start: `last` (what this command has not shown yet), an
    /// entry number such as 12 (0 for all), a duration such as 30s
    /// (logged in the last 30 seconds), or a mark (see `mark`).
    #[arg(long, default_value = "last", value_parser = parse_since, value_name = "WHEN")]
    pub(crate) since: engine::Since,

    /// Only entries whose text or URL matches this regular expression.
    #[arg(long, value_name = "REGEX")]
    pub(crate) grep: Option<String>,

    /// Show at most this many entries, the newest (0 = all).
    #[arg(long, default_value_t = 50, value_name = "N")]
    pub(crate) limit: usize,

    /// Keep printing new entries as they come, until Ctrl-C (or
    /// --timeout, if given).
    #[arg(short, long)]
    pub(crate) follow: bool,

    /// Resolve stack frames with the scripts' source maps (always on for
    /// localhost pages).
    #[arg(long)]
    pub(crate) sourcemaps: bool,

    /// Show the local variables of the top 3 frames of each uncaught
    /// error, read when it was thrown (localhost pages; see `debug
    /// locals`).
    #[arg(long)]
    pub(crate) locals: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `console --level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LevelArg {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warn,
    /// Everything but debug messages.
    Info,
    /// Everything, `console.debug` and verbose browser messages too.
    Debug,
}

impl LevelArg {
    pub(crate) fn to_level(self) -> protocol::schema::Level {
        match self {
            Self::Error => protocol::schema::Level::Error,
            Self::Warn => protocol::schema::Level::Warn,
            Self::Info => protocol::schema::Level::Info,
            Self::Debug => protocol::schema::Level::Debug,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct NetworkArgs {
    #[command(subcommand)]
    pub(crate) command: Option<NetworkCommand>,

    #[command(flatten)]
    pub(crate) list: NetworkListArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum NetworkCommand {
    /// Show one request in full: status, sizes, timing, initiator, and
    /// its headers and bodies on request.
    #[command(after_help = "\
A request a script sent shows the script's stack when it sent it (8
frames at most), across `await`s, promises and timers (`-- async
Promise.then --`; recorded where the debugger is on: localhost pages, see
`debug locals`), resolved with source maps on localhost pages (and
elsewhere with --sourcemaps).

Bodies are fetched from Chrome only when asked for. Chrome keeps them while
its buffer has room and the page that loaded them is open; if it no longer
has one, the output says so. JSON bodies are pretty-printed.

Examples:
  chrome-cdp network show 42
  chrome-cdp network show 42 --body
  chrome-cdp network show 42 --headers --request-body
  chrome-cdp network show 42 --body -q > body.json   # just the body
  chrome-cdp network show 7 -o logo.png              # save a body, binary too")]
    Show(NetworkShowArgs),

    /// List a WebSocket's frames, and with --follow new ones as they come.
    #[command(after_help = "\
ID is the socket's request id (`network --type websocket` lists them).
The log keeps each socket's last 500 frames, up to 200 characters of each
text frame, and the first 32 bytes of a binary one, in hex (what --grep
matches for it). Each line reads: N DIRECTION KIND SIZE TEXT, N counting
the socket's frames from 1. With
--follow, new frames print as they come until the socket closes, Ctrl-C,
or --timeout. `wait --ws` waits for one frame.

Examples:
  chrome-cdp network frames 12
  chrome-cdp network frames 12 --grep '\"type\":\"price\"' --dir received
  chrome-cdp network frames 12 --follow --timeout 10s
  chrome-cdp network frames 12 -q                 # just the texts")]
    Frames(NetworkFramesArgs),

    /// Forget the tab's requests so far (ids go on from where they were).
    #[command(after_help = "\
Examples:
  chrome-cdp network clear
  chrome-cdp network clear --tab t2")]
    Clear(NetworkClearArgs),
}

#[derive(Debug, Args)]
pub(crate) struct NetworkListArgs {
    /// Only requests that failed, or got a status of 400 or more.
    #[arg(long)]
    pub(crate) failed: bool,

    /// Only these resource types, comma-separated: document, script,
    /// stylesheet, image, font, media, xhr, fetch, websocket, other, …
    #[arg(long = "type", value_delimiter = ',', value_parser = parse_type, value_name = "TYPES")]
    pub(crate) types: Vec<String>,

    /// Only URLs that match: a glob (`**/api/*`), a `/regex/`, or a part
    /// of the URL.
    #[arg(long, value_name = "PATTERN")]
    pub(crate) url: Option<String>,

    /// Only this HTTP method (GET, POST, …).
    #[arg(long, value_name = "METHOD")]
    pub(crate) method: Option<String>,

    /// Only these statuses, comma-separated: 404, 4xx or 500-599.
    #[arg(long, value_delimiter = ',', value_parser = parse_status, value_name = "STATUS")]
    pub(crate) status: Vec<engine::StatusRange>,

    /// Where to start: `last` (what this command has not shown yet), a
    /// request id such as 12 (0 for all), a duration such as 30s
    /// (started in the last 30 seconds), or a mark (see `mark`).
    #[arg(long, default_value = "last", value_parser = parse_since, value_name = "WHEN")]
    pub(crate) since: engine::Since,

    /// Show at most this many requests, the newest (0 = all).
    #[arg(long, default_value_t = 50, value_name = "N")]
    pub(crate) limit: usize,

    /// Keep printing new requests (and pending ones as they end) as they
    /// come, until Ctrl-C (or --timeout, if given).
    #[arg(short, long)]
    pub(crate) follow: bool,

    /// Resolve initiator stacks with the scripts' source maps (always on
    /// for localhost pages).
    #[arg(long)]
    pub(crate) sourcemaps: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct NetworkShowArgs {
    /// The request's id, as `network` lists it: 42 or #42.
    #[arg(value_parser = parse_request_id, value_name = "ID")]
    pub(crate) id: u64,

    /// List its request and response headers.
    #[arg(long)]
    pub(crate) headers: bool,

    /// Fetch and show the response body.
    #[arg(long)]
    pub(crate) body: bool,

    /// Show the request body.
    #[arg(long)]
    pub(crate) request_body: bool,

    /// Save the response body to this file (decoded: an image as its
    /// bytes) and print its path, instead of showing it.
    #[arg(short, long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,

    /// Resolve initiator stacks with the scripts' source maps (always on
    /// for localhost pages).
    #[arg(long)]
    pub(crate) sourcemaps: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct NetworkFramesArgs {
    /// The WebSocket's request id, as `network` lists it: 12 or #12.
    #[arg(value_parser = parse_request_id, value_name = "ID")]
    pub(crate) id: u64,

    /// Only frames whose text (a binary frame's hex) matches this regular
    /// expression.
    #[arg(long, value_name = "REGEX")]
    pub(crate) grep: Option<String>,

    /// Only frames the page sent, or only those it received.
    #[arg(long, value_enum, value_name = "DIR")]
    pub(crate) dir: Option<FrameDirArg>,

    /// Show at most this many frames, the newest (0 = all).
    #[arg(long, default_value_t = 100, value_name = "N")]
    pub(crate) limit: usize,

    /// Keep printing new frames as they come, until the socket closes,
    /// Ctrl-C, or --timeout if given.
    #[arg(short, long)]
    pub(crate) follow: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `network frames --dir`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FrameDirArg {
    Sent,
    Received,
}

#[derive(Debug, Args)]
pub(crate) struct NetworkClearArgs {}

#[derive(Debug, Args)]
pub(crate) struct ScreenshotArgs {
    /// Capture only this element: a ref (e12) or a locator that matches
    /// one.
    #[arg(value_name = "LOCATOR", conflicts_with = "full")]
    pub(crate) locator: Option<String>,

    /// The whole page, not only what the viewport shows. A page that
    /// scrolls inside a container (a scrolling <main>) has that
    /// container's whole content captured; LOC names the container if the
    /// one found is not it.
    #[arg(long, value_name = "LOC", num_args = 0..=1, default_missing_value = "")]
    pub(crate) full: Option<String>,

    /// The image format; else the -o file's extension, else png.
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub(crate) format: Option<ImageFormatArg>,

    /// Quality from 0 to 100, for jpeg and webp.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(0..=100))]
    pub(crate) quality: Option<u8>,

    /// Where to save it (`-` for stdout); a numbered file in the session's
    /// shots/ directory if not given.
    #[arg(short, long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,

    /// css: one image pixel per CSS pixel; device: as many as the screen
    /// has (the device pixel ratio).
    #[arg(long, value_enum, default_value_t = ShotScaleArg::Css, value_name = "SCALE")]
    pub(crate) scale: ShotScaleArg,

    /// The longest side in pixels; a larger image is scaled down to fit
    /// (0 = no limit).
    #[arg(long, default_value_t = 1568, value_name = "PX")]
    pub(crate) max_size: u32,

    /// A transparent background where the page has none (png and webp).
    #[arg(long)]
    pub(crate) omit_background: bool,

    /// Paint over the elements this locator matches (repeatable).
    #[arg(long, value_name = "LOCATOR", action = ArgAction::Append)]
    pub(crate) mask: Vec<String>,

    /// Label the elements `snapshot -i` gives refs with their refs.
    #[arg(long)]
    pub(crate) annotate: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,
}

/// `screenshot --format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ImageFormatArg {
    Png,
    Jpeg,
    Webp,
}

impl ImageFormatArg {
    pub(crate) fn to_format(self) -> engine::ImageFormat {
        match self {
            Self::Png => engine::ImageFormat::Png,
            Self::Jpeg => engine::ImageFormat::Jpeg,
            Self::Webp => engine::ImageFormat::Webp,
        }
    }
}

/// `screenshot --scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ShotScaleArg {
    /// One image pixel per CSS pixel.
    Css,
    /// The device pixel ratio.
    Device,
}

impl ShotScaleArg {
    pub(crate) fn to_scale(self) -> engine::ShotScale {
        match self {
            Self::Css => engine::ShotScale::Css,
            Self::Device => engine::ShotScale::Device,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct RouteArgs {
    /// Which URLs: a glob (`**/api/*`), a `/regex/`, or a part of the URL.
    #[arg(value_name = "PATTERN")]
    pub(crate) pattern: String,

    /// Only these HTTP methods (GET, POST, …), comma-separated.
    #[arg(long, value_delimiter = ',', value_name = "METHODS")]
    pub(crate) method: Vec<String>,

    /// Only these resource types, comma-separated: document, script,
    /// stylesheet, image, font, media, xhr (or fetch: Chrome does not tell
    /// them apart here), websocket, other, …
    #[arg(long, value_delimiter = ',', value_parser = parse_type, value_name = "TYPES")]
    pub(crate) resource_type: Vec<String>,

    /// Remove the rule after it handled this many requests.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..), value_name = "N")]
    pub(crate) times: Option<u32>,

    /// Hold each matched request this long first (e.g. 2s, 500ms).
    #[arg(long, value_parser = parse_duration, value_name = "DURATION")]
    pub(crate) delay: Option<Duration>,

    /// Answer with this status (200 unless given).
    #[arg(long, value_parser = clap::value_parser!(u16).range(100..=599), value_name = "CODE")]
    pub(crate) status: Option<u16>,

    /// Answer with this text (`-` reads stdin); text/plain unless a
    /// --header sets Content-Type.
    #[arg(long, value_name = "TEXT", conflicts_with_all = ["json_body", "body_file"])]
    pub(crate) body: Option<String>,

    /// Answer with this JSON (`-` reads stdin), as application/json.
    #[arg(long, value_name = "JSON", conflicts_with = "body_file")]
    pub(crate) json_body: Option<String>,

    /// Answer with this file, read now; its extension sets Content-Type.
    #[arg(long, value_name = "PATH")]
    pub(crate) body_file: Option<PathBuf>,

    /// A response header, as 'Name: value'. Repeat for more.
    #[arg(long = "header", value_name = "NAME:VALUE")]
    pub(crate) headers: Vec<String>,

    /// Fail the request with this network error (default `failed`):
    /// failed, aborted, timedout, accessdenied, connectionrefused,
    /// connectionreset, namenotresolved, internetdisconnected,
    /// blockedbyclient, …
    #[arg(long, num_args = 0..=1, default_missing_value = "failed", value_name = "REASON")]
    pub(crate) abort: Option<String>,

    /// Send the request on with this header set, as 'Name: value'.
    /// Repeat for more.
    #[arg(long = "set-header", value_name = "NAME:VALUE")]
    pub(crate) set_headers: Vec<String>,

    /// Send the request on without this header. Repeat for more.
    #[arg(long = "remove-header", value_name = "NAME")]
    pub(crate) remove_headers: Vec<String>,

    /// Send the request to this URL instead (the page does not see it).
    #[arg(long, value_name = "URL")]
    pub(crate) rewrite_url: Option<String>,

    /// Send the request on and merge this JSON into the response's JSON
    /// (`-` reads stdin): objects merge key by key, null removes a key,
    /// anything else replaces.
    #[arg(long, value_name = "JSON")]
    pub(crate) patch_json: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct BlockArgs {
    /// URLs to block: globs (`**/*.mp4`), /regexes/, or parts of URLs.
    #[arg(value_name = "PATTERN")]
    pub(crate) patterns: Vec<String>,

    /// Block images.
    #[arg(long)]
    pub(crate) images: bool,

    /// Block web fonts.
    #[arg(long)]
    pub(crate) fonts: bool,

    /// Block audio and video.
    #[arg(long)]
    pub(crate) media: bool,

    /// Block well-known ad and tracking hosts.
    #[arg(long)]
    pub(crate) ads: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RoutesArgs {
    /// List the requests routing saw instead of the rules.
    #[arg(long)]
    pub(crate) log: bool,

    /// With --log: show at most this many requests, the newest (0 = all).
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub(crate) limit: usize,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("which").required(true).args(["ids", "all"]))]
pub(crate) struct UnrouteArgs {
    /// The rules' ids, as `routes` lists them (r1, r2, …).
    #[arg(value_name = "ID")]
    pub(crate) ids: Vec<String>,

    /// Remove every rule.
    #[arg(long, conflicts_with = "ids")]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct HistoryCmdArgs {
    /// Print the commands as a test with this name (`@test "NAME"`).
    #[arg(long, value_name = "NAME")]
    pub(crate) as_scenario: Option<String>,

    /// Start at command N (as `history` numbers them).
    #[arg(long, value_name = "N")]
    pub(crate) from: Option<u64>,

    /// Forget the commands so far (after printing them).
    #[arg(long)]
    pub(crate) clear: bool,

    /// Without --as-scenario: show at most this many commands, the newest
    /// (0 = all).
    #[arg(long, default_value_t = 50, value_name = "N")]
    pub(crate) limit: usize,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct ReproArgs {
    #[command(subcommand)]
    pub(crate) command: ReproCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ReproCommand {
    /// Mark where the repro starts, and keep the requests' bodies from now.
    #[command(after_help = "\
Keeps the session's cookies and storage (as `state save` does), the time
(the bundle's test freezes it there), the page's viewport, locale and time
zone, and the history position. From now on each request's response body
is kept as it finishes loading (Chrome drops a page's bodies once it
navigates away); --scope keeps only some (path or full-URL globs, as a
mock set's scope). The page open now is where the bundle's test starts.
Starting again starts over. Needs a running session.

Examples:
  chrome-cdp repro start
  chrome-cdp repro start --scope '/api/**' --scope '/app/**'")]
    Start(ReproStartArgs),

    /// Write the bundle: a test, its mocks, the state and the evidence.
    #[command(after_help = "\
DIR gets chrome-cdp.toml (a project whose every request is answered by
mocks/repro.toml; one no mock answers fails), repro.cdp (the commands since
`repro start` as a test, as `history --as-scenario` writes them, ending
with the last command that failed, its error in a comment), mocks/repro.toml and mocks/data/ (every request recorded),
state.json (the cookies and storage at the start), evidence/ (the last
snapshot, a screenshot, console, errors and requests) and README.md (what
happened, the last error, versions, how to replay). Bodies over --max-body
(5MB), and images and fonts over a fifth of it, become placeholders; the
README lists them. Passwords typed into password fields become
${secret:PASSWORD}; ${secret:…} values stay placeholders, and the README
names the secrets a replay needs. Without `repro start` it uses the whole
history, an empty state and the network log as it is, and warns. DIR must
be empty or new, unless --force.

Examples:
  chrome-cdp repro save bugs/cart-total
  chrome-cdp repro save bugs/cart-total --name \"cart total ignores the coupon\"
  chrome-cdp test bugs/cart-total")]
    Save(ReproSaveArgs),
}

#[derive(Debug, Args)]
pub(crate) struct RecordArgs {
    #[command(subcommand)]
    pub(crate) command: RecordCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RecordCommand {
    /// Start recording what happens in the page.
    #[command(after_help = "\
From now on every page and iframe of the session's tabs (or --context's)
reports what is done in it, from the active tab on. Each element is named
at the moment it is acted on, by the most robust locator that finds it
alone (testid= for the configured testid_attribute). Starting again drops
the earlier recording. Needs a running session.

Examples:
  chrome-cdp record start
  chrome-cdp record start --name \"signs in\"")]
    Start(RecordStartArgs),

    /// How many steps the recording has so far.
    #[command(after_help = "\
Typing still going on counts as one step.

Examples:
  chrome-cdp record status
  chrome-cdp record status -q     # just the number")]
    Status(RecordStatusArgs),

    /// Stop recording and print the test, or append it to a file.
    #[command(after_help = "\
Prints `@test \"NAME\"` and its steps, ending with a comment asking for
`expect` steps (the recorder guesses no assertions). URLs under base_url
become paths. A password field's value is never recorded: the step fills
${secret:PASSWORD}. A file chosen gets `upload LOC NAME` and a TODO for
its path. With -o, the test is appended to FILE (made if missing).

Examples:
  chrome-cdp record stop
  chrome-cdp record stop -o e2e/scenarios/signin.cdp
  chrome-cdp test e2e/scenarios/signin.cdp")]
    Stop(RecordStopArgs),
}

#[derive(Debug, Args)]
pub(crate) struct RecordStartArgs {
    /// The recorded test's name (`@test "NAME"`).
    #[arg(long, default_value = "recorded", value_name = "NAME")]
    pub(crate) name: String,
}

#[derive(Debug, Args)]
pub(crate) struct RecordStatusArgs {}

#[derive(Debug, Args)]
pub(crate) struct RecordStopArgs {
    /// Append the test to this file (made if missing) instead of printing
    /// it.
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub(crate) output: Option<PathBuf>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct ReproStartArgs {
    /// Keep only the bodies of these requests: a path glob (`/api/**`) or
    /// a full-URL glob (`https://api.example.com/**`); repeatable.
    #[arg(long, value_name = "GLOB")]
    pub(crate) scope: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ReproSaveArgs {
    /// The bundle's directory: new or empty, unless --force.
    #[arg(value_name = "DIR")]
    pub(crate) dir: PathBuf,

    /// The test's name (`@test "NAME"`); the directory's name if not given.
    #[arg(long, value_name = "NAME")]
    pub(crate) name: Option<String>,

    /// Bodies bigger than this become placeholders (images and fonts: a
    /// fifth of it): `5MB`, `200KB`, or bytes.
    #[arg(long, default_value = "5MB", value_name = "SIZE", value_parser = parse_bytes)]
    pub(crate) max_body: u64,

    /// Write into a directory that has files (the bundle's own are
    /// replaced).
    #[arg(long)]
    pub(crate) force: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 4_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `5MB`, `200KB`, `1.5MB`, `1024`: a size in bytes (K and M are 1024s).
fn parse_bytes(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let upper = t.to_ascii_uppercase();
    let (number, scale) = if let Some(n) = upper.strip_suffix("MB").or(upper.strip_suffix('M')) {
        (n, 1024.0 * 1024.0)
    } else if let Some(n) = upper.strip_suffix("KB").or(upper.strip_suffix('K')) {
        (n, 1024.0)
    } else {
        (upper.strip_suffix('B').unwrap_or(&upper), 1.0)
    };
    let number: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("`{text}` is no size; write 5MB, 200KB or a number of bytes"))?;
    if !(number.is_finite() && number >= 0.0) {
        return Err(format!("`{text}` is no size"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok((number * scale).round() as u64)
}

#[derive(Debug, Args)]
pub(crate) struct MockArgs {
    #[command(subcommand)]
    pub(crate) command: MockCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MockCommand {
    /// Answer requests with mock sets, instead of the sets in use.
    #[command(after_help = "\
The sets are the project's `default_mocks` ([e2e] in chrome-cdp.toml)
followed by the SETs named, so `mock use checkout` layers checkout onto the
defaults, as a scenario's `@mocks checkout` does. A SET is a name in the
mocks directory (`errors/api-down` is errors/api-down.toml) or a path to a
.toml file. Later sets are tried first. Every file is checked before any
is used; a problem is reported at file:line:col.

Examples:
  chrome-cdp mock use                      # the default_mocks
  chrome-cdp mock use checkout
  chrome-cdp mock use --add errors/api-down
  chrome-cdp mock use --no-defaults ./tmp/experiment.toml")]
    Use(MockUseArgs),

    /// Stop answering requests with mock sets (all, or those named).
    #[command(after_help = "\
Without mock sets or routes left, requests are no longer intercepted.

Examples:
  chrome-cdp mock off
  chrome-cdp mock off errors/api-down")]
    Off(MockOffArgs),

    /// List the mock sets in use, their mocks and hits.
    #[command(after_help = "\
Sets are listed in the order they are tried, each with its mocks:
file:line, name, what it matches, what it answers, and how many requests
it answered. --available lists the sets in the mocks directory instead;
it needs no session.

Examples:
  chrome-cdp mock list
  chrome-cdp mock list --available")]
    List(MockListArgs),

    /// The requests the mocks answered, and those no mock answered.
    #[command(after_help = "\
Each line reads: request number, tab, method, URL, type, then the mock
that answered it (file:line and name) and what it answered; or `no mock
in SET` for a request in a set's scope that no mock matched. --all adds
the requests no set had anything to do with.

Examples:
  chrome-cdp mock log
  chrome-cdp mock log --unmatched
  chrome-cdp mock log --all --limit 100")]
    Log(MockLogArgs),

    /// Start the mocks' state over: counters, sequences, named values, collections.
    #[command(after_help = "\
`{{seq}}` counts from 1 again, sequences start at their first answer,
named values are as [state] declares them, collections hold their seed
items again (seed files are read again), and `{{uuid}}` and
`{{faker.*}}` give the same values as after `mock use`.

Examples:
  chrome-cdp mock reset")]
    Reset(MockResetArgs),

    /// Which mock answers a request, and why the others do not.
    #[command(after_help = "\
Checks every mock of the sets in use, in the order they are tried, and
says which one answers the request and, for each other one, the first
condition that does not hold. A URL starting with / is relative to
base_url. Without a session using mock sets, it explains with --mocks, or
the project's default_mocks. Nothing is sent, and no hit is counted.

Examples:
  chrome-cdp mock explain GET /api/products/7
  chrome-cdp mock explain POST /api/login --json-body '{\"email\":\"ada@example.com\"}'
  chrome-cdp mock explain GET 'https://api.example.com/v1/cart?user=7' --mocks base")]
    Explain(MockExplainArgs),

    /// Show the mocks' named values and collections, or one of them.
    #[command(after_help = "\
Without a NAME: every named value (name = value) and every collection
with its item count. With a NAME: that value, or the collection's items,
as JSON (-q prints it on one line). --set changes named values first;
the VALUE is JSON (true, 3, [\"a\"], null to remove it), else text. Only
names a [state] declares, or a mock has set, can be set.

Examples:
  chrome-cdp mock state
  chrome-cdp mock state todos
  chrome-cdp mock state --set logged_in=true
  chrome-cdp mock state user --set user=ada@example.com")]
    State(MockStateArgs),

    /// Fill a collection with other items, or its seed items again.
    #[command(after_help = "\
FILE is a JSON array of objects, each with the collection's id field: a
path in the mocks directory (like a mock file's data files), else
relative to the current directory; `-` reads the JSON from stdin. Without
a FILE the collection starts over with its own seed items. Other
collections and named values are left as they are.

Examples:
  chrome-cdp mock seed todos data/many-todos.json
  chrome-cdp mock seed todos
  echo '[{\"id\":1,\"title\":\"Only one\"}]' | chrome-cdp mock seed todos -")]
    Seed(MockSeedArgs),

    /// Write a mock set from the requests the session sent (to a real backend).
    #[command(after_help = "\
Reads the session's network log (every tab of the context, or --tab's),
keeps the requests in --scope (globs as a set's scope takes them; without
it, the page's fetch and XHR requests), and writes one [[mock]] per
distinct request: method, path, query (and a GraphQL operation). A request
sent more than once with other answers becomes a `sequence`. Bodies go to
data files in data/<set>/ (small ones inline); volatile headers (date,
etag, set-cookie unless a later request sent the cookie, …) are left out;
ids and timestamps get a comment where a template would do. The file is
checked as `mock use` would read it.

--match-bodies tells requests to one path apart by their bodies (two
logins as different users): their mocks match `json` (or `form`, or a
GraphQL request's variables) with the body's stable fields, leaving out
fields whose values differ and look generated (UUIDs, the time, long
tokens). Requests with equal stable fields still form a sequence.

Browse first, then record: bodies are asked of Chrome, which keeps them
while the page that loaded them is open.

Examples:
  chrome-cdp mock record --scope '/api/**'
  chrome-cdp mock record --scope '/api/**' --match-bodies
  chrome-cdp mock record --scope '/api/**' -o e2e/mocks/checkout.toml --force
  chrome-cdp mock record --scope 'https://api.example.com/**' --tab t2")]
    Record(MockRecordArgs),

    /// Write a mock set from a HAR file, as `mock record` does.
    #[command(after_help = "\
The same conversion as `mock record`, from a HAR file (`har stop`, the
DevTools Network panel's \"Save all as HAR\", or Playwright's). The output is
<mocks dir>/<file's name>.toml unless -o says otherwise. --match-bodies
tells requests to one path apart by their bodies' stable fields.

Examples:
  chrome-cdp mock import requests.har --scope '/api/**'
  chrome-cdp mock import requests.har --match-bodies
  chrome-cdp mock import requests.har -o e2e/mocks/recorded.toml --force")]
    Import(MockImportArgs),

    /// Lint mock files, and check them against an OpenAPI spec.
    #[command(after_help = "\
Checks every mock file in the mocks directory (or the SETs named) for what
`mock use` would refuse (format, missing data files, templates), mocks an
earlier one always answers for, collections a mock always answers for,
collections of one name in two files, and state names no [state]
declares. With an OpenAPI spec (--openapi, or `openapi` in [e2e]), each
mock's path, method, status and JSON body must fit the operation it
stands for, which catches mocks the real API has moved away from.

Each problem reads file:line:col, severity, rule and message. Errors exit
1; warnings alone exit 0. `chrome-cdp test` runs the check on the sets it
uses before it starts, and stops on errors.

Examples:
  chrome-cdp mock check
  chrome-cdp mock check base checkout --openapi api/openapi.yaml
  chrome-cdp mock check --json")]
    Check(MockCheckArgs),

    /// Serve mock sets as a real HTTP server, for server-side fetches (SSR).
    #[command(after_help = "\
Answers HTTP requests with the same mocks as `mock use` (the default_mocks
followed by the SETs named), for requests that never pass through the
browser: the fetches of an SSR framework's server (Next.js, Remix,
SvelteKit load functions). Point the app at it (API_URL=http://localhost:4010).
It runs until Ctrl-C and prints each request it answered; mock files are
read again when they change.

State (counters, sequences, named values, collections) is shared by all
requests, except that a request with an `x-chrome-cdp-test: <id>` header
gets state of its own for that id. `chrome-cdp test` sends the header with
its browser requests; an app that forwards it to its API calls gets state
per test. `abort` closes the connection, `passthrough` answers 502 (there
is no real server behind), a request no set owns answers 404, and an `sse`
answer streams its events.

In a test run, `mock_server = { port = 4010 }` in [e2e.web_server] starts
one before the web server, with each test's own mock sets.

Examples:
  chrome-cdp mock serve --port 4010
  chrome-cdp mock serve checkout --no-defaults --port 0
  chrome-cdp mock serve base --host 0.0.0.0 --port 4010")]
    Serve(MockServeArgs),

    /// Talk to the page's mocked WebSockets.
    #[command(subcommand)]
    Ws(MockWsCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum MockWsCommand {
    /// Send a message to the page's sockets that a [[websocket]] mock answers.
    #[command(after_help = "\
Hands MESSAGE, as it is (with --base64, its bytes as a binary message), to
every open socket of the context whose URL the glob matches (a path glob
such as /ws/**, a full-URL glob, or * for all), as if the fake server sent
it. Waits (up to --timeout) for such a socket to open. Only sockets a
[[websocket]] mock answers can be sent to: the page's and its workers'
`WebSocket`s talk to the mocks after `mock use`.

Examples:
  chrome-cdp mock ws send '/ws/**' '{\"type\":\"price\",\"value\":42}'
  chrome-cdp mock ws send '*' hello
  chrome-cdp mock ws send /ws/feed AQID --base64    # a binary message: bytes 1, 2, 3
  echo '{\"type\":\"bye\"}' | chrome-cdp mock ws send /ws/prices -")]
    Send(MockWsSendArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MockServeArgs {
    /// Mock sets to serve after the defaults: names in the mocks
    /// directory, or paths to .toml files.
    #[arg(value_name = "SET")]
    pub(crate) sets: Vec<String>,

    /// The port to listen on (0 picks a free one).
    #[arg(long, default_value_t = 4010, value_name = "PORT")]
    pub(crate) port: u16,

    /// The address to listen on.
    #[arg(long, default_value = "127.0.0.1", value_name = "ADDR")]
    pub(crate) host: String,

    /// Leave out the project's default_mocks.
    #[arg(long)]
    pub(crate) no_defaults: bool,

    /// The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks).
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct MockWsSendArgs {
    /// Which sockets: a path glob (/ws/**), a full-URL glob, or * for all.
    #[arg(value_name = "URL")]
    pub(crate) url: String,

    /// The message, sent as it is; `-` reads stdin.
    #[arg(value_name = "MESSAGE")]
    pub(crate) message: String,

    /// MESSAGE is base64: send its bytes as a binary message.
    #[arg(long)]
    pub(crate) base64: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MockCheckArgs {
    /// The sets to check (names in the mocks directory, or .toml paths);
    /// every set in the mocks directory if none.
    #[arg(value_name = "SET")]
    pub(crate) sets: Vec<String>,

    /// Check the mocks against this OpenAPI spec (JSON or YAML) instead of
    /// the config's `openapi`.
    #[arg(long, value_name = "FILE")]
    pub(crate) openapi: Option<PathBuf>,

    /// The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks).
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MockRecordArgs {
    /// Record the requests these globs match (repeatable, or
    /// comma-separated): paths (`/api/**`) or full URLs. Default: the
    /// page's fetch and XHR requests.
    #[arg(long, value_name = "GLOB", value_delimiter = ',')]
    pub(crate) scope: Vec<String>,

    /// The mock file to write (default: recorded.toml in the mocks
    /// directory).
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub(crate) output: Option<PathBuf>,

    /// Tell requests to one path apart by their bodies: match the JSON or
    /// form fields that do not look generated (ids, the time, tokens).
    #[arg(long)]
    pub(crate) match_bodies: bool,

    /// Overwrite the mock file (and its data files) if it exists.
    #[arg(long)]
    pub(crate) force: bool,

    /// The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks).
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MockImportArgs {
    /// The HAR file; `-` reads it from stdin.
    #[arg(value_name = "FILE")]
    pub(crate) file: String,

    /// Import the requests these globs match (repeatable, or
    /// comma-separated): paths (`/api/**`) or full URLs. Default: the
    /// page's fetch and XHR requests.
    #[arg(long, value_name = "GLOB", value_delimiter = ',')]
    pub(crate) scope: Vec<String>,

    /// The mock file to write (default: <FILE's name>.toml in the mocks
    /// directory).
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub(crate) output: Option<PathBuf>,

    /// Tell requests to one path apart by their bodies: match the JSON or
    /// form fields that do not look generated (ids, the time, tokens).
    #[arg(long)]
    pub(crate) match_bodies: bool,

    /// Overwrite the mock file (and its data files) if it exists.
    #[arg(long)]
    pub(crate) force: bool,

    /// The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks).
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MockUseArgs {
    /// Mock sets to apply after the defaults: names in the mocks
    /// directory, or paths to .toml files.
    #[arg(value_name = "SET")]
    pub(crate) sets: Vec<String>,

    /// Layer the sets onto the ones in use (without the defaults).
    #[arg(long)]
    pub(crate) add: bool,

    /// Leave out the project's default_mocks.
    #[arg(long)]
    pub(crate) no_defaults: bool,

    /// The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks).
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,

    /// Tag the requests to --test-origin with `x-chrome-cdp-test: ID`, so
    /// `mock serve` gives them state of their own (`chrome-cdp test` does
    /// this for each test).
    #[arg(long, value_name = "ID", requires = "test_origins", hide = true)]
    pub(crate) test_id: Option<String>,

    /// With --test-id: an origin whose requests get the header
    /// (repeatable), e.g. http://localhost:5173.
    #[arg(
        long = "test-origin",
        value_name = "URL",
        requires = "test_id",
        hide = true
    )]
    pub(crate) test_origins: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct MockOffArgs {
    /// The sets to turn off; all when none is named.
    #[arg(value_name = "SET")]
    pub(crate) sets: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct MockListArgs {
    /// List the sets in the mocks directory instead (needs no session).
    #[arg(long)]
    pub(crate) available: bool,

    /// With --available: the mocks directory.
    #[arg(long, value_name = "DIR", requires = "available")]
    pub(crate) dir: Option<PathBuf>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MockLogArgs {
    /// Only the requests no mock answered (in a set's scope).
    #[arg(long, conflicts_with = "all")]
    pub(crate) unmatched: bool,

    /// Every request routing saw, not only mocked and unmatched ones.
    #[arg(long)]
    pub(crate) all: bool,

    /// Show at most this many requests, the newest (0 = all).
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub(crate) limit: usize,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MockResetArgs {}

#[derive(Debug, Args)]
pub(crate) struct MockStateArgs {
    /// A named value or collection to show; all of them when not given.
    #[arg(value_name = "NAME")]
    pub(crate) name: Option<String>,

    /// Change a named value first, NAME=VALUE (repeatable); VALUE is JSON,
    /// else text.
    #[arg(long = "set", value_name = "NAME=VALUE")]
    pub(crate) set: Vec<String>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MockSeedArgs {
    /// The collection.
    #[arg(value_name = "COLLECTION")]
    pub(crate) name: String,

    /// A JSON data file with the items; `-` reads them from stdin. Without
    /// it, the collection's own seed items.
    #[arg(value_name = "FILE")]
    pub(crate) file: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct HttpArgs {
    /// The method: GET, POST, PUT, PATCH, DELETE, …
    #[arg(value_name = "METHOD")]
    pub(crate) method: String,

    /// The URL: absolute, or a path relative to base_url.
    #[arg(value_name = "URL")]
    pub(crate) url: String,

    /// A JSON body (sent as application/json); `-` reads stdin.
    #[arg(long, value_name = "JSON", conflicts_with = "body")]
    pub(crate) json_body: Option<String>,

    /// A text body; `-` reads stdin.
    #[arg(long, value_name = "TEXT")]
    pub(crate) body: Option<String>,

    /// A request header, `Name: value` (repeatable).
    #[arg(long = "header", value_name = "HEADER")]
    pub(crate) headers: Vec<String>,

    /// The statuses that pass, comma-separated: 201, 2xx, 4xx, or any.
    #[arg(long, default_value = "2xx", value_name = "STATUSES")]
    pub(crate) expect_status: String,

    /// In a test scenario: keep the JSON answer as ${saved.NAME…}.
    #[arg(long, value_name = "NAME")]
    pub(crate) save: Option<String>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct MockExplainArgs {
    /// The request's method: GET, POST, …
    #[arg(value_name = "METHOD")]
    pub(crate) method: String,

    /// Its URL: absolute, or a path relative to base_url.
    #[arg(value_name = "URL")]
    pub(crate) url: String,

    /// A JSON body (sent as application/json); `-` reads stdin.
    #[arg(long, value_name = "JSON", conflicts_with = "body")]
    pub(crate) json_body: Option<String>,

    /// A text body; `-` reads stdin.
    #[arg(long, value_name = "TEXT")]
    pub(crate) body: Option<String>,

    /// A request header, `Name: value` (repeatable).
    #[arg(long = "header", value_name = "HEADER")]
    pub(crate) headers: Vec<String>,

    /// Explain with these sets (repeatable, or comma-separated) instead
    /// of the session's.
    #[arg(long, value_name = "SET", value_delimiter = ',')]
    pub(crate) mocks: Vec<String>,

    /// With --mocks or the defaults: the mocks directory.
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,

    /// Show at most this many mocks (0 = all).
    #[arg(long, default_value_t = 30, value_name = "N")]
    pub(crate) limit: usize,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct InitScriptArgs {
    #[command(subcommand)]
    pub(crate) command: InitScriptCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum InitScriptCommand {
    /// Add a script that runs in every new document before its own.
    #[command(after_help = "\
Prints the script's id (s1, s2, …), its world and size. It runs from each
page's next load (`reload` for the current one), or at once in the open
pages too with --now. The script is checked in the active tab first; one
that does not compile is refused. Errors it throws name its file (or
init-script-s1.js) in `console` and `errors`.

--world main (the default) runs it with the page's globals, to stub or
wrap them; --world isolated runs it where `eval --world isolated` runs,
with the same DOM but out of the page's reach.

Examples:
  chrome-cdp init-script add ./stubs.js
  chrome-cdp init-script add --code 'localStorage.setItem(\"tour-done\", \"1\")'
  chrome-cdp init-script add --code 'window.FEATURE_X = true' --now
  cat stubs.js | chrome-cdp init-script add -")]
    Add(InitScriptAddArgs),

    /// List the init scripts, in the order they run.
    #[command(after_help = "\
Each line reads: id, world, file (or the script's first line), size.

Examples:
  chrome-cdp init-script list")]
    List(InitScriptListArgs),

    /// Remove init scripts from new documents.
    #[command(after_help = "\
Open pages keep what a script already did until they load again.

Examples:
  chrome-cdp init-script remove s1
  chrome-cdp init-script remove --all")]
    Remove(InitScriptRemoveArgs),
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("script").required(true).args(["file", "code"]))]
pub(crate) struct InitScriptAddArgs {
    /// The script's file; `-` reads it from stdin.
    #[arg(value_name = "FILE")]
    pub(crate) file: Option<PathBuf>,

    /// The script itself, instead of a file.
    #[arg(long, value_name = "JS", conflicts_with = "file")]
    pub(crate) code: Option<String>,

    /// Where it runs: the page's own world, or chrome-cdp's isolated one.
    #[arg(long, value_enum, default_value_t = WorldArg::Main)]
    pub(crate) world: WorldArg,

    /// Also run it in the documents open now.
    #[arg(long)]
    pub(crate) now: bool,
}

#[derive(Debug, Args)]
pub(crate) struct InitScriptListArgs {
    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("which").required(true).args(["ids", "all"]))]
pub(crate) struct InitScriptRemoveArgs {
    /// The scripts' ids, as `init-script list` shows them (s1, s2, …).
    #[arg(value_name = "ID")]
    pub(crate) ids: Vec<String>,

    /// Remove every init script.
    #[arg(long, conflicts_with = "ids")]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ExposeArgs {
    /// Global function names (letters, digits, _ and $).
    #[arg(value_name = "NAME")]
    pub(crate) names: Vec<String>,

    /// Each call's promise resolves to this JSON value.
    #[arg(
        long,
        value_name = "JSON",
        requires = "names",
        allow_hyphen_values = true
    )]
    pub(crate) returns: Option<String>,

    /// The calls get these values in call order, the last one from then
    /// on (a JSON array).
    #[arg(
        long,
        value_name = "JSON-ARRAY",
        requires = "names",
        conflicts_with = "returns"
    )]
    pub(crate) returns_seq: Option<String>,
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("which").required(true).args(["names", "all"]))]
pub(crate) struct UnexposeArgs {
    /// The functions' names.
    #[arg(value_name = "NAME")]
    pub(crate) names: Vec<String>,

    /// Take every exposed function away.
    #[arg(long, conflicts_with = "names")]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct EmulateArgs {
    /// A device name as `--list` shows it (case and spaces are loose), or
    /// `reset`. Without it, prints what the session emulates.
    #[arg(value_name = "DEVICE", num_args = 0.., conflicts_with = "list")]
    pub(crate) device: Vec<String>,

    /// List the devices.
    #[arg(long)]
    pub(crate) list: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ViewportArgs {
    /// WIDTHxHEIGHT in CSS pixels (e.g. 1440x900), or `reset`.
    #[arg(value_name = "SIZE")]
    pub(crate) size: String,

    /// The device pixel ratio (devicePixelRatio), e.g. 2 or 3.
    #[arg(long, value_name = "N")]
    pub(crate) dpr: Option<f64>,

    /// A mobile browser: the page's meta viewport counts, scrollbars
    /// overlay.
    #[arg(long)]
    pub(crate) mobile: bool,

    /// Touch events, and navigator.maxTouchPoints > 0 (for `tap`).
    #[arg(long)]
    pub(crate) touch: bool,
}

#[derive(Debug, Args)]
pub(crate) struct GeoArgs {
    /// LATITUDE,LONGITUDE in degrees (e.g. 59.9139,10.7522), or `reset`.
    #[arg(value_name = "LAT,LON", allow_hyphen_values = true)]
    pub(crate) position: String,

    /// How precise the position is, in metres.
    #[arg(long, default_value_t = 10.0, value_name = "METRES")]
    pub(crate) accuracy: f64,
}

#[derive(Debug, Args)]
pub(crate) struct NameArgs {
    /// The value, or `reset` for the browser's own.
    #[arg(value_name = "NAME")]
    pub(crate) name: String,
}

#[derive(Debug, Args)]
pub(crate) struct UaArgs {
    /// The user agent (`-` reads stdin), or `reset`.
    #[arg(value_name = "USER-AGENT")]
    pub(crate) user_agent: String,
}

#[derive(Debug, Args)]
pub(crate) struct MediaArgs {
    /// `reset`: every media feature back to the browser's own.
    #[arg(value_name = "reset", value_parser = ["reset"], conflicts_with_all = ["type", "color_scheme", "reduced_motion", "forced_colors"])]
    pub(crate) reset: Option<String>,

    /// The media type.
    #[arg(long = "type", value_parser = ["screen", "print"], value_name = "TYPE")]
    pub(crate) r#type: Option<String>,

    /// prefers-color-scheme.
    #[arg(long, value_parser = ["light", "dark"], value_name = "SCHEME")]
    pub(crate) color_scheme: Option<String>,

    /// prefers-reduced-motion.
    #[arg(long, value_parser = ["reduce", "no-preference"], value_name = "VALUE")]
    pub(crate) reduced_motion: Option<String>,

    /// forced-colors (Windows high contrast).
    #[arg(long, value_parser = ["active", "none"], value_name = "VALUE")]
    pub(crate) forced_colors: Option<String>,
}

/// `on` or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OnOff {
    On,
    Off,
}

#[derive(Debug, Args)]
pub(crate) struct OfflineArgs {
    /// `on` takes the network away, `off` gives it back.
    #[arg(value_enum, value_name = "on|off")]
    pub(crate) state: OnOff,
}

#[derive(Debug, Args)]
pub(crate) struct ThrottleArgs {
    /// slow-3g, fast-3g, 4g, or `off`.
    #[arg(value_name = "PRESET")]
    pub(crate) preset: Option<String>,

    /// Added before each response starts, e.g. 300ms.
    #[arg(long, value_parser = parse_duration, value_name = "DURATION")]
    pub(crate) latency: Option<Duration>,

    /// Download bandwidth in kbit/s.
    #[arg(long, value_name = "KBPS")]
    pub(crate) down: Option<f64>,

    /// Upload bandwidth in kbit/s.
    #[arg(long, value_name = "KBPS")]
    pub(crate) up: Option<f64>,
}

#[derive(Debug, Args)]
pub(crate) struct CpuThrottleArgs {
    /// How many times slower (1 or `off`: full speed).
    #[arg(value_name = "RATE")]
    pub(crate) rate: String,
}

#[derive(Debug, Args)]
pub(crate) struct PermissionsArgs {
    #[command(subcommand)]
    pub(crate) command: Option<PermissionsCommand>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PermissionsCommand {
    /// Grant permissions, to every origin or to --origin.
    #[command(after_help = "\
Names: geolocation, notifications, clipboard-read, clipboard-write, camera,
microphone, midi, midi-sysex, background-sync, persistent-storage,
screen-wake-lock, local-fonts, storage-access, idle-detection,
window-management, accelerometer, gyroscope, magnetometer, payment-handler.

Examples:
  chrome-cdp permissions grant notifications
  chrome-cdp permissions grant camera microphone --origin http://localhost:3000")]
    Grant(PermissionNamesArgs),

    /// Deny permissions, to every origin or to --origin.
    #[command(after_help = "\
Examples:
  chrome-cdp permissions deny geolocation
  chrome-cdp permissions deny notifications --origin https://example.com")]
    Deny(PermissionNamesArgs),

    /// Put permissions back to asking (all of them without names).
    #[command(after_help = "\
Examples:
  chrome-cdp permissions reset
  chrome-cdp permissions reset notifications")]
    Reset(PermissionResetArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ExtensionsArgs {
    #[command(subcommand)]
    pub(crate) command: Option<ExtensionsCommand>,

    /// Cut the list at this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ExtensionsCommand {
    /// Load an unpacked extension into the running browser.
    #[command(after_help = "\
Its content scripts run in pages loaded after it, not in those already
open: reload them. Loading the directory of a loaded extension reloads it.

Examples:
  chrome-cdp extensions load ./my-extension
  chrome-cdp open --extension ./my-extension   # load it at launch instead")]
    Load(ExtensionLoadArgs),

    /// Load an extension again from its directory, after editing its files.
    #[command(after_help = "\
It keeps its id and its chrome.storage, as the Reload button of
chrome://extensions does. Pages its content scripts ran in keep the old
scripts until they load again; its popup and other pages close.

Examples:
  chrome-cdp extensions reload 'My Extension'
  chrome-cdp extensions reload dealjoeccjhpbcmihadkoaebbihfdicd")]
    Reload(ExtensionArgs),

    /// Uninstall an unpacked extension (its storage goes with it).
    #[command(after_help = "\
Examples:
  chrome-cdp extensions uninstall 'My Extension'")]
    Uninstall(ExtensionArgs),

    /// Run an extension's action on the tab, which opens its popup.
    #[command(after_help = "\
Clicks the extension's toolbar button for the active tab (or --tab). Its
popup opens as a new tab, which `tabs` lists and `tab` switches to. An
action without a popup runs its chrome.action.onClicked listener instead.

Examples:
  chrome-cdp extensions popup 'My Extension'
  chrome-cdp extensions popup 'My Extension' --tab t2")]
    Popup(ExtensionArgs),

    /// Read or write an extension's chrome.storage.
    #[command(
        args_conflicts_with_subcommands = false,
        subcommand_value_name = "OP",
        after_help = "\
Without a subcommand, prints what the area holds, as `get` does. --area
picks the area: local (the default), sync, session, or managed (set by
policy; read-only).

Examples:
  chrome-cdp extensions storage 'My Extension'
  chrome-cdp extensions storage 'My Extension' get token --area session
  chrome-cdp extensions storage 'My Extension' set '{\"onboarded\": true}'
  chrome-cdp extensions storage 'My Extension' clear"
    )]
    Storage(ExtensionStorageArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ExtensionLoadArgs {
    /// The unpacked extension's directory (the one holding its manifest.json).
    #[arg(value_name = "DIR")]
    pub(crate) dir: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct ExtensionArgs {
    /// The extension's id or name, as `extensions` lists them.
    #[arg(value_name = "ID|NAME")]
    pub(crate) extension: String,
}

#[derive(Debug, Args)]
pub(crate) struct ExtensionStorageArgs {
    /// The extension's id or name, as `extensions` lists them.
    #[arg(value_name = "ID|NAME")]
    pub(crate) extension: String,

    #[command(subcommand)]
    pub(crate) command: Option<ExtensionStorageCommand>,

    /// The storage area.
    #[arg(long, value_enum, default_value_t = ExtensionStorageAreaArg::Local, global = true)]
    pub(crate) area: ExtensionStorageAreaArg,

    /// Cut the output at this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N", global = true)]
    pub(crate) max_chars: usize,
}

/// A chrome.storage area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ExtensionStorageAreaArg {
    Local,
    Sync,
    Session,
    Managed,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ExtensionStorageCommand {
    /// Print the items of some keys, or all of them, as JSON.
    #[command(after_help = "\
Examples:
  chrome-cdp extensions storage 'My Extension' get
  chrome-cdp extensions storage 'My Extension' get token user --area sync")]
    Get(ExtensionStorageKeysArgs),

    /// Merge the items of a JSON object into the area.
    #[command(after_help = "\
Keys not in the object keep their values; `remove` takes keys out.

Examples:
  chrome-cdp extensions storage 'My Extension' set '{\"theme\": \"dark\", \"count\": 3}'
  chrome-cdp extensions storage 'My Extension' set - < seed.json")]
    Set(ExtensionStorageSetArgs),

    /// Remove some keys from the area.
    #[command(after_help = "\
Examples:
  chrome-cdp extensions storage 'My Extension' remove token")]
    Remove(ExtensionStorageRemoveArgs),

    /// Remove every item from the area.
    #[command(after_help = "\
Examples:
  chrome-cdp extensions storage 'My Extension' clear
  chrome-cdp extensions storage 'My Extension' clear --area session")]
    Clear(ExtensionStorageClearArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ExtensionStorageKeysArgs {
    /// The keys to print; every item without.
    #[arg(value_name = "KEY")]
    pub(crate) keys: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ExtensionStorageSetArgs {
    /// A JSON object of the items to set; `-` reads it from stdin.
    #[arg(value_name = "JSON")]
    pub(crate) items: String,
}

#[derive(Debug, Args)]
pub(crate) struct ExtensionStorageRemoveArgs {
    /// The keys to remove.
    #[arg(value_name = "KEY", required = true)]
    pub(crate) keys: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ExtensionStorageClearArgs {}

#[derive(Debug, Args)]
pub(crate) struct PermissionNamesArgs {
    /// Permission names, as the Permissions API has them.
    #[arg(value_name = "NAME", required = true)]
    pub(crate) names: Vec<String>,

    /// Only for this origin (e.g. https://example.com).
    #[arg(long, value_name = "ORIGIN")]
    pub(crate) origin: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct PermissionResetArgs {
    /// Permission names; every permission without.
    #[arg(value_name = "NAME")]
    pub(crate) names: Vec<String>,

    /// Only for this origin.
    #[arg(long, value_name = "ORIGIN")]
    pub(crate) origin: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct CookiesArgs {
    #[command(subcommand)]
    pub(crate) command: Option<CookiesCommand>,

    #[command(flatten)]
    pub(crate) filter: CookieFilterArgs,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CookiesCommand {
    /// Set a cookie, for the active tab's URL unless given --url or
    /// --domain.
    #[command(after_help = "\
Prints the cookie as Chrome stored it. --expires takes seconds since 1970
(as Playwright's expires), an ISO time (2026-12-31T23:59:00Z), or a time
from now (90s, 30m, 12h, 30d); without it the cookie lasts the browser
session. Chrome cuts expiry to at most 400 days. It refuses some cookies: SameSite=None needs --secure, and
__Host- cookies need --secure, --path / and a --url.

Examples:
  chrome-cdp cookies set theme=dark
  chrome-cdp cookies set session=abc123 --url https://app.example.com --httponly --secure
  chrome-cdp cookies set consent=yes --domain .example.com --expires 30d")]
    Set(CookieSetArgs),

    /// Remove cookies: all of them, or those the filters pick.
    #[command(after_help = "\
Examples:
  chrome-cdp cookies clear
  chrome-cdp cookies clear --domain example.com
  chrome-cdp cookies clear --name session")]
    Clear(CookieFilterArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CookieFilterArgs {
    /// Only the cookies a request to this URL carries.
    #[arg(long, value_name = "URL")]
    pub(crate) url: Option<String>,

    /// Only cookies of this domain and its subdomains.
    #[arg(long, value_name = "DOMAIN")]
    pub(crate) domain: Option<String>,

    /// Only cookies with this name.
    #[arg(long, value_name = "NAME")]
    pub(crate) name: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct CookieSetArgs {
    /// The cookie: NAME=VALUE.
    #[arg(value_name = "NAME=VALUE")]
    pub(crate) cookie: String,

    /// The URL it is for (host, path and scheme).
    #[arg(long, value_name = "URL", conflicts_with = "domain")]
    pub(crate) url: Option<String>,

    /// Its domain: example.com for that host only, .example.com for its
    /// subdomains too.
    #[arg(long, value_name = "DOMAIN")]
    pub(crate) domain: Option<String>,

    /// Its path (default: /, or the --url's directory).
    #[arg(long, value_name = "PATH")]
    pub(crate) path: Option<String>,

    /// Sent over HTTPS only.
    #[arg(long)]
    pub(crate) secure: bool,

    /// Hidden from page scripts (document.cookie).
    #[arg(long, alias = "http-only")]
    pub(crate) httponly: bool,

    /// When it goes with cross-site requests.
    #[arg(long, value_enum, value_name = "POLICY")]
    pub(crate) samesite: Option<SameSiteArg>,

    /// When it expires: seconds since 1970, an ISO time, or from now (30d).
    #[arg(long, value_name = "TIME", value_parser = parse_expires, allow_hyphen_values = true)]
    pub(crate) expires: Option<f64>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum SameSiteArg {
    Strict,
    Lax,
    None,
}

impl SameSiteArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

/// `cookies set --expires`: seconds since 1970, an ISO 8601 time, or a
/// time from now with a unit (s, m, h, d). Returns seconds since 1970.
pub(crate) fn parse_expires(text: &str) -> Result<f64, String> {
    let text = text.trim();
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse::<f64>().map_err(|e| format!("`{text}`: {e}"));
    }
    let unit = text.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let number = &text[..text.len() - unit.len()];
    let scale = match unit {
        "s" => Some(1.0),
        "m" => Some(60.0),
        "h" => Some(3600.0),
        "d" => Some(86_400.0),
        _ => None,
    };
    if let (Some(scale), Ok(number)) = (scale, number.parse::<f64>()) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        return Ok((now + number * scale).floor());
    }
    #[allow(clippy::cast_precision_loss)]
    protocol::format::parse_time(text)
        .map(|ms| (ms / 1000) as f64)
        .map_err(|_| {
            format!(
                "`{text}` is not a time; use seconds since 1970, an ISO time (2026-12-31T23:59:00Z) or a time from now (30d)"
            )
        })
}

#[derive(Debug, Args)]
pub(crate) struct StorageArgs {
    /// local (localStorage) or session (sessionStorage).
    #[arg(value_enum, value_name = "AREA")]
    pub(crate) area: StorageAreaArg,

    /// list, get, set, remove or clear.
    #[arg(value_enum, value_name = "ACTION", default_value_t = StorageActionArg::List)]
    pub(crate) action: StorageActionArg,

    /// The key (get, set, remove).
    #[arg(value_name = "KEY")]
    pub(crate) key: Option<String>,

    /// The value (set); `-` reads stdin.
    #[arg(value_name = "VALUE", allow_hyphen_values = true)]
    pub(crate) value: Option<String>,

    /// Use the tab's first frame of this origin (e.g. an iframe's).
    #[arg(long, value_name = "ORIGIN")]
    pub(crate) origin: Option<String>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum StorageAreaArg {
    Local,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum StorageActionArg {
    List,
    Get,
    Set,
    Remove,
    Clear,
}

#[derive(Debug, Args)]
pub(crate) struct StateArgs {
    #[command(subcommand)]
    pub(crate) command: StateCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum StateCommand {
    /// Write the cookies and storage to a file (readable by you only).
    #[command(after_help = "\
Saves the browser's cookies and the storage of every origin the session
visited (in a page or an iframe): localStorage, sessionStorage (a
chrome-cdp extension Playwright ignores) and, with --indexeddb, IndexedDB.
An origin no tab has open now (a login provider the app redirected
through) is read in a hidden page that gets an empty document of it, as
Playwright does; its sessionStorage went with its tab.

Examples:
  chrome-cdp state save auth.json
  chrome-cdp state save auth.json --indexeddb")]
    Save(StateSaveArgs),

    /// Load cookies and storage from a file (ours or Playwright's).
    #[command(after_help = "\
Adds the file's cookies, and gives each origin its storage: at once if a
tab has it open, else its next page load gets it before the page's
scripts run. The storage of those origins is replaced; other cookies and
origins stay. `open --state <file>` does the same before the first page
loads.

Examples:
  chrome-cdp state load auth.json
  chrome-cdp state load playwright/.auth/user.json")]
    Load(StateLoadArgs),
}

#[derive(Debug, Args)]
pub(crate) struct StateSaveArgs {
    /// The file to write.
    #[arg(value_name = "FILE")]
    pub(crate) file: PathBuf,

    /// Also save each origin's IndexedDB databases.
    #[arg(long)]
    pub(crate) indexeddb: bool,
}

#[derive(Debug, Args)]
pub(crate) struct StateLoadArgs {
    /// The file to read; `-` reads stdin.
    #[arg(value_name = "FILE")]
    pub(crate) file: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct TimeArgs {
    #[command(subcommand)]
    pub(crate) command: TimeCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TimeCommand {
    /// Freeze the time pages see (default: now).
    #[command(after_help = "\
TIME is ISO 8601 (UTC unless it has an offset) or ms since 1970.

Examples:
  chrome-cdp time freeze
  chrome-cdp time freeze 2026-01-31T09:30:00Z
  chrome-cdp time freeze 2026-12-24T18:00+01:00")]
    Freeze(TimeFreezeArgs),

    /// Move the frozen time on, running the timers due by then.
    #[command(after_help = "\
Timers set before the time was frozen in a page that was already open are
not run early. While paused (`time pause`), it fires each callback due by
then once, where `time run-for` fires every interval tick and frame.

Examples:
  chrome-cdp time advance 90s
  chrome-cdp time advance 2h
  chrome-cdp time advance 1500        # ms")]
    Advance(TimeAdvanceArgs),

    /// Freeze the time and stop timers and animation frames.
    #[command(after_help = "\
TIME is ISO 8601 (UTC unless it has an offset) or ms since 1970; without it,
the frozen time, or now. While paused, setTimeout, setInterval and
requestAnimationFrame callbacks are queued and performance.now() stands
still, until `time run-for`, `pause-at` or `resume`. Timers a page set
before the clock came to it keep running: reload for full control.

Examples:
  chrome-cdp time pause
  chrome-cdp time pause 2026-01-31T09:30:00Z
  chrome-cdp time run-for 5s")]
    Pause(TimePauseArgs),

    /// Move the paused time on, firing every callback due, in order.
    #[command(after_help = "\
Each interval tick fires (100ms over 1s: 10 times), animation frames run
every 16ms, and callbacks set on the way run when they are due. It stops
after 10,000 callbacks, leaving the rest queued. `time advance` while
paused fires each due callback only once.

Examples:
  chrome-cdp time run-for 1s
  chrome-cdp time run-for 15m
  chrome-cdp time run-for 500        # ms")]
    RunFor(TimeRunForArgs),

    /// Run the paused time on to TIME, as run-for, then stay paused.
    #[command(after_help = "\
TIME is ISO 8601 (UTC unless it has an offset) or ms since 1970, and not
before the paused time.

Examples:
  chrome-cdp time pause-at 2026-01-31T10:00:00Z")]
    PauseAt(TimePauseAtArgs),

    /// Let paused timers run in real time again; the time stays frozen.
    #[command(after_help = "\
Examples:
  chrome-cdp time resume
  chrome-cdp time reset              # the real time, too")]
    Resume,

    /// Give pages the real time back.
    #[command(after_help = "\
Examples:
  chrome-cdp time reset")]
    Reset,
}

#[derive(Debug, Args)]
pub(crate) struct TimeFreezeArgs {
    /// ISO 8601, e.g. 2026-01-31T09:30:00Z, or ms since 1970.
    #[arg(value_name = "TIME")]
    pub(crate) time: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct TimePauseArgs {
    /// ISO 8601, e.g. 2026-01-31T09:30:00Z, or ms since 1970.
    #[arg(value_name = "TIME")]
    pub(crate) time: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct TimeRunForArgs {
    /// How far: 1500 (ms), 90s, 15m, 2h, 7d.
    #[arg(value_name = "DURATION", value_parser = parse_advance)]
    pub(crate) by: u64,
}

#[derive(Debug, Args)]
pub(crate) struct TimePauseAtArgs {
    /// ISO 8601, e.g. 2026-01-31T10:00:00Z, or ms since 1970.
    #[arg(value_name = "TIME")]
    pub(crate) time: String,
}

#[derive(Debug, Args)]
pub(crate) struct TimeAdvanceArgs {
    /// How far: 1500 (ms), 90s, 15m, 2h, 7d.
    #[arg(value_name = "DURATION", value_parser = parse_advance)]
    pub(crate) by: u64,
}

/// `time advance`'s duration in ms: a number of ms, or with a unit (ms,
/// s, m, h, d).
pub(crate) fn parse_advance(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number
        .parse()
        .map_err(|_| format!("`{text}` is not a duration; use e.g. 1500, 90s, 15m or 2h"))?;
    let scale = match unit {
        "" | "ms" => 1.0,
        "s" => 1000.0,
        "m" => 60_000.0,
        "h" => 3_600_000.0,
        "d" => 86_400_000.0,
        _ => {
            return Err(format!(
                "`{text}` has an unknown unit; use ms, s, m, h or d"
            ));
        }
    };
    let ms = (number * scale).round();
    if ms < 1.0 || ms > 1e15 {
        return Err(format!("`{text}` is out of range"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(ms as u64)
}

#[derive(Debug, Args)]
pub(crate) struct HarArgs {
    #[command(subcommand)]
    pub(crate) command: HarCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HarCommand {
    /// Start recording requests (of every tab, or of --tab).
    #[command(after_help = "\
Starting again while recording keeps the recording and takes the new
--content.

Examples:
  chrome-cdp har start
  chrome-cdp har start --content omit      # sizes and types, no bodies
  chrome-cdp har start --tab t2")]
    Start(HarStartArgs),

    /// Save the requests recorded since `har start` to a HAR file.
    #[command(after_help = "\
Prints the file's path (-q: only the path); the file goes to the session's
hars/ directory (1.har, 2.har, …) unless -o names one, and its pages: a
line for each when a tab navigated (t1_2 http://…/b.html: 3 requests,
DOMContentLoaded 40ms, load 52ms). Bodies Chrome no longer has are left
out, with a warning.

Examples:
  chrome-cdp har stop -o checkout.har
  chrome-cdp har stop -q")]
    Stop(HarStopArgs),
}

#[derive(Debug, Args)]
pub(crate) struct HarStartArgs {
    /// Response bodies: in the file (`embed`) or left out (`omit`).
    #[arg(long, value_enum, default_value_t = HarContentArg::Embed, value_name = "HOW")]
    pub(crate) content: HarContentArg,
}

/// `har start --content`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum HarContentArg {
    /// Text as it is, binary bodies in base64.
    Embed,
    /// Only sizes and MIME types.
    Omit,
}

impl HarContentArg {
    pub(crate) fn to_content(self) -> engine::HarContent {
        match self {
            Self::Embed => engine::HarContent::Embed,
            Self::Omit => engine::HarContent::Omit,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct HarStopArgs {
    /// Where to save it; a numbered file in the session's hars/ directory
    /// if not given.
    #[arg(short, long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct TraceArgs {
    #[command(subcommand)]
    pub(crate) command: TraceCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TraceCommand {
    /// Start a performance trace of the tab.
    #[command(after_help = "\
Starting while a trace runs changes nothing and says so.

Examples:
  chrome-cdp trace start
  chrome-cdp trace start --screenshots                     # with a film strip
  chrome-cdp trace start --categories devtools.timeline,v8")]
    Start(TraceStartArgs),

    /// Stop the trace and save it.
    #[command(after_help = "\
Prints the file's path (-q: only the path); the trace goes to the
session's traces/ directory (1.json, 2.json, …) unless -o names a file. A
path ending in .gz is gzipped.

Examples:
  chrome-cdp trace stop -o trace.json
  chrome-cdp trace stop -o trace.json.gz
  chrome-cdp trace stop --timeout 2m          # a long trace takes a while")]
    Stop(TraceStopArgs),
}

#[derive(Debug, Args)]
pub(crate) struct TraceStartArgs {
    /// Also record screenshots of the page as it renders (the film strip).
    #[arg(long)]
    pub(crate) screenshots: bool,

    /// Trace categories instead of the Performance panel's,
    /// comma-separated; `-name` leaves one out (`-*` all others).
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "LIST",
        allow_hyphen_values = true
    )]
    pub(crate) categories: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct TraceStopArgs {
    /// Where to save it (`.gz`: gzipped); a numbered file in the session's
    /// traces/ directory if not given.
    #[arg(short, long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct PerfArgs {}

#[derive(Debug, Args)]
pub(crate) struct HighlightArgs {
    /// The element: a ref (e12) or a locator that matches one.
    #[arg(value_name = "LOCATOR")]
    pub(crate) locator: String,

    /// How long the highlight stays (0: until the next highlight).
    #[arg(long, default_value = "2s", value_parser = parse_duration_or_zero, value_name = "DURATION")]
    pub(crate) duration: Duration,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,
}

#[derive(Debug, Args)]
pub(crate) struct InspectArgs {
    /// The panel to open on.
    #[arg(long, value_enum, value_name = "PANEL")]
    pub(crate) panel: Option<PanelArg>,
}

/// `inspect --panel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PanelArg {
    Elements,
    Console,
    Network,
    Sources,
    /// Storage, cookies, service workers.
    Application,
    Performance,
    Memory,
    Lighthouse,
    Security,
    Recorder,
}

impl PanelArg {
    /// DevTools' id of the panel.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Elements => "elements",
            Self::Console => "console",
            Self::Network => "network",
            Self::Sources => "sources",
            Self::Application => "resources",
            Self::Performance => "timeline",
            Self::Memory => "heap-profiler",
            Self::Lighthouse => "lighthouse",
            Self::Security => "security",
            Self::Recorder => "chrome-recorder",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct CoverageArgs {
    #[command(subcommand)]
    pub(crate) command: CoverageCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CoverageCommand {
    /// Start measuring the tab's JavaScript and CSS coverage.
    #[command(after_help = "\
Starting while it measures changes nothing and says so. Load or reload the
page after starting to count its startup code.

Examples:
  chrome-cdp coverage start
  chrome-cdp coverage start --tab t2")]
    Start(CoverageStartArgs),

    /// Stop measuring and list the files by unused bytes.
    #[command(after_help = "\
-q prints one line per file: unused, total and URL, tab-separated.

Examples:
  chrome-cdp coverage stop
  chrome-cdp coverage stop --limit 0          # every file")]
    Stop(CoverageStopArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DebugArgs {
    #[command(subcommand)]
    pub(crate) command: DebugCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DebugCommand {
    /// Set a logpoint: log expressions' values each time a source line
    /// runs.
    #[command(after_help = "\
FILE is matched by path against the original sources of the tab's scripts
(`src/cart.ts` matches `http://localhost:5173/src/cart.ts` and
`webpack://app/./src/cart.ts`); when it names several, the error lists
them and more of the path picks one. A line with no code uses the next
line with code, and says so. A file no script has yet is pending: the
logpoint binds when a script with it loads (a lazy chunk, a reload).

Each EXPR is one JavaScript expression, run in the scope of the line
(before it runs) and shown with its text as the label; one that throws
shows its error in its place. Top-level commas split an EXPR: 'total,
items.length' is two (parentheses keep the comma operator: '(a, b)'). Values are previewed in the page: 2 levels
deep, strings cut at 80 characters, collections over 5 entries counted.
`-` reads one expression from stdin. A session has at most 20 logpoints.
-q prints the logpoint's id.

Examples:
  chrome-cdp debug log src/cart.ts:42 total 'items.length'
  chrome-cdp debug log src/cart.ts:42 'total, item.qty'
  chrome-cdp debug log src/cart.ts:42:7 item
  chrome-cdp debug log src/App.tsx:30             # was the line reached?
  chrome-cdp debug log src/cart.ts:42 total --limit 5")]
    Log(DebugLogArgs),

    /// List the logpoints, where they are and how often they were hit.
    #[command(after_help = "\
One line per logpoint: its id, place, expressions and hits, and whether it
is pending (no script has its file yet) or stopped (it reached --limit).
-q prints the ids.

Examples:
  chrome-cdp debug list
  chrome-cdp debug list --json")]
    List(DebugListArgs),

    /// Remove a logpoint, or all of them.
    #[command(after_help = "\
When no logpoint is left, the debugger is turned off in the tab, unless
locals capture keeps it on (see `debug locals`).

Examples:
  chrome-cdp debug clear 1
  chrome-cdp debug clear --all")]
    Clear(DebugClearArgs),

    /// Say where uncaught errors keep their local variables, or change it.
    #[command(after_help = "\
Where locals capture is on, the page pauses at each uncaught error (a
thrown error or a rejected promise nobody handles) for up to 200ms while
chrome-cdp reads the local, closure and block variables of its top 3
frames (10 each at most, previewed like logpoint values), then resumes.
`errors --locals` and `console --locals` show them. At most 10 errors a
second are read; the rest say so.

  on         every page, iframe and worker of the session
  off        none
  localhost  documents and workers on this machine (the default)

With no argument, it says where it is on. -q prints the setting.

Examples:
  chrome-cdp debug locals
  chrome-cdp debug locals on      # a staging site too
  chrome-cdp debug locals off")]
    Locals(DebugLocalsArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DebugLocalsArgs {
    /// on, off or localhost; none to show the setting.
    #[arg(value_enum, value_name = "on|off|localhost")]
    pub(crate) mode: Option<LocalsArg>,
}

/// `debug locals`' setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LocalsArg {
    On,
    Off,
    Localhost,
}

#[derive(Debug, Args)]
pub(crate) struct DebugLogArgs {
    /// The source line: FILE:LINE or FILE:LINE:COLUMN (1-based), e.g.
    /// src/cart.ts:42.
    #[arg(value_name = "FILE:LINE")]
    pub(crate) place: String,

    /// JavaScript expressions to log, each one argument; `-` reads one
    /// from stdin.
    #[arg(value_name = "EXPR")]
    pub(crate) exprs: Vec<String>,

    /// Stop logging after this many hits.
    #[arg(long, default_value_t = 1000, value_name = "N")]
    pub(crate) limit: u64,
}

#[derive(Debug, Args)]
pub(crate) struct DebugListArgs {}

#[derive(Debug, Args)]
pub(crate) struct DebugClearArgs {
    /// The logpoint's id (`debug list` shows them).
    #[arg(value_name = "ID", required_unless_present = "all")]
    pub(crate) id: Option<u32>,

    /// Remove every logpoint.
    #[arg(long, conflicts_with = "id")]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct CoverageStartArgs {}

#[derive(Debug, Args)]
pub(crate) struct CoverageStopArgs {
    /// List at most this many files, most unused bytes first (0 = all).
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub(crate) limit: usize,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct PdfArgs {
    /// Where to save it (`-` for stdout); a numbered file in the session's
    /// pdfs/ directory if not given.
    #[arg(short, long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,

    /// The paper size.
    #[arg(long, value_enum, ignore_case = true, default_value_t = PaperArg::Letter, value_name = "PAPER")]
    pub(crate) format: PaperArg,

    /// Turn the paper sideways.
    #[arg(long)]
    pub(crate) landscape: bool,

    /// Print background colours and images.
    #[arg(long)]
    pub(crate) background: bool,

    /// Page margins: one to four lengths in CSS order (top, right, bottom,
    /// left), each in in, cm, mm or px: 1cm, 10mm,15mm. Default 1cm.
    #[arg(long, value_parser = parse_margin, value_name = "LENGTHS")]
    pub(crate) margin: Option<engine::Margins>,

    /// Print only these pages: numbers and ranges, comma-separated (1-3,5;
    /// 3- runs to the end). Default all.
    #[arg(long, value_name = "RANGES")]
    pub(crate) pages: Option<String>,

    /// Use the page's own CSS `@page` size where it sets one, instead of
    /// --format.
    #[arg(long)]
    pub(crate) css_page_size: bool,

    /// Print this HTML at the top of each page, or @FILE's. Chrome fills
    /// elements of class pageNumber, totalPages, date, title and url. Give
    /// it a font size: Chrome's default is tiny.
    #[arg(long, value_name = "HTML|@FILE")]
    pub(crate) header: Option<String>,

    /// Print this HTML at the bottom of each page, or @FILE's (as
    /// --header).
    #[arg(long, value_name = "HTML|@FILE")]
    pub(crate) footer: Option<String>,

    /// Scale the page: 0.1 to 2. Default 1.
    #[arg(long, value_parser = parse_scale, value_name = "FACTOR")]
    pub(crate) scale: Option<f64>,
}

/// `pdf --format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PaperArg {
    #[value(name = "Letter")]
    Letter,
    #[value(name = "Legal")]
    Legal,
    #[value(name = "Tabloid")]
    Tabloid,
    #[value(name = "A3")]
    A3,
    #[value(name = "A4")]
    A4,
    #[value(name = "A5")]
    A5,
}

impl PaperArg {
    pub(crate) fn to_paper(self) -> engine::PaperFormat {
        match self {
            Self::Letter => engine::PaperFormat::Letter,
            Self::Legal => engine::PaperFormat::Legal,
            Self::Tabloid => engine::PaperFormat::Tabloid,
            Self::A3 => engine::PaperFormat::A3,
            Self::A4 => engine::PaperFormat::A4,
            Self::A5 => engine::PaperFormat::A5,
        }
    }
}

/// Parses `pdf --margin`.
fn parse_margin(s: &str) -> Result<engine::Margins, String> {
    engine::Margins::parse(s)
}

/// Parses `pdf --scale`: 0.1 to 2, as Chrome takes.
fn parse_scale(s: &str) -> Result<f64, String> {
    match s.trim().parse::<f64>() {
        Ok(scale) if (0.1..=2.0).contains(&scale) => Ok(scale),
        _ => Err(format!(
            "`{s}` is not a scale from 0.1 to 2 (1 is the page's size)"
        )),
    }
}

/// Parses one of `network --type`'s resource types.
fn parse_type(s: &str) -> Result<String, String> {
    let kind = s.trim().to_ascii_lowercase();
    if engine::RESOURCE_TYPES.contains(&kind.as_str()) {
        Ok(kind)
    } else {
        Err(format!(
            "`{s}` is not a resource type; they are {}",
            engine::RESOURCE_TYPES.join(", ")
        ))
    }
}

/// Parses `network --status`: `404`, `4xx` or `500-599`.
fn parse_status(s: &str) -> Result<engine::StatusRange, String> {
    engine::StatusRange::parse(s).ok_or_else(|| {
        format!(
            "`{s}` is not a status such as 404, a class such as 4xx, or a range such as 500-599"
        )
    })
}

/// Parses a request id: `42` or `#42`.
fn parse_request_id(s: &str) -> Result<u64, String> {
    let s = s.trim();
    s.strip_prefix('#')
        .unwrap_or(s)
        .parse()
        .map_err(|_| format!("`{s}` is not a request id such as 42 (see `chrome-cdp network`)"))
}

/// Parses `--since`: `last`, an entry number (`12` or `#12`), a
/// duration (`30s`), or the name of a `mark`.
pub(crate) fn parse_since(s: &str) -> Result<engine::Since, String> {
    let s = s.trim();
    if s == "last" {
        return Ok(engine::Since::Last);
    }
    let number = s.strip_prefix('#').unwrap_or(s);
    if let Ok(seq) = number.parse::<u64>() {
        return Ok(engine::Since::Seq { seq });
    }
    if let Ok(ago) = parse_duration(s) {
        return Ok(engine::Since::Ago {
            ms: u64::try_from(ago.as_millis()).unwrap_or(u64::MAX),
        });
    }
    if is_mark_name(s) {
        return Ok(engine::Since::Mark { name: s.to_owned() });
    }
    Err(format!(
        "`{s}` is not `last`, an entry number such as 12, a duration such as 30s, or a mark's name"
    ))
}

/// Whether `name` can name a `mark`: a letter, then letters, digits, `-`,
/// `_` or `.`, and not something `--since` reads otherwise (`last`, `5m`).
pub(crate) fn is_mark_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && name != "last"
        && parse_duration(name).is_err()
}

#[derive(Debug, Args)]
pub(crate) struct IdleArgs {
    /// Close the browser and stop the daemon after this long without
    /// commands, e.g. 10m or 2h; 0 means never. Default 30m.
    #[arg(
        long,
        env = "CHROME_CDP_IDLE_TIMEOUT",
        value_parser = parse_duration_or_zero,
        value_name = "DURATION"
    )]
    pub(crate) idle_timeout: Option<Duration>,
}

#[derive(Debug, Args)]
pub(crate) struct ConnectArgs {
    /// A port (9222), host:port, http://host:port, or a ws:// DevTools URL.
    #[arg(value_name = "ENDPOINT")]
    pub(crate) endpoint: String,

    #[command(flatten)]
    pub(crate) idle: IdleArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CloseArgs {
    /// Close every running session.
    #[arg(long)]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct StatusArgs {
    /// Also show the last lines of Chrome's log (its stderr).
    #[arg(long)]
    pub(crate) logs: bool,

    /// How many log lines --logs shows.
    #[arg(long, default_value_t = 50, value_name = "N", requires = "logs")]
    pub(crate) limit: usize,
}

#[derive(Debug, Args)]
pub(crate) struct SessionsArgs {}

#[derive(Debug, Args)]
pub(crate) struct CdpArgs {
    /// The CDP method, e.g. Browser.getVersion; optional with --listen.
    #[arg(value_name = "Domain.method", required_unless_present = "listen")]
    pub(crate) method: Option<String>,

    /// Params as a JSON object; `-` reads them from stdin.
    #[arg(value_name = "JSON")]
    pub(crate) params: Option<String>,

    /// Send the call to the browser, or to the active tab (or --tab).
    /// Default: page with --tab or --frame, else browser.
    #[arg(long, value_enum)]
    pub(crate) target: Option<Target>,

    /// Send the call to a frame of the tab: its name attribute, a part of
    /// its URL, or its id. Implies --target page. A cross-site frame has
    /// its own CDP session; Runtime.evaluate runs in the frame's page
    /// world unless the params name a context.
    #[arg(long, value_name = "FRAME")]
    pub(crate) frame: Option<String>,

    /// Also print these events of the call's session (Domain.event or
    /// Domain.*; repeat it or separate them with commas).
    #[arg(long, value_name = "EVENT", value_delimiter = ',')]
    pub(crate) listen: Vec<String>,

    /// How many events to wait for (0: all of them until --timeout).
    #[arg(long, value_name = "N", requires = "listen")]
    pub(crate) count: Option<usize>,

    /// Listen on every session of the tab, not only the call's: its
    /// page's, its cross-site iframes', its workers' (and those that start
    /// meanwhile). Each line starts with what sent it: [main],
    /// [iframe "pay"], [dedicated worker w.js]. Implies --target page.
    #[arg(long, requires = "listen")]
    pub(crate) all_sessions: bool,

    /// Bound the output to this many characters (0 = no limit). With --json,
    /// long strings and arrays in `data` are shortened instead.
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct EvalArgs {
    /// The script: an expression or statements; `-` reads it from stdin.
    #[arg(value_name = "JS")]
    pub(crate) script: String,

    /// Bind this element as `el` (a ref such as e12, or any locator); the
    /// script runs in its frame.
    #[arg(long = "in", value_name = "LOCATOR", conflicts_with = "frame")]
    pub(crate) locator: Option<String>,

    /// Run in a frame of the tab: its name attribute, a part of its URL,
    /// or its id.
    #[arg(long, value_name = "FRAME")]
    pub(crate) frame: Option<String>,

    /// The page's own world (its globals), or chrome-cdp's isolated one
    /// (the same DOM, where page scripts cannot see or change what the
    /// script does).
    #[arg(long, value_enum, default_value_t = WorldArg::Main)]
    pub(crate) world: WorldArg,

    /// Resolve an exception's stack frames with source maps (always on for
    /// localhost pages).
    #[arg(long)]
    pub(crate) sourcemaps: bool,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit). With --json,
    /// long strings and arrays in `data` are shortened instead.
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `eval --world`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum WorldArg {
    Main,
    Isolated,
}

impl WorldArg {
    pub(crate) fn to_engine(self) -> engine::EvalWorld {
        match self {
            Self::Main => engine::EvalWorld::Main,
            Self::Isolated => engine::EvalWorld::Isolated,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct GotoArgs {
    /// The URL; `-` reads it from stdin.
    #[arg(value_name = "URL")]
    pub(crate) url: String,

    #[command(flatten)]
    pub(crate) wait: WaitArgs,
}

#[derive(Debug, Args)]
pub(crate) struct HistoryArgs {
    #[command(flatten)]
    pub(crate) wait: WaitArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ReloadArgs {
    /// Bypass the cache.
    #[arg(long)]
    pub(crate) hard: bool,

    #[command(flatten)]
    pub(crate) wait: WaitArgs,
}

#[derive(Debug, Args)]
pub(crate) struct WaitArgs {
    /// What to wait for before returning; --timeout bounds it.
    #[arg(long, value_enum, default_value_t = Wait::Settle, value_name = "EVENT")]
    pub(crate) wait: Wait,
}

/// What a navigation waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Wait {
    /// The load event, then until the page settles (no requests for
    /// 500ms, no DOM changes for 300ms), 5s at most.
    Settle,
    /// The load event: the page with its images, scripts and styles.
    Load,
    /// DOMContentLoaded: the HTML is parsed.
    Domcontentloaded,
    /// DOMContentLoaded, then no requests for 500ms (not counting
    /// WebSockets, beacons and requests open over 10s).
    Networkidle,
    /// The server answered and the new page replaced the old one.
    Commit,
    /// Nothing: return once the navigation is sent.
    None,
}

impl Wait {
    pub(crate) fn to_engine(self) -> engine::WaitUntil {
        match self {
            Self::Settle => engine::WaitUntil::Settle,
            Self::Load => engine::WaitUntil::Load,
            Self::Domcontentloaded => engine::WaitUntil::DomContentLoaded,
            Self::Networkidle => engine::WaitUntil::NetworkIdle,
            Self::Commit => engine::WaitUntil::Commit,
            Self::None => engine::WaitUntil::None,
        }
    }
}

#[derive(Debug, Args)]
#[command(
    group(
        ArgGroup::new("condition")
            .required(true)
            .args([
                "target", "load", "domcontentloaded", "idle", "settle", "text", "text_gone",
                "url", "function", "request", "response", "download", "console",
                "binding", "ws", "hmr",
            ])
    )
)]
pub(crate) struct WaitCmdArgs {
    /// An element to wait for (any locator: e12, #id, text=…, role=…),
    /// or how long to sleep: 500 (milliseconds) or 2s.
    #[arg(value_name = "LOCATOR|MS")]
    pub(crate) target: Option<String>,

    /// The state the element must reach.
    #[arg(long, value_enum, value_name = "STATE")]
    pub(crate) state: Option<ElementStateArg>,

    /// A ref whose element is gone fails, instead of being re-resolved
    /// to the one element with its role and name.
    #[arg(
        long,
        env = "CHROME_CDP_STRICT_REFS",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) strict_refs: bool,

    /// The page's load event.
    #[arg(long)]
    pub(crate) load: bool,

    /// DOMContentLoaded: the HTML is parsed.
    #[arg(long)]
    pub(crate) domcontentloaded: bool,

    /// Network idle: no requests for 500ms (not counting WebSockets,
    /// beacons and requests open over 10s).
    #[arg(long)]
    pub(crate) idle: bool,

    /// Network idle and no DOM changes for 300ms (3s at most for the DOM).
    #[arg(long)]
    pub(crate) settle: bool,

    /// Visible text to wait for; `-` reads it from stdin.
    #[arg(long, value_name = "TEXT")]
    pub(crate) text: Option<String>,

    /// Visible text to wait to go away; `-` reads it from stdin.
    #[arg(long, value_name = "TEXT")]
    pub(crate) text_gone: Option<String>,

    /// Look for --text or --text-gone only in the first element this
    /// locator matches (or in the frame=… it names).
    #[arg(long = "in", value_name = "LOCATOR")]
    pub(crate) within: Option<String>,

    /// The page URL: a part of it, a glob (* and **) or /regex/.
    #[arg(long, value_name = "PATTERN")]
    pub(crate) url: Option<String>,

    /// A JavaScript expression, polled in the page until it is truthy
    /// (promises are awaited); `-` reads it from stdin.
    #[arg(long = "fn", value_name = "JS")]
    pub(crate) function: Option<String>,

    /// A request whose URL matches (a part, a glob or /regex/), sent since
    /// the previous command began.
    #[arg(long, value_name = "PATTERN")]
    pub(crate) request: Option<String>,

    /// A response whose URL matches, received since the previous command
    /// began.
    #[arg(long, value_name = "PATTERN")]
    pub(crate) response: Option<String>,

    /// The status the --response must have.
    #[arg(long, value_name = "CODE")]
    pub(crate) status: Option<u16>,

    /// A download, started since the previous command began, to finish.
    #[arg(long)]
    pub(crate) download: bool,

    /// A console entry whose text or URL matches this regular
    /// expression, logged since the previous command began.
    #[arg(long, value_name = "REGEX")]
    pub(crate) console: Option<String>,

    /// A call of the function `expose NAME` put in the page, since the
    /// previous command began.
    #[arg(long, value_name = "NAME")]
    pub(crate) binding: Option<String>,

    /// A frame, sent or received since the previous command began, on a
    /// WebSocket whose URL matches (a part, a glob or /regex/).
    #[arg(long, value_name = "PATTERN")]
    pub(crate) ws: Option<String>,

    /// The text the --ws frame must match (a binary frame's first bytes in
    /// hex): a regular expression.
    #[arg(long, value_name = "REGEX")]
    pub(crate) frame: Option<String>,

    /// The dev server's hot update since the previous command began (the
    /// latest), or the next one.
    #[arg(long)]
    pub(crate) hmr: bool,

    /// Bound the output (--fn's value) to this many characters (0 = no
    /// limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `expect`.
#[derive(Debug, Args)]
#[command(
    group(
        ArgGroup::new("assertion")
            .required(true)
            .args(["target", "url", "title", "no_errors", "no_issues", "no_failed_requests", "a11y", "perf", "layout", "audit"])
            .multiple(true)
    )
)]
pub(crate) struct ExpectArgs {
    /// The element(s) to check (any locator: e12, #id, text=…, role=…);
    /// with --a11y, where to check.
    #[arg(value_name = "LOCATOR")]
    pub(crate) target: Option<String>,

    /// What to check: visible, hidden, attached, detached, enabled,
    /// disabled, editable, checked, unchecked, focused, text, value,
    /// values, attr, class, css, count or in-viewport.
    #[arg(value_name = "MATCHER")]
    pub(crate) matcher: Option<String>,

    /// The matcher's values: text, value(s), attribute name and value,
    /// class, CSS property and value, or count; `-` reads one from stdin.
    #[arg(value_name = "VALUE")]
    pub(crate) values: Vec<String>,

    /// The assertion must not hold (element matchers, --url, --title).
    #[arg(long)]
    pub(crate) not: bool,

    /// `text` and --title: all of the text, not a part of it.
    #[arg(long)]
    pub(crate) exact: bool,

    /// `count`: at least this many.
    #[arg(long, value_name = "N")]
    pub(crate) min: Option<u64>,

    /// `count`: at most this many.
    #[arg(long, value_name = "N")]
    pub(crate) max: Option<u64>,

    /// The page URL matches: a part of it, a glob (* and **) or /regex/.
    #[arg(long, value_name = "PATTERN", conflicts_with_all = ["matcher", "title", "no_errors", "no_issues", "no_failed_requests", "a11y"])]
    pub(crate) url: Option<String>,

    /// The page title contains this (all of it with --exact), or matches
    /// /regex/.
    #[arg(long, value_name = "TEXT", conflicts_with_all = ["matcher", "no_errors", "no_issues", "no_failed_requests", "a11y"])]
    pub(crate) title: Option<String>,

    /// No uncaught exception or console error since the page loaded (or
    /// --since).
    #[arg(long, conflicts_with_all = ["matcher", "no_failed_requests", "a11y"])]
    pub(crate) no_errors: bool,

    /// No issue of Chrome's (see `issues`) of --min-severity or above
    /// since the page loaded (or --since).
    #[arg(long, conflicts_with_all = ["matcher", "no_errors", "no_failed_requests", "a11y"])]
    pub(crate) no_issues: bool,

    /// --no-issues and --audit: the least severe issues (findings) that
    /// fail it.
    #[arg(long, value_enum, value_name = "SEVERITY")]
    pub(crate) min_severity: Option<SeverityArg>,

    /// --no-errors, --no-issues and --no-failed-requests: in every tab of
    /// the session (or --context), each since it opened, whatever it
    /// loaded since.
    #[arg(long, conflicts_with = "since")]
    pub(crate) all_tabs: bool,

    /// No request failed or got a status of 400 or more since the page
    /// loaded (or --since).
    #[arg(long, conflicts_with_all = ["matcher", "a11y"])]
    pub(crate) no_failed_requests: bool,

    /// --no-errors, --no-issues, --no-failed-requests and `request`: only
    /// after this mark (see `mark`), not since the page loaded.
    #[arg(long, value_name = "MARK")]
    pub(crate) since: Option<String>,

    /// `request`: exactly this many such requests.
    #[arg(long, value_name = "N", conflicts_with_all = ["min", "max"])]
    pub(crate) count: Option<u64>,

    /// `request`: the query has this parameter, `name=value` (a glob;
    /// repeatable).
    #[arg(long = "query", value_name = "NAME=VALUE")]
    pub(crate) queries: Vec<String>,

    /// `request`: it has this header, 'Name: value' (a glob; repeatable).
    #[arg(long = "header", value_name = "HEADER")]
    pub(crate) headers: Vec<String>,

    /// `request`: its JSON body contains this, as a mock's match.json
    /// ("*" any value, strings with * are globs); `-` reads stdin.
    #[arg(long, value_name = "JSON")]
    pub(crate) json_body: Option<String>,

    /// --no-failed-requests: only requests to this origin; `self` is the
    /// page's.
    #[arg(long, value_name = "ORIGIN", requires = "no_failed_requests")]
    pub(crate) origin: Option<String>,

    /// Check the page and its iframes (or LOCATOR's element) against
    /// built-in accessibility rules.
    #[arg(long, conflicts_with = "matcher")]
    pub(crate) a11y: bool,

    /// --a11y: the WCAG level to check.
    #[arg(long, value_enum, value_name = "LEVEL", requires = "a11y")]
    pub(crate) level: Option<A11yLevelArg>,

    /// Check `perf`'s numbers against budgets, all of which must hold:
    /// 'lcp<2.5s cls<0.1 tbt<200', or good (the Web Vitals thresholds).
    #[arg(
        long,
        value_name = "BUDGETS",
        conflicts_with_all = ["matcher", "url", "title", "no_errors", "no_issues", "no_failed_requests", "a11y"]
    )]
    pub(crate) perf: Option<String>,

    /// --perf: what a metric not measured yet (INP before an
    /// interaction) does.
    #[arg(long, value_enum, value_name = "WHAT", requires = "perf")]
    pub(crate) perf_missing: Option<PerfMissingArg>,

    /// Check the page's layout (see `layout`) or LOCATOR's element: fail
    /// on error findings the --baseline does not list.
    #[arg(
        long,
        requires = "baseline",
        conflicts_with_all = ["matcher", "url", "title", "no_errors", "no_issues", "no_failed_requests", "a11y", "perf"]
    )]
    pub(crate) layout: bool,

    /// --layout: the baseline, <dir>/layout/<NAME>.txt (one finding key
    /// per line). --audit: the accepted findings, a .json file or NAME for
    /// <dir>/audit/NAME.json.
    #[arg(long, value_name = "NAME")]
    pub(crate) baseline: Option<String>,

    /// --layout and --audit: write the findings to the baseline instead
    /// of comparing.
    #[arg(long, requires = "baseline")]
    pub(crate) update: bool,

    /// --layout and --audit: the baselines' directory (default:
    /// .chrome-cdp in the project).
    #[arg(long, value_name = "DIR", requires = "baseline")]
    pub(crate) dir: Option<PathBuf>,

    /// Audit the page (see `audit`), or LOCATOR's element: fail on
    /// findings at --min-severity or above that the --baseline does not
    /// accept (any, without one).
    #[arg(
        long,
        conflicts_with_all = ["matcher", "url", "title", "no_errors", "no_issues", "no_failed_requests", "a11y", "perf", "layout"]
    )]
    pub(crate) audit: bool,

    /// --audit: the categories to check (comma-separated or repeated).
    #[arg(
        long = "category",
        value_enum,
        value_delimiter = ',',
        value_name = "CATEGORY"
    )]
    pub(crate) categories: Vec<AuditCategoryArg>,

    #[command(flatten)]
    pub(crate) layout_flags: LayoutFlags,

    /// A ref whose element is gone fails, instead of being re-resolved
    /// to the one element with its role and name.
    #[arg(
        long,
        env = "CHROME_CDP_STRICT_REFS",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) strict_refs: bool,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `expect --perf-missing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PerfMissingArg {
    /// It fails, with what to do (the default).
    Fail,
    /// It passes.
    Ok,
}

/// `expect --a11y --level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum A11yLevelArg {
    /// WCAG level A rules only.
    #[value(name = "A", alias = "a")]
    A,
    /// Level A and AA rules.
    #[value(name = "AA", alias = "aa")]
    Aa,
}

/// `mark`.
#[derive(Debug, Args)]
pub(crate) struct MarkArgs {
    /// The mark's name, e.g. before-save.
    #[arg(value_name = "NAME")]
    pub(crate) name: String,
}

/// `expect-snapshot`.
#[derive(Debug, Args)]
pub(crate) struct ExpectSnapshotArgs {
    /// The baseline's name: letters, digits, -, _ and ., with / for
    /// folders (login, checkout/cart).
    #[arg(value_name = "NAME")]
    pub(crate) name: String,

    /// Only this element's subtree (any locator).
    #[arg(long, value_name = "LOCATOR")]
    pub(crate) scope: Option<String>,

    /// Every node (as `snapshot --all` shows them), not only what
    /// `snapshot -i` shows.
    #[arg(long)]
    pub(crate) full: bool,

    /// Store the page as the baseline instead of comparing.
    #[arg(long)]
    pub(crate) update: bool,

    /// The directory of the baselines, instead of the project's
    /// .chrome-cdp/snapshots.
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,

    /// Bound the output to this many characters (0 = no limit); also
    /// bounds a failure's diff.
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `expect-screenshot`.
#[derive(Debug, Args)]
pub(crate) struct ExpectScreenshotArgs {
    /// The baseline's name: letters, digits, -, _ and ., with / for
    /// folders (home, checkout/cart).
    #[arg(value_name = "NAME")]
    pub(crate) name: String,

    /// Capture only this element (any locator).
    #[arg(value_name = "LOCATOR", conflicts_with = "full")]
    pub(crate) locator: Option<String>,

    /// The whole page, not only the viewport; with LOC, all of the
    /// container the page scrolls in (as screenshot --full does).
    #[arg(long, value_name = "LOC", num_args = 0..=1, default_missing_value = "")]
    pub(crate) full: Option<String>,

    /// Paint over the elements this locator matches (repeatable).
    #[arg(long, value_name = "LOCATOR", action = ArgAction::Append)]
    pub(crate) mask: Vec<String>,

    /// How far (0 to 1) a pixel's colour may move before it has changed.
    #[arg(long, default_value_t = 0.1, value_name = "T")]
    pub(crate) threshold: f64,

    /// How many pixels may change and still pass.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub(crate) max_diff_pixels: u64,

    /// Store the page as the baseline instead of comparing.
    #[arg(long)]
    pub(crate) update: bool,

    /// The directory of the baselines, instead of the project's
    /// .chrome-cdp/screenshots.
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: Option<PathBuf>,

    #[command(flatten)]
    pub(crate) refs: StrictRefsArgs,
}

/// `wait <locator> --state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ElementStateArg {
    /// A match has a box and is not visibility: hidden.
    Visible,
    /// No match is visible (or none exists).
    Hidden,
    /// A match is in the document.
    Attached,
    /// No match is in the document.
    Detached,
    /// A match is not disabled.
    Enabled,
}

impl ElementStateArg {
    pub(crate) fn to_engine(self) -> engine::ElementState {
        match self {
            Self::Visible => engine::ElementState::Visible,
            Self::Hidden => engine::ElementState::Hidden,
            Self::Attached => engine::ElementState::Attached,
            Self::Detached => engine::ElementState::Detached,
            Self::Enabled => engine::ElementState::Enabled,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct TabsArgs {
    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct FramesArgs {
    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct TestArgs {
    /// Scenario files or folders, or FILE:LINE for the test at that line.
    /// Default: the config's `tests` globs. One folder with its own
    /// chrome-cdp.toml runs as that project.
    #[arg(value_name = "PATH")]
    pub(crate) paths: Vec<String>,

    /// Only the tests whose `file > name` matches this regular expression.
    #[arg(long, value_name = "REGEX")]
    pub(crate) grep: Option<String>,

    /// Only tests with one of these tags; `!tag` leaves a tag out, e.g.
    /// smoke,!slow.
    #[arg(long, value_name = "TAGS", allow_hyphen_values = true)]
    pub(crate) tags: Option<String>,

    /// How many tests run at once. Default: `jobs` in the config, else 1.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) jobs: Option<u32>,

    /// Where the tests run at once: in browser contexts of one Chrome
    /// (the default), or in a Chrome each.
    #[arg(long, value_enum, value_name = "HOW", default_value_t = JobsIsolation::Context)]
    pub(crate) jobs_isolation: JobsIsolation,

    /// Run only part INDEX of TOTAL near-equal parts, e.g. 2/4 (for CI
    /// machines that split a suite).
    #[arg(long, value_name = "INDEX/TOTAL")]
    pub(crate) shard: Option<String>,

    /// Run a failed test again up to N times. Default: `retries` in the
    /// config, else 0; a test's @retries wins.
    #[arg(long, value_name = "N")]
    pub(crate) retries: Option<u32>,

    /// Run each test N times, without retries, to find flaky tests: a test
    /// fails if any of its runs does, and the likely cause of its failures
    /// is named.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..), conflicts_with = "retries")]
    pub(crate) repeat: Option<u32>,

    /// Stop after the first failed test.
    #[arg(long)]
    pub(crate) bail: bool,

    /// Show a browser window.
    #[arg(long)]
    pub(crate) headed: bool,

    /// Write the baselines of expect-snapshot and expect-screenshot steps
    /// (in e2e/snapshots) instead of comparing with them.
    #[arg(long)]
    pub(crate) update_snapshots: bool,

    /// List the tests the run would have, without running them.
    #[arg(long)]
    pub(crate) list: bool,

    /// Take a screenshot after every step, into the test's trace/ folder
    /// (and the html report).
    #[arg(long)]
    pub(crate) trace: bool,

    /// On the first failure, stop and leave that test's browser context
    /// open (session test-debug), with its mocks, to look at with the
    /// usual commands. Retries are off.
    #[arg(long)]
    pub(crate) debug: bool,

    /// After the run, run the affected tests again whenever a scenario,
    /// mock, data file or a file of [e2e] watch changes, until Ctrl-C.
    #[arg(long, conflicts_with_all = ["debug", "list"])]
    pub(crate) watch: bool,

    /// Also write reports to files, comma-separated: json, junit, html
    /// (in the artifacts directory, or NAME=FILE); text is stdout.
    #[arg(long, value_delimiter = ',', value_parser = crate::test::reporters::parse, value_name = "NAMES")]
    pub(crate) reporter: Vec<crate::test::reporters::Reporter>,

    /// The backend: mock (the mock sets), a real one of
    /// [e2e.backends.NAME], or all of them in turn. Default: the
    /// config's default_backend, else mock.
    #[arg(long, value_name = "NAME", env = "CHROME_CDP_BACKEND")]
    pub(crate) backend: Option<String>,

    /// With a real backend: compare each answer with the mock that would
    /// have given it, and report the mocks whose JSON shape drifted.
    #[arg(long)]
    pub(crate) verify_mocks: bool,

    /// With --verify-mocks: rewrite drifted JSON data files from the real
    /// answers, keeping the values whose shape still fits.
    #[arg(long, requires = "verify_mocks")]
    pub(crate) update: bool,

    /// Run the tests not marked @safe on a protected backend too; `http`
    /// writes are then allowed in their steps.
    #[arg(long)]
    pub(crate) allow_writes: bool,

    /// When a step's locator finds nothing, run it with the element its
    /// fingerprint matches (high confidence; =medium: medium too), and
    /// write the changes to heal.patch.
    #[arg(
        long,
        value_enum,
        value_name = "LEVEL",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "high"
    )]
    pub(crate) heal: Option<HealLevelArg>,

    /// With --heal: apply heal.patch to the scenario and step files after
    /// the run.
    #[arg(long, requires = "heal")]
    pub(crate) write: bool,

    /// With a real backend: write each passed test's fetch and XHR
    /// requests as a mock set, recorded/<file>/<test> in the mocks
    /// directory (replacing an older one).
    #[arg(long)]
    pub(crate) record_mocks: bool,

    /// Record what the tests reached: per page, the controls no step
    /// acted on or checked, and the mocks no request matched; writes
    /// ui-coverage.json in the artifacts directory and prints a summary.
    #[arg(long)]
    pub(crate) ui_coverage: bool,

    /// Bound the JSON output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 200_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// `test --heal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum HealLevelArg {
    /// Only high-confidence matches (0.8 and up).
    High,
    /// Medium ones (0.6 and up) too.
    Medium,
}

/// `test --jobs-isolation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum JobsIsolation {
    /// A browser context each, in one Chrome.
    Context,
    /// A Chrome (a session) each.
    Browser,
}

#[derive(Debug, Args)]
pub(crate) struct ContextArgs {
    #[command(subcommand)]
    pub(crate) command: ContextCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ContextCommand {
    /// Make a browser context with one tab, optionally loading a URL.
    #[command(after_help = "\
Starts the session's browser first if it has none. The new context starts
with nothing: no cookies, storage, routes, mock sets, init scripts or
emulation.

Examples:
  chrome-cdp context new
  chrome-cdp context new https://example.com
  chrome-cdp context new /login -q      # prints just the id (with base_url)")]
    New(ContextNewArgs),

    /// List the contexts, their tabs (`*` marks the active one) and
    /// settings.
    #[command(after_help = "\
Examples:
  chrome-cdp context list
  chrome-cdp context list -q     # one id per line")]
    List(ContextListArgs),

    /// Close contexts with their tabs, and drop their settings.
    #[command(after_help = "\
Without an id, closes --context's.

Examples:
  chrome-cdp context close c1
  chrome-cdp --context c2 context close
  chrome-cdp context close --all")]
    Close(ContextCloseArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ContextNewArgs {
    /// A URL to load; without a scheme, https:// (or http:// for
    /// localhost) is added.
    #[arg(value_name = "URL")]
    pub(crate) url: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ContextListArgs {
    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct ContextCloseArgs {
    /// The contexts to close, e.g. c1.
    #[arg(value_name = "ID", conflicts_with = "all")]
    pub(crate) ids: Vec<String>,

    /// Close every context of the session.
    #[arg(long)]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TabArgs {
    #[command(subcommand)]
    pub(crate) command: Option<TabCommand>,

    /// The tab to make active, e.g. t2.
    #[arg(value_name = "ID")]
    pub(crate) id: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TabCommand {
    /// Open a tab, make it active, and optionally load a URL in it.
    #[command(after_help = "\
Starts the session's browser first if it has none.

Examples:
  chrome-cdp tab new
  chrome-cdp tab new https://example.com
  chrome-cdp tab new localhost:3000/login -q     # prints just the new id")]
    New(TabNewArgs),

    /// Close a tab (default: the active one).
    #[command(after_help = "\
Closing the last tab leaves a blank one, so the session keeps its browser;
--close-browser closes the session instead.

Examples:
  chrome-cdp tab close
  chrome-cdp tab close t3
  chrome-cdp tab close --close-browser     # on the last tab: close the session")]
    Close(TabCloseArgs),
}

#[derive(Debug, Args)]
pub(crate) struct TabNewArgs {
    /// A URL to load; without a scheme, https:// (or http:// for
    /// localhost) is added.
    #[arg(value_name = "URL")]
    pub(crate) url: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct TabCloseArgs {
    /// The tab to close, e.g. t3. Default: --tab, else the active tab.
    #[arg(value_name = "ID")]
    pub(crate) id: Option<String>,

    /// If it is the last tab, close the session and its browser too.
    #[arg(long)]
    pub(crate) close_browser: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DaemonArgs {
    /// The build of the CLI that starts it.
    #[arg(long, value_name = "ID")]
    pub(crate) build: String,

    /// How long it waits for work while it has no browser (tests shorten
    /// it). Default: 10s.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub(crate) no_browser_grace: Option<Duration>,
}

/// The flags `open --fake-media` adds.
const FAKE_MEDIA: &[&str] = &[
    "--use-fake-device-for-media-stream",
    "--use-fake-ui-for-media-stream",
];

/// How to find and launch Chrome (`open`).
#[derive(Debug, Args)]
pub(crate) struct LaunchArgs {
    /// The Chrome or Chromium binary to launch. Default: $CHROME_CDP_CHROME,
    /// then browsers from `chrome-cdp install`, then a system Chrome.
    #[arg(long, value_name = "PATH")]
    pub(crate) chrome: Option<PathBuf>,

    /// If no Chrome is found, install Chrome for Testing first.
    #[arg(
        long,
        env = "CHROME_CDP_AUTO_INSTALL",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) auto_install: bool,

    /// Open a browser window instead of running headless.
    #[arg(long)]
    pub(crate) headed: bool,

    /// Keep the profile (logins, cookies, storage) between runs under this
    /// name. Default: a temporary profile, deleted on close.
    #[arg(long, value_name = "NAME", conflicts_with = "user_data_dir")]
    pub(crate) profile: Option<String>,

    /// Use this directory as Chrome's profile (never your everyday one).
    #[arg(long, value_name = "PATH")]
    pub(crate) user_data_dir: Option<PathBuf>,

    /// An extra flag for Chrome, e.g. --chrome-arg=--window-size=1280,800;
    /// repeat it for more. docs/chrome-flags.md lists chrome-cdp's own.
    #[arg(long = "chrome-arg", value_name = "FLAG", allow_hyphen_values = true)]
    pub(crate) chrome_args: Vec<String>,

    /// Give pages a fake camera and microphone, and allow them without a
    /// prompt (--use-fake-device-for-media-stream,
    /// --use-fake-ui-for-media-stream).
    #[arg(long)]
    pub(crate) fake_media: bool,

    /// Load the unpacked extension in this directory (the one holding its
    /// manifest.json); repeat it for more. Headless or headed, but not with
    /// chrome-headless-shell.
    #[arg(long = "extension", value_name = "DIR")]
    pub(crate) extensions: Vec<PathBuf>,
}

impl LaunchArgs {
    /// `--chrome-arg`s, with `--fake-media`'s flags.
    pub(crate) fn chrome_args(&self) -> Vec<String> {
        let mut args = self.chrome_args.clone();
        if self.fake_media {
            for flag in FAKE_MEDIA {
                if !args.iter().any(|a| a == flag) {
                    args.push((*flag).to_owned());
                }
            }
        }
        args
    }

    /// The profile asked for; `None` leaves it to the session (a temporary
    /// one for a new browser).
    pub(crate) fn profile(&self) -> Option<browser::ProfileChoice> {
        match (&self.profile, &self.user_data_dir) {
            (Some(name), _) => Some(browser::ProfileChoice::Named(name.clone())),
            (None, Some(dir)) => Some(browser::ProfileChoice::Dir(dir.clone())),
            (None, None) => None,
        }
    }
}

/// Where a raw CDP call goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Target {
    /// The browser session (Browser.*, Target.*, …).
    Browser,
    /// The active tab, or --tab (Page.*, Runtime.*, DOM.*, …).
    Page,
}

impl Target {
    pub(crate) fn to_engine(self) -> engine::Target {
        match self {
            Self::Browser => engine::Target::Browser,
            Self::Page => engine::Target::Page,
        }
    }
}

/// A duration, or `0` (for `--idle-timeout`, never; for `highlight
/// --duration`, until the next one).
pub(crate) fn parse_duration_or_zero(s: &str) -> Result<Duration, String> {
    if s.trim() == "0" {
        return Ok(Duration::ZERO);
    }
    parse_duration(s)
}

pub(crate) use protocol::format::parse_duration;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clap::{CommandFactory, Parser};

    use super::{Cli, parse_duration};

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn durations_need_a_unit() {
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(
            parse_duration("5000"),
            Err("`5000` needs a unit; use e.g. 5000s or 5000ms".into())
        );
        assert!(parse_duration("5h").is_err());
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("fast").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = Cli::try_parse_from([
            "chrome-cdp",
            "cdp",
            "Browser.getVersion",
            "--json",
            "--timeout",
            "2s",
            "--session",
            "s1",
            "-vv",
        ])
        .unwrap();
        assert!(cli.global.json);
        assert_eq!(cli.global.timeout, Some(Duration::from_secs(2)));
        assert_eq!(cli.global.session, "s1");
        assert_eq!(cli.global.verbose, 2);
    }

    /// Commands whose output cannot grow with the page, so they need no
    /// `--max-chars`/`--limit`. (`install --list` grows only with the
    /// browsers the user installed.)
    /// `open`, `connect` and `close` print a line or two; `sessions` grows
    /// only with the sessions the user started.
    /// `tab`, `tab new` and `tab close` describe one or two tabs.
    /// `goto`, `back`, `forward` and `reload` print one line.
    /// `dialog accept|dismiss` print one line (the dialog's text is cut).
    /// The input actions print a line naming their element(s) (names cut
    /// at 60 characters) or a point.
    /// `screenshot` and `pdf` print a path and a line or two (at most 30
    /// of `--annotate`'s refs are listed).
    /// The emulation commands print a line per setting (`emulate --list`
    /// the fixed device table); `permissions` a line per permission the
    /// user set. `init-script add` prints a line about the script (its
    /// first line cut at 60 characters) and `remove` its ids; `expose`
    /// and `unexpose` a line of names. `cookies set` and `cookies clear`
    /// print a line; `state save` and `state load` a line per origin of
    /// the file the user chose; `mark` a line with the name the user
    /// chose. `expect-screenshot` prints a line (a failure's notes are
    /// paths and counts).
    const FIXED_SIZE: &[&str] = &[
        "mark",
        "locator",
        "expect-screenshot",
        "emulate",
        "viewport",
        "geo",
        "tz",
        "locale",
        "ua",
        "media",
        "offline",
        "throttle",
        "cpu-throttle",
        "permissions grant",
        "permissions deny",
        "permissions reset",
        "time freeze",
        "time advance",
        "time pause",
        "time run-for",
        "time pause-at",
        "time resume",
        "time reset",
        "schema",
        "route",
        "block",
        "unroute",
        "mock use",
        "mock off",
        "mock reset",
        "mock seed",
        "init-script add",
        "init-script remove",
        "expose",
        "unexpose",
        "screenshot",
        "pdf",
        "har start",
        "repro start",
        "record start",
        "record status",
        "har stop",
        "trace start",
        "trace stop",
        "perf",
        "coverage start",
        // A line; `debug list` a line per logpoint, at most 20.
        "debug log",
        "debug list",
        "debug clear",
        "debug locals",
        "highlight",
        "inspect",
        "network clear",
        "count",
        "box",
        // The element and at most --depth components (default 8).
        "source",
        // A line per check, at most 7, and 5 candidates.
        "why",
        "install",
        "uninstall",
        "extensions load",
        "extensions reload",
        "extensions uninstall",
        "extensions popup",
        // A line per check; a few lines.
        "doctor",
        "upgrade",
        "open",
        "connect",
        "close",
        "sessions",
        "tab",
        "context new",
        "context close",
        "tab new",
        "tab close",
        "goto",
        "back",
        "forward",
        "reload",
        "dialog accept",
        "dialog dismiss",
        "clipboard write",
        "cookies set",
        "cookies clear",
        "state save",
        "state load",
        // A line per file of the layout.
        "init e2e",
        // Fixed texts: the help, the skill file, completion scripts.
        "help",
        "skill",
        "completions",
        // A line per request answered, each bounded, until Ctrl-C.
        "mock serve",
        // Each step is bounded by its own command's flags.
        "batch",
        "run",
    ];

    /// The plan 08 checklist: every command has an example in its help, a
    /// registered `--json` schema, the global flags, and a bounded output.
    #[test]
    fn every_command_follows_the_output_contract() {
        fn leaves(cmd: &clap::Command, path: &str, out: &mut Vec<(String, clap::Command)>) {
            // Skip clap's generated `help` subcommands of commands with
            // subcommands (the top-level `help` is ours and must pass).
            // Hidden `__` commands are internal (the daemon).
            for sub in cmd
                .get_subcommands()
                .filter(|s| s.get_name() != "help" || path.is_empty())
                .filter(|s| !s.get_name().starts_with("__"))
            {
                let name = format!("{path} {}", sub.get_name()).trim().to_owned();
                // A command with subcommands that also takes arguments
                // (`tab <id>`) is a command of its own too.
                if !sub.has_subcommands() || sub.get_positionals().next().is_some() {
                    out.push((name.clone(), sub.clone()));
                }
                if sub.has_subcommands() {
                    leaves(sub, &name, out);
                }
            }
        }
        let mut root = Cli::command();
        root.build();
        let mut commands = Vec::new();
        leaves(&root, "", &mut commands);
        assert!(!commands.is_empty());

        let mut problems = Vec::new();
        for (name, cmd) in &commands {
            let help = cmd
                .get_after_help()
                .or(cmd.get_after_long_help())
                .map(ToString::to_string)
                .unwrap_or_default();
            if !help.contains("Examples:\n  chrome-cdp ") {
                problems.push(format!("{name}: no `Examples:` in its help"));
            }
            if protocol::schema::command(name).is_none() {
                problems.push(format!("{name}: no entry in protocol::schema::COMMANDS"));
            }
            for flag in ["json", "quiet", "session", "timeout", "verbose"] {
                if !cmd.get_arguments().any(|a| a.get_long() == Some(flag)) {
                    problems.push(format!("{name}: no global --{flag}"));
                }
            }
            let bounded = cmd
                .get_arguments()
                .any(|a| matches!(a.get_long(), Some("max-chars" | "limit")));
            if !bounded && !FIXED_SIZE.contains(&name.as_str()) {
                problems.push(format!("{name}: no --max-chars or --limit"));
            }
        }
        // `--version` is a flag, not a command, but has a --json form too.
        if protocol::schema::command("version").is_none() {
            problems.push("--version: no schema".into());
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn idle_timeout_takes_zero_for_never() {
        let cli = Cli::try_parse_from(["chrome-cdp", "open", "--idle-timeout", "0"]).unwrap();
        let Some(super::Command::Open(args)) = cli.command else {
            panic!("not open")
        };
        assert_eq!(args.idle.idle_timeout, Some(Duration::ZERO));
        assert!(Cli::try_parse_from(["chrome-cdp", "open", "--idle-timeout", "5"]).is_err());
    }

    #[test]
    fn highlight_duration_takes_zero_for_until_the_next() {
        let cli = Cli::try_parse_from(["chrome-cdp", "highlight", "#cart", "--duration", "0"]);
        let Some(super::Command::Highlight(args)) = cli.unwrap().command else {
            panic!("not highlight")
        };
        assert_eq!(args.duration, Duration::ZERO);
    }

    #[test]
    fn since_takes_marks_by_name() {
        use super::{is_mark_name, parse_since};
        assert_eq!(parse_since("last"), Ok(engine::Since::Last));
        assert_eq!(parse_since("#12"), Ok(engine::Since::Seq { seq: 12 }));
        assert_eq!(parse_since("30s"), Ok(engine::Since::Ago { ms: 30_000 }));
        assert_eq!(
            parse_since("before-save"),
            Ok(engine::Since::Mark {
                name: "before-save".into()
            })
        );
        assert!(parse_since("x y").is_err());
        for name in ["before-save", "step_2", "v1.2", "lastly"] {
            assert!(is_mark_name(name), "{name}");
        }
        for name in ["last", "5m", "2", "-x", "a b", ""] {
            assert!(!is_mark_name(name), "{name}");
        }
    }

    #[test]
    fn wait_takes_exactly_one_condition() {
        let parse = |args: &[&str]| {
            Cli::try_parse_from(["chrome-cdp", "wait"].iter().chain(args)).map(|_| ())
        };
        assert!(parse(&["#go"]).is_ok());
        assert!(parse(&["#go", "--state", "hidden"]).is_ok());
        assert!(parse(&["--text", "Hi", "--in", "#main"]).is_ok());
        assert!(parse(&["--response", "/api", "--status", "201"]).is_ok());
        assert!(parse(&[]).is_err());
        assert!(parse(&["#go", "--text", "Hi"]).is_err());
        assert!(parse(&["--idle", "--load"]).is_err());
    }

    #[test]
    fn json_and_quiet_conflict() {
        assert!(Cli::try_parse_from(["chrome-cdp", "--json", "-q"]).is_err());
    }
}
