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

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cdp::CdpError;
use cdp::domains::{dom, runtime};
use protocol::schema::{EvalData, StackFrame};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::format::{arg_text, error_message, parse_stack, stack_frames};
use crate::errors::from_cdp;
use crate::frames::World;
use crate::input::origin;
use crate::locator::{LocateOptions, Locator};
use crate::request::{EvalRequest, EvalWorld, millis};
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

/// A script's value.
#[derive(Debug)]
struct ScriptValue {
    /// `string`, `object`, `undefined`, … (see [`EvalData::kind`]).
    kind: String,
    /// The value's JSON text as the page wrote it (its keys in order);
    /// empty for `undefined`.
    text: String,
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
        let run = Run {
            engine: self,
            running: &running,
            page: &page,
            req: &req,
            deadline,
            group: format!("chrome-cdp-eval-{}", GROUPS.fetch_add(1, Ordering::Relaxed)),
        };
        let (value, note) = self
            .unless_blocked(&running, &page, mark, run.run(locator.as_ref(), &options))
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
        let (context, element, note) = if let Some(locator) = locator {
            let (context, element, note) = self.element(locator, options).await?;
            (context, Some(element), note)
        } else {
            let context = self
                .engine
                .frame_context(
                    self.running,
                    self.page,
                    self.req.frame.as_deref(),
                    self.world(),
                    self.left(),
                )
                .await?;
            (context, None, None)
        };
        let result = self.evaluate(&context, element.as_deref()).await;
        let session = context.session.clone();
        let group = self.group.clone();
        tokio::spawn(async move {
            let _ = session
                .call(runtime::ReleaseObjectGroupParams::new(group))
                .await;
        });
        Ok((result?, note))
    }

    /// The `--in` element's frame context (in the chosen world) and the
    /// element's handle there.
    async fn element(
        &self,
        locator: &Locator,
        options: &LocateOptions,
    ) -> Result<(FrameContext, String, Option<String>), ErrorInfo> {
        let retry = |r: &str| Cmd::new("eval").flag("in", r).raw("…");
        let target = self
            .engine
            .locate_one(
                self.running,
                self.page,
                locator,
                options,
                Some(&retry),
                self.left(),
            )
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
        Ok((context, object, note))
    }

    /// Evaluates the script (with `element` as `el`), then finishes an
    /// object value.
    async fn evaluate(
        &self,
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
                return Err(self.thrown(details, wrapped.lines_before).await);
            }
            return self
                .finish(context, &result["result"], element, wrapped.lines_before)
                .await;
        }
        unreachable!("the last attempt returns")
    }

    /// The value of `object`, the script's result.
    async fn finish(
        &self,
        context: &FrameContext,
        object: &Value,
        element: Option<&str>,
        lines_before: u32,
    ) -> Result<ScriptValue, ErrorInfo> {
        let Some(object_id) = object["objectId"].as_str() else {
            return Ok(primitive(object));
        };
        let mut params = json!({
            "functionDeclaration": FINISH_JS,
            "objectId": object_id,
            "returnByValue": true,
            "awaitPromise": true,
            "userGesture": true,
        });
        if let Some(element) = element {
            params["arguments"] = json!([{ "objectId": element }]);
        }
        let session = context.session.with_timeout(self.left());
        let result = self
            .call(&session, "Runtime.callFunctionOn", params)
            .await?;
        if let Some(details) = result.get("exceptionDetails") {
            return Err(self.thrown(details, lines_before).await);
        }
        let value = &result["result"]["value"];
        Ok(ScriptValue {
            kind: value["type"].as_str().unwrap_or("object").to_owned(),
            text: value["text"].as_str().unwrap_or_default().to_owned(),
        })
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
                            .session(session),
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
                    Hint::run(Cmd::new("wait").switch("load").session(session))
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
                Hint::run(Cmd::new("eval").flag("in", "e12").raw("…").session(session))
                    .why("binds el to an element: a ref such as e12, or any locator"),
            );
        }
        if compile {
            return info.hint(
                Hint::run(Cmd::new("eval").arg("-").session(session))
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
    }
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
    };
    Ok(Output::typed(&data, text).quiet(quiet).limit(
        req.max_chars,
        "--max-chars 0 for all of it, or return less (one property, .slice(0, 20))",
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{compact, looks_like_object, primitive, wrap};

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
    fn compact_keeps_strings() {
        assert_eq!(
            compact("{\n  \"a b\": [\n    1,\n    \"x \\\" y\"\n  ]\n}"),
            "{\"a b\":[1,\"x \\\" y\"]}"
        );
    }
}
