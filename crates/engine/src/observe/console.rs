//! A tab's console (plan 06, milestone 6.1): a ring buffer of its
//! `console.*` calls, uncaught exceptions and Chrome's own messages, for
//! `console` and `errors`, and the console errors the post-action report
//! (plan 05, milestone 5.5) and wait timeouts name.
//!
//! Every page and iframe session of the tab has `Runtime` and `Log`
//! enabled from before its first script (see `tabs::setup`), and so has
//! each worker, so entries are recorded from the moment the tab exists,
//! whether or not anyone asks. A dedicated worker's entries go to its
//! tab; a shared or service worker's to each tab of its scope with a
//! frame on its origin (plan 13), marked so that counting across tabs
//! counts each once ([`ConsoleError::once`]). `Runtime.consoleAPICalled` and
//! `Runtime.exceptionThrown` bring the page's own messages;
//! `Log.entryAdded` brings the browser's (failed loads, CSP violations,
//! interventions, deprecations). Arguments are formatted as they arrive
//! (see [`super::format`]), so an entry holds only text.
//! `Runtime.bindingCalled` brings the calls of functions `expose` put in
//! the page (`binding <name>(args)`).

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::schema::{ConsoleEntry, Level, StackFrame, Worker};
use serde_json::Value;

use super::format::{self, one_based};
use crate::inject::BINDING_PREFIX;

/// How many entries a tab keeps unless `open --console-buffer` says
/// otherwise.
pub(crate) const DEFAULT_CAPACITY: usize = 1000;

/// How much of an entry's text is kept.
const TEXT_LEN: usize = 4000;

/// How many stack frames an entry keeps.
const STACK_LEN: usize = 5;

/// A `console.error` (or failed `console.assert`) or uncaught exception,
/// as the post-action report shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleError {
    /// Its place in the tab's console; later ones are larger.
    pub(crate) seq: u64,
    /// The message's first line: `TypeError: x is undefined`.
    pub(crate) message: String,
    /// Where it came from: the script's URL, line and column (1-based).
    pub(crate) url: Option<String>,
    pub(crate) line: Option<u32>,
    pub(crate) column: Option<u32>,
    /// An uncaught exception (else a `console.error` call).
    pub(crate) uncaught: bool,
    /// The worker it came from, if a worker's.
    pub(crate) worker: Option<Worker>,
    /// For a shared or service worker's, which is in several tabs: the
    /// worker's target id and the event's number in it, the same in
    /// every tab.
    pub(crate) once: Option<(String, u64)>,
}

/// The worker a console event came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FromWorker<'a> {
    pub(crate) worker: &'a Worker,
    /// See [`ConsoleError::once`]: set for a shared or service worker.
    pub(crate) once: Option<(&'a str, u64)>,
}

/// The read cursors of `console` and `errors`: the last entry each showed.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Cursors {
    pub(crate) console: u64,
    pub(crate) errors: u64,
}

/// The console of one tab.
#[derive(Debug, Default)]
pub(crate) struct Console {
    entries: VecDeque<Stored>,
    seq: u64,
    /// The newest entry dropped to make room.
    evicted: u64,
    pub(crate) cursors: Cursors,
}

#[derive(Debug)]
struct Stored {
    entry: ConsoleEntry,
    /// An uncaught exception's id, until a late handler revokes it.
    exception_id: Option<i64>,
    /// See [`ConsoleError::once`].
    once: Option<(String, u64)>,
}

impl Console {
    /// The number of the last entry logged: a mark for
    /// [`Console::errors_since`].
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// The newest entry dropped to make room (0 if none was).
    pub(crate) fn evicted(&self) -> u64 {
        self.evicted
    }

    /// The entries kept, oldest first.
    pub(crate) fn entries(&self) -> impl DoubleEndedIterator<Item = &ConsoleEntry> {
        self.entries.iter().map(|s| &s.entry)
    }

    /// Applies one `Runtime` or `Log` event (of worker `from`, if given),
    /// keeping at most `capacity` entries; `false` if it logged nothing.
    pub(crate) fn event(
        &mut self,
        method: &str,
        params: &Value,
        from: Option<FromWorker<'_>>,
        capacity: usize,
    ) -> bool {
        let (mut entry, exception_id) = match method {
            "Runtime.consoleAPICalled" => match console_call(params) {
                Some(entry) => (entry, None),
                None => return false,
            },
            "Runtime.bindingCalled" => match binding_call(params) {
                Some(entry) => (entry, None),
                None => return false,
            },
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                (exception(details), details["exceptionId"].as_i64())
            }
            "Runtime.exceptionRevoked" => {
                // A promise rejection got its handler after all.
                let id = params["exceptionId"].as_i64();
                let before = self.entries.len();
                self.entries
                    .retain(|s| id.is_none() || s.exception_id != id);
                return self.entries.len() != before;
            }
            // A worker's `Log` repeats its console calls.
            "Log.entryAdded" if params["entry"]["source"] == "worker" => return false,
            "Log.entryAdded" => (log_entry(&params["entry"]), None),
            _ => return false,
        };
        self.seq += 1;
        entry.seq = self.seq;
        entry.time_ms = now_ms();
        entry.worker = from.map(|f| f.worker.url.clone());
        entry.worker_kind = from.map(|f| f.worker.kind);
        format::cut(&mut entry.text, TEXT_LEN);
        entry.stack.truncate(STACK_LEN);
        if entry.url.is_none()
            && let Some(top) = entry.stack.first()
        {
            entry.url = Some(top.url.clone());
            entry.line = Some(top.line);
            entry.column = Some(top.column);
        }
        while self.entries.len() >= capacity.max(1) {
            if let Some(dropped) = self.entries.pop_front() {
                self.evicted = dropped.entry.seq;
            }
        }
        self.entries.push_back(Stored {
            entry,
            exception_id,
            once: from.and_then(|f| f.once).map(|(id, n)| (id.to_owned(), n)),
        });
        true
    }

    /// The `console.error` calls, failed `console.assert`s and uncaught
    /// exceptions logged after mark `after`, oldest first.
    pub(crate) fn errors_since(&self, after: u64) -> Vec<ConsoleError> {
        self.entries
            .iter()
            .filter(|s| s.entry.seq > after && is_page_error(&s.entry))
            .map(|s| {
                let e = &s.entry;
                ConsoleError {
                    seq: e.seq,
                    message: e.text.lines().next().unwrap_or_default().trim().to_owned(),
                    url: e.url.clone(),
                    line: e.line,
                    column: e.column,
                    uncaught: e.source == "exception",
                    worker: e
                        .worker_kind
                        .zip(e.worker.clone())
                        .map(|(kind, url)| Worker { kind, url }),
                    once: s.once.clone(),
                }
            })
            .collect()
    }
}

/// Whether `entry` is an error of the page's own scripts: a
/// `console.error`, a failed `console.assert` or an uncaught exception.
pub(crate) fn is_page_error(entry: &ConsoleEntry) -> bool {
    entry.source == "exception" || (entry.source == "console" && entry.level == Level::Error)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// An entry with nothing set yet but these.
fn entry(level: Level, source: &str, kind: Option<&str>, text: String) -> ConsoleEntry {
    ConsoleEntry {
        seq: 0,
        time_ms: 0,
        level,
        source: source.to_owned(),
        kind: kind.map(str::to_owned),
        text,
        url: None,
        line: None,
        column: None,
        stack: Vec::new(),
        worker: None,
        worker_kind: None,
    }
}

/// A call of a function `expose` put in the page: `binding onSave({"id":
/// 1}, "x")`, the arguments as JSON. `None` for a binding of someone else.
fn binding_call(params: &Value) -> Option<ConsoleEntry> {
    let name = params["name"].as_str()?.strip_prefix(BINDING_PREFIX)?;
    let payload = params["payload"].as_str().unwrap_or_default();
    // `{"call":N,"args":[…]}` from a function exposed with answers.
    let args = match serde_json::from_str::<Value>(payload) {
        Ok(Value::Object(mut call)) if call.get("args").is_some_and(Value::is_array) => {
            call.remove("args")
        }
        other => other.ok(),
    };
    let args = match args {
        Some(Value::Array(args)) => args
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        _ => payload.to_owned(),
    };
    Some(entry(
        Level::Info,
        "binding",
        Some(name),
        format!("binding {name}({args})"),
    ))
}

/// A `console.*` call; `None` for those that print nothing
/// (`console.groupEnd`, `console.clear`).
fn console_call(params: &Value) -> Option<ConsoleEntry> {
    let kind = params["type"].as_str().unwrap_or("log");
    let level = match kind {
        "error" | "assert" => Level::Error,
        "warning" => Level::Warn,
        "debug" => Level::Debug,
        "endGroup" | "clear" => return None,
        _ => Level::Info,
    };
    let args = params["args"].as_array().map_or(&[][..], Vec::as_slice);
    let (mut text, error_stack) = format::message(args);
    if kind == "assert" {
        text = if text.is_empty() {
            "Assertion failed".to_owned()
        } else {
            format!("Assertion failed: {text}")
        };
    }
    let kind = match kind {
        "warning" => "warn",
        "startGroup" | "startGroupCollapsed" => "group",
        other => other,
    };
    let mut entry = entry(level, "console", Some(kind), text);
    // An error logged with its stack: where it was thrown matters more
    // than where it was logged.
    entry.stack = if error_stack.is_empty() {
        format::stack_frames(&params["stackTrace"])
    } else {
        error_stack
    };
    Some(entry)
}

/// An uncaught exception from its `ExceptionDetails`.
fn exception(details: &Value) -> ConsoleEntry {
    let thrown = &details["exception"];
    let what = details["text"].as_str().unwrap_or_default();
    let text = if thrown.is_object() {
        format::arg_text(thrown)
    } else {
        what.to_owned()
    };
    // `Uncaught`, `Uncaught (in promise)`, or `Uncaught SyntaxError: …`
    // with no exception object.
    let kind = if what.starts_with("Uncaught (in promise)") {
        "uncaught (in promise)"
    } else {
        "uncaught"
    };
    let text = if thrown.is_object() {
        text
    } else {
        what.strip_prefix("Uncaught ").unwrap_or(what).to_owned()
    };
    let mut entry = entry(Level::Error, "exception", Some(kind), text);
    entry.stack = format::stack_frames(&details["stackTrace"]);
    if entry.stack.is_empty() {
        entry.stack = format::parse_stack(thrown["description"].as_str().unwrap_or_default());
    }
    if entry.stack.is_empty()
        && let Some(url) = details["url"].as_str().filter(|u| !u.is_empty())
        && let (Some(line), Some(column)) = (
            one_based(&details["lineNumber"]),
            one_based(&details["columnNumber"]),
        )
    {
        // A syntax error has only a place: a frame, so a source map can
        // resolve it too.
        entry.stack.push(StackFrame {
            function: String::new(),
            url: url.to_owned(),
            line,
            column,
            original: None,
        });
    }
    entry
}

/// A browser message from its `Log.LogEntry`.
fn log_entry(log: &Value) -> ConsoleEntry {
    let level = match log["level"].as_str() {
        Some("error") => Level::Error,
        Some("warning") => Level::Warn,
        Some("verbose") => Level::Debug,
        _ => Level::Info,
    };
    let source = log["source"].as_str().unwrap_or("other");
    let text = log["text"].as_str().unwrap_or_default().to_owned();
    let mut entry = entry(level, source, None, text);
    entry.stack = format::stack_frames(&log["stackTrace"]);
    if entry.stack.is_empty()
        && let Some(url) = log["url"].as_str().filter(|u| !u.is_empty())
    {
        entry.url = Some(url.to_owned());
        entry.line = one_based(&log["lineNumber"]);
    }
    entry
}

#[cfg(test)]
mod tests {
    use protocol::schema::{Level, Worker, WorkerKind};
    use serde_json::json;

    use super::{Console, FromWorker};

    const CAP: usize = 1000;

    #[test]
    fn logs_errors_and_uncaught_exceptions() {
        let mut console = Console::default();
        let mark = console.seq();
        assert!(console.event(
            "Runtime.consoleAPICalled",
            &json!({ "type": "log", "args": [{ "type": "string", "value": "hi" }] }),
            None,
            CAP,
        ));
        assert!(console.event(
            "Runtime.consoleAPICalled",
            &json!({
                "type": "error",
                "args": [
                    { "type": "string", "value": "save failed:" },
                    { "type": "number", "value": 500, "description": "500" },
                ],
                "stackTrace": { "callFrames": [
                    { "functionName": "save", "url": "http://x/app.js", "lineNumber": 11, "columnNumber": 4 },
                ] },
            }),
            None,
            CAP,
        ));
        assert!(console.event(
            "Runtime.exceptionThrown",
            &json!({ "exceptionDetails": {
                "exceptionId": 7,
                "text": "Uncaught",
                "lineNumber": 2,
                "columnNumber": 9,
                "url": "http://x/page.html",
                "exception": {
                    "type": "object",
                    "subtype": "error",
                    "description": "TypeError: x is undefined\n    at go (http://x/page.html:3:10)",
                },
            } }),
            None,
            CAP,
        ));
        let errors = console.errors_since(mark);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].message, "save failed: 500");
        assert_eq!(errors[0].url.as_deref(), Some("http://x/app.js"));
        assert_eq!((errors[0].line, errors[0].column), (Some(12), Some(5)));
        assert!(!errors[0].uncaught);
        assert_eq!(errors[1].message, "TypeError: x is undefined");
        assert_eq!(errors[1].line, Some(3));
        assert!(errors[1].uncaught);
        assert!(console.errors_since(errors[1].seq).is_empty());

        let entries: Vec<_> = console.entries().collect();
        assert_eq!(entries[0].level, Level::Info);
        assert_eq!(entries[0].kind.as_deref(), Some("log"));
        assert_eq!(entries[1].stack[0].function, "save");
        assert_eq!(entries[2].kind.as_deref(), Some("uncaught"));
        assert_eq!(entries[2].stack[0].function, "go");

        // A late handler revokes the rejection.
        assert!(console.event(
            "Runtime.exceptionRevoked",
            &json!({ "reason": "Handler added", "exceptionId": 7 }),
            None,
            CAP,
        ));
        assert_eq!(console.errors_since(mark).len(), 1);
    }

    #[test]
    fn browser_messages_levels_and_workers() {
        let mut console = Console::default();
        console.event(
            "Log.entryAdded",
            &json!({ "entry": {
                "source": "network", "level": "error",
                "text": "Failed to load resource: the server responded with a status of 404 (Not Found)",
                "url": "http://x/missing.js", "timestamp": 1.0,
            } }),
            None,
            CAP,
        );
        let dedicated = Worker {
            kind: WorkerKind::Dedicated,
            url: "http://x/worker.js".into(),
        };
        console.event(
            "Runtime.consoleAPICalled",
            &json!({ "type": "warning", "args": [{ "type": "string", "value": "slow" }] }),
            Some(FromWorker {
                worker: &dedicated,
                once: None,
            }),
            CAP,
        );
        console.event(
            "Runtime.consoleAPICalled",
            &json!({ "type": "assert", "args": [{ "type": "string", "value": "x > 0" }] }),
            None,
            CAP,
        );
        let entries: Vec<_> = console.entries().collect();
        assert_eq!(entries[0].source, "network");
        assert_eq!(entries[0].url.as_deref(), Some("http://x/missing.js"));
        assert_eq!(entries[0].line, None);
        assert_eq!(entries[1].level, Level::Warn);
        assert_eq!(entries[1].worker.as_deref(), Some("http://x/worker.js"));
        assert_eq!(entries[1].worker_kind, Some(WorkerKind::Dedicated));
        assert_eq!(entries[2].text, "Assertion failed: x > 0");
        // Chrome's own messages are not the page's errors.
        assert_eq!(console.errors_since(0).len(), 1);
        assert_eq!(console.errors_since(0)[0].worker, None);
    }

    #[test]
    fn a_shared_workers_error_knows_its_worker() {
        let mut console = Console::default();
        let shared = Worker {
            kind: WorkerKind::Shared,
            url: "http://x/shared.js".into(),
        };
        assert!(console.event(
            "Runtime.exceptionThrown",
            &json!({ "exceptionDetails": {
                "exceptionId": 1, "text": "Uncaught", "lineNumber": 0, "columnNumber": 0,
                "exception": { "type": "object", "subtype": "error", "description": "Error: boom" },
            } }),
            Some(FromWorker {
                worker: &shared,
                once: Some(("W1", 4)),
            }),
            CAP,
        ));
        let entry = console.entries().next().unwrap();
        assert_eq!(entry.worker_kind, Some(WorkerKind::Shared));
        let errors = console.errors_since(0);
        assert_eq!(errors[0].worker.as_ref(), Some(&shared));
        assert_eq!(errors[0].once, Some(("W1".to_owned(), 4)));
    }

    #[test]
    fn exposed_functions_calls() {
        let mut console = Console::default();
        let call = |name: &str, payload: &str| json!({ "name": name, "payload": payload, "executionContextId": 1 });
        let mut logged =
            |params: serde_json::Value| console.event("Runtime.bindingCalled", &params, None, CAP);
        assert!(logged(call(
            "__chromeCdpExpose_onSave",
            r#"[{"id":1},"x",null]"#
        )));
        assert!(logged(call("__chromeCdpExpose_ping", "[]")));
        assert!(logged(call("__chromeCdpExpose_raw", "not json")));
        // A function exposed with answers numbers its calls.
        assert!(logged(call(
            "__chromeCdpExpose_next",
            r#"{"call":2,"args":[1]}"#
        )));
        // Someone else's binding.
        assert!(!logged(call("otherBinding", "[]")));
        let entries: Vec<_> = console.entries().collect();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].text, r#"binding onSave({"id":1}, "x", null)"#);
        assert_eq!(entries[0].source, "binding");
        assert_eq!(entries[0].kind.as_deref(), Some("onSave"));
        assert_eq!(entries[0].level, Level::Info);
        assert_eq!(entries[1].text, "binding ping()");
        assert_eq!(entries[2].text, "binding raw(not json)");
        assert_eq!(entries[3].text, "binding next(1)");
        assert!(console.errors_since(0).is_empty());
    }

    #[test]
    fn the_buffer_is_bounded() {
        let mut console = Console::default();
        for i in 0..5 {
            console.event(
                "Runtime.consoleAPICalled",
                &json!({ "type": "log", "args": [{ "type": "string", "value": format!("m{i}") }] }),
                None,
                3,
            );
        }
        let texts: Vec<_> = console.entries().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["m2", "m3", "m4"]);
        assert_eq!(console.evicted(), 2);
        assert_eq!(console.seq(), 5);
    }
}
