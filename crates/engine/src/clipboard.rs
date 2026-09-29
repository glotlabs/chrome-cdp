//! `clipboard read` and `clipboard write <text>` (plan 05, milestone 5.4).
//!
//! The page's own async clipboard API (`navigator.clipboard`) does the
//! work, in chrome-cdp's isolated world of the tab's main frame, so the
//! text is what a page's paste reads and a `Control+V` pastes. The API
//! needs two things a user gives and an automated tab lacks: permission
//! (granted to the page's origin with `Browser.setPermission`, and kept,
//! as a user's "allow" is) and a focused document (emulated with
//! `Emulation.setFocusEmulationEnabled` for the call only). The grants
//! are the session's, so plan 07's `permissions` lists them.

use cdp::domains::browser::{PermissionDescriptor, PermissionSetting, SetPermissionParams};
use cdp::domains::emulation::SetFocusEmulationEnabledParams;
use cdp::domains::runtime;
use protocol::schema::{ClipboardReadData, ClipboardWriteData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::request::{ClipboardOp, ClipboardRequest, millis};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

/// The permissions the async clipboard API asks for.
const PERMISSIONS: [&str; 2] = ["clipboard-read", "clipboard-write"];

/// Where the page is, as the clipboard API sees it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageOrigin {
    origin: String,
    secure: bool,
    url: String,
}

impl Engine {
    /// Reads or writes the clipboard's text through the page in the tab
    /// `req` names, holding the tab's lock.
    pub async fn clipboard(&self, req: ClipboardRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let work = self.use_clipboard(&running, &page, &req);
        let output = self.unless_blocked(&running, &page, mark, work).await?;
        Ok(self.report_events(&running, mark, output, None).await)
    }

    async fn use_clipboard(
        &self,
        running: &Running,
        page: &Page,
        req: &ClipboardRequest,
    ) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let context = self
            .frame_context(running, page, None, World::Isolated, timeout)
            .await?;
        let session = context.session.with_timeout(timeout);
        let call = |function: &str, arguments: Vec<Value>| {
            let mut params = runtime::CallFunctionOnParams::new(function.to_owned());
            params.execution_context_id = Some(context.context_id);
            params.arguments = Some(
                arguments
                    .into_iter()
                    .map(|value| runtime::CallArgument {
                        value: Some(value),
                        unserializable_value: None,
                        object_id: None,
                    })
                    .collect(),
            );
            params.return_by_value = Some(true);
            params.await_promise = Some(true);
            session.call(params)
        };
        let origin = call(
            "function () { return { origin: location.origin, secure: isSecureContext, url: location.href }; }",
            Vec::new(),
        )
        .await
        .map_err(|e| from_cdp(&e, &self.session))?;
        let origin: PageOrigin = origin
            .result
            .value
            .and_then(|v| serde_json::from_value(v).ok())
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    "reading the page's origin gave nothing",
                )
            })?;
        if origin.origin == "null" || !origin.secure {
            return Err(self.no_clipboard(page, &origin));
        }
        let browser = running.conn.browser().with_timeout(timeout);
        // The page is of the call's context, whose permissions they are.
        let scope = self.context_key()?;
        for name in PERMISSIONS {
            let mut params = SetPermissionParams::new(
                PermissionDescriptor {
                    name: name.to_owned(),
                    sysex: None,
                    user_visible_only: None,
                    allow_without_sanitization: None,
                    allow_without_gesture: None,
                    pan_tilt_zoom: None,
                },
                PermissionSetting::Granted,
            );
            params.origin = Some(origin.origin.clone());
            params.browser_context_id.clone_from(&scope);
            browser
                .call(params)
                .await
                .map_err(|e| from_cdp(&e, &self.session))?;
            // `permissions` lists it, and can take it back.
            self.settings.emulation.record_permission(
                scope.as_deref(),
                name,
                "granted",
                Some(&origin.origin),
            );
        }
        let page_session = page.session.with_timeout(timeout);
        page_session
            .call(SetFocusEmulationEnabledParams::new(true))
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let result = match &req.op {
            ClipboardOp::Read => {
                call(
                    "function () { return navigator.clipboard.readText(); }",
                    Vec::new(),
                )
                .await
            }
            ClipboardOp::Write { text } => {
                call(
                    "function (text) { return navigator.clipboard.writeText(text); }",
                    vec![json!(text)],
                )
                .await
            }
        };
        // Focus goes back to what it really is (a later click may give
        // the page focus, as it does a user's).
        let _ = page_session
            .call(SetFocusEmulationEnabledParams::new(false))
            .await;
        let result = result.map_err(|e| from_cdp(&e, &self.session))?;
        let verb = match req.op {
            ClipboardOp::Read => "read",
            ClipboardOp::Write { .. } => "write",
        };
        if let Some(exception) = result.exception_details {
            let message = exception
                .exception
                .and_then(|e| e.description)
                .unwrap_or(exception.text);
            let first = message.lines().next().unwrap_or_default();
            return Err(ErrorInfo::new(
                ErrorKind::NotEnabled,
                format!("the page could not {verb} the clipboard: {first}"),
            )
            .note("page", origin.url)
            .hint(Hint::text(
                "the page may block the clipboard (a Permissions-Policy header); \
                 use press Mod+C / Mod+V in a field instead",
            )));
        }
        let tab = tab_name(page.id);
        Ok(match &req.op {
            ClipboardOp::Read => {
                let text = result
                    .result
                    .value
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let mut output = Output::typed(
                    &ClipboardReadData {
                        tab,
                        text: text.clone(),
                    },
                    text.clone(),
                )
                .quiet(text.clone())
                .limit(req.max_chars, "--max-chars 0 for all of it");
                if text.is_empty() {
                    output = output.warning("the clipboard holds no text");
                }
                output
            }
            ClipboardOp::Write { text } => {
                let chars = text.chars().count();
                let plural = if chars == 1 { "" } else { "s" };
                Output::typed(
                    &ClipboardWriteData { tab, chars },
                    format!("wrote {chars} character{plural} to the clipboard"),
                )
                .quiet("")
            }
        })
    }

    /// The error for a page the clipboard API is not available to.
    fn no_clipboard(&self, page: &Page, origin: &PageOrigin) -> ErrorInfo {
        let why = if origin.origin == "null" {
            "it has no origin"
        } else {
            "it is not a secure context (https, or http on localhost)"
        };
        ErrorInfo::new(
            ErrorKind::NotEnabled,
            format!(
                "the page in tab {} cannot use the clipboard: {why}",
                tab_name(page.id)
            ),
        )
        .note("page", origin.url.clone())
        .hint(
            Hint::run(Cmd::new("goto").raw("<https-url>").scoped(&self.session))
                .why("the clipboard works through the page"),
        )
        .details(json!({ "url": origin.url, "origin": origin.origin }))
    }
}
