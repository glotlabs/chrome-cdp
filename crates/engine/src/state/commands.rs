//! `cookies`, `cookies set`, `cookies clear`, `storage`, `state save` and
//! `state load` (plan 07, milestone 7.4).
//!
//! Cookies and state files belong to the browser, so those commands take
//! no tab lock; `storage` works in a frame of one tab and holds its lock.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cdp::domains::network::{self, Cookie, CookieParam};
use cdp::domains::{runtime, storage};
use protocol::schema::{
    CookieInfo, CookieSetData, CookiesClearData, CookiesData, StateData, StateOriginInfo,
    StorageData, StorageItem,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Value, json};

use super::{
    LIBRARY, OriginState, StorageState, cookie_info, cookie_param, expired, in_domain, origin_of,
    parse_same_site, state_cookie,
};
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::inject::SEED_BINDING;
use crate::observe::iso8601;
use crate::request::{
    CookieFilter, CookieSpec, CookiesOp, CookiesRequest, StateLoadRequest, StateSaveRequest,
    StorageArea, StorageOp, StorageRequest, millis,
};
use crate::tabs::{FrameContext, FrameError, tab_name};
use crate::{Engine, Running};

/// How much of a cookie's value the text list shows.
const VALUE_PREVIEW: usize = 60;

/// `storage`'s script: one Web Storage action in the frame's document.
const STORAGE_OP: &str = r"(area, op, key, value) => {
  if (location.origin === 'null') return { origin: 'null' };
  const s = globalThis[area];
  const out = { origin: location.origin };
  if (op === 'list') {
    out.items = [];
    for (let i = 0; i < s.length; i++) {
      const name = s.key(i);
      out.items.push({ name, value: s.getItem(name) });
    }
    out.items.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  } else if (op === 'get') {
    out.value = s.getItem(key);
  } else if (op === 'set') {
    s.setItem(key, value);
  } else if (op === 'remove') {
    out.had = s.getItem(key) !== null;
    s.removeItem(key);
  } else if (op === 'clear') {
    out.removed = s.length;
    s.clear();
  }
  out.length = s.length;
  return out;
}";

impl Engine {
    /// `cookies`, `cookies set` and `cookies clear`.
    pub async fn cookies(&self, req: CookiesRequest) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let running = self.running()?;
        match req.op {
            CookiesOp::List { filter } => {
                let cookies = self.find_cookies(&running, &filter, timeout).await?;
                Ok(self.cookie_list(&cookies, &filter, req.max_chars))
            }
            CookiesOp::Set { cookie } => self.set_cookie(&running, cookie, timeout).await,
            CookiesOp::Clear { filter } => self.clear_cookies(&running, &filter, timeout).await,
        }
    }

    /// The browser's cookies that `filter` keeps, by domain, path and
    /// name. With a URL, Chrome picks the cookies a request to it carries.
    async fn find_cookies(
        &self,
        running: &Running,
        filter: &CookieFilter,
        timeout: Duration,
    ) -> Result<Vec<Cookie>, ErrorInfo> {
        let mut cookies = if let Some(url) = &filter.url {
            check_url(url, "--url")?;
            let page = self.page(running, None, timeout).await?;
            let params = network::GetCookiesParams {
                urls: Some(vec![url.clone()]),
            };
            page.session
                .with_timeout(timeout)
                .call(params)
                .await
                .map_err(|e| from_cdp(&e, &self.session))?
                .cookies
        } else {
            self.all_cookies(running, timeout).await?
        };
        cookies.retain(|c| {
            filter
                .domain
                .as_deref()
                .is_none_or(|d| in_domain(&c.domain, d))
                && filter.name.as_deref().is_none_or(|n| c.name == n)
        });
        cookies.sort_by(|a, b| {
            let key = |c: &Cookie| (c.domain.trim_start_matches('.').to_owned(), c.path.clone());
            key(a).cmp(&key(b)).then_with(|| a.name.cmp(&b.name))
        });
        Ok(cookies)
    }

    async fn all_cookies(
        &self,
        running: &Running,
        timeout: Duration,
    ) -> Result<Vec<Cookie>, ErrorInfo> {
        let params = storage::GetCookiesParams {
            browser_context_id: self.context_key()?,
        };
        Ok(running
            .conn
            .browser()
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?
            .cookies)
    }

    /// What `cookies` prints: a line per cookie.
    fn cookie_list(&self, cookies: &[Cookie], filter: &CookieFilter, max_chars: usize) -> Output {
        let infos: Vec<CookieInfo> = cookies.iter().map(cookie_info).collect();
        let text = if infos.is_empty() {
            format!(
                "no cookies{}; {} sets one",
                filter_text(filter),
                Cmd::new("cookies")
                    .arg("set")
                    .raw("<name>=<value>")
                    .scoped(&self.session)
            )
        } else {
            let rows: Vec<Vec<String>> = infos.iter().map(cookie_row).collect();
            format::table(&rows)
        };
        let quiet: Vec<String> = infos
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        let data = CookiesData { cookies: infos };
        Output::typed(&data, text).quiet(quiet.join("\n")).limit(
            max_chars,
            "--max-chars 0 for all of them, or --domain to narrow",
        )
    }

    /// `cookies set`: sets the cookie, then reads back what Chrome stored.
    async fn set_cookie(
        &self,
        running: &Running,
        spec: CookieSpec,
        timeout: Duration,
    ) -> Result<Output, ErrorInfo> {
        if spec.name.is_empty() {
            return Err(
                ErrorInfo::new(ErrorKind::BadArgs, "the cookie has no name").hint(Hint::run(
                    Cmd::new("cookies")
                        .arg("set")
                        .arg("theme=dark")
                        .scoped(&self.session),
                )),
            );
        }
        let same_site = spec
            .same_site
            .as_deref()
            .map(|s| {
                parse_same_site(s).ok_or_else(|| {
                    ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("`{s}` is no SameSite value; use strict, lax or none"),
                    )
                })
            })
            .transpose()?;
        if let Some(expires) = spec.expires
            && expires <= now_secs()
        {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "the cookie's expiry is in the past, so it would be removed",
            )
            .hint(
                Hint::run(
                    Cmd::new("cookies")
                        .arg("clear")
                        .flag("name", &spec.name)
                        .scoped(&self.session),
                )
                .why("removes a cookie"),
            ));
        }
        let url = match (&spec.url, &spec.domain) {
            (Some(url), _) => {
                check_url(url, "--url")?;
                Some(url.clone())
            }
            (None, Some(_)) => None,
            (None, None) => Some(self.active_url(running, timeout).await?),
        };
        let host = match (&spec.domain, &url) {
            (Some(domain), _) => domain.trim_start_matches('.').to_owned(),
            (None, Some(url)) => url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default(),
            (None, None) => String::new(),
        };
        let param = CookieParam {
            name: spec.name.clone(),
            value: spec.value.clone(),
            url,
            domain: spec.domain.clone(),
            path: spec.path.clone(),
            secure: spec.secure.then_some(true),
            http_only: spec.http_only.then_some(true),
            same_site,
            expires: spec.expires,
            priority: None,
            source_scheme: None,
            source_port: None,
            partition_key: None,
        };
        let refused = |why: Option<String>| {
            let mut info = ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("Chrome refused cookie {}", spec.name),
            );
            if let Some(why) = why {
                info = info.note("reason", why);
            }
            info.hint(Hint::text(refusal_hint(&spec, same_site)))
        };
        let mut params = storage::SetCookiesParams::new(vec![param]);
        params.browser_context_id = self.context_key()?;
        running
            .conn
            .browser()
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| refused(Some(e.to_string())))?;
        let stored = self
            .all_cookies(running, timeout)
            .await?
            .into_iter()
            .filter(|c| c.name == spec.name && c.value == spec.value && in_domain(&c.domain, &host))
            .filter(|c| spec.path.as_deref().is_none_or(|p| c.path == p))
            .max_by_key(|c| c.domain.len() + c.path.len())
            .ok_or_else(|| refused(None))?;
        let cookie = cookie_info(&stored);
        let text = format!(
            "set {} for {}{} ({})",
            cookie.name,
            cookie.domain,
            cookie.path,
            flags(&cookie).join(", ")
        );
        let quiet = cookie.name.clone();
        Ok(Output::typed(&CookieSetData { cookie }, text).quiet(quiet))
    }

    /// The active tab's URL, for a cookie given neither URL nor domain.
    async fn active_url(&self, running: &Running, timeout: Duration) -> Result<String, ErrorInfo> {
        let no_page = || {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                "the active tab shows no web page to set the cookie for",
            )
            .hint(Hint::text(
                "give the cookie's --url (e.g. --url https://example.com) or --domain",
            ))
        };
        let page = match self.context_key()? {
            Some(scope) => running.tabs.page_in(&scope, None, timeout).await,
            None => running.tabs.page(None, timeout).await,
        };
        let Ok(page) = page else {
            return Err(no_page());
        };
        let url = running
            .tabs
            .frames(page.id)
            .into_iter()
            .next()
            .map(|f| f.url)
            .unwrap_or_default();
        if origin_of(&url).is_none() {
            return Err(no_page().note("url", url));
        }
        Ok(url)
    }

    /// `cookies clear`: all of them, or those the filter keeps (set again,
    /// expired).
    async fn clear_cookies(
        &self,
        running: &Running,
        filter: &CookieFilter,
        timeout: Duration,
    ) -> Result<Output, ErrorInfo> {
        let browser = running.conn.browser().with_timeout(timeout);
        let scope = self.context_key()?;
        let removed = if filter.is_empty() {
            let count = self.all_cookies(running, timeout).await?.len();
            let params = storage::ClearCookiesParams {
                browser_context_id: scope,
            };
            browser
                .call(params)
                .await
                .map_err(|e| from_cdp(&e, &self.session))?;
            count
        } else {
            let cookies = self.find_cookies(running, filter, timeout).await?;
            if !cookies.is_empty() {
                let mut params =
                    storage::SetCookiesParams::new(cookies.iter().map(expired).collect());
                params.browser_context_id = scope;
                browser
                    .call(params)
                    .await
                    .map_err(|e| from_cdp(&e, &self.session))?;
            }
            cookies.len()
        };
        let text = match removed {
            0 => format!("no cookies{} to remove", filter_text(filter)),
            n => format!("removed {}{}", count(n, "cookie"), filter_text(filter)),
        };
        Ok(Output::typed(&CookiesClearData { removed }, text).quiet(removed.to_string()))
    }

    /// `storage local|session`: lists, reads or changes the Web Storage of
    /// the tab's main frame, or of its first frame of `--origin`.
    pub async fn storage(&self, req: StorageRequest) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let running = self.running()?;
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let frame = match &req.origin {
            Some(origin) => Some(self.frame_of_origin(&running, page.id, origin)?),
            None => None,
        };
        let (key, value) = match &req.op {
            StorageOp::Get { key } | StorageOp::Remove { key } => (Some(key.as_str()), None),
            StorageOp::Set { key, value } => (Some(key.as_str()), Some(value.as_str())),
            StorageOp::List | StorageOp::Clear => (None, None),
        };
        let expression = format!(
            "({STORAGE_OP})({}, {}, {}, {})",
            json!(req.area.global()),
            json!(req.op.as_str()),
            json!(key),
            json!(value)
        );
        let result = self
            .in_frame(
                &running,
                &page,
                frame.as_deref(),
                World::Isolated,
                timeout,
                |context| {
                    let mut params = runtime::EvaluateParams::new(expression.clone());
                    params.context_id = Some(context.context_id);
                    params.return_by_value = Some(true);
                    context.session.with_timeout(timeout).call(params)
                },
            )
            .await?
            .map_err(|e| from_cdp(&e, &self.session))?;
        let global = req.area.global();
        if let Some(exception) = result.exception_details {
            let message = exception
                .exception
                .as_ref()
                .and_then(|e| e.description.clone())
                .unwrap_or(exception.text);
            let first = message.lines().next().unwrap_or_default().to_owned();
            return Err(ErrorInfo::new(
                ErrorKind::EvalError,
                format!("the page refused {global}: {first}"),
            ));
        }
        let result = result.result.value.unwrap_or(Value::Null);
        let origin = result["origin"].as_str().unwrap_or("null").to_owned();
        if origin == "null" {
            return Err(ErrorInfo::new(
                ErrorKind::NotFound,
                format!(
                    "tab {} shows a page with no origin, which keeps no {global}",
                    tab_name(page.id)
                ),
            )
            .hint(
                Hint::run(Cmd::new("goto").raw("<url>").scoped(&self.session))
                    .why("storage belongs to the page's origin"),
            ));
        }
        let length = result["length"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        let whose = format!("{global} of {origin}");
        let mut data = StorageData {
            area: req.area.as_str().to_owned(),
            origin: origin.clone(),
            tab: tab_name(page.id),
            action: req.op.as_str().to_owned(),
            items: Vec::new(),
            key: key.map(str::to_owned),
            value: None,
            length: None,
        };
        let (text, quiet) = match &req.op {
            StorageOp::List => {
                let items: Vec<StorageItem> =
                    serde_json::from_value(result["items"].clone()).unwrap_or_default();
                let mut text = format!("{whose}: {}", count(items.len(), "item"));
                if !items.is_empty() {
                    let rows: Vec<Vec<&str>> = items
                        .iter()
                        .map(|i| vec![i.name.as_str(), i.value.as_str()])
                        .collect();
                    text.push('\n');
                    text.push_str(&format::table(&rows));
                }
                let quiet: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
                let quiet = quiet.join("\n");
                data.items = items;
                (text, quiet)
            }
            StorageOp::Get { key } => {
                let Some(value) = result["value"].as_str() else {
                    return Err(ErrorInfo::new(
                        ErrorKind::NotFound,
                        format!("{whose} has no key `{key}`"),
                    )
                    .hint(
                        Hint::run(storage_cmd(req.area, &req, &self.session)).why("lists the keys"),
                    ));
                };
                data.value = Some(value.to_owned());
                (value.to_owned(), value.to_owned())
            }
            StorageOp::Set { key, value } => {
                data.value = Some(value.clone());
                data.length = Some(length);
                (
                    format!("set {key} in {whose} ({})", count(length, "item")),
                    key.clone(),
                )
            }
            StorageOp::Remove { key } => {
                data.length = Some(length);
                let text = if result["had"].as_bool().unwrap_or(false) {
                    format!(
                        "removed {key} from {whose} ({} left)",
                        count(length, "item")
                    )
                } else {
                    format!("{whose} has no key `{key}`; nothing removed")
                };
                (text, key.clone())
            }
            StorageOp::Clear => {
                data.length = Some(0);
                let removed = result["removed"]
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0);
                (
                    format!("cleared {whose} ({} removed)", count(removed, "item")),
                    removed.to_string(),
                )
            }
        };
        Ok(Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it"))
    }

    /// The id of tab `tab`'s first frame of `origin` (a URL's origin
    /// counts).
    fn frame_of_origin(
        &self,
        running: &Running,
        tab: u32,
        origin: &str,
    ) -> Result<String, ErrorInfo> {
        let wanted = origin_of(origin).ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{origin}` is not an origin; use e.g. https://example.com"),
            )
        })?;
        let frames = running.tabs.frames(tab);
        if let Some(frame) = frames
            .iter()
            .find(|f| origin_of(&f.url).as_deref() == Some(wanted.as_str()))
        {
            return Ok(frame.id.clone());
        }
        let mut open: Vec<String> = frames.iter().filter_map(|f| origin_of(&f.url)).collect();
        open.dedup();
        let mut info = ErrorInfo::new(
            ErrorKind::NotFound,
            format!("tab {} has no frame of {wanted}", tab_name(tab)),
        );
        if !open.is_empty() {
            info = info.note("origins", open.join(", "));
        }
        Err(info.hint(
            Hint::run(Cmd::new("goto").arg(&wanted).scoped(&self.session))
                .why("storage is read in a page of its origin"),
        ))
    }

    /// `state save`: writes the browser's cookies and the storage of every
    /// origin open in a tab, as Playwright's `storageState`, readable by
    /// the user only.
    pub async fn state_save(&self, req: StateSaveRequest) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let running = self.running()?;
        let scope = self.context_key()?;
        let cookies = self.all_cookies(&running, timeout).await?;
        let mut warnings = Vec::new();
        let mut origins = Vec::new();
        let expression = format!("({LIBRARY}).collect({})", req.indexed_db);
        for (tab, frame, origin) in open_origins(&running, scope.as_deref()) {
            let collected = match running
                .tabs
                .context(tab, &frame, World::Isolated, timeout)
                .await
            {
                Ok(context) => run(&context, expression.clone(), timeout).await,
                Err(e) => Err(frame_error(&e)),
            };
            match collected
                .and_then(|v| serde_json::from_value::<OriginState>(v).map_err(|e| e.to_string()))
            {
                Ok(state) => {
                    let has_db = state.indexed_db.as_ref().is_some_and(|d| !d.is_empty());
                    if !state.local_storage.is_empty()
                        || !state.session_storage.is_empty()
                        || has_db
                    {
                        origins.push(state);
                    }
                }
                Err(e) => warnings.push(format!("cannot read the storage of {origin}: {e}")),
            }
        }
        let state = StorageState {
            cookies: cookies.iter().map(state_cookie).collect(),
            origins,
        };
        let mut json = serde_json::to_string_pretty(&state).unwrap_or_default();
        json.push('\n');
        write_private(&req.path, json.as_bytes())?;
        let path = req.path.display().to_string();
        let data = StateData {
            path: Some(path.clone()),
            cookies: state.cookies.len(),
            origins: state.origins.iter().map(|o| origin_info(o, None)).collect(),
        };
        let text = format!("saved {path}: {}", summary(&data));
        let output = Output::typed(&data, text).quiet(path.clone()).warning(format!(
            "{path} holds the session's cookies and tokens; it is readable by you only, keep it out of version control"
        ));
        Ok(warnings.into_iter().fold(output, Output::warning))
    }

    /// `state load`.
    pub async fn state_load(&self, req: StateLoadRequest) -> Result<Output, ErrorInfo> {
        let (data, warnings) = self.load_state(&req).await?;
        let from = data.path.clone().unwrap_or_else(|| "the state".to_owned());
        let text = format!("loaded {from}: {}", summary(&data));
        let quiet = data.cookies.to_string();
        let output = Output::typed(&data, text).quiet(quiet);
        Ok(warnings.into_iter().fold(output, Output::warning))
    }

    /// Gives the browser a state's cookies, and each origin its storage:
    /// at once where a frame of it is open, else in its next document
    /// (a seed). Returns what was loaded, and warnings.
    pub(crate) async fn load_state(
        &self,
        req: &StateLoadRequest,
    ) -> Result<(StateData, Vec<String>), ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let running = self.running()?;
        let mut warnings = Vec::new();
        let now = now_secs();
        let (live, gone): (Vec<_>, Vec<_>) = req
            .state
            .cookies
            .iter()
            .partition(|c| c.expires < 0.0 || c.expires > now);
        if !gone.is_empty() {
            warnings.push(format!(
                "{} had expired and {} not loaded",
                count(gone.len(), "cookie"),
                if gone.len() == 1 { "was" } else { "were" }
            ));
        }
        let browser = running.conn.browser().with_timeout(timeout);
        let scope = self.context_key()?;
        let set_cookies = |params: Vec<CookieParam>| {
            let mut set = storage::SetCookiesParams::new(params);
            set.browser_context_id.clone_from(&scope);
            set
        };
        let params: Vec<CookieParam> = live.iter().map(|c| cookie_param(c)).collect();
        let mut loaded = live.len();
        if !params.is_empty() && browser.call(set_cookies(params.clone())).await.is_err() {
            // One bad cookie fails them all: set them one by one.
            for param in params {
                let name = param.name.clone();
                if let Err(e) = browser.call(set_cookies(vec![param])).await {
                    loaded -= 1;
                    warnings.push(format!("Chrome refused cookie {name}: {e}"));
                }
            }
        }
        let open = open_origins(&running, scope.as_deref());
        let mut origins = Vec::new();
        let mut seeded = false;
        for origin in &req.state.origins {
            let name = origin.origin.trim_end_matches('/');
            if origin_of(name).as_deref() != Some(name) {
                warnings.push(format!("`{}` is not an origin; skipped", origin.origin));
                continue;
            }
            let mut payload = json!({ "origin": name, "localStorage": origin.local_storage });
            if !origin.session_storage.is_empty() {
                payload["sessionStorage"] = json!(origin.session_storage);
            }
            if let Some(databases) = &origin.indexed_db {
                payload["indexedDB"] = json!(databases);
            }
            let mut applied = "next-load";
            if let Some((tab, frame, _)) = open.iter().find(|(_, _, o)| o == name) {
                let expression = format!("({LIBRARY}).restore({payload})");
                let result = match running
                    .tabs
                    .context(*tab, frame, World::Isolated, timeout)
                    .await
                {
                    Ok(context) => run(&context, expression, timeout).await,
                    Err(e) => Err(frame_error(&e)),
                };
                match result {
                    Ok(_) => applied = "now",
                    Err(e) => warnings.push(format!(
                        "cannot give the open page of {name} its storage ({e}); its next page load gets it"
                    )),
                }
            }
            if applied != "now" {
                let seed = format!("({LIBRARY}).seed({payload}, {});", json!(SEED_BINDING));
                self.settings.injections.seed(scope.as_deref(), name, &seed);
                seeded = true;
            }
            let mut info = origin_info(origin, Some(applied));
            name.clone_into(&mut info.origin);
            origins.push(info);
        }
        if seeded {
            self.apply_injections(false, timeout).await;
        }
        let data = StateData {
            path: req.path.as_ref().map(|p| p.display().to_string()),
            cookies: loaded,
            origins,
        };
        Ok((data, warnings))
    }
}

/// Every origin with a frame open in `scope` (a context of ours, or the
/// default one), each once: `(tab, frame id, origin)`, the active tab's
/// frames first.
fn open_origins(running: &Running, scope: Option<&str>) -> Vec<(u32, String, String)> {
    let mut tabs = running.tabs.tab_ids_in(scope);
    let active = match scope {
        Some(scope) => running.tabs.active_in(scope),
        None => running.tabs.active(),
    };
    if let Some(active) = active {
        tabs.retain(|&t| t != active);
        tabs.insert(0, active);
    }
    let mut found: Vec<(u32, String, String)> = Vec::new();
    for tab in tabs {
        for frame in running.tabs.frames(tab) {
            if let Some(origin) = origin_of(&frame.url)
                && !found.iter().any(|(_, _, o)| *o == origin)
            {
                found.push((tab, frame.id, origin));
            }
        }
    }
    found
}

/// Runs `expression` in `context` and returns its value (awaited).
async fn run(
    context: &FrameContext,
    expression: String,
    timeout: Duration,
) -> Result<Value, String> {
    let mut params = runtime::EvaluateParams::new(expression);
    params.context_id = Some(context.context_id);
    params.return_by_value = Some(true);
    params.await_promise = Some(true);
    let result = context
        .session
        .with_timeout(timeout)
        .call(params)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(exception) = result.exception_details {
        let message = exception
            .exception
            .as_ref()
            .and_then(|e| e.description.clone())
            .unwrap_or(exception.text);
        return Err(message.lines().next().unwrap_or_default().to_owned());
    }
    Ok(result.result.value.unwrap_or(Value::Null))
}

fn frame_error(err: &FrameError) -> String {
    match err {
        FrameError::Gone => "the frame went away".to_owned(),
        FrameError::NoContext => "the frame has no document yet".to_owned(),
        FrameError::Cdp(e) => e.to_string(),
    }
}

/// Writes `bytes` to `path` (its directories made first), readable and
/// writable by the user only, also when the file was there before.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ErrorInfo> {
    let failed = |e: std::io::Error| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot write {}: {e}", path.display()),
        )
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(failed)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(failed)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(failed)?;
    file.write_all(bytes).map_err(failed)
}

fn origin_info(origin: &OriginState, applied: Option<&str>) -> StateOriginInfo {
    StateOriginInfo {
        origin: origin.origin.clone(),
        local_storage: origin.local_storage.len(),
        session_storage: origin.session_storage.len(),
        indexed_db: origin.indexed_db.as_ref().map(Vec::len),
        applied: applied.map(str::to_owned),
    }
}

/// `2 cookies, 1 origin`, then a line per origin.
fn summary(data: &StateData) -> String {
    let mut text = format!(
        "{}, {}",
        count(data.cookies, "cookie"),
        count(data.origins.len(), "origin")
    );
    let rows: Vec<Vec<String>> = data
        .origins
        .iter()
        .map(|o| {
            let mut what = vec![format!("localStorage {}", o.local_storage)];
            if o.session_storage > 0 {
                what.push(format!("sessionStorage {}", o.session_storage));
            }
            if let Some(n) = o.indexed_db {
                what.push(format!("indexedDB {n}"));
            }
            let mut row = vec![o.origin.clone(), what.join(", ")];
            match o.applied.as_deref() {
                Some("now") => row.push("given to the open page".to_owned()),
                Some(_) => row.push("given to its next page load".to_owned()),
                None => {}
            }
            row
        })
        .collect();
    if !rows.is_empty() {
        let _ = write!(text, "\n{}", format::table(&rows));
    }
    text
}

/// `name=value` (cut), domain and path, expiry, flags.
fn cookie_row(cookie: &CookieInfo) -> Vec<String> {
    let mut pair = format!("{}={}", cookie.name, cookie.value);
    if pair.chars().count() > VALUE_PREVIEW {
        pair = pair.chars().take(VALUE_PREVIEW - 1).collect();
        pair.push('…');
    }
    vec![
        pair,
        format!("{}{}", cookie.domain, cookie.path),
        flags(cookie).join(" "),
    ]
}

/// `HttpOnly`, `Secure`, `SameSite=Lax`, then `session` or `expires …`.
fn flags(cookie: &CookieInfo) -> Vec<String> {
    let mut flags = Vec::new();
    if cookie.http_only {
        flags.push("HttpOnly".to_owned());
    }
    if cookie.secure {
        flags.push("Secure".to_owned());
    }
    if let Some(same_site) = &cookie.same_site {
        flags.push(format!("SameSite={same_site}"));
    }
    if let Some(site) = &cookie.partition_key {
        flags.push(format!("Partitioned({site})"));
    }
    match cookie.expires {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(secs) => {
            let iso = iso8601((secs * 1000.0) as u64);
            // Whole seconds: `2026-10-28T10:00:00Z`.
            let iso = match iso.split_once('.') {
                Some((whole, _)) => format!("{whole}Z"),
                None => iso,
            };
            flags.push(format!("expires {iso}"));
        }
        None => flags.push("session".to_owned()),
    }
    flags
}

/// ` for <url>`, ` of <domain>`, ` named <name>`, as the filter says.
fn filter_text(filter: &CookieFilter) -> String {
    let mut text = String::new();
    if let Some(url) = &filter.url {
        let _ = write!(text, " for {url}");
    }
    if let Some(domain) = &filter.domain {
        let _ = write!(text, " of {domain}");
    }
    if let Some(name) = &filter.name {
        let _ = write!(text, " named {name}");
    }
    text
}

/// Why Chrome may have refused a cookie, as a hint.
fn refusal_hint(spec: &CookieSpec, same_site: Option<network::CookieSameSite>) -> String {
    if same_site == Some(network::CookieSameSite::None) && !spec.secure {
        return "SameSite=None needs --secure".to_owned();
    }
    if spec.name.starts_with("__Host-") {
        return "a __Host- cookie needs --secure, --path / and a --url instead of --domain"
            .to_owned();
    }
    if spec.name.starts_with("__Secure-") && !spec.secure {
        return "a __Secure- cookie needs --secure".to_owned();
    }
    if spec.secure && spec.url.as_deref().is_some_and(|u| u.starts_with("http:")) {
        return "a --secure cookie needs an https --url".to_owned();
    }
    "check the cookie's --domain (it must match the URL's host) and --path".to_owned()
}

/// `storage <area>`, to list the keys.
fn storage_cmd(area: StorageArea, req: &StorageRequest, session: &str) -> Cmd {
    let mut cmd = Cmd::new("storage").arg(area.as_str());
    if let Some(origin) = &req.origin {
        cmd = cmd.flag("origin", origin);
    }
    cmd.scoped(session)
}

fn check_url(url: &str, flag: &str) -> Result<(), ErrorInfo> {
    if origin_of(url).is_some() {
        return Ok(());
    }
    Err(ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("{flag} `{url}` is not an http or https URL"),
    ))
}

/// `1 cookie`, `2 cookies`.
fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{} {what}s", format::thousands(n))
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}
