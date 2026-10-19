//! A load run's steps (plan 23.1): which commands a virtual user can
//! run, the engine request each one makes, and running it in-process.
//! The requests are built by the same functions the commands use
//! (`crate::session`, `crate::http`); only the daemon call is left out.

use engine::{
    ActionRequest, CookiesRequest, Engine, EvalRequest, ExpectRequest, HttpRequest,
    NavigateRequest, StorageRequest, WaitRequest,
};
use protocol::rpc::Settle;
use protocol::{ErrorInfo, ErrorKind, Hint, Output};

use super::think::Think;
use crate::cli::{Cli, Command, HttpArgs, Wait};
use crate::session;

/// What a load step does.
#[derive(Debug, Clone)]
pub(super) enum Call {
    /// `goto`, `back`, `forward`, `reload`; how it settles.
    Navigate(NavigateRequest, Settle),
    Act(ActionRequest),
    Expect(ExpectRequest),
    Wait(WaitRequest),
    /// `http`: the request, the statuses `--expect-status` takes, and the
    /// command (for the error), and what `--save` keeps it as.
    Http(Box<HttpCall>),
    Eval(EvalRequest),
    Cookies(CookiesRequest),
    Storage(StorageRequest),
    /// `think`: its time, or the run's `--think`.
    Think(Option<Think>),
}

#[derive(Debug, Clone)]
pub(super) struct HttpCall {
    request: HttpRequest,
    statuses: crate::http::Statuses,
    args: HttpArgs,
}

impl Call {
    /// What `--save NAME` keeps the answer as, for `http`.
    pub(super) fn saves(&self) -> Option<&str> {
        match self {
            Self::Http(http) => http.args.save.as_deref(),
            _ => None,
        }
    }
}

/// The commands a load run's steps can be.
const RUNS: &str = "navigation (goto, back, forward, reload), the input actions, expect, wait, http, eval, cookies, storage and think";

/// Whether `cli` can be a load step; the error says why not. `words` are
/// its words, for refs. Checked before any browser starts.
pub(super) fn check(cli: &Cli, words: &[String]) -> Result<(), ErrorInfo> {
    let Some(command) = &cli.command else {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "the line has no command",
        ));
    };
    if let Some(word) = words.iter().skip(1).find(|w| engine::is_ref(w)) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "{word} is a ref: it names an element of one session's snapshot, which a load run's users do not have"
            ),
        )
        .hint(Hint::text(
            "use a locator, e.g. 'role=button[name=\"Add to cart\"]' (history --as-scenario writes them)",
        )));
    }
    let interactive = |what: &str| {
        Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{what} is for one interactive user, not a load run's"),
        )
        .hint(Hint::text(format!(
            "leave it out of the journey; a load step is {RUNS}"
        ))))
    };
    match command {
        Command::Expect(args) if args.layout || args.audit => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "expect --layout and expect --audit are not load steps",
        )
        .hint(Hint::text(
            "check them in a test; a load run checks what users see with plain expect",
        ))),
        Command::Goto(_)
        | Command::Back(_)
        | Command::Forward(_)
        | Command::Reload(_)
        | Command::Expect(_)
        | Command::Wait(_)
        | Command::Http(_)
        | Command::Eval(_)
        | Command::Cookies(_)
        | Command::Storage(_)
        | Command::Think(_) => Ok(()),
        _ if crate::action(command)?.is_some() => Ok(()),
        Command::Snapshot(_) => interactive("snapshot"),
        Command::Screenshot(_) => interactive("screenshot"),
        Command::Debug(_) => interactive("debug"),
        Command::Highlight(_) => interactive("highlight"),
        Command::Inspect(_) => interactive("inspect"),
        Command::Explore(_) => interactive("explore"),
        _ => {
            let name = crate::typed_command(&words[..1]);
            Err(
                ErrorInfo::new(ErrorKind::BadArgs, format!("{name} is not a load step")).hint(
                    Hint::text(format!("a load step is {RUNS}; run the rest as a test")),
                ),
            )
        }
    }
}

/// The call of load step `cli` (which [`check`] took).
pub(super) fn call(cli: &Cli) -> Result<Call, ErrorInfo> {
    let global = &cli.global;
    let not_a_step = || ErrorInfo::new(ErrorKind::BadArgs, "not a load step");
    let Some(command) = &cli.command else {
        return Err(not_a_step());
    };
    // A step's own `--wait settle` asks for more than a test run's settle
    // (plan 22.3), as under `test`.
    let settle = |wait: &crate::cli::WaitArgs| {
        if wait.given && wait.wait == Wait::Settle {
            Settle::Full
        } else {
            Settle::Test
        }
    };
    Ok(match command {
        Command::Goto(args) => Call::Navigate(
            session::navigate_request(session::goto_navigation(args, global)?, &args.wait, global),
            settle(&args.wait),
        ),
        Command::Back(args) => Call::Navigate(
            session::navigate_request(engine::Navigation::Back, &args.wait, global),
            settle(&args.wait),
        ),
        Command::Forward(args) => Call::Navigate(
            session::navigate_request(engine::Navigation::Forward, &args.wait, global),
            settle(&args.wait),
        ),
        Command::Reload(args) => Call::Navigate(
            session::navigate_request(
                engine::Navigation::Reload { hard: args.hard },
                &args.wait,
                global,
            ),
            settle(&args.wait),
        ),
        Command::Expect(args) => Call::Expect(session::expect_request(args, global)?),
        Command::Wait(args) => Call::Wait(session::wait_request(args, global)?),
        Command::Http(args) => {
            let (request, statuses) = crate::http::request(args, global)?;
            Call::Http(Box::new(HttpCall {
                request,
                statuses,
                args: args.clone(),
            }))
        }
        Command::Eval(args) => Call::Eval(session::eval_request(args, global)?),
        Command::Cookies(args) => Call::Cookies(session::cookies_request(args, global)?),
        Command::Storage(args) => Call::Storage(session::storage_request(args, global)?),
        Command::Think(args) => Call::Think(args.time),
        command => match crate::action(command)? {
            Some((action, strict_refs, after)) => {
                Call::Act(session::act_request(action, strict_refs, after, global)?)
            }
            None => return Err(not_a_step()),
        },
    })
}

/// Runs `call` on `engine` in browser context `context`, as a load step:
/// settling as a test step does, with no action report. A `think` is the
/// user's to wait ([`super::user`]); here it does nothing.
pub(super) async fn run(engine: &Engine, context: &str, call: Call) -> Result<Output, ErrorInfo> {
    let settle = match &call {
        Call::Navigate(_, settle) => *settle,
        _ => Settle::Test,
    };
    let work = async {
        match call {
            Call::Navigate(req, _) => Box::pin(engine.navigate(req)).await,
            Call::Act(req) => Box::pin(engine.act(req)).await,
            Call::Expect(req) => Box::pin(engine.expect(req)).await,
            Call::Wait(req) => Box::pin(engine.wait(req)).await,
            Call::Http(http) => {
                let HttpCall {
                    request,
                    statuses,
                    args,
                } = *http;
                let output = Box::pin(engine.http(request)).await?;
                crate::http::check(&args, output, &statuses)
            }
            Call::Eval(req) => Box::pin(engine.eval(req)).await,
            Call::Cookies(req) => Box::pin(engine.cookies(req)).await,
            Call::Storage(req) => Box::pin(engine.storage(req)).await,
            Call::Think(_) => Ok(Output::new(serde_json::Value::Null, String::new())),
        }
    };
    Box::pin(engine::in_context(
        Some(context.to_owned()),
        engine::in_settle(settle, engine::in_lean(work)),
    ))
    .await
}

/// How a step ended, for `steps.jsonl` and the counts: `ok`, `failed` (a
/// check that did not hold), `timeout` or `error`; and the error's kind.
pub(super) fn result(error: Option<&ErrorInfo>) -> (&'static str, Option<String>) {
    let Some(error) = error else {
        return ("ok", None);
    };
    let result = match error.kind {
        ErrorKind::ExpectFailed => "failed",
        ErrorKind::Timeout => "timeout",
        _ => "error",
    };
    (result, Some(kind(error)))
}

/// An error's kind as the counts name it: `ExpectFailed`, `Timeout`, …,
/// `browser_crash` for a Chrome that went away, and `memory` for a page
/// above `[load] renderer_memory`.
pub(super) fn kind(error: &ErrorInfo) -> String {
    if error.kind == ErrorKind::BrowserCrashed {
        return super::BROWSER_CRASH.to_owned();
    }
    if error
        .details
        .as_ref()
        .is_some_and(|d| d["kind"] == super::MEMORY)
    {
        return super::MEMORY.to_owned();
    }
    serde_json::to_value(error.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{:?}", error.kind))
}
