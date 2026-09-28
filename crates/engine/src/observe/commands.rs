//! `console` and `errors` (plan 06, milestone 6.1).
//!
//! Both read the tab's console buffer (see [`super::console`]); neither
//! takes the tab's lock or counts as a page command, so they answer while
//! an action runs or a dialog is open. `errors` keeps to uncaught
//! exceptions, `console.error`s and the browser's other errors (a CSP
//! violation, a failed load from the page's own origin).
//!
//! Each tab has a read cursor per command: by default (`--since last`)
//! each shows only what it has not shown before, so repeated calls never
//! repeat output. `--limit` keeps the newest entries; the cursor then
//! moves past the older ones too, and a line says how to see them.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use protocol::schema::{ConsoleData, ConsoleEntry, Level, StackFrame};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use regex::Regex;
use tokio::time::Instant;

use super::console::Console;
use super::format::{self, frame_text};
use super::sourcemaps::{RESOLVE_LIMIT, is_local};
use crate::Engine;
use crate::input::origin;
use crate::request::{ConsoleRequest, Since, millis};
use crate::tabs::commands::path_of;
use crate::tabs::{Page, TabError, parse_tab_id, tab_name};

/// How much of an entry's text the text form shows.
const SHOWN_TEXT: usize = 500;

/// How many lines of an entry's text the text form shows.
const SHOWN_LINES: usize = 10;

/// What one call picked from the buffer.
#[derive(Debug)]
struct Selection {
    entries: Vec<ConsoleEntry>,
    /// Older matches `--limit` left out.
    skipped: usize,
    /// Entries after the starting point the buffer dropped.
    dropped: u64,
    /// Where the selection started (a sequence number).
    base: u64,
    /// The command's cursor after this call.
    cursor: u64,
    /// The console's last sequence number.
    latest: u64,
}

impl Engine {
    /// `console` and `errors` (with `req.errors`): the tab's entries after
    /// `--since`, filtered, the newest `--limit`. With `wait_ms` and
    /// nothing to show, waits that long for an entry (`--follow`).
    pub async fn console(&self, req: ConsoleRequest) -> Result<Output, ErrorInfo> {
        let grep = req.grep.as_deref().map(grep_regex).transpose()?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let id = req.tab.as_deref().map(parse_tab_id).transpose()?;
        // Not `Engine::page`: reading the console is no page command, so
        // it must not move `wait --request`'s "previous command" mark.
        let page = running
            .tabs
            .page(id, timeout)
            .await
            .map_err(|e| self.tab_error(&running, e))?;
        let frames = running.tabs.frames(page.id);
        let main_url = frames.first().map(|f| f.url.clone()).unwrap_or_default();
        let page_origin = origin(&main_url).map(str::to_owned);
        let deadline = Instant::now() + Duration::from_millis(req.wait_ms);
        let mut selection = loop {
            let selection = running
                .tabs
                .console(page.id, |console| {
                    select(
                        console,
                        &req,
                        grep.as_ref(),
                        page_origin.as_deref(),
                        now_ms(),
                    )
                })
                .ok_or_else(|| self.tab_error(&running, TabError::NoSuchTab(page.id)))?;
            if !selection.entries.is_empty() || Instant::now() >= deadline {
                break selection;
            }
            let latest = selection.latest;
            if !running.tabs.console_logged(page.id, latest, deadline).await {
                break selection;
            }
        };
        if req.sourcemaps || is_local(&main_url) {
            let documents: HashSet<String> = frames.iter().map(|f| f.url.clone()).collect();
            let frame_id = frames.first().map(|f| f.id.clone()).unwrap_or_default();
            let deadline = Instant::now() + timeout.min(RESOLVE_LIMIT);
            let mut stack: Vec<&mut StackFrame> = selection
                .entries
                .iter_mut()
                .flat_map(|e| e.stack.iter_mut())
                .collect();
            running
                .sourcemaps
                .resolve(&page.session, &frame_id, &documents, &mut stack, deadline)
                .await;
        }
        Ok(self.console_output(&page, &req, selection, page_origin.as_deref()))
    }

    fn console_output(
        &self,
        page: &Page,
        req: &ConsoleRequest,
        selection: Selection,
        page_origin: Option<&str>,
    ) -> Output {
        let tab = tab_name(page.id);
        let (one, many) = if req.errors {
            ("error", "errors")
        } else {
            ("console entry", "console entries")
        };
        let mut text = String::new();
        for entry in &selection.entries {
            entry_text(&mut text, entry, page_origin);
        }
        if selection.skipped > 0 {
            let limit = req.limit + selection.skipped;
            let cmd = self
                .console_cmd(req)
                .flag("since", since_arg(req.since, selection.base))
                .flag("limit", limit.to_string());
            let _ = writeln!(
                text,
                "… {} earlier {} not shown; {cmd} shows them",
                selection.skipped,
                if selection.skipped == 1 { one } else { many },
            );
        }
        if selection.entries.is_empty() {
            let new = if req.since == Since::Last { "new " } else { "" };
            let _ = write!(text, "no {new}{many} in {tab}");
            if req.since == Since::Last && selection.cursor > 0 {
                let cmd = self.console_cmd(req).flag("since", "0");
                let _ = write!(text, "; {cmd} shows the earlier ones");
            }
            text.push('\n');
        }
        let quiet: Vec<&str> = selection.entries.iter().map(|e| e.text.as_str()).collect();
        let quiet = quiet.join("\n");
        let mut output = Output::typed(
            &ConsoleData {
                tab: tab.clone(),
                entries: selection.entries,
                skipped: selection.skipped,
                dropped: selection.dropped,
                cursor: selection.cursor,
            },
            text.trim_end().to_owned(),
        )
        .quiet(quiet);
        if selection.dropped > 0 {
            output = output.warning(format!(
                "{} {} dropped from {tab}'s buffer before they were read; it keeps the newest {} \
                 (open --console-buffer N keeps more)",
                selection.dropped,
                if selection.dropped == 1 {
                    "console entry was"
                } else {
                    "console entries were"
                },
                self.settings.console_buffer(),
            ));
        }
        output.limit(
            req.max_chars,
            "--max-chars 0 for all of it, or --limit N for fewer",
        )
    }

    /// `console` or `errors`, with the flags of `req` that pick entries
    /// (and the session), for hints.
    fn console_cmd(&self, req: &ConsoleRequest) -> Cmd {
        let mut cmd = Cmd::new(if req.errors { "errors" } else { "console" });
        if let Some(tab) = &req.tab {
            cmd = cmd.flag("tab", tab);
        }
        if !req.errors && req.level != Level::Info {
            cmd = cmd.flag("level", req.level.as_str());
        }
        if let Some(grep) = &req.grep {
            cmd = cmd.flag("grep", grep);
        }
        cmd.session(&self.session)
    }
}

/// `--grep`'s pattern, or a usage error.
fn grep_regex(pattern: &str) -> Result<Regex, ErrorInfo> {
    Regex::new(pattern).map_err(|e| {
        let why = e.to_string();
        let why = why.lines().last().unwrap_or_default().trim();
        let why = why.strip_prefix("error: ").unwrap_or(why);
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--grep {pattern:?} is not a regular expression: {why}"),
        )
        .hint(Hint::text(
            "escape special characters with \\, e.g. --grep 'fetch\\('",
        ))
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The entries of `console` that `req` asks for, and moves the command's
/// cursor past them.
fn select(
    console: &mut Console,
    req: &ConsoleRequest,
    grep: Option<&Regex>,
    page_origin: Option<&str>,
    now_ms: u64,
) -> Selection {
    let cursor = if req.errors {
        console.cursors.errors
    } else {
        console.cursors.console
    };
    let (base, since_ms) = match req.since {
        Since::Last => (cursor, 0),
        Since::Seq { seq } => (seq, 0),
        Since::Ago { ms } => (0, now_ms.saturating_sub(ms)),
    };
    let wanted = |e: &&ConsoleEntry| {
        e.seq > base
            && e.time_ms >= since_ms
            && if req.errors {
                is_error(e, page_origin)
            } else {
                e.level <= req.level
            }
            && grep.is_none_or(|re| {
                re.is_match(&e.text) || e.url.as_deref().is_some_and(|u| re.is_match(u))
            })
    };
    let mut entries: Vec<ConsoleEntry> = console.entries().filter(wanted).cloned().collect();
    let skipped = match req.limit {
        0 => 0,
        limit => entries.len().saturating_sub(limit),
    };
    entries.drain(..skipped);
    let dropped = match req.since {
        Since::Ago { .. } => 0,
        _ => console.evicted().saturating_sub(base),
    };
    let shown = entries.last().map_or(0, |e| e.seq);
    let cursor = cursor.max(shown);
    if req.errors {
        console.cursors.errors = cursor;
    } else {
        console.cursors.console = cursor;
    }
    Selection {
        entries,
        skipped,
        dropped,
        base,
        cursor,
        latest: console.seq(),
    }
}

/// Whether `errors` shows `entry`: an error, but a failed load only from
/// the page's own origin (others are third parties' business), and not
/// the favicon Chrome asks for by itself.
fn is_error(entry: &ConsoleEntry, page_origin: Option<&str>) -> bool {
    if entry.level != Level::Error {
        return false;
    }
    if entry.source != "network" {
        return true;
    }
    entry.url.as_deref().is_some_and(|u| {
        origin(u).is_some() && origin(u) == page_origin && path_of(u) != "/favicon.ico"
    })
}

/// `--since` as typed for a hint: where a selection started.
fn since_arg(since: Since, base: u64) -> String {
    match since {
        Since::Ago { ms } => format!("{ms}ms"),
        _ => base.to_string(),
    }
}

/// A URL as the text form shows it: without the page's origin.
fn shown_url<'a>(url: &'a str, page_origin: Option<&str>) -> &'a str {
    if page_origin.is_some() && origin(url) == page_origin {
        path_of(url)
    } else {
        url
    }
}

/// A frame's place, the original one if a source map gave it:
/// `render (/src/app.ts:12:5)`. The function keeps its generated name: a
/// map names identifiers, not the function around them.
fn frame_line(frame: &StackFrame, page_origin: Option<&str>) -> String {
    match &frame.original {
        Some(original) => frame_text(
            &frame.function,
            shown_url(&original.source, page_origin),
            original.line,
            original.column,
        ),
        None => frame_text(
            &frame.function,
            shown_url(&frame.url, page_origin),
            frame.line,
            frame.column,
        ),
    }
}

/// Adds `entry`'s lines to `text`: `#12 error uncaught TypeError: …`,
/// its other lines indented, and for errors the stack (`    at …`).
fn entry_text(text: &mut String, entry: &ConsoleEntry, page_origin: Option<&str>) {
    let mut message = entry.text.clone();
    format::cut(&mut message, SHOWN_TEXT);
    let mut lines = message.lines();
    let first = lines.next().unwrap_or_default();
    let _ = write!(text, "#{} {} ", entry.seq, entry.level.as_str());
    if entry.worker.is_some() {
        text.push_str("[worker] ");
    }
    match (entry.source.as_str(), &entry.kind) {
        ("exception", Some(kind)) => {
            let _ = write!(text, "{kind} ");
        }
        ("console" | "exception", _) => {}
        (source, _) => {
            let _ = write!(text, "[{source}] ");
        }
    }
    text.push_str(first);
    let stack = entry.level == Level::Error && !entry.stack.is_empty();
    if !stack {
        let place = match (entry.stack.first(), &entry.url) {
            (Some(frame), _) => Some(match &frame.original {
                Some(o) => format!("{}:{}", shown_url(&o.source, page_origin), o.line),
                None => format!("{}:{}", shown_url(&frame.url, page_origin), frame.line),
            }),
            (None, Some(url)) => Some(match entry.line {
                Some(line) => format!("{}:{line}", shown_url(url, page_origin)),
                None => shown_url(url, page_origin).to_owned(),
            }),
            (None, None) => None,
        };
        if let Some(place) = place {
            let _ = write!(text, " ({place})");
        }
    }
    text.push('\n');
    for line in lines.by_ref().take(SHOWN_LINES - 1) {
        let _ = writeln!(text, "    {line}");
    }
    let more = lines.count();
    if more > 0 {
        let _ = writeln!(text, "    … {more} more lines (--json shows them)");
    }
    if stack {
        for frame in &entry.stack {
            let _ = writeln!(text, "    at {}", frame_line(frame, page_origin));
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::{ConsoleEntry, Level, SourcePosition, StackFrame};
    use serde_json::json;

    use super::{Console, entry_text, select};
    use crate::request::{ConsoleRequest, Since};

    fn request(errors: bool) -> ConsoleRequest {
        ConsoleRequest {
            errors,
            tab: None,
            level: Level::Info,
            since: Since::Last,
            grep: None,
            limit: 50,
            wait_ms: 0,
            sourcemaps: false,
            timeout_ms: 1000,
            max_chars: 0,
        }
    }

    fn log(console: &mut Console, kind: &str, text: &str) {
        console.event(
            "Runtime.consoleAPICalled",
            &json!({ "type": kind, "args": [{ "type": "string", "value": text }] }),
            None,
            1000,
        );
    }

    fn texts(console: &mut Console, req: &ConsoleRequest) -> Vec<String> {
        select(console, req, None, Some("http://x"), u64::MAX)
            .entries
            .into_iter()
            .map(|e| e.text)
            .collect()
    }

    #[test]
    fn cursors_show_each_entry_once() {
        let mut console = Console::default();
        log(&mut console, "log", "a");
        log(&mut console, "error", "b");
        let req = request(false);
        assert_eq!(texts(&mut console, &req), ["a", "b"]);
        assert!(texts(&mut console, &req).is_empty());
        log(&mut console, "log", "c");
        assert_eq!(texts(&mut console, &req), ["c"]);
        // `errors` has its own cursor.
        assert_eq!(texts(&mut console, &request(true)), ["b"]);
        // `--since 0` shows all again.
        let all = ConsoleRequest {
            since: Since::Seq { seq: 0 },
            ..request(false)
        };
        assert_eq!(texts(&mut console, &all), ["a", "b", "c"]);
    }

    #[test]
    fn levels_limits_and_grep() {
        let mut console = Console::default();
        log(&mut console, "debug", "d");
        log(&mut console, "warning", "w");
        log(&mut console, "log", "i1");
        log(&mut console, "log", "i2");
        let warn = ConsoleRequest {
            level: Level::Warn,
            since: Since::Seq { seq: 0 },
            ..request(false)
        };
        assert_eq!(texts(&mut console, &warn), ["w"]);
        let limited = ConsoleRequest {
            limit: 2,
            level: Level::Debug,
            since: Since::Seq { seq: 0 },
            ..request(false)
        };
        let picked = select(&mut console, &limited, None, None, u64::MAX);
        assert_eq!(picked.skipped, 2);
        assert_eq!(picked.entries[0].text, "i1");
        assert_eq!(picked.cursor, 4);
        let re = regex::Regex::new("^i").unwrap();
        let grep = ConsoleRequest {
            since: Since::Seq { seq: 0 },
            ..request(false)
        };
        let picked = select(&mut console, &grep, Some(&re), None, u64::MAX);
        assert_eq!(picked.entries.len(), 2);
    }

    #[test]
    fn errors_keep_failed_loads_of_the_page_origin() {
        let mut console = Console::default();
        for url in [
            "http://x/api/items",
            "http://other/pixel.gif",
            "http://x/favicon.ico",
        ] {
            console.event(
                "Log.entryAdded",
                &json!({ "entry": {
                    "source": "network", "level": "error", "url": url,
                    "text": "Failed to load resource: the server responded with a status of 404 (Not Found)",
                } }),
                None,
                1000,
            );
        }
        let picked = select(
            &mut console,
            &request(true),
            None,
            Some("http://x"),
            u64::MAX,
        );
        assert_eq!(picked.entries.len(), 1);
        assert_eq!(picked.entries[0].url.as_deref(), Some("http://x/api/items"));
    }

    #[test]
    fn the_buffer_s_losses_are_counted() {
        let mut console = Console::default();
        for i in 0..5 {
            console.event(
                "Runtime.consoleAPICalled",
                &json!({ "type": "log", "args": [{ "type": "string", "value": i.to_string() }] }),
                None,
                2,
            );
        }
        let picked = select(&mut console, &request(false), None, None, u64::MAX);
        assert_eq!(picked.dropped, 3);
        assert_eq!(picked.entries.len(), 2);
    }

    fn frame(function: &str, url: &str, line: u32) -> StackFrame {
        StackFrame {
            function: function.into(),
            url: url.into(),
            line,
            column: 5,
            original: None,
        }
    }

    #[test]
    fn text_lines() {
        let mut text = String::new();
        let mut error = ConsoleEntry {
            seq: 3,
            time_ms: 0,
            level: Level::Error,
            source: "exception".into(),
            kind: Some("uncaught".into()),
            text: "TypeError: x is undefined".into(),
            url: Some("http://x/app.js".into()),
            line: Some(3),
            column: Some(5),
            stack: vec![
                frame("go", "http://x/app.js", 3),
                frame("", "http://cdn/lib.js", 9),
            ],
            worker: None,
        };
        error.stack[0].original = Some(SourcePosition {
            source: "http://x/src/app.ts".into(),
            line: 12,
            column: 7,
            name: None,
        });
        entry_text(&mut text, &error, Some("http://x"));
        let log = ConsoleEntry {
            seq: 4,
            level: Level::Info,
            source: "console".into(),
            kind: Some("log".into()),
            text: "two\nlines".into(),
            stack: vec![frame("", "http://x/app.js", 8)],
            worker: Some("http://x/worker.js".into()),
            ..error.clone()
        };
        entry_text(&mut text, &log, Some("http://x"));
        let failed = ConsoleEntry {
            seq: 5,
            source: "network".into(),
            kind: None,
            text: "Failed to load resource: 404".into(),
            url: Some("http://x/api".into()),
            line: None,
            stack: Vec::new(),
            ..error
        };
        entry_text(&mut text, &failed, Some("http://x"));
        assert_eq!(
            text,
            "#3 error uncaught TypeError: x is undefined\n\
             \x20   at go (/src/app.ts:12:7)\n\
             \x20   at http://cdn/lib.js:9:5\n\
             #4 info [worker] two (/app.js:8)\n\
             \x20   lines\n\
             #5 error [network] Failed to load resource: 404 (/api)\n"
        );
    }
}
