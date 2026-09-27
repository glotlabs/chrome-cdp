//! Config files (plan 08 §4).
//!
//! Two optional TOML files set defaults for the global flags:
//!
//! - the user config: `$XDG_CONFIG_HOME/chrome-cdp/config.toml`, else
//!   `~/.config/chrome-cdp/config.toml` (macOS:
//!   `~/Library/Application Support/chrome-cdp/config.toml`);
//! - the project config: the nearest `.chrome-cdp.toml` walking up from the
//!   current directory, so a repo can pin its `base_url`, viewport, ….
//!
//! A flag beats its `CHROME_CDP_*` env var, which beats the project config,
//! which beats the user config, which beats the built-in default. Unknown
//! keys are errors, so a typo does not silently do nothing. Later plans add
//! keys to [`File`] and apply them in [`apply`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;
use clap::parser::ValueSource;
use protocol::{ErrorInfo, ErrorKind};
use serde::Deserialize;

use crate::cli::{GlobalArgs, parse_duration};

/// The project config's file name.
pub(crate) const PROJECT_FILE: &str = ".chrome-cdp.toml";

/// The keys a config file may set.
const KEYS: &str = "session, timeout, base_url";

/// One config file as written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    session: Option<String>,
    timeout: Option<String>,
    base_url: Option<String>,
}

/// Validated settings from one or more files.
#[derive(Debug, Default, PartialEq)]
struct Settings {
    session: Option<String>,
    timeout: Option<Duration>,
    base_url: Option<String>,
}

impl Settings {
    /// `self` with `over`'s values winning.
    fn merge(self, over: Self) -> Self {
        Self {
            session: over.session.or(self.session),
            timeout: over.timeout.or(self.timeout),
            base_url: over.base_url.or(self.base_url),
        }
    }
}

/// Loads the config files and fills in the global flags that were left at
/// their defaults (neither given nor set by env var).
pub(crate) fn apply(global: &mut GlobalArgs, matches: &ArgMatches) -> Result<(), ErrorInfo> {
    let cwd = std::env::current_dir().ok();
    let user = user_file().map(|p| load(&p)).transpose()?.flatten();
    let project = cwd
        .as_deref()
        .and_then(project_file)
        .map(|p| load(&p))
        .transpose()?
        .flatten();
    let settings = user.unwrap_or_default().merge(project.unwrap_or_default());

    let defaulted = |id: &str| matches.value_source(id) == Some(ValueSource::DefaultValue);
    if let Some(session) = settings.session
        && defaulted("session")
    {
        global.session = session;
    }
    if let Some(timeout) = settings.timeout
        && defaulted("timeout")
    {
        global.timeout = timeout;
    }
    global.base_url = settings.base_url;
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

/// The nearest `.chrome-cdp.toml` in `dir` or its ancestors.
fn project_file(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .map(|d| d.join(PROJECT_FILE))
        .find(|p| p.is_file())
}

/// Reads and validates one file; `None` if it does not exist.
fn load(path: &Path) -> Result<Option<Settings>, ErrorInfo> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(bad(path, &format!("cannot read it: {e}"))),
    };
    parse(&text).map(Some).map_err(|msg| bad(path, &msg))
}

fn parse(text: &str) -> Result<Settings, String> {
    let file: File = toml::from_str(text).map_err(|e| {
        // toml's message is multi-line with a source excerpt; the first line
        // plus the key is enough for a one-line error.
        e.message().trim().to_owned()
    })?;
    let timeout = file
        .timeout
        .map(|t| parse_duration(&t).map_err(|e| format!("timeout: {e}")))
        .transpose()?;
    if let Some(url) = &file.base_url
        && !["http://", "https://", "file://"]
            .iter()
            .any(|scheme| url.starts_with(scheme))
    {
        return Err(format!(
            "base_url: `{url}` is not an absolute http(s) or file URL"
        ));
    }
    Ok(Settings {
        session: file.session,
        timeout,
        base_url: file.base_url,
    })
}

fn bad(path: &Path, message: &str) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("config {}: {message}", path.display()),
    )
    .hint(format!(
        "fix or remove the key in {}; known keys: {KEYS}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Settings, parse, project_file};

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
            }
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
    fn project_file_is_found_walking_up() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(project_file(&deep), None);
        std::fs::write(dir.path().join("a/.chrome-cdp.toml"), "").unwrap();
        assert_eq!(
            project_file(&deep),
            Some(dir.path().join("a/.chrome-cdp.toml"))
        );
    }
}
