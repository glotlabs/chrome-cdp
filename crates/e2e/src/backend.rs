//! The backend a run uses (plan 11, milestone 11.11): `mock` (built in)
//! or a real one of `[e2e.backends.<name>]`.
//!
//! [`Config::backend`] resolves one into what a run needs: the `[e2e]`
//! settings with the backend's on top. In real mode the mock sets are off
//! but for the backend's own `mocks` (hybrid mode), time is not frozen
//! unless the backend says so (real servers issue real-time tokens), and
//! a failed test is retried once unless `retries` says otherwise.
//! [`Vars`] reads the `${var:…}` files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use protocol::{ErrorInfo, ErrorKind, Hint};

use crate::config::{Config, Determinism, FreezeTime, Jobs, MOCK_BACKEND, WebServer};

/// `--backend all`: every backend in turn, `mock` first.
pub const ALL: &str = "all";

/// The environment variable that picks the backend without `--backend`.
pub const ENV: &str = "CHROME_CDP_BACKEND";

/// How a backend answers the app's requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The mock sets answer (the built-in `mock` backend).
    Mock,
    /// A real server answers; only the backend's `mocks` are on.
    Real,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Real => "real",
        }
    }
}

/// A backend, resolved: the settings a run against it uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Selected {
    pub name: String,
    pub mode: Mode,
    /// What `goto /path` is relative to.
    pub base_url: Option<String>,
    pub web_servers: Vec<WebServer>,
    /// Shell commands run before the suite, and after it.
    pub setup: Vec<String>,
    pub teardown: Vec<String>,
    /// The `${var:…}` files, later ones winning.
    pub vars: Vec<PathBuf>,
    pub jobs: Jobs,
    pub timeout: Duration,
    /// Retries of a test without `@retries`.
    pub retries: u32,
    pub determinism: Determinism,
    /// Mock sets every test gets before its own `@mocks`: `default_mocks`
    /// in mock mode; in real mode the backend's `mocks` (hybrid mode),
    /// and the tests' `@mocks` are not applied.
    pub mocks: Vec<String>,
    /// Writes need `@safe` tests or `--allow-writes`.
    pub protected: bool,
}

impl Selected {
    /// Real mode with mock sets on.
    pub fn hybrid(&self) -> bool {
        self.mode == Mode::Real && !self.mocks.is_empty()
    }
}

impl Config {
    /// Every backend's name: `mock`, then the real ones in order.
    pub fn backend_names(&self) -> Vec<String> {
        std::iter::once(MOCK_BACKEND.to_owned())
            .chain(self.backends.keys().cloned())
            .collect()
    }

    /// The backend `name` (`mock` or a real one), resolved.
    pub fn backend(&self, name: &str) -> Result<Selected, ErrorInfo> {
        let vars: Vec<PathBuf> = self.vars.iter().cloned().collect();
        if name == MOCK_BACKEND {
            return Ok(Selected {
                name: name.to_owned(),
                mode: Mode::Mock,
                base_url: self.base_url.clone(),
                web_servers: self.web_servers.clone(),
                setup: Vec::new(),
                teardown: Vec::new(),
                vars,
                jobs: self.jobs,
                timeout: self.timeout,
                retries: self.retries,
                determinism: self.determinism.clone(),
                mocks: self.default_mocks.clone(),
                protected: false,
            });
        }
        let Some(backend) = self.backends.get(name) else {
            return Err(self.unknown_backend(name));
        };
        let real_defaults = Determinism {
            freeze_time: Some(FreezeTime::Off),
            ..Determinism::default()
        };
        let determinism = self
            .determinism
            .with(&real_defaults)
            .with(&backend.determinism);
        Ok(Selected {
            name: name.to_owned(),
            mode: Mode::Real,
            base_url: backend.base_url.clone().or_else(|| self.base_url.clone()),
            web_servers: backend
                .web_servers
                .clone()
                .unwrap_or_else(|| self.web_servers.clone()),
            setup: backend.setup.clone(),
            teardown: backend.teardown.clone(),
            vars: vars.into_iter().chain(backend.vars.clone()).collect(),
            jobs: backend.jobs.unwrap_or(self.jobs),
            timeout: backend.timeout.unwrap_or(self.timeout),
            retries: backend.retries.unwrap_or(self.retries.max(1)),
            determinism,
            mocks: backend.mocks.clone(),
            protected: backend.protected,
        })
    }

    /// The error for `--backend <name>` that names no backend.
    pub fn unknown_backend(&self, name: &str) -> ErrorInfo {
        let names = self.backend_names();
        let close = protocol::suggest::similar(name, names.iter().map(String::as_str), 1);
        let info = ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("no backend `{name}` in {}", self.file.display()),
        )
        .note("backends", format!("{}, or {ALL}", names.join(", ")));
        match close.first() {
            Some(known) => info.hint(Hint::text(format!("did you mean --backend {known}?"))),
            None => info.hint(Hint::text(format!(
                "add [e2e.backends.{name}] with mode = \"real\" to {}",
                self.file.display()
            ))),
        }
    }
}

/// The values of `${var:…}`: the backend's vars files, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vars {
    pub values: BTreeMap<String, String>,
    /// The files, as given (some may not exist).
    pub files: Vec<PathBuf>,
}

impl Vars {
    /// Reads `files` in order, later values winning. A file that does not
    /// exist is skipped (a backend may have no vars yet); one that does
    /// not parse is an error.
    pub fn load(files: &[PathBuf]) -> Result<Self, ErrorInfo> {
        let mut values = BTreeMap::new();
        for file in files {
            let text = match std::fs::read_to_string(file) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("cannot read {}: {e}", file.display()),
                    ));
                }
            };
            values.extend(parse(&text, file)?);
        }
        Ok(Self {
            values,
            files: files.to_vec(),
        })
    }

    /// The files that exist, for messages: `e2e/vars/local.toml`.
    pub fn describe(&self) -> String {
        let existing: Vec<String> = self
            .files
            .iter()
            .filter(|f| f.is_file())
            .map(|f| f.display().to_string())
            .collect();
        if existing.is_empty() {
            match self.files.last() {
                Some(file) => format!("{} (which does not exist)", file.display()),
                None => "no vars file (set vars in [e2e] or the backend)".to_owned(),
            }
        } else {
            existing.join(" and ")
        }
    }
}

/// A vars file: a flat TOML table of strings, numbers and booleans.
fn parse(text: &str, file: &Path) -> Result<BTreeMap<String, String>, ErrorInfo> {
    let bad = |message: String| {
        ErrorInfo::new(ErrorKind::BadArgs, format!("{}: {message}", file.display())).hint(
            Hint::text("a vars file is `name = \"value\"` lines (strings, numbers or booleans)"),
        )
    };
    let table: toml::Table =
        toml::from_str(text).map_err(|e| bad(e.message().trim().to_owned()))?;
    table
        .into_iter()
        .map(|(name, value)| {
            if !protocol::script::is_name(&name) {
                return Err(bad(format!(
                    "`{name}` is not a variable name (letters, digits, _ and -)"
                )));
            }
            let text = match value {
                toml::Value::String(s) => s,
                toml::Value::Integer(n) => n.to_string(),
                toml::Value::Float(n) => n.to_string(),
                toml::Value::Boolean(b) => b.to_string(),
                other => {
                    return Err(bad(format!(
                        "{name}: a {} is not a value ${{var:{name}}} can have",
                        other.type_str()
                    )));
                }
            };
            Ok((name, text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        Config::parse(text, Path::new("/p/chrome-cdp.toml"))
            .unwrap()
            .unwrap()
    }

    const PROJECT: &str = r#"
[e2e]
base_url = "http://localhost:5173"
default_mocks = ["base"]
retries = 0
vars = "e2e/vars/mock.toml"
web_server = { command = "npm run dev", url = "http://localhost:5173" }

[e2e.determinism]
freeze_time = "2026-01-01T10:00:00Z"

[e2e.backends.local]
mode = "real"
base_url = "http://localhost:8080"
web_server = { command = "docker compose up", url = "http://localhost:8080" }
setup = ["./reset-db.sh"]
vars = "e2e/vars/local.toml"
mocks = ["third-party/stripe"]

[e2e.backends.staging]
mode = "real"
base_url = "https://staging.example.com"
retries = 3
jobs = 2
protected = true
determinism = { freeze_time = "2026-02-02T00:00:00Z" }
"#;

    #[test]
    fn the_mock_backend_is_the_e2e_settings() {
        let c = config(PROJECT);
        let mock = c.backend("mock").unwrap();
        assert_eq!(mock.mode, Mode::Mock);
        assert_eq!(mock.base_url.as_deref(), Some("http://localhost:5173"));
        assert_eq!(mock.mocks, ["base"]);
        assert_eq!(mock.retries, 0);
        assert_eq!(mock.vars, [PathBuf::from("/p/e2e/vars/mock.toml")]);
        assert_eq!(
            mock.determinism.resolve().freeze_time.as_deref(),
            Some("2026-01-01T10:00:00Z")
        );
        assert!(!mock.hybrid());
    }

    #[test]
    fn a_real_backend_overrides_and_relaxes() {
        let c = config(PROJECT);
        let local = c.backend("local").unwrap();
        assert_eq!(local.mode, Mode::Real);
        assert_eq!(local.base_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(local.web_servers[0].command, "docker compose up");
        assert_eq!(local.setup, ["./reset-db.sh"]);
        assert_eq!(
            local.vars,
            [
                PathBuf::from("/p/e2e/vars/mock.toml"),
                PathBuf::from("/p/e2e/vars/local.toml")
            ]
        );
        // Real servers issue real-time tokens: time runs, but random
        // values stay seeded and animations off.
        let fixed = local.determinism.resolve();
        assert_eq!(fixed.freeze_time, None);
        assert_eq!(fixed.random_seed, Some(0));
        assert!(fixed.disable_animations);
        assert_eq!(local.retries, 1);
        assert_eq!(local.mocks, ["third-party/stripe"]);
        assert!(local.hybrid());

        let staging = c.backend("staging").unwrap();
        assert_eq!(staging.retries, 3);
        assert_eq!(staging.jobs.count(), 2);
        assert!(staging.protected);
        assert!(staging.mocks.is_empty() && !staging.hybrid());
        // A backend may freeze time itself; it keeps the [e2e] web server.
        assert_eq!(
            staging.determinism.resolve().freeze_time.as_deref(),
            Some("2026-02-02T00:00:00Z")
        );
        assert_eq!(staging.web_servers[0].command, "npm run dev");
        assert_eq!(c.backend_names(), ["mock", "local", "staging"]);
    }

    #[test]
    fn an_unknown_backend_suggests_one() {
        let c = config(PROJECT);
        let e = c.backend("stagin").unwrap_err();
        assert_eq!(e.message, "no backend `stagin` in /p/chrome-cdp.toml");
        assert_eq!(e.hint.as_deref(), Some("did you mean --backend staging?"));
        assert_eq!(e.notes[0].text, "mock, local, staging, or all");
    }

    #[test]
    fn vars_files_layer_and_skip_missing_ones() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("mock.toml");
        let b = dir.path().join("local.toml");
        std::fs::write(&a, "product = \"Mechanical keyboard\"\nprice = 49\n").unwrap();
        std::fs::write(&b, "product = \"Real keyboard\"\nsale = true\n").unwrap();
        let vars = Vars::load(&[a.clone(), b, dir.path().join("none.toml")]).unwrap();
        assert_eq!(vars.values["product"], "Real keyboard");
        assert_eq!(vars.values["price"], "49");
        assert_eq!(vars.values["sale"], "true");

        std::fs::write(&a, "[nested]\nx = 1\n").unwrap();
        let e = Vars::load(std::slice::from_ref(&a)).unwrap_err();
        assert!(
            e.message
                .ends_with("nested: a table is not a value ${var:nested} can have")
        );
        std::fs::write(&a, "x = \n").unwrap();
        assert!(Vars::load(&[a]).is_err());
    }
}
