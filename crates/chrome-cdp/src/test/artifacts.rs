//! What a test leaves in the artifacts directory (plan 11, milestone
//! 11.7): `<artifacts>/<file>/<test-slug>/` holds `steps.jsonl` (every
//! step: command, time, result), `requests.txt` (the request journal, with
//! the mock that answered each request) and `console.txt`; a failed test
//! adds `failure.png`, `snapshot.txt` (`snapshot -i` when it failed) and
//! `report.md` ([`super::report_md`]). With `--trace`, a screenshot after
//! every step goes to `trace/`.
//!
//! The runner reads the page through the CLI's own commands in the test's
//! context, as it runs steps; a command that fails (a crashed page, an
//! open dialog) leaves its file out, never the test's result.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use protocol::schema::{
    ConsoleData, ContextsData, MockLogData, NetworkData, NetworkEntry, RoutedRequest,
};
use protocol::{ErrorInfo, Output};
use serde::{Deserialize, Serialize};

pub(super) const STEPS: &str = "steps.jsonl";
pub(super) const REQUESTS: &str = "requests.txt";
pub(super) const CONSOLE: &str = "console.txt";
pub(super) const SCREENSHOT: &str = "failure.png";
pub(super) const SNAPSHOT: &str = "snapshot.txt";
pub(super) const REPORT: &str = "report.md";
/// `--trace`'s screenshots, `001.png` for step 1.
pub(super) const TRACE: &str = "trace";

/// How much of a step's output `steps.jsonl` keeps.
const OUTPUT_CHARS: usize = 2_000;
/// How much of the page `snapshot.txt` keeps.
const SNAPSHOT_CHARS: &str = "200000";
/// How long reading the page for the artifacts may take, per command.
const READ_TIMEOUT: &str = "5s";

/// One line of `steps.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct StepRecord {
    /// 1, 2, … in the order the steps ran (runner setup included).
    pub(super) n: usize,
    /// When it started, in ms since the test (or hook) started.
    pub(super) start_ms: u64,
    pub(super) duration_ms: u64,
    /// `file:line:col` of the step; the test's `file:line` for what the
    /// runner does around its steps.
    pub(super) at: String,
    /// The step as written.
    pub(super) command: String,
    /// `before`, `after`, `before-all`, `after-all`, `setup` or `check`;
    /// absent for the test's own steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) hook: Option<String>,
    /// The `@include` lines it came through, outermost first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) via: Vec<String>,
    pub(super) ok: bool,
    /// Its text output (cut to 2,000 characters).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<ErrorInfo>,
    /// `--trace`: the page after it, relative to the test's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) screenshot: Option<String>,
}

impl StepRecord {
    /// A record of a step that ran for `started..now` with `result`.
    pub(super) fn new(
        at: String,
        command: String,
        hook: Option<String>,
        via: Vec<String>,
        result: Result<&Output, &ErrorInfo>,
    ) -> Self {
        let (ok, output, warnings, error) = match result {
            Ok(out) => (
                true,
                cut(&out.text, OUTPUT_CHARS),
                out.warnings.clone(),
                None,
            ),
            Err(e) => (false, String::new(), Vec::new(), Some(e.clone())),
        };
        Self {
            n: 0,
            start_ms: 0,
            duration_ms: 0,
            at,
            command,
            hook,
            via,
            ok,
            output,
            warnings,
            error,
            screenshot: None,
        }
    }
}

/// Writes one test's (or hook's) `steps.jsonl` as its steps run, and keeps
/// them for its report.
pub(super) struct Recorder {
    pub(super) dir: PathBuf,
    started: Instant,
    steps: Vec<StepRecord>,
    file: Option<BufWriter<File>>,
    trace: bool,
}

impl Recorder {
    /// Starts `dir` over: what an earlier run left there is removed.
    pub(super) fn new(dir: PathBuf, trace: bool) -> Self {
        let file = fresh_dir(&dir)
            .and_then(|()| File::create(dir.join(STEPS)))
            .map(BufWriter::new)
            .map_err(|e| tracing::warn!("cannot write {}: {e}", dir.display()))
            .ok();
        Self {
            dir,
            started: Instant::now(),
            steps: Vec::new(),
            file,
            trace,
        }
    }

    /// Whether every step gets a screenshot (`--trace`).
    pub(super) fn trace(&self) -> bool {
        self.trace
    }

    /// Where the next step's `--trace` screenshot goes, and its name
    /// relative to the directory.
    pub(super) fn next_screenshot(&self) -> (PathBuf, String) {
        let name = format!("{TRACE}/{:03}.png", self.steps.len() + 1);
        (self.dir.join(&name), name)
    }

    /// Adds a step that started at `started` and took `took`.
    pub(super) fn record(&mut self, mut step: StepRecord, started: Instant, took: Duration) {
        step.n = self.steps.len() + 1;
        step.start_ms = millis(started.saturating_duration_since(self.started));
        step.duration_ms = millis(took);
        if let Some(file) = &mut self.file {
            let line = serde_json::to_string(&step).unwrap_or_default();
            let line = crate::output::mask(&line);
            if let Err(e) = writeln!(file, "{line}").and_then(|()| file.flush()) {
                tracing::warn!("cannot write {}: {e}", self.dir.join(STEPS).display());
                self.file = None;
            }
        }
        self.steps.push(step);
    }

    pub(super) fn steps(&self) -> &[StepRecord] {
        &self.steps
    }
}

/// Removes what `dir` holds and makes it again.
pub(super) fn fresh_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(dir)
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// `text` cut to `max` characters, with `…` at the cut.
pub(super) fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// Runs a CLI command in the test's context.
pub(super) type Invoke<'a> = dyn Fn(&[&str]) -> Result<Output, ErrorInfo> + 'a;

/// The page when a test failed.
#[derive(Debug)]
pub(super) struct Capture {
    /// `failure.png`, or why there is none.
    pub(super) screenshot: Result<PathBuf, String>,
    /// `snapshot -i`'s text, or why there is none.
    pub(super) snapshot: Result<String, String>,
}

/// Writes `failure.png` and `snapshot.txt` into `dir`.
pub(super) fn capture(dir: &Path, invoke: &Invoke<'_>) -> Capture {
    let png = dir.join(SCREENSHOT);
    let path = png.to_string_lossy().into_owned();
    let screenshot = invoke(&["screenshot", "-o", &path, "--timeout", READ_TIMEOUT])
        .map(|_| png)
        .map_err(|e| e.message);
    let snapshot = invoke(&[
        "snapshot",
        "-i",
        "--max-chars",
        SNAPSHOT_CHARS,
        "--timeout",
        READ_TIMEOUT,
    ])
    .map(|out| out.text)
    .map_err(|e| e.message);
    if let Ok(text) = &snapshot {
        write(&dir.join(SNAPSHOT), text);
    }
    Capture {
        screenshot,
        snapshot,
    }
}

/// What the report takes from the test's logs.
#[derive(Debug, Default)]
pub(super) struct Logs {
    /// Console errors, one line each, oldest first.
    pub(super) errors: Vec<String>,
    /// Requests that failed, got a status of 400 or more, or no mock
    /// answered; one line each.
    pub(super) bad_requests: Vec<String>,
}

/// Writes `console.txt` and `requests.txt` into `dir`, from every tab of
/// `context`.
/// `served` is the mock server's URL and its requests of the test, if
/// the run has one (plan 11.9); `requests.txt` lists them after the
/// browser's.
pub(super) fn logs(
    dir: &Path,
    context: &str,
    served: Option<(String, Vec<RoutedRequest>)>,
    invoke: &Invoke<'_>,
) -> Logs {
    let mut logs = Logs::default();
    let tabs = invoke(&["context", "list", "--max-chars", "0"])
        .ok()
        .and_then(|out| serde_json::from_value::<ContextsData>(out.data).ok())
        .and_then(|data| data.contexts.into_iter().find(|c| c.id == context))
        .map(|c| c.tabs)
        .unwrap_or_default();
    let mut console = String::new();
    let mut networks = Vec::new();
    for tab in &tabs {
        let read = &[
            "--tab",
            tab.as_str(),
            "--since",
            "0",
            "--limit",
            "0",
            "--max-chars",
            "0",
        ];
        let console_words = [&["console", "--level", "debug"][..], read].concat();
        if let Ok(out) = invoke(&console_words) {
            let _ = writeln!(console, "== {tab}");
            if !out.text.is_empty() {
                console.push_str(&out.text);
                console.push('\n');
            }
            if let Ok(data) = serde_json::from_value::<ConsoleData>(out.data) {
                logs.errors.extend(
                    data.entries
                        .iter()
                        .filter(|e| e.level == protocol::schema::Level::Error)
                        .map(|e| console_line(tab, e)),
                );
            }
        }
        if let Ok(out) = invoke(&[&["network"][..], read].concat())
            && let Ok(data) = serde_json::from_value::<NetworkData>(out.data)
        {
            networks.push(data);
        }
    }
    write(&dir.join(CONSOLE), &console);
    let routed = invoke(&["mock", "log", "--all", "--limit", "0", "--max-chars", "0"])
        .ok()
        .and_then(|out| serde_json::from_value::<MockLogData>(out.data).ok())
        .map(|data| data.log)
        .unwrap_or_default();
    let (mut text, bad) = journal(&networks, routed);
    if let Some((url, served)) = served {
        let _ = writeln!(text, "\n== mock server {url}");
        for entry in &served {
            let _ = writeln!(text, "{}", mock::text::log_line(entry));
        }
        if served.is_empty() {
            text.push_str("no requests\n");
        }
    }
    write(&dir.join(REQUESTS), &text);
    logs.bad_requests = bad;
    logs
}

fn console_line(tab: &str, e: &protocol::schema::ConsoleEntry) -> String {
    let mut line = format!("{tab} {}", e.text.lines().next().unwrap_or_default());
    if let Some(url) = &e.url {
        let _ = write!(line, " ({url}");
        if let Some(n) = e.line {
            let _ = write!(line, ":{n}");
        }
        line.push(')');
    }
    line
}

/// `requests.txt`: each tab's requests, each with the mock (or route) that
/// answered it; and the lines of the ones that went wrong.
fn journal(networks: &[NetworkData], mut routed: Vec<RoutedRequest>) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut bad = Vec::new();
    for data in networks {
        let _ = writeln!(text, "== {}", data.tab);
        for entry in &data.requests {
            // The routing journal's entry of the same request: the first
            // not taken yet with its tab, method and URL.
            let at = routed.iter().position(|r| {
                r.tab.as_deref() == Some(data.tab.as_str())
                    && r.method == entry.method
                    && r.url == entry.url
            });
            let route = at.map(|i| routed.remove(i));
            let line = request_line(entry, route.as_ref());
            let unmatched = route.as_ref().is_some_and(|r| r.unmatched.is_some());
            if unmatched || entry.error.is_some() || entry.status.is_some_and(|s| s >= 400) {
                bad.push(format!("{} {line}", data.tab));
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    // What routing saw that no tab's log has (a closed tab's, a worker's).
    if !routed.is_empty() {
        text.push_str("== routed only\n");
        for r in &routed {
            let line = format!("{} {} → {}{}", r.method, r.url, r.outcome, answered_by(r));
            if r.unmatched.is_some() {
                bad.push(line.clone());
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    (text, bad)
}

fn request_line(entry: &NetworkEntry, route: Option<&RoutedRequest>) -> String {
    let status = match (&entry.status, &entry.error) {
        (Some(status), _) => status.to_string(),
        (None, Some(_)) => "failed".to_owned(),
        (None, None) => format!("{:?}", entry.state).to_lowercase(),
    };
    let mut line = format!(
        "#{} {} {status} {} {}",
        entry.id, entry.method, entry.url, entry.kind
    );
    if let Some(ms) = entry.duration_ms {
        let _ = write!(
            line,
            " {}",
            protocol::format::duration(std::time::Duration::from_millis(ms))
        );
    }
    if let Some(error) = &entry.error {
        let _ = write!(line, " {error}");
    }
    if let Some(route) = route {
        line.push_str(&answered_by(route));
    }
    line
}

/// ` ← mock "price" (shop.toml:6)`, or the route or unmatched set.
fn answered_by(r: &RoutedRequest) -> String {
    if let Some(mock) = &r.mock {
        format!(" ← mock \"{}\" ({})", mock.name, mock.at)
    } else if let Some(route) = &r.route {
        format!(" ← route {route}")
    } else if let Some(set) = &r.unmatched {
        let mut text = format!(
            " ← unmatched in mock set {set} ({})",
            r.policy.as_deref().unwrap_or("fail")
        );
        if let Some(check) = r.closest.first() {
            let _ = write!(text, "; closest mock: {}", mock::text::closest_line(check));
        }
        text
    } else {
        String::new()
    }
}

/// Points a hint that reads the page (`chrome-cdp snapshot`, `errors`,
/// `network`, …) at the file in `dir` that holds it: the test's browser
/// context is closed when anyone reads the hint.
pub(super) fn point_hint_at_files(error: &mut ErrorInfo, dir: &Path) {
    let Some(hint) = &error.hint else {
        return;
    };
    let command = hint
        .strip_prefix(protocol::PROGRAM)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_default();
    let (file, what) = match command {
        "snapshot" | "find" | "text" | "html" => (SNAPSHOT, "the page when the test failed"),
        "errors" | "console" => (CONSOLE, "the console of every tab"),
        "network" => (REQUESTS, "every request and the mock that answered it"),
        "screenshot" => (SCREENSHOT, "the page when the test failed"),
        _ => return,
    };
    let path = dir.join(file);
    if path.exists() {
        error.hint = Some(format!(
            "see {}   ({what}; the test's page is closed: --debug keeps it open)",
            path.display()
        ));
    }
}

/// Writes `text` to `path`; a failure is only logged.
pub(super) fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(path, crate::output::mask(text).as_bytes()) {
        tracing::warn!("cannot write {}: {e}", path.display());
    }
}

/// A test name as a directory name: `Adds an item!` → `adds-an-item`.
pub(super) fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 60 {
            break;
        }
    }
    out.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_short_and_plain() {
        assert_eq!(slug("Adds an item!"), "adds-an-item");
        assert_eq!(slug("  Ünïcode & more  "), "n-code-more");
        assert_eq!(slug("!!!"), "");
        assert!(slug(&"a b ".repeat(40)).len() <= 60);
    }

    #[test]
    fn cut_keeps_whole_characters() {
        assert_eq!(cut("héllo", 2), "hé…");
        assert_eq!(cut("hi", 2), "hi");
    }
}
