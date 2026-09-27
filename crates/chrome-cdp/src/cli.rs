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
    after_help = "Exit codes: 0 ok, 1 action failed, 2 usage, 3 timeout, 4 browser/daemon, 5 dialog."
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
#[derive(Debug, Args)]
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

    /// Give up after this long, e.g. 500ms, 30s, 2m. Default: 10s for
    /// `wait`, 30s for navigation and everything else.
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

    /// Log to stderr: -v info, -vv debug, -vvv trace. CHROME_CDP_LOG
    /// (e.g. cdp=trace) overrides it.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub(crate) verbose: u8,

    /// `base_url` from the config files, for relative URLs (`goto /login`).
    #[arg(skip)]
    pub(crate) base_url: Option<String>,
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
  chrome-cdp schema version         # the schema of chrome-cdp --version --json")]
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

    /// Start the session's browser (or check the running one) and
    /// optionally open a URL.
    #[command(after_help = "\
Each session (--session, default `default`) has its own background daemon
and browser; later commands reuse them. Running open again is a no-op,
unless it asks for other launch options: add --restart for those.
The daemon closes the browser after --idle-timeout without commands.

Examples:
  chrome-cdp open https://example.com
  chrome-cdp open --headed                          # with a window
  chrome-cdp open --profile work https://app.test   # keeps logins
  chrome-cdp open --session shop http://localhost:3000
  chrome-cdp open --restart --chrome-arg=--window-size=1280,800
  chrome-cdp open --follow-popups                   # popups become the active tab
  chrome-cdp open --dialogs dismiss                 # answer every dialog with Cancel
  chrome-cdp open --http-credentials ada:s3cret http://localhost:8080/admin")]
    Open(OpenArgs),

    /// Attach the session to a browser that is already running.
    #[command(after_help = "\
The browser must run with --remote-debugging-port. `close` then only
detaches; the browser keeps running.

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
new tab but does not become active (unless `open --follow-popups`).

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

  <LOCATOR>           an element: #id, .class, [attr], css=…, xpath=…;
                      --state visible (default), hidden, attached,
                      detached or enabled
  --text, --text-gone visible text anywhere (or --in <LOCATOR>); spaces
                      are collapsed, case counts
  --url               the page URL: a part of it, a glob (* and **) or
                      /regex/
  --load, --domcontentloaded, --idle (no requests for 500ms, not counting
                      WebSockets, beacons and requests open over 10s),
  --settle            (network idle and no DOM changes for 300ms)
  --fn                a JavaScript expression that becomes truthy
  --request, --response [--status]  since the previous command began
  --download          a download started since the previous command began
                      has finished (prints its path with -q)
  <MS>                a fixed sleep, e.g. 500 or 2s (prefer a condition)

Examples:
  chrome-cdp wait '#results'
  chrome-cdp wait .spinner --state hidden
  chrome-cdp wait --text 'Welcome back'
  chrome-cdp wait --text-gone Loading --in '#main'
  chrome-cdp wait --url /dashboard
  chrome-cdp wait --idle
  chrome-cdp wait --fn 'window.app && window.app.ready'
  chrome-cdp wait --response '**/api/save' --status 200
  chrome-cdp wait --download")]
    Wait(WaitCmdArgs),

    /// Show the page as an accessibility tree, with refs (e12) to act on.
    #[command(after_help = "\
One node per line, two spaces per level:
  - role \"name\" [ref] (hN) value=\"…\" states… → href
Refs go on what can be acted on (controls, links, clickable elements,
scrollable boxes, iframes); a node keeps its ref while it stays in the
page. Hidden content is left out, lists longer than 30 collapse, and text
is cut at --text-limit. Needs a running session.

Examples:
  chrome-cdp snapshot
  chrome-cdp snapshot -i                  # only what can be acted on
  chrome-cdp snapshot --scope e12         # one part of the page
  chrome-cdp snapshot --scope '#results' --depth 3
  chrome-cdp snapshot --diff              # what changed since the last one")]
    Snapshot(SnapshotArgs),

    /// Answer the JavaScript dialog (confirm, prompt, …) the page waits on.
    #[command(
        subcommand_required = true,
        arg_required_else_help = true,
        after_help = "\
By default an alert, and a beforeunload (\"leave the page?\"), are accepted
at once; a confirm or prompt waits for this command, and the command that
ran into it fails with exit code 5 (see `open --dialogs`).

Examples:
  chrome-cdp dialog accept
  chrome-cdp dialog dismiss
  chrome-cdp dialog accept 'Ada Lovelace'     # a prompt's answer"
    )]
    Dialog(DialogArgs),

    /// List the files the session's pages downloaded.
    #[command(after_help = "\
Downloads go to the session's downloads directory, named as the server
suggests (made unique). A command that starts one says so; `wait
--download` waits for it to finish.

Examples:
  chrome-cdp downloads
  chrome-cdp downloads -q         # one path per line")]
    Downloads(DownloadsArgs),

    /// Make one raw CDP call on the session's browser or current page.
    #[command(after_help = "\
Starts the session's browser first if it has none.
Methods: https://chromedevtools.github.io/devtools-protocol/

Examples:
  chrome-cdp cdp Browser.getVersion
  chrome-cdp cdp --target page Runtime.evaluate '{\"expression\":\"1+1\"}'
  echo '{\"expression\":\"1+1\"}' | chrome-cdp cdp --target page Runtime.evaluate -
  chrome-cdp cdp Target.getTargets --session shop
  chrome-cdp cdp --target page --tab t2 Page.reload
  chrome-cdp cdp --frame checkout.example Runtime.evaluate '{\"expression\":\"location.href\"}'")]
    Cdp(CdpArgs),

    /// The session daemon; other commands start it.
    #[command(name = "__daemon", hide = true)]
    Daemon(DaemonArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SchemaArgs {
    /// The command, e.g. `version` or `tab close`; omit it to list them.
    #[arg(value_name = "COMMAND")]
    pub(crate) command: Vec<String>,
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

    /// Make popups (window.open, target=_blank) the active tab as they
    /// open. The session keeps this setting.
    #[arg(long)]
    pub(crate) follow_popups: bool,

    /// Load pages despite certificate errors (self-signed test servers).
    /// The session keeps this setting; it applies to a running browser
    /// too.
    #[arg(long)]
    pub(crate) ignore_https_errors: bool,

    /// How to answer JavaScript dialogs. The session keeps this setting.
    #[arg(long, value_enum, value_name = "POLICY")]
    pub(crate) dialogs: Option<Dialogs>,

    /// Answer HTTP authentication (basic, digest) with USER:PASSWORD; `-`
    /// reads it from stdin. The session keeps it; it applies to a running
    /// browser too.
    #[arg(
        long,
        env = "CHROME_CDP_HTTP_CREDENTIALS",
        hide_env_values = true,
        value_name = "USER:PASSWORD"
    )]
    pub(crate) http_credentials: Option<String>,

    #[command(flatten)]
    pub(crate) idle: IdleArgs,
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

    /// Only this part of the page: a ref (e12) or a CSS/XPath locator.
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

    /// How to read the page: ax (Chrome's accessibility tree, the
    /// default) or dom (chrome-cdp's DOM walker).
    #[arg(long, value_enum, value_name = "ENGINE")]
    pub(crate) engine: Option<SnapshotEngineArg>,

    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

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

#[derive(Debug, Args)]
pub(crate) struct DownloadsArgs {
    /// Bound the output to this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

#[derive(Debug, Args)]
pub(crate) struct IdleArgs {
    /// Close the browser and stop the daemon after this long without
    /// commands, e.g. 10m or 2h; 0 means never. Default 30m.
    #[arg(
        long,
        env = "CHROME_CDP_IDLE_TIMEOUT",
        value_parser = parse_idle_timeout,
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
    /// The CDP method, e.g. Browser.getVersion.
    #[arg(value_name = "Domain.method")]
    pub(crate) method: String,

    /// Params as a JSON object; `-` reads them from stdin.
    #[arg(value_name = "JSON")]
    pub(crate) params: Option<String>,

    /// Send the call to the browser, or to the active tab (or --tab).
    #[arg(long, value_enum, default_value_t = Target::Browser)]
    pub(crate) target: Target,

    /// Send the call to a frame of the tab: its name attribute, a part of
    /// its URL, or its id. Implies --target page. A cross-site frame has
    /// its own CDP session; Runtime.evaluate runs in the frame's page
    /// world unless the params name a context.
    #[arg(long, value_name = "FRAME")]
    pub(crate) frame: Option<String>,

    /// Bound the output to this many characters (0 = no limit). With --json,
    /// long strings and arrays in `data` are shortened instead.
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
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
                "url", "function", "request", "response", "download",
            ])
    )
)]
pub(crate) struct WaitCmdArgs {
    /// An element to wait for (#id, .class, [attr], css=…, xpath=…), or
    /// how long to sleep: 500 (milliseconds) or 2s.
    #[arg(value_name = "LOCATOR|MS")]
    pub(crate) target: Option<String>,

    /// The state the element must reach.
    #[arg(long, value_enum, value_name = "STATE")]
    pub(crate) state: Option<ElementStateArg>,

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
    /// locator matches.
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

    /// Bound the output (--fn's value) to this many characters (0 = no
    /// limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
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
}

impl LaunchArgs {
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

/// Parses `--idle-timeout`: a duration, or `0` for never.
pub(crate) fn parse_idle_timeout(s: &str) -> Result<Duration, String> {
    if s.trim() == "0" {
        return Ok(Duration::ZERO);
    }
    parse_duration(s)
}

/// Parses `500ms`, `30s`, `1.5s`, `2m`. A unit is required so `5000` is
/// never misread as seconds or milliseconds.
pub(crate) fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = num
        .parse()
        .map_err(|_| format!("`{s}` is not a duration; use e.g. 500ms, 30s or 2m"))?;
    let secs = match unit.trim() {
        "ms" => value / 1000.0,
        "s" => value,
        "m" => value * 60.0,
        "" => return Err(format!("`{s}` needs a unit; use e.g. {s}s or {s}ms")),
        other => return Err(format!("unknown unit `{other}`; use ms, s or m")),
    };
    let duration = Duration::try_from_secs_f64(secs).map_err(|e| format!("`{s}`: {e}"))?;
    if duration.is_zero() {
        return Err("the duration must be greater than zero".into());
    }
    Ok(duration)
}

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
    const FIXED_SIZE: &[&str] = &[
        "schema",
        "install",
        "uninstall",
        "open",
        "connect",
        "close",
        "sessions",
        "tab",
        "tab new",
        "tab close",
        "goto",
        "back",
        "forward",
        "reload",
        "dialog accept",
        "dialog dismiss",
    ];

    /// The plan 08 checklist: every command has an example in its help, a
    /// registered `--json` schema, the global flags, and a bounded output.
    #[test]
    fn every_command_follows_the_output_contract() {
        fn leaves(cmd: &clap::Command, path: &str, out: &mut Vec<(String, clap::Command)>) {
            // Skip clap's generated `help` subcommands (plan 08.3 replaces
            // the top-level one with its own, which must then pass too).
            // Hidden `__` commands are internal (the daemon).
            for sub in cmd
                .get_subcommands()
                .filter(|s| s.get_name() != "help" && !s.get_name().starts_with("__"))
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
