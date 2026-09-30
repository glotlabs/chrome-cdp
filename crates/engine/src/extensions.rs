//! Unpacked extensions: `open --extension` (plan 07, milestone 7.5) and
//! `extensions` (plan 14, milestone 14.4).
//!
//! They are loaded over the pipe with `Extensions.loadUnpacked` right after
//! the launch, before any URL, rather than with `--load-extension`: Google
//! Chrome 137 and later ignore that flag, and a broken manifest there is a
//! silent no-op, where `loadUnpacked` says what is wrong.
//!
//! `chrome.storage` is only reached through a session of one of the
//! extension's own targets (Chrome answers "Extension not found" to any
//! other): its service worker, else a tab showing one of its pages, else a
//! tab opened on its `manifest.json` for the call.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cdp::domains::extensions as cdp_extensions;
use cdp::domains::extensions::StorageArea;
use cdp::domains::target;
use cdp::{CdpError, Connection, Session};
use protocol::schema::{
    BrowserKind, ExtensionInfo, ExtensionPopupData, ExtensionStorageData, ExtensionsData,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Map, Value, json};

use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::request::{
    ExtensionStorageArea, ExtensionStorageOp, ExtensionsOp, ExtensionsRequest, millis,
};
use crate::tabs::tab_name;
use crate::{Engine, Running};

/// How long `popup` waits for the popup of an extension whose manifest
/// could not be read (a connected browser's files may be elsewhere).
const POPUP_WAIT: Duration = Duration::from_secs(2);

/// Loads each extension in `dirs` (absolute paths) into the browser behind
/// `conn`, in order, and describes them.
pub(crate) async fn load(
    conn: &Connection,
    dirs: &[PathBuf],
    session: &str,
) -> Result<Vec<ExtensionInfo>, ErrorInfo> {
    let browser = conn.browser();
    let mut ids = Vec::with_capacity(dirs.len());
    for dir in dirs {
        let loaded = browser
            .call(cdp_extensions::LoadUnpackedParams::new(
                dir.display().to_string(),
            ))
            .await
            .map_err(|e| load_failed(dir, &e, session))?;
        ids.push(loaded.id);
    }
    // The name and version as Chrome read them (a localized name resolved).
    let known = browser
        .call(cdp_extensions::GetExtensionsParams::default())
        .await
        .map(|r| r.extensions)
        .unwrap_or_default();
    Ok(ids
        .into_iter()
        .zip(dirs)
        .map(|(id, dir)| {
            let found = known.iter().find(|e| e.id == id);
            ExtensionInfo {
                name: found.map(|e| e.name.clone()).unwrap_or_default(),
                version: found.map(|e| e.version.clone()).unwrap_or_default(),
                path: dir.display().to_string(),
                id,
            }
        })
        .collect())
}

fn load_failed(dir: &Path, e: &CdpError, session: &str) -> ErrorInfo {
    match e {
        CdpError::Protocol { code: -32601, .. } => ErrorInfo::new(
            ErrorKind::LaunchFailed,
            "this browser cannot load extensions over CDP (Extensions.loadUnpacked is missing)",
        )
        .hint(Hint::text(
            "use Chrome 126 or later, e.g. Chrome for Testing from `chrome-cdp install`",
        )),
        CdpError::Protocol { message, .. } => ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot load the extension in {}: {message}", dir.display()),
        )
        .hint(Hint::text(
            "fix what Chrome names in the extension's files, then open again",
        )),
        _ => from_cdp(e, session),
    }
}

/// How `extensions` shows an extension Chrome reports.
fn info(e: &cdp_extensions::ExtensionInfo) -> ExtensionInfo {
    ExtensionInfo {
        id: e.id.clone(),
        name: e.name.clone(),
        version: e.version.clone(),
        path: e.path.clone(),
    }
}

/// `name 1.2.3, chrome-extension://<id>/ (/path)`, as `open` prints it.
fn line(e: &ExtensionInfo) -> String {
    format!(
        "{} {}, chrome-extension://{}/ ({})",
        e.name, e.version, e.id, e.path
    )
}

fn area_param(area: ExtensionStorageArea) -> StorageArea {
    match area {
        ExtensionStorageArea::Local => StorageArea::Local,
        ExtensionStorageArea::Sync => StorageArea::Sync,
        ExtensionStorageArea::Session => StorageArea::Session,
        ExtensionStorageArea::Managed => StorageArea::Managed,
    }
}

/// Whether the extension in `dir` has an action popup, from its manifest;
/// `None` when it cannot be read.
fn has_popup(dir: &str) -> Option<bool> {
    let text = std::fs::read_to_string(Path::new(dir).join("manifest.json")).ok()?;
    let manifest: Value = serde_json::from_str(&text).ok()?;
    Some(
        ["action", "browser_action", "page_action"]
            .iter()
            .any(|key| {
                manifest[key]["default_popup"]
                    .as_str()
                    .is_some_and(|p| !p.is_empty())
            }),
    )
}

impl Engine {
    /// `extensions` and its subcommands.
    pub async fn extensions(&self, req: ExtensionsRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let browser = running.conn.browser().with_timeout(timeout);
        let output = match &req.op {
            ExtensionsOp::List => {
                let extensions = self.extensions_loaded(&running, &browser).await?;
                let text = if extensions.is_empty() {
                    format!(
                        "no extensions loaded; load one: {}",
                        Cmd::new("extensions")
                            .arg("load")
                            .raw("DIR")
                            .scoped(&self.session)
                    )
                } else {
                    let rows: Vec<Vec<&str>> = extensions
                        .iter()
                        .map(|e| vec![e.id.as_str(), &e.name, &e.version, &e.path])
                        .collect();
                    format::table(&rows)
                };
                let quiet: Vec<String> = extensions.iter().map(|e| e.id.clone()).collect();
                let data = ExtensionsData {
                    extension: None,
                    extensions,
                };
                Output::typed(&data, text).quiet(quiet.join("\n"))
            }
            ExtensionsOp::Load { dir } => {
                let loaded = browser
                    .call(cdp_extensions::LoadUnpackedParams::new(
                        dir.display().to_string(),
                    ))
                    .await
                    .map_err(|e| self.cannot_load(&running, dir, &e))?;
                self.extension_changed(&running, &browser, &loaded.id, "loaded")
                    .await?
            }
            ExtensionsOp::Reload { extension } => {
                let found = self.find_extension(&running, &browser, extension).await?;
                // Loading an unpacked extension's directory again reloads
                // it with the same id, and keeps its `chrome.storage`, as
                // the Reload button does.
                let dir = PathBuf::from(&found.path);
                let loaded = browser
                    .call(cdp_extensions::LoadUnpackedParams::new(found.path.clone()))
                    .await
                    .map_err(|e| self.cannot_load(&running, &dir, &e))?;
                self.extension_changed(&running, &browser, &loaded.id, "reloaded")
                    .await?
            }
            ExtensionsOp::Uninstall { extension } => {
                let found = self.find_extension(&running, &browser, extension).await?;
                browser
                    .call(cdp_extensions::UninstallParams::new(found.id.clone()))
                    .await
                    .map_err(|e| self.extensions_error(&running, &e))?;
                let extensions = self.extensions_loaded(&running, &browser).await?;
                let text = format!("uninstalled {} ({})", found.name, found.id);
                let data = ExtensionsData {
                    extension: Some(found.clone()),
                    extensions,
                };
                Output::typed(&data, text).quiet(found.id)
            }
            ExtensionsOp::Popup { extension } => {
                let found = self.find_extension(&running, &browser, extension).await?;
                return self
                    .extension_popup(&running, &found, req.tab.as_deref(), timeout)
                    .await;
            }
            ExtensionsOp::Storage {
                extension,
                area,
                op,
            } => {
                let found = self.find_extension(&running, &browser, extension).await?;
                self.extension_storage(&running, &found, *area, op, timeout)
                    .await?
            }
        };
        Ok(output.limit(req.max_chars, "--max-chars 0 for all of it"))
    }

    /// The unpacked extensions the browser has loaded.
    async fn extensions_loaded(
        &self,
        running: &Running,
        browser: &Session,
    ) -> Result<Vec<ExtensionInfo>, ErrorInfo> {
        let list = browser
            .call(cdp_extensions::GetExtensionsParams::default())
            .await
            .map_err(|e| self.extensions_error(running, &e))?;
        Ok(list.extensions.iter().map(info).collect())
    }

    /// The extension `wanted` names: its id, else its name (exactly, then
    /// ignoring case).
    async fn find_extension(
        &self,
        running: &Running,
        browser: &Session,
        wanted: &str,
    ) -> Result<ExtensionInfo, ErrorInfo> {
        let extensions = self.extensions_loaded(running, browser).await?;
        if let Some(e) = extensions.iter().find(|e| e.id == wanted) {
            return Ok(e.clone());
        }
        let mut named: Vec<&ExtensionInfo> =
            extensions.iter().filter(|e| e.name == wanted).collect();
        if named.is_empty() {
            named = extensions
                .iter()
                .filter(|e| e.name.eq_ignore_ascii_case(wanted))
                .collect();
        }
        match named.as_slice() {
            [one] => Ok((*one).clone()),
            [] => {
                let mut error = ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("no extension {wanted} in session {}", self.session),
                );
                if extensions.is_empty() {
                    error = error.note("loaded", "none").hint(Hint::run(
                        Cmd::new("extensions")
                            .arg("load")
                            .raw("DIR")
                            .scoped(&self.session),
                    ));
                } else {
                    let loaded: Vec<String> = extensions
                        .iter()
                        .map(|e| format!("{} ({})", e.name, e.id))
                        .collect();
                    error = error.note("loaded", loaded.join(", ")).hint(
                        Hint::run(Cmd::new("extensions").scoped(&self.session))
                            .why("lists them with their ids"),
                    );
                }
                Err(error.details(json!({ "extension": wanted, "loaded": extensions })))
            }
            several => {
                let ids: Vec<&str> = several.iter().map(|e| e.id.as_str()).collect();
                Err(ErrorInfo::new(
                    ErrorKind::Ambiguous,
                    format!("{} extensions are named {wanted}", several.len()),
                )
                .note("ids", ids.join(", "))
                .hint(
                    Hint::run(Cmd::new("extensions").scoped(&self.session))
                        .why("name one by its id"),
                )
                .details(json!({ "extension": wanted, "ids": ids })))
            }
        }
    }

    /// The output of `load` and `reload`: the extension with id `id` as
    /// Chrome read it, and every loaded one.
    async fn extension_changed(
        &self,
        running: &Running,
        browser: &Session,
        id: &str,
        verb: &str,
    ) -> Result<Output, ErrorInfo> {
        let extensions = self.extensions_loaded(running, browser).await?;
        let extension = extensions
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .unwrap_or_else(|| ExtensionInfo {
                id: id.to_owned(),
                name: String::new(),
                version: String::new(),
                path: String::new(),
            });
        let text = format!("{verb} {}", line(&extension));
        let data = ExtensionsData {
            extension: Some(extension),
            extensions,
        };
        Ok(Output::typed(&data, text).quiet(id))
    }

    /// `load` or `reload` failed: Chrome names what is wrong with the
    /// extension, or does not let this browser load extensions.
    fn cannot_load(&self, running: &Running, dir: &Path, e: &CdpError) -> ErrorInfo {
        match e {
            CdpError::Protocol { code, message, .. } if !unavailable(*code, message) => {
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("cannot load the extension in {}: {message}", dir.display()),
                )
                .hint(
                    Hint::run(
                        Cmd::new("extensions")
                            .arg("load")
                            .arg(dir.display().to_string())
                            .scoped(&self.session),
                    )
                    .why("after fixing what Chrome names in the extension's files"),
                )
            }
            _ => self.extensions_error(running, e),
        }
    }

    /// An `Extensions` call failed: the browser does not allow extension
    /// debugging, or some other error.
    fn extensions_error(&self, running: &Running, e: &CdpError) -> ErrorInfo {
        let CdpError::Protocol { code, message, .. } = e else {
            return from_cdp(e, &self.session);
        };
        if !unavailable(*code, message) {
            return from_cdp(e, &self.session);
        }
        let error = ErrorInfo::new(
            ErrorKind::BadArgs,
            "this browser was started without extension debugging",
        )
        .note("chrome", message.clone());
        match running.info.kind {
            BrowserKind::Launched => error.hint(
                Hint::run(
                    Cmd::new("open")
                        .switch("restart")
                        .flag("extension", "DIR")
                        .session(&self.session),
                )
                .why("starts Chrome with it and loads DIR"),
            ),
            BrowserKind::Connected => error.hint(Hint::text(
                "start Chrome with --enable-unsafe-extension-debugging and connect again",
            )),
        }
    }

    /// `extensions popup`: runs the extension's action on the tab, and
    /// waits for its popup if it has one.
    async fn extension_popup(
        &self,
        running: &Running,
        extension: &ExtensionInfo,
        tab: Option<&str>,
        timeout: Duration,
    ) -> Result<Output, ErrorInfo> {
        let page = self.page(running, tab, timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let tab_target = tab_target_of(&running.conn, &page.target_id, timeout)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    format!(
                        "Chrome did not say which tab holds {}'s page",
                        tab_name(page.id)
                    ),
                )
                .hint(Hint::run(Cmd::new("tabs").scoped(&self.session)))
            })?;
        let before = running.tabs.tab_ids();
        let mark = running.tabs.mark();
        // An id Chrome does not know crashes it (Chrome 154), so it is
        // always one `find` got from Chrome.
        running
            .conn
            .browser()
            .with_timeout(timeout)
            .call(cdp_extensions::TriggerActionParams::new(
                extension.id.clone(),
                tab_target,
            ))
            .await
            .map_err(|e| self.extensions_error(running, &e))?;
        let expected = has_popup(&extension.path);
        let wait = match expected {
            Some(true) => timeout,
            Some(false) => Duration::from_millis(300),
            None => POPUP_WAIT.min(timeout),
        };
        let popup = running
            .tabs
            .wait_for_extension_tab(&extension.id, &before, wait)
            .await;
        if popup.is_none() && expected == Some(true) {
            return Err(ErrorInfo::new(
                ErrorKind::Timeout,
                format!(
                    "the popup of {} did not open within {}",
                    extension.name,
                    format::duration(timeout)
                ),
            )
            .hint(Hint::run(Cmd::new("tabs").scoped(&self.session)))
            .details(json!({ "extension": extension.id, "tab": tab_name(page.id) })));
        }
        let popup = popup.and_then(|id| running.tabs.info(id));
        let (text, quiet) = match &popup {
            Some(popup) => (
                format!(
                    "opened the popup of {} in tab {} ({}); switch to it: {}",
                    extension.name,
                    popup.id,
                    popup.url,
                    Cmd::new("tab").arg(&popup.id).scoped(&self.session)
                ),
                popup.id.clone(),
            ),
            None => (
                format!(
                    "ran the action of {} on tab {}; it opened no popup",
                    extension.name,
                    tab_name(page.id)
                ),
                tab_name(page.id),
            ),
        };
        let data = ExtensionPopupData {
            extension: extension.clone(),
            tab: tab_name(page.id),
            popup: popup.as_ref().map(|p| p.id.clone()),
            url: popup.as_ref().map(|p| p.url.clone()),
        };
        let mut output = Output::typed(&data, text).quiet(quiet);
        for event in self.events_since(running, mark, None).await {
            let about_popup = matches!(&event, protocol::schema::Event::TabOpened { tab, .. }
                if popup.as_ref().is_some_and(|p| &p.id == tab));
            if !about_popup {
                output = output.warning(self.event_line(&event));
            }
            output = output.event(event);
        }
        Ok(output)
    }

    /// `extensions storage`.
    async fn extension_storage(
        &self,
        running: &Running,
        extension: &ExtensionInfo,
        area: ExtensionStorageArea,
        op: &ExtensionStorageOp,
        timeout: Duration,
    ) -> Result<Output, ErrorInfo> {
        let writes = !matches!(op, ExtensionStorageOp::Get { .. });
        if writes && area == ExtensionStorageArea::Managed {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "the managed storage area is read-only (policies set it)",
            )
            .hint(
                Hint::run(
                    Cmd::new("extensions")
                        .arg("storage")
                        .arg(&extension.name)
                        .flag("area", "managed")
                        .scoped(&self.session),
                )
                .why("reads it"),
            ));
        }
        let (session, opened) = self
            .extension_storage_session(running, extension, timeout)
            .await?;
        let result = self
            .use_extension_storage(&session, extension, area, op, timeout)
            .await;
        if let Some(id) = opened {
            let _ = running.tabs.close(id, timeout).await;
        }
        let items = result.map_err(|e| match &e {
            CdpError::Protocol { message, .. } if message.contains("read-only") => ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("the {} storage area is read-only", area.name()),
            ),
            _ => self.extensions_error(running, &e),
        })?;
        let name = &extension.name;
        let area_name = area.name();
        let text = match op {
            ExtensionStorageOp::Get { .. } => {
                serde_json::to_string_pretty(&items).unwrap_or_default()
            }
            ExtensionStorageOp::Set { values } => {
                let keys: Vec<&str> = values.keys().map(String::as_str).collect();
                format!(
                    "set {} in the {area_name} storage of {name}",
                    keys.join(", ")
                )
            }
            ExtensionStorageOp::Remove { keys } => format!(
                "removed {} from the {area_name} storage of {name}",
                keys.join(", ")
            ),
            ExtensionStorageOp::Clear => format!("cleared the {area_name} storage of {name}"),
        };
        let mut text = text;
        if writes {
            let _ = write!(
                text,
                "; it holds {} item{}",
                items.len(),
                if items.len() == 1 { "" } else { "s" }
            );
        }
        let quiet = serde_json::to_string(&items).unwrap_or_default();
        let data = ExtensionStorageData {
            extension: extension.clone(),
            area: area_name.to_owned(),
            items,
        };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// A session `chrome.storage` calls work in: the extension's service
    /// worker's, else that of a tab showing one of its pages, else that of
    /// a tab opened on its `manifest.json` (its id, to close after).
    async fn extension_storage_session(
        &self,
        running: &Running,
        extension: &ExtensionInfo,
        timeout: Duration,
    ) -> Result<(Session, Option<u32>), ErrorInfo> {
        if let Some(worker) = running.tabs.extension_worker(&extension.id) {
            return Ok((worker, None));
        }
        let tabs = running.tabs.extension_tabs(&extension.id);
        if let Some(page) = match tabs.first() {
            Some(&id) => running.tabs.page(Some(id), timeout).await.ok(),
            None => None,
        } {
            return Ok((page.session, None));
        }
        let url = format!("chrome-extension://{}/manifest.json", extension.id);
        let page = running
            .tabs
            .create(&url, timeout)
            .await
            .map_err(|e| self.tab_error(running, e))?;
        Ok((page.session, Some(page.id)))
    }

    /// Does `op` in `session` and reads the area back: the items of the
    /// keys `get` asked for, else all of them.
    async fn use_extension_storage(
        &self,
        session: &Session,
        extension: &ExtensionInfo,
        area: ExtensionStorageArea,
        op: &ExtensionStorageOp,
        timeout: Duration,
    ) -> Result<Map<String, Value>, CdpError> {
        let session = session.with_timeout(timeout);
        let id = extension.id.clone();
        let storage_area = area_param(area);
        let mut keys = None;
        match op {
            ExtensionStorageOp::Get { keys: wanted } => {
                keys = (!wanted.is_empty()).then(|| wanted.clone());
            }
            ExtensionStorageOp::Set { values } => {
                session
                    .call(cdp_extensions::SetStorageItemsParams {
                        id: id.clone(),
                        storage_area,
                        values: values.clone(),
                    })
                    .await?;
            }
            ExtensionStorageOp::Remove { keys } => {
                session
                    .call(cdp_extensions::RemoveStorageItemsParams {
                        id: id.clone(),
                        storage_area,
                        keys: keys.clone(),
                    })
                    .await?;
            }
            ExtensionStorageOp::Clear => {
                session
                    .call(cdp_extensions::ClearStorageItemsParams {
                        id: id.clone(),
                        storage_area,
                    })
                    .await?;
            }
        }
        let got = session
            .call(cdp_extensions::GetStorageItemsParams {
                id,
                storage_area,
                keys,
            })
            .await?;
        Ok(got.data)
    }
}

/// Whether an `Extensions` call failed because the browser does not let
/// this client use the domain (no `--enable-unsafe-extension-debugging`).
fn unavailable(code: i64, message: &str) -> bool {
    code == -32601 || message.contains("not available") || message.contains("not allowed")
}

/// Chrome's id of the `tab` target that holds page target `page`, which
/// `Extensions.triggerAction` needs. Chrome does not link the two in
/// `getTargets`, so each tab target is attached to in turn, its page found
/// by auto-attach, and detached from.
async fn tab_target_of(
    conn: &Connection,
    page: &str,
    timeout: Duration,
) -> cdp::Result<Option<String>> {
    let browser = conn.browser().with_timeout(timeout);
    let tabs = browser
        .call(target::GetTargetsParams {
            filter: Some(vec![target::FilterEntry {
                exclude: None,
                r#type: Some("tab".to_owned()),
            }]),
        })
        .await?
        .target_infos;
    // The page's own URL first: usually the first attach finds it.
    let url = browser
        .call(target::GetTargetInfoParams {
            target_id: Some(page.to_owned()),
        })
        .await?
        .target_info
        .url;
    let mut tabs: Vec<target::TargetInfo> =
        tabs.into_iter().filter(|t| t.r#type == "tab").collect();
    tabs.sort_by_key(|t| t.url != url);
    for tab in tabs {
        let Ok(session) = conn.attach(tab.target_id.clone()).await else {
            continue;
        };
        let session = session.with_timeout(timeout);
        let mut attached = session.listen::<target::AttachedToTarget>();
        let mut params = target::SetAutoAttachParams::new(true, false);
        params.flatten = Some(true);
        let found = if session.call(params).await.is_ok() {
            tokio::time::timeout(Duration::from_secs(1), attached.next())
                .await
                .ok()
                .flatten()
                .is_some_and(|a| a.target_info.target_id == page)
        } else {
            false
        };
        let _ = browser
            .call(target::DetachFromTargetParams {
                session_id: session.id().map(str::to_owned),
                target_id: None,
            })
            .await;
        if found {
            return Ok(Some(tab.target_id));
        }
    }
    Ok(None)
}
