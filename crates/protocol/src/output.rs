//! Command results and the one renderer every command shares (plan 08 §2).
//!
//! A command returns `Result<Output, ErrorInfo>`. [`render`] turns that into
//! the bytes for stdout and stderr and the exit code, in text, quiet (`-q`)
//! or `--json` form. It does no I/O, so the CLI, `batch` and golden tests
//! all use the same code.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ErrorInfo;
use crate::bound::{self, Limit, Truncation};
use crate::schema::Event;
use crate::style::Style;

/// A command's successful result in every output form. The daemon returns
/// this over RPC, so it is serialisable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    /// The `data` of the `--json` envelope.
    pub data: Value,
    /// The default text output, without a trailing newline.
    pub text: String,
    /// The `-q` output; defaults to the text form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<String>,
    /// Non-fatal notes, printed on stderr in text modes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Page events observed while the command ran (dialogs, navigations,
    /// …). Only the `--json` form lists them; commands put what matters into
    /// their text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
    /// The output budget. Every command whose output can grow with the page
    /// (snapshot, text, eval, raw CDP, logs, …) must set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Limit>,
}

impl Output {
    /// A result with `data` for `--json` and `text` for everything else.
    pub fn new(data: Value, text: impl Into<String>) -> Self {
        Self {
            data,
            text: text.into(),
            quiet: None,
            warnings: Vec::new(),
            events: Vec::new(),
            limit: None,
        }
    }

    /// A result whose `data` is one of the typed [`schema`](crate::schema)
    /// structs.
    pub fn typed<T: Serialize>(data: &T, text: impl Into<String>) -> Self {
        // Plain structs with string keys always serialise.
        Self::new(serde_json::to_value(data).unwrap_or(Value::Null), text)
    }

    /// Sets the `-q` form: only the essential value (a ref, a path, …).
    #[must_use]
    pub fn quiet(mut self, quiet: impl Into<String>) -> Self {
        self.quiet = Some(quiet.into());
        self
    }

    /// Adds a warning.
    #[must_use]
    pub fn warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Adds a page event (`--json` only; say what matters in the text too).
    #[must_use]
    pub fn event(mut self, event: Event) -> Self {
        self.events.push(event);
        self
    }

    /// Bounds every form at `max_chars` (0 = no limit); `more` says how to
    /// see the rest, after "use": `--max-chars 0 for all of it`.
    #[must_use]
    pub fn limit(mut self, max_chars: usize, more: impl Into<String>) -> Self {
        self.limit = Some(Limit::new(max_chars, more));
        self
    }
}

/// The one JSON object a command prints on stdout with `--json`:
///
/// ```json
/// {"ok":true,"data":{},"warnings":[]}
/// {"ok":false,"error":{"kind":"Timeout","message":"…","hint":"…"}}
/// ```
///
/// `warnings`, `events` and `truncated` are omitted when empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Whether the command succeeded.
    pub ok: bool,
    /// The result, on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// The failure, when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    /// Non-fatal notes (the text form prints them on stderr).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Page events observed while the command ran (dialogs, navigations, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
    /// Values in `data` shortened to keep the output bounded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub truncated: Vec<Truncation>,
}

impl Envelope {
    /// A success carrying `data`.
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            warnings: Vec::new(),
            events: Vec::new(),
            truncated: Vec::new(),
        }
    }

    /// A failure.
    pub fn err(error: ErrorInfo) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error),
            warnings: Vec::new(),
            events: Vec::new(),
            truncated: Vec::new(),
        }
    }

    /// A success envelope for `output`, with `data` bounded by its limit.
    pub fn from_output(output: Output) -> Self {
        let Output {
            mut data,
            warnings,
            events,
            limit,
            ..
        } = output;
        let mut env = Self::ok(Value::Null);
        env.warnings = warnings;
        env.events = events;
        if let Some(limit) = &limit {
            env.truncated = bound::bound_json(&mut data, limit.max_chars);
            if !env.truncated.is_empty() {
                env.warnings
                    .push(bound::truncated_warning(&env.truncated, limit));
            }
        }
        env.data = Some(data);
        env
    }

    /// Serialises to one line of JSON (no trailing newline).
    pub fn to_json(&self) -> String {
        // An `Envelope` holds only `Value`s, strings and string-keyed maps,
        // which always serialise; the fallback keeps the output valid JSON.
        serde_json::to_string(self).unwrap_or_else(|e| {
            let msg = Value::String(format!("cannot serialise output: {e}"));
            format!(r#"{{"ok":false,"error":{{"kind":"ProtocolError","message":{msg}}}}}"#)
        })
    }
}

/// How results are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Compact human/agent text.
    Text,
    /// `--json`: one envelope object on stdout.
    Json,
    /// `-q`: only the essential value.
    Quiet,
}

/// What to print and the exit code.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rendered {
    /// Everything for stdout, ending in a newline (or empty).
    pub stdout: String,
    /// Everything for stderr, ending in a newline (or empty).
    pub stderr: String,
    /// The process exit code (plan 08 §3).
    pub exit_code: u8,
}

/// Renders `result` in `mode`. `stderr` styles warnings and errors; stdout
/// is never styled here, because commands build their own text.
///
/// - Text: the text on stdout (cut at the limit, ending with a line saying
///   how to see more); warnings and errors on stderr.
/// - Quiet: the quiet form (or the text) on stdout, cut at the limit with no
///   extra line, so `$(…)` captures only the value; the "truncated" note
///   goes to stderr with the warnings.
/// - JSON: one envelope line on stdout with `data` bounded by the limit;
///   nothing on stderr.
pub fn render(mode: Mode, result: Result<Output, ErrorInfo>, stderr: Style) -> Rendered {
    match (mode, result) {
        (Mode::Json, Ok(output)) => Rendered {
            stdout: line(&Envelope::from_output(output).to_json()),
            stderr: String::new(),
            exit_code: 0,
        },
        (Mode::Json, Err(err)) => Rendered {
            exit_code: err.exit_code(),
            stdout: line(&Envelope::err(err).to_json()),
            stderr: String::new(),
        },
        (Mode::Text | Mode::Quiet, Ok(output)) => {
            let Output {
                text,
                quiet,
                mut warnings,
                limit,
                ..
            } = output;
            let (max_chars, more) = limit.map_or((0, String::new()), |l| (l.max_chars, l.more));
            let stdout = if mode == Mode::Quiet {
                let value = quiet.unwrap_or(text);
                match bound::cut(&value, max_chars) {
                    None => line(&value),
                    Some((kept, dropped)) => {
                        warnings.push(bound::truncated_line(dropped, &more));
                        line(kept)
                    }
                }
            } else {
                line(&bound::truncate(&text, max_chars, &more))
            };
            let prefix = stderr.warning("warning:");
            let stderr = warnings.iter().fold(String::new(), |mut out, w| {
                let _ = writeln!(out, "{prefix} {w}");
                out
            });
            Rendered {
                stdout,
                stderr,
                exit_code: 0,
            }
        }
        (Mode::Text | Mode::Quiet, Err(err)) => Rendered {
            stdout: String::new(),
            stderr: err.render_text(stderr),
            exit_code: err.exit_code(),
        },
    }
}

/// `text` plus a newline, or nothing for empty text.
fn line(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!("{text}\n")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Envelope, Mode, Output, Rendered, render};
    use crate::style::Style;
    use crate::{ErrorInfo, ErrorKind};

    fn text(result: Result<Output, ErrorInfo>) -> Rendered {
        render(Mode::Text, result, Style::PLAIN)
    }

    #[test]
    fn ok_envelope() {
        let env = Envelope::ok(json!({"product": "Chrome/154"}));
        assert_eq!(
            env.to_json(),
            r#"{"ok":true,"data":{"product":"Chrome/154"}}"#
        );
    }

    #[test]
    fn err_envelope() {
        let env = Envelope::err(ErrorInfo::new(ErrorKind::Timeout, "slow").hint("retry"));
        assert_eq!(
            env.to_json(),
            r#"{"ok":false,"error":{"kind":"Timeout","message":"slow","hint":"retry"}}"#
        );
    }

    #[test]
    fn warnings_are_listed_when_present() {
        let out = Output::new(json!(1), "1").warning("careful");
        let r = render(Mode::Json, Ok(out), Style::PLAIN);
        assert_eq!(
            r.stdout,
            "{\"ok\":true,\"data\":1,\"warnings\":[\"careful\"]}\n"
        );
        assert_eq!(r.stderr, "");
    }

    #[test]
    fn text_puts_results_on_stdout_and_notes_on_stderr() {
        let r = text(Ok(Output::new(json!({}), "done").warning("slow page")));
        assert_eq!(
            r,
            Rendered {
                stdout: "done\n".into(),
                stderr: "warning: slow page\n".into(),
                exit_code: 0
            }
        );
        let r = render(
            Mode::Text,
            Ok(Output::new(json!({}), "done").warning("slow page")),
            Style::COLOR,
        );
        assert_eq!(r.stderr, "\x1b[1;33mwarning:\x1b[0m slow page\n");
        assert_eq!(text(Ok(Output::new(json!(null), ""))).stdout, "");
    }

    #[test]
    fn errors_go_to_stderr_with_their_exit_code() {
        let err = ErrorInfo::new(ErrorKind::DialogOpen, "a dialog is open")
            .hint("chrome-cdp dialog accept");
        let r = text(Err(err.clone()));
        assert_eq!(r.stdout, "");
        assert_eq!(
            r.stderr,
            "error: a dialog is open\n  hint: chrome-cdp dialog accept\n"
        );
        assert_eq!(r.exit_code, 5);
        let r = render(Mode::Json, Err(err), Style::COLOR);
        assert_eq!(r.stderr, "");
        assert_eq!(r.exit_code, 5);
        assert!(
            r.stdout
                .starts_with(r#"{"ok":false,"error":{"kind":"DialogOpen""#)
        );
    }

    #[test]
    fn quiet_prefers_the_quiet_form() {
        let out = Output::new(json!({"ref": "e5"}), "clicked e5 (button \"OK\")").quiet("e5");
        assert_eq!(render(Mode::Quiet, Ok(out), Style::PLAIN).stdout, "e5\n");
        let out = Output::new(json!({}), "only text");
        assert_eq!(
            render(Mode::Quiet, Ok(out), Style::PLAIN).stdout,
            "only text\n"
        );
    }

    #[test]
    fn every_form_honours_the_limit() {
        let long = "x".repeat(50);
        let out = || {
            Output::new(json!({"value": long}), long.clone())
                .quiet(long.clone())
                .limit(10, "--max-chars 0")
        };
        let r = text(Ok(out()));
        assert_eq!(
            r.stdout,
            "xxxxxxxxxx\n… truncated 40 chars; use --max-chars 0\n"
        );
        assert_eq!(r.stderr, "");

        // Quiet keeps stdout to the value and says it was cut on stderr.
        let r = render(Mode::Quiet, Ok(out()), Style::PLAIN);
        assert_eq!(r.stdout, "xxxxxxxxxx\n");
        assert_eq!(
            r.stderr,
            "warning: … truncated 40 chars; use --max-chars 0\n"
        );

        // JSON stays valid and says what it shortened.
        let r = render(
            Mode::Json,
            Ok(out().limit(30, "--max-chars 0")),
            Style::PLAIN,
        );
        let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
        assert!(v["data"].to_string().chars().count() <= 30, "{v}");
        assert_eq!(v["truncated"][0]["path"], json!("/value"));
        assert_eq!(v["truncated"][0]["kind"], json!("string"));
        assert_eq!(v["truncated"][0]["len"], json!(50));
        assert!(
            v["warnings"][0]
                .as_str()
                .unwrap()
                .starts_with("data truncated to 30 chars: /value 50 → "),
            "{v}"
        );
    }

    #[test]
    fn output_round_trips_through_json() {
        let out = Output::new(json!({"a": 1}), "a")
            .quiet("1")
            .limit(5, "--max-chars 0");
        let back: Output = serde_json::from_value(serde_json::to_value(&out).unwrap()).unwrap();
        assert_eq!(back, out);
    }
}
