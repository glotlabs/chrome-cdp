//! `chrome-cdp __direct cdp`: a temporary, daemon-less raw CDP call
//! (plan 01, milestone 1.4).
//!
//! It launches Chrome with `browser::launch` (milestone 2.3) and a
//! temporary profile unless `--profile`/`--user-data-dir` say otherwise,
//! makes one call, prints the result, and closes Chrome. Chrome is found by
//! `browser::discover` (2.1), and with `--auto-install` installed first if
//! there is none (2.2). Plan 02's daemon-backed `chrome-cdp cdp` replaces
//! this; delete this module then.

use browser::Host;
use cdp::domains::target;
use cdp::{CdpError, Connection};
use protocol::{Cmd, ErrorInfo, ErrorKind, Output};
use serde_json::{Map, Value};
use tokio::signal::unix::{Signal, SignalKind, signal};

use crate::cli::{DirectCdpArgs, GlobalArgs, Target};
use crate::{errors, input};

/// Runs `__direct cdp`.
pub(crate) fn cdp(args: &DirectCdpArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    // Check everything that needs no browser before starting one.
    check_method(&args.method)?;
    let params = parse_params(args.params.as_deref())?;
    let launch = &args.launch;
    let found = crate::install::find_chrome(
        launch.chrome.as_deref(),
        launch.auto_install,
        global.timeout,
    )?;
    tracing::info!(
        chrome = %found.path.display(),
        version = found.version.as_ref().map(ToString::to_string),
        "using Chrome {}",
        found.source.describe()
    );
    let options = launch.options(global.timeout);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot start the runtime: {e}"),
            )
        })?;
    let outcome = runtime.block_on(async {
        let mut signals = Signals::register();
        let work = async {
            let chrome = browser::launch(&found, &options, &Host::current()).await?;
            let result = call(chrome.connection(), args, params).await;
            let warnings = chrome.warnings.clone();
            chrome.close().await;
            result.map(|value| (value, warnings))
        };
        // A signal drops `work`, and with it the `Launched`, which kills
        // Chrome and deletes the temporary profile.
        tokio::select! {
            result = work => Ok(result),
            signal = signals.recv() => Err(signal),
        }
    });
    drop(runtime);
    let (value, launch_warnings) = match outcome {
        Ok(result) => result,
        Err((name, code)) => {
            crate::output::stderr(&format!("error: interrupted by {name}; Chrome was closed\n"));
            std::process::exit(code);
        }
    }
    // An error has no warnings, so a warning such as "Chrome is too old"
    // becomes a note: it may be the reason.
    .map_err(|info| {
        found
            .warnings
            .iter()
            .fold(info, |info, w| info.note("warning", w))
    })?;

    let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let compact = value.to_string();
    let output = Output::new(value, pretty)
        .quiet(compact)
        .limit(args.max_chars, "--max-chars 0 for all of it");
    Ok(found
        .warnings
        .into_iter()
        .chain(launch_warnings)
        .fold(output, Output::warning))
}

/// SIGINT, SIGTERM and SIGHUP, registered before Chrome starts so none is
/// missed.
struct Signals(Vec<(Signal, &'static str, i32)>);

impl Signals {
    fn register() -> Self {
        let kinds = [
            (SignalKind::interrupt(), "SIGINT", 130),
            (SignalKind::terminate(), "SIGTERM", 143),
            (SignalKind::hangup(), "SIGHUP", 129),
        ];
        let mut streams = Vec::new();
        for (kind, name, code) in kinds {
            match signal(kind) {
                Ok(stream) => streams.push((stream, name, code)),
                Err(e) => tracing::warn!("cannot handle {name}: {e}"),
            }
        }
        Self(streams)
    }

    /// Waits for one; returns its name and the shell's exit code for it
    /// (128 + its number).
    async fn recv(&mut self) -> (&'static str, i32) {
        std::future::poll_fn(|cx| {
            for (stream, name, code) in &mut self.0 {
                if stream.poll_recv(cx).is_ready() {
                    return std::task::Poll::Ready((*name, *code));
                }
            }
            std::task::Poll::Pending
        })
        .await
    }
}

fn bad_args(message: impl Into<String>) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadArgs, message)
}

/// Accepts `Domain.method`, e.g. `Browser.getVersion`.
fn check_method(method: &str) -> Result<(), ErrorInfo> {
    let ok = method.split_once('.').is_some_and(|(domain, name)| {
        domain.starts_with(|c: char| c.is_ascii_uppercase())
            && !name.is_empty()
            && [domain, name]
                .iter()
                .all(|part| part.chars().all(|c| c.is_ascii_alphanumeric()))
    });
    if ok {
        Ok(())
    } else {
        Err(
            bad_args(format!("`{method}` is not a CDP method name")).hint(
                "use Domain.method, e.g. chrome-cdp __direct cdp Browser.getVersion --chrome <path>",
            ),
        )
    }
}

/// Parses the params argument: absent means `{}`, `-` reads stdin.
fn parse_params(arg: Option<&str>) -> Result<Map<String, Value>, ErrorInfo> {
    let Some(arg) = arg else {
        return Ok(Map::new());
    };
    let text = input::text_arg(arg, "params")?;
    let example = r#"e.g. '{"expression":"1+1"}'"#;
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(bad_args(format!(
            "params must be a JSON object, not {}",
            json_type(&other)
        ))
        .hint(format!("wrap them in an object, {example}"))),
        Err(e) => Err(bad_args(format!("params are not valid JSON: {e}"))
            .hint(format!("quote the JSON for the shell, {example}"))),
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Makes the user's call on the requested target.
async fn call(
    conn: &Connection,
    args: &DirectCdpArgs,
    params: Map<String, Value>,
) -> Result<Value, ErrorInfo> {
    let session = match args.target {
        Target::Browser => conn.browser(),
        Target::Page => {
            let created = conn
                .browser()
                .call(target::CreateTargetParams::new("about:blank".into()))
                .await
                .map_err(|e| errors::from_cdp(&e))?;
            conn.attach(created.target_id)
                .await
                .map_err(|e| errors::from_cdp(&e))?
        }
    };
    session
        .call_raw(&args.method, Value::Object(params))
        .await
        .map_err(|e| {
            let info = errors::from_cdp(&e);
            let browser_domain = ["Browser.", "Target."]
                .iter()
                .any(|d| args.method.starts_with(d));
            match e {
                CdpError::Protocol { code: -32601, .. }
                    if args.target == Target::Browser && !browser_domain =>
                {
                    info.hint(format!(
                        "if {m} is a page method, send it to a page: {cmd}",
                        m = args.method,
                        cmd = Cmd::new("__direct cdp")
                            .flag("target", "page")
                            .arg(&args.method)
                            .raw("…")
                    ))
                }
                _ => info,
            }
        })
}

#[cfg(test)]
mod tests {
    use protocol::ErrorKind;
    use serde_json::json;

    use super::{check_method, parse_params};

    #[test]
    fn method_names() {
        assert!(check_method("Browser.getVersion").is_ok());
        assert!(check_method("DOMSnapshot.captureSnapshot").is_ok());
        for bad in [
            "",
            "Browser",
            "Browser.",
            ".getVersion",
            "browser.getVersion",
            "A.b.c",
        ] {
            let err = check_method(bad).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadArgs, "{bad}");
        }
    }

    #[test]
    fn params() {
        assert_eq!(
            parse_params(None).unwrap(),
            json!({}).as_object().unwrap().clone()
        );
        assert_eq!(
            parse_params(Some(r#"{"expression":"1+1"}"#)).unwrap()["expression"],
            json!("1+1")
        );
        let err = parse_params(Some("[1]")).unwrap_err();
        assert_eq!(err.message, "params must be a JSON object, not an array");
        let err = parse_params(Some("{expression:1}")).unwrap_err();
        assert!(
            err.message.starts_with("params are not valid JSON: "),
            "{err}"
        );
        assert_eq!(err.exit_code(), 2);
    }
}
