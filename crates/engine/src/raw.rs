//! `cdp <Domain.method> [json]`: one raw CDP call on the session's
//! browser or current page, the escape hatch (overview principle 6).
//! `--listen` also captures the events of the call's session that follow
//! it (or, with no method, just listens); with `--all-sessions`, those of
//! every session of the tab, its OOPIFs' and workers' too, each named
//! after what sent it (see [`crate::tabs::tap`]).
//!
//! A page call returns at once when the page opens a dialog or crashes
//! meanwhile (see [`crate::interrupt`]). One sent to a tab that already
//! has a dialog open (or crashed) goes out as is, since it may be what
//! answers it (`Page.handleJavaScriptDialog`, `Page.reload`); if the tab
//! is still blocked afterwards, the output says so. Events are listened
//! for after the call without the tab's lock, so other commands on the
//! tab run meanwhile (and can be what makes them fire).

use cdp::{CdpError, LosslessEventStream, Session};
use protocol::schema::{CdpEvent, CdpListenData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format, suggest};
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use crate::Engine;
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::request::{CdpRequest, Target, millis};
use crate::tabs::FrameContext;
use crate::tabs::tap::Tap;

/// Checks a `cdp` request without a browser: it needs a method or
/// events to listen for, and event names must be `Domain.event` or
/// `Domain.*`.
pub fn check_cdp(req: &CdpRequest) -> Result<(), ErrorInfo> {
    if req.method.is_none() && req.listen.is_empty() {
        return Err(
            ErrorInfo::new(ErrorKind::BadArgs, "cdp needs a method, or --listen")
                .hint(Hint::run(Cmd::new("cdp").arg("Browser.getVersion"))),
        );
    }
    if req.all_sessions && req.listen.is_empty() {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "--all-sessions needs --listen: it says where to listen",
        )
        .hint(Hint::run(
            Cmd::new("cdp")
                .switch("all-sessions")
                .flag("listen", "Network.requestWillBeSent")
                .arg("Page.reload"),
        )));
    }
    for name in &req.listen {
        let valid = name.split_once('.').is_some_and(|(domain, event)| {
            !domain.is_empty()
                && domain.chars().all(|c| c.is_ascii_alphanumeric())
                && (event == "*"
                    || (!event.is_empty() && event.chars().all(|c| c.is_ascii_alphanumeric())))
        });
        if !valid {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{name}` is not an event name; use Domain.event or Domain.*"),
            )
            .hint(Hint::run(
                Cmd::new("cdp")
                    .flag("target", "page")
                    .flag("listen", "Page.loadEventFired")
                    .arg("Page.reload"),
            )));
        }
    }
    Ok(())
}

/// Whether event `method` is one of `listen`'s.
fn wanted(listen: &[String], method: &str) -> bool {
    listen.iter().any(|name| match name.strip_suffix(".*") {
        Some(domain) => method
            .strip_prefix(domain)
            .is_some_and(|rest| rest.starts_with('.')),
        None => name == method,
    })
}

/// Makes the call (if any) on `session`, subscribed to its events first
/// when `listen` is set, so none that the call causes is missed, however
/// many it causes before the call returns.
async fn call_listening(
    session: &Session,
    method: Option<&str>,
    params: Value,
    listen: bool,
) -> cdp::Result<(Option<Value>, Option<LosslessEventStream>)> {
    let events = listen.then(|| session.events_lossless(""));
    let result = match method {
        Some(method) => Some(session.call_raw(method, params).await?),
        None => None,
    };
    Ok((result, events))
}

/// Where `--listen` hears events: the call's session, or every session
/// of the tab (`--all-sessions`).
enum Listening {
    Own(LosslessEventStream),
    Tap(Tap),
}

/// The most events one call captures (with `--count 0`).
const MAX_EVENTS: usize = 10_000;

/// The events of `stream` that `listen` names, until `count` of them came
/// (0: until the deadline, at most [`MAX_EVENTS`]), the deadline passed,
/// or the session went away.
async fn collect(
    mut stream: LosslessEventStream,
    listen: &[String],
    count: usize,
    deadline: Instant,
) -> Vec<CdpEvent> {
    let want = if count == 0 { MAX_EVENTS } else { count };
    let mut events = Vec::new();
    while events.len() < want {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(event)) if wanted(listen, &event.method) => events.push(CdpEvent {
                method: event.method.clone(),
                params: event.params.clone(),
                from: None,
                session: None,
            }),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    events
}

/// [`collect`] from every session of a tab (`--all-sessions`).
async fn collect_tapped(
    mut tap: Tap,
    listen: &[String],
    count: usize,
    deadline: Instant,
) -> Vec<CdpEvent> {
    let want = if count == 0 { MAX_EVENTS } else { count };
    let mut events = Vec::new();
    while events.len() < want {
        match tokio::time::timeout_at(deadline, tap.next()).await {
            Ok(Some(tapped)) if wanted(listen, &tapped.event.method) => events.push(CdpEvent {
                method: tapped.event.method.clone(),
                params: tapped.event.params.clone(),
                from: Some(tapped.from),
                session: Some(tapped.session),
            }),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    events
}

impl Engine {
    /// Makes the call and returns Chrome's result object. A page call holds
    /// the page's lock. With `--listen`, then waits for the events.
    pub async fn cdp(&self, req: CdpRequest) -> Result<Output, ErrorInfo> {
        check_cdp(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let deadline = Instant::now() + timeout;
        let has_params = !req.params.is_empty();
        let CdpRequest {
            method,
            params,
            target,
            tab,
            frame,
            listen,
            count,
            all_sessions,
            max_chars,
            ..
        } = req;
        if frame.is_some() && target == Target::Browser {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "--frame needs a page call, not a browser one",
            )
            .hint(Hint::run(
                Cmd::new("cdp")
                    .scoped(&self.session)
                    .flag("target", "page")
                    .raw("--frame … Runtime.evaluate …"),
            )));
        }
        if all_sessions && target == Target::Browser {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "--all-sessions listens on a tab's sessions, not the browser's",
            )
            .hint(Hint::run(
                Cmd::new("cdp")
                    .scoped(&self.session)
                    .flag("target", "page")
                    .switch("all-sessions")
                    .flag("listen", "Network.requestWillBeSent")
                    .arg("Page.reload"),
            )));
        }
        let mark = running.tabs.mark();
        let method = method.as_deref();
        let params = Value::Object(params);
        let listening = !listen.is_empty();
        // With --all-sessions the tab's tap hears the call's session too.
        let own = listening && !all_sessions;
        let mut blocked = None;
        let mut page = None;
        let mut tap = None;
        let result = match target {
            Target::Browser => {
                let session = running.conn.browser().with_timeout(timeout);
                call_listening(&session, method, params, own).await
            }
            Target::Page => {
                let tab_page = self.page(&running, tab.as_deref(), timeout).await?;
                let _tab = self.tabs.lock(&tab_page.target_id).await;
                // Before the call, so none of the events it causes is
                // missed.
                tap = (listening && all_sessions).then(|| running.tabs.tap(tab_page.id));
                let call = async {
                    let Some(frame) = frame.as_deref() else {
                        let session = tab_page.session.with_timeout(timeout);
                        return Ok(call_listening(&session, method, params, own).await);
                    };
                    let call = |context: &FrameContext| {
                        let params = match method {
                            Some(m) => in_context(m, params.clone(), context.context_id),
                            None => Value::Null,
                        };
                        let session = context.session.with_timeout(timeout);
                        async move { call_listening(&session, method, params, own).await }
                    };
                    self.in_frame(&running, &tab_page, Some(frame), World::Main, timeout, call)
                        .await
                };
                let result = self.unless_blocked(&running, &tab_page, mark, call).await?;
                // Still blocked after the call (it did not answer the
                // dialog or reload the page): say so.
                blocked = self.blocked_warning(&running, &tab_page);
                page = Some(tab_page);
                result
            }
        };
        let (value, events) = result.map_err(|e| {
            let info = self.cdp_error(&e, method.unwrap_or_default(), target, has_params);
            match &blocked {
                Some(blocked) => info.note("page", blocked.clone()),
                None => info,
            }
        })?;
        let events = match tap {
            Some(tap) => Some(Listening::Tap(tap)),
            None => events.map(Listening::Own),
        };
        let mut output = match events {
            None => {
                let value = value.unwrap_or_else(|| json!({}));
                let pretty =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
                let compact = value.to_string();
                Output::new(value, pretty)
                    .quiet(compact)
                    .limit(max_chars, "--max-chars 0 for all of it")
            }
            Some(listening) => {
                let events = match listening {
                    Listening::Own(stream) => collect(stream, &listen, count, deadline).await,
                    Listening::Tap(tap) => collect_tapped(tap, &listen, count, deadline).await,
                };
                // Blocked by now, maybe while listening.
                if let Some(page) = &page {
                    blocked = self.blocked_warning(&running, page);
                }
                self.listened(value, events, &listen, count, timeout, max_chars)?
            }
        };
        if let Some(blocked) = blocked {
            output = output.warning(blocked);
        }
        Ok(self.report_events(&running, mark, output, None).await)
    }

    /// The output of a call with `--listen`: the result, then one line per
    /// event (`Page.loadEventFired {"timestamp":…}`). No event at all when
    /// `--count` asked for some is a timeout.
    fn listened(
        &self,
        value: Option<Value>,
        events: Vec<CdpEvent>,
        listen: &[String],
        count: usize,
        timeout: std::time::Duration,
        max_chars: usize,
    ) -> Result<Output, ErrorInfo> {
        let names = listen.join(", ");
        let waited = format::duration(timeout);
        if count > 0 && events.is_empty() {
            let mut info = ErrorInfo::new(
                ErrorKind::Timeout,
                format!("no {names} event within {waited}"),
            )
            .hint(
                Hint::run(
                    Cmd::new("cdp")
                        .scoped(&self.session)
                        .flag("target", "page")
                        .flag("listen", "Animation.*")
                        .arg("Animation.enable"),
                )
                .why("a domain that is off sends no events; its enable call turns them on")
                .or(format!(
                    "a longer --timeout, e.g. --timeout {}",
                    format::duration(timeout * 2)
                )),
            );
            if let Some(value) = &value {
                info = info.details(json!({ "result": value }));
            }
            return Err(info);
        }
        let mut text = String::new();
        let mut quiet = String::new();
        if let Some(value) = &value {
            text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
            text.push('\n');
            quiet = value.to_string();
            quiet.push('\n');
        }
        for event in &events {
            let line = match &event.from {
                Some(from) => format!("[{from}] {} {}\n", event.method, event.params),
                None => format!("{} {}\n", event.method, event.params),
            };
            text.push_str(&line);
            quiet.push_str(&line);
        }
        let shown = events.len();
        let result = value.and_then(|v| match v {
            Value::Object(map) => Some(map),
            _ => None::<Map<String, Value>>,
        });
        let mut output = Output::typed(&CdpListenData { result, events }, text.trim_end())
            .quiet(quiet.trim_end())
            .limit(
                max_chars,
                "--max-chars 0 for all of it, or --count N for fewer events",
            );
        if count == 0 && shown == 0 {
            output = output.warning(format!("no {names} event within {waited}"));
        } else if count > 0 && shown < count {
            output = output.warning(format!(
                "only {shown} of {count} {names} events within {waited}; a longer --timeout \
                 waits for more"
            ));
        }
        Ok(output)
    }

    fn cdp_error(&self, e: &CdpError, method: &str, target: Target, has_params: bool) -> ErrorInfo {
        let info = from_cdp(e, &self.session);
        let browser_domain = |method: &str| {
            ["Browser.", "Target."]
                .iter()
                .any(|d| method.starts_with(d))
        };
        if let CdpError::Protocol { code: -32601, .. } = e
            && let Some(info) = self.did_you_mean(&info, method, target, has_params)
        {
            return info;
        }
        let browser_domain = browser_domain(method);
        match e {
            CdpError::Protocol { code: -32601, .. }
                if target == Target::Browser && !browser_domain =>
            {
                info.hint(format!(
                    "if {m} is a page method, send it to a page: {cmd}",
                    m = method,
                    cmd = Cmd::new("cdp")
                        .scoped(&self.session)
                        .flag("target", "page")
                        .arg(method)
                        .raw("…")
                ))
            }
            _ => info,
        }
    }
}

impl Engine {
    /// For a method Chrome did not find: the one of the protocol it most
    /// likely meant, as a note and a hint that sends it (never sent by
    /// itself). `None` for a method the protocol has (a page method sent
    /// to the browser) or nothing close.
    fn did_you_mean(
        &self,
        info: &ErrorInfo,
        method: &str,
        target: Target,
        has_params: bool,
    ) -> Option<ErrorInfo> {
        let methods = cdp::domains::names::METHODS;
        if methods.contains(&method) {
            return None;
        }
        let meant = *suggest::similar(method, methods.iter().copied(), 1).first()?;
        let mut cmd = Cmd::new("cdp").scoped(&self.session);
        let browser = ["Browser.", "Target."].iter().any(|d| meant.starts_with(d));
        if target == Target::Page || !browser {
            cmd = cmd.flag("target", "page");
        }
        cmd = cmd.arg(meant);
        if has_params {
            cmd = cmd.raw("…");
        }
        Some(
            info.clone()
                .note("did you mean", meant)
                .hint(Hint::run(cmd)),
        )
    }
}

/// Adds the frame's main-world context to a `Runtime` call that runs
/// script and names no context or object of its own.
fn in_context(method: &str, mut params: Value, context_id: i64) -> Value {
    let key = match method {
        "Runtime.evaluate" => "contextId",
        "Runtime.callFunctionOn" | "Runtime.compileScript" | "Runtime.globalLexicalScopeNames" => {
            "executionContextId"
        }
        _ => return params,
    };
    let own = [
        "contextId",
        "executionContextId",
        "uniqueContextId",
        "objectId",
    ];
    if let Value::Object(map) = &mut params
        && !own.iter().any(|k| map.contains_key(*k))
    {
        map.insert(key.to_owned(), context_id.into());
    }
    params
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::in_context;

    #[test]
    fn frame_calls_get_its_context_unless_they_name_one() {
        let eval = in_context("Runtime.evaluate", json!({ "expression": "1" }), 7);
        assert_eq!(eval, json!({ "expression": "1", "contextId": 7 }));
        let call = json!({ "functionDeclaration": "f" });
        let call = in_context("Runtime.callFunctionOn", call, 7);
        assert_eq!(call["executionContextId"], json!(7));
        let own = json!({ "expression": "1", "uniqueContextId": "u" });
        assert_eq!(in_context("Runtime.evaluate", own.clone(), 7), own);
        let bound = json!({ "functionDeclaration": "f", "objectId": "o" });
        assert_eq!(
            in_context("Runtime.callFunctionOn", bound.clone(), 7),
            bound
        );
        assert_eq!(in_context("Page.reload", json!({}), 7), json!({}));
    }
}
