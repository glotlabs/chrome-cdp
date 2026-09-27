//! Error kinds, exit codes and the error payload (plan 08 §3).

use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// What went wrong, as a stable machine-readable name.
///
/// The name is part of the `--json` contract (`"kind":"Timeout"`) and each
/// kind maps to one process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
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
    /// The session daemon cannot be reached or started.
    DaemonUnavailable,
    /// Chrome rejected a CDP call.
    ProtocolError,

    // Exit 5.
    /// A JavaScript dialog blocks the page.
    DialogOpen,
}

impl ErrorKind {
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
            | Self::DaemonUnavailable
            | Self::ProtocolError => 4,
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
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorInfo {
    /// The stable error name.
    pub kind: ErrorKind,
    /// One line saying what happened.
    pub message: String,
    /// The concrete next command to try.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Structured context (candidates, observed URL, CDP error code, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ErrorInfo {
    /// An error with no hint or details yet.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
            details: None,
        }
    }

    /// Sets the hint.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
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
    ///   hint: <hint>
    /// ```
    ///
    /// Details are only in the `--json` form; commands that want them in
    /// text put them in the message or hint.
    pub fn to_text(&self) -> String {
        let mut out = format!("error: {}\n", self.message);
        if let Some(hint) = &self.hint {
            out.push_str("  hint: ");
            out.push_str(hint);
            out.push('\n');
        }
        out
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
    fn json_form_omits_empty_fields() {
        let err = ErrorInfo::new(ErrorKind::BadArgs, "bad").details(json!({"arg": "x"}));
        assert_eq!(
            json!(err),
            json!({"kind": "BadArgs", "message": "bad", "details": {"arg": "x"}})
        );
    }
}
