//! The walk: breadth-first over the states `explore` finds, clicking each
//! candidate of each state with the normal action code, and getting back
//! to the state for the next one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::{
    Event, ExploreBroken, ExploreEdge, ExploreElement, ExploreFailed, ExploreLeft, ExploreSkip,
    ExploreSkipped, ExploreState, ExploreStep,
};
use protocol::{Cmd, ErrorInfo, ErrorKind};
use serde::Deserialize;
use serde_json::json;
use tokio::time::Instant;

use super::rules::{self, LinkKind};
use crate::errors::from_cdp;
use crate::expect::is_favicon;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::locator::{DEFAULT_TESTID_ATTRIBUTE, LocateOptions};
use crate::observe::save;
use crate::pattern::UrlPattern;
use crate::request::{Action, ActionRequest, Button, ExploreRequest, Navigation, WaitUntil};
use crate::tabs::{BLANK, Page, parse_tab_id, tab_name};
use crate::{Engine, Running};

/// How long a tab a click opened gets to show its URL.
const NEW_TAB_URL_WAIT: Duration = Duration::from_secs(2);

/// A state's key: its URL, normalised, and its skeleton's hash.
type Key = (String, u64);

/// What `exploreCandidates` lists.
#[derive(Debug, Clone, Deserialize)]
struct Found {
    kind: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tag: String,
    href: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    download: bool,
    locator: Option<String>,
    reason: Option<String>,
}

impl Found {
    fn element(&self) -> ExploreElement {
        ExploreElement {
            role: self.role.clone(),
            name: self.name.clone(),
            tag: self.tag.clone(),
        }
    }
}

/// A candidate the walk will follow from a state.
#[derive(Debug, Clone)]
struct Todo {
    step: ExploreStep,
    /// A link's target, normalised: a link to where another link led
    /// already is not clicked again.
    href: Option<String>,
}

/// A state found.
struct Known {
    data: ExploreState,
    key: Key,
    todo: Vec<Todo>,
    /// Whether loading its URL gets back to it (`None` until tried).
    direct: Option<bool>,
}

/// What the page shows now.
struct Look {
    url: String,
    title: String,
    key: Key,
    dialogs: Vec<String>,
}

/// Why the walk stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    MaxStates,
    MaxActions,
    Time,
}

impl Stop {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MaxStates => "max-states",
            Self::MaxActions => "max-actions",
            Self::Time => "time",
        }
    }
}

/// A walk in progress, in the tab `page` of explore's own context.
pub(super) struct Walk<'a> {
    engine: &'a Engine,
    running: &'a Running,
    req: &'a ExploreRequest,
    page: Page,
    timeout: Duration,
    pub(super) started: Instant,
    deadline: Instant,
    /// Origins links are followed to.
    allowed: Vec<String>,
    include: Vec<UrlPattern>,
    exclude: Vec<UrlPattern>,
    /// `states/` of `-o`.
    states_dir: Option<PathBuf>,
    /// The start's URL, as loaded.
    pub(super) start: String,
    known: Vec<Known>,
    /// The state the tab shows, when known.
    at: Option<usize>,
    pub(super) actions: u64,
    /// Where each link target followed led: a state, or nowhere (broken,
    /// or off the allowed origins).
    reached: HashMap<String, Option<usize>>,
    pub(super) edges: Vec<ExploreEdge>,
    pub(super) broken: Vec<ExploreBroken>,
    pub(super) skipped: ExploreSkipped,
    pub(super) unvisited: Vec<ExploreLeft>,
    pub(super) failed: Vec<ExploreFailed>,
    /// Something was not followed for `--depth`.
    pub(super) depth_left: bool,
    pub(super) stopped: Option<Stop>,
    pub(super) warnings: Vec<String>,
}

impl<'a> Walk<'a> {
    pub(super) fn new(
        engine: &'a Engine,
        running: &'a Running,
        req: &'a ExploreRequest,
        page: Page,
        states_dir: Option<PathBuf>,
    ) -> Result<Self, ErrorInfo> {
        let patterns = |list: &[String]| -> Result<Vec<UrlPattern>, ErrorInfo> {
            list.iter().map(|p| UrlPattern::parse(p)).collect()
        };
        let started = Instant::now();
        Ok(Self {
            engine,
            running,
            req,
            page,
            timeout: crate::request::millis(req.timeout_ms),
            started,
            deadline: started + Duration::from_millis(req.time_ms),
            allowed: req
                .origins
                .iter()
                .filter_map(|o| rules::origin(o))
                .collect(),
            include: patterns(&req.include)?,
            exclude: patterns(&req.exclude)?,
            states_dir,
            start: String::new(),
            known: Vec::new(),
            at: None,
            actions: 0,
            reached: HashMap::new(),
            edges: Vec::new(),
            broken: Vec::new(),
            skipped: ExploreSkipped::default(),
            unvisited: Vec::new(),
            failed: Vec::new(),
            depth_left: false,
            stopped: None,
            warnings: Vec::new(),
        })
    }

    /// The states found, with what each recorded.
    pub(super) fn states(&self) -> Vec<ExploreState> {
        self.known.iter().map(|k| k.data.clone()).collect()
    }

    /// Loads `url`, then follows every state's candidates, breadth first,
    /// until all are followed or the budget runs out.
    pub(super) async fn run(&mut self, url: &str) -> Result<(), ErrorInfo> {
        let marks = self.running.tabs.log_marks(self.page.id);
        let loaded = self.goto(url).await?;
        let look = self.look().await?;
        self.start.clone_from(&look.url);
        if let Some(origin) = rules::origin(&look.url) {
            self.allowed.insert(0, origin);
        }
        // A start that redirected elsewhere keeps the typed URL's origin.
        if let Some(typed) = rules::origin(&loaded.0)
            && !self.allowed.contains(&typed)
        {
            self.allowed.push(typed);
        }
        let s1 = self.found(look, loaded.1, 0, Vec::new(), marks).await?;
        self.at = Some(s1);
        let mut i = 0;
        while i < self.known.len() {
            let todo = std::mem::take(&mut self.known[i].todo);
            for (n, item) in todo.iter().enumerate() {
                match self.follow(i, item).await? {
                    Flow::Go => {}
                    Flow::Lost => {
                        // The rest of the state cannot be reached.
                        break;
                    }
                    Flow::Stop(stop) => {
                        self.stopped = Some(stop);
                        self.leave(i, &todo[n..]);
                        for j in i + 1..self.known.len() {
                            let rest = std::mem::take(&mut self.known[j].todo);
                            self.leave(j, &rest);
                        }
                        return Ok(());
                    }
                }
            }
            i += 1;
        }
        Ok(())
    }

    /// Records the candidates of state `i` the budget leaves (those a
    /// link followed elsewhere already reached are not left).
    fn leave(&mut self, i: usize, todo: &[Todo]) {
        for item in todo {
            if item
                .href
                .as_ref()
                .is_some_and(|h| self.reached.contains_key(h))
            {
                continue;
            }
            self.unvisited.push(ExploreLeft {
                from: self.known[i].data.id.clone(),
                step: item.step.clone(),
            });
        }
    }

    /// Why the walk must stop before another action, if it must.
    /// Why the walk must stop before `cost` more actions, if it must.
    fn over_budget(&self, cost: u64) -> Option<Stop> {
        if Instant::now() >= self.deadline {
            Some(Stop::Time)
        } else if self.actions + cost > self.req.max_actions {
            Some(Stop::MaxActions)
        } else if self.known.len() as u64 >= self.req.max_states {
            Some(Stop::MaxStates)
        } else {
            None
        }
    }

    /// How many actions getting back to state `i` takes: none when the
    /// tab is there, one load when its URL gets there (or may), else a
    /// load and its path.
    fn back_cost(&self, i: usize) -> u64 {
        match (self.at == Some(i), self.known[i].direct) {
            (true, _) => 0,
            (false, None | Some(true)) => 1,
            (false, Some(false)) => 1 + self.known[i].data.path.len() as u64,
        }
    }

    /// Follows one candidate of state `i`.
    async fn follow(&mut self, i: usize, item: &Todo) -> Result<Flow, ErrorInfo> {
        let from = self.known[i].data.id.clone();
        if let Some(href) = &item.href
            && let Some(&to) = self.reached.get(href)
        {
            // Where a link to the same place led already.
            if let Some(to) = to
                && to != i
            {
                self.edge(i, to, &item.step);
            }
            return Ok(Flow::Go);
        }
        if self.known[i].data.depth >= self.req.depth {
            self.depth_left = true;
            self.unvisited.push(ExploreLeft {
                from,
                step: item.step.clone(),
            });
            return Ok(Flow::Go);
        }
        if let Some(stop) = self.over_budget(self.back_cost(i) + 1) {
            return Ok(Flow::Stop(stop));
        }
        match self.back_to(i).await {
            Ok(true) => {}
            // Loading its URL did not get there, and the budget has no
            // room to replay its path.
            Ok(false) => return Ok(Flow::Stop(Stop::MaxActions)),
            Err(e) => {
                self.failed.push(ExploreFailed {
                    from: from.clone(),
                    step: None,
                    error: format!("could not get back to {from}: {}", e.message),
                });
                return Ok(Flow::Lost);
            }
        }
        let marks = self.running.tabs.log_marks(self.page.id);
        let events = match self.click(&item.step).await {
            Ok(events) => events,
            Err(e) => {
                self.failed.push(ExploreFailed {
                    from,
                    step: Some(item.step.clone()),
                    error: e.message,
                });
                return Ok(Flow::Go);
            }
        };
        let navigated = events.iter().find_map(|e| match e {
            Event::Navigated {
                url, status, error, ..
            } => Some((url.clone(), *status, error.clone())),
            _ => None,
        });
        if let Some((url, status, error)) = &navigated
            && (error.is_some() || status.is_some_and(|s| s >= 400))
        {
            self.broken.push(ExploreBroken {
                from,
                step: item.step.clone(),
                url: url.clone(),
                status: *status,
                error: error.clone(),
            });
            if let Some(href) = &item.href {
                self.reached.insert(href.clone(), None);
            }
            return Ok(Flow::Go);
        }
        let opened: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                Event::TabOpened { tab, .. } => Some(tab.clone()),
                _ => None,
            })
            .collect();
        let mut new_tab = None;
        for tab in opened {
            let url = self.close_opened(&tab).await;
            if new_tab.is_none() && navigated.is_none() && !url.is_empty() && url != BLANK {
                new_tab = Some(url);
            }
        }
        if let Some(url) = new_tab {
            return self.follow_new_tab(i, item, &url).await;
        }
        let status = navigated.and_then(|(_, status, _)| status);
        self.arrive(i, item, status, marks).await
    }

    /// Records where a step taken in state `i` led: the state the page is
    /// in now, found now or earlier.
    async fn arrive(
        &mut self,
        i: usize,
        item: &Todo,
        status: Option<u16>,
        marks: (u64, u64),
    ) -> Result<Flow, ErrorInfo> {
        let look = match self.look().await {
            Ok(look) => look,
            Err(e) => {
                self.failed.push(ExploreFailed {
                    from: self.known[i].data.id.clone(),
                    step: Some(item.step.clone()),
                    error: e.message,
                });
                return Ok(Flow::Go);
            }
        };
        if !self.allowed_origin(&look.url) {
            // It left the allowed origins (a redirect).
            self.skipped.external.push(ExploreSkip {
                from: self.known[i].data.id.clone(),
                element: item.step.element.clone().unwrap_or_else(|| ExploreElement {
                    role: String::new(),
                    name: String::new(),
                    tag: String::new(),
                }),
                url: Some(look.url),
            });
            if let Some(href) = &item.href {
                self.reached.insert(href.clone(), None);
            }
            return Ok(Flow::Go);
        }
        let to = if let Some(to) = self.known.iter().position(|k| k.key == look.key) {
            self.record_errors(to, marks).await;
            if to != i {
                self.edge(i, to, &item.step);
            }
            self.at = Some(to);
            to
        } else {
            let mut path = self.known[i].data.path.clone();
            path.push(item.step.clone());
            let depth = self.known[i].data.depth + 1;
            let to = self.found(look, status, depth, path, marks).await?;
            self.edge(i, to, &item.step);
            self.at = Some(to);
            to
        };
        if let Some(href) = &item.href {
            self.reached.insert(href.clone(), Some(to));
        }
        Ok(Flow::Go)
    }

    /// A click in state `i` opened `url` in a new tab (closed again): the
    /// walk loads it in its own tab instead, when it may follow it.
    async fn follow_new_tab(
        &mut self,
        i: usize,
        item: &Todo,
        url: &str,
    ) -> Result<Flow, ErrorInfo> {
        let from = self.known[i].data.id.clone();
        let step = ExploreStep {
            action: "goto".to_owned(),
            locator: item.step.locator.clone(),
            element: item.step.element.clone(),
            url: Some(url.to_owned()),
            new_tab: true,
        };
        let element = item.step.element.clone().unwrap_or_else(|| ExploreElement {
            role: String::new(),
            name: String::new(),
            tag: String::new(),
        });
        if !self.may_follow(url) {
            self.skipped.external.push(ExploreSkip {
                from,
                element,
                url: Some(url.to_owned()),
            });
            return Ok(Flow::Go);
        }
        let href = rules::normalize(url);
        if let Some(&to) = self.reached.get(&href) {
            if let Some(to) = to {
                self.edge(i, to, &step);
            }
            return Ok(Flow::Go);
        }
        if let Some(stop) = self.over_budget(1) {
            self.unvisited.push(ExploreLeft { from, step });
            return Ok(Flow::Stop(stop));
        }
        let marks = self.running.tabs.log_marks(self.page.id);
        self.at = None;
        let status = match self.goto(url).await {
            Ok((_, status)) => status,
            Err(e) => {
                self.broken.push(ExploreBroken {
                    from,
                    step,
                    url: url.to_owned(),
                    status: None,
                    error: Some(e.message),
                });
                self.reached.insert(href, None);
                return Ok(Flow::Go);
            }
        };
        if status.is_some_and(|s| s >= 400) {
            self.broken.push(ExploreBroken {
                from,
                step,
                url: url.to_owned(),
                status,
                error: None,
            });
            self.reached.insert(href, None);
            return Ok(Flow::Go);
        }
        let item = Todo {
            step,
            href: Some(href),
        };
        self.arrive(i, &item, status, marks).await
    }

    /// Closes tab `tab`, which a click opened; returns its URL.
    async fn close_opened(&self, tab: &str) -> String {
        let Ok(id) = parse_tab_id(tab) else {
            return String::new();
        };
        let until = Instant::now() + NEW_TAB_URL_WAIT;
        let mut url = String::new();
        loop {
            if let Some(info) = self.running.tabs.info(id) {
                url = info.url;
            }
            if (!url.is_empty() && url != BLANK) || Instant::now() >= until {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = self.running.tabs.close(id, self.timeout).await;
        url
    }

    /// Records a step from state `from` to state `to`, once.
    fn edge(&mut self, from: usize, to: usize, step: &ExploreStep) {
        let (from, to) = (
            self.known[from].data.id.clone(),
            self.known[to].data.id.clone(),
        );
        if !self
            .edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.step == *step)
        {
            self.edges.push(ExploreEdge {
                from,
                to,
                step: step.clone(),
            });
        }
    }

    /// Gets the tab back to state `i`: by loading its URL when that gives
    /// the same skeleton (checked the first time), else by replaying its
    /// path from the start.
    /// `false` when the budget leaves no room to replay the path.
    async fn back_to(&mut self, i: usize) -> Result<bool, ErrorInfo> {
        if self.at == Some(i) {
            return Ok(true);
        }
        self.at = None;
        let url = self.known[i].data.url.clone();
        let key = self.known[i].key.clone();
        if self.known[i].direct != Some(false) {
            self.goto(&url).await?;
            let look = self.look().await?;
            if look.key == key {
                self.known[i].direct = Some(true);
                self.at = Some(i);
                return Ok(true);
            }
            self.known[i].direct = Some(false);
            if self.over_budget(self.back_cost(i) + 1).is_some() {
                return Ok(false);
            }
        }
        let start = self.start.clone();
        self.goto(&start).await?;
        let path = self.known[i].data.path.clone();
        for step in &path {
            match (step.action.as_str(), &step.url) {
                ("goto", Some(url)) => {
                    self.goto(url).await?;
                }
                _ => {
                    self.click(step).await?;
                }
            }
        }
        let look = self.look().await?;
        if look.key == key {
            self.at = Some(i);
            Ok(true)
        } else {
            Err(ErrorInfo::new(
                ErrorKind::NotFound,
                format!(
                    "its path led to {} {:?}, which looks different",
                    look.url, look.title
                ),
            ))
        }
    }

    /// Clicks what `step` names with the normal action code; returns the
    /// report's events.
    async fn click(&mut self, step: &ExploreStep) -> Result<Vec<Event>, ErrorInfo> {
        let Some(locator) = &step.locator else {
            return Err(ErrorInfo::new(
                ErrorKind::NotFound,
                "no stable locator finds it",
            ));
        };
        self.actions += 1;
        self.at = None;
        let req = ActionRequest {
            action: Action::Click {
                locator: locator.clone(),
                button: Button::Left,
                count: 1,
                modifiers: Vec::new(),
                position: None,
                force: false,
                dblclick: false,
            },
            tab: Some(tab_name(self.page.id)),
            strict_refs: false,
            testid_attribute: self.req.testid_attribute.clone(),
            timeout_ms: self.req.timeout_ms,
            no_wait_after: false,
            snap: false,
            max_chars: 0,
        };
        let output = Box::pin(self.engine.act(req)).await?;
        Ok(output.events)
    }

    /// Loads `url` in the walk's tab; returns the URL as typed back and
    /// the document's status.
    async fn goto(&mut self, url: &str) -> Result<(String, Option<u16>), ErrorInfo> {
        self.actions += 1;
        self.at = None;
        let _tab = self.engine.tabs.lock(&self.page.target_id).await;
        let navigation = Navigation::Goto {
            url: url.to_owned(),
            base_url: self.req.base_url.clone(),
        };
        let navigated = self
            .engine
            .navigate_page(
                self.running,
                &self.page,
                &navigation,
                WaitUntil::Settle,
                self.timeout,
            )
            .await?;
        Ok((navigated.data.url.clone(), navigated.data.status))
    }

    /// Reads the page as it is: its URL, title and skeleton.
    async fn look(&self) -> Result<Look, ErrorInfo> {
        let _tab = self.engine.tabs.lock(&self.page.target_id).await;
        let mark = self.running.tabs.mark();
        let retry = |r: &str| Cmd::new("snapshot").flag("scope", r);
        let (url, title, nodes) = self
            .engine
            .compare_tree(
                self.running,
                &self.page,
                None,
                &self.options(),
                false,
                &retry,
                mark,
                self.timeout,
            )
            .await?;
        let (hash, dialogs) = rules::skeleton(&nodes);
        Ok(Look {
            key: (rules::normalize(&url), hash),
            url,
            title,
            dialogs,
        })
    }

    fn options(&self) -> LocateOptions {
        LocateOptions {
            strict_refs: false,
            testid_attribute: self.req.testid_attribute.clone(),
        }
    }

    /// Records a new state for what the page shows (`look`), reached by
    /// `path`, and what it offers to follow; returns its index.
    async fn found(
        &mut self,
        look: Look,
        status: Option<u16>,
        depth: u64,
        path: Vec<ExploreStep>,
        marks: (u64, u64),
    ) -> Result<usize, ErrorInfo> {
        let id = format!("s{}", self.known.len() + 1);
        let index = self.known.len();
        self.known.push(Known {
            data: ExploreState {
                id: id.clone(),
                url: look.url.clone(),
                title: look.title,
                status,
                depth,
                dialogs: look.dialogs,
                path,
                errors: Vec::new(),
                failed_requests: Vec::new(),
                layout: None,
                scenario: None,
                screenshot: None,
            },
            key: look.key,
            todo: Vec::new(),
            direct: None,
        });
        self.at = Some(index);
        self.record_errors(index, marks).await;
        match self.candidates().await {
            Ok(found) => self.sort(index, found),
            Err(e) => self.warnings.push(format!(
                "cannot list what {id} offers to follow: {}",
                e.message
            )),
        }
        if self.req.layout || self.req.screenshots {
            let _ = self
                .running
                .tabs
                .bring_to_front(self.page.id, self.timeout)
                .await;
        }
        if self.req.layout {
            let _tab = self.engine.tabs.lock(&self.page.target_id).await;
            match self
                .engine
                .layout_errors(
                    self.running,
                    &self.page,
                    self.req.testid_attribute.clone(),
                    self.timeout,
                )
                .await
            {
                Ok(findings) => self.known[index].data.layout = Some(findings),
                Err(e) => self
                    .warnings
                    .push(format!("cannot check the layout of {id}: {}", e.message)),
            }
        }
        if self.req.screenshots
            && let Some(dir) = &self.states_dir
        {
            let path = dir.join(format!("{id}.png"));
            let shot = async {
                let _tab = self.engine.tabs.lock(&self.page.target_id).await;
                let shot = self
                    .engine
                    .layout_shot(self.running, &self.page, self.timeout)
                    .await?;
                let png = shot
                    .encode()
                    .map_err(|e| ErrorInfo::new(ErrorKind::ProtocolError, e))?;
                save::save(&png, Some(&path), dir, "png")
            }
            .await;
            match shot {
                Ok(saved) => self.known[index].data.screenshot = Some(saved.display().to_string()),
                Err(e) => self
                    .warnings
                    .push(format!("cannot save a screenshot of {id}: {}", e.message)),
            }
        }
        Ok(index)
    }

    /// Adds the console errors and failed requests since `marks` to state
    /// `index`, each once.
    async fn record_errors(&mut self, index: usize, marks: (u64, u64)) {
        let errors = self
            .running
            .tabs
            .console_errors_since(self.page.id, marks.1);
        let errors = self
            .engine
            .console_error_events(self.running, &self.page, errors)
            .await;
        let tab = tab_name(self.page.id);
        let failures: Vec<Event> = self
            .running
            .tabs
            .failures_since(self.page.id, marks.0)
            .into_iter()
            .filter(|f| self.allowed_origin(&f.url) && !is_favicon(&f.url))
            .map(|f| Event::RequestFailed {
                tab: tab.clone(),
                id: None,
                method: f.method,
                url: f.url,
                resource_type: f.kind,
                status: f.status,
                error: f.error,
            })
            .collect();
        let state = &mut self.known[index].data;
        for error in errors {
            let seen = state.errors.iter().any(|e| same_error(e, &error));
            if !seen {
                state.errors.push(error);
            }
        }
        for failure in failures {
            if !state.failed_requests.contains(&failure) {
                state.failed_requests.push(failure);
            }
        }
    }

    /// What the page offers to follow, in document order.
    async fn candidates(&self) -> Result<Vec<Found>, ErrorInfo> {
        let _tab = self.engine.tabs.lock(&self.page.target_id).await;
        let (_, context) = self
            .engine
            .frame_and_context(
                self.running,
                &self.page,
                None,
                World::Isolated,
                self.timeout,
            )
            .await?;
        let testid = self
            .req
            .testid_attribute
            .clone()
            .unwrap_or_else(|| DEFAULT_TESTID_ATTRIBUTE.to_owned());
        let buttons = self.req.click_buttons;
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (testid, buttons) {{ {GUARD} return globalThis.__ccdp.exploreCandidates(testid, buttons); }}"
        ));
        params.arguments = Some(
            [json!(testid), json!(buttons)]
                .into_iter()
                .map(|value| runtime::CallArgument {
                    value: Some(value),
                    unserializable_value: None,
                    object_id: None,
                })
                .collect(),
        );
        params.return_by_value = Some(true);
        let session = context.session.with_timeout(self.timeout);
        let result = injected::call(&session, context.context_id, params)
            .await
            .map_err(|e| from_cdp(&e, &self.engine.session))?;
        if let Some(exception) = result.exception_details {
            return Err(ErrorInfo::new(
                ErrorKind::EvalError,
                format!("listing the candidates failed: {}", exception.text),
            ));
        }
        let json = result
            .result
            .value
            .as_ref()
            .and_then(|v| v.as_str())
            .unwrap_or("[]");
        serde_json::from_str(json).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("reading the candidates: {e}"),
            )
        })
    }

    /// Sorts what state `index` offers: what to follow goes in its todo,
    /// what not in `skipped` (each thing once over the walk).
    fn sort(&mut self, index: usize, found: Vec<Found>) {
        let from = self.known[index].data.id.clone();
        let here = self.known[index].key.0.clone();
        let mut todo: Vec<Todo> = Vec::new();
        for f in found {
            let element = f.element();
            let skip = |url: Option<String>| ExploreSkip {
                from: from.clone(),
                element: element.clone(),
                url,
            };
            let destructive = !self.req.allow_destructive && rules::destructive(&f.name);
            let mut href = None;
            match f.kind.as_str() {
                "link" => {
                    let Some(url) = f.href.clone() else { continue };
                    match rules::link_kind(&url, f.download) {
                        LinkKind::Other => {
                            add(&mut self.skipped.other, skip(Some(url)));
                            continue;
                        }
                        LinkKind::File => {
                            add(&mut self.skipped.files, skip(Some(url)));
                            continue;
                        }
                        LinkKind::Page => {}
                    }
                    if !self.may_follow(&url) {
                        add(&mut self.skipped.external, skip(Some(url)));
                        continue;
                    }
                    if destructive {
                        add(&mut self.skipped.destructive, skip(Some(url)));
                        continue;
                    }
                    let normalized = rules::normalize(&url);
                    // A link to where the page is: nothing to find.
                    if normalized == here && f.target.as_deref().is_none_or(|t| t == "_self") {
                        continue;
                    }
                    href = Some(normalized);
                }
                "submit" => {
                    if !self.req.submit_forms {
                        add(&mut self.skipped.forms, skip(None));
                        continue;
                    }
                    if destructive {
                        add(&mut self.skipped.destructive, skip(None));
                        continue;
                    }
                }
                "reset" => {
                    if !self.req.allow_destructive || !self.req.click_buttons {
                        add(&mut self.skipped.destructive, skip(None));
                        continue;
                    }
                }
                _ => {
                    if destructive {
                        add(&mut self.skipped.destructive, skip(None));
                        continue;
                    }
                }
            }
            let step = ExploreStep {
                action: "click".to_owned(),
                locator: f.locator.clone(),
                element: Some(element.clone()),
                url: f.href.clone().filter(|_| f.kind == "link"),
                new_tab: false,
            };
            if f.locator.is_none() {
                let reason = f.reason.as_deref().unwrap_or("none finds it alone");
                self.failed.push(ExploreFailed {
                    from: from.clone(),
                    step: Some(step),
                    error: format!("no stable locator: {reason}"),
                });
                continue;
            }
            if !todo.iter().any(|t| t.step.locator == step.locator) {
                todo.push(Todo { step, href });
            }
        }
        self.known[index].todo = todo;
    }

    /// Whether the walk may follow a link to `url`: an allowed origin,
    /// and `--include`/`--exclude` let it.
    fn may_follow(&self, url: &str) -> bool {
        self.allowed_origin(url)
            && (self.include.is_empty() || self.include.iter().any(|p| p.matches(url)))
            && !self.exclude.iter().any(|p| p.matches(url))
    }

    fn allowed_origin(&self, url: &str) -> bool {
        rules::origin(url).is_some_and(|o| self.allowed.contains(&o))
    }
}

/// What following a candidate came to.
enum Flow {
    Go,
    /// The walk could not get back to the state: skip the rest of it.
    Lost,
    Stop(Stop),
}

/// Adds `skip` unless the list has the same element (and URL) already.
fn add(list: &mut Vec<ExploreSkip>, skip: ExploreSkip) {
    if !list
        .iter()
        .any(|s| s.element == skip.element && s.url == skip.url)
    {
        list.push(skip);
    }
}

/// Whether two console errors are the same message from the same place.
fn same_error(a: &Event, b: &Event) -> bool {
    match (a, b) {
        (
            Event::ConsoleError {
                message: m1,
                url: u1,
                line: l1,
                ..
            },
            Event::ConsoleError {
                message: m2,
                url: u2,
                line: l2,
                ..
            },
        ) => m1 == m2 && u1 == u2 && l1 == l2,
        _ => a == b,
    }
}
