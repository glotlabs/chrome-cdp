//! Config files (plan 08 §4).
//!
//! Two optional TOML files set defaults for the global flags:
//!
//! - the user config: `$XDG_CONFIG_HOME/chrome-cdp/config.toml`, else
//!   `~/.config/chrome-cdp/config.toml` (macOS:
//!   `~/Library/Application Support/chrome-cdp/config.toml`);
//! - the project config: the nearest `chrome-cdp.toml` or `.chrome-cdp.toml`
//!   walking up from the current directory, so a repo can pin its
//!   `base_url`, viewport, ….
//!
//! A flag beats its `CHROME_CDP_*` env var, which beats the project config,
//! which beats the user config, which beats the built-in default. Unknown
//! keys are errors, so a typo does not silently do nothing. Later plans add
//! keys to [`File`] and apply them in [`apply`].
//!
//! `testid_attribute` (plan 04) names the attribute `testid=` locators
//! read; `CHROME_CDP_TESTID_ATTRIBUTE` beats it. `snap_after_action`
//! (plan 05) is `"diff"` to have every action add `snapshot --diff -i`
//! (as `--snap` does; `--no-snap` beats it), or `"off"`.
//!
//! The project config's `[e2e]` section (plan 11) is read and checked by
//! the commands that test (`e2e::Config`), not here, so a mistake in it
//! does not stop `click`. Its `base_url` is also the `goto /path` base
//! when the file sets no top-level `base_url`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;
use clap::parser::ValueSource;
use protocol::{ErrorInfo, ErrorKind};
use serde::Deserialize;

use crate::cli::{GlobalArgs, parse_duration};

/// The keys a config file may set.
pub(crate) const KEYS: &str = "session, timeout, base_url, testid_attribute, snap_after_action";

/// One config file as written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    session: Option<String>,
    timeout: Option<String>,
    base_url: Option<String>,
    testid_attribute: Option<String>,
    snap_after_action: Option<String>,
    /// Checked by `e2e::Config`; only its `base_url` is read here.
    e2e: Option<toml::Table>,
}

/// Validated settings from one or more files.
#[derive(Debug, Default, PartialEq)]
struct Settings {
    session: Option<String>,
    timeout: Option<Duration>,
    base_url: Option<String>,
    testid_attribute: Option<String>,
    /// `snap_after_action`: `diff` is true, `off` false.
    snap_after_action: Option<bool>,
}

impl Settings {
    /// `self` with `over`'s values winning.
    fn merge(self, over: Self) -> Self {
        Self {
            session: over.session.or(self.session),
            timeout: over.timeout.or(self.timeout),
            base_url: over.base_url.or(self.base_url),
            testid_attribute: over.testid_attribute.or(self.testid_attribute),
            snap_after_action: over.snap_after_action.or(self.snap_after_action),
        }
    }
}

/// Loads the config files and fills in the global flags that were left at
/// their defaults (neither given nor set by env var).
pub(crate) fn apply(global: &mut GlobalArgs, matches: &ArgMatches) -> Result<(), ErrorInfo> {
    let cwd = std::env::current_dir().ok();
    let user = user_file()
        .map(|p| load(&p, Scope::User))
        .transpose()?
        .flatten();
    let project = cwd
        .as_deref()
        .map(e2e::find_project_file)
        .transpose()?
        .flatten()
        .map(|p| load(&p, Scope::Project))
        .transpose()?
        .flatten();
    let settings = user.unwrap_or_default().merge(project.unwrap_or_default());

    let defaulted = |id: &str| matches.value_source(id) == Some(ValueSource::DefaultValue);
    if let Some(session) = settings.session
        && defaulted("session")
    {
        global.session = session;
    }
    // `--timeout` has no clap default: each command has its own.
    if global.timeout.is_none() {
        global.timeout = settings.timeout;
    }
    global.base_url = settings.base_url;
    global.testid_attribute = match std::env::var("CHROME_CDP_TESTID_ATTRIBUTE") {
        Ok(env) if !env.trim().is_empty() => Some(check_attribute(env.trim()).map_err(|m| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("CHROME_CDP_TESTID_ATTRIBUTE: {m}"),
            )
        })?),
        _ => settings.testid_attribute,
    };
    global.snap_after_action = settings.snap_after_action.unwrap_or(false);
    Ok(())
}

/// The user config path, whether or not it exists.
fn user_file() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty());
    let dir = if let Some(xdg) = var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(var("HOME")?).join("Library/Application Support")
    } else {
        PathBuf::from(var("HOME")?).join(".config")
    };
    Some(dir.join("chrome-cdp/config.toml"))
}

/// Which config file is read.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Scope {
    User,
    Project,
}

/// Reads and validates one file; `None` if it does not exist.
fn load(path: &Path, scope: Scope) -> Result<Option<Settings>, ErrorInfo> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(bad(path, &format!("cannot read it: {e}"))),
    };
    parse(&text, scope).map(Some).map_err(|msg| bad(path, &msg))
}

fn parse(text: &str, scope: Scope) -> Result<Settings, String> {
    let file: File = toml::from_str(text).map_err(|e| {
        // toml's message is multi-line with a source excerpt; the first line
        // plus the key is enough for a one-line error.
        e.message().trim().to_owned()
    })?;
    let e2e_base_url = match &file.e2e {
        Some(_) if scope == Scope::User => {
            return Err(format!(
                "[e2e] belongs in the project's {}, not the user config",
                e2e::config::PROJECT_FILES[0]
            ));
        }
        Some(e2e) => e2e
            .get("base_url")
            .and_then(toml::Value::as_str)
            .map(str::to_owned),
        None => None,
    };
    let base_url = file.base_url.or(e2e_base_url);
    let timeout = file
        .timeout
        .map(|t| parse_duration(&t).map_err(|e| format!("timeout: {e}")))
        .transpose()?;
    if let Some(url) = &base_url
        && !["http://", "https://", "file://"]
            .iter()
            .any(|scheme| url.starts_with(scheme))
    {
        return Err(format!(
            "base_url: `{url}` is not an absolute http(s) or file URL"
        ));
    }
    let testid_attribute = file
        .testid_attribute
        .map(|a| check_attribute(&a).map_err(|m| format!("testid_attribute: {m}")))
        .transpose()?;
    let snap_after_action = match file.snap_after_action.as_deref() {
        None => None,
        Some("diff") => Some(true),
        Some("off") => Some(false),
        Some(other) => {
            return Err(format!(
                "snap_after_action: `{other}` is not \"diff\" or \"off\""
            ));
        }
    };
    Ok(Settings {
        session: file.session,
        timeout,
        base_url,
        testid_attribute,
        snap_after_action,
    })
}

/// An HTML attribute name, e.g. `data-qa`.
fn check_attribute(name: &str) -> Result<String, String> {
    let ok = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'));
    if ok {
        Ok(name.to_owned())
    } else {
        Err(format!("`{name}` is not an attribute name (e.g. data-qa)"))
    }
}

fn bad(path: &Path, message: &str) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("config {}: {message}", path.display()),
    )
    .hint(format!(
        "fix or remove the key in {}; known keys: {KEYS}, and the [e2e] section",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Scope, Settings};

    fn parse(text: &str) -> Result<Settings, String> {
        super::parse(text, Scope::Project)
    }

    #[test]
    fn parses_known_keys() {
        let s =
            parse("session = \"shop\"\ntimeout = \"5s\"\nbase_url = \"http://localhost:3000\"\n")
                .unwrap();
        assert_eq!(
            s,
            Settings {
                session: Some("shop".into()),
                timeout: Some(Duration::from_secs(5)),
                base_url: Some("http://localhost:3000".into()),
                testid_attribute: None,
                snap_after_action: None,
            }
        );
        assert_eq!(
            parse("snap_after_action = \"diff\"")
                .unwrap()
                .snap_after_action,
            Some(true)
        );
        assert_eq!(
            parse("snap_after_action = \"off\"")
                .unwrap()
                .snap_after_action,
            Some(false)
        );
        assert_eq!(
            parse("testid_attribute = \"data-qa\"")
                .unwrap()
                .testid_attribute
                .as_deref(),
            Some("data-qa")
        );
        assert_eq!(parse("").unwrap(), Settings::default());
    }

    #[test]
    fn rejects_typos_and_bad_values() {
        let err = parse("sesion = \"x\"").unwrap_err();
        assert!(err.starts_with("unknown field `sesion`"), "{err}");
        assert_eq!(
            parse("timeout = \"5000\"").unwrap_err(),
            "timeout: `5000` needs a unit; use e.g. 5000s or 5000ms"
        );
        assert!(parse("timeout = 5").unwrap_err().contains("string"));
        assert_eq!(
            parse("base_url = \"/login\"").unwrap_err(),
            "base_url: `/login` is not an absolute http(s) or file URL"
        );
        assert_eq!(
            parse("snap_after_action = \"full\"").unwrap_err(),
            "snap_after_action: `full` is not \"diff\" or \"off\""
        );
        assert_eq!(
            parse("testid_attribute = \"data qa\"").unwrap_err(),
            "testid_attribute: `data qa` is not an attribute name (e.g. data-qa)"
        );
    }

    #[test]
    fn project_wins_over_user() {
        let user = parse("session = \"u\"\ntimeout = \"1s\"").unwrap();
        let project = parse("session = \"p\"").unwrap();
        let s = user.merge(project);
        assert_eq!(s.session.as_deref(), Some("p"));
        assert_eq!(s.timeout, Some(Duration::from_secs(1)));
    }

    #[test]
    fn the_e2e_base_url_is_the_goto_base_too() {
        let e2e = "[e2e]\nbase_url = \"http://localhost:5173\"\nretries = 1\n";
        assert_eq!(
            parse(e2e).unwrap().base_url.as_deref(),
            Some("http://localhost:5173")
        );
        let both = format!("base_url = \"http://localhost:3000\"\n{e2e}");
        assert_eq!(
            parse(&both).unwrap().base_url.as_deref(),
            Some("http://localhost:3000")
        );
        // The rest of [e2e] is `e2e::Config`'s to check.
        assert!(parse("[e2e]\nretires = 1\n").is_ok());
        assert_eq!(
            super::parse(e2e, Scope::User).unwrap_err(),
            "[e2e] belongs in the project's chrome-cdp.toml, not the user config"
        );
    }
}
