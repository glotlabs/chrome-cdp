//! `mock use|off|list|log|reset|explain` (plan 11, milestone 11.2).
//!
//! The CLI finds the project (the mocks directory, `default_mocks`, the
//! random seed) and the sets' files; the daemon reads and applies them.
//! `mock explain` and `mock list --available` also work without a session:
//! they read the files here.

use std::path::{Path, PathBuf};

use daemon::rpc;
use engine::{MockExplainRequest, MockListRequest, MockLogRequest, MockOffRequest, MockUseRequest};
use mock::{MockSet, Mocks, Source};
use protocol::schema::{MockExplainData, MockSetsData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde_json::json;

use crate::cli::{GlobalArgs, MockArgs, MockCommand, MockExplainArgs, MockUseArgs};
use crate::{input, route, session};

/// What the project says about mocks.
struct Project {
    /// The mocks directory (absolute).
    dir: PathBuf,
    /// `default_mocks`.
    defaults: Vec<String>,
    /// `[e2e.determinism] random_seed`, else 0.
    seed: u64,
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
            defaults: config.map(|c| c.default_mocks).unwrap_or_default(),
            seed,
            cwd,
        })
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
    }
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
    let request = MockUseRequest {
        sets: project.sources(&names)?,
        add: args.add,
        seed: project.seed,
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
fn absolute_url(url: &str, base: Option<&str>) -> Result<(String, bool), ErrorInfo> {
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
