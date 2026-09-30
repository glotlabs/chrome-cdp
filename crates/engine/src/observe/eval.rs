//! `eval <js>` (plan 06, milestone 6.4): runs a script in a tab's page
//! (or in a frame, or chrome-cdp's isolated world) and prints its value.
//!
//! The script is one `Runtime.evaluate` in REPL mode, as DevTools' console
//! runs what is typed into it: statements are allowed and the last one's
//! value is the result, `await` works at the top level, and `userGesture`
//! is set so the page can open popups or go fullscreen. It runs inside a
//! block, so its `let`s and `const`s do not stay behind in the page's
//! global scope. A script that is one object literal (`{a: 1}`) is run as
//! an expression. With `--in`, the element is bound as `el`.
//!
//! When the value is an object, one more call (`eval.js`) calls it if it
//! is a function (`() => …`, `el => el.value`), awaits it if it is a
//! promise, and turns it into JSON text: `returnByValue` alone would give
//! `{}` for DOM nodes and fail on cycles. Exceptions are `EvalError`s with
//! the message and the stack, its frames in page scripts resolved with
//! source maps as `errors` does; frames in the script itself are named
//! `eval` with the script's own line numbers.
//!
//! Elements of the frame's document in the value get refs from the tab's
//! registry (`button#save.primary [e14]`), the first [`MAX_REFS`] of them:
//! `eval.js` hands them back in a list, `DOM.describeNode` gives their
//! backend ids, and the helper bundle in the isolated world their role
//! and name, which the ref keeps for its role+name fallback.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cdp::CdpError;
use cdp::domains::{dom, runtime};
use futures_util::future::join_all;
use protocol::schema::{EvalData, StackFrame};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::format::{arg_text, error_message, parse_stack, stack_frames};
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::injected::{self, GUARD, own_properties};
use crate::input::origin;
use crate::locator::{LocateOptions, Locator, until_found};
use crate::request::{EvalRequest, EvalWorld, SnapshotEngine, millis};
use crate::snapshot::NodeKey;
use crate::tabs::{FrameContext, Page, tab_name};
use crate::{Engine, Running};

/// Finishes an object value: see the module docs.
const FINISH_JS: &str = include_str!("eval.js");

/// The script's `sourceURL`: what its stack frames are called.
const EVAL_URL: &str = "eval";

/// Numbers each `eval`'s object group, which holds everything it holds
/// on to: one per call, since a call's group is released after it
/// answers, when the next may have begun.
static GROUPS: AtomicU64 = AtomicU64::new(0);

/// Where the `--in` element waits for the script, which takes it into
/// `el` (and deletes it) first thing.
const ELEMENT_SLOT: &str = "Symbol.for('chrome-cdp.eval.el')";

/// How many frames of an exception's stack the text form shows.
const SHOWN_FRAMES: usize = 5;

/// How many elements of one value get refs: each costs a few calls.
pub(crate) const MAX_REFS: usize = 50;

/// Ends the mark `eval.js` puts after an element, where its ref goes.
const MARK_END: char = '⟧';

/// What each element's role and name are, for its ref (the bundle's
/// `describe`, engine B's code).
fn describe_js() -> String {
    format!(
        "function (...els) {{ {GUARD} return els.map((e) => {{ const d = globalThis.__ccdp.describe(e); return [d.role, d.name]; }}); }}"
    )
}

/// A script's value.
#[derive(Debug)]
struct ScriptValue {
    /// `string`, `object`, `undefined`, … (see [`EvalData::kind`]).
    kind: String,
    /// The value's JSON text as the page wrote it (its keys in order),
    /// with the elements' refs in it; empty for `undefined`.
    text: String,
    /// The refs given, in the order of the elements in the value.
    refs: Vec<String>,
    /// Elements past [`MAX_REFS`], which got none.
    cut: u64,
}

/// The script as it is sent: wrapped in a block, `el` bound, named.
#[derive(Debug)]
struct Wrapped {
    expression: String,
    /// How many lines come before the script's first one.
    lines_before: u32,
}

fn wrap(script: &str, bind_element: bool, as_expression: bool) -> Wrapped {
    let mut expression = String::from("{");
    if bind_element {
        let _ = write!(
            expression,
            " const el = ((k) => {{ const v = globalThis[k]; delete globalThis[k]; return v; }})({ELEMENT_SLOT});"
        );
    }
    expression.push('\n');
    let mut lines_before = 1;
    if as_expression {
        expression.push_str("(\n");
        lines_before += 1;
    }
    expression.push_str(script);
    if as_expression {
        expression.push_str("\n)");
    }
    let _ = write!(expression, "\n}}\n//# sourceURL={EVAL_URL}");
    Wrapped {
        expression,
        lines_before,
    }
}

/// Whether `script` reads as an object literal, which a statement would
/// take for a block: `{a: 1}`.
fn looks_like_object(script: &str) -> bool {
    let script = script.trim();
    script.starts_with('{') && script.ends_with('}')
}

impl Engine {
    /// `eval`: runs `req.script` in the active tab (or `--tab`) and
    /// returns its value.
    pub async fn eval(&self, req: EvalRequest) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let locator = check_eval(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let deadline = Instant::now() + timeout;
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let number = GROUPS.fetch_add(1, Ordering::Relaxed);
        let run = Run {
            engine: self,
            running: &running,
            page: &page,
            req: &req,
            deadline,
            group: format!("chrome-cdp-eval-{number}"),
            mark: ref_mark(number),
            // Elements seen from now on get refs only while this holds.
            generation: running
                .tabs
                .snapshot_memory(page.id, |m| m.refs.generation()),
        };
        let (value, note) = self
            .unless_blocked(
                &running,
                &page,
                mark,
                Box::pin(run.run(locator.as_ref(), &options)),
            )
            .await?;
        let output = eval_output(&page, &req, locator.as_ref(), value)?;
        let output = match note {
            Some(note) => output.warning(note),
            None => output,
        };
        Ok(self.report_events(&running, mark, output, None).await)
    }
}

/// Checks `eval`'s arguments without a browser; returns its `--in`
/// locator.
pub fn check_eval(req: &EvalRequest) -> Result<Option<Locator>, ErrorInfo> {
    if req.script.trim().is_empty() {
        return Err(
            ErrorInfo::new(ErrorKind::BadArgs, "eval needs a script").hint(
                Hint::run(Cmd::new("eval").arg("document.title"))
                    .or(format!("{} reads it from stdin", Cmd::new("eval").arg("-"))),
            ),
        );
    }
    if req.locator.is_some() && req.frame.is_some() {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "--in and --frame cannot be used together: the element's frame is where the script runs",
        )
        .hint(Hint::text(
            "drop --frame, or name the frame in the locator: --in 'frame=checkout >> #pay'",
        )));
    }
    let options = LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    };
    req.locator.as_deref().map(|l| options.parse(l)).transpose()
}

/// One `eval` in progress.
struct Run<'a> {
    engine: &'a Engine,
    running: &'a Running,
    page: &'a Page,
    req: &'a EvalRequest,
    deadline: Instant,
    /// The object group of this call.
    group: String,
    /// What `eval.js` marks an element's place for its ref with.
    mark: String,
    /// The tab's document generation when the call began.
    generation: Option<u64>,
}

impl Run<'_> {
    /// What is left of the time.
    fn left(&self) -> Duration {
        self.deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    }

    fn world(&self) -> World {
        match self.req.world {
            EvalWorld::Main => World::Main,
            EvalWorld::Isolated => World::Isolated,
        }
    }

    /// Runs the script; returns its value and the ref re-resolve note.
    async fn run(
        &self,
        locator: Option<&Locator>,
        options: &LocateOptions,
    ) -> Result<(ScriptValue, Option<String>), ErrorInfo> {
        let (frame, context, element, note) = if let Some(locator) = locator {
            let (frame, context, element, note) = self.element(locator, options).await?;
            (frame, context, Some(element), note)
        } else {
            let (frame, context) = self
                .engine
                .frame_and_context(
                    self.running,
                    self.page,
                    self.req.frame.as_deref(),
                    self.world(),
                    self.left(),
                )
                .await?;
            (frame, context, None, None)
        };
        let result = self.evaluate(&frame, &context, element.as_deref()).await;
        let session = context.session.clone();
        let group = self.group.clone();
        tokio::spawn(async move {
            let _ = session
                .call(runtime::ReleaseObjectGroupParams::new(group))
                .await;
        });
        Ok((result?, note))
    }

    /// The `--in` element's frame and its context (in the chosen world),
    /// and the element's handle there.
    async fn element(
        &self,
        locator: &Locator,
        options: &LocateOptions,
    ) -> Result<(String, FrameContext, String, Option<String>), ErrorInfo> {
        let retry = |r: &str| Cmd::new("eval").flag("in", r).raw("…");
        let retry = &retry;
        // It waits for the element, as the read commands do.
        let timeout = millis(self.req.timeout_ms);
        let target = until_found(self.deadline, timeout, || async move {
            self.engine
                .locate_one(
                    self.running,
                    self.page,
                    locator,
                    options,
                    Some(retry),
                    timeout,
                )
                .await
        })
        .await?;
        let (frame, backend, note) = (
            target.key.frame.clone(),
            target.key.backend,
            target.note.clone(),
        );
        target.release();
        let context = self
            .engine
            .frame_context(
                self.running,
                self.page,
                Some(&frame),
                self.world(),
                self.left(),
            )
            .await?;
        let object = context
            .session
            .with_timeout(self.left())
            .call(dom::ResolveNodeParams {
                node_id: None,
                backend_node_id: Some(backend),
                object_group: Some(self.group.clone()),
                execution_context_id: Some(context.context_id),
            })
            .await
            .map_err(|e| self.cdp_error(&e))?
            .object
            .object_id
            .ok_or_else(|| self.engine.not_found(self.page, locator))?;
        Ok((frame, context, object, note))
    }

    /// Evaluates the script (with `element` as `el`) in `context`, of
    /// frame `frame`, then finishes an object value.
    async fn evaluate(
        &self,
        frame: &str,
        context: &FrameContext,
        element: Option<&str>,
    ) -> Result<ScriptValue, ErrorInfo> {
        let session = context.session.with_timeout(self.left());
        if let Some(element) = element {
            let stash = format!("function () {{ globalThis[{ELEMENT_SLOT}] = this; }}");
            self.call(
                &session,
                "Runtime.callFunctionOn",
                json!({ "functionDeclaration": stash, "objectId": element }),
            )
            .await?;
        }
        let script = &self.req.script;
        let mut attempts = vec![wrap(script, element.is_some(), false)];
        if looks_like_object(script) {
            attempts.insert(0, wrap(script, element.is_some(), true));
        }
        let count = attempts.len();
        for (i, wrapped) in attempts.into_iter().enumerate() {
            let result = self
                .call(
                    &session.with_timeout(self.left()),
                    "Runtime.evaluate",
                    json!({
                        "expression": wrapped.expression,
                        "contextId": context.context_id,
                        "objectGroup": self.group,
                        "replMode": true,
                        "awaitPromise": true,
                        "userGesture": true,
                    }),
                )
                .await?;
            if let Some(details) = result.get("exceptionDetails") {
                // An object literal that is not one: run it as statements.
                if i + 1 < count && is_compile_error(details) {
                    continue;
                }
                if element.is_some() && is_compile_error(details) {
                    // Nothing ran, so nothing took `el` from its slot.
                    let delete = format!("delete globalThis[{ELEMENT_SLOT}]");
                    let _ = session
                        .call_raw(
                            "Runtime.evaluate",
                            json!({ "expression": delete, "contextId": context.context_id }),
                        )
                        .await;
                }
                return Err(self.thrown(details, wrapped.lines_before).await);
            }
            return self
                .finish(
                    frame,
                    context,
                    &result["result"],
                    element,
                    wrapped.lines_before,
                )
                .await;
        }
        unreachable!("the last attempt returns")
    }

    /// The value of `object`, the script's result.
    async fn finish(
        &self,
        frame: &str,
        context: &FrameContext,
        object: &Value,
        element: Option<&str>,
        lines_before: u32,
    ) -> Result<ScriptValue, ErrorInfo> {
        let Some(object_id) = object["objectId"].as_str() else {
            return Ok(primitive(object));
        };
        let el = element.map_or_else(|| json!({}), |id| json!({ "objectId": id }));
        let params = json!({
            "functionDeclaration": FINISH_JS,
            "objectId": object_id,
            "arguments": [el, { "value": self.mark }, { "value": MAX_REFS }],
            "objectGroup": self.group,
            "awaitPromise": true,
            "userGesture": true,
        });
        let session = context.session.with_timeout(self.left());
        let result = self
            .call(&session, "Runtime.callFunctionOn", params)
            .await?;
        if let Some(details) = result.get("exceptionDetails") {
            return Err(self.thrown(details, lines_before).await);
        }
        let answer = result["result"]["objectId"].as_str().ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                "the script's value came back empty",
            )
        })?;
        let properties = own_properties(&session, answer)
            .await
            .map_err(|e| self.cdp_error(&e))?;
        let field = |name: &str| {
            properties
                .iter()
                .find(|p| p.name == name)
                .and_then(|p| p.value.as_ref())
        };
        let read = |name: &str| field(name).and_then(|v| v.value.clone());
        let numbers = match field("els").and_then(|v| v.object_id.as_deref()) {
            Some(els) => self.refs(frame, &session, els).await,
            None => Vec::new(),
        };
        let text = read("text");
        let (text, refs) = put_refs(
            text.as_ref().and_then(Value::as_str).unwrap_or_default(),
            &self.mark,
            &numbers,
        );
        Ok(ScriptValue {
            kind: read("type")
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or("object")
                .to_owned(),
            text,
            refs,
            cut: read("over").as_ref().and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// The refs of the elements in the list `els` (by index, `None` for
    /// one that got none), given now to those without one. A ref needs
    /// the element's role and name, which the helper bundle in the
    /// frame's isolated world works out. Any failure means no refs: the
    /// value is worth more than they are.
    async fn refs(&self, frame: &str, session: &cdp::Session, els: &str) -> Vec<Option<u32>> {
        let Ok(listed) = own_properties(session, els).await else {
            return Vec::new();
        };
        let handles: Vec<(usize, String)> = listed
            .into_iter()
            .filter_map(|p| {
                let index = p.name.parse::<usize>().ok().filter(|&i| i < MAX_REFS)?;
                Some((index, p.value?.object_id?))
            })
            .collect();
        if handles.is_empty() {
            return Vec::new();
        }
        let described = join_all(handles.iter().map(|(_, id)| {
            session.call(dom::DescribeNodeParams {
                object_id: Some(id.clone()),
                ..dom::DescribeNodeParams::default()
            })
        }))
        .await;
        let backends: Vec<(usize, i64)> = handles
            .iter()
            .zip(described)
            .filter_map(|((index, _), d)| Some((*index, d.ok()?.node.backend_node_id)))
            .collect();
        let Ok(isolated) = self
            .engine
            .frame_context(
                self.running,
                self.page,
                Some(frame),
                World::Isolated,
                self.left(),
            )
            .await
        else {
            return Vec::new();
        };
        let isolated_session = isolated.session.with_timeout(self.left());
        let resolved = join_all(backends.iter().map(|(_, backend)| {
            isolated_session.call(dom::ResolveNodeParams {
                node_id: None,
                backend_node_id: Some(*backend),
                object_group: Some(self.group.clone()),
                execution_context_id: Some(isolated.context_id),
            })
        }))
        .await;
        let found: Vec<(usize, i64, String)> = backends
            .iter()
            .zip(resolved)
            .filter_map(|((index, backend), r)| Some((*index, *backend, r.ok()?.object.object_id?)))
            .collect();
        let mut describe = runtime::CallFunctionOnParams::new(describe_js());
        describe.arguments = Some(
            found
                .iter()
                .map(|(_, _, id)| runtime::CallArgument {
                    value: None,
                    unserializable_value: None,
                    object_id: Some(id.clone()),
                })
                .collect(),
        );
        describe.return_by_value = Some(true);
        let names: Vec<(String, String)> =
            match injected::call(&isolated_session, isolated.context_id, describe).await {
                Ok(r) if r.exception_details.is_none() => r
                    .result
                    .value
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
        if names.len() != found.len() {
            return Vec::new();
        }
        self.running
            .tabs
            .snapshot_memory(self.page.id, |m| {
                if Some(m.refs.generation()) != self.generation {
                    return Vec::new();
                }
                let mut numbers = vec![None; MAX_REFS];
                for ((index, backend, _), (role, name)) in found.iter().zip(&names) {
                    let key = NodeKey {
                        frame: frame.to_owned(),
                        backend: *backend,
                    };
                    numbers[*index] =
                        Some(m.refs.assign_from(&key, role, name, SnapshotEngine::Dom));
                }
                numbers
            })
            .unwrap_or_default()
    }

    async fn call(
        &self,
        session: &cdp::Session,
        method: &str,
        params: Value,
    ) -> Result<Value, ErrorInfo> {
        session
            .call_raw(method, params)
            .await
            .map_err(|e| self.cdp_error(&e))
    }

    fn cdp_error(&self, e: &CdpError) -> ErrorInfo {
        let session = &self.engine.session;
        match e {
            CdpError::Timeout { .. } => {
                let timeout = millis(self.req.timeout_ms);
                ErrorInfo::new(
                    ErrorKind::Timeout,
                    format!(
                        "the script did not finish within {}",
                        format::duration(timeout)
                    ),
                )
                .note("observed", "a promise it awaits has not settled")
                .hint(
                    Hint::run(
                        Cmd::new("eval")
                            .flag("timeout", format::duration(timeout * 2))
                            .raw("…")
                            .scoped(session),
                    )
                    .why("waits longer"),
                )
            }
            CdpError::Protocol { message, .. }
                if message.contains("context was destroyed")
                    || message.contains("navigated or closed")
                    || message.contains("Cannot find context") =>
            {
                ErrorInfo::new(
                    ErrorKind::EvalError,
                    "the page navigated away while the script ran, so it has no value",
                )
                .hint(
                    Hint::run(Cmd::new("wait").switch("load").scoped(session))
                        .why("waits for the new page; then run eval again"),
                )
            }
            _ => from_cdp(e, session),
        }
    }

    /// The `EvalError` of an exception the script threw (or a promise it
    /// awaited rejected with). `lines_before` is how many lines the
    /// wrapper put before the script's first.
    async fn thrown(&self, details: &Value, lines_before: u32) -> ErrorInfo {
        let exception = &details["exception"];
        let description = exception["description"].as_str().unwrap_or_default();
        let message = if exception.is_null() {
            details["text"]
                .as_str()
                .unwrap_or("an exception")
                .to_owned()
        } else if exception["subtype"] == "error" {
            error_message(description)
        } else {
            arg_text(exception)
        };
        let compile = is_compile_error(details);
        let mut stack = match details.get("stackTrace") {
            Some(trace) => stack_frames(trace),
            None => parse_stack(description),
        };
        for frame in &mut stack {
            if frame.url == EVAL_URL {
                frame.line = frame.line.saturating_sub(lines_before);
            }
        }
        if compile {
            // A syntax error has no stack; its place is in the script.
            let line = details["lineNumber"].as_u64().unwrap_or_default();
            let column = details["columnNumber"].as_u64().unwrap_or_default();
            stack.push(StackFrame {
                function: String::new(),
                url: EVAL_URL.to_owned(),
                line: u32::try_from(line + 1)
                    .unwrap_or(u32::MAX)
                    .saturating_sub(lines_before),
                column: u32::try_from(column + 1).unwrap_or(u32::MAX),
                original: None,
                file: None,
            });
        }
        let mut frames: Vec<&mut StackFrame> = stack.iter_mut().collect();
        let timeout = self.left();
        self.engine
            .map_stack(
                self.running,
                self.page,
                &mut frames,
                self.req.sourcemaps,
                timeout,
            )
            .await;
        let page_origin = self
            .running
            .tabs
            .frames(self.page.id)
            .first()
            .and_then(|f| origin(&f.url).map(str::to_owned));
        let first_line = message.lines().next().unwrap_or_default();
        let text = if compile {
            format!("the script does not parse: {first_line}")
        } else {
            format!("the script threw {first_line}")
        };
        let mut info = ErrorInfo::new(ErrorKind::EvalError, text).details(json!({
            "exception": exception["className"],
            "message": message,
            "stack": stack,
        }));
        for frame in stack.iter().take(SHOWN_FRAMES) {
            info = info.note(
                "at",
                super::commands::frame_line(frame, page_origin.as_deref()),
            );
        }
        self.teach(info, &message, compile)
    }

    /// The hint for an exception, when the message says what went wrong.
    fn teach(&self, info: ErrorInfo, message: &str, compile: bool) -> ErrorInfo {
        let session = &self.engine.session;
        if message.contains("Illegal return statement") {
            return info.hint(Hint::text(
                "a script is not a function body: drop `return` (the last expression is the \
                 value), or wrap it: (() => { … })()",
            ));
        }
        if message.contains("el is not defined") && self.req.locator.is_none() {
            return info.hint(
                Hint::run(Cmd::new("eval").flag("in", "e12").raw("…").scoped(session))
                    .why("binds el to an element: a ref such as e12, or any locator"),
            );
        }
        if compile {
            return info.hint(
                Hint::run(Cmd::new("eval").arg("-").scoped(session))
                    .why("reads the script from stdin, with no shell quoting to get wrong"),
            );
        }
        info
    }
}

/// Whether an exception is a syntax error of the script itself (no stack:
/// nothing ran).
fn is_compile_error(details: &Value) -> bool {
    details.get("stackTrace").is_none() && details["exception"]["className"] == "SyntaxError"
}

/// A value `Runtime.evaluate` returned with no handle: a primitive.
fn primitive(object: &Value) -> ScriptValue {
    let kind = match (object["type"].as_str(), object["subtype"].as_str()) {
        (Some("object"), Some("null")) => "null",
        (Some(kind), _) => kind,
        (None, _) => "undefined",
    };
    let value = match object.get("unserializableValue").and_then(Value::as_str) {
        // `NaN`, `-0`, `Infinity`, `12n`.
        Some("-0") => json!(0),
        Some(text) => Value::String(text.to_owned()),
        None => object.get("value").cloned().unwrap_or(Value::Null),
    };
    ScriptValue {
        kind: kind.to_owned(),
        text: if kind == "undefined" {
            String::new()
        } else {
            serde_json::to_string_pretty(&value).unwrap_or_default()
        },
        refs: Vec::new(),
        cut: 0,
    }
}

/// The mark of call `number`: odd enough that no page text has it.
fn ref_mark(number: u64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    format!(" ⟦ccdp{number:x}.{nanos:x}:")
}

/// `text` with each element's mark (`mark`, its index in `numbers`,
/// [`MARK_END`]) turned into its ref, ` [e14]`, or taken out if it got
/// none; and the refs, in the order they first appear.
fn put_refs(text: &str, mark: &str, numbers: &[Option<u32>]) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut refs: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(mark) {
        out.push_str(&rest[..at]);
        let after = &rest[at + mark.len()..];
        let Some(end) = after.find(MARK_END) else {
            rest = &rest[at..];
            break;
        };
        let number = after[..end]
            .parse::<usize>()
            .ok()
            .and_then(|i| numbers.get(i).copied().flatten());
        if let Some(number) = number {
            let reference = format!("e{number}");
            let _ = write!(out, " [{reference}]");
            if !refs.contains(&reference) {
                refs.push(reference);
            }
        }
        rest = &after[end + MARK_END.len_utf8()..];
    }
    out.push_str(rest);
    (out, refs)
}

/// JSON text without the whitespace between its tokens.
fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    for c in text.chars() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    out
}

fn eval_output(
    page: &Page,
    req: &EvalRequest,
    locator: Option<&Locator>,
    value: ScriptValue,
) -> Result<Output, ErrorInfo> {
    let parsed: Value = if value.text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&value.text).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("the script's value is not JSON: {e}"),
            )
        })?
    };
    let (text, quiet) = match (&parsed, value.kind.as_str()) {
        // A string, or what stands for a value JSON has no word for
        // (`NaN`, `12n`, `button#save`): as it is.
        (Value::String(s), _) => (s.clone(), s.clone()),
        (_, "undefined") => ("undefined".to_owned(), String::new()),
        _ => (value.text.clone(), compact(&value.text)),
    };
    let data = EvalData {
        tab: tab_name(page.id),
        locator: locator.map(ToString::to_string),
        frame: req.frame.clone(),
        world: req.world.as_str().to_owned(),
        kind: value.kind,
        value: parsed,
        refs: value.refs,
        refs_cut: value.cut,
    };
    let output = Output::typed(&data, text).quiet(quiet).limit(
        req.max_chars,
        "--max-chars 0 for all of it, or return less (one property, .slice(0, 20))",
    );
    Ok(if value.cut > 0 {
        output.warning(format!(
            "{} more elements have no ref: eval gives refs to the first {MAX_REFS} only; \
             return the rest to get theirs (.slice({MAX_REFS}))",
            format::thousands(usize::try_from(value.cut).unwrap_or(usize::MAX)),
        ))
    } else {
        output
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{compact, looks_like_object, primitive, put_refs, ref_mark, wrap};

    #[test]
    fn scripts_run_in_a_block_named_eval() {
        let plain = wrap("1 + 1", false, false);
        assert_eq!(plain.expression, "{\n1 + 1\n}\n//# sourceURL=eval");
        assert_eq!(plain.lines_before, 1);
        let object = wrap("{a: 1}", false, true);
        assert_eq!(object.expression, "{\n(\n{a: 1}\n)\n}\n//# sourceURL=eval");
        assert_eq!(object.lines_before, 2);
        let bound = wrap("el.id", true, false);
        assert!(bound.expression.starts_with("{ const el = "));
        assert!(bound.expression.contains("\nel.id\n}"));
        assert_eq!(bound.lines_before, 1);
    }

    #[test]
    fn object_literals_are_told_apart() {
        assert!(looks_like_object(" {a: 1} "));
        assert!(!looks_like_object("a = {}; a"));
    }

    #[test]
    fn primitives_keep_their_type() {
        let text = |v| primitive(&v).text;
        assert_eq!(text(json!({"type": "number", "value": 42})), "42");
        assert_eq!(
            text(json!({"type": "number", "unserializableValue": "NaN"})),
            "\"NaN\""
        );
        assert_eq!(
            text(json!({"type": "number", "unserializableValue": "-0"})),
            "0"
        );
        assert_eq!(primitive(&json!({"type": "undefined"})).text, "");
        let null = primitive(&json!({"type": "object", "subtype": "null", "value": null}));
        assert_eq!((null.kind.as_str(), null.text.as_str()), ("null", "null"));
    }

    #[test]
    fn marks_become_refs_or_go() {
        let mark = ref_mark(7);
        let text =
            format!("[\"a{mark}0⟧\", \"b{mark}1⟧\", \"a{mark}0⟧\", \"c{mark}2⟧\", \"{mark}\"]");
        let (text, refs) = put_refs(&text, &mark, &[Some(14), None, Some(3)]);
        assert_eq!(
            text,
            format!("[\"a [e14]\", \"b\", \"a [e14]\", \"c [e3]\", \"{mark}\"]")
        );
        assert_eq!(refs, ["e14", "e3"]);
        // No refs at all (a failure, or a new document): marks go.
        let (text, refs) = put_refs(&format!("x{mark}0⟧"), &mark, &[]);
        assert_eq!((text.as_str(), refs.len()), ("x", 0));
    }

    #[test]
    fn compact_keeps_strings() {
        assert_eq!(
            compact("{\n  \"a b\": [\n    1,\n    \"x \\\" y\"\n  ]\n}"),
            "{\"a b\":[1,\"x \\\" y\"]}"
        );
    }
}
