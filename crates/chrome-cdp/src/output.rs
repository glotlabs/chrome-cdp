//! Rendering results and errors (plan 08 §2).
//!
//! A command returns a [`Success`] (its JSON data plus its text and quiet
//! forms) or an [`ErrorInfo`]. [`render`] prints either according to the
//! [`Mode`]: results on stdout, errors and warnings on stderr, or one JSON
//! envelope on stdout with `--json`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Duration;

use protocol::{Envelope, ErrorInfo};
use serde_json::Value;

/// How results are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Compact human/agent text.
    Text,
    /// `--json`: one envelope object on stdout.
    Json,
    /// `-q`: only the essential value.
    Quiet,
}

/// A command's successful result in every output form.
#[derive(Debug)]
pub(crate) struct Success {
    /// The `data` of the `--json` envelope.
    pub(crate) data: Value,
    /// The default text output, without a trailing newline.
    pub(crate) text: String,
    /// The `-q` output; defaults to the text form.
    pub(crate) quiet: Option<String>,
    /// Non-fatal notes, printed on stderr in text modes.
    pub(crate) warnings: Vec<String>,
}

impl Success {
    pub(crate) fn new(data: Value, text: impl Into<String>) -> Self {
        Self {
            data,
            text: text.into(),
            quiet: None,
            warnings: Vec::new(),
        }
    }

    #[must_use]
    pub(crate) fn quiet(mut self, quiet: impl Into<String>) -> Self {
        self.quiet = Some(quiet.into());
        self
    }
}

/// Prints `result` and returns the process exit code.
pub(crate) fn render(mode: Mode, result: Result<Success, ErrorInfo>) -> u8 {
    match (mode, result) {
        (Mode::Json, Ok(ok)) => {
            let mut env = Envelope::ok(ok.data);
            env.warnings = ok.warnings;
            stdout_line(&env.to_json());
            0
        }
        (Mode::Json, Err(err)) => {
            let code = err.exit_code();
            stdout_line(&Envelope::err(err).to_json());
            code
        }
        (Mode::Text | Mode::Quiet, Ok(ok)) => {
            for warning in &ok.warnings {
                stderr(&format!("warning: {warning}\n"));
            }
            let out = match (mode, ok.quiet) {
                (Mode::Quiet, Some(quiet)) => quiet,
                _ => ok.text,
            };
            stdout_line(&out);
            0
        }
        (Mode::Text | Mode::Quiet, Err(err)) => {
            stderr(&err.to_text());
            err.exit_code()
        }
    }
}

/// Writes `line` and a newline to stdout. A closed stdout (`| head`) is not
/// an error worth reporting.
fn stdout_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}

/// Writes `text` to stderr as is.
pub(crate) fn stderr(text: &str) {
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
}

/// Cuts `text` to at most `max_chars` characters (0 = no limit). A cut text
/// ends with one line saying how much was dropped and how to see it.
pub(crate) fn truncate(text: String, max_chars: usize, more: &str) -> String {
    if max_chars == 0 {
        return text;
    }
    let Some((cut, _)) = text.char_indices().nth(max_chars) else {
        return text;
    };
    let dropped = text[cut..].chars().count();
    let mut out = text[..cut].to_owned();
    let _ = write!(
        out,
        "\n… truncated {} chars; use {more}",
        thousands(dropped)
    );
    out
}

/// `18203` → `18,203`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Human units: `340ms`, `1.2s`, `30s`, `2m`, `2m30s`.
pub(crate) fn duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 10_000 && !ms.is_multiple_of(1000) {
        format!("{:.1}s", d.as_secs_f64())
    } else if ms < 60_000 {
        format!("{}s", d.as_secs())
    } else {
        let (m, s) = (d.as_secs() / 60, d.as_secs() % 60);
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m{s}s")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{duration, thousands, truncate};

    #[test]
    fn truncate_keeps_short_text() {
        assert_eq!(truncate("hello".into(), 5, "--max-chars 10"), "hello");
        assert_eq!(truncate("hello".into(), 0, "--max-chars 10"), "hello");
    }

    #[test]
    fn truncate_cuts_on_char_boundaries_and_says_how_to_see_more() {
        let text = "ééééé".to_owned() + &"x".repeat(1234);
        assert_eq!(
            truncate(text, 3, "--max-chars 0"),
            "ééé\n… truncated 1,236 chars; use --max-chars 0"
        );
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(18_203), "18,203");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn durations_use_human_units() {
        assert_eq!(duration(Duration::from_millis(340)), "340ms");
        assert_eq!(duration(Duration::from_millis(1200)), "1.2s");
        assert_eq!(duration(Duration::from_secs(1)), "1s");
        assert_eq!(duration(Duration::from_secs(30)), "30s");
        assert_eq!(duration(Duration::from_secs(120)), "2m");
        assert_eq!(duration(Duration::from_secs(150)), "2m30s");
    }
}
