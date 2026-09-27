//! Mapping library errors to user-facing [`ErrorInfo`]s (plan 08 §3).

use cdp::CdpError;
use protocol::format;
use protocol::{ErrorInfo, ErrorKind};
use serde_json::json;

/// Maps a CDP client error to its kind, message and generic hint. Commands
/// replace the hint when they know a better next step.
pub(crate) fn from_cdp(err: &CdpError) -> ErrorInfo {
    match err {
        CdpError::Protocol {
            method,
            code,
            message,
            data,
        } => {
            let info = ErrorInfo::new(ErrorKind::ProtocolError, format!("{method}: {message}"))
                .details(json!({ "method": method, "code": code, "data": data }));
            match code {
                // Method not found / invalid params.
                -32601 | -32602 => info.hint(format!(
                    "check the method and its params at https://chromedevtools.github.io/devtools-protocol/tot/{}/",
                    method.split('.').next().unwrap_or(method)
                )),
                _ => info,
            }
        }
        CdpError::Disconnected { reason } => ErrorInfo::new(
            ErrorKind::BrowserCrashed,
            format!("Chrome went away: {reason}"),
        )
        .hint("run the command again; add -vv to see Chrome's output"),
        CdpError::Timeout { method, after } => ErrorInfo::new(
            ErrorKind::Timeout,
            format!(
                "{method}: no response from Chrome after {}",
                format::duration(*after)
            ),
        )
        .hint(format!(
            "retry with a longer timeout, e.g. --timeout {}",
            format::duration(*after * 2)
        ))
        .details(json!({ "method": method, "timeout_ms": after.as_millis() })),
        CdpError::Serialize { .. } | CdpError::Decode { .. } => {
            ErrorInfo::new(ErrorKind::ProtocolError, err.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cdp::CdpError;
    use protocol::ErrorKind;

    use super::from_cdp;

    #[test]
    fn unknown_method_points_at_the_domain_docs() {
        let err = from_cdp(&CdpError::Protocol {
            method: "Nope.nope".into(),
            code: -32601,
            message: "'Nope.nope' wasn't found".into(),
            data: None,
        });
        assert_eq!(err.kind, ErrorKind::ProtocolError);
        assert_eq!(
            err.to_text(),
            "error: Nope.nope: 'Nope.nope' wasn't found\n  \
             hint: check the method and its params at https://chromedevtools.github.io/devtools-protocol/tot/Nope/\n"
        );
    }

    #[test]
    fn timeout_suggests_a_longer_one() {
        let err = from_cdp(&CdpError::Timeout {
            method: "Runtime.evaluate".into(),
            after: Duration::from_millis(500),
        });
        assert_eq!(err.exit_code(), 3);
        assert_eq!(
            err.to_text(),
            "error: Runtime.evaluate: no response from Chrome after 500ms\n  \
             hint: retry with a longer timeout, e.g. --timeout 1s\n"
        );
    }

    #[test]
    fn disconnect_is_a_browser_crash() {
        let err = from_cdp(&CdpError::Disconnected {
            reason: "pipe closed".into(),
        });
        assert_eq!(err.kind, ErrorKind::BrowserCrashed);
        assert_eq!(err.exit_code(), 4);
    }
}
