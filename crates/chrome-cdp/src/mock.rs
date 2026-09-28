//! `mock use|off|list|log|reset|explain` (plan 11, milestone 11.2),
//! `mock state|seed` (milestone 11.3) and `mock record|import|check`
//! (milestone 11.8); `mock serve` and `mock ws send` (milestone 11.9).
//!
//! The CLI finds the project (the mocks directory, `default_mocks`, the
//! random seed) and the sets' files; the daemon reads and applies them.
//! `mock explain` and `mock list --available` also work without a session:
//! they read the files here.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use daemon::rpc;
use e2e::mock_server::{MockServer, Served};
use engine::{
    HarExportRequest, MockExplainRequest, MockListRequest, MockLogRequest, MockOffRequest,
    MockSeedRequest, MockStateRequest, MockTestTag, MockUseRequest, MockWsSendRequest,
};
use mock::{MockSet, Mocks, Source};
use protocol::schema::{
    MockCheckData, MockExplainData, MockProblem, MockRecordData, MockServeData, MockSetsData,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Mode, Output};
use serde_json::{Value, json};

use crate::cli::{
    GlobalArgs, MockArgs, MockCheckArgs, MockCommand, MockExplainArgs, MockImportArgs,
    MockRecordArgs, MockSeedArgs, MockServeArgs, MockStateArgs, MockUseArgs, MockWsCommand,
    MockWsSendArgs,
};
use crate::{input, output, route, session};

/// What the project says about mocks.
struct Project {
    /// The mocks directory (absolute).
    dir: PathBuf,
    /// `default_mocks`.
    defaults: Vec<String>,
    /// `[e2e.determinism] random_seed`, else 0.
    seed: u64,
    /// `openapi` of [e2e] (absolute).
    openapi: Option<PathBuf>,
    cwd: PathBuf,
}

impl Project {
    /// The project of the current directory; `dir` (`--dir`) wins over
    /// its mocks directory. Without a project config the mocks directory
    /// is ./e2e/mocks.
    fn find(dir: Option<&Path>) -> Result<Self, ErrorInfo> {
        let cwd = std::env::current_dir().map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read the current directory: {e}"),
            )
        })?;
        let config = match e2e::find_project_file(&cwd)? {
            None => None,
            Some(file) => {
                let text = std::fs::read_to_string(&file).map_err(|e| {
                    ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("cannot read {}: {e}", file.display()),
                    )
                })?;
                e2e::Config::parse(&text, &file)?
            }
        };
        let seed = match config.as_ref().and_then(|c| c.determinism.random_seed) {
            Some(e2e::config::RandomSeed::Seed(seed)) => seed,
            _ => 0,
        };
        let dir = match (dir, &config) {
            (Some(dir), _) => cwd.join(dir),
            (None, Some(config)) => config.mocks.clone(),
            (None, None) => cwd.join(e2e::config::DEFAULT_MOCKS),
        };
        Ok(Self {
            dir,
            openapi: config.as_ref().and_then(|c| c.openapi.clone()),
            defaults: config.map(|c| c.default_mocks).unwrap_or_default(),
            seed,
            cwd,
        })
    }

    /// The project of a `test` run's config.
    fn of(config: &e2e::Config, cwd: PathBuf) -> Self {
        let seed = match config.determinism.random_seed {
            Some(e2e::config::RandomSeed::Seed(seed)) => seed,
            _ => 0,
        };
        Self {
            dir: config.mocks.clone(),
            defaults: config.default_mocks.clone(),
            seed,
            openapi: config.openapi.clone(),
            cwd,
        }
    }

    /// The sources of sets `names`.
    fn sources(&self, names: &[String]) -> Result<Vec<Source>, ErrorInfo> {
        names
            .iter()
            .map(|n| Source::find(&self.dir, &self.cwd, n))
            .collect()
    }

    /// The error for "no set named and no defaults".
    fn nothing(&self, command: Cmd) -> ErrorInfo {
        let sets = mock::available(&self.dir);
        let info = ErrorInfo::new(
            ErrorKind::BadArgs,
            "no mock set named, and the project has no default_mocks",
        );
        match sets.first() {
            Some(first) => info
                .note("sets", sets.join(", "))
                .hint(Hint::run(command.arg(first))),
            None => info.hint(format!(
                "write a mock file in {} (see `chrome-cdp mock --help`), or run `chrome-cdp init e2e`",
                self.dir.display()
            )),
        }
    }
}

pub(crate) fn run(args: &MockArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let timeout_ms = session::timeout_ms(global);
    match &args.command {
        MockCommand::Use(args) => mock_use(args, global),
        MockCommand::Off(args) => {
            let request = MockOffRequest {
                sets: args.sets.clone(),
                timeout_ms,
            };
            session::call_session(global, rpc::MOCK_OFF, &request, "mock sets")
        }
        MockCommand::List(args) if args.available => available(args.dir.as_deref()),
        MockCommand::List(args) => {
            let request = MockListRequest {
                max_chars: args.max_chars,
            };
            session::call_session(global, rpc::MOCK_LIST, &request, "mock sets")
        }
        MockCommand::Log(args) => {
            let request = MockLogRequest {
                unmatched: args.unmatched,
                all: args.all,
                limit: args.limit,
                max_chars: args.max_chars,
            };
            session::call_session(global, rpc::MOCK_LOG, &request, "mock sets")
        }
        MockCommand::Reset(_) => {
            session::call_session(global, rpc::MOCK_RESET, &json!({}), "mock sets")
        }
        MockCommand::Explain(args) => explain(args, global),
        MockCommand::State(args) => state(args, global),
        MockCommand::Seed(args) => seed(args, global),
        MockCommand::Record(args) => record(args, global),
        MockCommand::Import(args) => import(args),
        MockCommand::Check(args) => check(args),
        MockCommand::Serve(args) => serve(args, global),
        MockCommand::Ws(MockWsCommand::Send(args)) => ws_send(args, global),
    }
}

/// `mock serve`: the sets as an HTTP server, until Ctrl-C.
fn serve(args: &MockServeArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let project = Project::find(args.dir.as_deref())?;
    let mut names: Vec<String> = Vec::new();
    if !args.no_defaults {
        names.extend(project.defaults.iter().cloned());
    }
    for name in &args.sets {
        names.retain(|n| n != name);
        names.push(name.clone());
    }
    if names.is_empty() {
        return Err(project.nothing(Cmd::new("mock").arg("serve")));
    }
    let sets = project
        .sources(&names)
        .and_then(|sources| {
            sources
                .iter()
                .map(MockSet::load)
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|mut info| {
            // The did-you-mean hints name `mock use`.
            info.hint = info
                .hint
                .map(|h| h.replacen("chrome-cdp mock use", "chrome-cdp mock serve", 1));
            info
        })?;
    let ip: std::net::IpAddr = args.host.parse().map_err(|_| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--host: `{}` is not an IP address", args.host),
        )
        .hint(Hint::text(
            "--host 127.0.0.1 (this machine only) or --host 0.0.0.0 (every interface)",
        ))
    })?;
    let addr = std::net::SocketAddr::new(ip, args.port);
    let mode = global.mode();
    let answered = Arc::new(AtomicU64::new(0));
    let count = answered.clone();
    let on_request = move |served: &Served| {
        count.fetch_add(1, Ordering::SeqCst);
        match mode {
            Mode::Json => {
                let line = serde_json::to_string(served).unwrap_or_default();
                output::stdout(&format!("{line}\n"));
            }
            Mode::Text => {
                let mut text = mock::text::log_line(&served.request);
                if let Some(test) = &served.test {
                    let _ = write!(text, " [test {test}]");
                }
                for check in &served.request.closest {
                    let _ = write!(
                        text,
                        "\n  closest mock: {}",
                        mock::text::closest_line(check)
                    );
                }
                output::stdout(&format!("{text}\n"));
            }
            Mode::Quiet => {}
        }
    };
    let server =
        MockServer::start(addr, sets, project.seed, Some(Box::new(on_request))).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("mock serve cannot listen on {addr}: {e}"),
            )
            .hint(
                Hint::run(Cmd::new("mock").arg("serve").flag("port", "0")).why("picks a free port"),
            )
        })?;
    let url = server.url();
    // The sets in the order they are tried.
    let tried: Vec<String> = names.iter().rev().cloned().collect();
    match mode {
        Mode::Json => output::stdout(&format!("{}\n", json!({ "listening": url, "sets": tried }))),
        Mode::Text => output::stdout(&format!(
            "mock server on {url}: {} (Ctrl-C stops it)\n",
            tried.join(", ")
        )),
        Mode::Quiet => output::stdout(&format!("{url}\n")),
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ErrorInfo::new(ErrorKind::LaunchFailed, format!("no runtime: {e}")))?;
    runtime.block_on(async {
        let _ = tokio::signal::ctrl_c().await;
    });
    server.stop();
    let requests = answered.load(Ordering::SeqCst);
    let text = format!(
        "mock server stopped after {requests} {}",
        if requests == 1 { "request" } else { "requests" }
    );
    let data = MockServeData {
        url,
        sets: tried,
        requests,
    };
    Ok(Output::typed(&data, text).quiet(String::new()))
}

/// `mock check`.
fn check(args: &MockCheckArgs) -> Result<Output, ErrorInfo> {
    let project = Project::find(args.dir.as_deref())?;
    let names = if args.sets.is_empty() {
        let all = mock::available(&project.dir);
        if all.is_empty() {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("no mock files in {}", project.dir.display()),
            )
            .hint(
                "write one (see `chrome-cdp mock --help`), or run `chrome-cdp init e2e`; --dir \
                 names another directory",
            ));
        }
        all
    } else {
        args.sets.clone()
    };
    let openapi = args
        .openapi
        .as_ref()
        .map(|p| project.cwd.join(p))
        .or_else(|| project.openapi.clone());
    let data = check_sets(&project, &names, openapi.as_deref())?;
    let text = check_text(&data);
    if data.errors > 0 {
        return Err(check_failed(
            &data,
            &Cmd::new("mock").arg("check").to_string(),
        ));
    }
    let quiet = format!("{} {}", data.errors, data.warnings);
    Ok(Output::typed(&data, text)
        .quiet(quiet)
        .limit(args.max_chars, "--max-chars 0 for all of it, or --json"))
}

/// `mock check` at the start of `test`: the sets the run uses (the
/// defaults and every test's `@mocks`). Errors stop the run; the
/// warnings come back to be shown.
pub(crate) fn check_for_run(
    config: &e2e::Config,
    names: &[String],
) -> Result<Vec<String>, ErrorInfo> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| config.root.clone());
    let project = Project::of(config, cwd);
    let data = check_sets(&project, names, config.openapi.as_deref())?;
    if data.errors > 0 {
        let mut cmd = Cmd::new("mock").arg("check");
        for name in names {
            cmd = cmd.arg(name);
        }
        return Err(check_failed(&data, &cmd.to_string()));
    }
    Ok(data.problems.iter().map(problem_line).collect())
}

/// `mock check` of mock files `files` (absolute) for `lint`, against
/// `openapi` (else the config's).
pub(crate) fn check_files(
    config: &e2e::Config,
    files: &[PathBuf],
    openapi: Option<&Path>,
) -> Result<MockCheckData, ErrorInfo> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| config.root.clone());
    let project = Project::of(config, cwd);
    let names: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
    let openapi = openapi
        .map(Path::to_path_buf)
        .or_else(|| project.openapi.clone());
    check_sets(&project, &names, openapi.as_deref())
}

/// Runs `mock check` on sets `names` of `project`, against `openapi`.
fn check_sets(
    project: &Project,
    names: &[String],
    openapi: Option<&Path>,
) -> Result<MockCheckData, ErrorInfo> {
    let spec = openapi
        .map(|file| {
            mock::openapi::OpenApi::load(file).map_err(|e| {
                ErrorInfo::new(ErrorKind::BadArgs, format!("openapi: {e}")).hint(Hint::text(
                    "give an OpenAPI 3 (or Swagger 2) spec, JSON or YAML",
                ))
            })
        })
        .transpose()?;
    let sources = project.sources(names)?;
    let checked = mock::check::check(&sources, &project.defaults, spec.as_ref());
    let errors = checked
        .problems
        .iter()
        .filter(|p| p.severity == "error")
        .count();
    Ok(MockCheckData {
        files: sources
            .iter()
            .map(|s| s.file.display().to_string())
            .collect(),
        mocks: checked.mocks,
        openapi: openapi.map(|p| p.display().to_string()),
        errors,
        warnings: checked.problems.len() - errors,
        problems: checked.problems,
    })
}

/// One problem as a line: `file:12:1: error [rule] message`.
fn problem_line(p: &MockProblem) -> String {
    let at = if p.line == 0 {
        p.file.clone()
    } else {
        format!("{}:{}:{}", p.file, p.line, p.col)
    };
    format!("{at}: {} [{}] {}", p.severity, p.rule, p.message)
}

fn check_summary(data: &MockCheckData) -> String {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut text = format!(
        "{}, {}: {}, {}",
        plural(data.files.len(), "mock file", "mock files"),
        plural(data.mocks, "mock", "mocks"),
        plural(data.errors, "error", "errors"),
        plural(data.warnings, "warning", "warnings"),
    );
    if let Some(spec) = &data.openapi {
        let _ = write!(text, " (checked against {spec})");
    }
    text
}

fn check_text(data: &MockCheckData) -> String {
    let mut text = String::new();
    for p in &data.problems {
        let _ = writeln!(text, "{}", problem_line(p));
        if let Some(hint) = &p.hint {
            let _ = writeln!(text, "  hint: {hint}");
        }
    }
    text.push_str(&check_summary(data));
    text
}

/// The error for a check with errors: the first one, the rest as notes.
pub(crate) fn check_failed(data: &MockCheckData, command: &str) -> ErrorInfo {
    const LISTED: usize = 10;
    let mut problems = data.problems.iter();
    let first = problems.next().map(problem_line).unwrap_or_default();
    let mut info = ErrorInfo::new(ErrorKind::ExpectFailed, first);
    if let Some(hint) = data.problems.first().and_then(|p| p.hint.clone()) {
        info = info.note("fix", hint);
    }
    let rest: Vec<&MockProblem> = problems.collect();
    for p in rest.iter().take(LISTED) {
        info = info.note("also", problem_line(p));
    }
    if rest.len() > LISTED {
        info = info.note("also", format!("{} more (see --json)", rest.len() - LISTED));
    }
    info.note("checked", check_summary(data))
        .hint(Hint::text(format!(
            "fix the errors in the mock files (or the spec), then run {command} again"
        )))
        .details(serde_json::to_value(data).unwrap_or(Value::Null))
}

/// `mock record`: the session's requests in scope, as a mock set.
fn record(args: &MockRecordArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let project = Project::find(args.dir.as_deref())?;
    let out = Target::new(&project, args.output.as_deref(), "recorded", args.force)?;
    let request = HarExportRequest {
        tab: global.tab.clone(),
        scope: args.scope.clone(),
        timeout_ms: session::timeout_ms(global),
    };
    mock::record::Scope::new(&args.scope).map_err(|e| ErrorInfo::new(ErrorKind::BadArgs, e))?;
    let exported = session::call_running(global, rpc::HAR_EXPORT, &request)?;
    let exchanges = mock::record::har_exchanges(&exported.data["har"])
        .map_err(|e| ErrorInfo::new(ErrorKind::ProtocolError, format!("bad export: {e}")))?;
    let origin = match args.scope.as_slice() {
        [] => "Recorded by `chrome-cdp mock record` from the session's fetch and XHR requests."
            .to_owned(),
        scope => format!(
            "Recorded by `chrome-cdp mock record --scope {}`.",
            scope.join(",")
        ),
    };
    let mut output = write_recording(
        &out,
        &exchanges,
        &args.scope,
        origin,
        args.max_chars,
        || {
            Hint::run(Cmd::new("network").raw("--limit").arg("50"))
                .why("lists what the session sent; browse the app first, then record")
        },
    )?;
    let count = |key: &str| exported.data[key].as_u64().unwrap_or(0);
    if count("missing_bodies") > 0 {
        output = output.warning(format!(
            "{} response {} not recorded: Chrome keeps bodies only while the page that loaded              them is open; the mocks say where",
            count("missing_bodies"),
            if count("missing_bodies") == 1 { "body was" } else { "bodies were" }
        ));
    }
    if count("dropped") > 0 {
        output = output.warning(format!(
            "{} older requests had left the network log (open --network-buffer N keeps more)",
            count("dropped")
        ));
    }
    Ok(output)
}

/// `test --record-mocks`: a test's exchanges as mock set `name` of the
/// project's mocks directory, replacing an older one. `None` when the
/// test sent no request to record.
pub(crate) fn record_for_test(
    config: &e2e::Config,
    name: &str,
    scope: &[String],
    exchanges: &[mock::record::Exchange],
    origin: String,
) -> Result<Option<MockRecordData>, ErrorInfo> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| config.root.clone());
    let project = Project::of(config, cwd);
    let file = project.dir.join(format!("{name}.toml"));
    let out = Target::new(&project, Some(&file), name, true)?;
    match write_recording(&out, exchanges, scope, origin, 0, || {
        Hint::text("the test sent no fetch or XHR request")
    }) {
        Ok(output) => Ok(serde_json::from_value(output.data).ok()),
        Err(info) if info.kind == ErrorKind::NotFound => Ok(None),
        Err(info) => Err(info),
    }
}

/// `mock import <file.har>`.
fn import(args: &MockImportArgs) -> Result<Output, ErrorInfo> {
    let project = Project::find(args.dir.as_deref())?;
    let (text, stem) = if args.file == "-" {
        (input::text_arg("-", "the HAR file")?, "imported".to_owned())
    } else {
        let path = project.cwd.join(&args.file);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read {}: {e}", path.display()),
            )
        })?;
        let stem = path.file_stem().map_or_else(
            || "imported".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        (text, stem)
    };
    let har: Value = serde_json::from_str(&text).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{} is not JSON: {e}", args.file),
        )
        .hint(Hint::text(
            "give a HAR file, e.g. one `chrome-cdp har stop` wrote",
        ))
    })?;
    let exchanges = mock::record::har_exchanges(&har).map_err(|e| {
        ErrorInfo::new(ErrorKind::BadArgs, format!("{}: {e}", args.file)).hint(Hint::text(
            "give a HAR file, e.g. one `chrome-cdp har stop` wrote",
        ))
    })?;
    let out = Target::new(&project, args.output.as_deref(), &stem, args.force)?;
    let origin = format!("Imported by `chrome-cdp mock import` from {}.", args.file);
    write_recording(
        &out,
        &exchanges,
        &args.scope,
        origin,
        args.max_chars,
        || {
            Hint::text(
                "check --scope: the HAR has no request it matches (without it, fetch and XHR ones)",
            )
        },
    )
}

/// Where a recorded set goes.
struct Target {
    /// The mock file (absolute).
    file: PathBuf,
    /// What its data files are relative to.
    base: PathBuf,
    /// Its name for `mock use`.
    set: String,
    /// `data/<set>`.
    data_dir: String,
}

impl Target {
    /// `output` (relative to the current directory), else `<default>.toml`
    /// in the mocks directory. A file in the mocks directory keeps its
    /// data files under the directory's `data/`, as a named set's are.
    fn new(
        project: &Project,
        output: Option<&Path>,
        default: &str,
        force: bool,
    ) -> Result<Self, ErrorInfo> {
        let file = match output {
            Some(path) => project.cwd.join(path),
            None => project.dir.join(format!("{default}.toml")),
        };
        if file.extension().is_none_or(|e| e != "toml") {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{} is not a .toml file", file.display()),
            )
            .hint(Hint::text(
                "name the mock file *.toml, e.g. -o e2e/mocks/recorded.toml",
            )));
        }
        if file.exists() && !force {
            return Err(
                ErrorInfo::new(ErrorKind::BadArgs, format!("{} exists", file.display())).hint(
                    Hint::text("--force overwrites it; or -o names another file"),
                ),
            );
        }
        let (base, set) = match file.strip_prefix(&project.dir) {
            Ok(rel) => (
                project.dir.clone(),
                rel.with_extension("").to_string_lossy().into_owned(),
            ),
            Err(_) => (
                file.parent()
                    .map_or_else(|| project.cwd.clone(), Path::to_path_buf),
                file.display().to_string(),
            ),
        };
        let stem = match file.strip_prefix(&project.dir) {
            Ok(_) => set.clone(),
            Err(_) => file.file_stem().map_or_else(
                || "recorded".to_owned(),
                |s| s.to_string_lossy().into_owned(),
            ),
        };
        Ok(Self {
            file,
            base,
            set,
            data_dir: format!("data/{stem}"),
        })
    }
}

/// Converts, writes and checks a recording.
fn write_recording(
    out: &Target,
    exchanges: &[mock::record::Exchange],
    scope: &[String],
    origin: String,
    max_chars: usize,
    none_hint: impl FnOnce() -> Hint,
) -> Result<Output, ErrorInfo> {
    let options = mock::record::RecordOptions {
        scope: scope.to_vec(),
        data_dir: out.data_dir.clone(),
        origin,
    };
    let recorded = mock::record::record(exchanges, &options)
        .map_err(|e| ErrorInfo::new(ErrorKind::BadArgs, e))?;
    if recorded.mocks == 0 {
        let what = if scope.is_empty() {
            "no fetch or XHR request".to_owned()
        } else {
            format!("no request in scope {}", scope.join(", "))
        };
        let mut info =
            ErrorInfo::new(ErrorKind::NotFound, format!("{what} to record")).hint(none_hint());
        for skipped in recorded.skipped.iter().take(3) {
            info = info.note("left out", skipped);
        }
        return Err(info);
    }
    let io = |path: &Path, e: std::io::Error| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot write {}: {e}", path.display()),
        )
    };
    let mut files = Vec::new();
    for (rel, bytes) in &recorded.files {
        let path = out.base.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        std::fs::write(&path, bytes).map_err(|e| io(&path, e))?;
        files.push(path.display().to_string());
    }
    if let Some(parent) = out.file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    std::fs::write(&out.file, &recorded.toml).map_err(|e| io(&out.file, e))?;
    let source = Source {
        name: out.set.clone(),
        file: out.file.clone(),
        base: out.base.clone(),
    };
    MockSet::load(&source).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::ProtocolError,
            format!(
                "the recorded file does not load (a chrome-cdp bug): {}",
                e.message
            ),
        )
    })?;
    let path = out.file.display().to_string();
    let mut text = format!(
        "{path}: {} {} from {} {}",
        recorded.mocks,
        if recorded.mocks == 1 { "mock" } else { "mocks" },
        recorded.requests,
        if recorded.requests == 1 {
            "request"
        } else {
            "requests"
        },
    );
    if !files.is_empty() {
        let _ = write!(
            text,
            ", {} data {} in {}",
            files.len(),
            if files.len() == 1 { "file" } else { "files" },
            out.base.join(&out.data_dir).display()
        );
    }
    let _ = write!(
        text,
        "\nnext: {}",
        Cmd::new("mock").arg("use").arg(&out.set)
    );
    let data = MockRecordData {
        path: path.clone(),
        set: out.set.clone(),
        requests: recorded.requests,
        mocks: recorded.mocks,
        files,
        scope: recorded.scope.clone(),
        skipped: recorded.skipped.clone(),
    };
    let mut output = Output::typed(&data, text)
        .quiet(path)
        .limit(max_chars, "--max-chars 0 for all of it");
    let listed = 5;
    for skipped in recorded.skipped.iter().take(listed) {
        output = output.warning(format!("left out {skipped}"));
    }
    if recorded.skipped.len() > listed {
        output = output.warning(format!(
            "left out {} more requests (see --json)",
            recorded.skipped.len() - listed
        ));
    }
    Ok(output)
}

/// A value as `mock state --set` and `expect mock-state` take it: JSON
/// (`true`, `3`, `["a"]`, `null`), else the text itself.
pub(crate) fn state_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

fn state(args: &MockStateArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let set = args
        .set
        .iter()
        .map(|pair| {
            let (name, value) = pair.split_once('=').ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("--set takes NAME=VALUE, got {pair:?}"),
                )
                .hint(Hint::run(
                    Cmd::new("mock").arg("state").flag("set", "logged_in=true"),
                ))
            })?;
            Ok((name.trim().to_owned(), state_value(value)))
        })
        .collect::<Result<Vec<_>, ErrorInfo>>()?;
    let request = MockStateRequest {
        name: args.name.clone(),
        set,
        max_chars: args.max_chars,
    };
    session::call_session(global, rpc::MOCK_STATE, &request, "mock sets")
}

fn seed(args: &MockSeedArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let (file, items) = match args.file.as_deref() {
        Some("-") => {
            let text = input::text_arg("-", "the items")?;
            let items = serde_json::from_str::<Value>(&text).map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("the items on stdin are not JSON: {e}"),
                )
                .hint(Hint::text(
                    "give a JSON array of objects, e.g. [{\"id\": 1, \"title\": \"Milk\"}]",
                ))
            })?;
            (None, Some(items))
        }
        Some(file) => (Some(file.to_owned()), None),
        None => (None, None),
    };
    let request = MockSeedRequest {
        name: args.name.clone(),
        file,
        items,
        cwd: std::env::current_dir().ok(),
    };
    session::call_session(global, rpc::MOCK_SEED, &request, "mock sets")
}

/// `mock ws send`.
fn ws_send(args: &MockWsSendArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let message = input::text_arg(&args.message, "the message")?;
    let request = MockWsSendRequest {
        url: args.url.clone(),
        message,
        max_chars: args.max_chars,
        timeout_ms: session::timeout_ms(global),
    };
    session::call_session(global, rpc::MOCK_WS_SEND, &request, "mocked sockets")
}

fn mock_use(args: &MockUseArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let project = Project::find(args.dir.as_deref())?;
    let mut names: Vec<String> = Vec::new();
    if !args.add && !args.no_defaults {
        names.extend(project.defaults.iter().cloned());
    }
    for name in &args.sets {
        // Named again, a default moves up to where it is named.
        names.retain(|n| n != name);
        names.push(name.clone());
    }
    if names.is_empty() {
        return Err(project.nothing(Cmd::new("mock").arg("use")));
    }
    let test = args.test_id.as_ref().map(|id| MockTestTag {
        id: id.clone(),
        origins: args.test_origins.clone(),
    });
    let request = MockUseRequest {
        sets: project.sources(&names)?,
        add: args.add,
        seed: project.seed,
        test,
        timeout_ms: session::timeout_ms(global),
    };
    session::call_session(global, rpc::MOCK_USE, &request, "mock sets")
}

/// `mock list --available`: the sets in the mocks directory.
fn available(dir: Option<&Path>) -> Result<Output, ErrorInfo> {
    let project = Project::find(dir)?;
    let sets = mock::available(&project.dir);
    let text = if sets.is_empty() {
        format!(
            "no mock sets in {}; `chrome-cdp mock --help` shows the format",
            project.dir.display()
        )
    } else {
        sets.join("\n")
    };
    let quiet = sets.join("\n");
    let data = MockSetsData {
        sets: Vec::new(),
        available: Some(sets),
        dir: Some(project.dir.display().to_string()),
    };
    Ok(Output::typed(&data, text).quiet(quiet))
}

fn explain(args: &MockExplainArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let method = args.method.trim().to_ascii_uppercase();
    if method.is_empty() || !method.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("`{}` is not an HTTP method", args.method),
        )
        .hint(Hint::text("e.g. mock explain GET /api/items")));
    }
    let (url, relative) = absolute_url(&args.url, global.base_url.as_deref())?;
    let mut headers: Vec<(String, String)> = args
        .headers
        .iter()
        .map(|h| route::header(h, "--header"))
        .collect::<Result<_, _>>()?;
    let body = match (&args.json_body, &args.body) {
        (Some(text), _) => {
            let value = route::json(text, "--json-body")?;
            if !headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("Content-Type".into(), "application/json".into()));
            }
            Some(value.to_string())
        }
        (None, Some(text)) => Some(input::text_arg(text, "--body")?),
        (None, None) => None,
    };
    let request = MockExplainRequest {
        method,
        url,
        headers,
        body,
        limit: args.limit,
        max_chars: args.max_chars,
    };
    let mut output = if args.mocks.is_empty()
        && let Some(output) = session::call_if_running(global, rpc::MOCK_EXPLAIN, &request)?
        && session_explained(&output)
    {
        output
    } else {
        explain_here(args, &request)?
    };
    if relative {
        output = output.warning(
            "no base_url is set, so the path was taken as http://localhost…; url mocks may not match",
        );
    }
    Ok(output)
}

/// Whether the session's answer says something: it has mock sets (or a
/// route answers).
fn session_explained(output: &Output) -> bool {
    serde_json::from_value::<MockExplainData>(output.data.clone())
        .is_ok_and(|d| !d.checked.is_empty() || d.route.is_some())
}

/// Explains with `--mocks`, or the defaults, read here.
fn explain_here(args: &MockExplainArgs, request: &MockExplainRequest) -> Result<Output, ErrorInfo> {
    let project = Project::find(args.dir.as_deref())?;
    let names = if args.mocks.is_empty() {
        project.defaults.clone()
    } else {
        args.mocks.clone()
    };
    if names.is_empty() {
        return Err(project
            .nothing(
                Cmd::new("mock")
                    .arg("explain")
                    .arg(&request.method)
                    .arg(&args.url)
                    .raw("--mocks"),
            )
            .note("session", "no mock sets are in use in the session"));
    }
    let sets = project
        .sources(&names)?
        .iter()
        .map(MockSet::load)
        .collect::<Result<Vec<_>, _>>()?;
    let mut mocks = Mocks::new(project.seed);
    mocks.apply(sets, false);
    let data = mocks.explain(&mock::Request {
        method: &request.method,
        url: &request.url,
        headers: &request.headers,
        body: request.body.as_deref().map(str::as_bytes),
    });
    let text = mock::text::explain(&data, request.limit);
    let quiet = data
        .answer
        .as_ref()
        .map_or_else(String::new, |m| m.at.clone());
    let mut output = Output::typed(&data, text).quiet(quiet).limit(
        request.max_chars,
        "--max-chars 0 for all of it, or --limit N",
    );
    if args.mocks.is_empty() {
        output = output.warning(format!(
            "explained with the project's default_mocks ({}), since no session uses mock sets",
            names.join(", ")
        ));
    }
    Ok(output)
}

/// `url` made absolute against `base` (or `http://localhost`, which the
/// `bool` says).
pub(crate) fn absolute_url(url: &str, base: Option<&str>) -> Result<(String, bool), ErrorInfo> {
    if url.starts_with('/') {
        let (base, guessed) = match base {
            Some(base) => (base, false),
            None => ("http://localhost", true),
        };
        let joined = url::Url::parse(base)
            .and_then(|b| b.join(url))
            .map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("cannot join {url} to {base}: {e}"),
                )
            })?;
        return Ok((joined.to_string(), guessed));
    }
    let parsed = url::Url::parse(url).map_err(|e| {
        ErrorInfo::new(ErrorKind::BadArgs, format!("`{url}` is not a URL: {e}")).hint(Hint::text(
            "give a full URL (https://…) or a path starting with /",
        ))
    })?;
    Ok((parsed.to_string(), false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_made_absolute() {
        assert_eq!(
            absolute_url("/api/x?q=1", Some("http://localhost:5173/app/")).unwrap(),
            ("http://localhost:5173/api/x?q=1".into(), false)
        );
        assert_eq!(
            absolute_url("/api/x", None).unwrap(),
            ("http://localhost/api/x".into(), true)
        );
        assert_eq!(
            absolute_url("https://api.example.com/v1", None).unwrap(),
            ("https://api.example.com/v1".into(), false)
        );
        assert!(absolute_url("api/x", None).is_err());
    }
}
