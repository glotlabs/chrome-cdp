//! Error kinds, exit codes and the error payload (plan 08 §3).

use std::fmt;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::style::Style;

/// What went wrong, as a stable machine-readable name.
///
/// The name is part of the `--json` contract (`"kind":"Timeout"`) and each
/// kind maps to one process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ErrorKind {
    // Exit 1: an action or assertion failed on the page.
    /// No element matches the locator.
    NotFound,
    /// The locator matches several elements where one is needed.
    Ambiguous,
    /// Another element covers the target.
    Obscured,
    /// The target is disabled.
    NotEnabled,
    /// The ref is from an older snapshot.
    RefStale,
    /// The ref's element was removed from the page.
    RefDetached,
    /// An `expect` assertion failed.
    ExpectFailed,
    /// JavaScript threw.
    EvalError,
    /// Navigation failed (DNS, TLS, HTTP error page, …).
    NavigationFailed,

    // Exit 2: the command line is wrong.
    /// Bad flags or arguments.
    BadArgs,
    /// The locator does not parse.
    BadLocator,

    // Exit 3.
    /// Something did not happen in time.
    Timeout,

    // Exit 4: the browser, daemon or environment failed.
    /// No Chrome binary was found.
    ChromeNotFound,
    /// Chrome was found but could not be started.
    LaunchFailed,
    /// Chrome exited or the connection to it dropped.
    BrowserCrashed,
    /// A tab's page crashed (its renderer process died); `reload` brings
    /// it back.
    PageCrashed,
    /// The session daemon cannot be reached or started.
    DaemonUnavailable,
    /// Chrome rejected a CDP call.
    ProtocolError,
    /// Downloading or unpacking a browser (`chrome-cdp install`) or a
    /// release (`chrome-cdp upgrade`) failed.
    InstallFailed,

    // Exit 5.
    /// A JavaScript dialog blocks the page.
    DialogOpen,
}

impl ErrorKind {
    /// Every kind, in exit-code order.
    pub const ALL: [Self; 20] = [
        Self::NotFound,
        Self::Ambiguous,
        Self::Obscured,
        Self::NotEnabled,
        Self::RefStale,
        Self::RefDetached,
        Self::ExpectFailed,
        Self::EvalError,
        Self::NavigationFailed,
        Self::BadArgs,
        Self::BadLocator,
        Self::Timeout,
        Self::ChromeNotFound,
        Self::LaunchFailed,
        Self::BrowserCrashed,
        Self::PageCrashed,
        Self::DaemonUnavailable,
        Self::ProtocolError,
        Self::InstallFailed,
        Self::DialogOpen,
    ];

    /// The process exit code for this kind (plan 08 §3).
    pub fn exit_code(self) -> u8 {
        match self {
            Self::NotFound
            | Self::Ambiguous
            | Self::Obscured
            | Self::NotEnabled
            | Self::RefStale
            | Self::RefDetached
            | Self::ExpectFailed
            | Self::EvalError
            | Self::NavigationFailed => 1,
            Self::BadArgs | Self::BadLocator => 2,
            Self::Timeout => 3,
            Self::ChromeNotFound
            | Self::LaunchFailed
            | Self::BrowserCrashed
            | Self::PageCrashed
            | Self::DaemonUnavailable
            | Self::ProtocolError
            | Self::InstallFailed => 4,
            Self::DialogOpen => 5,
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// A command failure as the user sees it: what happened, and the next
/// command to run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorInfo {
    /// The stable error name.
    pub kind: ErrorKind,
    /// One line saying what happened.
    pub message: String,
    /// The concrete next command to try.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Short labelled facts shown between the message and the hint in the
    /// text form (`covering: div "We use cookies" [e88]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// Structured context (candidates, observed URL, CDP error code, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// One labelled line of an error's text form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Note {
    /// A short lowercase label, e.g. `covering`.
    pub label: String,
    /// One line of text, or several for a block such as a diff (the
    /// text form prints them indented under the label).
    pub text: String,
}

/// Build an error with [`ErrorInfo::new`] and chain [`note`](Self::note),
/// [`hint`](Self::hint) (usually from a [`Hint`](crate::Hint)) and
/// [`details`](Self::details).
impl ErrorInfo {
    /// An error with no hint, notes or details yet. Line breaks in `message`
    /// become spaces: the message is one line.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: one_line(message.into()),
            hint: None,
            notes: Vec::new(),
            details: None,
        }
    }

    /// Sets the hint (a [`Hint`](crate::Hint), [`Cmd`](crate::Cmd) or text).
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(one_line(hint.into()));
        self
    }

    /// Adds a labelled line to the text form.
    #[must_use]
    pub fn note(mut self, label: impl Into<String>, text: impl Into<String>) -> Self {
        self.notes.push(Note {
            label: label.into(),
            text: one_line(text.into()),
        });
        self
    }

    /// Adds a labelled block of lines (a diff) to the text form: the
    /// label on a line of its own, then each line indented under it.
    #[must_use]
    pub fn block(mut self, label: impl Into<String>, text: impl Into<String>) -> Self {
        self.notes.push(Note {
            label: label.into(),
            text: text.into().trim_end_matches('\n').to_owned(),
        });
        self
    }

    /// Sets the details.
    #[must_use]
    pub fn details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// The process exit code for this error.
    pub fn exit_code(&self) -> u8 {
        self.kind.exit_code()
    }

    /// The text form for stderr:
    ///
    /// ```text
    /// error: <message>
    ///   <label>: <note>
    ///   hint: <hint>
    /// ```
    ///
    /// Details are only in the `--json` form; commands that want them in
    /// text add notes.
    pub fn to_text(&self) -> String {
        self.render_text(Style::PLAIN)
    }

    /// [`to_text`](Self::to_text) with the labels styled.
    pub fn render_text(&self, style: Style) -> String {
        let mut out = format!("{} {}\n", style.error("error:"), self.message);
        for note in &self.notes {
            let label = style.label(&format!("{}:", note.label));
            if note.text.contains('\n') {
                let _ = writeln!(out, "  {label}");
                for line in note.text.lines() {
                    let _ = writeln!(out, "    {line}");
                }
            } else {
                let _ = writeln!(out, "  {label} {}", note.text);
            }
        }
        if let Some(hint) = &self.hint {
            let _ = writeln!(out, "  {} {hint}", style.hint("hint:"));
        }
        out
    }
}

/// Joins the lines of `text` with spaces.
fn one_line(text: String) -> String {
    if text.contains(['\n', '\r']) {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        text
    }
}

impl fmt::Display for ErrorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ErrorInfo {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ErrorInfo, ErrorKind};

    #[test]
    fn exit_codes_follow_plan_08() {
        assert_eq!(ErrorKind::NotFound.exit_code(), 1);
        assert_eq!(ErrorKind::BadArgs.exit_code(), 2);
        assert_eq!(ErrorKind::Timeout.exit_code(), 3);
        assert_eq!(ErrorKind::ChromeNotFound.exit_code(), 4);
        assert_eq!(ErrorKind::ProtocolError.exit_code(), 4);
        assert_eq!(ErrorKind::PageCrashed.exit_code(), 4);
        assert_eq!(ErrorKind::DialogOpen.exit_code(), 5);
    }

    #[test]
    fn kind_serialises_as_its_name() {
        assert_eq!(json!(ErrorKind::RefStale), json!("RefStale"));
        assert_eq!(ErrorKind::RefStale.to_string(), "RefStale");
    }

    #[test]
    fn text_form_has_message_then_hint() {
        let err =
            ErrorInfo::new(ErrorKind::ChromeNotFound, "no Chrome found").hint("chrome-cdp install");
        assert_eq!(
            err.to_text(),
            "error: no Chrome found\n  hint: chrome-cdp install\n"
        );
        let bare = ErrorInfo::new(ErrorKind::Timeout, "too slow");
        assert_eq!(bare.to_text(), "error: too slow\n");
    }

    #[test]
    fn notes_sit_between_message_and_hint() {
        let err = ErrorInfo::new(
            ErrorKind::Obscured,
            "element e6 is covered by another element",
        )
        .note("covering", r#"div "We use cookies" [e88]"#)
        .hint(r#"chrome-cdp click e89   (button "Accept" inside the banner)"#);
        assert_eq!(
            err.to_text(),
            "error: element e6 is covered by another element\n  \
             covering: div \"We use cookies\" [e88]\n  \
             hint: chrome-cdp click e89   (button \"Accept\" inside the banner)\n"
        );
    }

    #[test]
    fn blocks_are_indented_under_their_label() {
        let err = ErrorInfo::new(ErrorKind::ExpectFailed, "expect-snapshot login failed")
            .note("baseline", "/p/login.txt")
            .block("diff", "@@ -1 +1 @@\n-- button \"A\"\n+- button \"B\"\n")
            .block("one", "line")
            .hint("chrome-cdp expect-snapshot login --update");
        assert_eq!(
            err.to_text(),
            "error: expect-snapshot login failed\n  \
             baseline: /p/login.txt\n  \
             diff:\n    \
             @@ -1 +1 @@\n    \
             -- button \"A\"\n    \
             +- button \"B\"\n  \
             one: line\n  \
             hint: chrome-cdp expect-snapshot login --update\n"
        );
    }

    #[test]
    fn messages_are_one_line() {
        let err = ErrorInfo::new(
            ErrorKind::EvalError,
            "Uncaught Error: x\n    at <anonymous>:1:7\n",
        )
        .hint("a\nb");
        assert_eq!(err.message, "Uncaught Error: x at <anonymous>:1:7");
        assert_eq!(err.hint.as_deref(), Some("a b"));
    }

    #[test]
    fn every_kind_is_listed_once() {
        let mut names: Vec<_> = ErrorKind::ALL.iter().map(ToString::to_string).collect();
        names.dedup();
        assert_eq!(names.len(), ErrorKind::ALL.len());
        assert!(
            ErrorKind::ALL
                .windows(2)
                .all(|w| w[0].exit_code() <= w[1].exit_code())
        );
    }

    #[test]
    fn round_trips_through_json() {
        let err = ErrorInfo::new(ErrorKind::RefStale, "stale")
            .note("ref", "e5")
            .hint("chrome-cdp snapshot")
            .details(json!({"ref": "e5"}));
        let back: ErrorInfo = serde_json::from_value(json!(err)).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn json_form_omits_empty_fields() {
        let err = ErrorInfo::new(ErrorKind::BadArgs, "bad").details(json!({"arg": "x"}));
        assert_eq!(
            json!(err),
            json!({"kind": "BadArgs", "message": "bad", "details": {"arg": "x"}})
        );
    }
}
