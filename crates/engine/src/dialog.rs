//! `dialog accept [text]` and `dialog dismiss` (plan 03, milestone 3.5):
//! answers the JavaScript dialog a tab waits on (see
//! [`crate::tabs::dialogs`]).
//!
//! It takes no tab lock: the command that ran into the dialog has
//! returned, but a raw `cdp` call may still wait on the page, and the
//! answer is what lets it go on. It does not wait for what the answer
//! causes (a navigation, a request); `wait` does that.

use std::fmt::Write as _;

use cdp::domains::page;
use protocol::schema::DialogData;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde_json::json;

use crate::Engine;
use crate::errors::from_cdp;
use crate::interrupt::describe;
use crate::request::{DialogRequest, millis};
use crate::tabs::tab_name;

impl Engine {
    /// `dialog accept|dismiss`: answers the dialog of the active tab (or
    /// `--tab`).
    pub async fn dialog(&self, req: DialogRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        let Some(open) = running.tabs.dialog(page.id) else {
            return Err(self.no_dialog(&running, page.id));
        };
        if req.text.is_some() && !(req.accept && open.info.kind == "prompt") {
            let which = if req.accept {
                format!("a {}", open.info.kind)
            } else {
                "dismiss".to_owned()
            };
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("only a prompt takes text, not {which}"),
            )
            .note("dialog", describe(&open.info))
            .hint(Hint::run(
                Cmd::new("dialog")
                    .arg(if req.accept { "accept" } else { "dismiss" })
                    .session(&self.session),
            )));
        }
        let mark = running.tabs.mark();
        let Some(open) = running.tabs.take_dialog(page.id) else {
            return Err(self.no_dialog(&running, page.id));
        };
        let prompt_text = if req.accept && open.info.kind == "prompt" {
            req.text
                .clone()
                .or_else(|| open.info.default_prompt.clone())
        } else {
            None
        };
        let mut params = page::HandleJavaScriptDialogParams::new(req.accept);
        params.prompt_text.clone_from(&prompt_text);
        open.session
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let verb = if req.accept { "accepted" } else { "dismissed" };
        let mut text = format!("{verb} {}", describe(&open.info));
        if let Some(answer) = &prompt_text {
            let _ = write!(text, " with {answer:?}");
        }
        let data = DialogData {
            tab: tab_name(page.id),
            dialog: open.info,
            accepted: req.accept,
            prompt_text,
        };
        let output = Output::typed(&data, text).quiet(verb);
        Ok(self.report_events(&running, mark, output, None).await)
    }

    /// `NotFound`: tab `id` has no dialog; names the tabs that do.
    fn no_dialog(&self, running: &crate::Running, id: u32) -> ErrorInfo {
        let others = running.tabs.dialogs();
        let mut info = ErrorInfo::new(
            ErrorKind::NotFound,
            format!("no dialog is open in tab {}", tab_name(id)),
        );
        for (other, dialog) in &others {
            info = info.note(
                "dialog open",
                format!("tab {}: {}", tab_name(*other), describe(dialog)),
            );
        }
        let hint = match others.first() {
            Some((other, _)) => Hint::run(
                Cmd::new("dialog")
                    .arg("accept")
                    .flag("tab", tab_name(*other))
                    .session(&self.session),
            )
            .or(format!(
                "{}",
                Cmd::new("dialog")
                    .arg("dismiss")
                    .flag("tab", tab_name(*other))
                    .session(&self.session)
            )),
            None => Hint::run(Cmd::new("tabs").session(&self.session))
                .why("shows each tab's open dialog, if any"),
        };
        let open: Vec<String> = others.iter().map(|(id, _)| tab_name(*id)).collect();
        info.hint(hint)
            .details(json!({ "tab": tab_name(id), "open_in": open }))
    }
}
