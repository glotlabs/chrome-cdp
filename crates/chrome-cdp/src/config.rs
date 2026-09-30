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
//! `node_stacks` (plan 15) is `open --node-stacks`' default: `"on"`,
//! `"off"` or `"localhost"`. `source_root` is the project root that
//! places in the page's code print relative to (`src/cart.ts:12`); a
//! relative path is relative to the config file's directory. Without it,
//! the root is the nearest directory up with a project config,
//! `.chrome-cdp/` or `.git` ([`project_root`]).
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
pub(crate) const KEYS: &str =
    "session, timeout, base_url, testid_attribute, snap_after_action, node_stacks, source_root";

/// One config file as written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    session: Option<String>,
    timeout: Option<String>,
    base_url: Option<String>,
    testid_attribute: Option<String>,
    snap_after_action: Option<String>,
    node_stacks: Option<String>,
    source_root: Option<String>,
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
    node_stacks: Option<engine::NodeStacks>,
    /// Absolute: a relative `source_root` is joined to the file's
    /// directory.
    source_root: Option<PathBuf>,
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
            node_stacks: over.node_stacks.or(self.node_stacks),
            source_root: over.source_root.or(self.source_root),
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
    global.node_stacks = settings.node_stacks;
    global.source_root = settings
        .source_root
        .or_else(|| cwd.as_deref().map(project_root));
    Ok(())
}

/// The project a command runs in: the nearest directory up from `cwd`
/// with a project config (`chrome-cdp.toml` or `.chrome-cdp.toml`),
/// `.chrome-cdp/` or `.git`, else `cwd`.
pub(crate) fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| {
            e2e::config::PROJECT_FILES
                .iter()
                .any(|name| dir.join(name).is_file())
                || dir.join(".chrome-cdp").is_dir()
                || dir.join(".git").exists()
        })
        .unwrap_or(cwd)
        .to_path_buf()
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
    let dir = path.parent().unwrap_or(Path::new("."));
    parse(&text, scope, dir)
        .map(Some)
        .map_err(|msg| bad(path, &msg))
}

/// Parses a config file in directory `dir`.
fn parse(text: &str, scope: Scope, dir: &Path) -> Result<Settings, String> {
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
    let node_stacks = match file.node_stacks.as_deref() {
        None => None,
        Some("on") => Some(engine::NodeStacks::On),
        Some("off") => Some(engine::NodeStacks::Off),
        Some("localhost") => Some(engine::NodeStacks::Localhost),
        Some(other) => {
            return Err(format!(
                "node_stacks: `{other}` is not \"on\", \"off\" or \"localhost\""
            ));
        }
    };
    let source_root = match file.source_root {
        Some(root) if root.trim().is_empty() => {
            return Err("source_root: empty; give a directory".to_owned());
        }
        Some(root) => {
            let root = dir.join(root.trim());
            if !root.is_dir() {
                return Err(format!(
                    "source_root: {} is not a directory",
                    root.display()
                ));
            }
            Some(root.canonicalize().unwrap_or(root))
        }
        None => None,
    };
    Ok(Settings {
        session: file.session,
        timeout,
        base_url,
        testid_attribute,
        snap_after_action,
        node_stacks,
        source_root,
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
        super::parse(text, Scope::Project, std::path::Path::new("/"))
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
                node_stacks: None,
                source_root: None,
            }
        );
        assert_eq!(
            parse("node_stacks = \"on\"").unwrap().node_stacks,
            Some(engine::NodeStacks::On)
        );
        assert_eq!(
            parse("node_stacks = \"localhost\"").unwrap().node_stacks,
            Some(engine::NodeStacks::Localhost)
        );
        assert_eq!(
            parse("node_stacks = \"all\"").unwrap_err(),
            "node_stacks: `all` is not \"on\", \"off\" or \"localhost\""
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
            super::parse(e2e, Scope::User, std::path::Path::new("/")).unwrap_err(),
            "[e2e] belongs in the project's chrome-cdp.toml, not the user config"
        );
    }

    #[test]
    fn source_root_is_relative_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("web/src")).unwrap();
        let root = super::parse("source_root = \"web\"", Scope::Project, dir.path())
            .unwrap()
            .source_root
            .unwrap();
        assert_eq!(root, dir.path().join("web").canonicalize().unwrap());
        let missing = super::parse("source_root = \"nope\"", Scope::Project, dir.path());
        assert!(missing.unwrap_err().ends_with("nope is not a directory"));
    }

    #[test]
    fn the_project_root_is_the_nearest_marked_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("app/.git")).unwrap();
        std::fs::create_dir_all(root.join("app/web/src")).unwrap();
        assert_eq!(
            super::project_root(&root.join("app/web/src")),
            root.join("app")
        );
        std::fs::write(root.join("app/web/chrome-cdp.toml"), "").unwrap();
        assert_eq!(
            super::project_root(&root.join("app/web/src")),
            root.join("app/web")
        );
        // Nothing marks a project: the directory itself.
        assert_eq!(super::project_root(root), root);
    }
}
