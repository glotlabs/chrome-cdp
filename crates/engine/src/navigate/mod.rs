//! Navigation (plan 03, milestone 3.3): `goto`, `back`, `forward` and
//! `reload`, and the page load that `open <url>` and `tab new <url>` share
//! with `goto`.
//!
//! Every navigation follows the tab's main frame through its events (see
//! [`watch`]) until `--wait` holds: by default the load event and then the
//! wait engine's settle (network idle and a quiet DOM, a few seconds at
//! most), or `load`, `domcontentloaded`, `networkidle` (the wait engine's
//! network idle after `DOMContentLoaded`), `commit` or `none`. A failed
//! load is a `NavigationFailed` error whose `details.reason` says why (see
//! [`failure`]); an HTTP 4xx/5xx is only a warning, and so is a page that
//! does not settle.
//!
//! A navigation returns at once with `DialogOpen` when the page opens a
//! dialog that waits for an answer (a `confirm` while it loads), or
//! `PageCrashed` (see [`crate::interrupt`]); a dialog already open is
//! dismissed by Chrome, and a crashed page loads again. `goto` to a URL
//! the browser downloads waits for the file instead.

mod failure;
mod url;
pub(crate) mod watch;

use std::time::Duration;

use cdp::CdpError;
use cdp::domains::{page, runtime};
use protocol::schema::{NavigateData, PageInfo};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Value, json};
use tokio::time::Instant;

pub use self::url::normalize_url;
use self::watch::Watch;
use crate::context::Scoped as _;
use crate::downloads;
use crate::errors::from_cdp;
use crate::network::InFlight;
use crate::request::{NavigateRequest, Navigation, WaitUntil, millis};
use crate::tabs::{BLANK, Page, tab_name};
use crate::wait::{NAVIGATION_SETTLE, Unmet, busy_unmet};
use crate::{Engine, Running};

/// How long describing the page may take in an error report.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a failed navigation waits for Chrome's error page.
const ERROR_PAGE_WAIT: Duration = Duration::from_secs(2);

/// How long `goto` to a download waits for the download to begin.
const DOWNLOAD_START_WAIT: Duration = Duration::from_secs(5);

/// A navigation that ended well: what `goto` and the like report, and
/// the warnings (an HTTP error status).
#[derive(Debug, Clone)]
pub(crate) struct Navigated {
    pub(crate) data: NavigateData,
    pub(crate) warnings: Vec<String>,
}

impl Navigated {
    /// The page, as `open` reports it.
    pub(crate) fn page_info(&self) -> PageInfo {
        PageInfo {
            url: self.data.url.clone(),
            title: self.data.title.clone(),
            status: self.data.status,
        }
    }

    /// The command's output: `→ 200 https://x/ "Title" (412ms)`.
    fn output(self, navigation: &Navigation, session: &str) -> Output {
        let data = &self.data;
        if let Some(download) = &data.download {
            let text = downloads::line(download, session);
            let quiet = download.path.clone().unwrap_or_default();
            return self.warnings.into_iter().fold(
                Output::typed(&self.data, text).quiet(quiet),
                Output::warning,
            );
        }
        let text = if data.navigated {
            arrow(data)
        } else {
            let (which, command) = match navigation {
                Navigation::Forward => ("later", "forward"),
                _ => ("earlier", "back"),
            };
            format!(
                "no {which} page in tab {}'s history to go {command} to; still at {} {:?}",
                data.tab, data.url, data.title
            )
        };
        let quiet = data.url.clone();
        self.warnings.into_iter().fold(
            Output::typed(&self.data, text).quiet(quiet),
            Output::warning,
        )
    }
}

/// `→ 200 https://final.url/ "Title" (412ms)`.
fn arrow(data: &NavigateData) -> String {
    let status = data.status.map(|s| format!("{s} ")).unwrap_or_default();
    let elapsed = format::duration(Duration::from_millis(data.elapsed_ms));
    let how = if data.same_document {
        format!("same document, {elapsed}")
    } else if data.restored {
        format!("from the back/forward cache, {elapsed}")
    } else {
        elapsed
    };
    format!("→ {status}{} {:?} ({how})", data.url, data.title)
}

impl Engine {
    /// `goto`, `back`, `forward` and `reload`.
    pub async fn navigate(&self, req: NavigateRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        let mark = running.tabs.mark();
        let navigated = {
            let _tab = self.tabs.lock(&page.target_id).await;
            self.navigate_page(&running, &page, &req.navigation, req.wait, timeout)
                .await?
        };
        let download = navigated.data.download.clone();
        let output = navigated.output(&req.navigation, &self.session);
        Ok(self
            .report_events(&running, mark, output, download.as_ref())
            .await)
    }

    /// Runs `navigation` in `page` and waits (up to `timeout` in all) for
    /// `wait`, unless the page opens a dialog or crashes first. The caller
    /// holds the tab's lock.
    pub(crate) async fn navigate_page(
        &self,
        running: &Running,
        page: &Page,
        navigation: &Navigation,
        wait: WaitUntil,
        timeout: Duration,
    ) -> Result<Navigated, ErrorInfo> {
        let since = running.tabs.mark();
        let navigate = self.navigate_unguarded(running, page, navigation, wait, timeout, since);
        tokio::select! {
            biased;
            result = self.unless_blocked(running, page, since, navigate) => result,
            () = running.tabs.stayed(page.id, since) => Err(self.stayed_error(running, page)),
        }
    }

    /// `NavigationFailed`: the page asked before it is left, and the
    /// session's `--dialogs` policy said stay.
    fn stayed_error(&self, running: &Running, page: &Page) -> ErrorInfo {
        let url = running
            .tabs
            .info(page.id)
            .map(|t| t.url)
            .unwrap_or_default();
        ErrorInfo::new(
            ErrorKind::NavigationFailed,
            "the page asks before it is left (beforeunload), and the session dismisses \
             dialogs, so the tab stayed",
        )
        .note("page", url.clone())
        .hint(
            Hint::run(
                Cmd::new("open")
                    .flag("dialogs", "auto")
                    .session(&self.session),
            )
            .why("accepts beforeunload; then navigate again"),
        )
        .details(json!({ "reason": "beforeunload", "url": url }))
    }

    /// [`Engine::navigate_page`] without the guard; `since` is the event
    /// mark from before it began.
    async fn navigate_unguarded(
        &self,
        running: &Running,
        page: &Page,
        navigation: &Navigation,
        wait: WaitUntil,
        timeout: Duration,
        since: u64,
    ) -> Result<Navigated, ErrorInfo> {
        let start = Instant::now();
        let deadline = start + timeout;
        let session = page.session.with_timeout(timeout);
        let cdp_err = |e: CdpError| from_cdp(&e, &self.session);
        // The main frame's id is the tab's target id.
        let mut watch = Watch::new(&page.session, &page.target_id);
        let mut requested = None;
        match navigation {
            Navigation::Goto { url, base_url } => {
                // On Chrome's error page, the frame's URL is the one that
                // failed (see `FrameTree::navigated`).
                let current = running
                    .tabs
                    .frames(page.id)
                    .into_iter()
                    .next()
                    .map(|f| f.url)
                    .filter(|url| !url.is_empty())
                    .unwrap_or_else(|| BLANK.to_owned());
                let url = url::resolve(
                    &normalize_url(url)?,
                    base_url.as_deref(),
                    &current,
                    &self.session,
                )?;
                requested = Some(url.clone());
                running
                    .tabs
                    .node_stacks_for(&page.session, &url, timeout)
                    .await;
                let nav = match session.call(page::NavigateParams::new(url.clone())).await {
                    Ok(nav) => nav,
                    Err(CdpError::Timeout { .. }) => {
                        return Err(self.no_answer(page, navigation, &url, timeout).await);
                    }
                    Err(e) => return Err(cdp_err(e)),
                };
                if let Some(error) = nav.error_text.filter(|e| !e.is_empty()) {
                    let download = nav.is_download.unwrap_or(false);
                    if download
                        && running.tabs.downloads_dir().is_some()
                        && running
                            .tabs
                            .download_began(since, DOWNLOAD_START_WAIT)
                            .await
                    {
                        return self
                            .downloaded(running, page, &url, since, start, deadline, timeout)
                            .await;
                    }
                    // Chrome still commits its error page; let it, so the
                    // next command does not race it.
                    if let Some(loader) = &nav.loader_id {
                        watch.expect(loader);
                        let settle = deadline.min(Instant::now() + ERROR_PAGE_WAIT);
                        watch.until(WaitUntil::Commit, settle).await;
                    }
                    return Err(failure::navigation_failed(
                        &url,
                        &error,
                        download,
                        &self.session,
                    ));
                }
                match &nav.loader_id {
                    Some(loader) => watch.expect(loader),
                    // `#fragment`: no new document.
                    None => watch.stayed_in_document(),
                }
            }
            Navigation::Back | Navigation::Forward => {
                let history = session
                    .call(page::GetNavigationHistoryParams::default())
                    .await
                    .map_err(cdp_err)?;
                let step = if matches!(navigation, Navigation::Back) {
                    -1
                } else {
                    1
                };
                let entry = usize::try_from(history.current_index + step)
                    .ok()
                    .and_then(|i| history.entries.get(i));
                let Some(entry) = entry else {
                    return self.stayed(page, wait, start).await;
                };
                session
                    .call(page::NavigateToHistoryEntryParams::new(entry.id))
                    .await
                    .map_err(cdp_err)?;
            }
            Navigation::Reload { hard } => {
                watch.new_document();
                let params = page::ReloadParams {
                    ignore_cache: Some(*hard),
                    ..page::ReloadParams::default()
                };
                session.call(params).await.map_err(cdp_err)?;
            }
        }

        if !watch.until(wait, deadline).await {
            return Err(self
                .wait_timeout(
                    page,
                    navigation,
                    requested.as_deref(),
                    wait,
                    &watch,
                    timeout,
                    &[],
                )
                .await);
        }
        if let Some(unreachable) = &watch.unreachable {
            // A reload or history step that failed shows Chrome's error
            // page; `goto` already failed on `errorText`.
            let error = error_code(&session).await;
            return Err(failure::navigation_failed(
                unreachable,
                &error,
                false,
                &self.session,
            ));
        }
        let mut warnings = Vec::new();
        match wait {
            WaitUntil::Settle => {
                let limit = deadline.min(Instant::now() + NAVIGATION_SETTLE);
                if let Err(unmet) = self.settle(running, page, start, limit).await? {
                    warnings.push(unsettled(&unmet));
                }
            }
            WaitUntil::NetworkIdle => {
                if let Err(busy) = running.tabs.network_idle(page.id, start, deadline).await {
                    return Err(self
                        .wait_timeout(
                            page,
                            navigation,
                            requested.as_deref(),
                            wait,
                            &watch,
                            timeout,
                            &busy,
                        )
                        .await);
                }
            }
            _ => {}
        }
        let elapsed = start.elapsed();
        let shown = describe(&session).await.map_err(cdp_err)?;
        // Chrome reports 200 for `data:` pages too; only HTTP has one.
        let http = shown.url.starts_with("http://") || shown.url.starts_with("https://");
        let status = (http && !watch.same_document)
            .then_some(shown.status)
            .flatten();
        if let Some(status) = status.filter(|s| *s >= 400) {
            warnings.push(format!(
                "the server answered HTTP {status}{} for {}",
                reason_phrase(status)
                    .map(|r| format!(" {r}"))
                    .unwrap_or_default(),
                shown.url
            ));
        }
        Ok(Navigated {
            data: NavigateData {
                tab: tab_name(page.id),
                requested_url: requested,
                url: shown.url,
                title: shown.title,
                status,
                navigated: true,
                same_document: watch.same_document,
                restored: watch.restored,
                wait: wait.as_str().to_owned(),
                elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                download: None,
            },
            warnings,
        })
    }

    /// `goto <url>` started a download (after event mark `since`): waits
    /// until it finished. The page stays as it was.
    #[allow(clippy::too_many_arguments)]
    async fn downloaded(
        &self,
        running: &Running,
        page: &Page,
        url: &str,
        since: u64,
        start: Instant,
        deadline: Instant,
        timeout: Duration,
    ) -> Result<Navigated, ErrorInfo> {
        let download = match running.tabs.download_done(since, None, deadline).await {
            Ok(download) => download,
            Err(partial) => {
                let mut info = ErrorInfo::new(
                    ErrorKind::Timeout,
                    format!(
                        "the download of {url} did not finish within {}",
                        format::duration(timeout)
                    ),
                );
                if let Some(partial) = &partial {
                    info = info.note("downloading", downloads::line(partial, &self.session));
                }
                return Err(info
                    .hint(
                        Hint::run(Cmd::new("wait").switch("download").scoped(&self.session))
                            .why("it goes on in the background"),
                    )
                    .details(json!({ "url": url, "download": partial })));
            }
        };
        let mut warnings = Vec::new();
        if download.state == "canceled" {
            warnings.push(format!("the browser canceled the download of {url}"));
        }
        let shown = describe(&page.session.with_timeout(DESCRIBE_TIMEOUT))
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        Ok(Navigated {
            data: NavigateData {
                tab: tab_name(page.id),
                requested_url: Some(url.to_owned()),
                url: shown.url,
                title: shown.title,
                status: None,
                navigated: false,
                same_document: false,
                restored: false,
                wait: "download".to_owned(),
                elapsed_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                download: Some(download),
            },
            warnings,
        })
    }

    /// `back` or `forward` with no entry to go to: the page as it is.
    async fn stayed(
        &self,
        page: &Page,
        wait: WaitUntil,
        start: Instant,
    ) -> Result<Navigated, ErrorInfo> {
        let shown = describe(&page.session.with_timeout(DESCRIBE_TIMEOUT))
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        Ok(Navigated {
            data: NavigateData {
                tab: tab_name(page.id),
                requested_url: None,
                url: shown.url,
                title: shown.title,
                status: None,
                navigated: false,
                same_document: false,
                restored: false,
                wait: wait.as_str().to_owned(),
                elapsed_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                download: None,
            },
            warnings: Vec::new(),
        })
    }

    /// `Page.navigate` got no answer: the server did not respond in time.
    async fn no_answer(
        &self,
        page: &Page,
        navigation: &Navigation,
        url: &str,
        timeout: Duration,
    ) -> ErrorInfo {
        let observed = describe(&page.session.with_timeout(DESCRIBE_TIMEOUT))
            .await
            .ok();
        let mut info = ErrorInfo::new(
            ErrorKind::Timeout,
            format!("{url} did not answer within {}", format::duration(timeout)),
        )
        .hint(
            Hint::run(
                retry(navigation, None)
                    .flag("timeout", format::duration(timeout * 2))
                    .scoped(&self.session),
            )
            .why("if the server is just slow"),
        );
        let mut details = json!({ "url": url, "reached": [] });
        if let Some(observed) = observed {
            info = info.note("page", format!("{} {:?}", observed.url, observed.title));
            details["page"] = json!({ "url": observed.url, "title": observed.title });
        }
        let info = self.note_unread_errors(page.id, info, &mut details);
        info.details(details)
    }

    /// The navigation started but `wait` did not hold in time; `busy` are
    /// the requests that kept the network from going idle.
    #[allow(clippy::too_many_arguments)]
    async fn wait_timeout(
        &self,
        page: &Page,
        navigation: &Navigation,
        requested: Option<&str>,
        wait: WaitUntil,
        watch: &Watch,
        timeout: Duration,
        busy: &[InFlight],
    ) -> ErrorInfo {
        let observed = describe(&page.session.with_timeout(DESCRIBE_TIMEOUT))
            .await
            .ok();
        let reached = watch.seen();
        let what = match (navigation, requested) {
            (_, Some(url)) => url.to_owned(),
            (Navigation::Back, _) => "going back".to_owned(),
            (Navigation::Forward, _) => "going forward".to_owned(),
            _ => "the reload".to_owned(),
        };
        let message = if watch.committed() {
            format!(
                "{what} did not reach {} within {}",
                milestone(wait),
                format::duration(timeout)
            )
        } else {
            format!(
                "{what} did not start a new page within {}",
                format::duration(timeout)
            )
        };
        let longer = format::duration(timeout * 2);
        let again = |flag: &str, value: &str| {
            retry(navigation, requested)
                .flag(flag, value)
                .scoped(&self.session)
        };
        let hint = match wait {
            WaitUntil::Load | WaitUntil::Settle if watch.committed() => {
                Hint::run(again("wait", "domcontentloaded"))
                    .why("does not wait for images, scripts and styles")
                    .or(format!("a longer --timeout, e.g. --timeout {longer}"))
            }
            WaitUntil::NetworkIdle if watch.committed() => Hint::run(again("wait", "load"))
                .why("the page keeps the network busy")
                .or(format!("a longer --timeout, e.g. --timeout {longer}")),
            _ => Hint::run(again("timeout", &longer)),
        };
        let mut info = ErrorInfo::new(ErrorKind::Timeout, message)
            .note(
                "reached",
                if reached.is_empty() {
                    "nothing yet".to_owned()
                } else {
                    reached.join(", ")
                },
            )
            .hint(hint);
        let mut details = json!({ "wait": wait.as_str(), "reached": reached });
        if let Some(url) = requested {
            details["url"] = json!(url);
        }
        if wait == WaitUntil::NetworkIdle && watch.committed() {
            let unmet = busy_unmet(busy);
            for (label, text) in unmet.notes {
                info = info.note(label, text);
            }
            details["observed"] = json!(unmet.observed);
        }
        if let Some(observed) = observed {
            info = info.note("page", format!("{} {:?}", observed.url, observed.title));
            details["page"] = json!({ "url": observed.url, "title": observed.title });
        }
        let info = self.note_unread_errors(page.id, info, &mut details);
        info.details(details)
    }
}

/// The command that runs `navigation` again, e.g. `goto <url>`.
fn retry(navigation: &Navigation, url: Option<&str>) -> Cmd {
    match navigation {
        Navigation::Goto { url: typed, .. } => Cmd::new("goto").arg(url.unwrap_or(typed)),
        Navigation::Back => Cmd::new("back"),
        Navigation::Forward => Cmd::new("forward"),
        Navigation::Reload { hard: true } => Cmd::new("reload").switch("hard"),
        Navigation::Reload { hard: false } => Cmd::new("reload"),
    }
}

/// The warning of a navigation whose page did not settle in time.
fn unsettled(unmet: &Unmet) -> String {
    let busy: Vec<&str> = unmet
        .notes
        .iter()
        .filter(|(label, _)| *label == "in flight")
        .map(|(_, text)| text.as_str())
        .collect();
    let why = if busy.is_empty() {
        "requests kept starting".to_owned()
    } else {
        format!("in flight: {}", busy.join(", "))
    };
    format!(
        "the page was still loading {} after the load event ({why}); \
         chrome-cdp wait --settle waits for it",
        format::duration(NAVIGATION_SETTLE)
    )
}

/// What `wait` waits for, in words.
fn milestone(wait: WaitUntil) -> &'static str {
    match wait {
        WaitUntil::Load | WaitUntil::Settle => "the load event",
        WaitUntil::DomContentLoaded => "DOMContentLoaded",
        WaitUntil::NetworkIdle => "network idle",
        WaitUntil::Commit | WaitUntil::None => "a new document",
    }
}

/// The page as it is now.
#[derive(Debug)]
pub(crate) struct Shown {
    pub(crate) url: String,
    pub(crate) title: String,
    /// The main document's HTTP status, if it has one.
    pub(crate) status: Option<u16>,
}

/// The page's URL, title and HTTP status (from the Navigation Timing
/// entry, so no `Network` events are needed).
pub(crate) async fn describe(page: &cdp::Session) -> cdp::Result<Shown> {
    let mut params = runtime::EvaluateParams::new(
        "(() => { const n = performance.getEntriesByType('navigation')[0]; \
         return [location.href, document.title, n ? n.responseStatus : 0]; })()"
            .into(),
    );
    params.return_by_value = Some(true);
    let result = page.call(params).await?.result.value.unwrap_or(Value::Null);
    let field = |i: usize| result[i].as_str().unwrap_or_default().to_owned();
    Ok(Shown {
        url: field(0),
        title: field(1),
        status: result[2]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .filter(|s| *s > 0),
    })
}

/// The `net::ERR_*` code of the error page the tab shows.
pub(crate) async fn error_code(page: &cdp::Session) -> String {
    let mut params = runtime::EvaluateParams::new(
        "typeof loadTimeDataRaw === 'object' && loadTimeDataRaw.errorCode || ''".into(),
    );
    params.return_by_value = Some(true);
    let code = match page.with_timeout(DESCRIBE_TIMEOUT).call(params).await {
        Ok(result) => result
            .result
            .value
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    if code.is_empty() {
        "net::ERR_FAILED".to_owned()
    } else if code.starts_with("net::") {
        code
    } else {
        format!("net::{code}")
    }
}

/// The reason phrase of common error statuses.
fn reason_phrase(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use protocol::schema::NavigateData;

    use super::arrow;

    fn data() -> NavigateData {
        NavigateData {
            tab: "t1".into(),
            requested_url: Some("http://localhost:3000/".into()),
            url: "http://localhost:3000/home".into(),
            title: "Home".into(),
            status: Some(200),
            navigated: true,
            same_document: false,
            restored: false,
            wait: "load".into(),
            elapsed_ms: 412,
            download: None,
        }
    }

    #[test]
    fn the_arrow_line() {
        assert_eq!(
            arrow(&data()),
            "→ 200 http://localhost:3000/home \"Home\" (412ms)"
        );
        let same = NavigateData {
            status: None,
            same_document: true,
            elapsed_ms: 3,
            ..data()
        };
        assert_eq!(
            arrow(&same),
            "→ http://localhost:3000/home \"Home\" (same document, 3ms)"
        );
        let restored = NavigateData {
            restored: true,
            elapsed_ms: 1500,
            ..data()
        };
        assert_eq!(
            arrow(&restored),
            "→ 200 http://localhost:3000/home \"Home\" (from the back/forward cache, 1.5s)"
        );
    }
}
