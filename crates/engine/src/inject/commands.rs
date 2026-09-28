//! `init-script add|list|remove`, `expose` and `unexpose` (plan 07,
//! milestone 7.3).
//!
//! Scripts and exposed functions belong to the session, not to a tab: they
//! go to every tab, popup and frame, those opened later too, and stay when
//! the browser is restarted. With `--context`, they belong to that browser
//! context and go only to its pages. None of these commands takes a tab
//! lock.

use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::{
    ExposeData, InitScriptAddData, InitScriptInfo, InitScriptRemoveData, InitScriptsData,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};

use super::{MAX_SCRIPT, check_binding_name, parse_script_id, script_name};
use crate::request::{
    ExposeRequest, InitScriptAddRequest, InitScriptListRequest, InitScriptRemoveRequest, millis,
};
use crate::{Engine, State};

impl Engine {
    /// `init-script add`: checks that the script compiles (in the active
    /// tab, if there is one), then adds it after the others.
    pub async fn init_script_add(&self, req: InitScriptAddRequest) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        if req.source.trim().is_empty() {
            return Err(
                ErrorInfo::new(ErrorKind::BadArgs, "the init script is empty").hint(Hint::run(
                    Cmd::new("init-script")
                        .arg("add")
                        .flag("code", "window.__test = true")
                        .session(&self.session),
                )),
            );
        }
        if req.source.len() > MAX_SCRIPT {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "the init script is {}; the most it can be is {}",
                    format::size(req.source.len() as u64),
                    format::size(MAX_SCRIPT as u64)
                ),
            ));
        }
        let scope = self.context_key()?;
        let scope = scope.as_deref();
        self.check_compiles(&req, scope, timeout).await?;
        let script = self
            .settings
            .injections
            .add(scope, &req.source, req.file.clone(), req.world);
        self.apply_injections(req.now, timeout).await;
        let added = self
            .settings
            .injections
            .list(scope)
            .into_iter()
            .find(|s| s.id == script_name(script.id))
            .ok_or_else(|| ErrorInfo::new(ErrorKind::BadArgs, "the init script was removed"))?;
        let mut text = script_line(&added);
        text.push('\n');
        if req.now {
            text.push_str(
                "ran in the open pages, and runs in every new document before its scripts",
            );
        } else {
            let _ = write!(
                text,
                "runs in every new document before its scripts; {} runs it in the current page",
                Cmd::new("reload").session(&self.session)
            );
        }
        let quiet = added.id.clone();
        let data = InitScriptAddData {
            added,
            ran_now: req.now,
            active: self.settings.injections.len(scope),
        };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// Errors when `req`'s script does not compile, as Chrome sees it in
    /// the active tab. Without a tab (or a browser) nothing is checked.
    async fn check_compiles(
        &self,
        req: &InitScriptAddRequest,
        scope: Option<&str>,
        timeout: Duration,
    ) -> Result<(), ErrorInfo> {
        let running = match &*self.lock_state() {
            State::Running(running) => running.clone(),
            State::Crashed(_) | State::Empty => return Ok(()),
        };
        let page = match scope {
            Some(scope) => running.tabs.page_in(scope, None, timeout).await,
            None => running.tabs.page(None, timeout).await,
        };
        let Ok(page) = page else {
            return Ok(());
        };
        let params = runtime::CompileScriptParams::new(req.source.clone(), String::new(), false);
        let compiled = match page.session.with_timeout(timeout).call(params).await {
            Ok(compiled) => compiled,
            Err(e) => {
                tracing::debug!("cannot compile the init script to check it: {e}");
                return Ok(());
            }
        };
        let Some(details) = compiled.exception_details else {
            return Ok(());
        };
        let message = details
            .exception
            .as_ref()
            .and_then(|e| e.description.clone())
            .unwrap_or(details.text);
        let message = message.lines().next().unwrap_or_default().to_owned();
        let place = format!(
            "{}line {}, column {}",
            req.file
                .as_ref()
                .map(|f| format!("{}, ", f.display()))
                .unwrap_or_default(),
            details.line_number + 1,
            details.column_number + 1
        );
        Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("the init script does not compile: {message}"),
        )
        .note("at", place)
        .hint(Hint::text("fix the script and add it again")))
    }

    /// `init-script list`: the scripts, in the order they run.
    pub fn init_script_list(&self, req: &InitScriptListRequest) -> Result<Output, ErrorInfo> {
        let scope = self.context_key()?;
        let scripts = self.settings.injections.list(scope.as_deref());
        let mut text = String::new();
        for script in &scripts {
            let _ = writeln!(text, "{}", script_line(script));
        }
        if scripts.is_empty() {
            let _ = write!(
                text,
                "no init scripts; {} adds one",
                Cmd::new("init-script")
                    .arg("add")
                    .raw("<file>")
                    .session(&self.session)
            );
        }
        let quiet: Vec<&str> = scripts.iter().map(|s| s.id.as_str()).collect();
        let quiet = quiet.join("\n");
        let data = InitScriptsData { scripts };
        Ok(Output::typed(&data, text.trim_end().to_owned())
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it"))
    }

    /// `init-script remove`: takes scripts out of new documents; pages
    /// keep what they already ran.
    pub async fn init_script_remove(
        &self,
        req: InitScriptRemoveRequest,
    ) -> Result<Output, ErrorInfo> {
        let injections = &self.settings.injections;
        let scope = self.context_key()?;
        let scope = scope.as_deref();
        let removed = if req.all {
            injections.clear(scope)
        } else {
            if req.ids.is_empty() {
                return Err(
                    ErrorInfo::new(ErrorKind::BadArgs, "no init script to remove")
                        .hint(Hint::run(
                            Cmd::new("init-script")
                                .arg("remove")
                                .arg("s1")
                                .session(&self.session),
                        ))
                        .hint(Hint::run(
                            Cmd::new("init-script")
                                .arg("remove")
                                .switch("all")
                                .session(&self.session),
                        )),
                );
            }
            let ids = req
                .ids
                .iter()
                .map(|id| parse_script_id(id))
                .collect::<Result<Vec<u32>, _>>()?;
            injections.remove(scope, &ids).map_err(|missing| {
                ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("there is no init script {}", script_name(missing)),
                )
                .hint(
                    Hint::run(Cmd::new("init-script").arg("list").session(&self.session))
                        .why("lists them"),
                )
            })?
        };
        self.apply_injections(false, millis(req.timeout_ms)).await;
        let removed: Vec<String> = removed.into_iter().map(script_name).collect();
        let active = injections.len(scope);
        let mut text = if removed.is_empty() {
            "no init scripts to remove".to_owned()
        } else {
            format!(
                "removed {} (open pages keep its effects until they load again)",
                removed.join(", ")
            )
        };
        let _ = write!(
            text,
            "; {}",
            match active {
                0 => "no init scripts left".to_owned(),
                1 => "1 init script left".to_owned(),
                n => format!("{n} init scripts left"),
            }
        );
        let quiet = removed.join("\n");
        let data = InitScriptRemoveData { removed, active };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// `expose` (lists without names) and `unexpose`.
    pub async fn expose(&self, req: ExposeRequest) -> Result<Output, ErrorInfo> {
        let injections = &self.settings.injections;
        for name in &req.names {
            check_binding_name(name)?;
        }
        let scope = self.context_key()?;
        let scope = scope.as_deref();
        let changed = if req.remove {
            if req.all {
                injections.unexpose_all(scope)
            } else {
                injections.unexpose(scope, &req.names).map_err(|missing| {
                    ErrorInfo::new(
                        ErrorKind::NotFound,
                        format!("no function called `{missing}` is exposed"),
                    )
                    .hint(Hint::run(Cmd::new("expose").session(&self.session)).why("lists them"))
                })?
            }
        } else {
            injections.expose(scope, &req.names)
        };
        if !changed.is_empty() {
            self.apply_injections(false, millis(req.timeout_ms)).await;
        }
        let bindings = injections.bindings(scope);
        let text = expose_text(&req, &changed, &bindings, &self.session);
        let quiet = bindings.join("\n");
        let data = ExposeData { changed, bindings };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// Makes the running browser (if any) have the session's init scripts
    /// and exposed functions; a browser started later gets them at its
    /// setup.
    pub(crate) async fn apply_injections(&self, now: bool, timeout: Duration) {
        let running = match &*self.lock_state() {
            State::Running(running) => Some(running.clone()),
            State::Crashed(_) | State::Empty => None,
        };
        if let Some(running) = running {
            running.tabs.apply_injections(now, timeout).await;
        }
    }
}

/// `s1 main /home/me/stub.js 2KB` or `s2 isolated 'window.x = 1' 12B`.
fn script_line(script: &InitScriptInfo) -> String {
    let what = match &script.file {
        Some(file) => file.clone(),
        None => format!("'{}'", script.preview),
    };
    format!(
        "{} {} {what} {}",
        script.id,
        script.world,
        format::size(script.size)
    )
}

/// What `expose` and `unexpose` print.
fn expose_text(
    req: &ExposeRequest,
    changed: &[String],
    bindings: &[String],
    session: &str,
) -> String {
    let mut text = String::new();
    if req.remove {
        if changed.is_empty() {
            text.push_str("no functions to remove");
        } else {
            let _ = write!(text, "removed {}", changed.join(", "));
        }
        let _ = write!(
            text,
            "; {}",
            if bindings.is_empty() {
                "no functions exposed".to_owned()
            } else {
                format!("still exposed: {}", bindings.join(", "))
            }
        );
        return text;
    }
    if req.names.is_empty() {
        if bindings.is_empty() {
            let _ = write!(
                text,
                "no functions exposed; {} adds one",
                Cmd::new("expose").arg("<name>").session(session)
            );
        } else {
            text = bindings.join("\n");
        }
        return text;
    }
    let already: Vec<&str> = req
        .names
        .iter()
        .filter(|n| !changed.contains(n))
        .map(String::as_str)
        .collect();
    if !changed.is_empty() {
        let _ = writeln!(
            text,
            "exposed {}; each call is logged as binding {}(args) in {}",
            changed.join(", "),
            changed[0],
            Cmd::new("console").session(session)
        );
    }
    if !already.is_empty() {
        let _ = writeln!(text, "already exposed: {}", already.join(", "));
    }
    text.trim_end().to_owned()
}
