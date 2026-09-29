//! Building error hints (plan 08 §3).
//!
//! A hint names the concrete next command. [`Cmd`] builds that command so it
//! can be pasted as is: arguments are shell-quoted and a non-default session
//! and a `--context` are carried over. [`Hint`] adds the reason and alternatives:
//!
//! ```
//! use protocol::{Cmd, Hint};
//!
//! let hint = Hint::run(Cmd::new("click").arg("e89").session("shop"))
//!     .why(r#"button "Accept" inside the banner"#);
//! assert_eq!(
//!     hint.to_string(),
//!     r#"chrome-cdp click e89 --session shop   (button "Accept" inside the banner)"#
//! );
//! ```

use std::fmt;

use crate::format::shell_quote;

/// The program name every hint command starts with.
pub const PROGRAM: &str = "chrome-cdp";

/// The session used when `--session` is not given.
pub const DEFAULT_SESSION: &str = "default";

/// A `chrome-cdp` command line for a hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    words: Vec<String>,
}

impl Cmd {
    /// `chrome-cdp <command>`; `command` may hold several words
    /// (`"tab close"`), which are not quoted.
    pub fn new(command: &str) -> Self {
        Self {
            words: command.split_whitespace().map(str::to_owned).collect(),
        }
    }

    /// Adds an argument, quoted for the shell when needed.
    #[must_use]
    pub fn arg(mut self, arg: impl AsRef<str>) -> Self {
        self.words.push(shell_quote(arg.as_ref()).into_owned());
        self
    }

    /// Adds `--name value` (the value is quoted when needed).
    #[must_use]
    pub fn flag(self, name: &str, value: impl AsRef<str>) -> Self {
        self.raw(format!("--{name}")).arg(value)
    }

    /// Adds `--name` with no value.
    #[must_use]
    pub fn switch(self, name: &str) -> Self {
        self.raw(format!("--{name}"))
    }

    /// Adds text as is, for placeholders the user fills in (`<path>`, `…`).
    #[must_use]
    pub fn raw(mut self, text: impl Into<String>) -> Self {
        self.words.push(text.into());
        self
    }

    /// Adds `--session <name>` unless `name` is the default session, so the
    /// hint acts on the session the failing command used.
    #[must_use]
    pub fn session(self, name: &str) -> Self {
        if name == DEFAULT_SESSION {
            self
        } else {
            self.flag("session", name)
        }
    }

    /// Adds `--context <name>` when the failing command acted in a context
    /// (`None` for the session's default one), so the hint acts there too.
    /// A command that already names a context keeps it.
    #[must_use]
    pub fn context(self, name: Option<&str>) -> Self {
        match name {
            Some(name) if !self.words.iter().any(|w| w == "--context") => {
                self.flag("context", name)
            }
            _ => self,
        }
    }
}

impl fmt::Display for Cmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PROGRAM)?;
        for word in &self.words {
            write!(f, " {word}")?;
        }
        Ok(())
    }
}

/// An error hint: the next command to run, why, and alternatives. It renders
/// to the one-line `hint` string of an [`ErrorInfo`](crate::ErrorInfo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    text: String,
}

impl Hint {
    /// Run this command next.
    pub fn run(cmd: impl Into<Cmd>) -> Self {
        Self {
            text: cmd.into().to_string(),
        }
    }

    /// Free-form advice, for when no single command fixes it. Prefer
    /// [`Hint::run`]; a hint should name a command where it can.
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Says why, or what the command acts on:
    /// `chrome-cdp click e89   (button "Accept")`.
    #[must_use]
    pub fn why(mut self, reason: impl AsRef<str>) -> Self {
        self.text.push_str("   (");
        self.text.push_str(reason.as_ref());
        self.text.push(')');
        self
    }

    /// Offers another way out: `<this>, or <other>`.
    #[must_use]
    pub fn or(mut self, other: impl Into<String>) -> Self {
        self.text.push_str(", or ");
        self.text.push_str(&other.into());
        self
    }
}

impl fmt::Display for Hint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<Hint> for String {
    fn from(hint: Hint) -> Self {
        hint.text
    }
}

impl From<Cmd> for String {
    fn from(cmd: Cmd) -> Self {
        cmd.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{Cmd, Hint};

    #[test]
    fn commands_quote_arguments() {
        let cmd = Cmd::new("fill").arg("label=Email").arg("a b@c.d");
        assert_eq!(cmd.to_string(), "chrome-cdp fill label=Email 'a b@c.d'");
        let cmd = Cmd::new("tab close").arg("2");
        assert_eq!(cmd.to_string(), "chrome-cdp tab close 2");
        let cmd = Cmd::new("goto").flag("timeout", "60s").switch("json");
        assert_eq!(cmd.to_string(), "chrome-cdp goto --timeout 60s --json");
        let cmd = Cmd::new("cdp").arg("Runtime.evaluate").raw("…");
        assert_eq!(cmd.to_string(), "chrome-cdp cdp Runtime.evaluate …");
    }

    #[test]
    fn default_session_is_left_out() {
        assert_eq!(
            Cmd::new("snapshot").session("default").to_string(),
            "chrome-cdp snapshot"
        );
        assert_eq!(
            Cmd::new("snapshot").session("my shop").to_string(),
            "chrome-cdp snapshot --session 'my shop'"
        );
    }

    #[test]
    fn a_context_is_carried_over() {
        assert_eq!(
            Cmd::new("snapshot")
                .session("shop")
                .context(Some("c2"))
                .to_string(),
            "chrome-cdp snapshot --session shop --context c2"
        );
        assert_eq!(
            Cmd::new("snapshot").context(None).to_string(),
            "chrome-cdp snapshot"
        );
        assert_eq!(
            Cmd::new("tab")
                .arg("t3")
                .flag("context", "c1")
                .context(Some("c2"))
                .to_string(),
            "chrome-cdp tab t3 --context c1"
        );
    }

    #[test]
    fn hints_chain_reasons_and_alternatives() {
        let hint = Hint::run(Cmd::new("snapshot").switch("interactive"))
            .why("refs change when the page does")
            .or(Hint::run(Cmd::new("find").arg("Sign in")));
        assert_eq!(
            String::from(hint),
            "chrome-cdp snapshot --interactive   (refs change when the page does), \
             or chrome-cdp find 'Sign in'"
        );
        assert_eq!(Hint::text("check the URL").to_string(), "check the URL");
    }
}
