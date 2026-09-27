//! A tab's console errors (plan 05, milestone 5.5): the `console.error`
//! calls and uncaught exceptions of its page and OOPIF sessions, for the
//! post-action report's `! console error:` lines.
//!
//! Every session of the tab has `Runtime` enabled for its execution
//! contexts, so the events arrive anyway (see `tabs::watch_host`). Plan
//! 06's console buffer can grow from the same events (every level, the
//! `console` and `errors` commands).

use std::collections::VecDeque;

use serde_json::Value;

/// How many errors the log keeps.
const LOG_LEN: usize = 100;

/// How much of a message the log keeps.
const MESSAGE_LEN: usize = 1000;

/// A `console.error` (or failed `console.assert`) or uncaught exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleError {
    /// Its place in the tab's log; later ones are larger.
    pub(crate) seq: u64,
    /// The message's first line: `TypeError: x is undefined`.
    pub(crate) message: String,
    /// Where it came from: the script's URL, line and column (1-based).
    pub(crate) url: Option<String>,
    pub(crate) line: Option<u32>,
    pub(crate) column: Option<u32>,
    /// An uncaught exception (else a `console.error` call).
    pub(crate) uncaught: bool,
}

/// The console errors of one tab.
#[derive(Debug, Default)]
pub(crate) struct Console {
    errors: VecDeque<ConsoleError>,
    seq: u64,
}

impl Console {
    /// The number of the last error logged: a mark for
    /// [`Console::errors_since`].
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// Applies one `Runtime` event; `false` if it logged nothing.
    pub(crate) fn event(&mut self, method: &str, params: &Value) -> bool {
        let error = match method {
            "Runtime.consoleAPICalled" => {
                if !matches!(params["type"].as_str(), Some("error" | "assert")) {
                    return false;
                }
                let args = params["args"].as_array().map_or(&[][..], Vec::as_slice);
                let words: Vec<String> = args.iter().map(remote_text).collect();
                let (url, line, column) = top_frame(&params["stackTrace"]);
                ConsoleError {
                    seq: 0,
                    message: words.join(" "),
                    url,
                    line,
                    column,
                    uncaught: false,
                }
            }
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let exception = &details["exception"];
                let message = if exception.is_object() {
                    remote_text(exception)
                } else {
                    details["text"].as_str().unwrap_or_default().to_owned()
                };
                let (mut url, mut line, mut column) = top_frame(&details["stackTrace"]);
                if url.is_none() {
                    url = details["url"]
                        .as_str()
                        .filter(|u| !u.is_empty())
                        .map(str::to_owned);
                    line = one_based(&details["lineNumber"]);
                    column = one_based(&details["columnNumber"]);
                }
                ConsoleError {
                    seq: 0,
                    message,
                    url,
                    line,
                    column,
                    uncaught: true,
                }
            }
            _ => return false,
        };
        self.seq += 1;
        let mut message = error
            .message
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if let Some((cut, _)) = message.char_indices().nth(MESSAGE_LEN) {
            message.truncate(cut);
            message.push('…');
        }
        if self.errors.len() >= LOG_LEN {
            self.errors.pop_front();
        }
        self.errors.push_back(ConsoleError {
            seq: self.seq,
            message,
            ..error
        });
        true
    }

    /// The errors logged after mark `after`, oldest first.
    pub(crate) fn errors_since(&self, after: u64) -> Vec<ConsoleError> {
        self.errors
            .iter()
            .filter(|e| e.seq > after)
            .cloned()
            .collect()
    }
}

/// How `console.log` would print a `Runtime.RemoteObject`, roughly.
fn remote_text(object: &Value) -> String {
    if let Some(text) = object["value"].as_str() {
        return text.to_owned();
    }
    if let Some(description) = object["description"].as_str() {
        return description.to_owned();
    }
    if let Some(value) = object["unserializableValue"].as_str() {
        return value.to_owned();
    }
    match &object["value"] {
        Value::Null if object["subtype"].as_str() == Some("null") => "null".to_owned(),
        Value::Null if object["type"].as_str() == Some("undefined") => "undefined".to_owned(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

/// The URL, line and column of a stack trace's top frame that has a URL.
fn top_frame(stack: &Value) -> (Option<String>, Option<u32>, Option<u32>) {
    let frames = stack["callFrames"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    frames
        .iter()
        .find(|f| f["url"].as_str().is_some_and(|u| !u.is_empty()))
        .map_or((None, None, None), |f| {
            (
                f["url"].as_str().map(str::to_owned),
                one_based(&f["lineNumber"]),
                one_based(&f["columnNumber"]),
            )
        })
}

/// A 0-based CDP line or column number, 1-based.
fn one_based(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n + 1).ok())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Console;

    #[test]
    fn logs_errors_and_uncaught_exceptions() {
        let mut console = Console::default();
        let mark = console.seq();
        assert!(!console.event(
            "Runtime.consoleAPICalled",
            &json!({ "type": "log", "args": [{ "type": "string", "value": "hi" }] }),
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
                    { "url": "http://x/app.js", "lineNumber": 11, "columnNumber": 4 },
                ] },
            }),
        ));
        assert!(console.event(
            "Runtime.exceptionThrown",
            &json!({ "exceptionDetails": {
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
    }
}
