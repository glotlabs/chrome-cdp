//! `tabs`, `tab new [url]`, `tab <id>` and `tab close [id]`, and what
//! every page command shares: finding its tab (`--tab`) and reporting the
//! tabs it opened.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use protocol::schema::{DownloadInfo, Event, TabCloseData, TabData, TabInfo, TabsData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::json;

use super::{BLANK, Page, TabError, parse_tab_id, tab_name};
use crate::context::Scoped as _;
use crate::downloads;
use crate::errors::from_cdp;
use crate::frames::iframe_label;
use crate::interrupt::{describe, describe_kind};
use crate::places;
use crate::request::{
    Navigation, TabCloseRequest, TabNewRequest, TabSelectRequest, TabsRequest, WaitUntil, millis,
};
use crate::{Engine, Running};

/// How long a command waits for popups being set up (before it picks its
/// tab, and before it reports new tabs), and then for them to have a URL.
const POPUP_URL_WAIT: Duration = Duration::from_millis(500);

/// What `tab close` did; the daemon closes the session when asked to.
#[derive(Debug)]
pub struct TabClosed {
    pub output: Output,
    /// `--close-browser` on the last tab: close the session now.
    pub close_session: bool,
}

impl Engine {
    /// The tab a page command acts on: `tab` (`t2`) or the active one.
    pub(crate) async fn page(
        &self,
        running: &Running,
        tab: Option<&str>,
        timeout: Duration,
    ) -> Result<Page, ErrorInfo> {
        let id = tab.map(parse_tab_id).transpose()?;
        // A popup a click opened after its command returned is still
        // being set up: until then its opener, likely the tab to act on,
        // is hidden behind it.
        running.tabs.wait_for_popups(POPUP_URL_WAIT).await;
        let page = match self.current_context()? {
            Some(scope) => running.tabs.page_in(&scope.browser_id, id, timeout).await,
            None => running.tabs.page(id, timeout).await,
        }
        .map_err(|e| self.tab_error(running, e))?;
        running.tabs.begin_command(page.id);
        crate::hmr::note_since_previous(running, page.id);
        Ok(page)
    }

    pub(crate) fn tab_error(&self, running: &Running, err: TabError) -> ErrorInfo {
        match err {
            TabError::NoSuchTab(id) => {
                let open: Vec<String> = running
                    .tabs
                    .list_cached()
                    .into_iter()
                    .map(|t| t.id)
                    .collect();
                ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("no tab {} in session {}", tab_name(id), self.session),
                )
                .note(
                    "open tabs",
                    if open.is_empty() {
                        "none".to_owned()
                    } else {
                        open.join(", ")
                    },
                )
                .hint(Hint::run(Cmd::new("tabs").scoped(&self.session)).why("lists them"))
                .details(json!({ "tab": tab_name(id), "open": open }))
            }
            TabError::OtherContext(id) => {
                let context = self.current_context().ok().flatten().map(|s| s.name());
                let context = context.as_deref().unwrap_or("the default context");
                let tab = running.tabs.info(id);
                let theirs = tab
                    .as_ref()
                    .and_then(|t| t.context.clone())
                    .unwrap_or_else(|| "the default context".to_owned());
                let hint = match tab.as_ref().and_then(|t| t.context.clone()) {
                    Some(c) => Hint::run(
                        Cmd::new("tab")
                            .arg(tab_name(id))
                            .flag("context", c)
                            .scoped(&self.session),
                    ),
                    None => Hint::text(format!(
                        "leave out --context to use {}; {} lists the tabs of this context",
                        tab_name(id),
                        Cmd::new("tabs").scoped(&self.session)
                    )),
                };
                ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("tab {} is in {theirs}, not in {context}", tab_name(id)),
                )
                .hint(hint)
                .details(json!({ "tab": tab_name(id), "context": theirs }))
            }
            TabError::NotAttached => ErrorInfo::new(
                ErrorKind::ProtocolError,
                "Chrome opened a tab but never reported it",
            )
            .hint(Hint::run(Cmd::new("tabs").scoped(&self.session))),
            TabError::Cdp(e) => from_cdp(&e, &self.session),
        }
    }

    /// Adds what happened since `mark` to `output`, as events and as
    /// warnings so the text forms mention them: the tabs that opened
    /// (popups), dialogs answered without a command, and downloads that
    /// began (except `shown`, a download the output already is about).
    pub(crate) async fn report_events(
        &self,
        running: &Running,
        mark: u64,
        mut output: Output,
        shown: Option<&DownloadInfo>,
    ) -> Output {
        for event in self.events_since(running, mark, shown).await {
            output = output.warning(self.event_line(&event));
            output = output.event(event);
        }
        output
    }

    /// What [`Engine::report_events`] reports: the tabs that opened since
    /// `mark` (with their URLs, once they have one), dialogs answered
    /// without a command, and downloads that began (except `shown`).
    pub(crate) async fn events_since(
        &self,
        running: &Running,
        mark: u64,
        shown: Option<&DownloadInfo>,
    ) -> Vec<Event> {
        running.tabs.wait_for_popups(POPUP_URL_WAIT).await;
        let ids: Vec<u32> = running
            .tabs
            .opened_since(mark)
            .iter()
            .filter_map(|event| match event {
                Event::TabOpened { tab, .. } => parse_tab_id(tab).ok(),
                _ => None,
            })
            .collect();
        if !ids.is_empty() {
            // A popup has no URL until its first navigation commits.
            running.tabs.wait_for_urls(&ids, POPUP_URL_WAIT).await;
            let _ = running.tabs.refresh(Duration::from_secs(2)).await;
        }
        running
            .tabs
            .opened_since(mark)
            .into_iter()
            .chain(
                running
                    .tabs
                    .happened_since(mark)
                    .into_iter()
                    .filter(|event| match (event, shown) {
                        (Event::Download(d), Some(shown)) => {
                            d.url != shown.url || d.file != shown.file
                        }
                        _ => true,
                    }),
            )
            .collect()
    }

    /// The line that reports `event` (a warning, or a line of an
    /// action's report).
    pub(crate) fn event_line(&self, event: &Event) -> String {
        match event {
            Event::TabOpened {
                tab, url, active, ..
            } => {
                let url = if url.is_empty() { BLANK } else { url };
                if *active {
                    format!("opened tab {tab} ({url}) and switched to it")
                } else {
                    format!(
                        "opened tab {tab} ({url}); switch to it: {}",
                        Cmd::new("tab").arg(tab).scoped(&self.session)
                    )
                }
            }
            Event::Dialog {
                kind,
                message,
                accepted,
                by,
                ..
            } => {
                let how = match (*accepted, by.as_str()) {
                    (true, "policy") => "accepted",
                    (false, "policy") => "dismissed",
                    (true, _) => "accepted outside chrome-cdp dialog",
                    (false, _) => "dismissed by a navigation or outside chrome-cdp dialog",
                };
                format!("dialog: {} ({how})", describe_kind(kind, message))
            }
            Event::Download(download) => downloads::line(download, &self.session),
            Event::Navigated {
                url,
                title,
                status,
                same_document,
                restored,
                error,
                loaded,
                ..
            } => {
                if let Some(error) = error {
                    return format!("navigation to {url} failed: {error}");
                }
                let how = if *same_document {
                    Some("same document".to_owned())
                } else if *restored {
                    Some("from the back/forward cache".to_owned())
                } else if !loaded {
                    Some("still loading".to_owned())
                } else {
                    status.map(|s| s.to_string())
                };
                let how = how.map(|h| format!(" ({h})")).unwrap_or_default();
                format!("navigated to {} {title:?}{how}", shown_url(url))
            }
            Event::ConsoleError {
                message,
                url,
                line,
                worker,
                original,
                ..
            } => {
                let place = match original {
                    // The original place: the file on disk, relative to
                    // the project root, if found.
                    Some(o) => {
                        let file = match &o.file {
                            Some(file) => places::shown(Path::new(file), places::root().as_deref()),
                            None => file_name(&o.source).to_owned(),
                        };
                        format!(" ({file}:{})", o.line)
                    }
                    None => url
                        .as_deref()
                        .map(|url| {
                            let file = file_name(url);
                            line.map_or_else(|| format!(" ({file})"), |l| format!(" ({file}:{l})"))
                        })
                        .unwrap_or_default(),
                };
                let from = worker
                    .as_ref()
                    .map(|w| format!(" {}", w.label()))
                    .unwrap_or_default();
                format!("console error: {message}{place}{from}")
            }
            Event::RequestFailed {
                id,
                method,
                url,
                status,
                error,
                ..
            } => {
                let why = status
                    .map(|s| s.to_string())
                    .or_else(|| error.clone())
                    .unwrap_or_default();
                let id = id.map(|id| format!("#{id} ")).unwrap_or_default();
                format!("request failed: {id}{method} {} ({why})", path_of(url))
            }
            Event::Logpoint { hit, .. } => super::hit_text(hit),
            Event::HotUpdate { update, .. } => crate::hmr::line(update),
            Event::FocusMoved {
                tab,
                element,
                frame,
            } => {
                let place = frame
                    .as_deref()
                    .and_then(|frame| {
                        let tab = parse_tab_id(tab).ok()?;
                        let frames = self.running().ok()?.tabs.frames(tab);
                        let info = frames.iter().find(|f| f.id == frame)?;
                        Some(format!(" in {}", iframe_label(info)))
                    })
                    .unwrap_or_default();
                format!("focus moved to {}{place}", crate::input::describe(element))
            }
        }
    }

    /// `tabs`: every tab, the active one marked.
    pub async fn tabs(&self, req: TabsRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let scope = self.current_context()?;
        let mut tabs = running
            .tabs
            .list(millis(req.timeout_ms))
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        if let Some(scope) = &scope {
            let name = scope.name();
            tabs.retain(|t| t.context.as_deref() == Some(name.as_str()));
        }
        let text = if tabs.is_empty() {
            format!(
                "no tabs; open one: {}",
                Cmd::new("tab").arg("new").scoped(&self.session)
            )
        } else {
            let rows: Vec<Vec<String>> = tabs
                .iter()
                .map(|t| {
                    let mut title = format!("{:?}", t.title);
                    if let Some(opener) = &t.opener {
                        let _ = write!(title, " (opened by {opener})");
                    }
                    if t.crashed {
                        title.push_str(" (crashed)");
                    }
                    if t.dialog.is_some() {
                        title.push_str(" (dialog open)");
                    }
                    if scope.is_none()
                        && let Some(context) = &t.context
                    {
                        let _ = write!(title, " [{context}]");
                    }
                    vec![
                        format!("{} {}", if t.active { "*" } else { " " }, t.id),
                        shown_url(&t.url).to_owned(),
                        title,
                    ]
                })
                .collect();
            let mut text = format::table(&rows);
            // What blocks a tab, and how to unblock it, under the table
            // (its last column is cut to fit).
            for t in &tabs {
                let on_tab = |cmd: Cmd| {
                    let cmd = if t.active {
                        cmd
                    } else {
                        cmd.flag("tab", &t.id)
                    };
                    cmd.scoped(&self.session)
                };
                if t.crashed {
                    let _ = write!(text, "\n{} crashed: {}", t.id, on_tab(Cmd::new("reload")));
                }
                if let Some(dialog) = &t.dialog {
                    let _ = write!(
                        text,
                        "\n{} has a dialog open: {}; {}, or {}",
                        t.id,
                        describe(dialog),
                        on_tab(Cmd::new("dialog").arg("accept")),
                        on_tab(Cmd::new("dialog").arg("dismiss"))
                    );
                }
            }
            text
        };
        let quiet: Vec<&str> = tabs.iter().map(|t| t.id.as_str()).collect();
        let quiet = quiet.join("\n");
        Ok(Output::typed(&TabsData { tabs }, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it"))
    }

    /// `tab new [url]`: opens a tab, makes it active and loads `url`.
    pub async fn tab_new(&self, req: TabNewRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let mark = running.tabs.mark();
        let scope = self.current_context()?;
        let browser_id = scope.as_ref().map(|s| s.browser_id.as_str());
        let page = running
            .tabs
            .create_in(BLANK, browser_id, timeout)
            .await
            .map_err(|e| self.tab_error(&running, e))?;
        match browser_id {
            Some(browser_id) => running.tabs.activate_in(browser_id, page.id),
            None => {
                running
                    .tabs
                    .activate(page.id, timeout)
                    .await
                    .map_err(|e| self.tab_error(&running, e))?;
            }
        }
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
        let tab = self.tab_info(&running, page.id).await;
        let text = format!("opened tab {}", describe_tab(&tab));
        let output = warnings.into_iter().fold(
            Output::typed(&TabData { tab: tab.clone() }, text).quiet(tab.id),
            Output::warning,
        );
        Ok(self.report_events(&running, mark, output, None).await)
    }

    /// `tab <id>`: makes a tab active (and brings it to the front).
    pub async fn tab_select(&self, req: TabSelectRequest) -> Result<Output, ErrorInfo> {
        let id = parse_tab_id(&req.tab)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        match self.current_context()? {
            Some(scope) => {
                running
                    .tabs
                    .page_in(&scope.browser_id, Some(id), timeout)
                    .await
                    .map_err(|e| self.tab_error(&running, e))?;
                running.tabs.activate_in(&scope.browser_id, id);
                running
                    .tabs
                    .bring_to_front(id, timeout)
                    .await
                    .map_err(|e| self.tab_error(&running, e))?;
            }
            None => {
                running
                    .tabs
                    .activate(id, timeout)
                    .await
                    .map_err(|e| self.tab_error(&running, e))?;
            }
        }
        let tab = self.tab_info(&running, id).await;
        let text = format!("switched to tab {}", describe_tab(&tab));
        Ok(Output::typed(&TabData { tab: tab.clone() }, text).quiet(tab.id))
    }

    /// `tab close [id]`: closes a tab. The last one is replaced by a blank
    /// tab, so the session keeps its browser, unless `close_browser` asks
    /// to close the session.
    pub async fn tab_close(&self, req: TabCloseRequest) -> Result<TabClosed, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        let closed = tab_name(page.id);
        let scope = self.current_context()?;
        // A context of ours may be left without tabs; the browser keeps
        // the default context's.
        let last = scope.is_none() && running.tabs.count() <= 1;
        if last && req.close_browser {
            let data = TabCloseData {
                closed: closed.clone(),
                active: None,
                opened_blank: false,
                session_closed: true,
            };
            let text = format!("closed tab {closed} and session {}", self.session);
            return Ok(TabClosed {
                output: Output::typed(&data, text).quiet(""),
                close_session: true,
            });
        }
        if last {
            // Open the blank tab first: a headed Chrome quits with its last
            // window.
            let blank = running
                .tabs
                .create(BLANK, timeout)
                .await
                .map_err(|e| self.tab_error(&running, e))?;
            running
                .tabs
                .activate(blank.id, timeout)
                .await
                .map_err(|e| self.tab_error(&running, e))?;
        }
        {
            let _tab = self.tabs.lock(&page.target_id).await;
            running
                .tabs
                .close(page.id, timeout)
                .await
                .map_err(|e| self.tab_error(&running, e))?;
        }
        let active = match scope.as_ref().map_or_else(
            || running.tabs.active(),
            |s| running.tabs.active_in(&s.browser_id),
        ) {
            Some(id) => {
                // Chrome shows the closed tab's neighbour, which may not be
                // the one active before it.
                if let Err(e) = running.tabs.bring_to_front(id, timeout).await {
                    tracing::debug!("bringing tab {id} to the front failed: {e:?}");
                }
                Some(self.tab_info(&running, id).await)
            }
            None => None,
        };
        let mut text = format!("closed tab {closed}");
        if last {
            let _ = write!(
                text,
                "; opened a blank tab so the session stays ({} closes the browser)",
                Cmd::new("tab")
                    .arg("close")
                    .switch("close-browser")
                    .scoped(&self.session)
            );
        }
        if let Some(active) = &active {
            let _ = write!(text, "\nactive tab: {}", describe_tab(active));
        }
        let quiet = active.as_ref().map(|t| t.id.clone()).unwrap_or_default();
        let data = TabCloseData {
            closed,
            active,
            opened_blank: last,
            session_closed: false,
        };
        Ok(TabClosed {
            output: Output::typed(&data, text).quiet(quiet),
            close_session: false,
        })
    }

    /// Tab `id` with a fresh URL and title.
    async fn tab_info(&self, running: &Running, id: u32) -> TabInfo {
        let _ = running.tabs.refresh(Duration::from_secs(2)).await;
        running.tabs.info(id).unwrap_or_else(|| TabInfo {
            id: tab_name(id),
            active: false,
            title: String::new(),
            url: String::new(),
            opener: None,
            crashed: false,
            dialog: None,
            context: None,
        })
    }
}

/// `t2 https://example.com/ "Example Domain"`.
fn describe_tab(tab: &TabInfo) -> String {
    format!("{} {} {:?}", tab.id, shown_url(&tab.url), tab.title)
}

/// The last part of a script's URL, `app.js` (the whole URL if it has
/// none).
fn file_name(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit('/').next() {
        Some(name) if !name.is_empty() && path.contains("://") => name,
        _ => url,
    }
}

/// A URL without its origin, `/api/save?x=1` (the whole URL if it has
/// none).
pub(crate) fn path_of(url: &str) -> &str {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url;
    };
    match rest.find(['/', '?', '#']) {
        Some(at) => &url[scheme.len() + 3 + at..],
        None => url,
    }
}

/// A new tab's URL is empty until it commits a navigation.
pub(crate) fn shown_url(url: &str) -> &str {
    if url.is_empty() { BLANK } else { url }
}

#[cfg(test)]
mod tests {
    use super::{file_name, path_of};

    #[test]
    fn urls_are_shortened_for_report_lines() {
        assert_eq!(file_name("http://x/js/app.js?v=2"), "app.js");
        assert_eq!(file_name("http://x/"), "http://x/");
        assert_eq!(file_name("debugger://anon"), "anon");
        assert_eq!(path_of("http://x:8080/api/save?id=1"), "/api/save?id=1");
        assert_eq!(path_of("http://x:8080"), "http://x:8080");
        assert_eq!(path_of("data:text/plain,hi"), "data:text/plain,hi");
    }
}
