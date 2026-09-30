//! The CLI <-> daemon wire format: line-delimited JSON-RPC 2.0 over the
//! session socket (overview §3).
//!
//! Each line is one [`Request`] or one [`Response`]. A command's result is
//! a [`crate::Output`] in `result`, and its failure an [`ErrorInfo`] in
//! `error.data`, so the CLI renders both with [`crate::render`] exactly as
//! if the command had run in-process. Method params are the daemon's own
//! types; this module only knows the envelope.
//!
//! `params` stays a [`Value`] here so a daemon can answer a method it does
//! not know (an older daemon, a newer CLI) with [`RpcError::METHOD_NOT_FOUND`]
//! instead of failing to parse the line.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ErrorInfo, ErrorKind};

/// The `jsonrpc` member of every message.
pub const JSONRPC: &str = "2.0";

/// A call from the CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Echoed in the response.
    pub id: u64,
    /// E.g. `open`, `cdp`, `status`.
    pub method: String,
    /// The method's params object (`null` when it takes none).
    #[serde(default)]
    pub params: Value,
    /// `--context`: the browser context (`c1`) the call acts in. Like
    /// `--session` it says where a command runs, not what it does, so it
    /// travels beside the params rather than in each method's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// The words of the command this call is for, as typed (plan 11.8):
    /// the daemon keeps them in the session's history. Only a command's
    /// first call carries them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<Vec<String>>,
    /// The project root of the CLI (plan 15): places in the page's code
    /// print as paths relative to it when the file is there. Like
    /// `--context` it travels beside the params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
}

impl Request {
    /// A request with `params` serialised from `params`.
    pub fn new(id: u64, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id,
            method: method.into(),
            params,
            context: None,
            line: None,
            root: None,
        }
    }

    /// The request, acting in browser context `context`.
    #[must_use]
    pub fn in_context(mut self, context: Option<String>) -> Self {
        self.context = context;
        self
    }
}

/// The daemon's answer to one [`Request`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// The request's id; `null` when the request could not be read.
    pub id: Option<u64>,
    /// The result, on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// A success.
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    /// A failure; `id` is `None` for a line that was not a request.
    pub fn err(id: Option<u64>, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id,
            result: None,
            error: Some(error),
        }
    }

    /// The result, or the error. A response with neither is an error.
    pub fn into_result(self) -> Result<Value, RpcError> {
        match (self.result, self.error) {
            (_, Some(error)) => Err(error),
            (Some(result), None) => Ok(result),
            (None, None) => Err(RpcError::new(
                RpcError::INVALID_REQUEST,
                "the response has neither result nor error",
            )),
        }
    }
}

/// A JSON-RPC error. Command failures use [`RpcError::COMMAND_FAILED`] and
/// carry the [`ErrorInfo`] for the user in `data`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// A JSON-RPC error code.
    pub code: i64,
    /// One line.
    pub message: String,
    /// The error to show the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<ErrorInfo>>,
}

impl RpcError {
    /// The line is not JSON.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON is not a request.
    pub const INVALID_REQUEST: i64 = -32600;
    /// The daemon has no such method (it is older than the CLI).
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// The params do not fit the method.
    pub const INVALID_PARAMS: i64 = -32602;
    /// The command ran and failed; `data` says how.
    pub const COMMAND_FAILED: i64 = -32000;

    /// An error with no [`ErrorInfo`].
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// A command failure.
    pub fn command(info: ErrorInfo) -> Self {
        Self {
            code: Self::COMMAND_FAILED,
            message: info.message.clone(),
            data: Some(Box::new(info)),
        }
    }

    /// The error to show the user: `data`, or a `DaemonUnavailable` error
    /// for a protocol-level failure (which means the CLI and daemon do not
    /// understand each other).
    pub fn into_error_info(self) -> ErrorInfo {
        match self.data {
            Some(info) => *info,
            None => ErrorInfo::new(
                ErrorKind::DaemonUnavailable,
                format!("the session daemon rejected the call: {}", self.message),
            ),
        }
    }
}

/// Reads one request line, as the daemon does with each line of its
/// socket. Anything but a JSON-RPC 2.0 request is an error to send back:
/// [`RpcError::PARSE_ERROR`] when the line is not JSON,
/// [`RpcError::INVALID_REQUEST`] when it is JSON but not a request. Never
/// panics: `fuzz/fuzz_targets/rpc_decode.rs` checks it on any input.
pub fn decode_request(line: &str) -> Result<Request, RpcError> {
    let req: Request = serde_json::from_str(line).map_err(|e| {
        let code = if e.is_data() {
            RpcError::INVALID_REQUEST
        } else {
            RpcError::PARSE_ERROR
        };
        RpcError::new(code, format!("not a request: {e}"))
    })?;
    if req.jsonrpc != JSONRPC {
        return Err(RpcError::new(
            RpcError::INVALID_REQUEST,
            format!("not a request: jsonrpc is {:?}, not \"2.0\"", req.jsonrpc),
        ));
    }
    Ok(req)
}

/// Reads one response line, as the CLI does with the daemon's answer.
/// The error is the line's fault, for the CLI to report as a daemon it
/// cannot understand.
pub fn decode_response(line: &str) -> Result<Response, String> {
    serde_json::from_str(line).map_err(|e| format!("bad response: {e}"))
}

/// A method's params as its own type; `null` reads as `{}`, so a method
/// whose params are all optional takes none.
pub fn decode_params<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        params
    };
    serde_json::from_value(params)
        .map_err(|e| RpcError::new(RpcError::INVALID_PARAMS, format!("bad params: {e}")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Request, Response, RpcError, decode_params, decode_request, decode_response};
    use crate::{ErrorInfo, ErrorKind};

    #[test]
    fn requests_and_responses_are_json_rpc_2() {
        let req = Request::new(7, "status", json!({"logs": true}));
        assert_eq!(
            json!(req),
            json!({"jsonrpc": "2.0", "id": 7, "method": "status", "params": {"logs": true}})
        );
        let bare: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"close"}"#).unwrap();
        assert!(bare.params.is_null());

        let ok = Response::ok(7, json!({"text": "x"}));
        assert_eq!(
            json!(ok),
            json!({"jsonrpc": "2.0", "id": 7, "result": {"text": "x"}})
        );
        assert_eq!(ok.into_result().unwrap(), json!({"text": "x"}));
    }

    #[test]
    fn command_failures_carry_the_error_info() {
        let info = ErrorInfo::new(ErrorKind::BrowserCrashed, "Chrome crashed").hint("x");
        let resp = Response::err(Some(3), RpcError::command(info.clone()));
        let back: Response = serde_json::from_value(json!(resp)).unwrap();
        let err = back.into_result().unwrap_err();
        assert_eq!(err.code, RpcError::COMMAND_FAILED);
        assert_eq!(err.into_error_info(), info);

        let unknown = RpcError::new(RpcError::METHOD_NOT_FOUND, "no method `tab`");
        let info = unknown.into_error_info();
        assert_eq!(info.kind, ErrorKind::DaemonUnavailable);
        assert_eq!(
            info.message,
            "the session daemon rejected the call: no method `tab`"
        );
    }

    #[test]
    fn decoding_tells_bad_json_from_bad_requests() {
        #[derive(serde::Deserialize, Debug)]
        struct P {
            #[serde(default)]
            n: u32,
        }

        let req = decode_request(r#"{"jsonrpc":"2.0","id":2,"method":"hello"}"#).unwrap();
        assert_eq!((req.id, req.method.as_str()), (2, "hello"));
        for (line, code) in [
            ("{\"jsonrpc\":", RpcError::PARSE_ERROR),
            ("hello", RpcError::PARSE_ERROR),
            ("[1,2]", RpcError::INVALID_REQUEST),
            (
                r#"{"jsonrpc":"2.0","id":-1,"method":"x"}"#,
                RpcError::INVALID_REQUEST,
            ),
            (
                r#"{"jsonrpc":"1.0","id":1,"method":"x"}"#,
                RpcError::INVALID_REQUEST,
            ),
        ] {
            let err = decode_request(line).unwrap_err();
            assert_eq!(err.code, code, "{line}");
            assert!(
                err.message.starts_with("not a request: "),
                "{}",
                err.message
            );
        }
        // serde_json's recursion limit keeps a deep line from overflowing
        // the stack.
        let deep = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"x","params":{}{}}}"#,
            "[".repeat(100_000),
            "]".repeat(100_000)
        );
        assert_eq!(
            decode_request(&deep).unwrap_err().code,
            RpcError::PARSE_ERROR
        );

        assert!(decode_response("{}").is_err());
        assert!(decode_response(r#"{"jsonrpc":"2.0","id":null,"result":1}"#).is_ok());

        assert_eq!(decode_params::<P>(serde_json::Value::Null).unwrap().n, 0);
        let err = decode_params::<P>(json!({"n": "x"})).unwrap_err();
        assert_eq!(err.code, RpcError::INVALID_PARAMS);
    }
}
