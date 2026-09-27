//! `tabs`, `tab new [url]`, `tab <id>` and `tab close [id]`, and what
//! every page command shares: finding its tab (`--tab`) and reporting the
//! tabs it opened.

use std::fmt::Write as _;
use std::time::Duration;

use protocol::schema::{TabCloseData, TabData, TabInfo, TabsData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::json;

use super::{BLANK, Page, TabError, parse_tab_id, tab_name};
use crate::errors::from_cdp;
use crate::request::{TabCloseRequest, TabNewRequest, TabSelectRequest, TabsRequest, millis};
use crate::{Engine, Running, navigate};

/// How long a report of a new tab waits for it to have a URL.
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
        running
            .tabs
            .page(id, timeout)
            .await
            .map_err(|e| self.tab_error(running, e))
    }

    fn tab_error(&self, running: &Running, err: TabError) -> ErrorInfo {
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
                .hint(Hint::run(Cmd::new("tabs").session(&self.session)).why("lists them"))
                .details(json!({ "tab": tab_name(id), "open": open }))
            }
            TabError::NotAttached => ErrorInfo::new(
                ErrorKind::ProtocolError,
                "Chrome opened a tab but never reported it",
            )
            .hint(Hint::run(Cmd::new("tabs").session(&self.session))),
            TabError::Cdp(e) => from_cdp(&e, &self.session),
        }
    }

    /// Adds the tabs that opened since `mark` (popups) to `output`: as
    /// events, and as warnings so the text forms mention them.
    pub(crate) async fn report_opened(
        &self,
        running: &Running,
        mark: u64,
        mut output: Output,
    ) -> Output {
        let ids: Vec<u32> = running
            .tabs
            .opened_since(mark)
            .iter()
            .filter_map(|protocol::schema::Event::TabOpened { tab, .. }| parse_tab_id(tab).ok())
            .collect();
        if ids.is_empty() {
            return output;
        }
        // A popup has no URL until its first navigation commits.
        running.tabs.wait_for_urls(&ids, POPUP_URL_WAIT).await;
        let _ = running.tabs.refresh(Duration::from_secs(2)).await;
        for event in running.tabs.opened_since(mark) {
            let protocol::schema::Event::TabOpened {
                tab, url, active, ..
            } = &event;
            let url = if url.is_empty() { BLANK } else { url };
            output = output.warning(if *active {
                format!("opened tab {tab} ({url}) and switched to it")
            } else {
                format!(
                    "opened tab {tab} ({url}); switch to it: {}",
                    Cmd::new("tab").arg(tab).session(&self.session)
                )
            });
            output = output.event(event);
        }
        output
    }

    /// `tabs`: every tab, the active one marked.
    pub async fn tabs(&self, req: TabsRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let tabs = running
            .tabs
            .list(millis(req.timeout_ms))
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let text = if tabs.is_empty() {
            format!(
                "no tabs; open one: {}",
                Cmd::new("tab").arg("new").session(&self.session)
            )
        } else {
            let rows: Vec<Vec<String>> = tabs
                .iter()
                .map(|t| {
                    let mut title = format!("{:?}", t.title);
                    if let Some(opener) = &t.opener {
                        let _ = write!(title, " (opened by {opener})");
                    }
                    vec![
                        format!("{} {}", if t.active { "*" } else { " " }, t.id),
                        shown_url(&t.url).to_owned(),
                        title,
                    ]
                })
                .collect();
            format::table(&rows)
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
        let page = running
            .tabs
            .create(BLANK, timeout)
            .await
            .map_err(|e| self.tab_error(&running, e))?;
        running
            .tabs
            .activate(page.id, timeout)
            .await
            .map_err(|e| self.tab_error(&running, e))?;
        if let Some(url) = &req.url {
            let _tab = self.tabs.lock(&page.target_id).await;
            navigate::open_url(&page.session, url, timeout, &self.session).await?;
        }
        let tab = self.tab_info(&running, page.id).await;
        let text = format!("opened tab {}", describe_tab(&tab));
        let output = Output::typed(&TabData { tab: tab.clone() }, text).quiet(tab.id);
        Ok(self.report_opened(&running, mark, output).await)
    }

    /// `tab <id>`: makes a tab active (and brings it to the front).
    pub async fn tab_select(&self, req: TabSelectRequest) -> Result<Output, ErrorInfo> {
        let id = parse_tab_id(&req.tab)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        running
            .tabs
            .activate(id, timeout)
            .await
            .map_err(|e| self.tab_error(&running, e))?;
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
        let last = running.tabs.count() <= 1;
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
        let active = match running.tabs.active() {
            Some(id) => Some(self.tab_info(&running, id).await),
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
                    .session(&self.session)
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
        })
    }
}

/// `t2 https://example.com/ "Example Domain"`.
fn describe_tab(tab: &TabInfo) -> String {
    format!("{} {} {:?}", tab.id, shown_url(&tab.url), tab.title)
}

/// A new tab's URL is empty until it commits a navigation.
fn shown_url(url: &str) -> &str {
    if url.is_empty() { BLANK } else { url }
}
