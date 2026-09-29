//! Printing results and errors (plan 08 §2).
//!
//! Commands return `Result<Output, ErrorInfo>`; [`protocol::render`] turns
//! that into stdout, stderr and an exit code, and [`print`] writes them.
//! This module decides whether stderr gets colour, and masks secrets: the
//! values of `${secret:…}` (plan 11.11) never reach stdout, stderr or a
//! file the test runner writes ([`mask`]).

use std::borrow::Cow;
use std::io::{IsTerminal as _, Write as _};
use std::sync::{Mutex, PoisonError};

use protocol::{ErrorInfo, Mode, Output, Rendered, Style};

/// Prints `result` in `mode` and returns the process exit code.
pub(crate) fn print(mode: Mode, result: Result<Output, ErrorInfo>) -> u8 {
    let rendered = render(mode, result);
    stdout(&rendered.stdout);
    stderr(&rendered.stderr);
    rendered.exit_code
}

/// Renders `result` in `mode` with this terminal's stderr style.
pub(crate) fn render(mode: Mode, result: Result<Output, ErrorInfo>) -> Rendered {
    protocol::render(mode, result, stderr_style())
}

/// Writes `text` to stdout (secrets masked, control characters shown as
/// symbols), and flushes it. Every stdout line goes through here, so what
/// a page says (a `--follow` stream too) never reaches the terminal as an
/// escape sequence; chrome-cdp's own colours are on stderr only.
pub(crate) fn stdout(text: &str) {
    // A closed stdout (`| head`) is not an error worth reporting.
    let mut out = std::io::stdout().lock();
    let text = mask(text);
    let _ = out
        .write_all(protocol::format::printable(&text).as_bytes())
        .and_then(|()| out.flush());
}

/// Writes `text` to stderr (secrets masked).
pub(crate) fn stderr(text: &str) {
    let _ = std::io::stderr().lock().write_all(mask(text).as_bytes());
}

/// The secrets to mask, with their names; the longest first, so one
/// secret inside another is not masked in part.
static SECRETS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Masks `value`, the value of `${secret:name}`, from now on.
pub(crate) fn add_secret(name: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    let mut secrets = SECRETS.lock().unwrap_or_else(PoisonError::into_inner);
    if !secrets.iter().any(|(_, v)| v == value) {
        secrets.push((name.to_owned(), value.to_owned()));
        secrets.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    }
}

/// `text` with every secret's value replaced by `<secret:name>`.
pub(crate) fn mask(text: &str) -> Cow<'_, str> {
    let secrets = SECRETS.lock().unwrap_or_else(PoisonError::into_inner);
    if !secrets.iter().any(|(_, v)| text.contains(v.as_str())) {
        return Cow::Borrowed(text);
    }
    let mut out = text.to_owned();
    for (name, value) in secrets.iter() {
        out = out.replace(value.as_str(), &format!("<secret:{name}>"));
    }
    Cow::Owned(out)
}

/// Colour on stderr only when it is a terminal and `NO_COLOR` is unset or
/// empty (<https://no-color.org>).
fn stderr_style() -> Style {
    Style::new(std::io::stderr().is_terminal() && no_color_unset(std::env::var_os("NO_COLOR")))
}

fn no_color_unset(value: Option<std::ffi::OsString>) -> bool {
    value.is_none_or(|v| v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{add_secret, mask, no_color_unset};

    #[test]
    fn secrets_are_masked() {
        add_secret("pw", "hunter2-secret");
        add_secret("pin", "hunter2");
        add_secret("empty", "");
        assert_eq!(
            mask("fill hunter2-secret and hunter2"),
            "fill <secret:pw> and <secret:pin>"
        );
        assert!(matches!(
            mask("nothing here"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn no_color_counts_only_when_non_empty() {
        assert!(no_color_unset(None));
        assert!(no_color_unset(Some("".into())));
        assert!(!no_color_unset(Some("1".into())));
    }
}
