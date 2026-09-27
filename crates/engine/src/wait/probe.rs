//! The polled half of the wait engine: a probe (`probe.js`) that checks a
//! condition in the page on every animation frame until it holds or the
//! time is up, in one `Runtime.callFunctionOn`. Element, text and load
//! probes run in chrome-cdp's isolated world, so page scripts cannot see
//! or break them; `--fn` runs in the page's own world, where its globals
//! are. When the document goes away mid-wait (a navigation), the probe
//! starts again in the new one.

use std::time::Duration;

use cdp::CdpError;
use cdp::domains::runtime;
use protocol::{ErrorInfo, ErrorKind, Hint};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{Condition, DOM_QUIET, Met, Unmet};
use crate::errors::from_cdp;
use crate::frames::World;
use crate::tabs::Page;
use crate::{Engine, Running};

/// The probe; `__FN__` becomes `--fn`'s expression (or `null`).
const PROBE_JS: &str = include_str!("probe.js");

/// Resolves once the DOM has not changed for a while.
const QUIET_JS: &str = include_str!("quiet.js");

/// How much longer than its budget a probe call may take to answer.
const CALL_SLACK: Duration = Duration::from_secs(2);

/// What one probe run saw.
#[derive(Debug, Default)]
struct Probed {
    done: bool,
    observed: Value,
    value: Option<Value>,
}

impl Engine {
    /// Polls `condition` (a load, element, text or `--fn` condition) in
    /// `page`'s main frame until it holds or `deadline` passes.
    pub(super) async fn probe_until(
        &self,
        running: &Running,
        page: &Page,
        condition: &Condition,
        deadline: Instant,
    ) -> Result<Result<Met, Unmet>, ErrorInfo> {
        let (spec, expression) = spec(condition);
        let world = if expression.is_some() {
            World::Main
        } else {
            World::Isolated
        };
        let declaration = PROBE_JS.replace(
            "__FN__",
            &expression.map_or_else(|| "null".to_owned(), |e| format!("() => (\n{e}\n)")),
        );
        let mut last = Probed::default();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() && !last.observed.is_null() {
                break;
            }
            let context = match self
                .frame_context(
                    running,
                    page,
                    None,
                    world,
                    left.max(Duration::from_millis(1)),
                )
                .await
            {
                Ok(context) => context,
                // No document to look in (yet).
                Err(e) if e.kind == ErrorKind::Timeout => {
                    if last.observed.is_null() {
                        last.observed = json!({ "document": "none" });
                    }
                    break;
                }
                Err(e) => return Err(e),
            };
            let budget = deadline.saturating_duration_since(Instant::now());
            let mut params = runtime::CallFunctionOnParams::new(declaration.clone());
            params.execution_context_id = Some(context.context_id);
            params.arguments = Some(vec![
                argument(spec.clone()),
                argument(json!(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX))),
            ]);
            params.return_by_value = Some(true);
            params.await_promise = Some(true);
            let result = context
                .session
                .with_timeout(budget + CALL_SLACK)
                .call(params)
                .await;
            let result = match result {
                Ok(result) => result,
                Err(e) if is_gone(&e) => {
                    // The document went away; look in the next one.
                    running.tabs.forget_context(page.id, &context);
                    continue;
                }
                Err(CdpError::Timeout { .. }) => {
                    last.observed = json!({ "page": "not responding (a script may be busy)" });
                    break;
                }
                Err(e) => return Err(from_cdp(&e, &self.session)),
            };
            if let Some(exception) = result.exception_details {
                // Our wrapper catches what the expression throws at run
                // time, so this is a syntax error in it.
                let message = exception
                    .exception
                    .and_then(|e| e.description)
                    .unwrap_or(exception.text);
                let first = message.lines().next().unwrap_or_default().to_owned();
                return Err(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!("the --fn expression does not compile: {first}"),
                )
                .hint(Hint::text(
                    "--fn takes one JavaScript expression, e.g. --fn 'window.appReady === true'",
                ))
                .details(json!({ "exception": message })));
            }
            let value = result.result.value.unwrap_or(Value::Null);
            if let Some(fatal) = value["fatal"].as_str() {
                return Err(bad_selector(condition, fatal));
            }
            last = Probed {
                done: value["done"].as_bool().unwrap_or(false),
                observed: value["observed"].clone(),
                value: value.get("value").cloned(),
            };
            if last.done || Instant::now() >= deadline {
                break;
            }
        }
        Ok(if last.done {
            Ok(met(condition, last))
        } else {
            Err(unmet(condition, &last.observed))
        })
    }

    /// Waits up to `max` for `page`'s DOM to go [`DOM_QUIET`] without a
    /// change; `false` if it did not (or there was no document).
    pub(super) async fn dom_quiet(
        &self,
        running: &Running,
        page: &Page,
        max: Duration,
    ) -> Result<bool, ErrorInfo> {
        if max.is_zero() {
            return Ok(false);
        }
        let context = match self
            .frame_context(running, page, None, World::Isolated, max)
            .await
        {
            Ok(context) => context,
            Err(e) if e.kind == ErrorKind::Timeout => return Ok(false),
            Err(e) => return Err(e),
        };
        let mut params = runtime::CallFunctionOnParams::new(QUIET_JS.to_owned());
        params.execution_context_id = Some(context.context_id);
        params.arguments = Some(vec![
            argument(json!(u64::try_from(DOM_QUIET.as_millis()).unwrap_or(300))),
            argument(json!(u64::try_from(max.as_millis()).unwrap_or(u64::MAX))),
        ]);
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        let result = context
            .session
            .with_timeout(max + CALL_SLACK)
            .call(params)
            .await;
        Ok(match result {
            Ok(result) => result
                .result
                .value
                .is_some_and(|v| v["quiet"] == json!(true)),
            Err(e) if is_gone(&e) => {
                running.tabs.forget_context(page.id, &context);
                false
            }
            Err(e @ CdpError::Disconnected { .. }) => return Err(from_cdp(&e, &self.session)),
            Err(_) => false,
        })
    }
}

fn argument(value: Value) -> runtime::CallArgument {
    runtime::CallArgument {
        value: Some(value),
        unserializable_value: None,
        object_id: None,
    }
}

/// Whether a call failed because its document went away (it navigated):
/// the probe never ran or its result is moot, so it runs again.
fn is_gone(err: &CdpError) -> bool {
    matches!(err, CdpError::Protocol { message, .. }
        if message.starts_with("Cannot find context with specified id")
            || message.starts_with("Execution context was destroyed")
            || message.starts_with("Inspected target navigated or closed"))
}

/// The probe's spec for `condition`, and `--fn`'s expression.
fn spec(condition: &Condition) -> (Value, Option<&str>) {
    match condition {
        Condition::Load { dom_content_loaded } => (
            json!({ "kind": "load", "domContentLoaded": dom_content_loaded }),
            None,
        ),
        Condition::Element { locator, state } => (
            json!({ "kind": "element", "locator": locator, "state": state }),
            None,
        ),
        Condition::Text { text, within, gone } => (
            json!({ "kind": "text", "text": text, "within": within, "gone": gone }),
            None,
        ),
        Condition::Function(expression) => (json!({ "kind": "fn" }), Some(expression)),
        _ => (Value::Null, None),
    }
}

/// A selector the page's `querySelectorAll` or `evaluate` rejected.
fn bad_selector(condition: &Condition, why: &str) -> ErrorInfo {
    let locator = match condition {
        Condition::Element { locator, .. } => Some(locator),
        Condition::Text { within, .. } => within.as_ref(),
        _ => None,
    };
    let what = locator.map_or_else(|| "the selector".to_owned(), |l| format!("`{l}`"));
    let first = why.lines().next().unwrap_or_default();
    ErrorInfo::new(
        ErrorKind::BadLocator,
        format!("{what} is not a valid selector: {first}"),
    )
    .hint(Hint::text("check the CSS or XPath syntax"))
}

fn met(condition: &Condition, probed: Probed) -> Met {
    match condition {
        Condition::Load {
            dom_content_loaded: false,
        } => Met::says("load event reached"),
        Condition::Load {
            dom_content_loaded: true,
        } => Met::says("DOMContentLoaded reached"),
        Condition::Element { locator, state } => {
            let count = probed.observed["count"].as_u64().unwrap_or(0);
            let matches = if count > 1 {
                format!(" ({count} matches)")
            } else {
                String::new()
            };
            Met {
                text: format!("{locator} is {}{matches}", state.as_str()),
                count: Some(count),
                ..Met::default()
            }
        }
        Condition::Text { text, gone, .. } => Met::says(if *gone {
            format!("text {text:?} is gone")
        } else {
            format!("text {text:?} is visible")
        }),
        Condition::Function(_) => {
            let value = probed.value.unwrap_or(Value::Null);
            Met {
                text: format!("--fn returned {value}"),
                value: Some(value),
                ..Met::default()
            }
        }
        _ => Met::default(),
    }
}

/// The observations of a probe that timed out.
fn unmet(condition: &Condition, observed: &Value) -> Unmet {
    let unmet = Unmet {
        observed: observed.as_object().cloned().unwrap_or_default(),
        ..Unmet::default()
    };
    if observed["document"].is_string() {
        return unmet.note("observed", "the page has no document to look in yet");
    }
    if let Some(page) = observed["page"].as_str() {
        return unmet.note("observed", format!("the page is {page}"));
    }
    match condition {
        Condition::Load { .. } => unmet.note(
            "observed",
            format!(
                "document.readyState is {}",
                observed["readyState"]
                    .as_str()
                    .map_or_else(|| "unknown".to_owned(), |s| format!("{s:?}"))
            ),
        ),
        Condition::Element { .. } => {
            let count = observed["count"].as_u64().unwrap_or(0);
            let text = match count {
                0 => "no element matches".to_owned(),
                1 => format!(
                    "1 match, {}, {}",
                    if observed["visible"].as_u64() == Some(1) {
                        "visible"
                    } else {
                        "not visible"
                    },
                    if observed["enabled"].as_u64() == Some(1) {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ),
                n => format!(
                    "{n} matches, {} visible, {} enabled",
                    observed["visible"].as_u64().unwrap_or(0),
                    observed["enabled"].as_u64().unwrap_or(0)
                ),
            };
            unmet.note("observed", text)
        }
        Condition::Text { within, gone, .. } => {
            if observed["scope"] == json!(false)
                && let Some(within) = within
            {
                return unmet.note("observed", format!("no element matches {within}"));
            }
            unmet.note(
                "observed",
                if *gone {
                    "the text is still visible"
                } else {
                    "not in the page's visible text"
                },
            )
        }
        Condition::Function(_) => match observed["error"].as_str() {
            Some(error) => unmet.note("last error", error.to_owned()),
            None => unmet.note(
                "last value",
                observed.get("value").unwrap_or(&Value::Null).to_string(),
            ),
        },
        _ => unmet,
    }
}
