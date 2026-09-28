//! Finding, reading and checking mock files. Every problem is a `BadArgs`
//! error at `file:line:col`, found before any request is answered.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, suggest};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::file::{
    At, BodyMatch, Cors, JsonMode, RawFile, RawMatch, RawMock, RawRespond, Scalar, Unmatched,
};
use crate::glob::{Glob, PathGlob};
use crate::matcher::{BodyMatcher, Matcher, Target};
use crate::respond::{Body, Latency, Respond, is_header_name, network_error, parse_delay};
use crate::template::{self, is_template};

/// Where a mock set comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// `base`, `errors/api-down`, or the path as given.
    pub name: String,
    /// The mock file (absolute).
    pub file: PathBuf,
    /// What its data files are relative to: the mocks directory (absolute).
    pub base: PathBuf,
}

impl Source {
    /// The set `name` of mocks directory `dir`: `dir/<name>.toml`, or a
    /// path to a `.toml` file (relative to `cwd`), whose data files are
    /// then next to it. `dir` and `cwd` are absolute.
    pub fn find(dir: &Path, cwd: &Path, name: &str) -> Result<Self, ErrorInfo> {
        let name = name.trim();
        if Path::new(name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
        {
            let file = cwd.join(name);
            if !file.is_file() {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("no mock file {}", file.display()),
                ));
            }
            let base = file.parent().unwrap_or(cwd).to_path_buf();
            return Ok(Self {
                name: name.strip_suffix(".toml").unwrap_or(name).to_owned(),
                file,
                base,
            });
        }
        let valid = !name.is_empty()
            && !name.starts_with('/')
            && name
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != "..");
        let file = dir.join(format!("{name}.toml"));
        if valid && file.is_file() {
            return Ok(Self {
                name: name.to_owned(),
                file,
                base: dir.to_path_buf(),
            });
        }
        let sets = available(dir);
        let err = ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("no mock set `{name}` in {}", dir.display()),
        );
        let close = suggest::similar(name, sets.iter().map(String::as_str), 3);
        Err(match close.first() {
            Some(first) => err
                .note("did you mean", close.join(", "))
                .hint(Hint::run(Cmd::new("mock").arg("use").arg(*first))),
            None if sets.is_empty() => err.hint(format!(
                "write {} (see `chrome-cdp mock --help`), or run `chrome-cdp init e2e`",
                file.display()
            )),
            None => err.note("sets", sets.join(", ")).hint(
                Hint::run(Cmd::new("mock").arg("list").switch("available")).why("lists the sets"),
            ),
        })
    }
}

/// The sets in mocks directory `dir`: every `.toml` file under it, by
/// name, sorted.
pub fn available(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                if depth < 8 {
                    walk(&path, &format!("{prefix}{name}/"), out, depth + 1);
                }
            } else if let Some(stem) = name.strip_suffix(".toml") {
                out.push(format!("{prefix}{stem}"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out, 0);
    out.sort();
    out
}

/// A mock set, checked and compiled.
#[derive(Debug, Clone)]
pub struct MockSet {
    pub(crate) source: Source,
    /// The file's modification time when it was read.
    pub(crate) modified: Option<SystemTime>,
    pub(crate) scope: Vec<PathGlob>,
    pub(crate) scope_text: Vec<String>,
    pub(crate) unmatched: Unmatched,
    pub(crate) cors: Cors,
    pub(crate) latency: Latency,
    pub(crate) mocks: Vec<Mock>,
    /// Why the file's latest change was not loaded.
    pub(crate) reload_error: Option<String>,
}

/// A mock, checked and compiled.
#[derive(Debug, Clone)]
pub(crate) struct Mock {
    pub(crate) name: String,
    pub(crate) line: usize,
    pub(crate) matcher: Matcher,
    pub(crate) respond: Respond,
    /// `delay`: replaces the set's latency.
    pub(crate) delay: Option<std::time::Duration>,
    pub(crate) hits: u64,
}

impl MockSet {
    /// Reads and checks the mock file of `source`.
    pub fn load(source: &Source) -> Result<Self, ErrorInfo> {
        let file = &source.file;
        let modified = std::fs::metadata(file).and_then(|m| m.modified()).ok();
        let text = std::fs::read_to_string(file).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read mock file {}: {e}", file.display()),
            )
        })?;
        let mut set = Self::parse(&text, source)?;
        set.modified = modified;
        Ok(set)
    }

    /// Checks mock file text `text` of `source`.
    pub fn parse(text: &str, source: &Source) -> Result<Self, ErrorInfo> {
        let file = &source.file;
        let at = |span: Range<usize>, message: String| {
            let (line, col) = line_col(text, span.start);
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{}:{line}:{col}: {message}", file.display()),
            )
            .hint(format!(
                "fix it in {}; `chrome-cdp mock --help` shows the format, `chrome-cdp schema mocks` its JSON Schema",
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
        let check = Check {
            text,
            base: &source.base,
        };
        check.set(raw, source).map_err(|(span, m)| at(span, m))
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.source.name
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Whether `url`'s request is in the set's scope; `path` is the URL's
    /// path, `base` the URL without query.
    pub(crate) fn owns(&self, path: &str, base: &str) -> bool {
        self.scope.iter().any(|glob| {
            if glob.is_path() {
                glob.matches(path)
            } else {
                glob.matches(base)
            }
        })
    }

    /// `base.toml`: the file relative to its mocks directory.
    pub(crate) fn file_name(&self) -> String {
        let file = &self.source.file;
        file.strip_prefix(&self.source.base)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned()
    }
}

/// The 1-based line and column (in characters) of byte `offset`.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

type Problem = (Range<usize>, String);

struct Check<'a> {
    text: &'a str,
    base: &'a Path,
}

impl Check<'_> {
    fn set(&self, raw: RawFile, source: &Source) -> Result<MockSet, Problem> {
        let mut scope = Vec::new();
        let mut scope_text = Vec::new();
        let (mut unmatched, mut cors, mut latency) = (
            Unmatched::default(),
            Cors::default(),
            Latency::Fixed(std::time::Duration::ZERO),
        );
        if let Some(settings) = raw.settings {
            let settings = settings.into_inner();
            if let Some(globs) = settings.scope {
                let span = globs.span();
                for text in globs.into_inner().into_vec() {
                    let glob = if text.starts_with('/') {
                        PathGlob::path(&text)
                    } else {
                        PathGlob::url(&text).map_err(|_| {
                            format!("`{text}` is neither a path glob (/api/**) nor a full-URL glob (https://api.example.com/**)")
                        })
                    }
                    .map_err(|m| (span.clone(), format!("settings.scope: {m}")))?;
                    scope.push(glob);
                    scope_text.push(text);
                }
            }
            unmatched = settings.unmatched.unwrap_or_default();
            cors = settings.cors.unwrap_or_default();
            if let Some(text) = settings.latency {
                latency = Latency::parse(text.get())
                    .map_err(|m| (text.span(), format!("settings.latency: {m}")))?;
            }
        }
        let mocks = raw
            .mocks
            .into_iter()
            .map(|m| self.mock(m))
            .collect::<Result<_, _>>()?;
        Ok(MockSet {
            source: source.clone(),
            modified: None,
            scope,
            scope_text,
            unmatched,
            cors,
            latency,
            mocks,
            reload_error: None,
        })
    }

    fn mock(&self, raw: At<RawMock>) -> Result<Mock, Problem> {
        let span = raw.span();
        let (line, _) = line_col(self.text, span.start);
        let raw = raw.into_inner();
        let matcher = Self::matcher(raw.matcher)?;
        let Some(respond) = raw.respond else {
            return Err((span, "the mock has no `respond`".into()));
        };
        let (respond, delay) = self.respond(respond)?;
        let name = raw.name.unwrap_or_else(|| matcher.summary());
        Ok(Mock {
            name,
            line,
            matcher,
            respond,
            delay,
            hits: 0,
        })
    }

    fn matcher(raw: At<RawMatch>) -> Result<Matcher, Problem> {
        let span = raw.span();
        let raw = raw.into_inner();
        let mut matcher = Matcher::default();
        if let Some(methods) = raw.method {
            let span = methods.span();
            for m in methods.into_inner().into_vec() {
                let m = m.trim().to_ascii_uppercase();
                if m == "*" {
                    matcher.methods.clear();
                    break;
                }
                if m.is_empty() || !m.chars().all(|c| c.is_ascii_alphabetic()) {
                    return Err((span, format!("match.method: `{m}` is not an HTTP method")));
                }
                matcher.methods.push(m);
            }
        }
        let targets = [raw.path.is_some(), raw.url.is_some(), raw.regex.is_some()];
        if targets.iter().filter(|t| **t).count() > 1 {
            return Err((span, "match: give one of `path`, `url` and `regex`".into()));
        }
        if let Some(path) = raw.path {
            let glob = PathGlob::path(path.get())
                .map_err(|m| (path.span(), format!("match.path: {m}")))?;
            matcher.target = Some(Target::Path(glob));
        } else if let Some(url) = raw.url {
            let glob =
                PathGlob::url(url.get()).map_err(|m| (url.span(), format!("match.url: {m}")))?;
            matcher.target = Some(Target::Url(glob));
        } else if let Some(regex) = raw.regex {
            let re =
                Regex::new(regex.get()).map_err(|e| (regex.span(), format!("match.regex: {e}")))?;
            matcher.target = Some(Target::Regex(re));
        }
        matcher.query = globs(raw.query);
        matcher.headers = globs(raw.headers)
            .into_iter()
            .map(|(n, g)| (n.to_ascii_lowercase(), g))
            .collect();
        let bodies = [
            raw.json.is_some(),
            raw.form.is_some(),
            raw.body.is_some(),
            raw.graphql.is_some(),
        ];
        if bodies.iter().filter(|b| **b).count() > 1 {
            return Err((
                span,
                "match: give one of `json`, `form`, `body` and `graphql`".into(),
            ));
        }
        if raw.json_mode.is_some() && raw.json.is_none() {
            return Err((span, "match: `json_mode` needs `json`".into()));
        }
        if let Some(json) = raw.json {
            matcher.body = Some(BodyMatcher::Json {
                value: json.into_inner(),
                contains: raw.json_mode == Some(JsonMode::Contains),
            });
        } else if let Some(form) = raw.form {
            matcher.body = Some(BodyMatcher::Form(globs(Some(form))));
        } else if let Some(body) = raw.body {
            let span = body.span();
            matcher.body = Some(match body.into_inner() {
                BodyMatch::Glob(glob) => BodyMatcher::Glob(Glob::new(&glob)),
                BodyMatch::Regex { regex } => BodyMatcher::Regex(
                    Regex::new(&regex).map_err(|e| (span, format!("match.body.regex: {e}")))?,
                ),
            });
        } else if let Some(gql) = raw.graphql {
            let gql = gql.into_inner();
            matcher.body = Some(BodyMatcher::Graphql {
                operation: gql.operation,
                variables: gql.variables,
            });
        }
        let empty = matcher.methods.is_empty()
            && matcher.target.is_none()
            && matcher.query.is_empty()
            && matcher.headers.is_empty()
            && matcher.body.is_none();
        if empty {
            return Err((
                span,
                "match: no condition, so it would answer every request; use path = \"/**\" if that is meant".into(),
            ));
        }
        Ok(matcher)
    }

    fn respond(
        &self,
        raw: At<RawRespond>,
    ) -> Result<(Respond, Option<std::time::Duration>), Problem> {
        let span = raw.span();
        let raw = raw.into_inner();
        let delay = raw
            .delay
            .map(|d| parse_delay(d.get()).map_err(|m| (d.span(), format!("respond.delay: {m}"))))
            .transpose()?;
        let bodies: Vec<&str> = [
            (raw.json.is_some(), "json"),
            (raw.json_file.is_some(), "json_file"),
            (raw.body.is_some(), "body"),
            (raw.body_file.is_some(), "body_file"),
            (raw.file.is_some(), "file"),
        ]
        .into_iter()
        .filter_map(|(given, name)| given.then_some(name))
        .collect();
        if bodies.len() > 1 {
            return Err((
                span,
                format!("respond: give one body, not {}", bodies.join(" and ")),
            ));
        }
        let answers = raw.status.is_some() || raw.headers.is_some() || !bodies.is_empty();
        let passthrough = raw.passthrough.as_ref().is_some_and(|p| *p.get());
        if let Some(abort) = raw.abort {
            if answers || passthrough || raw.patch.is_some() {
                return Err((span, "respond: `abort` fails the request, so it takes no status, headers, body or passthrough".into()));
            }
            let reason = network_error(abort.get()).ok_or_else(|| {
                let names: Vec<String> = crate::NETWORK_ERRORS
                    .iter()
                    .map(|r| r.to_ascii_lowercase())
                    .collect();
                (
                    abort.span(),
                    format!(
                        "respond.abort: `{}` is not a network error; they are {}",
                        abort.get(),
                        names.join(", ")
                    ),
                )
            })?;
            return Ok((Respond::Abort(reason), delay));
        }
        if passthrough {
            if answers {
                return Err((span, "respond: `passthrough` sends the request on, so it takes no status, headers or body; use `patch` to change the JSON response".into()));
            }
            let patch = raw.patch.map(At::into_inner);
            if let Some(patch) = &patch {
                template::check_json(patch)
                    .map_err(|m| (span.clone(), format!("respond.patch: {m}")))?;
            }
            return Ok((Respond::Passthrough(patch), delay));
        }
        if let Some(patch) = raw.patch {
            return Err((
                patch.span(),
                "respond.patch: needs passthrough = true".into(),
            ));
        }
        let status = match raw.status {
            None => 200,
            Some(s) => {
                let n = *s.get();
                u16::try_from(n)
                    .ok()
                    .filter(|n| *n == 100 || (200..=599).contains(n))
                    .ok_or_else(|| {
                        (
                            s.span(),
                            format!("respond.status: {n} cannot be answered; use 100 or 200-599"),
                        )
                    })?
            }
        };
        let mut headers = Vec::new();
        for (name, value) in raw.headers.unwrap_or_default() {
            if !is_header_name(&name) {
                return Err((
                    value.span(),
                    format!("respond.headers: `{name}` is not a header name"),
                ));
            }
            template::check(value.get())
                .map_err(|m| (value.span(), format!("respond.headers.{name}: {m}")))?;
            headers.push((name, value.into_inner()));
        }
        let body = if let Some(json) = raw.json {
            template::check_json(json.get())
                .map_err(|m| (json.span(), format!("respond.json: {m}")))?;
            Body::Json(json.into_inner())
        } else if let Some(text) = raw.body {
            template::check(text.get()).map_err(|m| (text.span(), format!("respond.body: {m}")))?;
            Body::Text(text.into_inner())
        } else if let Some(path) = raw.json_file {
            self.data_file("json_file", &path, true)?;
            Body::JsonFile(path.into_inner())
        } else if let Some(path) = raw.body_file {
            self.data_file("body_file", &path, false)?;
            Body::TextFile(path.into_inner())
        } else if let Some(path) = raw.file {
            self.data_file("file", &path, false)?;
            Body::File(path.into_inner())
        } else {
            Body::Empty
        };
        Ok((
            Respond::Fulfill {
                status,
                headers,
                body,
            },
            delay,
        ))
    }

    /// Checks a data file: a templated path's syntax, or that a fixed one
    /// exists (and is JSON, for `json_file`).
    fn data_file(&self, key: &str, path: &At<String>, json: bool) -> Result<(), Problem> {
        let text = path.get();
        let problem = |m: String| (path.span(), format!("respond.{key}: {m}"));
        if text.trim().is_empty() {
            return Err(problem("empty".into()));
        }
        if Path::new(text).is_absolute() {
            return Err(problem(format!(
                "`{text}` is absolute; data files are relative to {}",
                self.base.display()
            )));
        }
        if is_template(text) {
            return template::check(text).map_err(problem);
        }
        let file = self.base.join(text);
        if !file.is_file() {
            return Err(problem(format!(
                "{} does not exist (data files are relative to {})",
                file.display(),
                self.base.display()
            )));
        }
        if json && !text.ends_with(".tmpl.json") {
            let bytes =
                std::fs::read(&file).map_err(|e| problem(format!("{}: {e}", file.display())))?;
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|e| problem(format!("{} is not JSON: {e}", file.display())))?;
        }
        Ok(())
    }
}

fn globs(map: Option<BTreeMap<String, At<Scalar>>>) -> Vec<(String, Glob)> {
    map.unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, Glob::new(&v.get().text())))
        .collect()
}
