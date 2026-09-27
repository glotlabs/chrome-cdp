//! The `--json` output envelope (plan 08 §2).

use serde::Serialize;
use serde_json::Value;

use crate::ErrorInfo;

/// The one JSON object a command prints on stdout with `--json`:
///
/// ```json
/// {"ok":true,"data":{},"warnings":[]}
/// {"ok":false,"error":{"kind":"Timeout","message":"…","hint":"…"}}
/// ```
///
/// `warnings` and `events` are omitted when empty.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope {
    /// Whether the command succeeded.
    pub ok: bool,
    /// The result, on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// The failure, when `ok` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    /// Non-fatal notes (the text form prints them on stderr).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Page events observed while the command ran (dialogs, navigations, …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Value>,
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
        }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Envelope;
    use crate::{ErrorInfo, ErrorKind};

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
        let mut env = Envelope::ok(json!(1));
        env.warnings.push("careful".into());
        assert_eq!(
            env.to_json(),
            r#"{"ok":true,"data":1,"warnings":["careful"]}"#
        );
    }
}
