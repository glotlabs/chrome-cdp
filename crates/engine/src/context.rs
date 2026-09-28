//! Browser contexts (plan 11, milestone 11.4): `context new|list|close`
//! and `--context` on every command.
//!
//! A context made with `context new` is a Chrome browser context: its own
//! cookies, storage and cache, like an incognito window. chrome-cdp gives
//! it its own settings too: routing rules, mock sets (and their state),
//! init scripts and exposed functions, storage seeds, and emulation. They
//! start empty and never apply outside the context, and the session's own
//! (those set without `--context`) never apply inside it, so two contexts
//! can be two logged-in users side by side, and the test runner gives
//! each test a fresh one. Permissions and the dialog, download and popup
//! policies stay the session's.
//!
//! Ids are `c1`, `c2`, … in the order contexts are made, never reused in
//! a session. Each context remembers its own active tab: a page command
//! with `--context c1` and no `--tab` acts on it (opening a blank tab in
//! the context when it has none).
//!
//! `--context` travels in the RPC request beside the params (see
//! `protocol::rpc::Request::context`): the daemon runs the call inside
//! [`in_context`], and the code that picks a tab or a settings scope asks
//! [`Engine::current_context`]. Internally a context's settings are keyed by
//! Chrome's `browserContextId`, which a paused request and a new target
//! carry.

use std::fmt::Write as _;
use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};

use cdp::domains::target;
use protocol::schema::{ContextCloseData, ContextInfo, ContextNewData, ContextsData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::json;

use crate::errors::from_cdp;
use crate::request::{
    ContextCloseRequest, ContextListRequest, ContextNewRequest, Navigation, WaitUntil, millis,
};
use crate::tabs::{BLANK, tab_name};
use crate::{Engine, Running};

tokio::task_local! {
    /// `--context` of the call being handled, as typed (`c1`).
    static CURRENT: Option<String>;
}

/// Runs `fut` (a call's handling) acting in context `context` (`c1`, as
/// `--context` gave it; `None` for the session's default context).
pub async fn in_context<F: Future>(context: Option<String>, fut: F) -> F::Output {
    CURRENT.scope(context, fut).await
}

/// `--context` of the call being handled.
fn current() -> Option<String> {
    CURRENT.try_with(Clone::clone).ok().flatten()
}

/// `c3`.
pub(crate) fn context_name(id: u32) -> String {
    format!("c{id}")
}

/// Parses a context id as typed: `c3` or `3`.
pub fn parse_context_id(text: &str) -> Result<u32, ErrorInfo> {
    let text = text.trim();
    let digits = text.strip_prefix('c').unwrap_or(text);
    digits
        .parse::<u32>()
        .ok()
        .filter(|&n| n > 0 && !digits.starts_with('+'))
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{text}` is not a context id; context ids look like c1, c2, …"),
            )
            .hint(Hint::run(Cmd::new("context").arg("list")).why("lists them"))
        })
}

/// The contexts made with `context new`. They belong to the running
/// browser: a new one starts without any (see [`Contexts::clear`]).
#[derive(Debug, Default)]
pub(crate) struct Contexts {
    inner: Mutex<Registry>,
}

#[derive(Debug, Default)]
struct Registry {
    /// The last id given.
    last: u32,
    /// By id.
    entries: Vec<Scope>,
}

/// A context, as the code acting in it needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scope {
    /// The number in its id (`3` for `c3`).
    pub(crate) id: u32,
    /// Chrome's `browserContextId`: the key of its settings.
    pub(crate) browser_id: String,
}

impl Scope {
    pub(crate) fn name(&self) -> String {
        context_name(self.id)
    }
}

impl Contexts {
    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records Chrome's context `browser_id`; returns its id.
    fn add(&self, browser_id: &str) -> u32 {
        let mut registry = self.lock();
        registry.last += 1;
        let id = registry.last;
        registry.entries.push(Scope {
            id,
            browser_id: browser_id.to_owned(),
        });
        id
    }

    pub(crate) fn get(&self, id: u32) -> Option<Scope> {
        self.lock().entries.iter().find(|e| e.id == id).cloned()
    }

    pub(crate) fn list(&self) -> Vec<Scope> {
        self.lock().entries.clone()
    }

    fn remove(&self, id: u32) -> Option<Scope> {
        let mut registry = self.lock();
        let index = registry.entries.iter().position(|e| e.id == id)?;
        Some(registry.entries.remove(index))
    }

    /// Forgets every context (their browser is gone); returns them.
    pub(crate) fn clear(&self) -> Vec<Scope> {
        std::mem::take(&mut self.lock().entries)
    }

    /// The settings scope of a target or request in Chrome's context
    /// `browser_id`: the id itself when it is one of ours, else `None`
    /// (the session's default context, or one chrome-cdp did not make).
    pub(crate) fn scope_of(&self, browser_id: Option<&str>) -> Option<String> {
        let browser_id = browser_id?;
        self.lock()
            .entries
            .iter()
            .any(|e| e.browser_id == browser_id)
            .then(|| browser_id.to_owned())
    }

    /// `c1` for Chrome's context `browser_id`, when it is one of ours.
    pub(crate) fn name_of(&self, browser_id: Option<&str>) -> Option<String> {
        let browser_id = browser_id?;
        self.lock()
            .entries
            .iter()
            .find(|e| e.browser_id == browser_id)
            .map(Scope::name)
    }
}

impl Engine {
    /// The context the call acts in (`--context`), or `None` for the
    /// session's default one. An id that is no context is `NotFound`.
    pub(crate) fn current_context(&self) -> Result<Option<Scope>, ErrorInfo> {
        let Some(text) = current() else {
            return Ok(None);
        };
        let id = parse_context_id(&text)?;
        self.settings
            .contexts
            .get(id)
            .map(Some)
            .ok_or_else(|| self.no_context(id))
    }

    /// The settings key of the call's context: Chrome's id for a context
    /// of ours, `None` for the session's default one.
    pub(crate) fn context_key(&self) -> Result<Option<String>, ErrorInfo> {
        Ok(self.current_context()?.map(|s| s.browser_id))
    }

    fn no_context(&self, id: u32) -> ErrorInfo {
        let open: Vec<String> = self
            .settings
            .contexts
            .list()
            .iter()
            .map(Scope::name)
            .collect();
        ErrorInfo::new(
            ErrorKind::NotFound,
            format!(
                "no context {} in session {}",
                context_name(id),
                self.session
            ),
        )
        .note(
            "contexts",
            if open.is_empty() {
                "none".to_owned()
            } else {
                open.join(", ")
            },
        )
        .hint(
            Hint::run(Cmd::new("context").arg("new").session(&self.session))
                .why("makes one")
                .or(Cmd::new("context")
                    .arg("list")
                    .session(&self.session)
                    .to_string()),
        )
        .details(json!({ "context": context_name(id), "open": open }))
    }

    /// Drops everything the session keeps for context `browser_id`: its
    /// routes, mock sets, injections and emulation.
    pub(crate) fn forget_scope(&self, browser_id: &str) {
        self.settings.routes.drop_scope(browser_id);
        self.settings.injections.drop_scope(browser_id);
        self.settings.emulation.drop_scope(browser_id);
    }

    /// `context new [url]`: makes a browser context with one tab (loading
    /// `url`), and makes that tab the context's active one.
    pub async fn context_new(&self, req: ContextNewRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let browser = running.conn.browser().with_timeout(timeout);
        let created = browser
            .call(target::CreateBrowserContextParams {
                // A connected browser keeps no context of ours once we
                // go.
                dispose_on_detach: Some(true),
                ..target::CreateBrowserContextParams::default()
            })
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let browser_id = created.browser_context_id;
        let id = self.settings.contexts.add(&browser_id);
        let page = match running
            .tabs
            .create_in(BLANK, Some(&browser_id), timeout)
            .await
        {
            Ok(page) => page,
            Err(e) => {
                let info = self.tab_error(&running, e);
                self.dispose(&running, id, timeout).await;
                return Err(info);
            }
        };
        running.tabs.activate_in(&browser_id, page.id);
        let mut warnings = Vec::new();
        if let Some(url) = &req.url {
            let _tab = self.tabs.lock(&page.target_id).await;
            let navigation = Navigation::Goto {
                url: url.clone(),
                base_url: req.base_url.clone(),
            };
            let navigated = self
                .navigate_page(&running, &page, &navigation, WaitUntil::Settle, timeout)
                .await?;
            warnings = navigated.warnings;
        }
        let _ = running.tabs.refresh(timeout).await;
        let scope = self
            .settings
            .contexts
            .get(id)
            .ok_or_else(|| self.no_context(id))?;
        let context = self.context_info(&running, &scope);
        let tab = running.tabs.info(page.id).ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::NotFound,
                format!(
                    "tab {} of context {} closed",
                    tab_name(page.id),
                    scope.name()
                ),
            )
        })?;
        let text = format!(
            "created context {} with tab {} {}\nuse it: {}",
            context.id,
            tab.id,
            crate::tabs::commands::shown_url(&tab.url),
            Cmd::new("snapshot")
                .flag("context", &context.id)
                .session(&self.session)
        );
        let quiet = context.id.clone();
        let data = ContextNewData { context, tab };
        let output = Output::typed(&data, text).quiet(quiet);
        Ok(warnings.into_iter().fold(output, Output::warning))
    }

    /// `context list`: the contexts, their tabs and settings.
    pub async fn context_list(&self, req: ContextListRequest) -> Result<Output, ErrorInfo> {
        let contexts = match self.running() {
            Ok(running) => {
                let _ = running.tabs.refresh(millis(req.timeout_ms)).await;
                self.settings
                    .contexts
                    .list()
                    .iter()
                    .map(|scope| self.context_info(&running, scope))
                    .collect()
            }
            Err(_) => Vec::new(),
        };
        let text = if contexts.is_empty() {
            format!(
                "no contexts; commands act in the session's default one ({} makes one)",
                Cmd::new("context").arg("new").session(&self.session)
            )
        } else {
            let rows: Vec<Vec<String>> = contexts
                .iter()
                .map(|c| {
                    let tabs = if c.tabs.is_empty() {
                        "no tabs".to_owned()
                    } else {
                        let list: Vec<String> = c
                            .tabs
                            .iter()
                            .map(|t| {
                                if c.active.as_ref() == Some(t) {
                                    format!("*{t}")
                                } else {
                                    t.clone()
                                }
                            })
                            .collect();
                        format!("tabs {}", list.join(" "))
                    };
                    let mut settings = Vec::new();
                    if !c.mocks.is_empty() {
                        settings.push(format!("mocks {}", c.mocks.join(" ")));
                    }
                    if c.routes > 0 {
                        settings.push(format!(
                            "{} route{}",
                            c.routes,
                            if c.routes == 1 { "" } else { "s" }
                        ));
                    }
                    vec![c.id.clone(), tabs, settings.join(", ")]
                })
                .collect();
            format::table(&rows)
        };
        let quiet: Vec<&str> = contexts.iter().map(|c| c.id.as_str()).collect();
        let quiet = quiet.join("\n");
        Ok(Output::typed(&ContextsData { contexts }, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it"))
    }

    /// `context close [id…] [--all]`: closes contexts (`--context`'s when
    /// no id is given), their tabs, and drops their settings.
    pub async fn context_close(&self, req: ContextCloseRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let ids: Vec<u32> = if req.all {
            self.settings.contexts.list().iter().map(|s| s.id).collect()
        } else if req.ids.is_empty() {
            match self.current_context()? {
                Some(scope) => vec![scope.id],
                None => {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        "which context? name one, or pass --context or --all",
                    )
                    .hint(
                        Hint::run(Cmd::new("context").arg("list").session(&self.session))
                            .why("lists them"),
                    ));
                }
            }
        } else {
            let ids = req
                .ids
                .iter()
                .map(|t| parse_context_id(t))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(&missing) = ids
                .iter()
                .find(|&&id| self.settings.contexts.get(id).is_none())
            {
                return Err(self.no_context(missing));
            }
            ids
        };
        let mut closed = Vec::new();
        let mut tabs = Vec::new();
        for id in ids {
            let Some(scope) = self.settings.contexts.get(id) else {
                continue;
            };
            tabs.extend(
                running
                    .tabs
                    .in_context(&scope.browser_id)
                    .into_iter()
                    .map(tab_name),
            );
            self.dispose(&running, id, timeout).await;
            closed.push(scope.name());
        }
        let mut text = if closed.is_empty() {
            "no contexts to close".to_owned()
        } else {
            format!(
                "closed context{} {}",
                if closed.len() == 1 { "" } else { "s" },
                closed.join(", ")
            )
        };
        if !tabs.is_empty() {
            let _ = write!(text, " (tabs {})", tabs.join(", "));
        }
        let quiet = closed.join("\n");
        Ok(Output::typed(&ContextCloseData { closed, tabs }, text).quiet(quiet))
    }

    /// Disposes of context `id` in Chrome (closing its tabs) and forgets
    /// it and its settings.
    async fn dispose(&self, running: &Running, id: u32, timeout: std::time::Duration) {
        let Some(scope) = self.settings.contexts.remove(id) else {
            return;
        };
        let result = running
            .conn
            .browser()
            .with_timeout(timeout)
            .call(target::DisposeBrowserContextParams::new(
                scope.browser_id.clone(),
            ))
            .await;
        if let Err(e) = result {
            tracing::debug!("cannot dispose of context {}: {e}", scope.name());
        }
        running.tabs.drop_context_tabs(&scope.browser_id);
        self.forget_scope(&scope.browser_id);
        running.tabs.apply_interception(timeout).await;
    }

    fn context_info(&self, running: &Running, scope: &Scope) -> ContextInfo {
        let tabs = running.tabs.in_context(&scope.browser_id);
        let active = running.tabs.active_in(&scope.browser_id).map(tab_name);
        ContextInfo {
            id: scope.name(),
            tabs: tabs.into_iter().map(tab_name).collect(),
            active,
            mocks: self.settings.routes.mock_names(Some(&scope.browser_id)),
            routes: self.settings.routes.count(Some(&scope.browser_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Contexts, context_name, parse_context_id};

    #[test]
    fn context_ids_parse_with_or_without_the_c() {
        assert_eq!(parse_context_id("c3").unwrap(), 3);
        assert_eq!(parse_context_id(" 12 ").unwrap(), 12);
        for bad in ["c0", "t1", "c", "", "c+1", "c-1", "cx"] {
            assert!(parse_context_id(bad).is_err(), "{bad}");
        }
        assert_eq!(context_name(7), "c7");
    }

    #[test]
    fn scopes_are_only_the_contexts_made_here() {
        let contexts = Contexts::default();
        let id = contexts.add("B1");
        assert_eq!(id, 1);
        assert_eq!(contexts.scope_of(Some("B1")).as_deref(), Some("B1"));
        assert_eq!(contexts.scope_of(Some("OTHER")), None);
        assert_eq!(contexts.scope_of(None), None);
        assert_eq!(contexts.name_of(Some("B1")).as_deref(), Some("c1"));
        assert_eq!(contexts.add("B2"), 2);
        assert!(contexts.remove(1).is_some());
        assert_eq!(contexts.scope_of(Some("B1")), None);
        // Ids are never reused.
        assert_eq!(contexts.add("B3"), 3);
        assert_eq!(contexts.clear().len(), 2);
        assert!(contexts.list().is_empty());
    }
}
