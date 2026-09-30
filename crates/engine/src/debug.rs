//! `debug log`, `debug list` and `debug clear` (plan 15, milestone
//! 15.4): logpoints, printf debugging without editing the page's code.
//!
//! The logpoints themselves (binding, hits, pauses) are a feature of the
//! tabs (see `tabs::debug`); this is the commands' side: checking the
//! expressions, and the text and `--json` output.

use std::fmt::Write as _;
use std::path::PathBuf;

use cdp::domains::runtime;
use protocol::schema::{DebugClearData, DebugListData, DebugLocalsData, DebugLogData, Logpoint};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde_json::json;

use crate::Engine;
use crate::frames::World;
use crate::input::origin;
use crate::observe::shown_url;
use crate::places;
use crate::request::{
    DebugClearRequest, DebugListRequest, DebugLocalsRequest, DebugLogRequest, LocalsMode, millis,
};
use crate::tabs::{Cleared, LogError, Spec, parse_tab_id};

impl Engine {
    /// `debug log`: a logpoint at a source line of the tab's scripts.
    pub async fn debug_log(&self, req: DebugLogRequest) -> Result<Output, ErrorInfo> {
        if req.line == 0 {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("line 0 of {} is no line: lines count from 1", req.path),
            )
            .hint(Cmd::new("debug log").arg(format!("{}:1", req.path))));
        }
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let id = req.tab.as_deref().map(parse_tab_id).transpose()?;
        // Not `Engine::page`: setting a logpoint is no page command.
        let page = running
            .tabs
            .page(id, timeout)
            .await
            .map_err(|e| self.tab_error(&running, e))?;
        self.check_exprs(&running, &page, &req.exprs, timeout)
            .await?;
        let spec = Spec {
            path: req.path.clone(),
            file: req.file.as_ref().map(PathBuf::from),
            line: req.line,
            column: req.column,
            exprs: req.exprs.clone(),
            limit: req.limit,
        };
        let logpoint = running
            .tabs
            .debug_log(page.id, spec, places::root(), timeout)
            .await
            .map_err(|e| self.log_error(&req, e))?;
        let main_url = running
            .tabs
            .frames(page.id)
            .first()
            .map(|f| f.url.clone())
            .unwrap_or_default();
        let text = log_text(&logpoint, origin(&main_url));
        let quiet = logpoint.id.to_string();
        Ok(Output::typed(&DebugLogData { logpoint }, text).quiet(quiet))
    }

    /// `debug list`: the session's logpoints.
    pub fn debug_list(&self, _req: DebugListRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let logpoints = running.tabs.debug_list(places::root());
        let text = if logpoints.is_empty() {
            format!(
                "no logpoints; {} sets one",
                Cmd::new("debug log")
                    .raw("<file>:<line>")
                    .raw("<expr>…")
                    .session(&self.session)
            )
        } else {
            let tabs = logpoints
                .iter()
                .map(|l| l.tab.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len();
            logpoints
                .iter()
                .map(|l| list_line(l, tabs > 1))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let quiet = logpoints
            .iter()
            .map(|l| l.id.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let limit = 20_000;
        Ok(Output::typed(&DebugListData { logpoints }, text)
            .quiet(quiet)
            .limit(limit, "--json for all of it"))
    }

    /// `debug clear`: removes a logpoint, or all of them.
    pub async fn debug_clear(&self, req: DebugClearRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let cleared = running
            .tabs
            .debug_clear(req.id, timeout)
            .await
            .map_err(|ids| self.no_logpoint(req.id.unwrap_or_default(), &ids))?;
        let text = clear_text(&cleared, &self.session);
        let data = DebugClearData {
            cleared: cleared.ids,
            remaining: cleared.remaining,
            debugger: cleared.debugger,
        };
        Ok(Output::typed(&data, text))
    }

    /// `debug locals`: where uncaught errors get their frames' local
    /// variables, changed if asked.
    pub async fn debug_locals(&self, req: DebugLocalsRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let (setting, capturing) = match req.set {
            Some(mode) => {
                let setting = match mode {
                    LocalsMode::On => Some(true),
                    LocalsMode::Off => Some(false),
                    LocalsMode::Localhost => None,
                };
                running
                    .tabs
                    .debug_locals(setting, millis(req.timeout_ms))
                    .await
            }
            None => running.tabs.locals_now(),
        };
        let locals = match setting {
            Some(true) => "on",
            Some(false) => "off",
            None => "localhost",
        };
        let text = locals_text(setting, &self.session);
        Ok(Output::typed(
            &DebugLocalsData {
                locals: locals.to_owned(),
                capturing,
            },
            text,
        )
        .quiet(locals))
    }

    /// Fails with `BadArgs` for an expression that is not JavaScript,
    /// compiled (not run) in the tab's main frame.
    async fn check_exprs(
        &self,
        running: &crate::Running,
        page: &crate::tabs::Page,
        exprs: &[String],
        timeout: std::time::Duration,
    ) -> Result<(), ErrorInfo> {
        if exprs.is_empty() {
            return Ok(());
        }
        let context = self
            .frame_context(running, page, None, World::Main, timeout)
            .await?;
        let session = context.session.with_timeout(timeout);
        for expr in exprs {
            let mut params = runtime::CompileScriptParams::new(
                format!("(() => ({expr}\n))"),
                String::new(),
                false,
            );
            params.execution_context_id = Some(context.context_id);
            let compiled = session
                .call(params)
                .await
                .map_err(|e| crate::errors::from_cdp(&e, &self.session))?;
            if let Some(details) = compiled.exception_details {
                let why = details
                    .exception
                    .and_then(|e| e.description)
                    .unwrap_or(details.text);
                let why = why.lines().next().unwrap_or_default().to_owned();
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("{expr:?} is not a JavaScript expression: {why}"),
                )
                .hint("give each expression as one argument, e.g. chrome-cdp debug log src/cart.ts:42 total 'items.length'")
                .details(json!({ "expr": expr, "error": why })));
            }
        }
        Ok(())
    }

    fn log_error(&self, req: &DebugLogRequest, error: LogError) -> ErrorInfo {
        match error {
            LogError::TooMany => ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "the session has {} logpoints, the most it can have",
                    DebugLogRequest::MAX
                ),
            )
            .hint(
                Hint::run(Cmd::new("debug clear").raw("<id>").session(&self.session))
                    .why("chrome-cdp debug list shows them"),
            ),
            LogError::Ambiguous(candidates) => {
                let place = |candidate: &str| {
                    let mut cmd = Cmd::new("debug log").arg(format!("{candidate}:{}", req.line));
                    for expr in &req.exprs {
                        cmd = cmd.arg(expr);
                    }
                    cmd.session(&self.session)
                };
                let mut error = ErrorInfo::new(
                    ErrorKind::Ambiguous,
                    format!(
                        "{} names {} source files of the tab's scripts",
                        req.path,
                        candidates.len()
                    ),
                );
                for candidate in &candidates {
                    error = error.note("candidate", candidate.clone());
                }
                error
                    .hint(Hint::run(place(&candidates[0])).why("more of the path picks one"))
                    .details(json!({ "candidates": candidates }))
            }
            LogError::Cdp(e) => crate::errors::from_cdp(&e, &self.session),
        }
    }

    fn no_logpoint(&self, id: u32, ids: &[u32]) -> ErrorInfo {
        let known = if ids.is_empty() {
            "the session has none".to_owned()
        } else {
            let ids: Vec<String> = ids.iter().map(u32::to_string).collect();
            format!("the session has {}", ids.join(", "))
        };
        ErrorInfo::new(ErrorKind::NotFound, format!("no logpoint {id}; {known}"))
            .hint(Cmd::new("debug list").session(&self.session))
            .details(json!({ "id": id, "ids": ids }))
    }
}

/// `debug log`'s text: where the logpoint is bound, or that it is
/// pending.
fn log_text(logpoint: &Logpoint, page_origin: Option<&str>) -> String {
    let path = logpoint
        .place
        .rsplit_once(':')
        .map_or(logpoint.place.as_str(), |(path, _)| path);
    if logpoint.state == "pending" {
        return format!(
            "logpoint {} at {} is pending: no script of {} has {path} yet; it binds when one loads",
            logpoint.id, logpoint.place, logpoint.tab
        );
    }
    let mut why = Vec::new();
    if let Some(bound) = logpoint.bound_line {
        let what = if bound > logpoint.line {
            "no code"
        } else {
            "no code from there on"
        };
        why.push(format!("line {} has {what}", logpoint.line));
    }
    why.push(match logpoint.scripts.as_slice() {
        [script] => format!("bound in {}", shown_url(script, page_origin)),
        scripts => format!("bound in {} scripts", scripts.len()),
    });
    format!(
        "logpoint {} at {} ({})",
        logpoint.id,
        logpoint.place,
        why.join("; ")
    )
}

/// A line of `debug list`: `1 src/cart.ts:42  total, items.length  hits
/// 1`, with the tab when the logpoints are in several.
fn list_line(logpoint: &Logpoint, tabs: bool) -> String {
    let mut line = format!("{} {}", logpoint.id, logpoint.place);
    if tabs {
        let _ = write!(line, " [{}]", logpoint.tab);
    }
    if !logpoint.exprs.is_empty() {
        let _ = write!(line, "  {}", logpoint.exprs.join(", "));
    }
    let _ = write!(line, "  hits {}", logpoint.hits);
    match logpoint.state.as_str() {
        "pending" => line.push_str("  (pending: no script has it yet)"),
        "stopped" => {
            let _ = write!(line, "  (stopped: reached --limit {})", logpoint.limit);
        }
        _ => {}
    }
    line
}

/// `debug clear`'s text.
fn clear_text(cleared: &Cleared, session: &str) -> String {
    let ids: Vec<String> = cleared.ids.iter().map(u32::to_string).collect();
    let what = match ids.as_slice() {
        [] => "no logpoints to clear".to_owned(),
        [id] => format!("cleared logpoint {id}"),
        ids => format!("cleared logpoints {}", ids.join(", ")),
    };
    match (cleared.remaining, cleared.debugger) {
        (0, false) => format!("{what}; the debugger is off"),
        // Only locals keep it on.
        (0, true) => format!(
            "{what}; the debugger stays on for locals at uncaught errors ({} turns it off)",
            Cmd::new("debug locals").arg("off").session(session)
        ),
        (left, _) => format!("{what}; {left} left"),
    }
}

/// `debug locals`' text: where locals are captured.
fn locals_text(setting: Option<bool>, session: &str) -> String {
    let errors = Cmd::new("errors").switch("locals").session(session);
    match setting {
        Some(true) => {
            format!("locals capture is on everywhere: {errors} shows an uncaught error's variables")
        }
        Some(false) => format!(
            "locals capture is off; {} turns it on",
            Cmd::new("debug locals").arg("on").session(session)
        ),
        None => format!(
            "locals capture is on for localhost pages: {errors} shows an uncaught error's variables"
        ),
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::Logpoint;

    use super::{list_line, log_text};

    fn logpoint() -> Logpoint {
        Logpoint {
            id: 1,
            tab: "t1".into(),
            place: "src/cart.ts:15".into(),
            file: None,
            source: Some("http://localhost:1/src/cart.ts".into()),
            line: 15,
            bound_line: None,
            column: None,
            exprs: vec!["what".into(), "clicks".into()],
            state: "bound".into(),
            scripts: vec!["http://localhost:1/assets/index.js".into()],
            hits: 2,
            limit: 1000,
        }
    }

    #[test]
    fn texts() {
        let mut l = logpoint();
        assert_eq!(
            log_text(&l, Some("http://localhost:1")),
            "logpoint 1 at src/cart.ts:15 (bound in /assets/index.js)"
        );
        assert_eq!(
            list_line(&l, false),
            "1 src/cart.ts:15  what, clicks  hits 2"
        );
        l.line = 12;
        l.bound_line = Some(15);
        assert_eq!(
            log_text(&l, Some("http://localhost:1")),
            "logpoint 1 at src/cart.ts:15 (line 12 has no code; bound in /assets/index.js)"
        );
        l.state = "pending".into();
        l.place = "src/lazy.ts:3".into();
        assert_eq!(
            log_text(&l, None),
            "logpoint 1 at src/lazy.ts:3 is pending: no script of t1 has src/lazy.ts yet; it binds when one loads"
        );
        l.state = "stopped".into();
        l.exprs.clear();
        assert_eq!(
            list_line(&l, true),
            "1 src/lazy.ts:3 [t1]  hits 2  (stopped: reached --limit 1000)"
        );
    }
}
