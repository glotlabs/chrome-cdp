//! Optional ANSI styling for text output (plan 08 §2).
//!
//! Output is plain unless the stream is a TTY and `NO_COLOR` is unset; the
//! binary decides that per stream and passes a [`Style`] to the renderer.
//! Everything else (pipes, `$(…)`, agents) gets no escape codes.

/// Whether and how to colour one output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    color: bool,
}

impl Style {
    /// No escape codes.
    pub const PLAIN: Self = Self { color: false };
    /// ANSI colours.
    pub const COLOR: Self = Self { color: true };

    /// Colour if `color` is true.
    pub fn new(color: bool) -> Self {
        Self { color }
    }

    /// Whether this style emits escape codes.
    pub fn is_color(self) -> bool {
        self.color
    }

    /// `error:` prefix: bold red.
    pub fn error(self, text: &str) -> String {
        self.paint("1;31", text)
    }

    /// `warning:` prefix: bold yellow.
    pub fn warning(self, text: &str) -> String {
        self.paint("1;33", text)
    }

    /// `hint:` prefix: bold cyan.
    pub fn hint(self, text: &str) -> String {
        self.paint("1;36", text)
    }

    /// Note labels and other secondary text: dim.
    pub fn label(self, text: &str) -> String {
        self.paint("2", text)
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Style;

    #[test]
    fn plain_has_no_escape_codes() {
        assert_eq!(Style::PLAIN.error("error:"), "error:");
        assert_eq!(Style::COLOR.error("error:"), "\x1b[1;31merror:\x1b[0m");
        assert_eq!(Style::new(false), Style::PLAIN);
    }
}
