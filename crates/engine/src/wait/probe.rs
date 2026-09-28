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

use super::{Condition, Met, Unmet};
use crate::errors::from_cdp;
use crate::frames::World;
use crate::injected;
use crate::locator::{FrameOnly, LocateOptions, Locator, Place, bad_locator};
use crate::request::ElementState;
use crate::tabs::{FrameContext, Page};
use crate::{Engine, Running};

/// The probe; `__FN__` becomes `--fn`'s expression (or `null`).
const PROBE_JS: &str = include_str!("probe.js");

/// Resolves once the DOM has not changed for a while.
const QUIET_JS: &str = include_str!("quiet.js");

/// Marks a probe's `fatal` error as one in the command's arguments (an
/// `expect` regex), not in its locator.
pub(crate) const ARG_ERROR: &str = "bad_args";

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
    /// `page` until it holds or `deadline` passes: in the main frame, or
    /// where the condition's locator points (a ref's element, `frame=`).
    pub(crate) async fn probe_until(
        &self,
        running: &Running,
        page: &Page,
        condition: &Condition,
        options: &LocateOptions,
        deadline: Instant,
    ) -> Result<Result<Met, Unmet>, ErrorInfo> {
        let (_, expression) = spec(condition, None);
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
        let mut note = None;
        // `expect`'s tries over every probe run: the probe counts its own,
        // from zero in each document.
        let mut tries = 0;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() && !last.observed.is_null() {
                break;
            }
            let at = self
                .probe_place(running, page, condition, options, world, left)
                .await;
            let (context, place) = match at {
                Ok(At::Place(context, place)) => (context, place),
                Ok(At::Absent(observed)) => {
                    tries += 1;
                    last = Probed {
                        done: absent_is_met(condition),
                        observed,
                        value: None,
                    };
                    if last.done || Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(ABSENT_POLL.min(left)).await;
                    continue;
                }
                // No document to look in (yet).
                Err(e) if e.kind == ErrorKind::Timeout => {
                    if last.observed.is_null() {
                        last.observed = json!({ "document": "none" });
                    }
                    break;
                }
                Err(e) => return Err(e),
            };
            if let Some(place) = &place {
                note.clone_from(&place.note);
            }
            let (spec, _) = spec(condition, place.as_ref());
            let root = place.as_ref().map_or_else(
                || argument(Value::Null),
                crate::locator::Place::root_argument,
            );
            let budget = deadline.saturating_duration_since(Instant::now());
            let mut params = runtime::CallFunctionOnParams::new(declaration.clone());
            params.arguments = Some(vec![
                argument(spec),
                argument(json!(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX))),
                root,
            ]);
            params.return_by_value = Some(true);
            params.await_promise = Some(true);
            let sent = Instant::now();
            let result = injected::call(
                &context.session.with_timeout(budget + CALL_SLACK),
                context.context_id,
                params,
            )
            .await;
            if let Some(place) = place {
                place.release();
            }
            let result = match result {
                Ok(result) => result,
                Err(e) if is_gone(&e) => {
                    // The document went away; look in the next one.
                    tries += lost_tries(&e, sent.elapsed());
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
                if value[ARG_ERROR] == json!(true) {
                    return Err(ErrorInfo::new(ErrorKind::BadArgs, fatal.to_owned()));
                }
                return Err(bad_selector(condition, fatal));
            }
            tries += value["observed"]["tries"].as_u64().unwrap_or(0);
            last = Probed {
                done: value["done"].as_bool().unwrap_or(false),
                observed: value["observed"].clone(),
                value: value.get("value").cloned(),
            };
            if last.done || Instant::now() >= deadline {
                break;
            }
        }
        if matches!(condition, Condition::Expect { .. })
            && let Some(observed) = last.observed.as_object_mut()
        {
            observed.insert("tries".into(), json!(tries.max(1)));
        }
        Ok(if last.done {
            let mut met = met(condition, last);
            met.note = note;
            Ok(met)
        } else {
            Err(unmet(condition, &last.observed))
        })
    }

    /// Waits up to `max` for `page`'s DOM to go `quiet` without a
    /// change; `false` if it did not (or there was no document).
    pub(super) async fn dom_quiet(
        &self,
        running: &Running,
        page: &Page,
        quiet: Duration,
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
            argument(json!(u64::try_from(quiet.as_millis()).unwrap_or(300))),
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

/// About how many times a probe checked before its document went away
/// (`err`, after `ran`): it had checked at least once a frame or every
/// 100ms, unless it never started.
fn lost_tries(err: &CdpError, ran: Duration) -> u64 {
    if matches!(err, CdpError::Protocol { message, .. }
        if message.starts_with("Cannot find context with specified id"))
    {
        return 0;
    }
    1 + u64::try_from(ran.as_millis() / 100).unwrap_or(u64::MAX)
}

/// The probe's spec for `condition` (its locator's in-frame steps, with
/// `place` resolved), and `--fn`'s expression.
fn spec<'a>(condition: &'a Condition, place: Option<&Place>) -> (Value, Option<&'a str>) {
    match condition {
        Condition::Load { dom_content_loaded } => (
            json!({ "kind": "load", "domContentLoaded": dom_content_loaded }),
            None,
        ),
        Condition::Element { locator, state } => (
            json!({ "kind": "element", "steps": locator.in_frame_steps(), "state": state }),
            None,
        ),
        Condition::Text { text, within, gone } => {
            // A frame's document (`--in frame=…`) is read whole.
            let within = within.as_ref().and_then(|l| {
                let steps = l.in_frame_steps();
                (!steps.is_empty() || place.is_some_and(|p| p.root.is_some())).then_some(steps)
            });
            (
                json!({ "kind": "text", "text": text, "within": within, "gone": gone }),
                None,
            )
        }
        Condition::Function(expression) => (json!({ "kind": "fn" }), Some(expression)),
        Condition::Expect {
            locator,
            check,
            not,
            ..
        } => (
            json!({
                "kind": "expect",
                "steps": locator.as_ref().map(Locator::in_frame_steps),
                "check": check,
                "not": not,
            }),
            None,
        ),
        _ => (Value::Null, None),
    }
}

/// Where a probe runs.
enum At {
    /// In this context, from this place (`None`: the main frame's
    /// document).
    Place(FrameContext, Option<Place>),
    /// The locator's frame or ref element is not there: what was observed.
    Absent(Value),
}

/// How often a probe looks again for a frame or element that is absent.
const ABSENT_POLL: Duration = Duration::from_millis(100);

impl Engine {
    /// Where to run the probe for `condition` now.
    async fn probe_place(
        &self,
        running: &Running,
        page: &Page,
        condition: &Condition,
        options: &LocateOptions,
        world: World,
        left: Duration,
    ) -> Result<At, ErrorInfo> {
        let left = left.max(Duration::from_millis(1));
        let (locator, frame_only) = match condition {
            Condition::Element { locator, .. }
            | Condition::Expect {
                locator: Some(locator),
                ..
            } => (locator, FrameOnly::Owner),
            Condition::Text {
                within: Some(within),
                ..
            } => (within, FrameOnly::Document),
            _ => {
                let context = self.frame_context(running, page, None, world, left).await?;
                return Ok(At::Place(context, None));
            }
        };
        match self
            .place(running, page, locator, options, frame_only, left)
            .await
        {
            Ok(place) => Ok(At::Place(place.context.clone(), Some(place))),
            Err(e)
                if is_absent(&e)
                    || ((gone_is_met(condition)
                        || matches!(condition, Condition::Expect { .. }))
                        && is_gone_ref(&e)) =>
            {
                let observed = if is_gone_ref(&e) {
                    json!({ "count": 0, "ref": "gone" })
                } else {
                    json!({ "count": 0, "scope": false, "frame": "not found" })
                };
                Ok(At::Absent(observed))
            }
            Err(e) => Err(e),
        }
    }
}

/// A frame the locator names that is not (yet, or any more) in the page.
fn is_absent(err: &ErrorInfo) -> bool {
    err.kind == ErrorKind::NotFound
        && err
            .details
            .as_ref()
            .is_some_and(|d| d.get("frame").is_some())
}

/// A ref whose element is gone.
fn is_gone_ref(err: &ErrorInfo) -> bool {
    matches!(err.kind, ErrorKind::RefDetached | ErrorKind::RefStale)
}

/// Whether the condition holds once its element is gone.
fn gone_is_met(condition: &Condition) -> bool {
    matches!(
        condition,
        Condition::Element {
            state: ElementState::Detached | ElementState::Hidden,
            ..
        } | Condition::Text { gone: true, .. }
    )
}

/// Whether the condition holds while its frame or element is absent.
fn absent_is_met(condition: &Condition) -> bool {
    match condition {
        Condition::Expect { not, absent, .. } => absent.is_some_and(|pass| pass != *not),
        _ => gone_is_met(condition),
    }
}

/// A locator the injected bundle rejected (a bad selector or regex).
fn bad_selector(condition: &Condition, why: &str) -> ErrorInfo {
    let locator = match condition {
        Condition::Element { locator, .. } => Some(locator),
        Condition::Text { within, .. } => within.as_ref(),
        Condition::Expect { locator, .. } => locator.as_ref(),
        _ => None,
    };
    let first = why.lines().next().unwrap_or_default();
    match locator {
        Some(locator) => bad_locator(locator, first),
        None => ErrorInfo::new(ErrorKind::BadLocator, first.to_owned()),
    }
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
        Condition::Expect { .. } => Met {
            count: probed.observed["count"].as_u64(),
            value: Some(probed.observed),
            ..Met::default()
        },
        _ => Met::default(),
    }
}

/// The observations of a probe that timed out.
fn unmet(condition: &Condition, observed: &Value) -> Unmet {
    let unmet = Unmet {
        observed: observed.as_object().cloned().unwrap_or_default(),
        ..Unmet::default()
    };
    if matches!(condition, Condition::Expect { .. }) {
        // `expect` words its own failure from what was observed.
        return unmet;
    }
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
