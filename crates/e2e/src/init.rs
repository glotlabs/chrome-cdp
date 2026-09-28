//! `chrome-cdp init e2e` (plan 11, milestone 11.1): scaffolds a project's
//! E2E layout.
//!
//! - The framework comes from `package.json`'s dependencies (Next,
//!   SvelteKit, Astro, create-react-app, then Vite) and the package
//!   manager from its `packageManager` field or the lock file. Together
//!   they give the `[e2e.web_server]` command and `base_url`.
//! - It writes the `[e2e]` section (a new `chrome-cdp.toml`, or appended
//!   to the project config that has none), an example scenario, the mock
//!   set "base" with a data file, and a GitHub Actions workflow, and adds
//!   the artifacts directory to `.gitignore`.
//! - It never overwrites a file: one that exists is kept, and the config
//!   and `.gitignore` only get lines added at their end.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use protocol::schema::{InitAction, InitData, InitFile, InitWebServer};
use protocol::{ErrorInfo, ErrorKind};
use serde::Deserialize;

use crate::config::{DEFAULT_ARTIFACTS, PROJECT_FILES};

/// The example scenario.
pub const SCENARIO: &str = include_str!("init/example.cdp");
const MOCKS: &str = include_str!("init/base.toml");
const DATA: &str = include_str!("init/items.json");

/// The files `init e2e` writes, relative to the project root.
pub const SCENARIO_PATH: &str = "e2e/scenarios/example.cdp";
pub const MOCKS_PATH: &str = "e2e/mocks/base.toml";
pub const DATA_PATH: &str = "e2e/mocks/data/items.json";
pub const WORKFLOW_PATH: &str = ".github/workflows/e2e.yml";

/// The `base_url` written when no framework is known.
const FALLBACK_URL: &str = "http://localhost:3000";

/// A frontend framework `init` knows how to serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framework {
    Vite,
    Next,
    Cra,
    Astro,
    SvelteKit,
}

impl Framework {
    /// Every framework, in the order `package.json` is checked: a
    /// SvelteKit or Astro app depends on Vite too.
    pub const ALL: [Self; 5] = [
        Self::Next,
        Self::SvelteKit,
        Self::Astro,
        Self::Cra,
        Self::Vite,
    ];

    /// The name `--framework` takes and `--json` shows.
    pub fn name(self) -> &'static str {
        match self {
            Self::Vite => "vite",
            Self::Next => "next",
            Self::Cra => "cra",
            Self::Astro => "astro",
            Self::SvelteKit => "sveltekit",
        }
    }

    /// The name people know it by.
    pub fn label(self) -> &'static str {
        match self {
            Self::Vite => "Vite",
            Self::Next => "Next.js",
            Self::Cra => "Create React App",
            Self::Astro => "Astro",
            Self::SvelteKit => "SvelteKit",
        }
    }

    /// The dependency that gives it away.
    fn dependency(self) -> &'static str {
        match self {
            Self::Vite => "vite",
            Self::Next => "next",
            Self::Cra => "react-scripts",
            Self::Astro => "astro",
            Self::SvelteKit => "@sveltejs/kit",
        }
    }

    /// Its dev server's usual port.
    fn port(self) -> u16 {
        match self {
            Self::Vite | Self::SvelteKit => 5173,
            Self::Next | Self::Cra => 3000,
            Self::Astro => 4321,
        }
    }

    /// The `package.json` script that serves it, and the arguments that
    /// pin the port (create-react-app takes `PORT` instead).
    fn script(self) -> (&'static str, String) {
        let port = self.port();
        match self {
            // --strictPort: fail when the port is taken instead of moving
            // to another one that `url` does not name.
            Self::Vite | Self::SvelteKit => ("dev", format!("--port {port} --strictPort")),
            Self::Next | Self::Astro => ("dev", format!("--port {port}")),
            Self::Cra => ("start", String::new()),
        }
    }
}

/// The package manager that runs the scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageManager {
    pub fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [Self::Npm, Self::Pnpm, Self::Yarn, Self::Bun]
            .into_iter()
            .find(|pm| pm.name() == name)
    }

    /// The command line that runs `script` with `args`.
    fn run(self, script: &str, args: &str) -> String {
        let line = match (self, args.is_empty()) {
            (Self::Npm, _) if script == "start" && args.is_empty() => "npm start".to_owned(),
            (Self::Npm, true) => format!("npm run {script}"),
            (Self::Npm, false) => format!("npm run {script} -- {args}"),
            (Self::Pnpm, _) => format!("pnpm run {script} {args}"),
            (Self::Yarn, _) => format!("yarn {script} {args}"),
            (Self::Bun, _) => format!("bun run {script} {args}"),
        };
        line.trim_end().to_owned()
    }

    /// The workflow steps that set up Node and install the dependencies.
    /// pnpm's setup action needs a version unless `packageManager` names
    /// one (`declared`), and fails if both do.
    fn ci_steps(self, declared: bool) -> &'static str {
        match self {
            Self::Pnpm if !declared => {
                "      - uses: pnpm/action-setup@v4\n        with:\n          version: latest\n      - uses: actions/setup-node@v4\n        with:\n          node-version: lts/*\n          cache: pnpm\n      - run: pnpm install --frozen-lockfile\n"
            }
            Self::Npm => {
                "      - uses: actions/setup-node@v4\n        with:\n          node-version: lts/*\n          cache: npm\n      - run: npm ci\n"
            }
            Self::Pnpm => {
                "      - uses: pnpm/action-setup@v4\n      - uses: actions/setup-node@v4\n        with:\n          node-version: lts/*\n          cache: pnpm\n      - run: pnpm install --frozen-lockfile\n"
            }
            Self::Yarn => {
                "      - uses: actions/setup-node@v4\n        with:\n          node-version: lts/*\n          cache: yarn\n      - run: yarn install --frozen-lockfile\n"
            }
            Self::Bun => {
                "      - uses: oven-sh/setup-bun@v2\n      - run: bun install --frozen-lockfile\n"
            }
        }
    }
}

/// `--framework`: find it, none, or this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameworkChoice {
    Detect,
    None,
    Is(Framework),
}

/// The parts of `package.json` `init` reads.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Package {
    #[serde(default)]
    dependencies: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    dev_dependencies: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    scripts: BTreeMap<String, serde_json::Value>,
    /// `packageManager`: `pnpm@9.1.0`.
    #[serde(rename = "packageManager")]
    manager: Option<String>,
}

impl Package {
    fn depends_on(&self, name: &str) -> bool {
        self.dependencies.contains_key(name) || self.dev_dependencies.contains_key(name)
    }
}

/// What the project is: its framework and package manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Project {
    pub framework: Option<Framework>,
    /// `None` without a `package.json`.
    pub package_manager: Option<PackageManager>,
}

/// Scaffolds the E2E layout in `dir`.
pub fn run(dir: &Path, choice: FrameworkChoice) -> Result<InitData, ErrorInfo> {
    let dir = std::path::absolute(dir).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot resolve {}: {e}", dir.display()),
        )
    })?;
    if !dir.is_dir() {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{} is not a directory", dir.display()),
        )
        .hint("create it, or pass the project's directory with --dir"));
    }
    let package = read_package(&dir)?;
    let project = detect(&dir, package.as_ref(), choice);
    let pm = project.package_manager.unwrap_or(PackageManager::Npm);
    let server = project
        .framework
        .map(|f| web_server(f, pm, package.as_ref()));

    let mut files = Vec::new();
    files.push(config_file(&dir, server.as_ref())?);
    for (path, text) in [
        (SCENARIO_PATH, SCENARIO),
        (MOCKS_PATH, MOCKS),
        (DATA_PATH, DATA),
    ] {
        files.push(new_file(&dir, path, text)?);
    }
    let declared = package.as_ref().is_some_and(|p| p.manager.is_some());
    let node = package
        .as_ref()
        .and(project.package_manager)
        .map_or("", |pm| pm.ci_steps(declared));
    files.push(new_file(&dir, WORKFLOW_PATH, &workflow(node))?);
    files.push(gitignore(&dir)?);

    let written = files[0].action != InitAction::Kept;
    Ok(InitData {
        dir: dir.display().to_string(),
        framework: project.framework.map(|f| f.name().to_owned()),
        package_manager: project.package_manager.map(|pm| pm.name().to_owned()),
        base_url: written.then(|| {
            server
                .as_ref()
                .map_or(FALLBACK_URL.to_owned(), |s| s.url.clone())
        }),
        web_server: server.filter(|_| written).map(|s| InitWebServer {
            command: s.command,
            url: s.url,
        }),
        files,
    })
}

/// The project's `package.json`, if it has one.
fn read_package(dir: &Path) -> Result<Option<Package>, ErrorInfo> {
    let path = dir.join("package.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read {}: {e}", path.display()),
            ));
        }
    };
    serde_json::from_str(&text).map(Some).map_err(|e| {
        // serde_json's message ends with the position, given before it here.
        let message = e.to_string();
        let message = message
            .rsplit_once(" at line ")
            .map_or(message.as_str(), |(m, _)| m);
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{}:{}:{}: {message}", path.display(), e.line(), e.column()),
        )
        .hint("fix package.json; init reads its dependencies and scripts")
    })
}

/// The framework and package manager of the project in `dir`.
fn detect(dir: &Path, package: Option<&Package>, choice: FrameworkChoice) -> Project {
    let framework = match choice {
        FrameworkChoice::Is(f) => Some(f),
        FrameworkChoice::None => None,
        FrameworkChoice::Detect => package.and_then(|p| {
            Framework::ALL
                .into_iter()
                .find(|f| p.depends_on(f.dependency()))
        }),
    };
    let declared = package
        .and_then(|p| p.manager.as_deref())
        .and_then(|pm| PackageManager::from_name(pm.split('@').next().unwrap_or_default()));
    let locked = [
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
        ("package-lock.json", PackageManager::Npm),
    ]
    .into_iter()
    .find(|(file, _)| dir.join(file).is_file())
    .map(|(_, pm)| pm);
    let package_manager = declared
        .or(locked)
        .or((package.is_some() || framework.is_some()).then_some(PackageManager::Npm));
    Project {
        framework,
        package_manager,
    }
}

/// The `[e2e.web_server]` to write.
#[derive(Debug)]
struct Server {
    label: &'static str,
    command: String,
    url: String,
    env: Vec<(&'static str, String)>,
}

fn web_server(framework: Framework, pm: PackageManager, package: Option<&Package>) -> Server {
    let (mut script, args) = framework.script();
    // An app whose dev server is `start` (and has no `dev`).
    if let Some(p) = package
        && !p.scripts.is_empty()
        && !p.scripts.contains_key(script)
        && p.scripts.contains_key("start")
    {
        script = "start";
    }
    let port = framework.port();
    Server {
        label: framework.label(),
        command: pm.run(script, &args),
        url: format!("http://localhost:{port}"),
        env: if framework == Framework::Cra {
            vec![("PORT", port.to_string())]
        } else {
            Vec::new()
        },
    }
}

/// The `[e2e]` section.
fn config_section(server: Option<&Server>) -> String {
    let url = server.map_or(FALLBACK_URL, |s| s.url.as_str());
    let mut text = format!(
        "# E2E tests: `chrome-cdp help config` lists every key.\n\
         [e2e]\n\
         base_url = {url}\n\
         tests = [\"e2e/scenarios/**/*.cdp\"]\n\
         mocks = \"e2e/mocks\"\n\
         default_mocks = [\"base\"]\n\
         jobs = \"auto\"\n\
         retries = 0\n\
         timeout = \"30s\"\n\
         artifacts = \"{DEFAULT_ARTIFACTS}\"\n\
         viewport = \"1280x800\"\n\
         # watch = [\"src/**\"]   # test --watch also reruns every test when these change\n\n\
         # Every test sees the same time, random values, locale and time zone,\n\
         # and no animations (docs/e2e/determinism.md). To change that:\n\
         # [e2e.determinism]\n\
         # freeze_time = \"2026-01-01T10:00:00Z\"   # or false\n\
         # random_seed = 0                        # or false\n\
         # disable_animations = true\n\
         # locale = \"en-US\"\n\
         # timezone = \"UTC\"\n\n",
        url = toml_string(url),
    );
    match server {
        Some(s) => {
            let _ = write!(
                text,
                "# {}, found in package.json. Started before the tests unless\n\
                 # something already answers on url, and stopped after them.\n\
                 [e2e.web_server]\n\
                 command = {}\n\
                 url = {}\n\
                 reuse_existing = true\n\
                 timeout = \"60s\"\n",
                s.label,
                toml_string(&s.command),
                toml_string(&s.url),
            );
            if !s.env.is_empty() {
                let pairs: Vec<String> = s
                    .env
                    .iter()
                    .map(|(k, v)| format!("{k} = {}", toml_string(v)))
                    .collect();
                let _ = writeln!(text, "env = {{ {} }}", pairs.join(", "));
            }
        }
        None => text.push_str(
            "# The command that serves the app, started before the tests unless\n\
             # something already answers on url, and stopped after them:\n\
             # [e2e.web_server]\n\
             # command = \"npm run dev\"\n\
             # url = \"http://localhost:3000\"\n",
        ),
    }
    text
}

/// A TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Writes the `[e2e]` section: into a new `chrome-cdp.toml`, or at the
/// end of the project config in `dir` if it has none.
fn config_file(dir: &Path, server: Option<&Server>) -> Result<InitFile, ErrorInfo> {
    let existing: Vec<PathBuf> = PROJECT_FILES
        .iter()
        .map(|name| dir.join(name))
        .filter(|p| p.is_file())
        .collect();
    let section = config_section(server);
    let path = match existing.as_slice() {
        [] => return new_file(dir, PROJECT_FILES[0], &section),
        [one] => one,
        _ => {
            return Err(crate::find_project_file(dir)
                .err()
                .unwrap_or_else(|| ErrorInfo::new(ErrorKind::BadArgs, "two project configs")));
        }
    };
    let text = read(path)?;
    let table: toml::Table = toml::from_str(&text).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "cannot add [e2e] to {}: it is not valid TOML: {}",
                path.display(),
                e.message().trim()
            ),
        )
        .hint("fix it, then run `chrome-cdp init e2e` again")
    })?;
    if table.contains_key("e2e") {
        return Ok(file(path, InitAction::Kept, Some("has an [e2e] section")));
    }
    let separator = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    append(path, &format!("{separator}{section}"))?;
    Ok(file(
        path,
        InitAction::Appended,
        Some("added an [e2e] section"),
    ))
}

/// Adds the artifacts directory to `.gitignore`.
fn gitignore(dir: &Path) -> Result<InitFile, ErrorInfo> {
    let path = dir.join(".gitignore");
    let entry = format!("{DEFAULT_ARTIFACTS}/");
    if !path.exists() {
        write_new(&path, &format!("{entry}\n"))?;
        return Ok(file(&path, InitAction::Created, Some(&entry)));
    }
    let text = read(&path)?;
    let listed = text.lines().any(|line| {
        let line = line.trim().trim_start_matches('/').trim_end_matches('/');
        line == DEFAULT_ARTIFACTS
    });
    if listed {
        return Ok(file(
            &path,
            InitAction::Kept,
            Some(&format!("ignores {entry} already")),
        ));
    }
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    append(&path, &format!("{separator}{entry}\n"))?;
    Ok(file(&path, InitAction::Appended, Some(&entry)))
}

/// The GitHub Actions workflow; `node` sets up Node and the dependencies
/// when the project has a `package.json`.
fn workflow(node: &str) -> String {
    format!(
        "# E2E tests with chrome-cdp, written by `chrome-cdp init e2e`.
name: E2E

on:
  push:
    branches: [main]
  pull_request:
  # For the nightly job below, which checks the mocks against a real backend:
  # schedule:
  #   - cron: \"0 3 * * *\"

jobs:
  e2e:
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        shard: [1/2, 2/2]
    env:
      # Installs Chrome for Testing when the runner has no Chrome.
      CHROME_CDP_AUTO_INSTALL: \"1\"
    steps:
      - uses: actions/checkout@v4
{node}      - name: Install chrome-cdp
        run: cargo install --locked chrome-cdp
      - uses: actions/cache@v4
        with:
          path: ~/.cache/chrome-cdp
          key: chrome-cdp-${{{{ runner.os }}}}-${{{{ runner.arch }}}}
      - name: E2E tests
        run: chrome-cdp test --reporter text,junit --shard ${{{{ matrix.shard }}}}
      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: e2e-results-${{{{ strategy.job-index }}}}
          path: {DEFAULT_ARTIFACTS}/

  # Nightly: run the scenarios against a real backend and report where the
  # mocks no longer match it. Needs [e2e.backends.staging] in chrome-cdp.toml.
  # verify-mocks:
  #   if: github.event_name == 'schedule'
  #   runs-on: ubuntu-latest
  #   steps:
  #     - uses: actions/checkout@v4
  #     - run: cargo install --locked chrome-cdp
  #     - run: chrome-cdp test --backend staging --verify-mocks
"
    )
}

/// Writes `text` to `dir/path` unless the file exists.
fn new_file(dir: &Path, path: &str, text: &str) -> Result<InitFile, ErrorInfo> {
    let path = dir.join(path);
    if path.exists() {
        return Ok(file(&path, InitAction::Kept, Some("exists")));
    }
    write_new(&path, text)?;
    Ok(file(&path, InitAction::Created, None))
}

fn file(path: &Path, action: InitAction, reason: Option<&str>) -> InitFile {
    InitFile {
        path: path.display().to_string(),
        action,
        reason: reason.map(str::to_owned),
    }
}

fn read(path: &Path) -> Result<String, ErrorInfo> {
    std::fs::read_to_string(path).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read {}: {e}", path.display()),
        )
    })
}

fn cannot_write(path: &Path, e: &std::io::Error) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("cannot write {}: {e}", path.display()),
    )
}

/// Creates `path` (and its directories); fails if it exists, so a file
/// made meanwhile is never overwritten.
fn write_new(path: &Path, text: &str) -> Result<(), ErrorInfo> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| cannot_write(parent, &e))?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| cannot_write(path, &e))
}

fn append(path: &Path, text: &str) -> Result<(), ErrorInfo> {
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| cannot_write(path, &e))
}

#[cfg(test)]
mod tests;
