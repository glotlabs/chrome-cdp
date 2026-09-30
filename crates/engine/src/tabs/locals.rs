//! Locals at uncaught errors (plan 15, milestone 15.5): the variables of
//! the frames an uncaught error went through, read while the page is
//! paused at it.
//!
//! Where locals capture is on (`localhost` pages by default, `debug
//! locals on|off` for the session), the session has Debugger on with
//! `setPauseOnExceptions("uncaught")` (see `super::debug`). At a pause of
//! reason `exception` or `promiseRejection` ([`capture`]), the top
//! [`FRAMES`] frames' `local`, `closure` and `block` scopes are listed
//! (`Runtime.getProperties`), at most [`VARIABLES`] names a frame, and
//! their values previewed in the page as a logpoint's are (one
//! `Debugger.evaluateOnCallFrame` a frame, running `logpoint.js`). The
//! pause lasts at most [`PAUSE_LIMIT`]: then it resumes with what was
//! read. A session gets at most [`PER_SECOND`] such reads a second; the
//! errors past that resume at once, and say why they have no locals.
//!
//! The page reports the error (`Runtime.exceptionThrown`) only once it
//! resumed, so what was read waits in the session ([`Pauses`]) for the
//! console entry of the same exception ([`for_exception`]): the same
//! thrown value (its description, which holds its stack, else its value)
//! thrown from the same script and line; else the same value from the
//! same script. The locals go to the entry's frames of the same index and
//! function name.
//!
//! Code chrome-cdp runs (`eval`, the injected scripts) has scripts with
//! no URL: a pause there resumes at once, and nothing is read.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use cdp::Session;
use cdp::domains::{debugger, runtime};
use protocol::schema::{ConsoleEntry, Local};
use serde::Deserialize;
use serde_json::Value;
use tokio::time::Instant;

use super::{Shared, State};
use crate::observe::is_local;

/// How many frames, innermost first, get their locals.
pub(super) const FRAMES: usize = 3;

/// How many variables a frame gets at most.
const VARIABLES: usize = 10;

/// How long a pause at an uncaught error may last.
const PAUSE_LIMIT: Duration = Duration::from_millis(200);

/// How many errors a second a session reads the locals of.
const PER_SECOND: usize = 10;

/// How many reads wait for their console entry, and for how long.
const KEPT: usize = 16;
const KEPT_FOR: Duration = Duration::from_secs(10);

/// The scopes read: not `global`, `script`, `module`, `with`, …
const SCOPES: [&str; 3] = ["local", "closure", "block"];

/// Why an error has no locals: the debugger did not stop at it (code
/// caught it, and reported it itself), or stopped where it was thrown
/// again (a framework's error handler). A logpoint on its line is the
/// next thing to try: text output says so.
pub(crate) const NOT_STOPPED: &str =
    "the debugger did not stop at it: code caught it first and reported it (React 19 does)";
pub(crate) const STOPPED_ELSEWHERE: &str =
    "it was caught and thrown again (Vue and Svelte do), so the debugger stopped elsewhere";

/// The logpoints' helper, which previews values.
const HELPER: &str = include_str!("../injected/logpoint.js");

/// A session's reads that wait for their console entries, and when the
/// latest were made.
#[derive(Debug, Default)]
pub(super) struct Pauses {
    read: VecDeque<Read>,
    recent: VecDeque<Instant>,
}

/// What one pause at an uncaught error gave.
#[derive(Debug, Clone)]
struct Read {
    /// The thrown value, as [`thrown`] gives it.
    thrown: String,
    /// The top frame's script and 0-based line.
    script: String,
    line: i64,
    /// The top frames' locals, innermost first.
    frames: Vec<FrameLocals>,
    /// Why it has no locals, if it was not read.
    reason: Option<String>,
    at: Instant,
}

/// A frame's function and its locals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameLocals {
    function: String,
    locals: Vec<Local>,
}

/// What an uncaught exception's console entry gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    /// Its top frames' locals.
    Locals(Vec<FrameLocals>),
    /// Why it has none.
    Missing(String),
}

impl Pauses {
    /// Whether one more read fits in this second, counting it if so.
    fn admit(&mut self, now: Instant) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1))
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= PER_SECOND {
            return false;
        }
        self.recent.push_back(now);
        true
    }

    fn keep(&mut self, read: Read) {
        self.expire(read.at);
        while self.read.len() >= KEPT {
            self.read.pop_front();
        }
        self.read.push_back(read);
    }

    fn expire(&mut self, now: Instant) {
        self.read.retain(|r| now.duration_since(r.at) < KEPT_FOR);
    }

    /// Takes the newest read of the exception `thrown` from `script` at
    /// `line`, else of the same value from the same script.
    fn take(&mut self, thrown: &str, script: &str, line: i64) -> Option<Read> {
        self.expire(Instant::now());
        let same = |r: &Read| r.thrown == thrown && r.script == script;
        let at = self
            .read
            .iter()
            .rposition(|r| same(r) && r.line == line)
            .or_else(|| self.read.iter().rposition(same))?;
        self.read.remove(at)
    }
}

/// The session paused at an uncaught error (`params` of
/// `Debugger.paused`): reads its top frames' locals, if capture is on
/// there, keeps them for the console entry, and resumes.
pub(super) async fn capture(shared: &Arc<Shared>, session: &Session, params: &Value) {
    let session_id = session.id().unwrap_or_default().to_owned();
    let top = &params["callFrames"][0];
    let script = top["location"]["scriptId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let now = Instant::now();
    let admitted = {
        let mut state = shared.lock();
        match state.debug.sessions.get_mut(&session_id) {
            // chrome-cdp's own code (`eval`, injected scripts) has no URL.
            Some(s) if s.mode.locals && !s.by_id.get(&script).is_some_and(|s| s.url.is_empty()) => {
                Some(s.pauses.admit(now))
            }
            _ => None,
        }
    };
    if let Some(admitted) = admitted {
        let mut read = Read {
            thrown: thrown(&params["data"]),
            script,
            line: top["location"]["lineNumber"].as_i64().unwrap_or(-1),
            frames: Vec::new(),
            reason: None,
            at: now,
        };
        if admitted {
            read.frames = frames(session, params, now + PAUSE_LIMIT).await;
        } else {
            read.reason = Some(format!(
                "too many errors: at most {PER_SECOND} a second get their locals"
            ));
        }
        if let Some(s) = shared.lock().debug.sessions.get_mut(&session_id) {
            s.pauses.keep(read);
        }
    }
    super::debug::resume(session).await;
}

/// The locals of the top [`FRAMES`] frames of a pause, until `deadline`:
/// a frame not read by then ends the list.
async fn frames(session: &Session, params: &Value, deadline: Instant) -> Vec<FrameLocals> {
    let mut out = Vec::new();
    let frames = params["callFrames"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    for frame in frames.iter().take(FRAMES) {
        let Some(call_frame) = frame["callFrameId"].as_str() else {
            break;
        };
        let mut names: Vec<String> = Vec::new();
        let scopes = frame["scopeChain"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        for scope in scopes {
            if names.len() >= VARIABLES {
                break;
            }
            let (Some(kind), Some(object)) =
                (scope["type"].as_str(), scope["object"]["objectId"].as_str())
            else {
                continue;
            };
            if !SCOPES.contains(&kind) {
                continue;
            }
            let Some(left) = left(deadline) else {
                return out;
            };
            let mut params = runtime::GetPropertiesParams::new(object.to_owned());
            params.own_properties = Some(true);
            let Ok(properties) = session.with_timeout(left).call(params).await else {
                return out;
            };
            for property in properties.result {
                // An inner scope's variable hides an outer one's.
                if names.len() < VARIABLES
                    && is_identifier(&property.name)
                    && !names.contains(&property.name)
                {
                    names.push(property.name);
                }
            }
        }
        let Some(locals) = values(session, call_frame, &names, deadline).await else {
            return out;
        };
        out.push(FrameLocals {
            function: frame["functionName"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            locals,
        });
    }
    out
}

/// What's left of the time until `deadline`, if any.
fn left(deadline: Instant) -> Option<Duration> {
    Some(deadline.saturating_duration_since(Instant::now())).filter(|d| !d.is_zero())
}

/// What `logpoint.js`'s `locals` sends back for each variable.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Shown {
    value: Option<String>,
    error: Option<String>,
}

/// Variables `names` of frame `call_frame`, previewed in the page.
/// `None` if that failed or the time ran out.
async fn values(
    session: &Session,
    call_frame: &str,
    names: &[String],
    deadline: Instant,
) -> Option<Vec<Local>> {
    if names.is_empty() {
        return Some(Vec::new());
    }
    let left = left(deadline)?;
    let mut params =
        debugger::EvaluateOnCallFrameParams::new(call_frame.to_owned(), expression(names));
    params.silent = Some(true);
    params.return_by_value = Some(true);
    let evaluated = session.with_timeout(left).call(params).await.ok()?;
    let shown: Vec<Shown> = serde_json::from_str(evaluated.result.value?.as_str()?).ok()?;
    let mut shown = shown.into_iter();
    Some(
        names
            .iter()
            .map(|name| {
                let s = shown.next().unwrap_or_default();
                let value = match (s.value, s.error) {
                    (Some(value), _) => value,
                    (None, Some(error)) => format!("<{error}>"),
                    (None, None) => "?".to_owned(),
                };
                Local {
                    name: name.clone(),
                    value,
                }
            })
            .collect(),
    )
}

/// The expression that previews `names` on a paused frame: a closure per
/// variable, so one that cannot be read (a `let` before its line) shows
/// its error and the others still show.
fn expression(names: &[String]) -> String {
    let thunks: Vec<String> = names.iter().map(|n| format!("() => {n}")).collect();
    format!(
        "(() => {{\n{HELPER}\nreturn __ccdpLogpoint.locals([{}]);\n}})()",
        thunks.join(", ")
    )
}

/// Whether `name` can be named in an expression (not an internal slot
/// such as `.generator_object`).
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// A thrown value as a pause's `data` and an exception's details both
/// show it: its description (an error's holds its stack), else its
/// value.
fn thrown(object: &Value) -> String {
    match object["description"].as_str() {
        Some(description) => description.to_owned(),
        None => object["value"].to_string(),
    }
}

/// What the console entry of `Runtime.exceptionThrown` (`params`, of
/// session `session_id`) gets: the locals read at its pause, or why it
/// has none. `None` for an exception with no stack (a syntax error).
pub(super) fn for_exception(state: &mut State, session_id: &str, params: &Value) -> Option<Found> {
    let details = &params["exceptionDetails"];
    let top = details["stackTrace"]["callFrames"].as_array()?.first()?;
    let local = state
        .targets
        .get(session_id)
        .is_some_and(|t| is_local(&t.url));
    let debug = &mut state.debug;
    let Some(session) = debug.sessions.get_mut(session_id).filter(|s| s.mode.locals) else {
        let why = match debug.locals {
            Some(false) => "capture is off; chrome-cdp debug locals on",
            None if !local => "the page is not on localhost; chrome-cdp debug locals on",
            _ => "the debugger was not on yet",
        };
        return Some(Found::Missing(why.to_owned()));
    };
    let read = session.pauses.take(
        &thrown(&details["exception"]),
        top["scriptId"].as_str().unwrap_or_default(),
        top["lineNumber"].as_i64().unwrap_or(-1),
    );
    Some(match read {
        Some(Read {
            reason: Some(reason),
            ..
        }) => Found::Missing(reason),
        Some(read) if read.frames.is_empty() => Found::Missing(format!(
            "reading them took over {}ms, the most a pause may last",
            PAUSE_LIMIT.as_millis()
        )),
        Some(read) => Found::Locals(read.frames),
        None => Found::Missing(NOT_STOPPED.to_owned()),
    })
}

/// Puts what was `found` for an exception in its console `entry`: the
/// locals in the frames of the same index and function.
pub(crate) fn attach(entry: &mut ConsoleEntry, found: &Found) {
    if entry.source != "exception" {
        return;
    }
    match found {
        Found::Locals(frames) => {
            let mut any = false;
            for (frame, read) in entry.stack.iter_mut().zip(frames) {
                if frame.function == read.function {
                    frame.locals = Some(read.locals.clone());
                    any = true;
                }
            }
            if !any && !frames.is_empty() {
                entry.no_locals = Some(STOPPED_ELSEWHERE.to_owned());
            }
        }
        Found::Missing(why) => entry.no_locals = Some(why.clone()),
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::{Level, Local, StackFrame};
    use tokio::time::Instant;

    use super::{Found, FrameLocals, Pauses, Read, attach, expression, is_identifier};
    use crate::observe::console;

    fn read(thrown: &str, line: i64, function: &str) -> Read {
        Read {
            thrown: thrown.into(),
            script: "7".into(),
            line,
            frames: vec![FrameLocals {
                function: function.into(),
                locals: vec![Local {
                    name: "item".into(),
                    value: "undefined".into(),
                }],
            }],
            reason: None,
            at: Instant::now(),
        }
    }

    #[test]
    fn reads_are_matched_by_value_script_and_line() {
        let mut pauses = Pauses::default();
        pauses.keep(read("TypeError: a", 20, "lineTotal"));
        pauses.keep(read("TypeError: a", 30, "other"));
        pauses.keep(read("TypeError: b", 20, "b"));
        assert_eq!(pauses.take("TypeError: a", "7", 20).unwrap().line, 20);
        // Else the same value from the same script.
        assert_eq!(pauses.take("TypeError: a", "7", 99).unwrap().line, 30);
        assert!(pauses.take("TypeError: a", "7", 20).is_none());
        assert!(pauses.take("TypeError: b", "8", 20).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn reads_are_bounded() {
        let mut pauses = Pauses::default();
        let now = Instant::now();
        assert!((0..10).all(|_| pauses.admit(now)));
        assert!(!pauses.admit(now));
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert!(pauses.admit(Instant::now()));
        for i in 0..20 {
            pauses.keep(read("e", i, "f"));
        }
        assert_eq!(pauses.read.len(), 16);
        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        assert!(pauses.take("e", "7", 19).is_none());
    }

    #[test]
    fn locals_go_to_the_frames_of_their_function() {
        let mut entry = console::entry(Level::Error, "exception", Some("uncaught"), "x".into());
        let frame = |function: &str| StackFrame {
            function: function.into(),
            url: "http://x/app.js".into(),
            line: 1,
            column: 1,
            ..StackFrame::default()
        };
        entry.stack = vec![frame("lineTotal"), frame("total")];
        let found = Found::Locals(vec![
            read("x", 1, "lineTotal").frames.remove(0),
            read("x", 1, "renamed").frames.remove(0),
        ]);
        attach(&mut entry, &found);
        assert_eq!(entry.stack[0].locals.as_ref().unwrap()[0].name, "item");
        assert!(entry.stack[1].locals.is_none());
        let mut entry = console::entry(Level::Error, "exception", None, "x".into());
        attach(&mut entry, &Found::Missing("why".into()));
        assert_eq!(entry.no_locals.as_deref(), Some("why"));
    }

    #[test]
    fn expressions_and_names() {
        assert!(is_identifier("item"));
        assert!(is_identifier("$x_1"));
        assert!(!is_identifier(".generator_object"));
        assert!(!is_identifier("1a"));
        assert!(!is_identifier(""));
        let text = expression(&["item".into(), "qty".into()]);
        assert!(text.ends_with("return __ccdpLogpoint.locals([() => item, () => qty]);\n})()"));
    }
}
