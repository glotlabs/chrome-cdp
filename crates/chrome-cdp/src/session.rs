//! The session commands (plan 02, milestone 2.4): `open`, `connect`,
//! `close`, `status`, `sessions` and `cdp`; the tab commands (plan 03,
//! milestone 3.1): `tabs`, `tab <id>`, `tab new` and `tab close`; and
//! navigation (milestone 3.3): `goto`, `back`, `forward` and `reload`;
//! `wait` (milestone 3.4); `dialog` and `downloads` (milestone 3.5); and
//! `snapshot` (plan 04).
//!
//! Each one finds the session's daemon (starting it when the command needs
//! a browser), sends one call, and returns the daemon's `Output` or
//! `ErrorInfo` as is, so rendering stays here, where the terminal is.
//! Anything that needs no daemon (argument checks, finding and installing
//! Chrome, with its progress bar) happens on this side first.

use std::time::Duration;

use daemon::client::{self, Session};
use daemon::meta::Meta;
use daemon::paths::{Env, SessionPaths, check_name};
use daemon::rpc::{self, ConnectParams, OpenParams, StatusParams};
use engine::{
    CdpRequest, ConnectRequest, DialogRequest, DownloadsRequest, ElementState, HttpCredentials,
    NavigateRequest, Navigation, OpenRequest, SnapshotRequest, TabCloseRequest, TabNewRequest,
    TabSelectRequest, TabsRequest, WaitFor, WaitRequest,
};
use protocol::schema::{ClosedSession, OpenData, SessionEntry, SessionState, StatusData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde_json::{Map, Value, json};
use tokio::runtime::Runtime;

use crate::cli::{
    CdpArgs, CloseArgs, ConnectArgs, DEFAULT_TIMEOUT, DialogArgs, DialogCommand, DownloadsArgs,
    ElementStateArg, GlobalArgs, GotoArgs, HistoryArgs, IdleArgs, OpenArgs, ReloadArgs,
    SessionsArgs, SnapshotArgs, StatusArgs, TabArgs, TabCommand, TabsArgs, WAIT_TIMEOUT,
    WaitCmdArgs, parse_duration,
};
use crate::{input, version};

/// How long `status` and `close` may take.
const QUICK: Duration = Duration::from_secs(30);

/// A command's session: its name, paths and runtime.
struct Ctx {
    env: Env,
    paths: SessionPaths,
    build: String,
    runtime: Runtime,
    timeout: Duration,
    /// `--tab`.
    tab: Option<String>,
    /// `base_url` from the config.
    base_url: Option<String>,
}

impl Ctx {
    /// A command whose timeout defaults to [`DEFAULT_TIMEOUT`].
    fn new(global: &GlobalArgs) -> Result<Self, ErrorInfo> {
        Self::with_default(global, DEFAULT_TIMEOUT)
    }

    /// A command whose timeout defaults to `default`.
    fn with_default(global: &GlobalArgs, default: Duration) -> Result<Self, ErrorInfo> {
        check_name(&global.session)?;
        let env = Env::current();
        let paths = SessionPaths::new(&env, &global.session);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::DaemonUnavailable,
                    format!("cannot start the runtime: {e}"),
                )
            })?;
        Ok(Self {
            env,
            paths,
            build: version::build_id(),
            runtime,
            timeout: global.timeout_or(default),
            tab: global.tab.clone(),
            base_url: global.base_url.clone(),
        })
    }

    fn session(&self) -> &str {
        &self.paths.name
    }

    fn start(&self) -> Result<Session, ErrorInfo> {
        self.runtime.block_on(client::connect_or_start(
            &self.env,
            &self.paths,
            &self.build,
        ))
    }

    /// How long a call that launches Chrome or loads a page may take: the
    /// daemon bounds each step by `--timeout`.
    fn long_wait(&self) -> Duration {
        self.timeout * 2 + Duration::from_secs(15)
    }

    /// Stops a daemon this command started when the command failed, so
    /// no idle daemon is left behind.
    fn abandon(&self, session: &mut Session) {
        if session.spawned {
            let _ = self.runtime.block_on(async {
                let _ = session.client.call(rpc::CLOSE, &json!({}), QUICK).await;
                client::wait_for_exit(&self.paths).await
            });
        }
    }
}

fn idle_ms(idle: &IdleArgs) -> Option<u64> {
    idle.idle_timeout
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// An error has no warnings, so a discovery warning (Chrome too old)
/// becomes a note: it may be the reason.
fn with_warnings_as_notes(info: ErrorInfo, warnings: &[String]) -> ErrorInfo {
    warnings
        .iter()
        .fold(info, |info, w| info.note("warning", w))
}

/// `open [url]`.
pub(crate) fn open(args: &OpenArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let url = args.url.as_deref().map(engine::normalize_url).transpose()?;
    check_tab(global.tab.as_deref())?;
    let credentials = args
        .http_credentials
        .as_deref()
        .map(http_credentials)
        .transpose()?;
    let launch = &args.launch;
    browser::launch::flags::check_extra(&launch.chrome_args)?;
    if let Some(name) = &launch.profile {
        browser::launch::check_profile_name(name)?;
    }
    let ctx = Ctx::new(global)?;
    let mut session = ctx.start()?;
    let result = open_in(&ctx, &mut session, args, url, credentials);
    if result.is_err() {
        ctx.abandon(&mut session);
    }
    result
}

fn open_in(
    ctx: &Ctx,
    session: &mut Session,
    args: &OpenArgs,
    url: Option<String>,
    http_credentials: Option<HttpCredentials>,
) -> Result<Output, ErrorInfo> {
    let tab = ctx.tab.clone();
    let launch = &args.launch;
    // Find (and maybe install) Chrome here, where the progress bar can
    // show, unless the session already runs one that `open` will reuse.
    let launches = args.restart || session.client.hello.state != SessionState::Running;
    let chrome = if launches {
        Some(crate::install::find_chrome(
            launch.chrome.as_deref(),
            launch.auto_install,
            ctx.timeout,
        )?)
    } else {
        None
    };
    let warnings = chrome
        .as_ref()
        .map(|c| c.warnings.clone())
        .unwrap_or_default();
    let mut request = OpenRequest {
        chrome,
        chrome_flag: launch.chrome.clone(),
        headed: launch.headed.then_some(true),
        profile: launch.profile(),
        chrome_args: launch.chrome_args.clone(),
        restart: args.restart,
        url,
        base_url: ctx.base_url.clone(),
        tab,
        follow_popups: args.follow_popups.then_some(true),
        ignore_https_errors: args.ignore_https_errors.then_some(true),
        dialogs: args.dialogs.map(crate::cli::Dialogs::to_engine),
        http_credentials,
        timeout_ms: millis(ctx.timeout),
    };
    if let Some(previous) = session.replaced.as_ref().and_then(|m| m.launch.as_ref()) {
        keep_previous_options(&mut request, previous);
    }
    let params = OpenParams {
        request,
        idle_timeout_ms: idle_ms(&args.idle),
    };
    let output = ctx
        .runtime
        .block_on(
            session
                .client
                .call_output(rpc::OPEN, &params, ctx.long_wait()),
        )
        .map_err(|info| with_warnings_as_notes(info, &warnings))?;
    Ok(warnings.into_iter().fold(output, Output::warning))
}

/// After an upgrade replaced the daemon, options not given now come from
/// how the old browser was opened.
fn keep_previous_options(request: &mut OpenRequest, previous: &OpenRequest) {
    if request.chrome_flag.is_none() {
        request.chrome_flag.clone_from(&previous.chrome_flag);
    }
    if request.headed.is_none() {
        request.headed = previous.headed;
    }
    if request.profile.is_none() {
        request.profile.clone_from(&previous.profile);
    }
    if request.chrome_args.is_empty() {
        request.chrome_args.clone_from(&previous.chrome_args);
    }
    if request.follow_popups.is_none() {
        request.follow_popups = previous.follow_popups;
    }
    if request.ignore_https_errors.is_none() {
        request.ignore_https_errors = previous.ignore_https_errors;
    }
    if request.dialogs.is_none() {
        request.dialogs = previous.dialogs;
    }
    if request.http_credentials.is_none() {
        request
            .http_credentials
            .clone_from(&previous.http_credentials);
    }
}

/// Parses `--http-credentials USER:PASSWORD` (`-` reads it from stdin).
fn http_credentials(arg: &str) -> Result<HttpCredentials, ErrorInfo> {
    let text = input::text_arg(arg, "USER:PASSWORD")?;
    let text = text.trim_end_matches(['\r', '\n']);
    match text.split_once(':') {
        Some((username, password)) if !username.is_empty() => Ok(HttpCredentials {
            username: username.to_owned(),
            password: password.to_owned(),
        }),
        _ => Err(
            bad_args("--http-credentials needs USER:PASSWORD").hint(Hint::run(
                Cmd::new("open").flag("http-credentials", "ada:s3cret"),
            )),
        ),
    }
}

/// `connect <endpoint>`.
pub(crate) fn connect(args: &ConnectArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    engine::parse_endpoint(&args.endpoint)?;
    let ctx = Ctx::new(global)?;
    let mut session = ctx.start()?;
    let params = ConnectParams {
        request: ConnectRequest {
            endpoint: args.endpoint.clone(),
            timeout_ms: millis(ctx.timeout),
        },
        idle_timeout_ms: idle_ms(&args.idle),
    };
    let result = ctx.runtime.block_on(session.client.call_output(
        rpc::CONNECT,
        &params,
        ctx.long_wait(),
    ));
    if result.is_err() {
        ctx.abandon(&mut session);
    }
    result
}

/// `cdp <Domain.method> [json]`.
pub(crate) fn cdp(args: &CdpArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    // Check everything that needs no browser before starting one.
    check_method(&args.method)?;
    let params = parse_params(args.params.as_deref())?;
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let (mut session, warnings) = with_browser(&ctx)?;
    let request = CdpRequest {
        method: args.method.clone(),
        params,
        target: if args.frame.is_some() {
            engine::Target::Page
        } else {
            args.target.to_engine()
        },
        tab: ctx.tab.clone(),
        frame: args.frame.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    let wait = ctx.timeout * 2 + Duration::from_secs(10);
    let output = ctx
        .runtime
        .block_on(session.client.call_output(rpc::CDP, &request, wait))?;
    Ok(warnings.into_iter().fold(output, Output::warning))
}

/// The session's daemon, started with a browser (as `open` would) if it
/// has none, and the warnings that start gave.
fn with_browser(ctx: &Ctx) -> Result<(Session, Vec<String>), ErrorInfo> {
    let mut session = ctx.start()?;
    let mut warnings = Vec::new();
    if session.client.hello.state == SessionState::Idle {
        match auto_open(ctx, &mut session) {
            Ok(opened) => warnings = opened,
            Err(info) => {
                ctx.abandon(&mut session);
                return Err(info);
            }
        }
    }
    Ok((session, warnings))
}

/// The running session's daemon; tab commands other than `tab new` need
/// one and do not start it.
fn running_session(ctx: &Ctx) -> Result<daemon::client::Client, ErrorInfo> {
    ctx.runtime
        .block_on(client::connect(&ctx.paths, &ctx.build))?
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::DaemonUnavailable,
                format!("session {} is not running", ctx.session()),
            )
            .hint(Hint::run(Cmd::new("open").session(ctx.session())))
        })
}

/// Checks a `--tab` before any daemon starts.
fn check_tab(tab: Option<&str>) -> Result<(), ErrorInfo> {
    tab.map(engine::parse_tab_id).transpose().map(|_| ())
}

/// `tabs`.
pub(crate) fn tabs(args: &TabsArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = TabsRequest {
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    ctx.runtime
        .block_on(client.call_output(rpc::TABS, &request, ctx.timeout + QUICK))
}

/// `tab <id>`, `tab new [url]` and `tab close [id]`.
pub(crate) fn tab(args: &TabArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    match &args.command {
        Some(TabCommand::New(new)) => {
            let url = new.url.as_deref().map(engine::normalize_url).transpose()?;
            let ctx = Ctx::new(global)?;
            let (mut session, warnings) = with_browser(&ctx)?;
            let request = TabNewRequest {
                url,
                base_url: ctx.base_url.clone(),
                timeout_ms: millis(ctx.timeout),
            };
            let output = ctx.runtime.block_on(session.client.call_output(
                rpc::TAB_NEW,
                &request,
                ctx.long_wait(),
            ))?;
            Ok(warnings.into_iter().fold(output, Output::warning))
        }
        Some(TabCommand::Close(close)) => {
            let tab = close.id.clone().or_else(|| global.tab.clone());
            check_tab(tab.as_deref())?;
            let ctx = Ctx::new(global)?;
            let mut client = running_session(&ctx)?;
            let request = TabCloseRequest {
                tab,
                close_browser: close.close_browser,
                timeout_ms: millis(ctx.timeout),
            };
            let output = ctx.runtime.block_on(client.call_output(
                rpc::TAB_CLOSE,
                &request,
                ctx.long_wait(),
            ))?;
            if output.data["session_closed"] == json!(true) {
                // Like `close`: nothing of the session runs on return.
                drop(client);
                ctx.runtime.block_on(client::wait_for_exit(&ctx.paths))?;
            }
            Ok(output)
        }
        None => {
            let Some(id) = &args.id else {
                return Err(bad_args("tab needs a tab id or a subcommand").hint(
                    Hint::run(Cmd::new("tabs").session(&global.session)).why("lists the ids"),
                ));
            };
            check_tab(Some(id))?;
            let ctx = Ctx::new(global)?;
            let mut client = running_session(&ctx)?;
            let request = TabSelectRequest {
                tab: id.clone(),
                timeout_ms: millis(ctx.timeout),
            };
            ctx.runtime
                .block_on(client.call_output(rpc::TAB_SELECT, &request, ctx.timeout + QUICK))
        }
    }
}

/// `goto <url>`: starts the session's browser if it has none, like `cdp`.
pub(crate) fn goto(args: &GotoArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let url = engine::normalize_url(&input::text_arg(&args.url, "URL")?)?;
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let (mut session, warnings) = with_browser(&ctx)?;
    let navigation = Navigation::Goto {
        url,
        base_url: ctx.base_url.clone(),
    };
    let output = navigate(&ctx, &mut session.client, navigation, args.wait.wait)?;
    Ok(warnings.into_iter().fold(output, Output::warning))
}

/// `back` and `forward`: need a running session.
pub(crate) fn history(
    navigation: Navigation,
    args: &HistoryArgs,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    navigate(&ctx, &mut client, navigation, args.wait.wait)
}

/// `reload [--hard]`: needs a running session.
pub(crate) fn reload(args: &ReloadArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let navigation = Navigation::Reload { hard: args.hard };
    navigate(&ctx, &mut client, navigation, args.wait.wait)
}

fn navigate(
    ctx: &Ctx,
    client: &mut daemon::client::Client,
    navigation: Navigation,
    wait: crate::cli::Wait,
) -> Result<Output, ErrorInfo> {
    let request = NavigateRequest {
        navigation,
        wait: wait.to_engine(),
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    ctx.runtime
        .block_on(client.call_output(rpc::NAVIGATE, &request, ctx.long_wait()))
}

/// `wait`: needs a running session.
pub(crate) fn wait(args: &WaitCmdArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let condition = wait_condition(args)?;
    engine::check_condition(&condition)?;
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::with_default(global, WAIT_TIMEOUT)?;
    let mut client = running_session(&ctx)?;
    let delay = match condition {
        WaitFor::Delay { ms } => Duration::from_millis(ms),
        _ => Duration::ZERO,
    };
    let request = WaitRequest {
        condition,
        tab: ctx.tab.clone(),
        strict_refs: args.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    let wait = ctx.timeout + delay + QUICK;
    ctx.runtime
        .block_on(client.call_output(rpc::WAIT, &request, wait))
}

/// `snapshot`: needs a running session.
pub(crate) fn snapshot(args: &SnapshotArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = SnapshotRequest {
        tab: ctx.tab.clone(),
        interactive: args.interactive,
        scope: args.scope.clone(),
        depth: args.depth,
        diff: args.diff,
        all_refs: args.all_refs,
        all: args.all,
        text_limit: args.text_limit,
        no_text: args.no_text,
        engine: args.engine.map(crate::cli::SnapshotEngineArg::to_engine),
        strict_refs: args.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    engine::check_snapshot(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::SNAPSHOT, &request, ctx.timeout + QUICK))
}

/// `dialog accept [text]` and `dialog dismiss`: need a running session.
pub(crate) fn dialog(args: &DialogArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let (accept, text) = match &args.command {
        DialogCommand::Accept(accept) => (
            true,
            accept
                .text
                .as_deref()
                .map(|t| input::text_arg(t, "TEXT"))
                .transpose()?,
        ),
        DialogCommand::Dismiss(_) => (false, None),
    };
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = DialogRequest {
        accept,
        text,
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    ctx.runtime
        .block_on(client.call_output(rpc::DIALOG, &request, ctx.timeout + QUICK))
}

/// `downloads`: needs a running session.
pub(crate) fn downloads(args: &DownloadsArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = DownloadsRequest {
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    ctx.runtime
        .block_on(client.call_output(rpc::DOWNLOADS, &request, ctx.timeout + QUICK))
}

/// The one condition `wait`'s arguments name (clap makes sure there is
/// exactly one).
fn wait_condition(args: &WaitCmdArgs) -> Result<WaitFor, ErrorInfo> {
    // From stdin, without the line's end.
    let text = |value: &str, what: &str| {
        input::text_arg(value, what).map(|t| t.trim_end_matches(['\r', '\n']).to_owned())
    };
    let needs = |flag: &str, given: bool, what: &str, example: Cmd| {
        if given {
            Err(bad_args(format!("{flag} needs {what}")).hint(Hint::run(example)))
        } else {
            Ok(())
        }
    };
    needs(
        "--state",
        args.state.is_some() && args.target.is_none(),
        "a locator",
        Cmd::new("wait").arg(".spinner").flag("state", "hidden"),
    )?;
    needs(
        "--in",
        args.within.is_some() && args.text.is_none() && args.text_gone.is_none(),
        "--text or --text-gone",
        Cmd::new("wait").flag("text", "Saved").flag("in", "#main"),
    )?;
    needs(
        "--status",
        args.status.is_some() && args.response.is_none(),
        "--response",
        Cmd::new("wait")
            .flag("response", "/api/save")
            .flag("status", "200"),
    )?;
    if let Some(target) = &args.target {
        if let Some(ms) = delay_ms(target) {
            needs(
                "--state",
                args.state.is_some(),
                "a locator, not a delay",
                Cmd::new("wait").arg(".spinner").flag("state", "hidden"),
            )?;
            return Ok(WaitFor::Delay { ms });
        }
        return Ok(WaitFor::Element {
            locator: target.clone(),
            state: args
                .state
                .map_or(ElementState::Visible, ElementStateArg::to_engine),
        });
    }
    Ok(if args.load {
        WaitFor::Load
    } else if args.domcontentloaded {
        WaitFor::DomContentLoaded
    } else if args.idle {
        WaitFor::Idle
    } else if args.settle {
        WaitFor::Settle
    } else if let Some(value) = &args.text {
        WaitFor::Text {
            text: text(value, "TEXT")?,
            within: args.within.clone(),
            gone: false,
        }
    } else if let Some(value) = &args.text_gone {
        WaitFor::Text {
            text: text(value, "TEXT")?,
            within: args.within.clone(),
            gone: true,
        }
    } else if let Some(pattern) = &args.url {
        WaitFor::Url {
            pattern: pattern.clone(),
        }
    } else if let Some(js) = &args.function {
        WaitFor::Function {
            expression: text(js, "JS")?,
        }
    } else if let Some(pattern) = &args.request {
        WaitFor::Request {
            pattern: pattern.clone(),
        }
    } else if let Some(pattern) = &args.response {
        WaitFor::Response {
            pattern: pattern.clone(),
            status: args.status,
        }
    } else if args.download {
        WaitFor::Download
    } else {
        return Err(
            bad_args("wait needs a condition").hint(Cmd::new("wait").switch("help").to_string())
        );
    })
}

/// `wait 500` or `wait 2s`: a delay, not a locator.
fn delay_ms(target: &str) -> Option<u64> {
    let target = target.trim();
    if !target.is_empty() && target.bytes().all(|b| b.is_ascii_digit()) {
        return target.parse().ok();
    }
    if !target.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    parse_duration(target)
        .ok()
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Gives a session without a browser one, as `open` would (or as the
/// daemon an upgrade replaced had), and returns the warnings to show.
fn auto_open(ctx: &Ctx, session: &mut Session) -> Result<Vec<String>, ErrorInfo> {
    let idle_timeout_ms = std::env::var("CHROME_CDP_IDLE_TIMEOUT")
        .ok()
        .and_then(|v| crate::cli::parse_idle_timeout(&v).ok())
        .map(millis);
    let replaced = session.replaced.clone().unwrap_or_else(empty_meta);
    let value = if let Some(endpoint) = &replaced.endpoint {
        let params = ConnectParams {
            request: ConnectRequest {
                endpoint: endpoint.clone(),
                timeout_ms: millis(ctx.timeout),
            },
            idle_timeout_ms,
        };
        let wait = ctx.long_wait();
        ctx.runtime
            .block_on(session.client.call_output(rpc::CONNECT, &params, wait))?
    } else {
        let previous = replaced.launch.unwrap_or_default();
        let auto_install = std::env::var("CHROME_CDP_AUTO_INSTALL").is_ok_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "y"
            )
        });
        let chrome = crate::install::find_chrome(
            previous.chrome_flag.as_deref(),
            auto_install,
            ctx.timeout,
        )?;
        let mut warnings = chrome.warnings.clone();
        let request = OpenRequest {
            chrome: Some(chrome),
            timeout_ms: millis(ctx.timeout),
            ..previous
        };
        let params = OpenParams {
            request,
            idle_timeout_ms,
        };
        let wait = ctx.long_wait();
        let mut output = ctx
            .runtime
            .block_on(session.client.call_output(rpc::OPEN, &params, wait))
            .map_err(|info| with_warnings_as_notes(info, &warnings))?;
        warnings.append(&mut output.warnings);
        output.warnings = warnings;
        output
    };
    let mut warnings = value.warnings;
    if let Ok(data) = serde_json::from_value::<OpenData>(value.data) {
        warnings.push(format!(
            "session {} had no browser; started {}",
            ctx.session(),
            engine::describe(&data.browser)
        ));
    }
    Ok(warnings)
}

fn empty_meta() -> Meta {
    Meta {
        session: String::new(),
        home: std::path::PathBuf::new(),
        build: String::new(),
        daemon_pid: 0,
        started_at: 0,
        idle_timeout_ms: 0,
        browser: None,
        launch: None,
        endpoint: None,
    }
}

/// `status`.
pub(crate) fn status(args: &StatusArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let ctx = Ctx::new(global)?;
    let logs = args.logs.then_some(args.limit);
    let client = ctx
        .runtime
        .block_on(client::connect(&ctx.paths, &ctx.build))?;
    if let Some(mut client) = client {
        return ctx.runtime.block_on(client.call_output(
            rpc::STATUS,
            &StatusParams { logs },
            QUICK,
        ));
    }
    ctx.runtime.block_on(client::clean_up_dead(&ctx.paths))?;
    let logs = logs.map(|n| daemon::output::log_tail(&ctx.paths.chrome_log, n));
    Ok(daemon::output::status(&daemon::output::stopped(
        ctx.session(),
        &ctx.paths.dir,
        logs,
    )))
}

/// `close [--all]`.
pub(crate) fn close(args: &CloseArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let ctx = Ctx::new(global)?;
    let targets = if args.all {
        client::live_sessions(&ctx.env)
    } else {
        vec![ctx.paths.clone()]
    };
    let mut closed = Vec::new();
    for paths in &targets {
        closed.push(ctx.runtime.block_on(close_one(paths, &ctx.build))?);
    }
    Ok(daemon::output::closed(closed, args.all))
}

async fn close_one(paths: &SessionPaths, build: &str) -> Result<ClosedSession, ErrorInfo> {
    let Some(mut client) = client::connect(paths, build).await? else {
        client::clean_up_dead(paths).await?;
        return Ok(ClosedSession {
            session: paths.name.clone(),
            was_running: false,
            browser: "none".into(),
        });
    };
    let value = client.call(rpc::CLOSE, &json!({}), QUICK).await?;
    drop(client);
    // Chrome has exited when `close` answers; wait for the daemon too, so
    // nothing of the session runs when this returns.
    client::wait_for_exit(paths).await?;
    serde_json::from_value(value).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::DaemonUnavailable,
            format!(
                "the daemon of session {} sent a bad result: {e}",
                paths.name
            ),
        )
    })
}

/// `sessions`.
pub(crate) fn sessions(_args: &SessionsArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let ctx = Ctx::new(global)?;
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    for paths in client::live_sessions(&ctx.env) {
        match ctx.runtime.block_on(session_entry(&paths, &ctx.build)) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(info) => warnings.push(format!("session {}: {}", paths.name, info.message)),
        }
    }
    Ok(warnings
        .into_iter()
        .fold(daemon::output::sessions(entries), Output::warning))
}

async fn session_entry(
    paths: &SessionPaths,
    build: &str,
) -> Result<Option<SessionEntry>, ErrorInfo> {
    let Some(mut client) = client::connect(paths, build).await? else {
        return Ok(None);
    };
    let pid = client.hello.pid;
    let output = client
        .call_output(rpc::STATUS, &StatusParams::default(), QUICK)
        .await?;
    let status: StatusData = serde_json::from_value(output.data)
        .map_err(|e| ErrorInfo::new(ErrorKind::DaemonUnavailable, format!("bad status: {e}")))?;
    Ok(Some(SessionEntry {
        session: status.session,
        state: status.state,
        product: status.browser.as_ref().map(|b| b.product.clone()),
        kind: status.browser.as_ref().map(|b| b.kind),
        tabs: status.tabs,
        uptime_ms: status.uptime_ms,
        daemon_pid: status.daemon_pid.unwrap_or(pid),
    }))
}

/// Accepts `Domain.method`, e.g. `Browser.getVersion`.
fn check_method(method: &str) -> Result<(), ErrorInfo> {
    let ok = method.split_once('.').is_some_and(|(domain, name)| {
        domain.starts_with(|c: char| c.is_ascii_uppercase())
            && !name.is_empty()
            && [domain, name]
                .iter()
                .all(|part| part.chars().all(|c| c.is_ascii_alphanumeric()))
    });
    if ok {
        Ok(())
    } else {
        Err(
            bad_args(format!("`{method}` is not a CDP method name")).hint(
                Hint::run(Cmd::new("cdp").arg("Browser.getVersion"))
                    .why("methods are Domain.method"),
            ),
        )
    }
}

fn bad_args(message: impl Into<String>) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadArgs, message)
}

/// Parses the params argument: absent means `{}`, `-` reads stdin.
fn parse_params(arg: Option<&str>) -> Result<Map<String, Value>, ErrorInfo> {
    let Some(arg) = arg else {
        return Ok(Map::new());
    };
    let text = input::text_arg(arg, "params")?;
    let example = r#"e.g. '{"expression":"1+1"}'"#;
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(bad_args(format!(
            "params must be a JSON object, not {}",
            json_type(&other)
        ))
        .hint(format!("wrap them in an object, {example}"))),
        Err(e) => Err(bad_args(format!("params are not valid JSON: {e}"))
            .hint(format!("quote the JSON for the shell, {example}"))),
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use engine::OpenRequest;
    use protocol::ErrorKind;
    use serde_json::json;

    use super::{check_method, delay_ms, keep_previous_options, parse_params};

    #[test]
    fn flags_that_need_another() {
        use clap::Parser as _;
        let condition = |args: &[&str]| {
            let cli =
                crate::cli::Cli::try_parse_from(["chrome-cdp", "wait"].iter().chain(args)).unwrap();
            let Some(crate::cli::Command::Wait(args)) = cli.command else {
                panic!("not wait")
            };
            super::wait_condition(&args)
        };
        for (args, message) in [
            (
                &["--idle", "--state", "hidden"][..],
                "--state needs a locator",
            ),
            (
                &["500", "--state", "hidden"],
                "--state needs a locator, not a delay",
            ),
            (
                &["--url", "/x", "--in", "#main"],
                "--in needs --text or --text-gone",
            ),
            (
                &["--request", "/x", "--status", "200"],
                "--status needs --response",
            ),
        ] {
            let err = condition(args).unwrap_err();
            assert_eq!(err.message, message);
            assert_eq!(err.exit_code(), 2);
        }
        assert!(matches!(
            condition(&["--text", "Hi", "--in", "#main"]).unwrap(),
            engine::WaitFor::Text {
                within: Some(_),
                gone: false,
                ..
            }
        ));
        assert!(matches!(
            condition(&["2s"]).unwrap(),
            engine::WaitFor::Delay { ms: 2000 }
        ));
    }

    #[test]
    fn delays_and_locators() {
        assert_eq!(delay_ms("500"), Some(500));
        assert_eq!(delay_ms("2s"), Some(2_000));
        assert_eq!(delay_ms("1.5s"), Some(1_500));
        assert_eq!(delay_ms("#results"), None);
        assert_eq!(delay_ms("css=div"), None);
        assert_eq!(delay_ms("2x"), None);
    }

    #[test]
    fn method_names() {
        assert!(check_method("Browser.getVersion").is_ok());
        assert!(check_method("DOMSnapshot.captureSnapshot").is_ok());
        for bad in [
            "",
            "Browser",
            "Browser.",
            ".getVersion",
            "browser.getVersion",
            "A.b.c",
        ] {
            let err = check_method(bad).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadArgs, "{bad}");
        }
    }

    #[test]
    fn params() {
        assert_eq!(
            parse_params(None).unwrap(),
            json!({}).as_object().unwrap().clone()
        );
        assert_eq!(
            parse_params(Some(r#"{"expression":"1+1"}"#)).unwrap()["expression"],
            json!("1+1")
        );
        let err = parse_params(Some("[1]")).unwrap_err();
        assert_eq!(err.message, "params must be a JSON object, not an array");
        let err = parse_params(Some("{expression:1}")).unwrap_err();
        assert!(
            err.message.starts_with("params are not valid JSON: "),
            "{err}"
        );
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn an_upgrade_keeps_the_options_not_given_again() {
        let previous = OpenRequest {
            headed: Some(true),
            follow_popups: Some(true),
            profile: Some(browser::ProfileChoice::Named("work".into())),
            chrome_args: vec!["--lang=de".into()],
            ..OpenRequest::default()
        };
        let mut request = OpenRequest {
            chrome_args: vec!["--lang=fr".into()],
            ..OpenRequest::default()
        };
        keep_previous_options(&mut request, &previous);
        assert_eq!(request.headed, Some(true));
        assert_eq!(request.profile, previous.profile);
        assert_eq!(request.chrome_args, ["--lang=fr"]);
        assert_eq!(request.follow_popups, Some(true));
    }
}
