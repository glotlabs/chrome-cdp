//! The session commands (plan 02, milestone 2.4): `open`, `connect`,
//! `close`, `status`, `sessions` and `cdp`; the tab commands (plan 03,
//! milestone 3.1): `tabs`, `tab <id>`, `tab new` and `tab close`; and
//! navigation (milestone 3.3): `goto`, `back`, `forward` and `reload`;
//! `wait` (milestone 3.4); `dialog` and `downloads` (milestone 3.5);
//! `snapshot` (plan 04); and the read commands `find`, `text`, `html`,
//! `attr`, `value`, `count`, `box` and `markdown` (milestone 4.5);
//! `console` and `errors` (plan 06, milestone 6.1), `network` (6.2), and
//! `screenshot` and `pdf` (6.3); `route`, `block`, `routes` and `unroute`
//! (plan 07, milestone 7.1).
//!
//! Each one finds the session's daemon (starting it when the command needs
//! a browser), sends one call, and returns the daemon's `Output` or
//! `ErrorInfo` as is, so rendering stays here, where the terminal is.
//! Anything that needs no daemon (argument checks, finding and installing
//! Chrome, with its progress bar) happens on this side first.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use daemon::client::{self, Session};
use daemon::meta::Meta;
use daemon::paths::{Env, SessionPaths, check_name};
use daemon::rpc::{self, ConnectParams, OpenParams, StatusParams};
use engine::{
    Action, ActionRequest, CdpRequest, ClipboardOp, ClipboardRequest, ComponentsRequest,
    ConnectRequest, ConsoleRequest, ContextCloseRequest, ContextListRequest, ContextNewRequest,
    CoverageStartRequest, CoverageStopRequest, DebugClearRequest, DebugListRequest,
    DebugLocalsRequest, DebugLogRequest, DialogRequest, DownloadsRequest, ElementState,
    EvalRequest, FillFormRequest, FindRequest, FormsRequest, FramesRequest, HarStartRequest,
    HarStopRequest, HighlightRequest, HttpCredentials, ImageFormat, InspectRequest, IssuesRequest,
    LayoutRequest, LocalsMode, LocatorRequest, NavigateRequest, Navigation, NetworkClearRequest,
    NetworkFramesRequest, NetworkRequest, NetworkShowRequest, OpenInitScript, OpenRequest,
    PdfRequest, PerfRequest, Read, ReadRequest, RouteRequest, RoutesRequest, ScreenshotRequest,
    SnapshotRequest, SourceRequest, TabCloseRequest, TabNewRequest, TabSelectRequest, TabsRequest,
    TraceStartRequest, TraceStopRequest, UnrouteRequest, WaitFor, WaitRequest, WhyRequest,
};
use engine::{
    CompareCapture, CompareCaptureRequest, CompareNameRequest, CompareRequest, CompareSide,
    ExploreRequest,
};
use engine::{
    CookieFilter, CookieSpec, CookiesOp, CookiesRequest, StateLoadRequest, StateSaveRequest,
    StorageArea, StorageOp, StorageRequest,
};
use engine::{EmulateRequest, EmulationChange, PermissionsOp, PermissionsRequest};
use engine::{ExposeRequest, InitScriptAddRequest, InitScriptListRequest, InitScriptRemoveRequest};
use engine::{ExtensionStorageArea, ExtensionStorageOp, ExtensionsOp, ExtensionsRequest};
use protocol::schema::{
    ClosedSession, CompareData, OpenData, SessionEntry, SessionState, StatusData,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Mode, Output};
use serde_json::{Map, Value, json};
use tokio::runtime::Runtime;

use crate::cli::{
    AfterArgs, BlockArgs, CdpArgs, ClipboardReadArgs, CloseArgs, CompareArgs, ComponentsArgs,
    ConnectArgs, ContextArgs, ContextCommand, CookieFilterArgs, CookieSetArgs, CookiesArgs,
    CookiesCommand, CoverageArgs, CoverageCommand, DEFAULT_TIMEOUT, DebugArgs, DebugCommand,
    DialogArgs, DialogCommand, DownloadsArgs, ElementStateArg, EvalArgs, ExploreArgs,
    ExtensionStorageAreaArg, ExtensionStorageCommand, ExtensionsArgs, ExtensionsCommand,
    FillFormArgs, FindArgs, FormsArgs, FrameDirArg, FramesArgs, GlobalArgs, GotoArgs, HarArgs,
    HarCommand, HighlightArgs, HistoryArgs, IdleArgs, ImageFormatArg, InitScriptAddArgs,
    InitScriptArgs, InitScriptCommand, InspectArgs, IssuesArgs, LayoutArgs, LocalsArg, LocatorArgs,
    LogReadArgs, NetworkArgs, NetworkCommand, OpenArgs, PdfArgs, PermissionsArgs,
    PermissionsCommand, ReloadArgs, RouteArgs, RoutesArgs, ScreenshotArgs, SessionsArgs,
    SnapshotArgs, SourceArgs, StateArgs, StateCommand, StatusArgs, StorageActionArg,
    StorageAreaArg, StorageArgs, StrictRefsArgs, TabArgs, TabCommand, TabsArgs, TraceArgs,
    TraceCommand, UnrouteArgs, WAIT_TIMEOUT, WaitCmdArgs, WhyArgs, parse_duration,
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
    /// `--context`: every call acts in this browser context.
    context: Option<String>,
    /// `base_url` from the config.
    base_url: Option<String>,
    /// `node_stacks` from the config: `open --node-stacks`' default.
    node_stacks: Option<engine::NodeStacks>,
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
            context: global.context.clone(),
            base_url: global.base_url.clone(),
            node_stacks: global.node_stacks,
        })
    }

    fn session(&self) -> &str {
        &self.paths.name
    }

    fn start(&self) -> Result<Session, ErrorInfo> {
        let mut session = self.runtime.block_on(client::connect_or_start(
            &self.env,
            &self.paths,
            &self.build,
        ))?;
        session.client.context.clone_from(&self.context);
        Ok(session)
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
        .map(|arg| http_credentials(arg, args.http_credentials_origin.as_deref()))
        .transpose()?;
    let launch = &args.launch;
    browser::launch::flags::check_extra(&launch.chrome_args)?;
    let extensions = extension_dirs(&launch.extensions, "--extension")?;
    if let Some(device) = &args.device {
        engine::find_device(device, &global.session)?;
    }
    if let Some(name) = &launch.profile {
        browser::launch::check_profile_name(name)?;
    }
    // Init scripts and the state file are read before a browser starts.
    let init_scripts = open_init_scripts(args)?;
    let state = args
        .state
        .as_deref()
        .map(|p| read_state(p, global))
        .transpose()?;
    let ctx = Ctx::new(global)?;
    let mut session = ctx.start()?;
    // What is read and checked before a browser starts; `open_in` adds
    // the rest.
    let read = OpenRequest {
        url,
        http_credentials: credentials,
        state,
        extensions,
        init_scripts,
        ..OpenRequest::default()
    };
    let result = open_in(&ctx, &mut session, args, read);
    if result.is_err() {
        ctx.abandon(&mut session);
    }
    result
}

fn open_in(
    ctx: &Ctx,
    session: &mut Session,
    args: &OpenArgs,
    read: OpenRequest,
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
    let mut warnings = chrome
        .as_ref()
        .map(|c| c.warnings.clone())
        .unwrap_or_default();
    warnings.extend(browser::launch::flags::debug_port_warning(
        &launch.chrome_args(),
    ));
    let mut request = OpenRequest {
        chrome,
        chrome_flag: launch.chrome.clone(),
        headed: launch.headed.then_some(true),
        profile: launch.profile(),
        chrome_args: launch.chrome_args(),
        restart: args.restart,
        base_url: ctx.base_url.clone(),
        tab,
        follow_popups: args.follow_popups.then_some(true),
        ignore_https_errors: args.ignore_https_errors.then_some(true),
        bypass_service_worker: args.bypass_service_worker.then_some(true),
        node_stacks: args
            .node_stacks
            .map(crate::cli::NodeStacks::to_engine)
            .or(ctx.node_stacks),
        device: args.device.clone(),
        state: read.state.map(|state| StateLoadRequest {
            timeout_ms: millis(ctx.timeout),
            ..state
        }),
        dialogs: args.dialogs.map(crate::cli::Dialogs::to_engine),
        console_buffer: args
            .console_buffer
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        network_buffer: args
            .network_buffer
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        downloads_dir: args
            .downloads_dir
            .as_deref()
            .map(input::absolute_path)
            .transpose()?
            .map(PathBuf::from),
        replaced_daemon: session
            .replaced
            .as_ref()
            .is_some_and(|m| m.browser.is_some()),
        timeout_ms: millis(ctx.timeout),
        ..read
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
    if request.extensions.is_empty() {
        request.extensions.clone_from(&previous.extensions);
    }
    if request.follow_popups.is_none() {
        request.follow_popups = previous.follow_popups;
    }
    if request.ignore_https_errors.is_none() {
        request.ignore_https_errors = previous.ignore_https_errors;
    }
    if request.bypass_service_worker.is_none() {
        request.bypass_service_worker = previous.bypass_service_worker;
    }
    if request.node_stacks.is_none() {
        request.node_stacks = previous.node_stacks;
    }
    if request.device.is_none() {
        request.device.clone_from(&previous.device);
    }
    if request.dialogs.is_none() {
        request.dialogs = previous.dialogs;
    }
    if request.http_credentials.is_none() {
        request
            .http_credentials
            .clone_from(&previous.http_credentials);
    }
    if request.console_buffer.is_none() {
        request.console_buffer = previous.console_buffer;
    }
    if request.network_buffer.is_none() {
        request.network_buffer = previous.network_buffer;
    }
    if request.downloads_dir.is_none() {
        request.downloads_dir.clone_from(&previous.downloads_dir);
    }
    request.init_scripts = OpenInitScript::merged(&previous.init_scripts, &request.init_scripts);
}

/// `--init-script` files (read now, so a missing one fails before a
/// browser starts), then `--init-script-code`s.
fn open_init_scripts(args: &OpenArgs) -> Result<Vec<OpenInitScript>, ErrorInfo> {
    let mut scripts = Vec::new();
    for path in &args.init_script {
        let absolute = PathBuf::from(input::absolute_path(path)?);
        scripts.push(OpenInitScript {
            source: read_script(&absolute)?,
            file: Some(absolute),
        });
    }
    scripts.extend(args.init_script_code.iter().map(|code| OpenInitScript {
        source: code.clone(),
        file: None,
    }));
    Ok(scripts)
}

/// `--extension`s as absolute paths (the daemon has another working
/// directory), each an unpacked extension's directory.
fn extension_dirs(dirs: &[PathBuf], what: &str) -> Result<Vec<PathBuf>, ErrorInfo> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(dirs.len());
    for dir in dirs {
        let hint = || {
            Hint::text(format!(
                "{what} takes an unpacked extension's directory, the one holding its manifest.json"
            ))
        };
        let abs = dir
            .canonicalize()
            .map_err(|e| bad_args(format!("{what} {}: {e}", dir.display())).hint(hint()))?;
        if !abs.join("manifest.json").is_file() {
            return Err(bad_args(format!(
                "{what} {}: no manifest.json in {}",
                dir.display(),
                abs.display()
            ))
            .hint(hint()));
        }
        if !out.contains(&abs) {
            out.push(abs);
        }
    }
    Ok(out)
}

/// Parses `--http-credentials USER:PASSWORD` (`-` reads it from stdin)
/// and `--http-credentials-origin`.
fn http_credentials(arg: &str, origin: Option<&str>) -> Result<HttpCredentials, ErrorInfo> {
    let origin = origin.map(credentials_origin).transpose()?;
    let text = input::text_arg(arg, "USER:PASSWORD")?;
    let text = text.trim_end_matches(['\r', '\n']);
    match text.split_once(':') {
        Some((username, password)) if !username.is_empty() => Ok(HttpCredentials {
            username: username.to_owned(),
            password: password.to_owned(),
            origin,
        }),
        _ => Err(
            bad_args("--http-credentials needs USER:PASSWORD").hint(Hint::run(
                Cmd::new("open").flag("http-credentials", "ada:s3cret"),
            )),
        ),
    }
}

/// `--http-credentials-origin` as an origin: `https://App.test/x` is
/// `https://app.test`.
fn credentials_origin(arg: &str) -> Result<String, ErrorInfo> {
    let origin = url::Url::parse(arg.trim())
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .map(|u| u.origin().ascii_serialization());
    origin.ok_or_else(|| {
        bad_args(format!(
            "--http-credentials-origin `{arg}` is not an http(s) origin"
        ))
        .hint(Hint::run(
            Cmd::new("open")
                .flag("http-credentials", "ada:s3cret")
                .flag("http-credentials-origin", "https://app.test"),
        ))
    })
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
    if let Some(method) = &args.method {
        check_method(method)?;
    }
    let params = parse_params(args.params.as_deref())?;
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let target = match args.target {
        Some(target) => target.to_engine(),
        None if args.frame.is_some() || args.all_sessions || ctx.tab.is_some() => {
            engine::Target::Page
        }
        None => engine::Target::Browser,
    };
    let request = CdpRequest {
        method: args.method.clone(),
        params,
        target: if args.frame.is_some() {
            engine::Target::Page
        } else {
            target
        },
        tab: ctx.tab.clone(),
        frame: args.frame.clone(),
        listen: args.listen.clone(),
        count: args.count.unwrap_or(1),
        all_sessions: args.all_sessions,
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    engine::check_cdp(&request)?;
    let (mut session, warnings) = with_browser(&ctx)?;
    let wait = ctx.timeout * 2 + Duration::from_secs(10);
    let output = ctx
        .runtime
        .block_on(session.client.call_output(rpc::CDP, &request, wait))?;
    Ok(warnings.into_iter().fold(output, Output::warning))
}

/// `eval`: needs a running session.
pub(crate) fn eval(args: &EvalArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = EvalRequest {
        script: input::text_arg(&args.script, "the script")?,
        locator: args.locator.clone(),
        frame: args.frame.clone(),
        world: args.world.to_engine(),
        tab: ctx.tab.clone(),
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        sourcemaps: args.sourcemaps,
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    engine::check_eval(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::EVAL, &request, ctx.timeout + QUICK))
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
    let mut client = ctx
        .runtime
        .block_on(client::connect(&ctx.paths, &ctx.build))?
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::DaemonUnavailable,
                format!("session {} is not running", ctx.session()),
            )
            .hint(Hint::run(Cmd::new("open").session(ctx.session())))
        })?;
    client.context.clone_from(&ctx.context);
    Ok(client)
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

/// `frames`: needs a running session.
pub(crate) fn frames(args: &FramesArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = FramesRequest {
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    ctx.runtime
        .block_on(client.call_output(rpc::FRAMES, &request, ctx.timeout + QUICK))
}

/// `context new|list|close`.
pub(crate) fn context(args: &ContextArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_context(global.context.as_deref())?;
    match &args.command {
        ContextCommand::New(new) => {
            let url = new.url.as_deref().map(engine::normalize_url).transpose()?;
            let ctx = Ctx::new(global)?;
            let (mut session, warnings) = with_browser(&ctx)?;
            let request = ContextNewRequest {
                url,
                base_url: ctx.base_url.clone(),
                timeout_ms: millis(ctx.timeout),
            };
            let output = ctx.runtime.block_on(session.client.call_output(
                rpc::CONTEXT_NEW,
                &request,
                ctx.long_wait(),
            ))?;
            Ok(warnings.into_iter().fold(output, Output::warning))
        }
        ContextCommand::List(list) => {
            let ctx = Ctx::new(global)?;
            let mut client = running_session(&ctx)?;
            let request = ContextListRequest {
                timeout_ms: millis(ctx.timeout),
                max_chars: list.max_chars,
            };
            ctx.runtime.block_on(client.call_output(
                rpc::CONTEXT_LIST,
                &request,
                ctx.timeout + QUICK,
            ))
        }
        ContextCommand::Close(close) => {
            for id in &close.ids {
                engine::parse_context_id(id)?;
            }
            let ctx = Ctx::new(global)?;
            let mut client = running_session(&ctx)?;
            let request = ContextCloseRequest {
                ids: close.ids.clone(),
                all: close.all,
                timeout_ms: millis(ctx.timeout),
            };
            ctx.runtime
                .block_on(client.call_output(rpc::CONTEXT_CLOSE, &request, ctx.long_wait()))
        }
    }
}

/// Checks a `--context` before any daemon starts.
fn check_context(context: Option<&str>) -> Result<(), ErrorInfo> {
    context
        .map(engine::parse_context_id)
        .transpose()
        .map(|_| ())
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
                    Hint::run(
                        Cmd::new("tabs")
                            .session(&global.session)
                            .context(global.context.as_deref()),
                    )
                    .why("lists the ids"),
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

/// `expect`: needs a running session.
pub(crate) fn expect(
    args: &crate::cli::ExpectArgs,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    if args.layout {
        return expect_layout(args, global);
    }
    if args.audit {
        return expect_audit(args, global);
    }
    let shared = [
        (args.baseline.is_some(), "--baseline"),
        (args.update, "--update"),
        (args.dir.is_some(), "--dir"),
    ];
    if let Some((_, flag)) = shared.into_iter().find(|(given, _)| *given) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{flag} is for expect --layout and expect --audit"),
        )
        .hint(Hint::run(
            Cmd::new("expect")
                .switch("audit")
                .flag("baseline", "checkout"),
        )));
    }
    if !args.categories.is_empty() {
        return Err(
            ErrorInfo::new(ErrorKind::BadArgs, "--category is for expect --audit").hint(Hint::run(
                Cmd::new("expect")
                    .switch("audit")
                    .flag("category", "a11y,meta"),
            )),
        );
    }
    if args.min_severity.is_some() && !args.no_issues {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "--min-severity is for expect --no-issues and expect --audit",
        )
        .hint(Hint::run(
            Cmd::new("expect")
                .switch("no-issues")
                .flag("min-severity", "error"),
        )));
    }
    if let Some(flag) = args.layout_flags.given() {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{flag} is for layout and expect --layout"),
        )
        .hint(Hint::run(
            Cmd::new("expect")
                .switch("layout")
                .flag("baseline", "cart")
                .raw(flag),
        )));
    }
    let expectation = crate::expect::expectation(args)?;
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::with_default(global, crate::expect::EXPECT_TIMEOUT)?;
    let request = engine::ExpectRequest {
        expectation,
        not: args.not,
        tab: ctx.tab.clone(),
        strict_refs: args.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    engine::check_expect(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::EXPECT, &request, ctx.timeout + QUICK))
}

/// `expect-snapshot <name>`: needs a running session.
pub(crate) fn expect_snapshot(
    args: &crate::cli::ExpectSnapshotArgs,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::with_default(global, crate::expect::EXPECT_TIMEOUT)?;
    let request = engine::ExpectSnapshotRequest {
        name: args.name.clone(),
        path: crate::expect::snapshot_baseline(args.dir.as_deref(), &args.name)?,
        scope: args.scope.clone(),
        full: args.full,
        update: args.update,
        create: crate::expect::create_missing(),
        dir: args.dir.as_ref().map(|d| d.display().to_string()),
        tab: ctx.tab.clone(),
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    engine::check_expect_snapshot(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::EXPECT_SNAPSHOT, &request, ctx.timeout + QUICK))
}

/// `expect-screenshot <name>`: needs a running session.
pub(crate) fn expect_screenshot(
    args: &crate::cli::ExpectScreenshotArgs,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::with_default(global, crate::expect::EXPECT_TIMEOUT)?;
    let path = crate::expect::screenshot_baseline(args.dir.as_deref(), &args.name)?;
    let other_os = (!args.update)
        .then(|| crate::expect::other_os_baselines(&path))
        .flatten();
    let request = engine::ExpectScreenshotRequest {
        name: args.name.clone(),
        path,
        locator: args.locator.clone(),
        full: args.full.is_some(),
        container: args.full.clone().filter(|c| !c.is_empty()),
        mask: args.mask.clone(),
        threshold: args.threshold,
        max_diff_pixels: args.max_diff_pixels,
        update: args.update,
        create: crate::expect::create_missing(),
        dir: args.dir.as_ref().map(|d| d.display().to_string()),
        tab: ctx.tab.clone(),
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    engine::check_expect_screenshot(&request)?;
    let mut client = running_session(&ctx)?;
    let result = ctx.runtime.block_on(client.call_output(
        rpc::EXPECT_SCREENSHOT,
        &request,
        ctx.timeout + QUICK,
    ));
    match (result, other_os) {
        (Ok(out), Some(warning)) => Ok(out.warning(warning)),
        (Err(info), Some(warning)) => Err(info.note("other OS", warning)),
        (result, None) => result,
    }
}

/// `mark <name>`: needs a running session.
pub(crate) fn mark(args: &crate::cli::MarkArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    if !crate::cli::is_mark_name(&args.name) {
        return Err(bad_args(format!(
            "{:?} cannot name a mark: start with a letter, then letters, digits, -, _ or . \
             (and not last or a duration, which --since reads as such)",
            args.name
        ))
        .hint(Hint::run(Cmd::new("mark").arg("before-save"))));
    }
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = engine::MarkRequest {
        name: args.name.clone(),
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    ctx.runtime
        .block_on(client.call_output(rpc::MARK, &request, ctx.timeout + QUICK))
}

/// `snapshot`: needs a running session.
pub(crate) fn snapshot(args: &SnapshotArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let budget = args.budget.or(global.snapshot_budget).filter(|&b| b > 0);
    // A budget above the default --max-chars raises it, so the budget is
    // what bounds the output.
    let max_chars = match budget {
        Some(tokens) if args.max_chars == crate::cli::SNAPSHOT_MAX_CHARS => {
            args.max_chars.max(tokens.saturating_mul(3))
        }
        _ => args.max_chars,
    };
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
        source: args.source,
        fold: args.fold,
        budget,
        expand: args.expand.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars,
    };
    engine::check_snapshot(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::SNAPSHOT, &request, ctx.timeout + QUICK))
}

/// `find <query>`: needs a running session.
pub(crate) fn find(args: &FindArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let query = input::text_arg(&args.query.join(" "), "QUERY")?;
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = FindRequest {
        query,
        role: args.role.clone(),
        scope: args.scope.clone(),
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        limit: args.limit,
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    engine::check_find(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::FIND, &request, ctx.timeout + QUICK))
}

/// `locator`: needs a running session.
pub(crate) fn locator(args: &LocatorArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let locator = input::text_arg(&args.locator, "LOCATOR")?;
    engine::parse_locator(&locator)?;
    check_tab(global.tab.as_deref())?;
    // It waits for its element as actions do, so it has their timeout.
    let ctx = Ctx::with_default(global, WAIT_TIMEOUT)?;
    let request = LocatorRequest {
        locator,
        all: args.all,
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::LOCATOR, &request, ctx.timeout + QUICK))
}

/// `text`, `html`, `attr`, `value`, `count`, `box` and `markdown`: need a
/// running session. They wait up to 10s by default, like actions.
pub(crate) fn read(
    read: Read,
    locator: Option<String>,
    refs: &StrictRefsArgs,
    max_chars: usize,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    // They wait for their element as actions do, so they have their
    // timeout.
    let ctx = Ctx::with_default(global, WAIT_TIMEOUT)?;
    let request = ReadRequest {
        read,
        locator,
        tab: ctx.tab.clone(),
        strict_refs: refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars,
    };
    engine::check_read(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::READ, &request, ctx.timeout + QUICK))
}

/// The input actions (`click`, `hover`, `drag`, `scroll`, `mouse`, …):
/// need a running session. They wait up to 10s by default, like `wait`.
pub(crate) fn act(
    action: Action,
    strict_refs: bool,
    after: &AfterArgs,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::with_default(global, WAIT_TIMEOUT)?;
    let request = ActionRequest {
        action,
        tab: ctx.tab.clone(),
        strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        no_wait_after: after.no_wait_after,
        snap: after.snap(global),
        max_chars: after.max_chars,
    };
    engine::check_action(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::ACT, &request, ctx.timeout + QUICK))
}

/// `clipboard read`: needs a running session. An image needs `-o`; with
/// `-o -`, it goes to stdout and nothing else does.
pub(crate) fn clipboard_read(
    args: &ClipboardReadArgs,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    let image = !args.mime.starts_with("text/");
    if image && args.output.is_none() {
        return Err(
            bad_args(format!("an image needs -o: {} is not text", args.mime)).hint(Hint::run(
                Cmd::new("clipboard read")
                    .flag("type", &args.mime)
                    .raw("-o <file.png>")
                    .session(&global.session),
            )),
        );
    }
    let (path, stdout) = if image {
        output_path(args.output.as_deref(), global, "clipboard read")?
    } else {
        if args.output.is_some() {
            return Err(
                bad_args("-o saves an image; text is printed").hint(Hint::text(
                    "redirect it: chrome-cdp clipboard read > file.txt",
                )),
            );
        }
        (None, false)
    };
    let op = ClipboardOp::Read {
        mime: Some(args.mime.clone()),
        path,
    };
    let output = clipboard(op, args.max_chars, global)?;
    if stdout {
        to_stdout(output)
    } else {
        Ok(output)
    }
}

/// `clipboard read` and `clipboard write`: need a running session.
pub(crate) fn clipboard(
    op: ClipboardOp,
    max_chars: usize,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = ClipboardRequest {
        op,
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars,
    };
    ctx.runtime
        .block_on(client.call_output(rpc::CLIPBOARD, &request, ctx.timeout + QUICK))
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
        clear: args.clear,
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    ctx.runtime
        .block_on(client.call_output(rpc::DOWNLOADS, &request, ctx.timeout + QUICK))
}

/// `network`, `network show` and `network clear`: need a running
/// session.
pub(crate) fn network(args: &NetworkArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let timeout_ms = millis(ctx.timeout);
    let wait = ctx.timeout + QUICK;
    match &args.command {
        None => {
            let list = &args.list;
            let mut request = NetworkRequest {
                tab: ctx.tab.clone(),
                failed: list.failed,
                types: list.types.clone(),
                url: list.url.clone(),
                method: list.method.clone(),
                status: list.status.clone(),
                since: list.since.clone(),
                limit: list.limit,
                wait_ms: 0,
                sourcemaps: list.sourcemaps,
                timeout_ms,
                max_chars: list.max_chars,
            };
            if !list.follow {
                return ctx
                    .runtime
                    .block_on(client.call_output(rpc::NETWORK, &request, wait));
            }
            follow(
                global,
                "network",
                "requests",
                |_| false,
                |poll, first| {
                    request.wait_ms = millis(poll);
                    if !first {
                        request.since = engine::Since::Last;
                    }
                    ctx.runtime
                        .block_on(client.call_output(rpc::NETWORK, &request, poll + wait))
                },
            )
        }
        Some(NetworkCommand::Show(show)) => {
            let output = match show.output.as_deref() {
                Some(path) if path.as_os_str() == "-" => {
                    return Err(bad_args("network show -o needs a file name").hint(
                        Hint::run(
                            Cmd::new("network")
                                .arg("show")
                                .arg(show.id.to_string())
                                .switch("body")
                                .raw("-q"),
                        )
                        .why("prints a text body alone"),
                    ));
                }
                Some(path) => Some(PathBuf::from(input::absolute_path(path)?)),
                None => None,
            };
            let request = NetworkShowRequest {
                tab: ctx.tab.clone(),
                id: show.id,
                headers: show.headers,
                body: show.body,
                request_body: show.request_body,
                output,
                sourcemaps: show.sourcemaps,
                timeout_ms,
                max_chars: show.max_chars,
            };
            ctx.runtime
                .block_on(client.call_output(rpc::NETWORK_SHOW, &request, wait))
        }
        Some(NetworkCommand::Frames(frames)) => {
            let mut request = NetworkFramesRequest {
                tab: ctx.tab.clone(),
                id: frames.id,
                grep: frames.grep.clone(),
                sent: frames.dir.map(|d| d == FrameDirArg::Sent),
                limit: frames.limit,
                after: 0,
                wait_ms: 0,
                timeout_ms,
                max_chars: frames.max_chars,
            };
            if !frames.follow {
                return ctx.runtime.block_on(client.call_output(
                    rpc::NETWORK_FRAMES,
                    &request,
                    wait,
                ));
            }
            let closed = |output: &Output| output.data["request"]["websocket"]["closed"] == true;
            follow(global, "frames", "frames", closed, |poll, _| {
                request.wait_ms = millis(poll);
                let output = ctx.runtime.block_on(client.call_output(
                    rpc::NETWORK_FRAMES,
                    &request,
                    poll + wait,
                ))?;
                request.after = output.data["last"].as_u64().unwrap_or(request.after);
                Ok(output)
            })
        }
        Some(NetworkCommand::Clear(_)) => {
            let request = NetworkClearRequest {
                tab: ctx.tab.clone(),
                timeout_ms,
            };
            ctx.runtime
                .block_on(client.call_output(rpc::NETWORK_CLEAR, &request, wait))
        }
    }
}

/// `screenshot`: needs a running session. With `-o -`, the image goes to
/// stdout and nothing else does.
pub(crate) fn screenshot(args: &ScreenshotArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    // An element is waited for as actions and reads wait for theirs.
    let ctx = if args.locator.is_some() {
        Ctx::with_default(global, WAIT_TIMEOUT)?
    } else {
        Ctx::new(global)?
    };
    let (path, stdout) = output_path(args.output.as_deref(), global, "screenshot")?;
    let format = args
        .format
        .map(ImageFormatArg::to_format)
        .or_else(|| path.as_deref().and_then(ImageFormat::from_path))
        .unwrap_or_default();
    let request = ScreenshotRequest {
        tab: ctx.tab.clone(),
        locator: args.locator.clone(),
        full: args.full.is_some(),
        container: args.full.clone().filter(|c| !c.is_empty()),
        format,
        quality: args.quality,
        path,
        scale: args.scale.to_scale(),
        max_size: args.max_size,
        omit_background: args.omit_background,
        mask: args.mask.clone(),
        annotate: args.annotate,
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    engine::check_screenshot(&request)?;
    let mut client = running_session(&ctx)?;
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::SCREENSHOT, &request, ctx.timeout + QUICK))?;
    if stdout {
        to_stdout(output)
    } else {
        Ok(output)
    }
}

/// `pdf`: needs a running session. With `-o -`, the PDF goes to stdout
/// and nothing else does.
pub(crate) fn pdf(args: &PdfArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let (path, stdout) = output_path(args.output.as_deref(), global, "pdf")?;
    let request = PdfRequest {
        tab: ctx.tab.clone(),
        path,
        format: args.format.to_paper(),
        landscape: args.landscape,
        background: args.background,
        margin: args.margin,
        pages: args.pages.clone(),
        css_page_size: args.css_page_size,
        header: args
            .header
            .as_deref()
            .map(|t| pdf_template(t, "--header"))
            .transpose()?,
        footer: args
            .footer
            .as_deref()
            .map(|t| pdf_template(t, "--footer"))
            .transpose()?,
        scale: args.scale,
        timeout_ms: millis(ctx.timeout),
    };
    let mut client = running_session(&ctx)?;
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::PDF, &request, ctx.timeout + QUICK))?;
    if stdout {
        to_stdout(output)
    } else {
        Ok(output)
    }
}

/// A `pdf --header`/`--footer` template: the HTML given, or `@FILE`'s.
fn pdf_template(template: &str, flag: &str) -> Result<String, ErrorInfo> {
    let Some(path) = template.strip_prefix('@') else {
        return Ok(template.to_owned());
    };
    std::fs::read_to_string(path).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{flag} @{path}: cannot read it: {e}"),
        )
        .hint(Hint::text(format!(
            "{flag} takes the HTML itself, or @FILE for a file holding it"
        )))
    })
}

/// `route`: needs a running session.
pub(crate) fn route(args: &RouteArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let rule = crate::route::spec(args)?;
    add_routes(vec![rule], global)
}

/// `block`: needs a running session.
pub(crate) fn block(args: &BlockArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let rules = crate::route::block_specs(args)?;
    add_routes(rules, global)
}

fn add_routes(rules: Vec<engine::RouteSpec>, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = RouteRequest {
        rules,
        timeout_ms: millis(ctx.timeout),
    };
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::ROUTE, &request, ctx.timeout + QUICK))?;
    Ok(every_tab(output, &ctx, "routes"))
}

/// `routes`: needs a running session.
pub(crate) fn routes(args: &RoutesArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = RoutesRequest {
        log: args.log,
        limit: args.limit,
        max_chars: args.max_chars,
    };
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::ROUTES, &request, ctx.timeout + QUICK))?;
    Ok(every_tab(output, &ctx, "routes"))
}

/// `unroute`: needs a running session.
pub(crate) fn unroute(args: &UnrouteArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = UnrouteRequest {
        ids: args.ids.clone(),
        all: args.all,
        timeout_ms: millis(ctx.timeout),
    };
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::UNROUTE, &request, ctx.timeout + QUICK))?;
    Ok(every_tab(output, &ctx, "routes"))
}

/// A call of a command that needs a running session and applies to all
/// of its tabs (the `mock` commands): `what` names them in the warning
/// about `--tab`.
pub(crate) fn call_session<T: serde::Serialize>(
    global: &GlobalArgs,
    method: &str,
    request: &T,
    what: &str,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let output = ctx
        .runtime
        .block_on(client.call_output(method, request, ctx.timeout + QUICK))?;
    Ok(every_tab(output, &ctx, what))
}

/// A call that needs a running session, with the request as given
/// (`--tab`, if any, is in it).
pub(crate) fn call_running<T: serde::Serialize>(
    global: &GlobalArgs,
    method: &str,
    request: &T,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(method, request, ctx.timeout + QUICK))
}

/// As [`call_session`], but `None` when the session is not running.
pub(crate) fn call_if_running<T: serde::Serialize>(
    global: &GlobalArgs,
    method: &str,
    request: &T,
) -> Result<Option<Output>, ErrorInfo> {
    let ctx = Ctx::new(global)?;
    let Some(mut client) = ctx
        .runtime
        .block_on(client::connect(&ctx.paths, &ctx.build))?
    else {
        return Ok(None);
    };
    client.context.clone_from(&ctx.context);
    ctx.runtime
        .block_on(client.call_output(method, request, ctx.timeout + QUICK))
        .map(Some)
}

/// The command's `--timeout` in ms.
pub(crate) fn timeout_ms(global: &GlobalArgs) -> u64 {
    millis(global.timeout_or(DEFAULT_TIMEOUT))
}

/// `init-script add|list|remove`: need a running session.
pub(crate) fn init_script(args: &InitScriptArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let timeout_ms = millis(ctx.timeout);
    // The script is read before the session is looked for.
    let (method, request) = match &args.command {
        InitScriptCommand::Add(add) => {
            let mut request = init_script_request(add)?;
            request.timeout_ms = timeout_ms;
            (rpc::INIT_SCRIPT_ADD, json!(request))
        }
        InitScriptCommand::List(list) => (
            rpc::INIT_SCRIPT_LIST,
            json!(InitScriptListRequest {
                max_chars: list.max_chars,
            }),
        ),
        InitScriptCommand::Remove(remove) => (
            rpc::INIT_SCRIPT_REMOVE,
            json!(InitScriptRemoveRequest {
                ids: remove.ids.clone(),
                all: remove.all,
                timeout_ms,
            }),
        ),
    };
    let mut client = running_session(&ctx)?;
    let output = ctx
        .runtime
        .block_on(client.call_output(method, &request, ctx.timeout + QUICK))?;
    Ok(every_tab(output, &ctx, "init scripts"))
}

/// `init-script add`'s request without its timeout: the script read from
/// its file, stdin or `--code`.
fn init_script_request(args: &InitScriptAddArgs) -> Result<InitScriptAddRequest, ErrorInfo> {
    let (source, file) = match (&args.code, &args.file) {
        (Some(code), _) => (code.clone(), None),
        (None, Some(path)) if path.as_os_str() == "-" => {
            (input::text_arg("-", "the init script")?, None)
        }
        (None, Some(path)) => {
            let absolute = PathBuf::from(input::absolute_path(path)?);
            (read_script(&absolute)?, Some(absolute))
        }
        (None, None) => {
            return Err(ErrorInfo::new(ErrorKind::BadArgs, "no init script given")
                .hint(Hint::run(Cmd::new("init-script").arg("add").raw("<file>"))));
        }
    };
    Ok(InitScriptAddRequest {
        source,
        file,
        world: args.world.to_engine(),
        now: args.now,
        timeout_ms: 0,
    })
}

/// Reads an init script's file, up to [`engine::MAX_SCRIPT`] bytes.
fn read_script(path: &Path) -> Result<String, ErrorInfo> {
    let cannot = |e: &dyn std::fmt::Display| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read {}: {e}", path.display()),
        )
    };
    let len = std::fs::metadata(path).map_err(|e| cannot(&e))?.len();
    if len > engine::MAX_SCRIPT as u64 {
        return Err(cannot(&format_args!(
            "it is {}; the most an init script can be is {}",
            protocol::format::size(len),
            protocol::format::size(engine::MAX_SCRIPT as u64)
        )));
    }
    std::fs::read_to_string(path).map_err(|e| cannot(&e))
}

/// What `expose` or `unexpose` does to its names.
#[derive(Clone, Copy)]
pub(crate) enum Exposing<'a> {
    /// `expose`, with `--returns` or `--returns-seq` (JSON, unchecked).
    Add {
        returns: Option<&'a str>,
        returns_seq: Option<&'a str>,
    },
    /// `unexpose`, `--all` or not.
    Remove { all: bool },
}

/// `expose` and `unexpose`: need a running session.
pub(crate) fn expose(
    names: &[String],
    what: Exposing<'_>,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    for name in names {
        engine::check_binding_name(name)?;
    }
    let (remove, all, returns) = match what {
        Exposing::Add {
            returns,
            returns_seq,
        } => (false, false, expose_returns(returns, returns_seq)?),
        Exposing::Remove { all } => (true, all, Vec::new()),
    };
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = ExposeRequest {
        names: names.to_vec(),
        remove,
        all,
        returns,
        timeout_ms: millis(ctx.timeout),
    };
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::EXPOSE, &request, ctx.timeout + QUICK))?;
    Ok(every_tab(output, &ctx, "exposed functions"))
}

/// The answers of `expose --returns JSON` (`[JSON]`) or `--returns-seq
/// JSON-ARRAY`.
fn expose_returns(returns: Option<&str>, seq: Option<&str>) -> Result<Vec<Value>, ErrorInfo> {
    let (flag, text) = match (returns, seq) {
        (Some(text), _) => ("--returns", text),
        (None, Some(text)) => ("--returns-seq", text),
        (None, None) => return Ok(Vec::new()),
    };
    let value: Value = serde_json::from_str(text).map_err(|e| {
        bad_args(format!("{flag} is not valid JSON: {e}")).hint(Hint::text(
            "quote it for the shell, e.g. --returns '{\"ok\": true}'; a string is '\"text\"'",
        ))
    })?;
    if returns.is_some() {
        return Ok(vec![value]);
    }
    match value {
        Value::Array(values) if !values.is_empty() => Ok(values),
        Value::Array(_) => Err(bad_args("--returns-seq needs at least one value")
            .hint(Hint::text("e.g. --returns-seq '[1, 2, 3]'"))),
        other => Err(bad_args(format!(
            "--returns-seq must be a JSON array, not {}",
            json_type(&other)
        ))
        .hint(Hint::text(
            "wrap the values in an array, e.g. --returns-seq '[1, 2, 3]', or give one with --returns",
        ))),
    }
}

/// The emulation commands (`emulate`, `viewport`, `geo`, …): need a
/// running session.
pub(crate) fn emulate(change: EmulationChange, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = EmulateRequest {
        change,
        timeout_ms: millis(ctx.timeout),
    };
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::EMULATE, &request, ctx.timeout + QUICK))?;
    Ok(if ctx.tab.is_some() {
        output.warning("emulation applies to every tab of the session; --tab is ignored")
    } else {
        output
    })
}

/// `permissions`: needs a running session.
pub(crate) fn permissions(
    args: &PermissionsArgs,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let (op, names, origin) = match &args.command {
        None => (PermissionsOp::List, Vec::new(), None),
        Some(PermissionsCommand::Grant(a)) => {
            (PermissionsOp::Grant, a.names.clone(), a.origin.clone())
        }
        Some(PermissionsCommand::Deny(a)) => {
            (PermissionsOp::Deny, a.names.clone(), a.origin.clone())
        }
        Some(PermissionsCommand::Reset(a)) => {
            (PermissionsOp::Reset, a.names.clone(), a.origin.clone())
        }
    };
    for name in &names {
        engine::check_permission(name, op, &global.session)?;
    }
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = PermissionsRequest {
        op,
        names,
        origin,
        timeout_ms: millis(ctx.timeout),
    };
    let output = ctx.runtime.block_on(client.call_output(
        rpc::PERMISSIONS,
        &request,
        ctx.timeout + QUICK,
    ))?;
    Ok(if ctx.tab.is_some() {
        output.warning("permissions apply to every tab of the session; --tab is ignored")
    } else {
        output
    })
}

/// `extensions` and its subcommands: need a running session.
pub(crate) fn extensions(args: &ExtensionsArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let mut max_chars = args.max_chars;
    let op = match &args.command {
        None => ExtensionsOp::List,
        Some(ExtensionsCommand::Load(load)) => {
            let dir = extension_dirs(std::slice::from_ref(&load.dir), "extensions load")?;
            ExtensionsOp::Load {
                dir: dir.into_iter().next().unwrap_or_default(),
            }
        }
        Some(ExtensionsCommand::Reload(a)) => ExtensionsOp::Reload {
            extension: a.extension.clone(),
        },
        Some(ExtensionsCommand::Uninstall(a)) => ExtensionsOp::Uninstall {
            extension: a.extension.clone(),
        },
        Some(ExtensionsCommand::Popup(a)) => ExtensionsOp::Popup {
            extension: a.extension.clone(),
        },
        Some(ExtensionsCommand::Storage(storage)) => {
            max_chars = storage.max_chars;
            let op = match &storage.command {
                None => ExtensionStorageOp::Get { keys: Vec::new() },
                Some(ExtensionStorageCommand::Get(get)) => ExtensionStorageOp::Get {
                    keys: get.keys.clone(),
                },
                Some(ExtensionStorageCommand::Set(set)) => ExtensionStorageOp::Set {
                    values: storage_values(&input::text_arg(&set.items, "JSON")?, global)?,
                },
                Some(ExtensionStorageCommand::Remove(remove)) => ExtensionStorageOp::Remove {
                    keys: remove.keys.clone(),
                },
                Some(ExtensionStorageCommand::Clear(_)) => ExtensionStorageOp::Clear,
            };
            ExtensionsOp::Storage {
                extension: storage.extension.clone(),
                area: match storage.area {
                    ExtensionStorageAreaArg::Local => ExtensionStorageArea::Local,
                    ExtensionStorageAreaArg::Sync => ExtensionStorageArea::Sync,
                    ExtensionStorageAreaArg::Session => ExtensionStorageArea::Session,
                    ExtensionStorageAreaArg::Managed => ExtensionStorageArea::Managed,
                },
                op,
            }
        }
    };
    let popup = matches!(op, ExtensionsOp::Popup { .. });
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = ExtensionsRequest {
        op,
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars,
    };
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::EXTENSIONS, &request, ctx.timeout + QUICK))?;
    Ok(if ctx.tab.is_some() && !popup {
        output.warning("extensions belong to the browser, not a tab; --tab is ignored")
    } else {
        output
    })
}

/// The JSON object `extensions storage set` merges in.
fn storage_values(text: &str, global: &GlobalArgs) -> Result<Map<String, Value>, ErrorInfo> {
    let example = || {
        Hint::run(
            Cmd::new("extensions")
                .arg("storage")
                .raw("NAME")
                .arg("set")
                .arg(r#"{"theme": "dark"}"#)
                .session(&global.session),
        )
    };
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(values)) => Ok(values),
        Ok(other) => Err(bad_args(format!(
            "set takes a JSON object of the items to set, not {}",
            json_kind(&other)
        ))
        .hint(example())),
        Err(e) => Err(bad_args(format!("set: the JSON is not valid: {e}")).hint(example())),
    }
}

/// `an array`, `a string`, …: what a JSON value is, for errors.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// `cookies`, `cookies set` and `cookies clear`: need a running session.
pub(crate) fn cookies(args: &CookiesArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let op = match &args.command {
        None => CookiesOp::List {
            filter: cookie_filter(&args.filter),
        },
        Some(CookiesCommand::Set(set)) => CookiesOp::Set {
            cookie: cookie_spec(set)?,
        },
        Some(CookiesCommand::Clear(filter)) => CookiesOp::Clear {
            filter: cookie_filter(filter),
        },
    };
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = CookiesRequest {
        op,
        max_chars: args.max_chars,
        timeout_ms: millis(ctx.timeout),
    };
    let output =
        ctx.runtime
            .block_on(client.call_output(rpc::COOKIES, &request, ctx.timeout + QUICK))?;
    Ok(every_tab(output, &ctx, "cookies"))
}

fn cookie_filter(args: &CookieFilterArgs) -> CookieFilter {
    CookieFilter {
        url: args.url.clone(),
        domain: args.domain.clone(),
        name: args.name.clone(),
    }
}

/// `cookies set`'s cookie: NAME=VALUE and the flags.
fn cookie_spec(args: &CookieSetArgs) -> Result<CookieSpec, ErrorInfo> {
    let (name, value) = args
        .cookie
        .split_once('=')
        .filter(|(name, _)| !name.trim().is_empty())
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{}` is not a cookie; use NAME=VALUE", args.cookie),
            )
            .hint(Hint::run(Cmd::new("cookies").arg("set").arg("theme=dark")))
        })?;
    Ok(CookieSpec {
        name: name.trim().to_owned(),
        value: value.to_owned(),
        url: args.url.clone(),
        domain: args.domain.clone(),
        path: args.path.clone(),
        secure: args.secure,
        http_only: args.httponly,
        same_site: args.samesite.map(|s| s.as_str().to_owned()),
        expires: args.expires,
    })
}

/// `storage local|session [ACTION] [KEY] [VALUE]`: needs a running
/// session.
pub(crate) fn storage(args: &StorageArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let area = match args.area {
        StorageAreaArg::Local => StorageArea::Local,
        StorageAreaArg::Session => StorageArea::Session,
    };
    let action = format!("{:?}", args.action).to_lowercase();
    let usage = |what: &str| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("storage {} {action} {what}", area.as_str()),
        )
        .hint(Hint::run(Cmd::new("storage").switch("help")))
    };
    let key = || args.key.clone().ok_or_else(|| usage("needs a KEY"));
    if args.value.is_some() && args.action != StorageActionArg::Set {
        return Err(usage("takes no VALUE"));
    }
    let op = match args.action {
        StorageActionArg::List | StorageActionArg::Clear if args.key.is_some() => {
            return Err(usage("takes no KEY"));
        }
        StorageActionArg::List => StorageOp::List,
        StorageActionArg::Clear => StorageOp::Clear,
        StorageActionArg::Get => StorageOp::Get { key: key()? },
        StorageActionArg::Remove => StorageOp::Remove { key: key()? },
        StorageActionArg::Set => {
            let key = key()?;
            let value = args
                .value
                .as_deref()
                .ok_or_else(|| usage("needs a KEY and a VALUE"))?;
            StorageOp::Set {
                key,
                value: input::text_arg(value, "VALUE")?,
            }
        }
    };
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = StorageRequest {
        area,
        op,
        origin: args.origin.clone(),
        tab: ctx.tab.clone(),
        max_chars: args.max_chars,
        timeout_ms: millis(ctx.timeout),
    };
    ctx.runtime
        .block_on(client.call_output(rpc::STORAGE, &request, ctx.timeout + QUICK))
}

/// `state save` and `state load`: need a running session.
pub(crate) fn state(args: &StateArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let timeout_ms = millis(ctx.timeout);
    // The file is read before the session is looked for.
    let (method, request) = match &args.command {
        StateCommand::Save(save) => (
            rpc::STATE_SAVE,
            json!(StateSaveRequest {
                path: PathBuf::from(input::absolute_path(&save.file)?),
                indexed_db: save.indexeddb,
                timeout_ms,
            }),
        ),
        StateCommand::Load(load) => {
            let mut request = read_state(&load.file, global)?;
            request.timeout_ms = timeout_ms;
            (rpc::STATE_LOAD, json!(request))
        }
    };
    let mut client = running_session(&ctx)?;
    let output = ctx
        .runtime
        .block_on(client.call_output(method, &request, ctx.timeout + QUICK))?;
    Ok(every_tab(output, &ctx, "cookies and storage state"))
}

/// Reads a storage state file (`-`: stdin) into `state load`'s request,
/// without its timeout.
fn read_state(path: &Path, global: &GlobalArgs) -> Result<StateLoadRequest, ErrorInfo> {
    let save = Cmd::new("state")
        .arg("save")
        .raw("<file>")
        .session(&global.session)
        .context(global.context.as_deref());
    let (text, file) = if path.as_os_str() == "-" {
        (input::text_arg("-", "the storage state")?, None)
    } else {
        let absolute = PathBuf::from(input::absolute_path(path)?);
        let text = std::fs::read_to_string(&absolute).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read {}: {e}", absolute.display()),
            )
            .hint(Hint::run(save.clone()).why("writes one"))
        })?;
        (text, Some(absolute))
    };
    let where_ = file
        .as_ref()
        .map_or_else(|| "stdin".to_owned(), |f| f.display().to_string());
    let state = engine::StorageState::parse(&text).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{where_} is not a storage state file: {e}"),
        )
        .hint(Hint::run(save).why("writes one, as does Playwright's storageState()"))
    })?;
    Ok(StateLoadRequest {
        state,
        path: file,
        timeout_ms: 0,
    })
}

/// Routes belong to a browser context: a `--tab` is no scope for them.
fn every_tab(output: Output, ctx: &Ctx, what: &str) -> Output {
    if ctx.tab.is_some() {
        let whose = ctx.context.as_ref().map_or_else(
            || "the session".to_owned(),
            |context| format!("context {context}"),
        );
        output.warning(format!(
            "{what} apply to every tab of {whose}; --tab is ignored"
        ))
    } else {
        output
    }
}

/// `har start` and `har stop`: need a running session.
pub(crate) fn har(args: &HarArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let timeout_ms = millis(ctx.timeout);
    match &args.command {
        HarCommand::Start(start) => {
            let request = HarStartRequest {
                tab: ctx.tab.clone(),
                content: start.content.to_content(),
                timeout_ms,
            };
            ctx.runtime
                .block_on(client.call_output(rpc::HAR_START, &request, ctx.timeout + QUICK))
        }
        HarCommand::Stop(stop) => {
            let path = stop
                .output
                .as_deref()
                .map(input::absolute_path)
                .transpose()?
                .map(PathBuf::from);
            let request = HarStopRequest { path, timeout_ms };
            ctx.runtime
                .block_on(client.call_output(rpc::HAR_STOP, &request, ctx.timeout + QUICK))
        }
    }
}

/// `trace start` and `trace stop`: need a running session.
pub(crate) fn trace(args: &TraceArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let timeout_ms = millis(ctx.timeout);
    match &args.command {
        TraceCommand::Start(start) => {
            let request = TraceStartRequest {
                tab: ctx.tab.clone(),
                screenshots: start.screenshots,
                categories: start.categories.clone(),
                timeout_ms,
            };
            ctx.runtime.block_on(client.call_output(
                rpc::TRACE_START,
                &request,
                ctx.timeout + QUICK,
            ))
        }
        TraceCommand::Stop(stop) => {
            let path = stop
                .output
                .as_deref()
                .map(input::absolute_path)
                .transpose()?
                .map(PathBuf::from);
            let request = TraceStopRequest { path, timeout_ms };
            ctx.runtime
                .block_on(client.call_output(rpc::TRACE_STOP, &request, ctx.timeout + QUICK))
        }
    }
}

/// `perf`: needs a running session.
pub(crate) fn perf(global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let request = PerfRequest {
        tab: ctx.tab.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    ctx.runtime
        .block_on(client.call_output(rpc::PERF, &request, ctx.timeout + QUICK))
}

/// `debug log`, `debug list`, `debug clear` and `debug locals`: need a
/// running session.
pub(crate) fn debug(args: &DebugArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let (method, request) = match &args.command {
        DebugCommand::Log(log) => {
            let (path, line, column) = parse_place(&log.place)?;
            if log.exprs.iter().filter(|e| *e == "-").count() > 1 {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    "only one expression can be read from stdin (`-`)",
                )
                .hint("give the others on the command line, each one argument"));
            }
            let mut exprs = Vec::with_capacity(log.exprs.len());
            for expr in &log.exprs {
                exprs.push(input::text_arg(expr, "expression")?.trim().to_owned());
            }
            // The file the path names, when it is one here: the daemon
            // matches sources on disk against it.
            let file = (!path.contains("://"))
                .then(|| std::fs::canonicalize(&path).ok())
                .flatten()
                .filter(|f| f.is_file())
                .and_then(|f| f.into_os_string().into_string().ok());
            let request = DebugLogRequest {
                tab: global.tab.clone(),
                path,
                file,
                line,
                column,
                exprs,
                limit: log.limit,
                timeout_ms: 0,
            };
            (rpc::DEBUG_LOG, serde_json::to_value(request))
        }
        DebugCommand::List(_) => (
            rpc::DEBUG_LIST,
            serde_json::to_value(DebugListRequest { timeout_ms: 0 }),
        ),
        DebugCommand::Clear(clear) => (
            rpc::DEBUG_CLEAR,
            serde_json::to_value(DebugClearRequest {
                id: if clear.all { None } else { clear.id },
                timeout_ms: 0,
            }),
        ),
        DebugCommand::Locals(locals) => (
            rpc::DEBUG_LOCALS,
            serde_json::to_value(DebugLocalsRequest {
                set: locals.mode.map(|m| match m {
                    LocalsArg::On => LocalsMode::On,
                    LocalsArg::Off => LocalsMode::Off,
                    LocalsArg::Localhost => LocalsMode::Localhost,
                }),
                timeout_ms: 0,
            }),
        ),
    };
    let mut request = request.map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot encode the request: {e}"),
        )
    })?;
    let ctx = Ctx::new(global)?;
    request["timeout_ms"] = json!(millis(ctx.timeout));
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(method, &request, ctx.timeout + QUICK))
}

/// `FILE:LINE` or `FILE:LINE:COLUMN` (1-based).
fn parse_place(place: &str) -> Result<(String, u32, Option<u32>), ErrorInfo> {
    let bad = || {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{place:?} is not a source line: give FILE:LINE or FILE:LINE:COLUMN"),
        )
        .hint(Cmd::new("debug log").arg("src/cart.ts:42").raw("<expr>…"))
    };
    let number = |text: &str| text.parse::<u32>().ok().filter(|n| *n > 0);
    let (rest, last) = place.rsplit_once(':').ok_or_else(bad)?;
    let last = number(last).ok_or_else(bad)?;
    let (path, line, column) = match rest.rsplit_once(':') {
        Some((path, line)) if number(line).is_some() => (path, number(line), Some(last)),
        _ => (rest, Some(last), None),
    };
    let line = line.ok_or_else(bad)?;
    if path.is_empty() {
        return Err(bad());
    }
    Ok((path.to_owned(), line, column))
}

/// `coverage start` and `coverage stop`: need a running session.
pub(crate) fn coverage(args: &CoverageArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let timeout_ms = millis(ctx.timeout);
    match &args.command {
        CoverageCommand::Start(_) => {
            let request = CoverageStartRequest {
                tab: ctx.tab.clone(),
                timeout_ms,
            };
            ctx.runtime.block_on(client.call_output(
                rpc::COVERAGE_START,
                &request,
                ctx.timeout + QUICK,
            ))
        }
        CoverageCommand::Stop(stop) => {
            let request = CoverageStopRequest {
                limit: stop.limit,
                timeout_ms,
                max_chars: stop.max_chars,
            };
            ctx.runtime.block_on(client.call_output(
                rpc::COVERAGE_STOP,
                &request,
                ctx.timeout + QUICK,
            ))
        }
    }
}

/// `highlight`: needs a running session.
pub(crate) fn highlight(args: &HighlightArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = HighlightRequest {
        tab: ctx.tab.clone(),
        locator: args.locator.clone(),
        duration_ms: millis(args.duration),
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::HIGHLIGHT, &request, ctx.timeout + QUICK))
}

/// `source`: needs a running session.
pub(crate) fn source(args: &SourceArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = SourceRequest {
        tab: ctx.tab.clone(),
        locator: args.locator.clone(),
        depth: args.depth,
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::SOURCE, &request, ctx.timeout + QUICK))
}

/// `components`: needs a running session.
pub(crate) fn components(args: &ComponentsArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = ComponentsRequest {
        tab: ctx.tab.clone(),
        locator: args.locator.clone(),
        depth: args.depth,
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::COMPONENTS, &request, ctx.timeout + QUICK))
}

/// `why`: needs a running session.
pub(crate) fn why(args: &WhyArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = WhyRequest {
        tab: ctx.tab.clone(),
        locator: args.locator.clone(),
        action: args.action.to_engine(),
        no_scroll: args.no_scroll,
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
    };
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::WHY, &request, ctx.timeout + QUICK))
}

/// `layout`'s request, from its flags; `baseline` for `expect --layout`.
fn layout_request(
    locator: Option<&String>,
    flags: &crate::cli::LayoutFlags,
    all: bool,
    strict_refs: bool,
    max_chars: usize,
    ctx: &Ctx,
    global: &GlobalArgs,
) -> LayoutRequest {
    LayoutRequest {
        tab: ctx.tab.clone(),
        locator: locator.cloned(),
        viewports: flags.viewports.clone(),
        only: flags.only.clone(),
        all,
        full: flags.full,
        min_target: flags.min_target,
        stretch_text: flags.stretch_text,
        strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars,
        baseline: None,
    }
}

/// `layout`: needs a running session.
pub(crate) fn layout(args: &LayoutArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = layout_request(
        args.locator.as_ref(),
        &args.flags,
        args.all,
        args.refs.strict_refs,
        args.max_chars,
        &ctx,
        global,
    );
    engine::check_layout(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::LAYOUT, &request, ctx.timeout + QUICK))
}

/// `expect --layout --baseline NAME`: needs a running session.
fn expect_layout(args: &crate::cli::ExpectArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    if let Some(matcher) = &args.matcher {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--layout checks the layout; it takes no matcher, got {matcher:?}"),
        )
        .hint(Hint::run(
            Cmd::new("expect").switch("layout").flag("baseline", "cart"),
        )));
    }
    let name = args.baseline.clone().unwrap_or_default();
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::with_default(global, crate::expect::EXPECT_TIMEOUT)?;
    let mut request = layout_request(
        args.target.as_ref(),
        &args.layout_flags,
        false,
        args.strict_refs,
        args.max_chars,
        &ctx,
        global,
    );
    request.baseline = Some(engine::LayoutBaseline {
        path: crate::expect::layout_baseline(args.dir.as_deref(), &name)?,
        name,
        update: args.update,
        dir: args.dir.as_ref().map(|d| d.display().to_string()),
    });
    engine::check_layout(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::LAYOUT, &request, ctx.timeout + QUICK))
}

/// What `audit` and `expect --audit` send.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn audit_request(
    locator: Option<&String>,
    flags: &crate::cli::AuditFlags,
    min_severity: protocol::schema::Severity,
    show_accepted: bool,
    all_tabs: bool,
    expect: bool,
    strict_refs: bool,
    max_chars: usize,
    ctx: &Ctx,
    global: &GlobalArgs,
) -> Result<engine::AuditRequest, ErrorInfo> {
    let baseline = match &flags.baseline {
        Some(name) => Some(engine::AuditBaseline {
            path: crate::expect::audit_baseline(flags.dir.as_deref(), name)?,
            name: name.clone(),
            update: flags.update,
            create: expect && crate::expect::create_missing(),
            dir: flags.dir.as_ref().map(|d| d.display().to_string()),
        }),
        None => None,
    };
    Ok(engine::AuditRequest {
        tab: ctx.tab.clone(),
        all_tabs,
        locator: locator.cloned(),
        categories: flags.category_names(),
        min_severity,
        baseline,
        show_accepted,
        expect,
        strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars,
    })
}

/// How long an audit may take: each category waits up to the timeout,
/// and `--all-tabs` audits tab after tab.
fn audit_wait(ctx: &Ctx, all_tabs: bool) -> Duration {
    let per_tab = ctx.timeout.saturating_mul(4);
    if all_tabs {
        per_tab.saturating_mul(8) + QUICK
    } else {
        per_tab + QUICK
    }
}

/// `audit`: needs a running session.
pub(crate) fn audit(
    args: &crate::cli::AuditArgs,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = audit_request(
        args.locator.as_ref(),
        &args.flags,
        args.min_severity.to_severity(),
        args.show_accepted,
        args.all_tabs,
        false,
        args.refs.strict_refs,
        args.max_chars,
        &ctx,
        global,
    )?;
    engine::check_audit(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::AUDIT, &request, audit_wait(&ctx, args.all_tabs)))
}

/// `expect --audit`: needs a running session.
fn expect_audit(args: &crate::cli::ExpectArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    // clap turns down a matcher with --audit.
    if let Some(flag) = args.layout_flags.given() {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{flag} is for layout and expect --layout"),
        )
        .hint(Hint::run(Cmd::new("expect").switch("audit"))));
    }
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::with_default(global, crate::expect::EXPECT_TIMEOUT)?;
    let flags = crate::cli::AuditFlags {
        categories: args.categories.clone(),
        baseline: args.baseline.clone(),
        update: args.update,
        dir: args.dir.clone(),
    };
    let request = audit_request(
        args.target.as_ref(),
        &flags,
        args.min_severity.map_or(
            protocol::schema::Severity::Warning,
            crate::cli::SeverityArg::to_severity,
        ),
        false,
        false,
        true,
        args.strict_refs,
        args.max_chars,
        &ctx,
        global,
    )?;
    engine::check_audit(&request)?;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::AUDIT, &request, audit_wait(&ctx, false)))
}

/// `forms`: needs a running session.
pub(crate) fn forms(args: &FormsArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    if let Some(locator) = &args.locator {
        engine::parse_locator(locator)?;
    }
    let request = FormsRequest {
        tab: ctx.tab.clone(),
        locator: args.locator.clone(),
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::FORMS, &request, ctx.timeout + QUICK))
}

/// `fill-form`: needs a running session. Like an action, each field
/// waits up to 10s by default.
pub(crate) fn fill_form(args: &FillFormArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let (text, from) = match (&args.values, &args.file) {
        (Some(values), _) => (input::text_arg(values, "values")?, values.as_str()),
        (None, Some(file)) => (
            std::fs::read_to_string(file).map_err(|e| {
                bad_args(format!("cannot read {}: {e}", file.display())).hint(Hint::text(
                    "--file names a JSON file of field names to values",
                ))
            })?,
            "",
        ),
        (None, None) => return Err(bad_args("fill-form needs --values JSON or --file FILE")),
    };
    let values = form_values(&text)?;
    if from == "-" || args.file.is_some() {
        let inline = values_json(&values);
        daemon::client::edit_history_line(|words| inline_values(words, &inline));
    }
    let ctx = Ctx::with_default(global, WAIT_TIMEOUT)?;
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|d| input::absolute_path(&d).ok());
    let request = FillFormRequest {
        tab: ctx.tab.clone(),
        locator: args.locator.clone(),
        values,
        submit: args.submit,
        cwd,
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        no_wait_after: args.after.no_wait_after,
        snap: args.after.snap(global),
        max_chars: args.after.max_chars,
    };
    engine::check_fill_form(&request)?;
    // Each field may wait its timeout.
    let fields = u32::try_from(request.values.len() + 1).unwrap_or(u32::MAX);
    let wait = ctx.timeout.saturating_mul(fields) + QUICK;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::FILL_FORM, &request, wait))
}

/// `fill-form`'s values: a JSON object, its keys in the order given.
fn form_values(text: &str) -> Result<Vec<(String, serde_json::Value)>, ErrorInfo> {
    struct Ordered(Vec<(String, serde_json::Value)>);
    impl<'de> serde::Deserialize<'de> for Ordered {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Visit;
            impl<'de> serde::de::Visitor<'de> for Visit {
                type Value = Ordered;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a JSON object of field names to values")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Ordered, A::Error> {
                    let mut out: Vec<(String, serde_json::Value)> = Vec::new();
                    while let Some((key, value)) = map.next_entry()? {
                        out.retain(|(k, _)| *k != key);
                        out.push((key, value));
                    }
                    Ok(Ordered(out))
                }
            }
            d.deserialize_map(Visit)
        }
    }
    serde_json::from_str::<Ordered>(text)
        .map(|o| o.0)
        .map_err(|e| {
            bad_args(format!(
                "the values are not a JSON object of field names to values: {e}"
            ))
            .hint(Hint::run(Cmd::new("fill-form").flag(
                "values",
                r#"{"Email": "ada@example.com", "Remember me": true}"#,
            )))
        })
}

/// The values as one line of JSON, keys in their order.
fn values_json(values: &[(String, serde_json::Value)]) -> String {
    let pairs: Vec<String> = values
        .iter()
        .map(|(k, v)| format!("{}:{v}", serde_json::Value::String(k.clone())))
        .collect();
    format!("{{{}}}", pairs.join(","))
}

/// `words` with `--file F` and `--values -` replaced by `--values
/// <inline>`.
fn inline_values(words: &mut Vec<String>, inline: &str) {
    let mut out = Vec::with_capacity(words.len());
    let mut iter = std::mem::take(words).into_iter();
    while let Some(word) = iter.next() {
        match word.as_str() {
            "--file" | "--values" => {
                iter.next();
            }
            w if w.starts_with("--file=") || w.starts_with("--values=") => {}
            _ => {
                out.push(word);
                continue;
            }
        }
        out.push("--values".to_owned());
        out.push(inline.to_owned());
    }
    *words = out;
}

/// `compare A B`: needs a running session; a `session:NAME` side needs
/// that session running too.
pub(crate) fn compare(args: &CompareArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    if let Some(scope) = &args.scope {
        engine::parse_locator(scope).map_err(|e| {
            let message = format!("--scope: {}", e.message);
            ErrorInfo { message, ..e }
        })?;
    }
    let sides = [compare_side(&args.a)?, compare_side(&args.b)?];
    let mut request = CompareRequest {
        a: CompareSide::Tab(String::new()),
        b: CompareSide::Tab(String::new()),
        interactive: args.interactive,
        scope: args.scope.clone(),
        visual: args.visual,
        threshold: args.threshold,
        viewport: args.viewport,
        base_url: ctx.base_url.clone(),
        strict_refs: args.refs.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    let mut resolved = Vec::with_capacity(2);
    for side in &sides {
        resolved.push(match side {
            Given::Side(side) => side.clone(),
            Given::File(path) => CompareSide::Captured(snapshot_file(path, args.scope.as_deref())?),
            Given::Session(name) => {
                CompareSide::Captured(session_capture(&ctx, name, &request, global)?)
            }
        });
    }
    let mut resolved = resolved.into_iter();
    request.a = resolved.next().unwrap_or(CompareSide::Tab(String::new()));
    request.b = resolved.next().unwrap_or(CompareSide::Tab(String::new()));
    engine::check_compare(&request)?;
    // Each URL side may load for its timeout.
    let wait = ctx.timeout.saturating_mul(3) + QUICK;
    let mut client = running_session(&ctx)?;
    let mut output = ctx
        .runtime
        .block_on(client.call_output(rpc::COMPARE, &request, wait))?;
    if let Given::Session(name) = &sides[1] {
        name_regions(&ctx, name, &mut output);
    }
    Ok(output)
}

pub(crate) fn explore(args: &ExploreArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::with_default(global, WAIT_TIMEOUT)?;
    let dir = args
        .output
        .as_deref()
        .map(|dir| input::absolute_path(dir).map(PathBuf::from))
        .transpose()?;
    let request = ExploreRequest {
        url: args.url.clone(),
        base_url: ctx.base_url.clone(),
        max_states: args.max_states,
        max_actions: args.max_actions,
        depth: args.depth,
        time_ms: millis(args.time),
        click_buttons: args.click_buttons,
        submit_forms: args.submit_forms,
        allow_destructive: args.allow_destructive,
        origins: args.origins.clone(),
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        layout: args.layout,
        screenshots: args.screenshots,
        dir,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    engine::check_explore(&request)?;
    // The walk stops at --time; its last action, a replay and closing
    // the context may run on.
    let wait = args.time + ctx.timeout.saturating_mul(4) + QUICK;
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::EXPLORE, &request, wait))
}

/// A side of `compare` as typed.
enum Given {
    Side(CompareSide),
    File(PathBuf),
    Session(String),
}

/// `tab:<id>`, `session:<name>`, a `.json` file, else a URL.
fn compare_side(text: &str) -> Result<Given, ErrorInfo> {
    if let Some(tab) = text.strip_prefix("tab:") {
        let id = engine::parse_tab_id(tab)?;
        return Ok(Given::Side(CompareSide::Tab(format!("t{id}"))));
    }
    if let Some(name) = text.strip_prefix("session:") {
        check_name(name)?;
        return Ok(Given::Session(name.to_owned()));
    }
    let path = Path::new(text);
    if !text.contains("://")
        && (path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            || path.is_file())
    {
        return Ok(Given::File(input::absolute_path(path)?.into()));
    }
    Ok(Given::Side(CompareSide::Url(text.to_owned())))
}

/// A `snapshot --json` file as a side: the envelope, or its `data`.
fn snapshot_file(path: &Path, scope: Option<&str>) -> Result<CompareCapture, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct Snapshot {
        #[serde(default)]
        url: String,
        #[serde(default)]
        title: String,
        nodes: Vec<protocol::schema::SnapshotNode>,
    }
    let hint = || {
        Hint::text(format!(
            "write one with: chrome-cdp snapshot --json > {}",
            path.display()
        ))
    };
    let text = std::fs::read_to_string(path)
        .map_err(|e| bad_args(format!("cannot read {}: {e}", path.display())).hint(hint()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| bad_args(format!("{} is not JSON: {e}", path.display())).hint(hint()))?;
    let data = value.get("data").cloned().unwrap_or(value);
    let snapshot: Snapshot = serde_json::from_value(data).map_err(|e| {
        bad_args(format!(
            "{} is not what snapshot --json writes: {e}",
            path.display()
        ))
        .hint(hint())
    })?;
    let side = path.display().to_string();
    let warnings = scope
        .map(|_| {
            format!("--scope does not apply to {side}, a snapshot file: all of it is compared")
        })
        .into_iter()
        .collect();
    Ok(CompareCapture {
        side,
        url: snapshot.url,
        title: snapshot.title,
        nodes: snapshot.nodes,
        shot: None,
        warnings,
    })
}

/// A connection to session `name`'s daemon, which must be running.
fn other_session(ctx: &Ctx, name: &str) -> Result<daemon::client::Client, ErrorInfo> {
    let paths = SessionPaths::new(&ctx.env, name);
    ctx.runtime
        .block_on(client::connect(&paths, &ctx.build))?
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::DaemonUnavailable,
                format!("session {name} is not running"),
            )
            .hint(Hint::run(Cmd::new("open").session(name)))
        })
}

/// The active tab of session `name`, read as `request` asks.
fn session_capture(
    ctx: &Ctx,
    name: &str,
    request: &CompareRequest,
    global: &GlobalArgs,
) -> Result<CompareCapture, ErrorInfo> {
    let side = format!("session:{name}");
    let capture = CompareCaptureRequest {
        side: side.clone(),
        interactive: request.interactive,
        scope: request.scope.clone(),
        visual: request.visual,
        strict_refs: request.strict_refs,
        testid_attribute: global.testid_attribute.clone(),
        timeout_ms: request.timeout_ms,
    };
    let mut client = other_session(ctx, name)?;
    let output = ctx.runtime.block_on(client.call_output(
        rpc::COMPARE_CAPTURE,
        &capture,
        ctx.timeout + QUICK,
    ))?;
    serde_json::from_value(output.data).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::DaemonUnavailable,
            format!("session {name} sent a bad page for {side}: {e}"),
        )
    })
}

/// Names `compare --visual`'s regions on B, the active tab of session
/// `name`, and renders the output again. Left unnamed if that fails.
fn name_regions(ctx: &Ctx, name: &str, output: &mut Output) {
    let Ok(mut data) = serde_json::from_value::<CompareData>(output.data.clone()) else {
        return;
    };
    let Some(visual) = data.visual.as_mut() else {
        return;
    };
    if visual.regions.is_empty() {
        return;
    }
    let points: Vec<(f64, f64)> = visual
        .regions
        .iter()
        .map(|r| {
            (
                f64::from(r.x) + f64::from(r.width) / 2.0,
                f64::from(r.y) + f64::from(r.height) / 2.0,
            )
        })
        .collect();
    let request = CompareNameRequest {
        points,
        timeout_ms: millis(ctx.timeout),
    };
    let Ok(mut client) = other_session(ctx, name) else {
        return;
    };
    let Ok(named) =
        ctx.runtime
            .block_on(client.call_output(rpc::COMPARE_NAME, &request, ctx.timeout + QUICK))
    else {
        return;
    };
    let elements: Vec<Option<protocol::schema::WhyElement>> =
        serde_json::from_value(named.data["elements"].clone()).unwrap_or_default();
    for (region, element) in visual.regions.iter_mut().zip(elements) {
        region.element = element;
    }
    output.text = engine::compare_text(&data);
    output.data = serde_json::to_value(&data).unwrap_or(Value::Null);
}

/// `inspect`: needs a running session.
pub(crate) fn inspect(args: &InspectArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let request = InspectRequest {
        tab: ctx.tab.clone(),
        panel: args.panel.map(|p| p.id().to_owned()),
        timeout_ms: millis(ctx.timeout),
    };
    let mut client = running_session(&ctx)?;
    ctx.runtime
        .block_on(client.call_output(rpc::INSPECT, &request, ctx.timeout + QUICK))
}

/// `-o` as an absolute path, and whether it is `-` (stdout; then the
/// daemon saves to its own directory and [`to_stdout`] moves the file).
fn output_path(
    output: Option<&Path>,
    global: &GlobalArgs,
    command: &str,
) -> Result<(Option<PathBuf>, bool), ErrorInfo> {
    match output {
        None => Ok((None, false)),
        Some(path) if path.as_os_str() == "-" => {
            if global.json {
                return Err(
                    bad_args("-o - writes the file to stdout, where --json cannot go too")
                        .hint(Hint::run(Cmd::new(command).switch("json"))),
                );
            }
            Ok((None, true))
        }
        Some(path) => Ok((Some(PathBuf::from(input::absolute_path(path)?)), false)),
    }
}

/// Writes the file `output` names to stdout and deletes it; the command
/// then prints nothing else there (warnings stay on stderr).
fn to_stdout(output: Output) -> Result<Output, ErrorInfo> {
    let path = output
        .data
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let bytes = std::fs::read(&path).map_err(|e| bad_args(format!("cannot read {path}: {e}")))?;
    let _ = std::fs::remove_file(&path);
    let mut out = std::io::stdout().lock();
    // A closed stdout (`| head`) is not an error worth reporting.
    let _ = out.write_all(&bytes).and_then(|()| out.flush());
    Ok(Output {
        text: String::new(),
        quiet: Some(String::new()),
        limit: None,
        ..output
    })
}

/// How long one `--follow` call waits for new entries before asking
/// again.
const FOLLOW_POLL: Duration = Duration::from_secs(25);

/// `console` and `errors` (`errors`): need a running session. With
/// `--follow`, prints each batch of new entries as it comes and returns
/// the last (empty) one.
pub(crate) fn console(
    read: &LogReadArgs,
    errors: bool,
    level: protocol::schema::Level,
    global: &GlobalArgs,
) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let mut request = ConsoleRequest {
        errors,
        tab: ctx.tab.clone(),
        level: if errors {
            protocol::schema::Level::Error
        } else {
            level
        },
        since: read.since.clone(),
        grep: read.grep.clone(),
        limit: read.limit,
        wait_ms: 0,
        sourcemaps: read.sourcemaps,
        locals: read.locals,
        timeout_ms: millis(ctx.timeout),
        max_chars: read.max_chars,
    };
    if !read.follow {
        return ctx.runtime.block_on(client.call_output(
            rpc::CONSOLE,
            &request,
            ctx.timeout + QUICK,
        ));
    }
    let what = if errors { "errors" } else { "console" };
    follow(
        global,
        what,
        "entries",
        |_| false,
        |wait, first| {
            request.wait_ms = millis(wait);
            if !first {
                // From now on, what is new since the last batch.
                request.since = engine::Since::Last;
            }
            ctx.runtime.block_on(client.call_output(
                rpc::CONSOLE,
                &request,
                wait + ctx.timeout + QUICK,
            ))
        },
    )
}

/// `issues`: needs a running session. With `--follow`, prints each
/// batch of new issues as it comes and returns the last (empty) one.
pub(crate) fn issues(args: &IssuesArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    check_tab(global.tab.as_deref())?;
    let ctx = Ctx::new(global)?;
    let mut client = running_session(&ctx)?;
    let mut request = IssuesRequest {
        tab: ctx.tab.clone(),
        all_tabs: args.all_tabs,
        since: args.since.clone(),
        grep: args.grep.clone(),
        limit: args.limit,
        min_severity: args.min_severity.to_severity(),
        wait_ms: 0,
        timeout_ms: millis(ctx.timeout),
        max_chars: args.max_chars,
    };
    if !args.follow {
        return ctx.runtime.block_on(client.call_output(
            rpc::ISSUES,
            &request,
            ctx.timeout + QUICK,
        ));
    }
    follow(
        global,
        "issues",
        "issues",
        |_| false,
        |wait, first| {
            request.wait_ms = millis(wait);
            if !first {
                request.since = engine::Since::Last;
            }
            ctx.runtime.block_on(client.call_output(
                rpc::ISSUES,
                &request,
                wait + ctx.timeout + QUICK,
            ))
        },
    )
}

/// `--follow`: calls `call` with how long the daemon may wait for
/// something new (and whether it is the first call) until Ctrl-C, or
/// `--timeout` if given, or a batch that is `done`, printing each batch
/// whose `data[key]` list is not empty; returns the last, empty (or done)
/// one. `what` names what is followed.
fn follow(
    global: &GlobalArgs,
    what: &str,
    key: &str,
    done: impl Fn(&Output) -> bool,
    mut call: impl FnMut(Duration, bool) -> Result<Output, ErrorInfo>,
) -> Result<Output, ErrorInfo> {
    let deadline = global.timeout.map(|t| std::time::Instant::now() + t);
    let mode = global.mode();
    let mut first = true;
    loop {
        let left = deadline.map(|d| d.saturating_duration_since(std::time::Instant::now()));
        let wait = if first {
            Duration::ZERO
        } else {
            left.unwrap_or(FOLLOW_POLL).min(FOLLOW_POLL)
        };
        let mut output = call(wait, first)?;
        let shown = output.data[key].as_array().is_some_and(|e| !e.is_empty());
        if first && mode == Mode::Text {
            let tab = output.data["tab"].as_str().unwrap_or("the tab");
            crate::output::stderr(&format!("following {tab}'s {what}; Ctrl-C stops\n"));
        }
        first = false;
        if done(&output) {
            return Ok(output);
        }
        if left.is_some_and(|l| l.is_zero()) {
            if !shown {
                output.text.clear();
                output.quiet = Some(String::new());
            }
            return Ok(output);
        }
        if shown {
            crate::output::print(mode, Ok(output));
        }
    }
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
    needs(
        "--frame",
        args.frame.is_some() && args.ws.is_none(),
        "--ws",
        Cmd::new("wait").flag("ws", "/live").flag("frame", "price"),
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
    } else if let Some(pattern) = &args.console {
        WaitFor::Console {
            pattern: pattern.clone(),
        }
    } else if let Some(name) = &args.binding {
        WaitFor::Binding { name: name.clone() }
    } else if let Some(pattern) = &args.ws {
        WaitFor::WebSocket {
            pattern: pattern.clone(),
            frame: args.frame.clone(),
        }
    } else if args.hmr {
        WaitFor::Hmr
    } else {
        return Err(
            bad_args("wait needs a condition").hint(Cmd::new("wait").switch("help").to_string())
        );
    })
}

/// `wait 500` or `wait 2s`: a delay, not a locator.
pub(crate) fn delay_ms(target: &str) -> Option<u64> {
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
        .and_then(|v| crate::cli::parse_duration_or_zero(&v).ok())
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
            node_stacks: previous.node_stacks.or(ctx.node_stacks),
            replaced_daemon: session
                .replaced
                .as_ref()
                .is_some_and(|m| m.browser.is_some()),
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
        return Ok(());
    }
    // `page.navigate`, or `navigate` when only one domain has it.
    let methods = cdp::domains::names::METHODS;
    let named: Vec<&str> = methods
        .iter()
        .copied()
        .filter(|m| m.split_once('.').is_some_and(|(_, name)| name == method))
        .collect();
    let meant = methods
        .iter()
        .copied()
        .find(|m| m.eq_ignore_ascii_case(method))
        .or(if named.len() == 1 {
            named.first().copied()
        } else {
            None
        })
        .unwrap_or("Browser.getVersion");
    Err(bad_args(format!("`{method}` is not a CDP method name"))
        .hint(Hint::run(Cmd::new("cdp").arg(meant)).why("methods are Domain.method")))
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

    use super::{
        check_method, delay_ms, init_script_request, keep_previous_options, parse_params,
        parse_place,
    };

    #[test]
    fn source_lines() {
        let place = |text: &str| parse_place(text).map_err(|e| e.kind);
        assert_eq!(
            place("src/cart.ts:42"),
            Ok(("src/cart.ts".into(), 42, None))
        );
        assert_eq!(
            place("src/cart.ts:42:7"),
            Ok(("src/cart.ts".into(), 42, Some(7)))
        );
        assert_eq!(
            place("http://localhost:5173/src/cart.ts:9"),
            Ok(("http://localhost:5173/src/cart.ts".into(), 9, None))
        );
        for bad in [
            "src/cart.ts",
            "src/cart.ts:",
            "src/cart.ts:x",
            ":4",
            "a.ts:0",
        ] {
            assert_eq!(place(bad), Err(ErrorKind::BadArgs), "{bad}");
        }
    }

    #[test]
    fn init_scripts_are_read_from_files_or_code() {
        use clap::Parser as _;
        let request = |args: &[&str]| {
            let cli = crate::cli::Cli::try_parse_from(
                ["chrome-cdp", "init-script", "add"].iter().chain(args),
            )
            .unwrap();
            let Some(crate::cli::Command::InitScript(script)) = cli.command else {
                panic!("not init-script");
            };
            let crate::cli::InitScriptCommand::Add(add) = script.command else {
                panic!("not add");
            };
            init_script_request(&add)
        };
        let code = request(&["--code", "window.a = 1", "--world", "isolated"]).unwrap();
        assert_eq!(code.source, "window.a = 1");
        assert_eq!(code.file, None);
        assert_eq!(code.world, engine::EvalWorld::Isolated);

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stub.js");
        std::fs::write(&file, "window.stub = true;\n").unwrap();
        let from_file = request(&[file.to_str().unwrap(), "--now"]).unwrap();
        assert_eq!(from_file.source, "window.stub = true;\n");
        assert_eq!(from_file.file.as_deref(), Some(file.as_path()));
        assert!(from_file.now);

        let missing = dir.path().join("missing.js");
        let err = request(&[missing.to_str().unwrap()]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs);
        assert!(err.message.contains("missing.js"), "{}", err.message);
    }

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
        // The hint names the method meant, when it is clear.
        let hint = |bad: &str| check_method(bad).unwrap_err().hint.unwrap_or_default();
        assert!(hint("page.navigate").starts_with("chrome-cdp cdp Page.navigate "));
        assert!(hint("getFullAXTree").starts_with("chrome-cdp cdp Accessibility.getFullAXTree "));
        // `enable` is in most domains, `captureSnapshot` in two.
        assert!(hint("enable").starts_with("chrome-cdp cdp Browser.getVersion "));
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
            downloads_dir: Some("/tmp/dl".into()),
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
        assert_eq!(request.downloads_dir, previous.downloads_dir);
    }
}
