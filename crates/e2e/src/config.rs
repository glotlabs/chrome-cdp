//! The `[e2e]` section of the project config (plan 11, milestone 11.1).
//!
//! The project config is the nearest `chrome-cdp.toml` or `.chrome-cdp.toml`
//! walking up from the current directory ([`find_project_file`]); the
//! global keys of plan 08 §4 live in the same file. Only the commands that
//! test read `[e2e]`, so [`Config::parse`] is strict: an unknown key or a
//! bad value is a `BadArgs` error that points at `file:line:col`.
//!
//! Relative paths (`mocks`, `artifacts`, a web server's `cwd`, a backend's
//! `vars`) are relative to the config file's directory, the project root.
//! Globs in `tests` are kept as written, relative to the same root.
//!
//! Later milestones read the keys they own (`determinism` in 11.6,
//! `backends` in 11.11) and add new ones here.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use protocol::format::parse_duration;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use toml::Spanned;

/// The project config's names; `init e2e` writes the first.
pub const PROJECT_FILES: [&str; 2] = ["chrome-cdp.toml", ".chrome-cdp.toml"];

/// Where tests are found when `tests` is not set.
pub const DEFAULT_TESTS: &str = "e2e/scenarios/**/*.cdp";
/// Where mock sets live when `mocks` is not set.
pub const DEFAULT_MOCKS: &str = "e2e/mocks";
/// Where test artifacts go when `artifacts` is not set.
pub const DEFAULT_ARTIFACTS: &str = "e2e/.results";
/// A test's time limit when `timeout` is not set.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a web server may take to answer when its `timeout` is not set.
pub const DEFAULT_SERVER_TIMEOUT: Duration = Duration::from_secs(60);
/// The viewport when `viewport` is not set.
pub const DEFAULT_VIEWPORT: Viewport = Viewport {
    width: 1280,
    height: 800,
};
/// The built-in backend's name.
pub const MOCK_BACKEND: &str = "mock";

/// The nearest project config in `dir` or its ancestors. A directory with
/// both names is an error: which one wins would be a guess.
pub fn find_project_file(dir: &Path) -> Result<Option<PathBuf>, ErrorInfo> {
    for d in dir.ancestors() {
        let [a, b] = PROJECT_FILES.map(|name| d.join(name));
        match (a.is_file(), b.is_file()) {
            (true, true) => {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!(
                        "{} has both {} and {}",
                        d.display(),
                        PROJECT_FILES[0],
                        PROJECT_FILES[1]
                    ),
                )
                .hint(format!(
                    "merge them into {} and delete {}",
                    a.display(),
                    b.display()
                )));
            }
            (true, false) => return Ok(Some(a)),
            (false, true) => return Ok(Some(b)),
            (false, false) => {}
        }
    }
    Ok(None)
}

/// The `[e2e]` settings, validated, with defaults filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The config file.
    pub file: PathBuf,
    /// The project root: the config file's directory.
    pub root: PathBuf,
    /// What `goto /path` in a scenario is relative to.
    pub base_url: Option<String>,
    /// Globs of scenario files, relative to [`Self::root`].
    pub tests: Vec<String>,
    /// The mock sets' directory.
    pub mocks: PathBuf,
    /// Mock sets every test gets unless it says otherwise.
    pub default_mocks: Vec<String>,
    /// The backend `test` uses without `--backend`: [`MOCK_BACKEND`] or a
    /// key of [`Self::backends`].
    pub default_backend: String,
    pub jobs: Jobs,
    pub retries: u32,
    /// Whether a test may log console errors; if not (the default), the
    /// runner checks `expect --no-errors` at the end of each (plan 11.5).
    pub allow_console_errors: bool,
    /// A test's time limit.
    pub timeout: Duration,
    /// Where test artifacts go (`.results`).
    pub artifacts: PathBuf,
    /// Globs of more files whose change makes `test --watch` run every
    /// test again (a build's output, say), relative to [`Self::root`].
    pub watch: Vec<String>,
    pub viewport: Viewport,
    /// `[e2e.web_server]` or `[[e2e.web_server]]`, in the order written.
    pub web_servers: Vec<WebServer>,
    /// `[e2e.determinism]` as written; [`Determinism::resolve`] fills in
    /// the defaults.
    pub determinism: Determinism,
    /// `[e2e.backends.<name>]`, the real backends (plan 11.11).
    pub backends: BTreeMap<String, Backend>,
}

/// How many tests run at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jobs {
    /// Half the CPUs, at least one.
    Auto,
    Count(NonZeroU32),
}

impl Jobs {
    /// The number of tests to run at once.
    pub fn count(self) -> u32 {
        match self {
            Self::Count(n) => n.get(),
            Self::Auto => std::thread::available_parallelism()
                .map_or(1, |n| u32::try_from(n.get() / 2).unwrap_or(u32::MAX))
                .max(1),
        }
    }
}

/// A viewport size in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    /// `1280x800`; `None` unless both sizes are positive numbers.
    pub fn parse(text: &str) -> Option<Self> {
        text.split_once('x')
            .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
            .filter(|&(w, h): &(u32, u32)| w > 0 && h > 0)
            .map(|(width, height)| Self { width, height })
    }
}

impl fmt::Display for Viewport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// A command that serves the app, and how to know it is up.
#[derive(Debug, Clone, PartialEq)]
pub struct WebServer {
    /// A shell command line (`sh -c`).
    pub command: String,
    /// Ready when a GET of it gets an HTTP status below 500.
    pub url: String,
    /// When something already answers on `url`, use it and start nothing.
    pub reuse_existing: bool,
    /// How long it may take to answer.
    pub timeout: Duration,
    /// Extra environment variables for the command.
    pub env: BTreeMap<String, String>,
    /// The command's directory: the project root unless `cwd` is set.
    pub cwd: PathBuf,
}

/// `[e2e.determinism]` (plan 11.6). `None` means "not set here";
/// [`Determinism::resolve`] fills in the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Determinism {
    pub freeze_time: Option<FreezeTime>,
    pub random_seed: Option<RandomSeed>,
    pub disable_animations: Option<bool>,
    pub locale: Option<Pin>,
    pub timezone: Option<Pin>,
}

/// `freeze_time = false` or a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreezeTime {
    Off,
    /// As written, e.g. `2026-01-01T10:00:00Z`; checked to parse (with
    /// [`protocol::format::parse_time`]) when the config is read.
    At(String),
}

/// `random_seed = false` or a seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomSeed {
    Off,
    Seed(u64),
}

/// `locale` or `timezone`: a value, or `false` for the browser's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pin {
    Off,
    To(String),
}

/// The time tests see when `freeze_time` is not set.
pub const DEFAULT_FREEZE_TIME: &str = "2026-01-01T10:00:00Z";
/// Pages' `Math.random` and mocks' random values without `random_seed`.
pub const DEFAULT_RANDOM_SEED: u64 = 0;
/// The locale tests get when `locale` is not set.
pub const DEFAULT_LOCALE: &str = "en-US";
/// The time zone tests get when `timezone` is not set.
pub const DEFAULT_TIMEZONE: &str = "UTC";

/// What every test gets (plan 11.6): [`Determinism`] with the defaults
/// filled in. `None` is off: the browser's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixed {
    /// The time `Date` gives, as written.
    pub freeze_time: Option<String>,
    /// The seed of pages' `Math.random`, `crypto.randomUUID` and
    /// `crypto.getRandomValues`.
    pub random_seed: Option<u64>,
    /// No CSS animations or transitions, and `prefers-reduced-motion`.
    pub disable_animations: bool,
    pub locale: Option<String>,
    pub timezone: Option<String>,
}

impl Determinism {
    /// These settings with `over`'s (a backend's) on top.
    #[must_use]
    pub fn with(&self, over: &Self) -> Self {
        Self {
            freeze_time: over
                .freeze_time
                .clone()
                .or_else(|| self.freeze_time.clone()),
            random_seed: over.random_seed.or(self.random_seed),
            disable_animations: over.disable_animations.or(self.disable_animations),
            locale: over.locale.clone().or_else(|| self.locale.clone()),
            timezone: over.timezone.clone().or_else(|| self.timezone.clone()),
        }
    }

    /// What tests get: the settings, else the defaults.
    pub fn resolve(&self) -> Fixed {
        let pin = |pin: &Option<Pin>, default: &str| match pin {
            None => Some(default.to_owned()),
            Some(Pin::To(value)) => Some(value.clone()),
            Some(Pin::Off) => None,
        };
        Fixed {
            freeze_time: match &self.freeze_time {
                None => Some(DEFAULT_FREEZE_TIME.to_owned()),
                Some(FreezeTime::At(time)) => Some(time.clone()),
                Some(FreezeTime::Off) => None,
            },
            random_seed: match self.random_seed {
                None => Some(DEFAULT_RANDOM_SEED),
                Some(RandomSeed::Seed(seed)) => Some(seed),
                Some(RandomSeed::Off) => None,
            },
            disable_animations: self.disable_animations.unwrap_or(true),
            locale: pin(&self.locale, DEFAULT_LOCALE),
            timezone: pin(&self.timezone, DEFAULT_TIMEZONE),
        }
    }
}

/// A real backend, `[e2e.backends.<name>]` (plan 11.11). Unset values
/// fall back to the `[e2e]` ones when 11.11 runs against it.
#[derive(Debug, Clone, PartialEq)]
pub struct Backend {
    pub base_url: Option<String>,
    /// `None` when the backend sets no `web_server`.
    pub web_servers: Option<Vec<WebServer>>,
    /// Commands run before the suite, and after it.
    pub setup: Vec<String>,
    pub teardown: Vec<String>,
    /// The `${var:…}` file.
    pub vars: Option<PathBuf>,
    pub jobs: Option<Jobs>,
    pub timeout: Option<Duration>,
    pub retries: Option<u32>,
    pub determinism: Determinism,
    /// Mock sets kept on in real mode (hybrid mode).
    pub mocks: Vec<String>,
    /// Refuses writes unless the run allows them.
    pub protected: bool,
}

impl Config {
    /// The `[e2e]` config of the project `dir` is in. An error when there
    /// is no project config, or it has no `[e2e]` section; its hint is
    /// `init e2e`.
    pub fn find(dir: &Path) -> Result<Self, ErrorInfo> {
        let Some(file) = find_project_file(dir)? else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "no {} in {} or a directory above it",
                    PROJECT_FILES[0],
                    dir.display()
                ),
            )
            .hint(Hint::run(Cmd::new("init").arg("e2e")).why("writes one")));
        };
        let text = std::fs::read_to_string(&file).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read {}: {e}", file.display()),
            )
        })?;
        Self::parse(&text, &file)?.ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{} has no [e2e] section", file.display()),
            )
            .hint(Hint::run(Cmd::new("init").arg("e2e")).why("adds one and keeps the rest"))
        })
    }

    /// The `[e2e]` section of the config file `file` whose text is `text`;
    /// `None` if it has none. Other keys are not checked here.
    pub fn parse(text: &str, file: &Path) -> Result<Option<Self>, ErrorInfo> {
        let root = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let at = |span: Range<usize>, message: String| {
            let (line, col) = line_col(text, span.start);
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{}:{line}:{col}: {message}", file.display()),
            )
            .hint(format!(
                "fix it in {}; `chrome-cdp help config` lists the [e2e] keys",
                file.display()
            ))
        };
        let raw: RawFile = toml::from_str(text).map_err(|e| {
            let message = e.message().trim().to_owned();
            match e.span() {
                Some(span) => at(span, message),
                None => {
                    ErrorInfo::new(ErrorKind::BadArgs, format!("{}: {message}", file.display()))
                }
            }
        })?;
        let Some(raw) = raw.e2e else {
            return Ok(None);
        };
        let check = Check { root: &root };
        check
            .config(raw, file)
            .map(Some)
            .map_err(|(span, m)| at(span, m))
    }
}

/// The 1-based line and column (in characters) of byte `offset`.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// A problem at a span of the file.
type Problem = (Range<usize>, String);

/// Turns the raw file into [`Config`].
struct Check<'a> {
    root: &'a Path,
}

impl Check<'_> {
    fn config(&self, raw: RawE2e, file: &Path) -> Result<Config, Problem> {
        let backends = raw
            .backends
            .unwrap_or_default()
            .into_iter()
            .map(|(name, backend)| {
                let key = format!("e2e.backends.{}", name.get_ref());
                Self::backend_name(&name)?;
                Ok((name.into_inner(), self.backend(backend, &key)?))
            })
            .collect::<Result<BTreeMap<_, _>, Problem>>()?;
        let default_backend = match raw.default_backend {
            None => MOCK_BACKEND.to_owned(),
            Some(name) => {
                let known = name.get_ref() == MOCK_BACKEND || backends.contains_key(name.get_ref());
                if !known {
                    let names: Vec<&str> = std::iter::once(MOCK_BACKEND)
                        .chain(backends.keys().map(String::as_str))
                        .collect();
                    return Err((
                        name.span(),
                        format!(
                            "e2e.default_backend: no backend `{}`; the backends are {}",
                            name.get_ref(),
                            names.join(", ")
                        ),
                    ));
                }
                name.into_inner()
            }
        };
        let tests = match raw.tests {
            None => vec![DEFAULT_TESTS.to_owned()],
            Some(tests) if tests.get_ref().is_empty() => {
                return Err((tests.span(), "e2e.tests: name at least one glob".into()));
            }
            Some(tests) => {
                let span = tests.span();
                let tests = tests.into_inner();
                if tests.iter().any(|t| t.trim().is_empty()) {
                    return Err((span, "e2e.tests: a glob is empty".into()));
                }
                tests
            }
        };
        Ok(Config {
            file: file.to_path_buf(),
            root: self.root.to_path_buf(),
            base_url: raw.base_url.map(|u| url("e2e.base_url", u)).transpose()?,
            tests,
            mocks: self.path(raw.mocks.as_deref().unwrap_or(DEFAULT_MOCKS)),
            default_mocks: raw
                .default_mocks
                .map(|m| mock_sets("e2e.default_mocks", m))
                .transpose()?
                .unwrap_or_default(),
            default_backend,
            jobs: raw.jobs.map_or(Jobs::Auto, Spanned::into_inner),
            retries: raw.retries.unwrap_or(0),
            allow_console_errors: raw.allow_console_errors.unwrap_or(false),
            timeout: raw
                .timeout
                .map(|t| duration("e2e.timeout", &t))
                .transpose()?
                .unwrap_or(DEFAULT_TIMEOUT),
            artifacts: self.path(raw.artifacts.as_deref().unwrap_or(DEFAULT_ARTIFACTS)),
            watch: raw.watch.unwrap_or_default(),
            viewport: raw
                .viewport
                .map(|v| viewport(&v))
                .transpose()?
                .unwrap_or(DEFAULT_VIEWPORT),
            web_servers: raw
                .web_server
                .map(|w| self.web_servers("e2e.web_server", w))
                .transpose()?
                .unwrap_or_default(),
            determinism: raw.determinism.map(Determinism::from).unwrap_or_default(),
            backends,
        })
    }

    fn backend_name(name: &Spanned<String>) -> Result<(), Problem> {
        let n = name.get_ref();
        if n == MOCK_BACKEND {
            return Err((
                name.span(),
                "e2e.backends.mock: `mock` is the built-in backend; name a real one differently"
                    .into(),
            ));
        }
        let ok = !n.is_empty()
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        if ok {
            Ok(())
        } else {
            Err((
                name.span(),
                format!(
                    "e2e.backends: `{n}` is not a backend name (letters, digits, - and _), \
                     since it is typed after --backend"
                ),
            ))
        }
    }

    fn backend(&self, raw: Spanned<RawBackend>, key: &str) -> Result<Backend, Problem> {
        let span = raw.span();
        let raw = raw.into_inner();
        match raw.mode {
            None => {
                return Err((
                    span,
                    format!("{key}: set mode = \"real\" (the mock backend is built in)"),
                ));
            }
            Some(mode) if mode.get_ref() != "real" => {
                return Err((
                    mode.span(),
                    format!(
                        "{key}.mode: `{}` is not \"real\"; the mock backend is built in",
                        mode.get_ref()
                    ),
                ));
            }
            Some(_) => {}
        }
        Ok(Backend {
            base_url: raw
                .base_url
                .map(|u| url(&format!("{key}.base_url"), u))
                .transpose()?,
            web_servers: raw
                .web_server
                .map(|w| self.web_servers(&format!("{key}.web_server"), w))
                .transpose()?,
            setup: commands(&format!("{key}.setup"), raw.setup)?,
            teardown: commands(&format!("{key}.teardown"), raw.teardown)?,
            vars: raw.vars.as_deref().map(|v| self.path(v)),
            jobs: raw.jobs.map(Spanned::into_inner),
            timeout: raw
                .timeout
                .map(|t| duration(&format!("{key}.timeout"), &t))
                .transpose()?,
            retries: raw.retries,
            determinism: raw.determinism.map(Determinism::from).unwrap_or_default(),
            mocks: raw
                .mocks
                .map(|m| mock_sets(&format!("{key}.mocks"), m))
                .transpose()?
                .unwrap_or_default(),
            protected: raw.protected.unwrap_or(false),
        })
    }

    fn web_servers(
        &self,
        key: &str,
        raw: Spanned<OneOrMany<RawWebServer>>,
    ) -> Result<Vec<WebServer>, Problem> {
        let span = raw.span();
        let list = match raw.into_inner() {
            OneOrMany::One(one) => vec![one],
            OneOrMany::Many(many) => many,
        };
        if list.is_empty() {
            return Err((span, format!("{key}: an empty list; remove it instead")));
        }
        list.into_iter()
            .map(|raw| self.web_server(key, raw))
            .collect()
    }

    fn web_server(&self, key: &str, raw: RawWebServer) -> Result<WebServer, Problem> {
        if raw.command.get_ref().trim().is_empty() {
            return Err((raw.command.span(), format!("{key}.command: empty")));
        }
        Ok(WebServer {
            command: raw.command.into_inner(),
            url: url(&format!("{key}.url"), raw.url)?,
            reuse_existing: raw.reuse_existing.unwrap_or(true),
            timeout: raw
                .timeout
                .map(|t| duration(&format!("{key}.timeout"), &t))
                .transpose()?
                .unwrap_or(DEFAULT_SERVER_TIMEOUT),
            env: raw.env.unwrap_or_default(),
            cwd: raw
                .cwd
                .as_deref()
                .map_or_else(|| self.root.to_path_buf(), |c| self.path(c)),
        })
    }

    /// `path` relative to the project root, unless it is absolute.
    fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

fn url(key: &str, raw: Spanned<String>) -> Result<String, Problem> {
    let u = raw.get_ref();
    let host = u
        .strip_prefix("http://")
        .or_else(|| u.strip_prefix("https://"))
        .unwrap_or_default();
    if host.is_empty() || host.starts_with('/') {
        return Err((
            raw.span(),
            format!("{key}: `{u}` is not an absolute http(s) URL, e.g. http://localhost:5173"),
        ));
    }
    Ok(raw.into_inner())
}

fn duration(key: &str, raw: &Spanned<String>) -> Result<Duration, Problem> {
    parse_duration(raw.get_ref()).map_err(|e| (raw.span(), format!("{key}: {e}")))
}

fn viewport(raw: &Spanned<String>) -> Result<Viewport, Problem> {
    Viewport::parse(raw.get_ref()).ok_or_else(|| {
        (
            raw.span(),
            format!(
                "e2e.viewport: `{}` is not WIDTHxHEIGHT, e.g. 1280x800",
                raw.get_ref()
            ),
        )
    })
}

/// Mock set names: paths below `mocks` without `.toml`.
fn mock_sets(key: &str, raw: Spanned<Vec<String>>) -> Result<Vec<String>, Problem> {
    for name in raw.get_ref() {
        let problem = if name.trim().is_empty() {
            Some("a name is empty".to_owned())
        } else if let Some(stem) = name.strip_suffix(".toml") {
            Some(format!("name the mock set `{stem}`, without .toml"))
        } else if name.starts_with('/') || name.split('/').any(|part| part == "..") {
            Some(format!(
                "`{name}` leaves the mocks directory; name a set below it"
            ))
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err((raw.span(), format!("{key}: {problem}")));
        }
    }
    Ok(raw.into_inner())
}

fn commands(key: &str, raw: Option<Spanned<Vec<String>>>) -> Result<Vec<String>, Problem> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.get_ref().iter().any(|c| c.trim().is_empty()) {
        return Err((raw.span(), format!("{key}: a command is empty")));
    }
    Ok(raw.into_inner())
}

// The file as written. Keys outside `[e2e]` are plan 08's, checked by the
// CLI's config loader.

#[derive(Deserialize)]
struct RawFile {
    e2e: Option<RawE2e>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawE2e {
    base_url: Option<Spanned<String>>,
    tests: Option<Spanned<Vec<String>>>,
    mocks: Option<String>,
    default_mocks: Option<Spanned<Vec<String>>>,
    default_backend: Option<Spanned<String>>,
    jobs: Option<Spanned<Jobs>>,
    retries: Option<u32>,
    allow_console_errors: Option<bool>,
    timeout: Option<Spanned<String>>,
    artifacts: Option<String>,
    watch: Option<Vec<String>>,
    viewport: Option<Spanned<String>>,
    web_server: Option<Spanned<OneOrMany<RawWebServer>>>,
    determinism: Option<RawDeterminism>,
    backends: Option<BTreeMap<Spanned<String>, Spanned<RawBackend>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWebServer {
    command: Spanned<String>,
    url: Spanned<String>,
    reuse_existing: Option<bool>,
    timeout: Option<Spanned<String>>,
    env: Option<BTreeMap<String, String>>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeterminism {
    freeze_time: Option<FreezeTime>,
    random_seed: Option<RandomSeed>,
    disable_animations: Option<bool>,
    locale: Option<Pin>,
    timezone: Option<Pin>,
}

impl From<RawDeterminism> for Determinism {
    fn from(raw: RawDeterminism) -> Self {
        Self {
            freeze_time: raw.freeze_time,
            random_seed: raw.random_seed,
            disable_animations: raw.disable_animations,
            locale: raw.locale,
            timezone: raw.timezone,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBackend {
    mode: Option<Spanned<String>>,
    base_url: Option<Spanned<String>>,
    web_server: Option<Spanned<OneOrMany<RawWebServer>>>,
    setup: Option<Spanned<Vec<String>>>,
    teardown: Option<Spanned<Vec<String>>>,
    vars: Option<String>,
    jobs: Option<Spanned<Jobs>>,
    timeout: Option<Spanned<String>>,
    retries: Option<u32>,
    determinism: Option<RawDeterminism>,
    mocks: Option<Spanned<Vec<String>>>,
    protected: Option<bool>,
}

/// A table, or an array of tables (`[x]` or `[[x]]`), keeping the TOML
/// deserializer's own errors and spans.
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OneOrMany<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = OneOrMany<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a table or an array of tables")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(OneOrMany::One)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                Vec::deserialize(de::value::SeqAccessDeserializer::new(seq)).map(OneOrMany::Many)
            }
        }
        d.deserialize_any(V(PhantomData))
    }
}

impl<'de> Deserialize<'de> for Jobs {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Jobs;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a number of jobs above 0, or \"auto\"")
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> Result<Jobs, E> {
                u32::try_from(n)
                    .ok()
                    .and_then(NonZeroU32::new)
                    .map(Jobs::Count)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Signed(n), &self))
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Jobs, E> {
                if s == "auto" {
                    Ok(Jobs::Auto)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(s), &self))
                }
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> Deserialize<'de> for FreezeTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = FreezeTime;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a time such as \"2026-01-01T10:00:00Z\", or false")
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> Result<FreezeTime, E> {
                if b {
                    Err(E::invalid_value(de::Unexpected::Bool(b), &self))
                } else {
                    Ok(FreezeTime::Off)
                }
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<FreezeTime, E> {
                if s.trim().is_empty() {
                    return Err(E::invalid_value(de::Unexpected::Str(s), &self));
                }
                protocol::format::parse_time(s).map_err(E::custom)?;
                Ok(FreezeTime::At(s.trim().to_owned()))
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> Deserialize<'de> for RandomSeed {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = RandomSeed;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a seed (a whole number of 0 or more), or false")
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> Result<RandomSeed, E> {
                if b {
                    Err(E::invalid_value(de::Unexpected::Bool(b), &self))
                } else {
                    Ok(RandomSeed::Off)
                }
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> Result<RandomSeed, E> {
                u64::try_from(n)
                    .map(RandomSeed::Seed)
                    .map_err(|_| E::invalid_value(de::Unexpected::Signed(n), &self))
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> Deserialize<'de> for Pin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Pin;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a name such as \"en-US\" or \"UTC\", or false for the browser's own")
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> Result<Pin, E> {
                if b {
                    Err(E::invalid_value(de::Unexpected::Bool(b), &self))
                } else {
                    Ok(Pin::Off)
                }
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Pin, E> {
                if s.trim().is_empty() || s.contains(char::is_whitespace) {
                    Err(E::invalid_value(de::Unexpected::Str(s), &self))
                } else {
                    Ok(Pin::To(s.to_owned()))
                }
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests;
