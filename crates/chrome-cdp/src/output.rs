//! Printing results and errors (plan 08 §2).
//!
//! Commands return `Result<Output, ErrorInfo>`; [`protocol::render`] turns
//! that into stdout, stderr and an exit code, and [`print`] writes them.
//! This module only decides whether stderr gets colour.

use std::io::{IsTerminal as _, Write as _};

use protocol::{ErrorInfo, Mode, Output, Style};

/// Prints `result` in `mode` and returns the process exit code.
pub(crate) fn print(mode: Mode, result: Result<Output, ErrorInfo>) -> u8 {
    let rendered = protocol::render(mode, result, stderr_style());
    // A closed stdout (`| head`) is not an error worth reporting.
    let mut out = std::io::stdout().lock();
    let _ = out
        .write_all(rendered.stdout.as_bytes())
        .and_then(|()| out.flush());
    stderr(&rendered.stderr);
    rendered.exit_code
}

/// Writes `text` to stderr as is.
pub(crate) fn stderr(text: &str) {
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
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
    use super::no_color_unset;

    #[test]
    fn no_color_counts_only_when_non_empty() {
        assert!(no_color_unset(None));
        assert!(no_color_unset(Some("".into())));
        assert!(!no_color_unset(Some("1".into())));
    }
}
