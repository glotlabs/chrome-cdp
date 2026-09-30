//! Coverage as a feature of the registry (plan 13, milestone 13.4): while
//! `coverage start` measures a tab, every page, iframe and worker session
//! of it has V8's precise coverage on (`Profiler`), and pages and iframes
//! CSS rule tracking too (`CSS`, which needs `DOM`): those that exist when
//! it starts ([`Tabs::coverage_start`]) and those that attach while it
//! runs, set up before they run a script ([`setup_steps`]).
//! [`Tabs::coverage_stop`] takes what each measured and turns it off.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cdp::Session;
use cdp::domains::{css, dom, profiler, runtime};
use futures_util::future::join_all;
use protocol::schema::Worker;
use tokio::task::AbortHandle;

use super::features::NewTarget;
use super::targets::{Target, TargetKind};
use super::{Shared, State, Step, Tabs, lock, step};

/// How many stylesheets a session keeps the size of.
const SHEETS: usize = 5000;

/// A stylesheet a session reported: its URL, whether inline, and length.
#[derive(Debug, Clone)]
pub(crate) struct Sheet {
    pub(crate) url: String,
    pub(crate) inline: bool,
    pub(crate) length: f64,
}

/// A running `coverage start`.
#[derive(Debug)]
pub(super) struct Measuring {
    tab: u32,
    started: Instant,
    /// The sessions measured, by id.
    sessions: HashMap<String, Session>,
    /// Each page's or iframe's stylesheets, by session id and sheet id.
    sheets: Arc<Mutex<HashMap<String, HashMap<String, Sheet>>>>,
    /// The tasks that collect them.
    watchers: Vec<AbortHandle>,
}

impl Measuring {
    /// Whether it measures on session `id` (with `DOM` on in a page or
    /// iframe).
    pub(super) fn uses(&self, id: &str) -> bool {
        self.sessions.contains_key(id)
    }

    /// The ids of the sessions it measures on.
    pub(super) fn session_ids(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }
}

impl Drop for Measuring {
    fn drop(&mut self) {
        for watcher in &self.watchers {
            watcher.abort();
        }
    }
}

/// What one session measured.
#[derive(Debug)]
pub(crate) struct Measured {
    pub(crate) scripts: Vec<profiler::ScriptCoverage>,
    pub(crate) rules: Vec<css::RuleUsage>,
    pub(crate) sheets: HashMap<String, Sheet>,
    /// An out-of-process iframe's frame id.
    pub(crate) frame: Option<String>,
    /// A worker, as logs name it.
    pub(crate) worker: Option<Worker>,
}

/// What `coverage stop` took.
#[derive(Debug)]
pub(crate) struct Stopped {
    pub(crate) tab: u32,
    pub(crate) started: Instant,
    /// The tab's page first.
    pub(crate) measured: Vec<Measured>,
}

impl Tabs {
    /// The tab `coverage start` measures, if it runs.
    pub(crate) fn coverage_tab(&self) -> Option<u32> {
        self.shared.lock().coverage.as_ref().map(|c| c.tab)
    }

    /// Whether `coverage` measures on `session` now (it needs `DOM` on).
    pub(crate) fn coverage_uses(&self, session: &Session) -> bool {
        let id = session.id().unwrap_or_default();
        self.shared
            .lock()
            .coverage
            .as_ref()
            .is_some_and(|c| c.sessions.contains_key(id))
    }

    /// Turns coverage on in every page, iframe and worker session of tab
    /// `tab`, and in those that attach until [`Tabs::coverage_stop`].
    /// Fails if the tab's page fails; another's failure (it went away)
    /// only leaves it out.
    pub(crate) async fn coverage_start(
        &self,
        tab: u32,
        timeout: std::time::Duration,
    ) -> cdp::Result<()> {
        let (page_id, targets): (Option<String>, Vec<(Session, TargetKind)>) = {
            let mut state = self.shared.lock();
            state.coverage = Some(Measuring {
                tab,
                started: Instant::now(),
                sessions: HashMap::new(),
                sheets: Arc::default(),
                watchers: Vec::new(),
            });
            let ids: Vec<String> = state
                .targets
                .iter()
                .filter(|(_, t)| measured(&state, t, tab))
                .map(|(id, _)| id.clone())
                .collect();
            let targets = ids
                .iter()
                .filter_map(|id| {
                    let target = state.targets.get(id)?;
                    let (session, kind) = (target.session.clone(), target.kind);
                    register(&mut state, &session, kind);
                    Some((session, kind))
                })
                .collect();
            let page_id = state
                .tabs
                .get(&tab)
                .and_then(|t| t.session.id().map(str::to_owned));
            (page_id, targets)
        };
        let calls = targets.iter().map(|(session, kind)| {
            let session = session.with_timeout(timeout);
            let steps = steps(&session, *kind);
            async move {
                let mut result = Ok(());
                for (what, step) in steps {
                    if let Err(e) = step.await {
                        tracing::debug!("coverage: {what} failed: {e}");
                        result = result.and(Err(e));
                    }
                }
                (session.id().unwrap_or_default().to_owned(), result)
            }
        });
        for (id, result) in join_all(calls).await {
            if Some(&id) == page_id.as_ref() {
                result?;
            }
        }
        Ok(())
    }

    /// Takes what every session measured since [`Tabs::coverage_start`]
    /// and turns coverage off; `None` if it was not on. Fails if the
    /// tab's page cannot say; a session that went away meanwhile is left
    /// out.
    ///
    /// Out-of-process iframes of one site share a process, and so a V8
    /// isolate, whose coverage any of their sessions reports whole: it is
    /// taken once per isolate (`Runtime.getIsolateId`), and each script
    /// goes to the iframe whose document it is inline in, else to the
    /// first of them in document order.
    pub(crate) async fn coverage_stop(
        &self,
        timeout: std::time::Duration,
    ) -> Option<cdp::Result<Stopped>> {
        let measuring = self.shared.lock().coverage.take()?;
        let (page_id, mut sessions) = {
            let state = self.shared.lock();
            let tab = state.tabs.get(&measuring.tab);
            let page_id = tab.and_then(|t| t.session.id().map(str::to_owned));
            let frames = tab.map(|t| t.frames.list()).unwrap_or_default();
            let sessions: Vec<Of> = measuring
                .sessions
                .iter()
                .filter_map(|(id, session)| {
                    let target = state.targets.get(id)?;
                    let documents = frames
                        .iter()
                        .filter(|f| tab.and_then(|t| t.frames.owner(&f.id)) == Some(id.as_str()))
                        .map(|f| f.url.clone())
                        .collect();
                    Some(Of {
                        id: id.clone(),
                        session: session.with_timeout(timeout),
                        host: target.kind.is_host(),
                        keep_dom: target.node_stacks,
                        frame: (target.kind == TargetKind::Iframe).then(|| target.id.clone()),
                        worker: target.worker(),
                        documents,
                        order: frames.iter().position(|f| f.id == target.id),
                    })
                })
                .collect();
            (page_id, sessions)
        };
        // The page first, then its iframes in document order, then the
        // workers.
        sessions.sort_by(|a, b| {
            let key = |s: &Of| (Some(&s.id) != page_id.as_ref(), s.order.is_none(), s.order);
            key(a).cmp(&key(b)).then_with(|| a.id.cmp(&b.id))
        });
        let isolates: Vec<Option<String>> = join_all(
            sessions
                .iter()
                .map(|s| s.session.call(runtime::GetIsolateIdParams {})),
        )
        .await
        .into_iter()
        .map(|r| r.ok().map(|r| r.id))
        .collect();
        // The first session of each isolate takes its scripts; every one
        // is asked before any is turned off.
        let mut seen = HashSet::new();
        let takers: Vec<bool> = isolates
            .iter()
            .map(|isolate| isolate.as_ref().is_none_or(|id| seen.insert(id.clone())))
            .collect();
        let takes = sessions.iter().zip(&takers).map(|(of, &taker)| {
            let scripts = taker.then(|| of.session.call(profiler::TakePreciseCoverageParams {}));
            let rules = of
                .host
                .then(|| of.session.call(css::StopRuleUsageTrackingParams {}));
            async move {
                let scripts = match scripts {
                    Some(call) => call.await.map(|r| r.result),
                    None => Ok(Vec::new()),
                };
                let rules = match rules {
                    Some(call) => call.await.map(|r| r.rule_usage),
                    None => Ok(Vec::new()),
                };
                scripts.and_then(|scripts| Ok((scripts, rules?)))
            }
        });
        let taken = join_all(takes).await;
        join_all(
            sessions
                .iter()
                .map(|of| off(&of.session, of.host, of.keep_dom)),
        )
        .await;
        let mut all_sheets = std::mem::take(&mut *lock(&measuring.sheets));
        let mut measured: Vec<Measured> = Vec::new();
        let mut scripts_of: Vec<Vec<profiler::ScriptCoverage>> = vec![Vec::new(); sessions.len()];
        for (i, (of, result)) in sessions.iter().zip(taken).enumerate() {
            let (scripts, rules) = match result {
                Ok(taken) => taken,
                Err(e) if Some(&of.id) == page_id.as_ref() => return Some(Err(e)),
                Err(e) => {
                    tracing::debug!("coverage: a session went away: {e}");
                    (Vec::new(), Vec::new())
                }
            };
            // The iframes of its isolate: a script inline in one's
            // document is that one's.
            let mates: Vec<usize> = (0..sessions.len())
                .filter(|&j| {
                    j == i
                        || (isolates[j].is_some()
                            && isolates[j] == isolates[i]
                            && sessions[i].frame.is_some()
                            && sessions[j].frame.is_some())
                })
                .collect();
            for script in scripts {
                let owner = mates
                    .iter()
                    .copied()
                    .find(|&j| sessions[j].documents.contains(&script.url))
                    .unwrap_or(i);
                scripts_of[owner].push(script);
            }
            measured.push(Measured {
                scripts: Vec::new(),
                rules,
                sheets: all_sheets.remove(&of.id).unwrap_or_default(),
                frame: of.frame.clone(),
                worker: of.worker.clone(),
            });
        }
        for (entry, scripts) in measured.iter_mut().zip(scripts_of) {
            entry.scripts = scripts;
        }
        Some(Ok(Stopped {
            tab: measuring.tab,
            started: measuring.started,
            measured,
        }))
    }
}

/// A session `coverage stop` asks, and what marks its files.
struct Of {
    id: String,
    session: Session,
    /// A page's or iframe's: it has `CSS`.
    host: bool,
    /// Node stacks record in it: `DOM` stays on.
    keep_dom: bool,
    /// An out-of-process iframe's frame id.
    frame: Option<String>,
    worker: Option<Worker>,
    /// The URLs of the documents of its frames.
    documents: Vec<String>,
    /// Where its frame is in the tab's frames.
    order: Option<usize>,
}

/// Whether coverage of tab `tab` measures `target`.
fn measured(state: &State, target: &Target, tab: u32) -> bool {
    super::features::Feature::Coverage.applies(target) && state.tabs_of(target).contains(&tab)
}

/// Adds `session` to the running measurement, with a collector of its
/// stylesheets if it has them (before `CSS.enable`, which reports those
/// there are). Returns `false` if it was already in it.
fn register(state: &mut State, session: &Session, kind: TargetKind) -> bool {
    let Some(measuring) = state.coverage.as_mut() else {
        return false;
    };
    let id = session.id().unwrap_or_default().to_owned();
    if measuring.sessions.contains_key(&id) {
        return false;
    }
    measuring.sessions.insert(id.clone(), session.clone());
    if kind.is_host() {
        let mut events = session.listen::<css::StyleSheetAdded>();
        let sheets = measuring.sheets.clone();
        let watcher = tokio::spawn(async move {
            while let Some(added) = events.next().await {
                let header = added.header;
                let mut sheets = lock(&sheets);
                let sheets = sheets.entry(id.clone()).or_default();
                if sheets.len() < SHEETS {
                    sheets.insert(
                        header.style_sheet_id,
                        Sheet {
                            url: header.source_url,
                            inline: header.is_inline,
                            length: header.length,
                        },
                    );
                }
            }
        });
        measuring.watchers.push(watcher.abort_handle());
    }
    true
}

/// The calls that turn coverage on in a session of `kind`, sent at once.
fn steps(session: &Session, kind: TargetKind) -> Vec<(&'static str, Step)> {
    let start = profiler::StartPreciseCoverageParams {
        call_count: Some(false),
        detailed: Some(true),
        allow_triggered_updates: None,
    };
    let mut steps: Vec<(&'static str, Step)> = vec![
        (
            "Profiler.enable",
            step(session.call(profiler::EnableParams {})),
        ),
        ("Profiler.startPreciseCoverage", step(session.call(start))),
    ];
    if kind.is_host() {
        steps.push((
            "DOM.enable",
            step(session.call(dom::EnableParams::default())),
        ));
        steps.push(("CSS.enable", step(session.call(css::EnableParams {}))));
        steps.push((
            "CSS.startRuleUsageTracking",
            step(session.call(css::StartRuleUsageTrackingParams {})),
        ));
    }
    steps
}

/// A new target's steps: coverage on, if it runs for a tab the target
/// belongs to.
pub(super) fn setup_steps(
    shared: &Arc<Shared>,
    target: NewTarget<'_>,
    steps: &mut Vec<(&'static str, Step)>,
) {
    let mut state = shared.lock();
    let Some(tab) = state.coverage.as_ref().map(|c| c.tab) else {
        return;
    };
    let id = target.session.id().unwrap_or_default();
    let belongs = state
        .targets
        .get(id)
        .is_some_and(|t| measured(&state, t, tab));
    if belongs && register(&mut state, target.session, target.kind) {
        steps.extend(self::steps(target.session, target.kind));
    }
}

/// Turns the domains coverage turned on (in a page or iframe if `host`)
/// off again; `DOM` stays on if `keep_dom` (node stacks need it).
async fn off(session: &Session, host: bool, keep_dom: bool) {
    let stop = session.call(profiler::StopPreciseCoverageParams {});
    let disable = session.call(profiler::DisableParams {});
    let css = host.then(|| session.call(css::DisableParams {}));
    let dom = (host && !keep_dom).then(|| session.call(dom::DisableParams {}));
    let _ = stop.await;
    let _ = disable.await;
    if let Some(css) = css {
        let _ = css.await;
    }
    if let Some(dom) = dom {
        let _ = dom.await;
    }
}
