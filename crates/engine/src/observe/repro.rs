//! `repro start` (plan 18, milestone 18.5): marks where a repro bundle
//! starts, for `repro save`.
//!
//! The start keeps the context's cookies and storage (what `state save`
//! writes), the time, the page's viewport, locale and time zone, and the
//! history position the CLI read; it notes where each tab's current
//! document starts in its network log, and from then on keeps the
//! response bodies of the requests in scope as they finish (see
//! [`super::kept`]), since Chrome drops them once their page goes. The
//! bodies of the documents open at the start are asked for right away,
//! while Chrome still has them. `repro save` reads it all back with
//! [`Engine::repro_info`] and exports the requests with `har export`'s
//! `repro` flag.

use std::collections::HashMap;
use std::time::Duration;

use cdp::domains::network as cdp_network;
use protocol::schema::{ReproStartData, RequestState};
use protocol::{Cmd, ErrorInfo, ErrorKind, Output};
use serde_json::{Value, json};

use super::kept::KeptBody;
use super::netlog::Record;
use crate::context::Scoped as _;
use crate::frames::World;
use crate::request::{ReproStartRequest, millis};
use crate::state::StorageState;
use crate::{Engine, Running};

/// What a request scope of none means: every request of the tabs.
pub const EVERYTHING: &str = "/**";

/// A running `repro start`.
#[derive(Debug, Clone)]
pub(crate) struct ReproRecording {
    /// When it started (ms since the epoch).
    started_ms: u64,
    scope: Vec<String>,
    /// The first history number after the start.
    history_from: u64,
    /// The context's cookies and storage at the start.
    state: StorageState,
    /// The active tab's URL at the start.
    url: Option<String>,
    /// The active page's `innerWidth`, `innerHeight`, `devicePixelRatio`,
    /// `locale` and `timezone` at the start.
    page: Value,
    /// Each tab's last request before its current document; tabs not here
    /// opened later and count from their first.
    pub(crate) marks: HashMap<u32, u64>,
}

/// Reads what the page emulates, for the bundle's config.
const PAGE: &str = "({width: innerWidth, height: innerHeight, scale: devicePixelRatio, \
                    locale: navigator.language, \
                    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone})";

impl Engine {
    /// `repro start`.
    pub async fn repro_start(&self, req: ReproStartRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let scope = if req.scope.is_empty() {
            vec![EVERYTHING.to_owned()]
        } else {
            req.scope.clone()
        };
        let globs =
            mock::record::Scope::new(&scope).map_err(|e| ErrorInfo::new(ErrorKind::BadArgs, e))?;
        let (state, warnings) = self.collect_state(&running, false, timeout).await?;
        let page = self.page(&running, None, timeout).await.ok();
        let url = page
            .as_ref()
            .and_then(|p| running.tabs.info(p.id))
            .map(|info| info.url)
            .filter(|url| url.starts_with("http"));
        let emulated = self
            .evaluate(None, None, World::Isolated, PAGE, timeout)
            .await
            .unwrap_or(Value::Null);
        let context = self.context_key()?;
        let tabs = running.tabs.tab_ids_in(context.as_deref());
        let marks: HashMap<u32, u64> = tabs
            .iter()
            .filter_map(|&id| {
                let mark = running.tabs.netlog(id, |log| {
                    log.records()
                        .rev()
                        .find(|r| r.navigation.is_some())
                        .map_or(log.seq(), |r| r.entry.id.saturating_sub(1))
                })?;
                Some((id, mark))
            })
            .collect();
        running.tabs.keep_bodies(globs.clone());
        let kept = self
            .keep_open_bodies(&running, &marks, &globs, timeout)
            .await;
        let already = lock(&running.repro).is_some();
        let recording = ReproRecording {
            started_ms: now_ms(),
            scope: scope.clone(),
            history_from: req.history_from,
            state,
            url,
            page: emulated,
            marks,
        };
        let started = super::iso8601(recording.started_ms);
        let data = ReproStartData {
            started: started.clone(),
            scope: scope.clone(),
            cookies: recording.state.cookies.len(),
            origins: recording.state.origins.len(),
            url: recording.url.clone(),
            bodies: kept,
            restarted: already,
        };
        *lock(&running.repro) = Some(recording);
        let save = Cmd::new("repro save").arg("repro").scoped(&self.session);
        let what = if scope == [EVERYTHING] {
            "every request".to_owned()
        } else {
            scope.join(", ")
        };
        let text = format!(
            "{} a repro at {started}: kept {} {} and storage of {} {}; keeping the bodies of {what}\nnext: reproduce the bug, then {save}",
            if already { "restarted" } else { "started" },
            data.cookies,
            if data.cookies == 1 {
                "cookie"
            } else {
                "cookies"
            },
            data.origins,
            if data.origins == 1 {
                "origin"
            } else {
                "origins"
            },
        );
        let mut output = Output::typed(&data, text).quiet(started);
        if already {
            output =
                output.warning("the earlier repro start was dropped; this one starts from now");
        }
        Ok(warnings.into_iter().fold(output, Output::warning))
    }

    /// What `repro start` kept, for `repro save`: `{"recording": null}`
    /// without one.
    pub fn repro_info(&self) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let recording = lock(&running.repro).clone();
        let (kept, over) = running.tabs.kept(|k| (k.len(), k.over)).unwrap_or_default();
        let data = recording.map_or(Value::Null, |r| {
            json!({
                "started_ms": r.started_ms,
                "started": super::iso8601(r.started_ms),
                "scope": r.scope,
                "history_from": r.history_from,
                "state": r.state,
                "url": r.url,
                "page": r.page,
                "bodies": kept,
                "bodies_over": over,
            })
        });
        Ok(Output::new(json!({ "recording": data }), String::new()))
    }

    /// Keeps the bodies of the requests in scope of the documents open at
    /// the start, which Chrome has now but drops once their page goes;
    /// returns how many it kept.
    async fn keep_open_bodies(
        &self,
        running: &Running,
        marks: &HashMap<u32, u64>,
        scope: &mock::record::Scope,
        timeout: Duration,
    ) -> usize {
        let mut records: Vec<Record> = Vec::new();
        for (&id, &mark) in marks {
            running.tabs.netlog(id, |log| {
                records.extend(
                    log.records()
                        .filter(|r| {
                            r.entry.id > mark
                                && r.entry.state == RequestState::Done
                                && r.entry.redirect.is_none()
                                && r.entry.websocket.is_none()
                                && scope.matches(&r.entry.url, &r.entry.kind)
                        })
                        .cloned(),
                );
            });
        }
        let mut kept = 0;
        for record in records {
            let Some(session) = running.tabs.host_session(&record.session) else {
                continue;
            };
            let params = cdp_network::GetResponseBodyParams::new(record.request_id.clone());
            if let Ok(body) = session.with_timeout(timeout).call(params).await {
                running.tabs.kept(|k| {
                    k.keep(
                        &record.session,
                        &record.request_id,
                        KeptBody {
                            body: body.body,
                            base64: body.base64_encoded,
                        },
                    );
                });
                kept += 1;
            }
        }
        kept
    }
}

/// The marks of `running`'s repro recording, if one runs.
pub(crate) fn marks(running: &Running) -> Option<HashMap<u32, u64>> {
    lock(&running.repro).as_ref().map(|r| r.marks.clone())
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
