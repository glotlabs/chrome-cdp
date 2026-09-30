//! `explore` (plan 16, milestone 16.6): maps an app. From a start URL it
//! follows links (and with `--click-buttons` buttons, tabs, menu items
//! and disclosure controls), breadth first, and records every state it
//! finds, how to reach each, and what was wrong on each: console errors,
//! failed requests, broken links and with `--layout` `layout`'s errors.
//!
//! A state is the normalised URL plus the page's skeleton ([`rules`]),
//! so an open modal is a state of its own and a list with other data is
//! not. The walk ([`walk`]) clicks with the normal action code, so
//! client-side routing runs, and gets back to a state by loading its URL
//! when that gives the same skeleton, else by replaying its path.
//!
//! It runs in a browser context of its own, with a copy of the calling
//! context's cookies, storage, routes, mocks, init scripts and emulation,
//! so a logged-in or mocked app looks the same and the agent's tabs are
//! left alone. The context is closed at the end, also when the call is
//! dropped (the CLI went away on Ctrl-C). Its dialogs are answered at
//! once: `alert` and `beforeunload` accepted, `confirm` and `prompt`
//! dismissed.

mod render;
mod rules;
mod walk;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cdp::domains::target;
use protocol::schema::{ExploreBudget, ExploreData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use crate::context::{context_name, dispose, in_context};
use crate::errors::from_cdp;
use crate::pattern::UrlPattern;
use crate::request::{ExploreRequest, StateLoadRequest, millis};
use crate::tabs::{BLANK, Settings};
use crate::{Engine, Running};

/// Checks an `explore` request's arguments without a browser.
pub fn check_explore(req: &ExploreRequest) -> Result<(), ErrorInfo> {
    for (flag, value) in [
        ("max-states", req.max_states),
        ("max-actions", req.max_actions),
        ("time", req.time_ms),
    ] {
        if value == 0 {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("--{flag} must be more than 0"),
            ));
        }
    }
    for (flag, list) in [("include", &req.include), ("exclude", &req.exclude)] {
        for pattern in list {
            UrlPattern::parse(pattern).map_err(|e| {
                let message = format!("--{flag}: {}", e.message);
                ErrorInfo { message, ..e }
            })?;
        }
    }
    for text in &req.origins {
        let trimmed = text.trim_end_matches('/');
        if rules::origin(trimmed).as_deref() != Some(trimmed) {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("--origin {text}: not an origin"),
            )
            .hint(Hint::text(
                "an origin is a scheme, host and port: --origin https://api.example.com",
            )));
        }
    }
    if req.screenshots && req.dir.is_none() {
        return Err(
            ErrorInfo::new(ErrorKind::BadArgs, "--screenshots saves them in -o DIR").hint(
                Hint::run(
                    Cmd::new("explore")
                        .switch("screenshots")
                        .raw("-o")
                        .arg("map"),
                ),
            ),
        );
    }
    if let Some(dir) = &req.dir {
        check_dir(dir)?;
    }
    Ok(())
}

/// `-o DIR` must be empty or not exist.
fn check_dir(dir: &Path) -> Result<(), ErrorInfo> {
    let used = match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_some(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("-o {}: {e}", dir.display()),
            ));
        }
    };
    if used {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("-o {}: the directory is not empty", dir.display()),
        )
        .hint(Hint::text("name a new directory, or empty that one")));
    }
    Ok(())
}

/// Disposes of explore's context unless disarmed: when the call is
/// dropped half-way, from a task of its own.
struct Disposal {
    running: Arc<Running>,
    settings: Arc<Settings>,
    id: u32,
    timeout: Duration,
    armed: bool,
}

impl Drop for Disposal {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (running, settings, id, timeout) = (
            self.running.clone(),
            self.settings.clone(),
            self.id,
            self.timeout,
        );
        tokio::spawn(async move {
            dispose(&running, &settings, id, timeout).await;
        });
    }
}

impl Engine {
    /// `explore [URL]`.
    pub async fn explore(&self, req: ExploreRequest) -> Result<Output, ErrorInfo> {
        check_explore(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let url = if let Some(url) = &req.url {
            url.clone()
        } else {
            let page = self.page(&running, None, timeout).await?;
            let url = running
                .tabs
                .info(page.id)
                .map(|t| t.url)
                .unwrap_or_default();
            if url.is_empty() || url == BLANK {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    "explore needs a URL: the active tab shows no page",
                )
                .hint(Hint::run(Cmd::new("explore").arg("http://localhost:5173/"))));
            }
            url
        };
        // What the caller's context has, for explore's own.
        let from = self.context_key()?;
        let (storage, mut warnings) = match self.collect_state(&running, false, timeout).await {
            Ok(collected) => collected,
            Err(e) => (
                crate::state::StorageState::default(),
                vec![format!(
                    "cannot copy the cookies and storage: {}",
                    e.message
                )],
            ),
        };
        let created = running
            .conn
            .browser()
            .with_timeout(timeout)
            .call(target::CreateBrowserContextParams {
                dispose_on_detach: Some(true),
                ..target::CreateBrowserContextParams::default()
            })
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let browser_id = created.browser_context_id;
        let id = self.settings.contexts.add(&browser_id);
        let mut disposal = Disposal {
            running: running.clone(),
            settings: self.settings.clone(),
            id,
            timeout,
            armed: true,
        };
        self.settings
            .routes
            .copy_scope(from.as_deref(), &browser_id);
        self.settings
            .injections
            .copy_scope(from.as_deref(), &browser_id);
        let emulation = self.settings.emulation.get_in(from.as_deref());
        self.settings.emulation.set_in(Some(&browser_id), emulation);
        self.settings.answer_dialogs_in(&browser_id, true);
        running.tabs.apply_interception(timeout, false).await;
        let result = in_context(
            Some(context_name(id)),
            Box::pin(self.explore_in(&running, &req, &url, &browser_id, storage)),
        )
        .await;
        disposal.armed = false;
        dispose(&running, &self.settings, id, timeout).await;
        if (req.layout || req.screenshots)
            && let Some(active) = running.tabs.active()
        {
            let _ = running.tabs.bring_to_front(active, timeout).await;
        }
        let (data, walk_warnings) = result?;
        warnings.extend(walk_warnings);
        let lines = |event: &protocol::schema::Event| self.event_line(event);
        let text = render::explore_text(&data, &lines);
        let quiet = data.problems.to_string();
        let output = Output::typed(&data, text).quiet(quiet).limit(
            req.max_chars,
            "--json, -o DIR or --max-chars 0 for all of it",
        );
        Ok(warnings.into_iter().fold(output, Output::warning))
    }

    /// The walk, in explore's context `browser_id` (the call's context
    /// now): gives it the caller's storage, opens its tab and walks.
    async fn explore_in(
        &self,
        running: &Running,
        req: &ExploreRequest,
        url: &str,
        browser_id: &str,
        storage: crate::state::StorageState,
    ) -> Result<(ExploreData, Vec<String>), ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let mut warnings = Vec::new();
        if !storage.cookies.is_empty() || !storage.origins.is_empty() {
            let load = StateLoadRequest {
                state: storage,
                path: None,
                timeout_ms: req.timeout_ms,
            };
            match self.load_state(&load).await {
                Ok((_, loaded)) => warnings.extend(loaded),
                Err(e) => warnings.push(format!(
                    "cannot copy the cookies and storage: {}",
                    e.message
                )),
            }
        }
        let page = running
            .tabs
            .create_in(BLANK, Some(browser_id), timeout)
            .await
            .map_err(|e| self.tab_error(running, e))?;
        running.tabs.activate_in(browser_id, page.id);
        let states_dir = match &req.dir {
            Some(dir) => {
                let states = dir.join("states");
                std::fs::create_dir_all(&states).map_err(|e| {
                    ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("cannot make {}: {e}", states.display()),
                    )
                })?;
                Some(states)
            }
            None => None,
        };
        let mut walk = walk::Walk::new(self, running, req, page, states_dir.clone())?;
        walk.run(url).await?;
        let mut states = walk.states();
        let stopped_by = match walk.stopped {
            Some(stop) => stop.as_str(),
            None if walk.depth_left => "depth",
            None => "done",
        };
        let problems: usize = states
            .iter()
            .map(|s| {
                s.errors.len() + s.failed_requests.len() + s.layout.as_ref().map_or(0, Vec::len)
            })
            .sum::<usize>()
            + walk.broken.len();
        if let Some(states_dir) = &states_dir {
            for state in &mut states {
                let path = states_dir.join(format!("{}.cdp", state.id));
                let text = render::scenario(state, &walk.start);
                std::fs::write(&path, text).map_err(|e| {
                    ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("cannot write {}: {e}", path.display()),
                    )
                })?;
                state.scenario = Some(path.display().to_string());
            }
        }
        warnings.extend(std::mem::take(&mut walk.warnings));
        let data = ExploreData {
            start: walk.start.clone(),
            stopped_by: stopped_by.to_owned(),
            budget: ExploreBudget {
                max_states: req.max_states,
                max_actions: req.max_actions,
                depth: req.depth,
                time_ms: req.time_ms,
            },
            actions: walk.actions,
            duration_ms: u64::try_from(walk.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            states,
            edges: std::mem::take(&mut walk.edges),
            broken: std::mem::take(&mut walk.broken),
            skipped: std::mem::take(&mut walk.skipped),
            unvisited: std::mem::take(&mut walk.unvisited),
            failed: std::mem::take(&mut walk.failed),
            problems: problems as u64,
            dir: req.dir.as_ref().map(|d| d.display().to_string()),
        };
        if let Some(dir) = &req.dir {
            write_map(dir, &data)?;
        }
        Ok((data, warnings))
    }
}

/// `map.json`: the data, as `--json` has it.
fn write_map(dir: &Path, data: &ExploreData) -> Result<PathBuf, ErrorInfo> {
    let path = dir.join("map.json");
    let mut json = serde_json::to_string_pretty(data).unwrap_or_default();
    json.push('\n');
    std::fs::write(&path, json).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot write {}: {e}", path.display()),
        )
    })?;
    Ok(path)
}
